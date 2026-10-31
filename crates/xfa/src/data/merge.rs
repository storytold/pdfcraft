use super::build::{Build, CONFLICT, CONTROL, NewNodes, STRUCTURED, TOO_MANY, build_children, escape, fresh_datasets, skipped, xml_name};
use super::import::{ImportNode, serialize_nodes};
use super::values::Want;
use super::write::MAX_PACKET_WRITE;
use super::{DataPath, HashMap, MAX_NODES, XFA_DATA_NS};

/// What a rewrite of the datasets packet does.
#[derive(Clone, Copy)]
pub(super) enum Op<'a> {
    /// Put these values in (adding nodes as needed).
    Values(&'a [Want]),
    /// Take the node at this path out.
    Remove(&'a DataPath),
    /// Make these nodes the whole content of the `xfa:data` element (an import).
    ReplaceData(&'a [ImportNode]),
    /// Make the leaf at this path a picture: base64 text with its content type.
    Picture(&'a DataPath, &'a str, &'a str),
}

/// Where an element's start tag ends (the index of its `>`), skipping quoted attribute values.
pub(super) fn start_tag_end(src: &str, range: &std::ops::Range<usize>) -> Option<usize> {
    let bytes = src.as_bytes();
    let mut quote = None;
    for i in range.clone() {
        let b = *bytes.get(i)?;
        match (quote, b) {
            (Some(q), _) if b == q => quote = None,
            (Some(_), _) => {}
            (None, b'"' | b'\'') => quote = Some(b),
            (None, b'>') => return Some(i),
            _ => {}
        }
    }
    None
}

/// An element's qualified name as written.
pub(super) fn qname<'s>(src: &'s str, range: &std::ops::Range<usize>) -> Option<&'s str> {
    let rest = src.get(range.start.checked_add(1)?..range.end)?;
    let end = rest.find(|c: char| c.is_whitespace() || c == '/' || c == '>')?;
    rest.get(..end).filter(|n| !n.is_empty())
}

/// One change to the packet text: replace `start..end` with `text`.
struct Edit {
    start: usize,
    end: usize,
    text: String,
}

/// The edit that puts `inner` (markup) at the end of `node`'s content.
fn append_edit(src: &str, node: roxmltree::Node, inner: &str) -> Option<Edit> {
    let range = node.range();
    let tag_end = start_tag_end(src, &range)?;
    if tag_end.checked_add(1)? == range.end && src.get(..tag_end)?.ends_with('/') {
        // `<a/>` becomes `<a>inner</a>`.
        let name = qname(src, &range)?;
        return Some(Edit { start: tag_end.checked_sub(1)?, end: range.end, text: format!(">{inner}</{name}>") });
    }
    let close = range.start + src.get(range.clone())?.rfind("</")?;
    Some(Edit { start: close, end: close, text: inner.to_string() })
}

/// The edit that makes `text` (escaped) the whole content of the leaf `node`.
fn replace_edit(src: &str, node: roxmltree::Node, text: &str) -> Option<Edit> {
    let range = node.range();
    let tag_end = start_tag_end(src, &range)?;
    if tag_end.checked_add(1)? == range.end && src.get(..tag_end)?.ends_with('/') {
        let name = qname(src, &range)?;
        return Some(Edit { start: tag_end.checked_sub(1)?, end: range.end, text: format!(">{text}</{name}>") });
    }
    let close = range.start + src.get(range.clone())?.rfind("</")?;
    (close > tag_end).then(|| Edit { start: tag_end + 1, end: close, text: text.to_string() })
}

/// What merging the values into a packet's text came to.
enum Merged {
    /// The data already holds every value.
    Unchanged,
    Changed(String),
    /// The text could not be merged into (the reason is in the warnings).
    Failed,
}

/// Where a datasets packet sits: one packet of an array (`Packet`), or inside the whole XDP.
#[derive(Clone, Copy, PartialEq)]
pub(super) enum Within {
    Packet,
    Xdp,
}

pub(super) const WRAP_OPEN: &str = "<pdfcraft-wrap xmlns:xfa=\"http://www.xfa.org/schema/xfa-data/1.0/\" xmlns:xdp=\"http://ns.adobe.com/xdp/\">";

pub(super) const WRAP_CLOSE: &str = "</pdfcraft-wrap>";

/// Supply inherited packet namespaces after any validated leading declaration. The returned
/// offset locates the wrapper opening tag, so edits keep the declaration outside that tag.
/// Malformed declarations return `None`; allocation adds only the fixed wrapper to the
/// caller's size-bounded packet, without changing the packet body or declaration bytes.
fn wrapped_packet(text: &str) -> Option<(String, usize)> {
    let body = crate::packets::after_declaration(text).ok()?;
    let declaration_len = text.len().checked_sub(body.len())?;
    let declaration = text.get(..declaration_len)?;
    Some((format!("{declaration}{WRAP_OPEN}{body}{WRAP_CLOSE}"), declaration_len))
}

/// Write `wants` into the data of the datasets packet in `text`, changing only what has to
/// change: values go into the data nodes they bind to, missing nodes are added, and everything
/// else (data no field binds to, other namespaces, attributes, comments, layout) stays as
/// written.
/// The prefix bound to the XFA data namespace where `n` is (`None`: unbound or the default
/// namespace, which attributes can't use).
fn xfa_prefix(n: roxmltree::Node) -> Option<String> {
    n.namespaces().find(|ns| ns.uri() == XFA_DATA_NS).and_then(|ns| ns.name()).filter(|p| !p.is_empty()).map(str::to_string)
}

/// Merge one operation using parser byte coordinates, then remove only synthetic wrapper
/// tags. Parse or coordinate failures leave the original packet untouched with a warning.
fn merge(text: &str, within: Within, op: Op, warnings: &mut Vec<String>) -> Merged {
    let wants: &[Want] = match op {
        Op::Values(w) => w,
        Op::Remove(_) | Op::ReplaceData(_) | Op::Picture(..) => &[],
    };
    // Fresh data, where none exists yet (declared with the `xfa` prefix).
    let fresh_children = |warnings: &mut Vec<String>| match op {
        Op::ReplaceData(nodes) => serialize_nodes(nodes, "xfa"),
        _ => build_children(wants, warnings),
    };
    if crate::xml_nesting_depth(text) > crate::MAX_XML_DEPTH {
        warnings.push("The XFA data nests XML elements deeper than the reader accepts; the field values were not written to it".into());
        return Merged::Failed;
    }
    let opts = || roxmltree::ParsingOptions { allow_dtd: false, nodes_limit: MAX_NODES, ..Default::default() };
    // A packet of an array may use the prefixes the XDP envelope declares: read it inside
    // an element declaring the usual ones.
    let wrapped;
    let (src, wrapper_start, parsed) = match roxmltree::Document::parse_with_options(text, opts()) {
        Ok(d) => (text, None, d),
        Err(first) => {
            wrapped = (within == Within::Packet).then(|| wrapped_packet(text)).flatten();
            match wrapped
                .as_ref()
                .and_then(|(src, start)| roxmltree::Document::parse_with_options(src, opts()).ok().map(|parsed| (src.as_str(), Some(*start), parsed)))
            {
                Some(parsed) => parsed,
                None => {
                    warnings.push(format!("The XFA data could not be read ({first}); the field values were not written to it"));
                    return Merged::Failed;
                }
            }
        }
    };
    let is = |n: &roxmltree::Node, name: &str| n.is_element() && n.tag_name().name() == name;
    let datasets = parsed
        .descendants()
        .find(|n| is(n, "datasets") && n.tag_name().namespace() == Some(XFA_DATA_NS))
        .or_else(|| parsed.descendants().find(|n| is(n, "datasets")));
    let mut edits = Vec::new();
    let mut data = None;
    match datasets {
        None if matches!(op, Op::Remove(_) | Op::Picture(..)) => return Merged::Unchanged,
        None if within == Within::Xdp => {
            let root = parsed.root_element();
            let children = fresh_children(warnings);
            match append_edit(src, root, &fresh_datasets(&children)) {
                Some(e) => edits.push(e),
                None => {
                    warnings.push("The XFA packets could not be extended with data; the field values were not written".into());
                    return Merged::Failed;
                }
            }
        }
        None => {
            warnings.push("The XFA datasets packet holds no datasets element; the field values were not written to it".into());
            return Merged::Failed;
        }
        Some(ds) => match ds.children().find(|n| is(n, "data")) {
            Some(d) => data = Some(d),
            None if matches!(op, Op::Remove(_) | Op::Picture(..)) => return Merged::Unchanged,
            None => {
                let prefix = qname(src, &ds.range()).and_then(|q| q.split_once(':')).map(|(p, _)| format!("{p}:")).unwrap_or_default();
                let children = fresh_children(warnings);
                let element = format!("<{prefix}data>{}</{prefix}data>", children);
                match append_edit(src, ds, &element) {
                    Some(e) => edits.push(e),
                    None => {
                        warnings.push("The XFA datasets could not be extended with data; the field values were not written".into());
                        return Merged::Failed;
                    }
                }
            }
        },
    }
    if let Some(data) = data {
        match op {
            Op::Values(w) => merge_values(src, data, w, &mut edits, warnings),
            Op::ReplaceData(nodes) => {
                // Everything between the start and end tags; `<xfa:data/>` opens up. Written
                // with the prefix the packet binds to the data namespace.
                let prefix = xfa_prefix(data);
                let markup = serialize_nodes(nodes, prefix.as_deref().unwrap_or("xfa"));
                let range = data.range();
                match (start_tag_end(src, &range), qname(src, &range)) {
                    (Some(tag_end), Some(name)) if tag_end + 1 == range.end && src.get(..tag_end).is_some_and(|s| s.ends_with('/')) => {
                        edits.push(Edit { start: range.start, end: range.end, text: format!("<{name}>{markup}</{name}>") });
                    }
                    (Some(tag_end), _) => {
                        let close = src.get(range.clone()).and_then(|s| s.rfind("</")).map(|c| range.start + c);
                        match close {
                            Some(close) if tag_end < close => edits.push(Edit { start: tag_end + 1, end: close, text: markup }),
                            _ => {
                                warnings.push("The XFA data element could not be rewritten; the data was not imported".into());
                                return Merged::Failed;
                            }
                        }
                    }
                    _ => {
                        warnings.push("The XFA data element could not be rewritten; the data was not imported".into());
                        return Merged::Failed;
                    }
                }
            }
            Op::Picture(path, content_type, base64) => {
                let mut node = Some(data);
                for (name, idx) in path.iter() {
                    let want = xml_name(name);
                    node = node.and_then(|n| n.children().filter(|c| c.is_element() && c.tag_name().name() == want).nth(*idx));
                }
                let Some(n) = node else { return Merged::Unchanged };
                if n.children().any(|c| c.is_element()) {
                    // A data group, not a leaf: a picture would wipe what's under it.
                    skipped(warnings, STRUCTURED, &path.iter().map(|(n, i)| format!("{n}[{i}]")).collect::<Vec<_>>().join("."));
                    return Merged::Failed;
                }
                let name = qname(src, &n.range()).unwrap_or("value").to_string();
                let (ct, _) = escape(content_type);
                let attr = match xfa_prefix(n) {
                    Some(p) => format!("{p}:contentType=\"{ct}\""),
                    None => format!("xmlns:xfa=\"{XFA_DATA_NS}\" xfa:contentType=\"{ct}\""),
                };
                edits.push(Edit { start: n.range().start, end: n.range().end, text: format!("<{name} {attr}>{base64}</{name}>") });
            }
            Op::Remove(path) => {
                // The element at `path`, with the whitespace before it.
                let mut node = Some(data);
                for (name, idx) in path {
                    node = node.and_then(|n| n.children().filter(|c| c.is_element() && c.tag_name().name() == name).nth(*idx));
                }
                if let Some(n) = node {
                    let r = n.range();
                    let start = src.get(..r.start).map_or(r.start, |before| r.start - before.len() + before.trim_end().len());
                    edits.push(Edit { start, end: r.end, text: String::new() });
                }
            }
        }
    }
    if edits.is_empty() {
        return Merged::Unchanged;
    }
    // Apply from the end, so earlier positions stay valid; overlapping edits (two fields bound
    // to one node) keep the first.
    edits.sort_by_key(|e| (e.start, e.end));
    let mut kept: Vec<Edit> = Vec::with_capacity(edits.len());
    for e in edits {
        if kept.last().is_some_and(|k| e.start < k.end) {
            if kept.last().is_some_and(|k| k.text != e.text) && !warnings.iter().any(|w| w == CONFLICT) {
                warnings.push(CONFLICT.into());
            }
            continue;
        }
        kept.push(e);
    }
    let mut out = src.to_string();
    for e in kept.iter().rev() {
        if out.get(e.start..e.end).is_none() {
            warnings.push("The XFA data could not be updated; the field values were not written to it".into());
            return Merged::Failed;
        }
        out.replace_range(e.start..e.end, &e.text);
    }
    if let Some(start) = wrapper_start {
        let end = start.checked_add(WRAP_OPEN.len());
        let close = out.len().checked_sub(WRAP_CLOSE.len());
        match (end, close) {
            (Some(end), Some(close)) if end <= close && out.get(start..end) == Some(WRAP_OPEN) && out.get(close..) == Some(WRAP_CLOSE) => {
                out.truncate(close);
                out.replace_range(start..end, "");
            }
            _ => {
                warnings.push("The XFA data could not be updated; the field values were not written to it".into());
                return Merged::Failed;
            }
        }
    }
    Merged::Changed(out)
}

/// The edits that put `wants` into the `data` element.
fn merge_values(src: &str, data: roxmltree::Node, wants: &[Want], edits: &mut Vec<Edit>, warnings: &mut Vec<String>) {
    // Element children by local name, per element visited (built once each).
    let mut index: HashMap<roxmltree::NodeId, HashMap<String, Vec<roxmltree::NodeId>>> = HashMap::new();
    let tree = data.document();
    // Nodes to add, per existing element they go into.
    let mut added: Vec<(roxmltree::NodeId, Build)> = Vec::new();
    let mut budget = NewNodes::new();
    for w in wants {
        let (escaped, dropped) = escape(&w.text);
        let mut node = data;
        let mut depth = 0;
        let mut have = 0;
        for (name, idx) in &w.path {
            let kids = index.entry(node.id()).or_insert_with(|| {
                let mut by: HashMap<String, Vec<roxmltree::NodeId>> = HashMap::new();
                for c in node.children().filter(|c| c.is_element()) {
                    by.entry(c.tag_name().name().to_string()).or_default().push(c.id());
                }
                by
            });
            let same = kids.get(name.as_str());
            match same.and_then(|v| v.get(*idx)).and_then(|id| tree.get_node(*id)) {
                Some(next) => {
                    node = next;
                    depth += 1;
                }
                None => {
                    have = same.map_or(0, Vec::len);
                    break;
                }
            }
        }
        if depth == w.path.len() {
            // The data node exists: give it the value, unless it already holds it. A node
            // only wanted to exist (a row instance) is left as it is.
            if w.ensure && w.text.is_empty() {
                continue;
            }
            let current: String = node.children().filter(|c| c.is_text()).filter_map(|c| c.text()).collect();
            if node.children().any(|c| c.is_element()) {
                let all: String = node.descendants().filter(|c| c.is_text()).filter_map(|c| c.text()).collect();
                // Ignore existing XML indentation, but not whitespace in the accepted value.
                // A skipped mismatch must warn: relayout would otherwise use stale data.
                if all.trim() != w.text {
                    skipped(warnings, STRUCTURED, &w.som);
                }
                continue;
            }
            if current == w.text {
                continue;
            }
            match replace_edit(src, node, &escaped) {
                Some(e) => edits.push(e),
                None => skipped(warnings, STRUCTURED, &w.som),
            }
        } else {
            // Missing: add the rest of the path below the deepest node that exists. An empty
            // value needs no node, unless the node itself is wanted (a new instance).
            if w.text.is_empty() && !w.ensure {
                continue;
            }
            let mut rest: DataPath = w.path.get(depth..).unwrap_or_default().to_vec();
            if let Some(first) = rest.first_mut() {
                first.1 = first.1.saturating_sub(have);
            }
            let pos = match added.iter().position(|(id, _)| *id == node.id()) {
                Some(p) => p,
                None => {
                    added.push((node.id(), Build::default()));
                    added.len() - 1
                }
            };
            let built = added.get_mut(pos).and_then(|(_, b)| b.at(&rest, &mut budget));
            match built {
                Some(b) => b.text = w.text.clone(),
                None => {
                    skipped(warnings, TOO_MANY, &w.som);
                    continue;
                }
            }
        }
        if dropped {
            skipped(warnings, CONTROL, &w.som);
        }
    }
    for (id, build) in added {
        let Some(node) = tree.get_node(id) else { continue };
        let mut inner = String::new();
        build.write_children(&mut inner);
        match append_edit(src, node, &inner) {
            Some(e) => edits.push(e),
            None => warnings.push("Some XFA data nodes could not be added; their field values were not written".into()),
        }
    }
}

/// Merge `ops` into `text` in order. `None`: nothing changed (or nothing could be merged).
pub(super) fn merge_all(text: &str, within: Within, ops: &[Op], warnings: &mut Vec<String>) -> Option<String> {
    let mut current: Option<String> = None;
    for op in ops {
        match merge(current.as_deref().unwrap_or(text), within, *op, warnings) {
            Merged::Changed(t) => current = Some(t),
            Merged::Unchanged => {}
            // Later steps still apply to what merged so far; the reason is in the warnings.
            Merged::Failed => {}
        }
    }
    current
}

/// Is `text` a packet the reader will take back (well-formed, within size)? A rewrite that
/// isn't is not written: better the old data than an unreadable packet.
pub(super) fn readable(text: &str, within: Within, warnings: &mut Vec<String>) -> bool {
    if text.len() > MAX_PACKET_WRITE {
        warnings.push(format!("The XFA data would be larger than {} MB; it was not written", MAX_PACKET_WRITE >> 20));
        return false;
    }
    let opts = || roxmltree::ParsingOptions { allow_dtd: false, nodes_limit: MAX_NODES, ..Default::default() };
    let ok = roxmltree::Document::parse_with_options(text, opts()).is_ok()
        || (within == Within::Packet
            && wrapped_packet(text).is_some_and(|(wrapped, _)| {
                roxmltree::Document::parse_with_options(
                    &wrapped,
                    roxmltree::ParsingOptions { allow_dtd: false, nodes_limit: MAX_NODES, ..Default::default() },
                )
                .is_ok()
            }));
    if !ok {
        warnings.push("The XFA data would not be well-formed XML after this change; it was not written".into());
    }
    ok
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::packets::{Encoding, decode, encode};
    use pdfcraft_cos::{Dict, Document, Object, PdfString, SaveOptions, Stream, write_incremental};
    use std::sync::Arc;

    /// Install one synthetic datasets stream under an envelope that supplies its namespace.
    fn packet_document(bytes: &[u8]) -> (Document, pdfcraft_cos::ObjRef) {
        let mut doc = Document::open(Arc::new(crate::fixtures::shell(""))).unwrap();
        let mut items = Vec::new();
        let mut data_ref = None;
        for (name, data) in [
            ("preamble", b"<xdp:xdp xmlns:xdp=\"http://ns.adobe.com/xdp/\" xmlns:xfa=\"http://www.xfa.org/schema/xfa-data/1.0/\">".as_slice()),
            ("template", b"<template><subform name='form'/></template>".as_slice()),
            ("datasets", bytes),
            ("postamble", b"</xdp:xdp>".as_slice()),
        ] {
            let stream = doc.add(Object::Stream(Stream::flate(Dict::new(), data)));
            if name == "datasets" {
                data_ref = Some(stream);
            }
            items.extend([Object::String(PdfString::text(name)), Object::Ref(stream)]);
        }
        doc.update_dict(doc.root().unwrap(), |catalog| {
            let mut acro = Dict::new();
            acro.set(b"XFA".to_vec(), Object::Array(items));
            catalog.set(b"AcroForm".to_vec(), Object::Dict(acro));
        })
        .unwrap();
        let bytes = write_incremental(&doc, &SaveOptions::default()).unwrap();
        (Document::open(Arc::new(bytes)).unwrap(), data_ref.unwrap())
    }

    /// Preserve declaration spelling, unedited markup, encoding, BOM and original stream bytes
    /// while writing through the inherited-namespace fallback and reading the saved data back.
    #[test]
    fn dataset_declaration_writer_preserves_xml_and_encoding() {
        for encoding in [
            Encoding::Utf8 { bom: false },
            Encoding::Utf8 { bom: true },
            Encoding::Utf16Le { bom: false },
            Encoding::Utf16Le { bom: true },
            Encoding::Utf16Be { bom: false },
            Encoding::Utf16Be { bom: true },
        ] {
            let label = if matches!(encoding, Encoding::Utf8 { .. }) { "UTF-8" } else { "UTF-16" };
            let original = format!(
                "<?xml version = '1.0' encoding='{label}' standalone = 'yes'?>\r\n<!--kept--><?keep data?><xfa:datasets><xfa:data><form><entry a='x>y'>old-é</entry><other><![CDATA[<?xml literal?>]]></other></form></xfa:data></xfa:datasets>\n"
            );
            let bytes = encode(&original, encoding);
            let (mut doc, old_ref) = packet_document(&bytes);
            let report = crate::write_data_value(&mut doc, &crate::som_to_path("form[0].entry[0]"), "new-é").unwrap();
            assert!(report.written && report.warnings.is_empty(), "{encoding:?}: {report:?}");
            let Object::Stream(stream) = &*doc.get(report.stream.unwrap()) else { panic!("datasets stream") };
            let (text, actual_encoding) = decode(&stream.decoded().unwrap()).unwrap();
            assert_eq!(actual_encoding, encoding);
            assert_eq!(text, original.replace("old-é", "new-é"));
            assert!(!text.contains("pdfcraft-wrap") && !text.contains("xmlns:xfa"));
            let Object::Stream(old) = &*doc.get(old_ref) else { panic!("original stream") };
            assert_eq!(old.decoded().unwrap(), bytes);
            let saved = Document::open(Arc::new(write_incremental(&doc, &SaveOptions::default()).unwrap())).unwrap();
            let xdp = crate::read_packets(&saved).unwrap().unwrap().xdp;
            assert_eq!(crate::parse_datasets(&xdp).unwrap().text_at(&crate::som_to_path("form[0].entry[0]")), Some("new-é"));
        }
    }

    /// Invalid declarations, misplaced declarations and broken markup must warn and retain the
    /// original stream; the final validation must also refuse each packet.
    #[test]
    fn dataset_declaration_writer_rejects_invalid_packets_without_replacing_stream() {
        for text in [
            "<?xml?><xfa:datasets/>",
            "<?xml version='1.0' encoding='bad encoding'?><xfa:datasets/>",
            "<?xml version='1.0'<xfa:datasets/>",
            " <!--comment--><?xml version='1.0'?><xfa:datasets/>",
            "<?xml version='1.0'?><xfa:datasets><broken></xfa:datasets>",
        ] {
            let (mut doc, original) = packet_document(text.as_bytes());
            let report = crate::write_data_value(&mut doc, &crate::som_to_path("form[0].entry[0]"), "new").unwrap();
            assert!(!report.written && report.stream.is_none() && !report.warnings.is_empty());
            assert_eq!(super::super::write::datasets_stream_ref(&doc), Some(original));
            let Object::Stream(stream) = &*doc.get(original) else { panic!("original stream") };
            assert_eq!(stream.decoded().unwrap(), text.as_bytes());
            assert!(!readable(text, Within::Packet, &mut Vec::new()));
        }
    }

    /// Refuse oversized output before allocating a namespace wrapper or parsing it.
    #[test]
    fn dataset_declaration_writer_rejects_oversized_output() {
        let text = " ".repeat(MAX_PACKET_WRITE + 1);
        let mut warnings = Vec::new();
        assert!(!readable(&text, Within::Packet, &mut warnings));
        assert_eq!(warnings.len(), 1);
        let original = b"<?xml version='1.0'?><xfa:datasets><xfa:data><form><entry>old</entry></form></xfa:data></xfa:datasets>";
        let (mut doc, original_ref) = packet_document(original);
        let report = crate::write_data_value(&mut doc, &crate::som_to_path("form[0].entry[0]"), &text).unwrap();
        assert!(!report.written && report.stream.is_none() && !report.warnings.is_empty());
        assert_eq!(super::super::write::datasets_stream_ref(&doc), Some(original_ref));
        let Object::Stream(stream) = &*doc.get(original_ref) else { panic!("original stream") };
        assert_eq!(stream.decoded().unwrap(), original);
    }
}
