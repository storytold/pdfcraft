//! The last word on what Apply removed (#7885): an annotation or form field it took off the pages
//! and out of the form must not stay in the file. A full save writes whatever the trailer reaches
//! by any reference, and the removal steps clean only the references they know. Form actions name
//! their targets by reference too: a ResetForm or SubmitForm action's `/Fields` and a Hide
//! action's `/T` (ISO 32000-2 §12.7.6.2-3, §12.6.4.10). Those references are dropped from every
//! object a full save would write; any other reference that still keeps a removed object refuses
//! the redaction, naming the object that holds it.

use std::collections::HashSet;

use pdfcraft_cos::{Dict, Document, ObjRef, Object};

use crate::RedactError;

/// How deep objects written in place are followed looking for actions. A deeper action is left as
/// it is; the check after the cleanup still finds what it keeps.
const MAX_DEPTH: usize = 64;

/// Drop the references form actions hold to `removed`, then refuse if anything else a full save
/// would write still reaches one of them. Costs one walk of the reachable objects when nothing
/// refers to them, and two more walks of the objects when something does.
pub(crate) fn drop_action_targets(doc: &mut Document, removed: &HashSet<ObjRef>) -> Result<(), RedactError> {
    if removed.is_empty() {
        return Ok(());
    }
    let numbers: HashSet<u32> = removed.iter().map(|r| r.num).collect();
    let reachable = pdfcraft_cos::reachable_objects(doc)?;
    if !numbers.iter().any(|n| reachable.contains(n)) {
        return Ok(());
    }
    let mut order: Vec<u32> = reachable.into_iter().collect();
    order.sort_unstable();
    let mut changed = Vec::new();
    for &num in &order {
        let r = ObjRef::new(num, doc.generation(num));
        if let Some(object) = without_targets(doc, &doc.get(r), removed, 0) {
            changed.push((r, object));
        }
    }
    for (r, object) in changed {
        doc.set(r, object);
    }
    let reachable = pdfcraft_cos::reachable_objects(doc)?;
    let mut kept: Vec<u32> = numbers.into_iter().filter(|n| reachable.contains(n)).collect();
    kept.sort_unstable();
    let Some(&still) = kept.first() else { return Ok(()) };
    let mut order: Vec<u32> = reachable.into_iter().collect();
    order.sort_unstable();
    let by = order.into_iter().find(|&num| num != still && names(&doc.get(ObjRef::new(num, doc.generation(num))), still, 0));
    Err(RedactError::StillReferred { removed: still, by })
}

/// `o` with the action targets in `removed` dropped, or `None` when it has none. Streams are left
/// as they are: an action is a dictionary of its own.
fn without_targets(doc: &Document, o: &Object, removed: &HashSet<ObjRef>, depth: usize) -> Option<Object> {
    if depth > MAX_DEPTH {
        return None;
    }
    match o {
        Object::Array(items) => {
            let mut changed = false;
            let out: Vec<Object> = items
                .iter()
                .map(|item| match without_targets(doc, item, removed, depth + 1) {
                    Some(new) => {
                        changed = true;
                        new
                    }
                    None => item.clone(),
                })
                .collect();
            changed.then_some(Object::Array(out))
        }
        Object::Dict(d) => dict_without_targets(doc, d, removed, depth).map(Object::Dict),
        _ => None,
    }
}

/// [`without_targets`] for a dictionary: an action's own targets, then whatever it holds in place
/// (a `/Next` action, the actions of an `/AA` dictionary, an annotation's `/A`).
fn dict_without_targets(doc: &Document, d: &Dict, removed: &HashSet<ObjRef>, depth: usize) -> Option<Dict> {
    let targets: Option<&[u8]> = match d.name(b"S") {
        Some(b"ResetForm" | b"SubmitForm") => Some(b"Fields"),
        Some(b"Hide") => Some(b"T"),
        _ => None,
    };
    let mut out: Option<Dict> = None;
    if let Some(key) = targets
        && let Some(value) = d.get(key)
        && let Some(kept) = targets_without(doc, value, removed)
    {
        out.get_or_insert_with(|| d.clone()).set(key.to_vec(), kept);
    }
    for (key, value) in d.iter() {
        if targets == Some(key.as_slice()) {
            continue;
        }
        if let Some(new) = without_targets(doc, value, removed, depth + 1) {
            out.get_or_insert_with(|| d.clone()).set(key.clone(), new);
        }
    }
    out
}

/// An action's targets without those in `removed`, or `None` when it names none of them. Targets
/// are one reference or an array of them (possibly held by reference), mixed with field names,
/// which stay. A lone removed target becomes an empty array, which names nothing: removing the
/// key would make ResetForm and SubmitForm act on every field.
fn targets_without(doc: &Document, value: &Object, removed: &HashSet<ObjRef>) -> Option<Object> {
    let gone = |o: &Object| o.as_ref().is_some_and(|r| removed.contains(&r));
    match &*doc.resolve(value) {
        Object::Array(items) if items.iter().any(gone) => Some(Object::Array(items.iter().filter(|o| !gone(o)).cloned().collect())),
        Object::Array(_) => None,
        _ => gone(value).then(|| Object::Array(Vec::new())),
    }
}

/// Whether `o` refers to object `num` itself or through what it holds in place.
fn names(o: &Object, num: u32, depth: usize) -> bool {
    if depth > MAX_DEPTH {
        return false;
    }
    match o {
        Object::Ref(r) => r.num == num,
        Object::Array(items) => items.iter().any(|item| names(item, num, depth + 1)),
        Object::Dict(d) => d.iter().any(|(_, v)| names(v, num, depth + 1)),
        Object::Stream(s) => s.dict.iter().any(|(_, v)| names(v, num, depth + 1)),
        _ => false,
    }
}
