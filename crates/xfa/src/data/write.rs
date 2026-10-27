use super::build::{MAX_INDEX, build_children, fresh_datasets};
use super::merge::{Op, Within, merge_all, readable};
use super::path::path_som;
use super::values::{FieldDatum, Want, wanted, with_template};
use super::*;

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
fn packet_text(doc: &Document, o: &Object, warnings: &mut Vec<String>) -> Option<(String, Encoding)> {
    let s = doc.resolve(o);
    let Object::Stream(s) = &*s else {
        warnings.push("The XFA data packet is not a stream; the field values were not written to it".into());
        return None;
    };
    let bytes = match s.decoded_within(64 << 20) {
        Ok(b) => b,
        Err(e) => {
            warnings.push(format!("The XFA data could not be decoded ({e}); the field values were not written to it"));
            return None;
        }
    };
    let decoded = crate::packets::decode(&bytes);
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
    match old.and_then(Object::as_ref) {
        Some(r) if reuse == Some(r) || doc.is_edited(r.num) => {
            doc.set(r, stream);
            r
        }
        _ => doc.add(stream),
    }
}

/// Longest packet text written (the packets together are read up to 64 MB).
pub(super) const MAX_PACKET_WRITE: usize = 48 << 20;

pub(super) fn rewrite(doc: &mut Document, ops: &[Op], reuse: Option<ObjRef>) -> Result<DatasetsWrite, XfaError> {
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
            let at = (0..items.len())
                .step_by(2)
                .find(|&i| items.get(i).and_then(|n| n.as_string()).is_some_and(|s| s.to_text() == "datasets") && i + 1 < items.len());
            match at {
                Some(i) => {
                    let Some(existing) = items.get(i + 1).cloned() else { return Ok(report) };
                    let Some((text, encoding)) = packet_text(doc, &existing, warnings) else { return Ok(report) };
                    let Some(t) = merge_all(&text, Within::Packet, ops, warnings) else { return Ok(report) };
                    if !readable(&t, Within::Packet, warnings) {
                        return Ok(report);
                    }
                    let r = put_stream(doc, Some(&existing), reuse, &encode(&t, encoding));
                    report.stream = Some(r);
                    if let Some(slot) = items.get_mut(i + 1) {
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
                    let r = doc.add(Object::Stream(Stream::flate(Dict::new(), text.as_bytes())));
                    report.stream = Some(r);
                    let pos = items.iter().position(|n| n.as_string().is_some_and(|s| s.to_text() == "postamble")).unwrap_or(items.len());
                    items.insert(pos, Object::Ref(r));
                    items.insert(pos, Object::String(PdfString::text("datasets")));
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
            let r = put_stream(doc, Some(&xfa), reuse, &encode(&new, encoding));
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
            let at = (0..items.len())
                .step_by(2)
                .find(|&i| items.get(i).and_then(|n| n.as_string()).is_some_and(|s| s.to_text() == "datasets") && i + 1 < items.len())?;
            items.get(at + 1)?.as_ref()
        }
        Object::Stream(_) => xfa.as_ref(),
        _ => None,
    }
}
