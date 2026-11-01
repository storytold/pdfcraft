use super::build::{MAX_INDEX, build_children, fresh_datasets};
use super::merge::{Op, Within, merge_all, readable};
use super::path::path_som;
use super::values::{FieldDatum, Want, wanted, with_template};
use super::{DataOp, DataPath, Dict, Document, Encoding, HashMap, ObjRef, Object, PdfString, Stream, Template, XfaError, encode};
use crate::packets::{MAX_XDP, encoded_len, packet_entries, validate_write_encoding};

/// What [`write_datasets`] did.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct DatasetsWrite {
    /// The datasets packet changed (a new revision of it was added to the document).
    pub written: bool,
    /// Values that could not be written, and why.
    pub warnings: Vec<String>,
    /// The datasets stream written, when one was.
    pub stream: Option<ObjRef>,
}

/// A packet stream's text and encoding; `None` (with a warning) when it can't be read exactly.
/// The declaration must also name a supported, consistent encoding the writer can preserve.
fn packet_text(doc: &Document, o: &Object, warnings: &mut Vec<String>) -> Option<(String, Encoding)> {
    let s = doc.resolve(o);
    let Object::Stream(s) = &*s else {
        warnings.push("The XFA data packet is not a stream; the field values were not written to it".into());
        return None;
    };
    let bytes = match s.decoded_within(MAX_XDP) {
        Ok(b) => b,
        Err(e) => {
            warnings.push(format!("The XFA data could not be decoded ({e}); the field values were not written to it"));
            return None;
        }
    };
    let decoded = crate::packets::decode(&bytes);
    if let Some((text, _)) = &decoded
        && let Err(e) = validate_write_encoding(&bytes, text)
    {
        warnings.push(format!("The XFA data was not written: {e}"));
        return None;
    }
    if decoded.is_none() {
        warnings.push("The XFA data is neither UTF-8 nor UTF-16; the field values were not written to it, so it stays as it was".into());
    }
    decoded
}

/// Write the fields' values into the datasets packet, keeping all the data no field binds to.
/// Nothing is written when the document has no XFA entry or the data already holds the values;
/// values that can't be written are reported, never silently dropped.
pub fn write_datasets(doc: &mut Document, fields: &[FieldDatum]) -> Result<DatasetsWrite, XfaError> {
    write_datasets_reusing(doc, fields, None)
}

/// [`write_datasets`], replacing the stream `reuse` in place when the packet is that stream
/// (written earlier in the same edit) rather than adding another.
pub fn write_datasets_reusing(doc: &mut Document, fields: &[FieldDatum], reuse: Option<ObjRef>) -> Result<DatasetsWrite, XfaError> {
    write_datasets_with(doc, None, fields, reuse)
}

/// [`write_datasets_reusing`] with the form's parsed template when the caller has it (it says
/// which SOM segments are areas, see [`data_path`]); `None` parses the document's.
pub fn write_datasets_with(
    doc: &mut Document,
    tpl: Option<&Template>,
    fields: &[FieldDatum],
    reuse: Option<ObjRef>,
) -> Result<DatasetsWrite, XfaError> {
    let wants = with_template(doc, tpl, |t| wanted(doc, fields, t));
    rewrite(doc, &[Op::Values(&wants)], reuse)
}

/// Make sure the data has at least `count` instances of `name` under `parent` (a script added
/// rows: the layout repeats the subform once per data instance).
pub fn add_data_instances(doc: &mut Document, parent: &DataPath, name: &str, count: usize) -> Result<DatasetsWrite, XfaError> {
    write_data_ops(doc, &[DataOp::Instances { parent: parent.clone(), name: name.to_string(), count }], None)
}

/// Put `text` at the data node `path` (a field with no widget, a hidden one or a draw, that a
/// script gave a value), adding the node as needed.
pub fn write_data_value(doc: &mut Document, path: &DataPath, text: &str) -> Result<DatasetsWrite, XfaError> {
    write_data_ops(doc, &[DataOp::Value { path: path.clone(), text: text.to_string() }], None)
}

/// Remove the data node at `path` (a script removed a row).
pub fn remove_data_instance(doc: &mut Document, path: &DataPath) -> Result<DatasetsWrite, XfaError> {
    write_data_ops(doc, &[DataOp::Remove(path.clone())], None)
}

/// Most instance nodes one [`write_data_ops`] queues over all its passes (a removal starts a new
/// pass, which asks for the instances again).
const MAX_ENSURED: usize = 200_000;

// Instance nodes write_data_ops has queued on this thread, for tests that bound its work.
#[cfg(test)]
thread_local! {
    pub(crate) static ENSURED: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

/// Apply `ops` to the datasets packet, in order, as one rewrite of it: everything one script
/// event changed becomes a single new datasets stream. When the packet is already the stream
/// `reuse` (written earlier for the same event), that stream is replaced in place instead of
/// a new one being added. The stream written is in the report.
pub fn write_data_ops(doc: &mut Document, ops: &[DataOp], reuse: Option<ObjRef>) -> Result<DatasetsWrite, XfaError> {
    // Runs of values (and instances) merge in one pass; a value set twice keeps the last.
    let mut steps: Vec<Vec<Want>> = Vec::new();
    let mut removes: Vec<Option<DataPath>> = Vec::new();
    let mut run: Vec<Want> = Vec::new();
    // Instances queued in this run, by parent and name: asking for more only queues the rest.
    let mut queued: HashMap<(&DataPath, &str), usize> = HashMap::new();
    let (mut ensured, mut capped) = (0usize, false);
    let flush = |run: &mut Vec<Want>, steps: &mut Vec<Vec<Want>>, removes: &mut Vec<Option<DataPath>>| {
        if !run.is_empty() {
            let mut seen = std::collections::HashSet::new();
            let mut kept: Vec<Want> = std::mem::take(run).into_iter().rev().filter(|w| seen.insert((w.ensure, w.path.clone()))).collect();
            kept.reverse();
            steps.push(kept);
            removes.push(None);
        }
    };
    for op in ops {
        match op {
            DataOp::Value { path, text } if !path.is_empty() => {
                // An empty value clears an existing node and needs no new one.
                run.push(Want { som: path_som(path), path: path.clone(), text: text.clone(), ensure: false });
            }
            DataOp::Instances { parent, name, count } => {
                let done = queued.entry((parent, name.as_str())).or_insert(0);
                while *done < (*count).min(MAX_INDEX) {
                    if ensured >= MAX_ENSURED {
                        capped = true;
                        break;
                    }
                    ensured += 1;
                    #[cfg(test)]
                    ENSURED.with(|n| n.set(n.get() + 1));
                    let k = *done;
                    *done += 1;
                    let mut path = parent.clone();
                    path.push((name.clone(), k));
                    run.push(Want { som: format!("{name}[{k}]"), path, text: String::new(), ensure: true });
                }
            }
            DataOp::Remove(path) if !path.is_empty() => {
                flush(&mut run, &mut steps, &mut removes);
                queued.clear();
                steps.push(Vec::new());
                removes.push(Some(path.clone()));
            }
            _ => {}
        }
    }
    flush(&mut run, &mut steps, &mut removes);
    let plan: Vec<Op> = steps
        .iter()
        .zip(&removes)
        .map(|(w, r)| match r {
            Some(p) => Op::Remove(p),
            None => Op::Values(w),
        })
        .collect();
    let mut written = rewrite(doc, &plan, reuse)?;
    if capped {
        written.warnings.push(format!("XFA data instances not written past {MAX_ENSURED}: the rows were added and removed too often"));
    }
    Ok(written)
}

/// Put `bytes` in the document as a datasets stream: in place of `old` when that is the stream
/// `reuse` or already an unsaved overlay stream of `doc`, else as a new object.
fn put_stream(doc: &mut Document, old: Option<&Object>, reuse: Option<ObjRef>, bytes: &[u8]) -> ObjRef {
    let stream = Object::Stream(Stream::flate(Dict::new(), bytes));
    match reused_stream(doc, old, reuse) {
        Some(r) => {
            doc.set(r, stream);
            r
        }
        _ => doc.add(stream),
    }
}

/// Match put_stream's decision before mutation, including explicit reuse and unsaved overlays.
fn reused_stream(doc: &Document, old: Option<&Object>, reuse: Option<ObjRef>) -> Option<ObjRef> {
    old.and_then(Object::as_ref).filter(|r| reuse == Some(*r) || doc.is_edited(r.num))
}

/// A proposed array edit. The slot gets a new reference unless reused identifies an object
/// replaced in place; every occurrence of that reference then sees the new byte count.
struct PacketReplacement<'a> {
    items: &'a [Object],
    slot: usize,
    reused: Option<ObjRef>,
}

/// Calculate encoded size before allocating, then validate the proposed graph with the reader's
/// traversal and distinct-reference rules. Decode untouched streams one at a time within the
/// remaining allowance. Refusal leaves the document, its overlay and allocation of objects alone.
fn encode_for_rewrite(
    doc: &Document,
    text: &str,
    encoding: Encoding,
    replacement: Option<PacketReplacement<'_>>,
    limit: usize,
    warnings: &mut Vec<String>,
) -> Option<Vec<u8>> {
    let too_large = || XfaError::TooLarge(format!("the proposed XFA exceeds the reader's {limit}-byte decoded packet allowance"));
    let validate = || -> Result<(), XfaError> {
        let candidate = encoded_len(text, encoding).filter(|n| *n <= limit).ok_or_else(too_large)?;
        if let Some(proposal) = replacement {
            let mut seen = std::collections::HashSet::new();
            let mut total = 0usize;
            for (slot, _, object) in packet_entries(proposal.items) {
                // A new reference at the edited slot does not consume the original reference's
                // seen-set entry: another packet may still point at that original stream.
                let new_reference = slot == proposal.slot && proposal.reused.is_none();
                if !new_reference && object.as_ref().is_some_and(|r| !seen.insert(r)) {
                    continue;
                }
                let size = if slot == proposal.slot || proposal.reused.is_some_and(|r| object.as_ref() == Some(r)) {
                    candidate
                } else {
                    let resolved = doc.resolve(object);
                    let Object::Stream(stream) = &*resolved else { continue };
                    stream.decoded_within(limit.checked_sub(total).ok_or_else(too_large)?)?.len()
                };
                total = total.checked_add(size).filter(|n| *n <= limit).ok_or_else(too_large)?;
            }
        }
        Ok(())
    };
    if let Err(e) = validate() {
        warnings.push(format!("The XFA data was not written; it must fit the reader's {limit}-byte decoded packet allowance ({e})"));
        return None;
    }
    Some(encode(text, encoding))
}

/// Longest packet text written (the packets together are read up to 64 MB).
pub(super) const MAX_PACKET_WRITE: usize = 48 << 20;

/// Rewrite with the reader's production allowance; all byte checks precede object mutation.
pub(super) fn rewrite(doc: &mut Document, ops: &[Op], reuse: Option<ObjRef>) -> Result<DatasetsWrite, XfaError> {
    rewrite_with_limit(doc, ops, reuse, MAX_XDP)
}

/// Shared implementation with an explicit byte allowance so boundary tests can use small
/// fixtures. Product entry points always supply MAX_XDP, including imports and script edits.
fn rewrite_with_limit(doc: &mut Document, ops: &[Op], reuse: Option<ObjRef>, limit: usize) -> Result<DatasetsWrite, XfaError> {
    let mut report = DatasetsWrite::default();
    if ops.is_empty() {
        return Ok(report);
    }
    let Some(root) = doc.root() else { return Ok(report) };
    let catalog = doc.get(root);
    let Some(acro_obj) = catalog.as_dict().and_then(|c| c.get(b"AcroForm")).cloned() else { return Ok(report) };
    let acro_ref = acro_obj.as_ref();
    let Some(mut acro) = doc.resolve(&acro_obj).as_dict().cloned() else { return Ok(report) };
    let Some(xfa) = acro.get(b"XFA").cloned() else { return Ok(report) };
    let warnings = &mut report.warnings;
    match &*doc.resolve(&xfa) {
        Object::Array(items) => {
            let mut items = items.clone();
            let at = packet_entries(&items).find_map(|(slot, name, _)| (name.as_deref() == Some("datasets")).then_some(slot));
            match at {
                Some(i) => {
                    let Some(existing) = items.get(i).cloned() else { return Ok(report) };
                    let Some((text, encoding)) = packet_text(doc, &existing, warnings) else { return Ok(report) };
                    let Some(t) = merge_all(&text, Within::Packet, ops, warnings) else { return Ok(report) };
                    if !readable(&t, Within::Packet, warnings) {
                        return Ok(report);
                    }
                    let replacement = PacketReplacement { items: &items, slot: i, reused: reused_stream(doc, Some(&existing), reuse) };
                    let Some(bytes) = encode_for_rewrite(doc, &t, encoding, Some(replacement), limit, warnings) else { return Ok(report) };
                    let r = put_stream(doc, Some(&existing), reuse, &bytes);
                    report.stream = Some(r);
                    if let Some(slot) = items.get_mut(i) {
                        *slot = Object::Ref(r);
                    }
                }
                None => {
                    // No datasets packet yet: made from the first values, then the rest merged
                    // in; before the postamble, or last.
                    let Some(first) = ops.iter().position(|op| matches!(op, Op::Values(_))) else { return Ok(report) };
                    let Some(Op::Values(wants)) = ops.get(first) else { return Ok(report) };
                    let fresh = fresh_datasets(&build_children(wants, warnings));
                    let text = merge_all(&fresh, Within::Packet, ops.get(first + 1..).unwrap_or_default(), warnings).unwrap_or(fresh);
                    if !readable(&text, Within::Packet, warnings) {
                        return Ok(report);
                    }
                    let pos = items.iter().position(|n| n.as_string().is_some_and(|s| s.to_text() == "postamble")).unwrap_or(items.len());
                    items.insert(pos, Object::Null);
                    items.insert(pos, Object::String(PdfString::text("datasets")));
                    let replacement = PacketReplacement { items: &items, slot: pos.saturating_add(1), reused: None };
                    let Some(bytes) = encode_for_rewrite(doc, &text, Encoding::Utf8 { bom: false }, Some(replacement), limit, warnings) else {
                        return Ok(report);
                    };
                    let r = doc.add(Object::Stream(Stream::flate(Dict::new(), &bytes)));
                    report.stream = Some(r);
                    if let Some(slot) = items.get_mut(pos.saturating_add(1)) {
                        *slot = Object::Ref(r);
                    }
                }
            }
            acro.set(b"XFA".to_vec(), Object::Array(items));
        }
        Object::Stream(_) => {
            let Some((text, encoding)) = packet_text(doc, &xfa, warnings) else { return Ok(report) };
            let Some(new) = merge_all(&text, Within::Xdp, ops, warnings) else { return Ok(report) };
            if !readable(&new, Within::Xdp, warnings) {
                return Ok(report);
            }
            let Some(bytes) = encode_for_rewrite(doc, &new, encoding, None, limit, warnings) else { return Ok(report) };
            let r = put_stream(doc, Some(&xfa), reuse, &bytes);
            report.stream = Some(r);
            acro.set(b"XFA".to_vec(), Object::Ref(r));
        }
        _ => return Ok(report),
    }
    match acro_ref {
        Some(ar) => doc.set(ar, Object::Dict(acro)),
        None => doc.update_dict(root, |c| c.set(b"AcroForm".to_vec(), Object::Dict(acro)))?,
    }
    report.written = true;
    Ok(report)
}

/// The datasets stream's object reference in `doc` (the `datasets` item in the `/XFA` array, or
/// the single `/XFA` stream), when `doc` has one.
pub(super) fn datasets_stream_ref(doc: &Document) -> Option<ObjRef> {
    let root = doc.root()?;
    let catalog = doc.get(root);
    let acro = catalog.as_dict()?.get(b"AcroForm").map(|a| doc.resolve(a))?;
    let xfa = acro.as_dict()?.get(b"XFA")?;
    match &*doc.resolve(xfa) {
        Object::Array(items) => {
            packet_entries(items).find_map(|(_, name, object)| (name.as_deref() == Some("datasets")).then(|| object.as_ref()).flatten())
        }
        Object::Stream(_) => xfa.as_ref(),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pdfcraft_cos::{SaveOptions, write_incremental};
    use std::sync::Arc;

    // Preserve the original real-limit regressions in PDF-filter-decoded bytes, not UTF-8 text
    // bytes. Additional boundary cases below use the explicit internal rewrite allowance.
    const READER_BYTES: usize = 64 << 20;
    const DECL16: &str = "<?xml version=\"1.0\" encoding=\"UTF-16\"?>";
    const OPEN: &str = "<xdp:xdp xmlns:xdp=\"http://ns.adobe.com/xdp/\">";
    const TEMPLATE: &str = "<template><subform name=\"form\"/></template>";
    const DATA: &str =
        "<xfa:datasets xmlns:xfa=\"http://www.xfa.org/schema/xfa-data/1.0/\"><xfa:data><form><entry>old</entry></form></xfa:data></xfa:datasets>";
    const CLOSE: &str = "</xdp:xdp>";

    /// Install independent, compressed synthetic streams and reopen so a datasets edit must
    /// preserve the stored objects, rather than treating the original as an unsaved overlay.
    fn packet_document(packets: &[(&str, Vec<u8>)], whole: bool) -> Document {
        let mut doc = Document::new_empty();
        let mut items = Vec::new();
        for (name, bytes) in packets {
            let stream = doc.add(Object::Stream(Stream::flate(Dict::new(), bytes)));
            items.extend([Object::String(PdfString::text(name)), Object::Ref(stream)]);
        }
        let xfa = if whole { items[1].clone() } else { Object::Array(items) };
        doc.update_dict(doc.root().unwrap(), |catalog| {
            let mut acro = Dict::new();
            acro.set(b"XFA".to_vec(), xfa);
            catalog.set(b"AcroForm".to_vec(), Object::Dict(acro));
        })
        .unwrap();
        Document::open(Arc::new(write_incremental(&doc, &SaveOptions::default()).unwrap())).unwrap()
    }

    /// Snapshot the XFA entry itself, including its original packet references and order.
    fn xfa_entry(doc: &Document) -> Object {
        let catalog = doc.get(doc.root().unwrap());
        let acro = doc.resolve(catalog.as_dict().unwrap().get(b"AcroForm").unwrap());
        acro.as_dict().unwrap().get(b"XFA").unwrap().clone()
    }

    /// A reader-incompatible edit must warn, report no stream, and preserve the entire original
    /// document. Include the actual reader result in failures without printing large XML values.
    fn assert_oversized_edit_refused(mut doc: Document, value: &str) {
        let packets = crate::read_packets(&doc).unwrap().unwrap();
        let parsed = roxmltree::Document::parse(&packets.xdp).expect("the original assembled XDP is meaningful XML");
        assert!(parsed.descendants().any(|n| n.has_tag_name("template")));
        assert_eq!(crate::parse_datasets(&packets.xdp).unwrap().text_at(&crate::som_to_path("form[0].entry[0]")), Some("old"));
        drop(parsed);
        drop(packets);
        let original_xfa = xfa_entry(&doc);
        let original_ref = datasets_stream_ref(&doc).unwrap();
        let original_stream = doc.get(original_ref);
        let original_pdf = write_incremental(&doc, &SaveOptions::default()).unwrap();
        let original_edits = doc.modified_objects();
        let original_modified = doc.is_modified();
        let report = write_data_value(&mut doc, &crate::som_to_path("form[0].entry[0]"), value).unwrap();
        let reader_after = crate::read_packets(&doc).map(|p| p.is_some());
        let references_preserved = xfa_entry(&doc) == original_xfa && datasets_stream_ref(&doc) == Some(original_ref);
        let stream_preserved = doc.get(original_ref) == original_stream;
        let document_preserved = write_incremental(&doc, &SaveOptions::default()).unwrap() == original_pdf;
        assert_eq!(doc.modified_objects(), original_edits);
        assert_eq!(doc.is_modified(), original_modified);
        assert!(
            !report.written
                && report.stream.is_none()
                && !report.warnings.is_empty()
                && references_preserved
                && stream_preserved
                && document_preserved
                && matches!(reader_after, Ok(true)),
            "oversized edit must refuse with warnings and preserve original XFA: report={report:?}; \
             references_preserved={references_preserved}; stream_preserved={stream_preserved}; \
             document_preserved={document_preserved}; reader_after={reader_after:?}"
        );
    }

    /// Exercise either writer branch with a valid UTF-16 source. ASCII replacement text is under
    /// the 48 MiB text budget, but its UTF-16 encoding (including BOM) exceeds the 64 MiB reader.
    fn check_utf16_output_limit(whole: bool) {
        let _large = crate::packets::LARGE_PACKET_TEST.lock().unwrap_or_else(|e| e.into_inner());
        let encoding = Encoding::Utf16Le { bom: true };
        let original = if whole { format!("{DECL16}{OPEN}{TEMPLATE}{DATA}{CLOSE}") } else { format!("{DECL16}{DATA}") };
        let value = "x".repeat(READER_BYTES / 2);
        assert!(original.is_ascii() && value.is_ascii());
        let output_text_bytes = original.len() - "old".len() + value.len();
        assert!(output_text_bytes < MAX_PACKET_WRITE, "this is not the existing text-budget refusal");
        assert!(2 + 2 * output_text_bytes > READER_BYTES, "encoded UTF-16 must exceed the reader's allowance");
        let packets = if whole {
            vec![("xdp", encode(&original, encoding))]
        } else {
            vec![
                ("preamble", OPEN.as_bytes().to_vec()),
                ("template", TEMPLATE.as_bytes().to_vec()),
                ("datasets", encode(&original, encoding)),
                ("postamble", CLOSE.as_bytes().to_vec()),
            ]
        };
        assert_oversized_edit_refused(packet_document(&packets, whole), &value);
    }

    /// Refuse a UTF-16 datasets replacement that would make a previously readable array unreadable.
    #[test]
    fn utf16_datasets_edit_refuses_output_over_reader_limit() {
        check_utf16_output_limit(false);
    }

    /// Refuse the same UTF-16 expansion in the separate whole-XDP-stream writer branch.
    #[test]
    fn utf16_whole_xdp_edit_refuses_output_over_reader_limit() {
        check_utf16_output_limit(true);
    }

    /// All individual packets and the rewritten datasets fit their limits. The sum starts one
    /// byte below the reader's allowance; a small datasets edit must be refused before replacement.
    /// A compressed synthetic XML comment supplies padding without adding millions of XML nodes.
    #[test]
    fn datasets_edit_refuses_aggregate_packet_bytes_over_reader_limit() {
        let _large = crate::packets::LARGE_PACKET_TEST.lock().unwrap_or_else(|e| e.into_inner());
        let mut packets = vec![
            ("preamble", OPEN.as_bytes().to_vec()),
            ("template", TEMPLATE.as_bytes().to_vec()),
            ("datasets", DATA.as_bytes().to_vec()),
            ("postamble", CLOSE.as_bytes().to_vec()),
        ];
        let fixed: usize = packets.iter().map(|(_, bytes)| bytes.len()).sum();
        let padding = format!("<!--{}-->", " ".repeat(READER_BYTES - 1 - fixed - "<!---->".len()));
        packets.insert(1, ("padding", padding.into_bytes()));
        let before_bytes: usize = packets.iter().map(|(_, bytes)| bytes.len()).sum();
        assert_eq!(before_bytes, READER_BYTES - 1);
        assert!(packets.iter().all(|(_, bytes)| bytes.len() < READER_BYTES));
        let value = "new-value";
        assert!(DATA.len() - "old".len() + value.len() < MAX_PACKET_WRITE);
        assert!(before_bytes - "old".len() + value.len() > READER_BYTES);
        let doc = packet_document(&packets, false);
        drop(packets);
        assert_oversized_edit_refused(doc, value);
    }

    /// Small well-formed packets for the same rewrite implementation with an explicit allowance.
    fn small_document(with_data: bool) -> Document {
        let mut packets = vec![("preamble", OPEN.as_bytes().to_vec()), ("template", TEMPLATE.as_bytes().to_vec())];
        if with_data {
            packets.push(("datasets", DATA.as_bytes().to_vec()));
        }
        packets.push(("postamble", CLOSE.as_bytes().to_vec()));
        packet_document(&packets, false)
    }

    /// Exercise the production rewrite implementation with one small value and a test allowance.
    fn small_rewrite(doc: &mut Document, value: &str, reuse: Option<ObjRef>, limit: usize) -> DatasetsWrite {
        let wants = [Want { som: "form[0].entry[0]".into(), path: crate::som_to_path("form[0].entry[0]"), text: value.into(), ensure: false }];
        rewrite_with_limit(doc, &[Op::Values(&wants)], reuse, limit).unwrap()
    }

    /// Count fixture stream bytes independently of packet_entries. These fixtures have only
    /// direct names, harmless scalar entries, inline streams and references to actual streams.
    fn fixture_bytes(doc: &Document) -> usize {
        let Object::Array(items) = xfa_entry(doc) else { panic!("array fixture") };
        let mut seen = std::collections::HashSet::new();
        items
            .iter()
            .filter(|o| o.as_ref().is_none_or(|r| seen.insert(r)))
            .filter_map(|o| match &*doc.resolve(o) {
                Object::Stream(s) => Some(s.decoded().unwrap().len()),
                _ => None,
            })
            .sum()
    }

    /// Check refusal before any mutation, including pre-existing unsaved overlays and the next
    /// object number. Both the original and refused result must remain meaningful, parseable XDP.
    fn assert_small_refusal(mut doc: Document, value: &str, reuse: Option<ObjRef>, limit: usize) {
        let before_xml = crate::read_packets(&doc).unwrap().unwrap().xdp;
        let parsed = roxmltree::Document::parse(&before_xml).unwrap();
        assert!(parsed.descendants().any(|n| n.has_tag_name("template")));
        let before_xfa = xfa_entry(&doc);
        let before_pdf = write_incremental(&doc, &SaveOptions::default()).unwrap();
        let mut before_edits = doc.modified_objects();
        before_edits.sort_unstable();
        let before_modified = doc.is_modified();
        let mut untouched = doc.clone();
        let next = untouched.add(Object::Null);
        let report = small_rewrite(&mut doc, value, reuse, limit);
        assert!(!report.written && report.stream.is_none(), "{report:?}");
        assert!(report.warnings.iter().any(|w| w.contains("byte") && w.contains("not written")), "{report:?}");
        assert_eq!(xfa_entry(&doc), before_xfa);
        assert_eq!(write_incremental(&doc, &SaveOptions::default()).unwrap(), before_pdf);
        let mut after_edits = doc.modified_objects();
        after_edits.sort_unstable();
        assert_eq!(after_edits, before_edits);
        assert_eq!(doc.is_modified(), before_modified);
        assert_eq!(crate::read_packets(&doc).unwrap().unwrap().xdp, before_xml);
        assert_eq!(doc.add(Object::Null), next, "refusal must not allocate an unreachable stream object");
    }

    /// Existing replacement and missing-datasets insertion refuse one byte below the exact
    /// resulting total, and succeed at equality. No large allocation is needed for these paths.
    #[test]
    fn small_budget_replacement_and_insertion_preserve_documents_on_refusal() {
        for with_data in [true, false] {
            let doc = small_document(with_data);
            let mut probe = doc.clone();
            let report = small_rewrite(&mut probe, "new-value", None, 4096);
            assert!(report.written && report.warnings.is_empty(), "{report:?}");
            let needed = fixture_bytes(&probe);
            assert_small_refusal(doc.clone(), "new-value", None, needed - 1);
            let mut exact = doc;
            let report = small_rewrite(&mut exact, "new-value", None, needed);
            assert!(report.written && report.warnings.is_empty(), "{report:?}");
            assert_eq!(fixture_bytes(&exact), needed);
            let xml = crate::read_packets(&exact).unwrap().unwrap().xdp;
            assert!(roxmltree::Document::parse(&xml).is_ok());
            assert_eq!(crate::parse_datasets(&xml).unwrap().text_at(&crate::som_to_path("form[0].entry[0]")), Some("new-value"));
        }
    }

    /// UTF-16BE BOMs and surrogate pairs use encoded bytes in both array and whole-stream
    /// branches. The same small boundary rejects one byte short and accepts exact equality.
    #[test]
    fn small_budget_utf16_bom_and_surrogate_pairs_use_encoded_bytes() {
        let encoding = Encoding::Utf16Be { bom: true };
        for whole in [true, false] {
            let original = if whole { format!("{DECL16}{OPEN}{TEMPLATE}{DATA}{CLOSE}") } else { format!("{DECL16}{DATA}") };
            let packets = if whole {
                vec![("xdp", encode(&original, encoding))]
            } else {
                vec![
                    ("preamble", OPEN.as_bytes().to_vec()),
                    ("template", TEMPLATE.as_bytes().to_vec()),
                    ("datasets", encode(&original, encoding)),
                    ("postamble", CLOSE.as_bytes().to_vec()),
                ]
            };
            let doc = packet_document(&packets, whole);
            let value = "new-é-\u{1F600}";
            let mut probe = doc.clone();
            let report = small_rewrite(&mut probe, value, None, 4096);
            assert!(report.written && report.warnings.is_empty(), "{report:?}");
            let needed = if whole {
                let object = probe.get(report.stream.unwrap());
                let Object::Stream(s) = &*object else { panic!("stream fixture") };
                s.decoded().unwrap().len()
            } else {
                fixture_bytes(&probe)
            };
            assert_small_refusal(doc.clone(), value, None, needed - 1);
            let mut exact = doc;
            let report = small_rewrite(&mut exact, value, None, needed);
            assert!(report.written && report.warnings.is_empty(), "{report:?}");
            let xml = crate::read_packets(&exact).unwrap().unwrap().xdp;
            assert_eq!(crate::parse_datasets(&xml).unwrap().text_at(&crate::som_to_path("form[0].entry[0]")), Some(value));
        }
    }

    /// Explicit reuse and automatic unsaved-overlay replacement must refuse before overwriting
    /// the shared stream. Successful in-place edits count duplicate references exactly once.
    #[test]
    fn small_budget_reuse_and_overlay_preserve_streams_and_edit_state() {
        for explicit in [true, false] {
            let mut doc = small_document(true);
            let original = datasets_stream_ref(&doc).unwrap();
            let Object::Array(mut items) = xfa_entry(&doc) else { panic!("array fixture") };
            // The duplicate is visited before the datasets slot, testing first-occurrence accounting.
            items.insert(2, Object::Ref(original));
            items.insert(2, Object::String(PdfString::text("duplicate")));
            doc.update_dict(doc.root().unwrap(), |c| {
                c.get_mut(b"AcroForm").unwrap().as_dict_mut().unwrap().set(b"XFA".to_vec(), Object::Array(items))
            })
            .unwrap();
            if !explicit {
                doc.set(original, (*doc.get(original)).clone());
            }
            let reuse = explicit.then_some(original);
            let mut probe = doc.clone();
            let report = small_rewrite(&mut probe, "new-value", reuse, 4096);
            assert!(report.written && report.warnings.is_empty(), "{report:?}");
            assert_eq!(report.stream, Some(original));
            let needed = fixture_bytes(&probe);
            assert_small_refusal(doc.clone(), "new-value", reuse, needed - 1);
            let report = small_rewrite(&mut doc, "new-value", reuse, needed);
            assert!(report.written && report.warnings.is_empty(), "{report:?}");
            assert_eq!(report.stream, Some(original));
            assert_eq!(fixture_bytes(&doc), needed);
            let xml = crate::read_packets(&doc).unwrap().unwrap().xdp;
            assert_eq!(crate::parse_datasets(&xml).unwrap().text_at(&crate::som_to_path("form[0].entry[0]")), Some("new-value"));
        }
    }

    /// A new datasets reference leaves aliases of the old stream alive. Count that old stream
    /// once, and count equal direct streams separately; traverse mixed named/unnamed array slots.
    #[test]
    fn small_budget_counts_old_aliases_and_direct_streams_like_reader() {
        let mut doc = small_document(true);
        let original = datasets_stream_ref(&doc).unwrap();
        let Object::Array(mut items) = xfa_entry(&doc) else { panic!("array fixture") };
        let comment = Object::Stream(Stream::flate(Dict::new(), b"<!--padding-->"));
        items.insert(2, comment.clone());
        items.insert(2, comment);
        items.insert(0, Object::Bool(false));
        let end = items.len() - 2;
        items.splice(end..end, [Object::String(PdfString::text("alias")), Object::Ref(original), Object::Ref(original)]);
        doc.update_dict(doc.root().unwrap(), |c| c.get_mut(b"AcroForm").unwrap().as_dict_mut().unwrap().set(b"XFA".to_vec(), Object::Array(items)))
            .unwrap();
        let before_stream = doc.get(original);
        let mut probe = doc.clone();
        let report = small_rewrite(&mut probe, "new-value", None, 4096);
        assert!(report.written && report.warnings.is_empty(), "{report:?}");
        assert_ne!(report.stream, Some(original), "the stored original is not an overlay");
        let needed = fixture_bytes(&probe);
        assert_eq!(needed, fixture_bytes(&doc) + DATA.replace("old", "new-value").len());
        assert_small_refusal(doc.clone(), "new-value", None, needed - 1);
        let report = small_rewrite(&mut doc, "new-value", None, needed);
        assert!(report.written && report.warnings.is_empty(), "{report:?}");
        assert_eq!(doc.get(original), before_stream);
        assert_eq!(fixture_bytes(&doc), needed);
        assert!(roxmltree::Document::parse(&crate::read_packets(&doc).unwrap().unwrap().xdp).is_ok());
    }

    /// A BOM or declaration contradiction must refuse datasets writes with original storage and
    /// references intact. This also guards the writer's exact-decoding path independently.
    #[test]
    fn encoding_contradictions_refuse_dataset_writes_without_mutation() {
        for bytes in [
            format!("{DECL16}{DATA}").into_bytes(),
            encode(&format!("{}{DATA}", DECL16.replace("UTF-16", "UTF-16BE")), Encoding::Utf16Le { bom: true }),
        ] {
            let mut doc = packet_document(&[("datasets", bytes)], false);
            let before = write_incremental(&doc, &SaveOptions::default()).unwrap();
            let reference = datasets_stream_ref(&doc);
            let report = write_data_value(&mut doc, &crate::som_to_path("form[0].entry[0]"), "new").unwrap();
            assert!(!report.written && report.stream.is_none() && !report.warnings.is_empty(), "{report:?}");
            assert_eq!(datasets_stream_ref(&doc), reference);
            assert_eq!(write_incremental(&doc, &SaveOptions::default()).unwrap(), before);
            assert!(!doc.is_modified());
        }
    }

    /// ASCII input with a legacy label remains readable without mutation. A non-ASCII edit
    /// must either refuse atomically or produce bytes an external consumer decodes correctly
    /// under the stored declaration. Decode Latin-1 byte-for-byte here, independently of the
    /// tolerant XFA reader; include a matching UTF-8 raw-byte control before the regression.
    #[test]
    fn legacy_label_dataset_edit_preserves_external_xml_encoding() {
        let utf8_decl = "<?xml version=\"1.0\" encoding=\"UTF-8\"?>";
        let legacy_decl = "<?xml version=\"1.0\" encoding=\"ISO-8859-1\"?>";
        let original = format!("{legacy_decl}{DATA}");
        assert!(original.is_ascii());
        let packets = |data: Vec<u8>| {
            vec![
                ("preamble", OPEN.as_bytes().to_vec()),
                ("template", TEMPLATE.as_bytes().to_vec()),
                ("datasets", data),
                ("postamble", CLOSE.as_bytes().to_vec()),
            ]
        };
        let path = crate::som_to_path("form[0].entry[0]");
        let value = "\u{e9}";

        let mut control = packet_document(&packets(format!("{utf8_decl}{DATA}").into_bytes()), false);
        let report = write_data_value(&mut control, &path, value).unwrap();
        assert!(report.written && report.warnings.is_empty(), "{report:?}");
        let object = control.get(report.stream.unwrap());
        let Object::Stream(stream) = &*object else { panic!("UTF-8 control stream") };
        assert_eq!(stream.decoded().unwrap(), format!("{utf8_decl}{}", DATA.replace("old", value)).as_bytes());
        eprintln!("UTF8_RAW_BYTE_CONTROL_PASSED: e-acute is C3 A9 under UTF-8");

        let mut doc = packet_document(&packets(original.as_bytes().to_vec()), false);
        let before_xfa = xfa_entry(&doc);
        let before_ref = datasets_stream_ref(&doc).unwrap();
        let before_stream = doc.get(before_ref);
        let before_pdf = write_incremental(&doc, &SaveOptions::default()).unwrap();
        let xml = crate::read_packets(&doc).unwrap().unwrap().xdp;
        assert!(roxmltree::Document::parse(&xml).is_ok());
        assert_eq!(crate::parse_datasets(&xml).unwrap().text_at(&path), Some("old"));
        assert_eq!(doc.get(before_ref), before_stream);
        assert_eq!(xfa_entry(&doc), before_xfa);
        assert_eq!(write_incremental(&doc, &SaveOptions::default()).unwrap(), before_pdf);
        assert!(!doc.is_modified());
        eprintln!("LEGACY_READ_ONLY_CONTROL_PASSED: ASCII ISO-8859-1 input and original storage are preserved");

        let report = write_data_value(&mut doc, &path, value).unwrap();
        if !report.written {
            assert!(report.stream.is_none() && !report.warnings.is_empty(), "{report:?}");
            assert_eq!(doc.get(before_ref), before_stream);
            assert_eq!(datasets_stream_ref(&doc), Some(before_ref));
            assert_eq!(xfa_entry(&doc), before_xfa);
            assert_eq!(write_incremental(&doc, &SaveOptions::default()).unwrap(), before_pdf);
            assert!(!doc.is_modified());
            return;
        }
        let object = doc.get(report.stream.unwrap());
        let Object::Stream(stream) = &*object else { panic!("written datasets stream") };
        let raw = stream.decoded().unwrap(); // Only PDF filters, not XFA's tolerant text decoder.
        let external_text = if raw.starts_with(legacy_decl.as_bytes()) {
            raw.iter().copied().map(char::from).collect::<String>() // Exact ISO-8859-1 mapping.
        } else if raw.starts_with(utf8_decl.as_bytes()) {
            String::from_utf8(raw.clone()).expect("an explicitly converted UTF-8 declaration requires UTF-8 bytes")
        } else {
            panic!("unexpected declaration in written packet: {raw:02X?}");
        };
        let parsed = roxmltree::Document::parse(&external_text).unwrap();
        let external_value = parsed.descendants().find(|n| n.has_tag_name("entry")).and_then(|n| n.text()).unwrap();
        eprintln!(
            "LEGACY_WRITE_REPORT: {report:?}; stored bytes={raw:02X?}; external codepoints={:X?}",
            external_value.chars().map(u32::from).collect::<Vec<_>>()
        );
        assert_eq!(external_value, value, "the stored encoding declaration must agree with the field's raw encoded bytes");
    }

    /// Unsupported declarations refuse both ASCII and non-ASCII edits, independently of value
    /// representability. Array/whole-stream and stored/reuse/overlay paths must preserve bytes,
    /// references, edit state and the next object number; reading the original remains tolerated.
    #[test]
    fn unsupported_declared_encodings_refuse_edits_without_mutation() {
        for label in ["ISO-8859-1", "US-ASCII", "x-test-legacy"] {
            for whole in [false, true] {
                let declaration = format!("<?xml version=\"1.0\" encoding=\"{label}\"?>");
                let original = if whole { format!("{declaration}{OPEN}{TEMPLATE}{DATA}{CLOSE}") } else { format!("{declaration}{DATA}") };
                let packets = if whole {
                    vec![("xdp", original.into_bytes())]
                } else {
                    vec![
                        ("preamble", OPEN.as_bytes().to_vec()),
                        ("template", TEMPLATE.as_bytes().to_vec()),
                        ("datasets", original.into_bytes()),
                        ("postamble", CLOSE.as_bytes().to_vec()),
                    ]
                };
                for value in ["new-ascii", "\u{e9}"] {
                    for mode in 0..3 {
                        let mut doc = packet_document(&packets, whole);
                        let reference = datasets_stream_ref(&doc).unwrap();
                        if mode == 1 {
                            doc.set(reference, (*doc.get(reference)).clone());
                        }
                        let before_stream = doc.get(reference);
                        let before_xfa = xfa_entry(&doc);
                        let before_pdf = write_incremental(&doc, &SaveOptions::default()).unwrap();
                        let mut before_edits = doc.modified_objects();
                        before_edits.sort_unstable();
                        let before_modified = doc.is_modified();
                        let before_xml = crate::read_packets(&doc).unwrap().unwrap().xdp;
                        assert!(roxmltree::Document::parse(&before_xml).is_ok());
                        assert_eq!(crate::parse_datasets(&before_xml).unwrap().text_at(&crate::som_to_path("form[0].entry[0]")), Some("old"));
                        let mut untouched = doc.clone();
                        let next = untouched.add(Object::Null);
                        let op = DataOp::Value { path: crate::som_to_path("form[0].entry[0]"), text: value.into() };
                        let report = write_data_ops(&mut doc, &[op], (mode == 2).then_some(reference)).unwrap();
                        assert!(!report.written && report.stream.is_none(), "{label}, whole={whole}, mode={mode}: {report:?}");
                        assert!(
                            report.warnings.iter().any(|w| w.contains(label) && w.contains("unsupported") && w.contains("not written")),
                            "{report:?}"
                        );
                        assert_eq!(doc.get(reference), before_stream);
                        assert_eq!(datasets_stream_ref(&doc), Some(reference));
                        assert_eq!(xfa_entry(&doc), before_xfa);
                        assert_eq!(write_incremental(&doc, &SaveOptions::default()).unwrap(), before_pdf);
                        let mut after_edits = doc.modified_objects();
                        after_edits.sort_unstable();
                        assert_eq!(after_edits, before_edits);
                        assert_eq!(doc.is_modified(), before_modified);
                        assert_eq!(crate::read_packets(&doc).unwrap().unwrap().xdp, before_xml);
                        assert_eq!(doc.add(Object::Null), next);
                    }
                }
            }
        }
    }

    /// The writer-only restriction must preserve undeclared UTF-8/UTF-16 and their BOM variants,
    /// as well as matching explicit endian labels. Compare the stored bytes, not a repaired read.
    #[test]
    fn supported_and_undeclared_encodings_remain_writable() {
        for encoding in [
            Encoding::Utf8 { bom: false },
            Encoding::Utf8 { bom: true },
            Encoding::Utf16Le { bom: false },
            Encoding::Utf16Le { bom: true },
            Encoding::Utf16Be { bom: false },
            Encoding::Utf16Be { bom: true },
        ] {
            let label = match encoding {
                Encoding::Utf8 { .. } => "utf-8",
                Encoding::Utf16Le { .. } => "utf-16le",
                Encoding::Utf16Be { .. } => "utf-16be",
            };
            for declaration in [String::new(), format!("<?xml version='1.0' encoding='{label}'?>")] {
                let original = format!("{declaration}{OPEN}{TEMPLATE}{DATA}{CLOSE}");
                let mut doc = packet_document(&[("xdp", encode(&original, encoding))], true);
                let report = write_data_value(&mut doc, &crate::som_to_path("form[0].entry[0]"), "\u{e9}").unwrap();
                assert!(report.written && report.warnings.is_empty(), "{encoding:?}: {report:?}");
                let object = doc.get(report.stream.unwrap());
                let Object::Stream(stream) = &*object else { panic!("written XDP stream") };
                assert_eq!(stream.decoded().unwrap(), encode(&original.replace("old", "\u{e9}"), encoding));
            }
        }
    }
}
