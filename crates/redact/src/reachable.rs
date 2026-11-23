//! The last word on what Apply removed (#7885): an annotation or form field it took off the pages
//! and out of the form must not stay in the file. A full save writes whatever the trailer reaches
//! by any reference, and the removal steps clean only the references they know. Form actions name
//! their targets by reference too: a ResetForm or SubmitForm action's `/Fields` and a Hide
//! action's `/T` (ISO 32000-2 §12.7.6.2-3, §12.6.4.10). So does a field's `/Kids`, which can list a
//! widget or field that names no `/Parent` and so wasn't taken out. Those references are dropped
//! from every object a full save would write; any other reference that still keeps a removed
//! object refuses the redaction, naming the object that holds it.

use std::cell::Cell;
use std::collections::{HashMap, HashSet};

use pdfcraft_cos::{Dict, Document, ObjRef, Object};

use crate::RedactError;

/// How deep objects written in place are followed looking for actions. A deeper action is left as
/// it is; the check after the cleanup still finds what it keeps.
const MAX_DEPTH: usize = 64;

/// How many `/Next` entries one pass follows past actions it drops. Actions shared between chains
/// are followed once per chain, so a file can multiply them; past this, the actions after a
/// dropped one go with it.
const MAX_SUCCESSORS: usize = 1 << 16;

/// Drop the references form actions and fields' `/Kids` hold to `removed`, then refuse if anything
/// else a full save would write still reaches one of them. A field whose `/Kids` held only removed
/// objects is deleted too, as the form deletes an ancestor left without kids: it would keep the
/// value of the field the redaction removed. Costs one walk of the reachable objects when nothing
/// refers to them, and a few more when something does.
pub(crate) fn drop_references(doc: &mut Document, removed: &HashSet<ObjRef>) -> Result<(), RedactError> {
    if removed.is_empty() {
        return Ok(());
    }
    let mut removed = removed.clone();
    // Each round deletes the fields the one before left without kids, one level further up.
    for _ in 0..=MAX_DEPTH {
        let childless = drop_once(doc, &removed)?;
        let mut deleted = Vec::new();
        for field in childless {
            if pdfcraft_forms::fields(doc).iter().any(|f| f.obj == field) {
                deleted.extend(crate::field_and_ancestors(doc, field));
                pdfcraft_forms::delete_field_object(doc, field)?;
            }
        }
        if deleted.is_empty() {
            break;
        }
        // An ancestor the form keeps (it has other kids) is not removed.
        let in_form: HashSet<ObjRef> = pdfcraft_forms::fields(doc).iter().flat_map(|f| crate::field_and_ancestors(doc, f.obj)).collect();
        removed.extend(deleted.into_iter().filter(|f| !in_form.contains(f)));
    }
    refuse_kept(doc, &removed)
}

/// One pass of [`drop_references`] over the objects a full save would write: the fields whose
/// `/Kids` it emptied.
fn drop_once(doc: &mut Document, removed: &HashSet<ObjRef>) -> Result<Vec<ObjRef>, RedactError> {
    let reachable = pdfcraft_cos::reachable_objects(doc)?;
    if !removed.iter().any(|r| reachable.contains(&r.num)) {
        return Ok(Vec::new());
    }
    let mut order: Vec<u32> = reachable.into_iter().collect();
    order.sort_unstable();
    let mut cleaner = Cleaner { doc, removed, emptied: HashMap::new(), followed: Cell::new(0) };
    for &num in &order {
        if let Some(d) = doc.get(ObjRef::new(num, doc.generation(num))).as_dict()
            && cleaner.acts_on_none(d)
        {
            cleaner.emptied.insert(num, d.get(b"Next").cloned());
        }
    }
    let mut changed = Vec::new();
    let mut childless = Vec::new();
    for &num in &order {
        let r = ObjRef::new(num, doc.generation(num));
        let object = doc.get(r);
        if let Some(new) = cleaner.object(&object, 0) {
            if kids(doc, &object).is_some_and(|n| n > 0) && kids(doc, &new) == Some(0) {
                childless.push(r);
            }
            changed.push((r, new));
        }
    }
    // An object whose xref and header generations disagree is reached by either: replace it under
    // both, or a reference by the other one would read null and drop what it names.
    for (r, object) in changed {
        doc.set_existing(r, object)?;
    }
    Ok(childless)
}

/// How many kids a field (or page tree node) `o` lists.
fn kids(doc: &Document, o: &Object) -> Option<usize> {
    let kids = o.as_dict()?.get(b"Kids")?;
    doc.resolve(kids).as_array().map(Vec::len)
}

/// Refuse if anything a full save would write still reaches an object in `removed`.
fn refuse_kept(doc: &Document, removed: &HashSet<ObjRef>) -> Result<(), RedactError> {
    let reachable = pdfcraft_cos::reachable_objects(doc)?;
    let mut kept: Vec<u32> = removed.iter().map(|r| r.num).filter(|n| reachable.contains(n)).collect();
    kept.sort_unstable();
    let Some(&still) = kept.first() else { return Ok(()) };
    let mut order: Vec<u32> = reachable.into_iter().collect();
    order.sort_unstable();
    let by = order.into_iter().find(|&num| num != still && names(&doc.get(ObjRef::new(num, doc.generation(num))), still, 0));
    Err(RedactError::StillReferred { removed: still, by })
}

/// What one pass drops: the removed objects, and the indirect actions left acting on no field.
struct Cleaner<'a> {
    doc: &'a Document,
    removed: &'a HashSet<ObjRef>,
    /// Indirect actions that reset or submit only removed fields, by number, with their `/Next`.
    emptied: HashMap<u32, Option<Object>>,
    /// `/Next` entries followed so far (see [`MAX_SUCCESSORS`]).
    followed: Cell<usize>,
}

impl Cleaner<'_> {
    fn gone(&self, o: &Object) -> bool {
        o.as_ref().is_some_and(|r| self.removed.contains(&r))
    }

    /// `o` without the references to removed objects it can lose, or `None` when it has none.
    /// Streams are left as they are: an action is a dictionary of its own.
    fn object(&self, o: &Object, depth: usize) -> Option<Object> {
        if depth > MAX_DEPTH {
            return None;
        }
        match o {
            Object::Array(items) => self.actions(items, depth).map(Object::Array),
            Object::Dict(d) => self.dict(d, depth).map(Object::Dict),
            _ => None,
        }
    }

    /// `items` cleaned, an action left acting on no field replaced by the actions after it, or
    /// `None` when nothing changed.
    fn actions(&self, items: &[Object], depth: usize) -> Option<Vec<Object>> {
        let mut changed = false;
        let mut out = Vec::with_capacity(items.len());
        for item in items {
            if let Some(next) = self.emptied_next(item) {
                changed = true;
                out.extend(self.successors(next.as_ref(), depth + 1));
            } else if let Some(new) = self.object(item, depth + 1) {
                changed = true;
                out.push(new);
            } else {
                out.push(item.clone());
            }
        }
        changed.then_some(out)
    }

    /// [`Self::object`] for a dictionary: an action's own targets and a field's kids, then whatever
    /// it holds in place (a `/Next` action, the actions of an `/AA` dictionary, an annotation's `/A`).
    fn dict(&self, d: &Dict, depth: usize) -> Option<Dict> {
        let targets: Option<&[u8]> = match d.name(b"S") {
            Some(b"ResetForm" | b"SubmitForm") => Some(b"Fields"),
            Some(b"Hide") => Some(b"T"),
            _ => None,
        };
        let mut out: Option<Dict> = None;
        for key in targets.into_iter().chain([b"Kids".as_slice()]) {
            if let Some(value) = d.get(key)
                && let Some(kept) = self.list_without(value)
            {
                out.get_or_insert_with(|| d.clone()).set(key.to_vec(), kept);
            }
        }
        for (key, value) in d.iter() {
            if targets == Some(key.as_slice()) || key.as_slice() == b"Kids" {
                continue;
            }
            if let Some(next) = self.emptied_next(value) {
                let actions = self.successors(next.as_ref(), depth + 1);
                let out = out.get_or_insert_with(|| d.clone());
                if key.as_slice() == b"Next" && actions.len() > 1 {
                    out.set(key.clone(), Object::Array(actions));
                } else if let Some(action) = self.one_action(actions, depth) {
                    out.set(key.clone(), action);
                } else {
                    out.remove(key);
                }
            } else if let Some(new) = self.object(value, depth + 1) {
                out.get_or_insert_with(|| d.clone()).set(key.clone(), new);
            }
        }
        out
    }

    /// A list of references without those in `removed`, or `None` when it names none of them. The
    /// list is one reference or an array of them (possibly held by reference), mixed with field
    /// names, which stay. A lone removed target becomes an empty array, which names nothing:
    /// removing a Hide action's `/T` would leave it invalid.
    fn list_without(&self, value: &Object) -> Option<Object> {
        match &*self.doc.resolve(value) {
            Object::Array(items) if items.iter().any(|o| self.gone(o)) => {
                Some(Object::Array(items.iter().filter(|o| !self.gone(o)).cloned().collect()))
            }
            Object::Array(_) => None,
            _ => self.gone(value).then(|| Object::Array(Vec::new())),
        }
    }

    /// Whether `d` resets or submits only fields the redaction removed. Its `/Fields` would be
    /// empty, which readers (ours too) may take for every field, as an omitted one means (ISO
    /// 32000-2 §12.7.6.3), so the action goes and the actions after it run in its place. A list of
    /// fields to leave out (Include/Exclude flag set) loses nothing by emptying.
    fn acts_on_none(&self, d: &Dict) -> bool {
        if !matches!(d.name(b"S"), Some(b"ResetForm" | b"SubmitForm")) {
            return false;
        }
        if d.get(b"Flags").and_then(|f| self.doc.resolve(f).as_int()).is_some_and(|f| f & 1 != 0) {
            return false;
        }
        let Some(v) = d.get(b"Fields") else { return false };
        let listed = self.doc.resolve(v);
        let entries = listed.as_array().map_or(std::slice::from_ref(v), Vec::as_slice);
        !entries.is_empty() && entries.iter().all(|e| self.gone(e))
    }

    /// For an action that [`Self::acts_on_none`], held in place or by reference, its `/Next`.
    fn emptied_next(&self, o: &Object) -> Option<Option<Object>> {
        match o {
            Object::Ref(r) => self.emptied.get(&r.num).cloned(),
            Object::Dict(d) if self.acts_on_none(d) => Some(d.get(b"Next").cloned()),
            _ => None,
        }
    }

    /// The actions a `/Next` entry runs, cleaned as [`Self::actions`] cleans them.
    fn successors(&self, next: Option<&Object>, depth: usize) -> Vec<Object> {
        let Some(next) = next.filter(|_| depth <= MAX_DEPTH && self.followed.get() < MAX_SUCCESSORS) else { return Vec::new() };
        self.followed.set(self.followed.get() + 1);
        let items = match &*self.doc.resolve(next) {
            Object::Array(items) => items.clone(),
            _ => vec![next.clone()],
        };
        self.actions(&items, depth).unwrap_or(items)
    }

    /// One action that runs `actions` in order, for an entry that holds one action (`/A`, an `/AA`
    /// trigger, `/OpenAction`): the first, with the rest after its own `/Next`.
    fn one_action(&self, actions: Vec<Object>, depth: usize) -> Option<Object> {
        let mut actions = actions.into_iter();
        loop {
            let first = actions.next()?;
            let rest: Vec<Object> = actions.clone().collect();
            if rest.is_empty() {
                return Some(first);
            }
            let Some(head) = self.doc.resolve(&first).as_dict().cloned() else { continue };
            let mut head = self.dict(&head, depth + 1).unwrap_or(head);
            let mut next = match head.get(b"Next") {
                Some(n) => match &*self.doc.resolve(n) {
                    Object::Array(items) => items.clone(),
                    _ => vec![n.clone()],
                },
                None => Vec::new(),
            };
            next.extend(rest);
            head.set(b"Next".to_vec(), Object::Array(next));
            return Some(Object::Dict(head));
        }
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
