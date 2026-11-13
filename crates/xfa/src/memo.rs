//! What the crate keeps alongside a parsed template for as long as it lives, so that the work of
//! an event doesn't grow with the property changes the form already holds:
//!
//! - the property changes last read from a document of the form
//!   ([`crate::script::overrides_of`]), with the object they were read from: an event whose
//!   document holds the same object takes them from here instead of checking every one of them
//!   again;
//! - the property trees those changes give the template's objects ([`applied`]): the live form
//!   is built for each event, and handing back the tree it had keeps the script engine's copy of
//!   it too (that copy is kept by tree);
//! - the fields those changes give the template's fields when they change more than the tree
//!   (a validation, a picture, items: [`restyled_field`]), for the same reason, and because
//!   making one reads the field again.
//! - who owns each SOM path the live form names ([`crate::script::SomOwners`]), which depends
//!   on the template alone, so that telling whether a path names an object the template makes
//!   once ([`crate::som_made_once`]) costs a few look-ups each time it is asked.
//!
//! None is part of the template: a clone starts empty, and two compare equal. A poisoned lock
//! only means a panic elsewhere while it was held; all are memos, so their contents stay valid.

use std::collections::{BTreeMap, HashMap};
use std::sync::{Arc, Mutex, PoisonError, Weak};

use pdfcraft_cos::Object;

use crate::model::{Field, Props, Value};

/// The property changes as kept, by SOM path then property key (see
/// [`crate::script::Overrides::props`]).
pub(crate) type PropMap = BTreeMap<String, BTreeMap<String, String>>;

/// The property changes read from one object of a document: what was kept of them, how many
/// changes and bytes that is, and how many were not kept.
#[derive(Clone)]
pub(crate) struct KeptProps {
    pub(crate) props: Arc<PropMap>,
    pub(crate) count: usize,
    pub(crate) copies: usize,
    pub(crate) bytes: usize,
    pub(crate) dropped: usize,
}

/// Most trees [`applied`] keeps in one generation, and most bytes they hold (see
/// [`tree_bytes`]): past either, the generation becomes the old one and the one before it goes.
/// A form keeps at most [`crate::restyle::MAX_PROP_OVERRIDES`] changes, so one generation holds
/// a tree for every changed object of a form; one build of the live form asks for at most that
/// many, so what it needs is in the newest generation or the one before it. Two generations
/// bound what is kept when a script changes the same object to new values event after event.
const MAX_APPLIED: usize = 20_000;
pub(crate) const MAX_APPLIED_BYTES: usize = 16 << 20;

/// One result [`applied`] (or [`restyled_field`]) made: the template object's tree it was
/// made from (to tell a tree that is gone from one now at its address), the changes, and the
/// result.
type AppliedEntry<T> = (Weak<Props>, BTreeMap<String, String>, Arc<T>);

/// Results [`applied`] made, by the address of the tree they were made from and a hash of the
/// changes, so a look-up compares only the entries with that tree and those changes (and the
/// rare collision), however many trees one template object has.
type AppliedMap<T> = HashMap<(usize, u64), Vec<AppliedEntry<T>>>;

/// The two generations of [`applied`]'s results, and how many results and bytes the newest
/// holds.
struct Applied<T> {
    new: AppliedMap<T>,
    old: AppliedMap<T>,
    trees: usize,
    bytes: usize,
}

impl<T> Default for Applied<T> {
    fn default() -> Self {
        Applied { new: HashMap::new(), old: HashMap::new(), trees: 0, bytes: 0 }
    }
}

/// See the module documentation.
#[derive(Default)]
pub struct Memo {
    /// The object the property changes were last read from, and what was read.
    kept: Mutex<Option<(Arc<Object>, KeptProps)>>,
    applied: Mutex<Applied<Props>>,
    fields: Mutex<Applied<Field>>,
    /// Who owns each SOM path the live form names ([`crate::script::SomOwners`]), made the first
    /// time it is asked. Shared with those who ask away from the template
    /// ([`crate::SomPaths`]).
    som_owners: std::sync::OnceLock<Arc<crate::script::SomOwners>>,
}

impl Clone for Memo {
    fn clone(&self) -> Self {
        Memo::default()
    }
}

impl PartialEq for Memo {
    fn eq(&self, _: &Self) -> bool {
        true
    }
}

impl std::fmt::Debug for Memo {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Memo")
    }
}

impl Memo {
    /// What was read from `object` when it was last read, if it is the object last read (the
    /// same object, or one equal to it: a document's objects are read from its file again
    /// after an edit).
    pub(crate) fn kept(&self, object: &Arc<Object>) -> Option<KeptProps> {
        let kept = self.kept.lock().unwrap_or_else(PoisonError::into_inner);
        kept.as_ref().filter(|(o, _)| Arc::ptr_eq(o, object) || **o == **object).map(|(_, k)| k.clone())
    }

    /// `kept` was read from `object`.
    pub(crate) fn read(&self, object: &Arc<Object>, kept: KeptProps) {
        *self.kept.lock().unwrap_or_else(PoisonError::into_inner) = Some((Arc::clone(object), kept));
    }

    /// Who owns each SOM path the live form of `tpl` (the template holding this memo) names.
    pub(crate) fn som_owners(&self, tpl: &crate::model::Template) -> &Arc<crate::script::SomOwners> {
        self.som_owners.get_or_init(|| Arc::new(crate::script::SomOwners::of(tpl)))
    }
}

#[cfg(test)]
thread_local! {
    /// Property trees [`applied`] made (rather than handed back), on this thread.
    static APPLIED_MADE: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
    /// Fields [`restyled_field`] made (rather than handed back), on this thread.
    static FIELDS_MADE: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

/// How many property trees [`applied`] has made on this thread, since the last call.
#[cfg(test)]
pub(crate) fn take_applied_made() -> usize {
    APPLIED_MADE.with(|c| c.replace(0))
}

/// How many fields [`restyled_field`] has made on this thread, since the last call.
#[cfg(test)]
pub(crate) fn take_fields_made() -> usize {
    FIELDS_MADE.with(|c| c.replace(0))
}

/// Bytes a property tree holds, roughly: each element and attribute, and their text.
pub(crate) fn tree_bytes(props: &Props) -> usize {
    fn prop(p: &crate::model::Prop, depth: usize) -> usize {
        let own = std::mem::size_of::<crate::model::Prop>()
            .saturating_add(p.tag.len())
            .saturating_add(p.attrs.iter().fold(0usize, |n, (k, v)| n.saturating_add(k.len()).saturating_add(v.len()).saturating_add(48)))
            .saturating_add(p.text.as_deref().map_or(0, str::len));
        if depth > crate::parse::MAX_PROP_DEPTH.saturating_add(4) {
            return own;
        }
        p.children.iter().fold(own, |n, c| n.saturating_add(prop(c, depth + 1)))
    }
    props
        .attrs
        .iter()
        .fold(std::mem::size_of::<Props>(), |n, (k, v)| n.saturating_add(k.len()).saturating_add(v.len()).saturating_add(48))
        .saturating_add(props.elements.iter().fold(0usize, |n, p| n.saturating_add(prop(p, 0))))
}

/// A hash of `sets`, for [`applied`]'s key.
pub(crate) fn sets_hash(sets: &BTreeMap<String, String>) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    sets.hash(&mut h);
    h.finish()
}

#[cfg(test)]
thread_local! {
    /// Entries [`applied`] compared looking for a tree, on this thread.
    static APPLIED_COMPARED: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

/// How many entries [`applied`] has compared on this thread, since the last call.
#[cfg(test)]
pub(crate) fn take_applied_compared() -> usize {
    APPLIED_COMPARED.with(|c| c.replace(0))
}

/// How many trees and bytes the memo holds, both generations together.
#[cfg(test)]
pub(crate) fn applied_held(memo: &Memo) -> (usize, usize) {
    let a = memo.applied.lock().unwrap_or_else(PoisonError::into_inner);
    let trees = |m: &AppliedMap<Props>| m.values().map(Vec::len).sum::<usize>();
    let bytes = |m: &AppliedMap<Props>| m.values().flatten().map(|(_, s, t)| tree_bytes(t).saturating_add(set_bytes(s))).sum::<usize>();
    (trees(&a.new) + trees(&a.old), bytes(&a.new) + bytes(&a.old))
}

/// How many bytes the newest generation of fields [`restyled_field`] made holds.
#[cfg(test)]
pub(crate) fn fields_bytes(memo: &Memo) -> usize {
    memo.fields.lock().unwrap_or_else(PoisonError::into_inner).bytes
}

/// Bytes of the changes `sets`.
pub(crate) fn set_bytes(sets: &BTreeMap<String, String>) -> usize {
    sets.iter().fold(0usize, |n, (k, v)| n.saturating_add(k.len()).saturating_add(v.len()).saturating_add(48))
}

/// `base` with the changes `sets` applied ([`crate::restyle::apply_props`]), the same `Arc` each
/// time the template's memo is asked for the same tree and changes. Kept by the tree's
/// address, checked against the tree itself so a freed tree's address can't match; at most
/// two generations of trees are kept (see [`MAX_APPLIED`]).
pub(crate) fn applied(memo: &Memo, base: &Arc<Props>, sets: &BTreeMap<String, String>) -> Arc<Props> {
    remembered(&memo.applied, base, sets, tree_bytes, || {
        #[cfg(test)]
        APPLIED_MADE.with(|c| c.set(c.get().saturating_add(1)));
        crate::restyle::apply_props(base, sets).0
    })
}

/// Template field `f` with the changes `sets` applied ([`crate::restyle::restyled_field`]),
/// the same `Arc` each time the template's memo is asked for the same field and changes: the
/// live form takes such a field's validation, pictures and items from it, and building the form
/// again for each event doesn't read the field again. Kept by the field's property tree, which
/// is its own (the reader makes one for each object), as [`applied`] keeps trees.
pub(crate) fn restyled_field(memo: &Memo, f: &Field, sets: &BTreeMap<String, String>) -> Arc<Field> {
    remembered(&memo.fields, &f.common.props, sets, field_bytes, || {
        #[cfg(test)]
        FIELDS_MADE.with(|c| c.set(c.get().saturating_add(1)));
        crate::restyle::restyled_field(f, sets)
    })
}

/// Bytes a field holds, roughly: its property tree, and what it holds of its own beside it.
pub(crate) fn field_bytes(f: &Field) -> usize {
    let scripts = f.scripts.iter().chain(&f.calculate).chain(&f.validate).chain(&f.validate_off).fold(0usize, |n, s| n.saturating_add(s.text.len()));
    // Its lists are its own copies (an item list holds up to `parse::MAX_ITEMS` entries,
    // outside the object's property budget), so they count too, as do its texts.
    let strings = f.items.iter().chain(&f.item_values).fold(0usize, |n, i| n.saturating_add(i.len()).saturating_add(std::mem::size_of::<String>()));
    let texts = [&f.tooltip, &f.picture, &f.data_picture, &f.validate_message, &f.format_picture, &f.format_message, &f.display_picture]
        .into_iter()
        .fold(0usize, |n, t| n.saturating_add(t.as_ref().map_or(0, String::len)));
    // So is its value (a default text, a picture's bytes), and its caption's.
    let values = value_bytes(&f.value).saturating_add(f.caption.as_ref().map_or(0, |c| value_bytes(&c.value)));
    tree_bytes(&f.common.props)
        .saturating_add(std::mem::size_of::<Field>())
        .saturating_add(scripts)
        .saturating_add(strings)
        .saturating_add(texts)
        .saturating_add(values)
}

/// Bytes a value holds of its own, roughly: its text, its rich text's runs, its picture.
pub(crate) fn value_bytes(v: &Value) -> usize {
    match v {
        Value::Text(t) => t.len(),
        Value::Rich(r) => r.paragraphs.iter().flat_map(|p| &p.runs).fold(0usize, |n, run| {
            n.saturating_add(std::mem::size_of::<crate::model::Run>())
                .saturating_add(run.text.len())
                .saturating_add(run.embed.as_ref().map_or(0, String::len))
        }),
        Value::Image { content_type, data, href } => {
            data.len().saturating_add(content_type.len()).saturating_add(href.as_ref().map_or(0, String::len))
        }
        Value::Empty | Value::Line { .. } | Value::Rectangle(_) => 0,
    }
}

/// What `make` gives for the template object whose tree is `base` and the changes `sets`, from
/// `memo` when it holds it, else made and kept there (see [`applied`]); `cost` is the bytes a
/// result holds.
fn remembered<T>(
    memo: &Mutex<Applied<T>>,
    base: &Arc<Props>,
    sets: &BTreeMap<String, String>,
    cost: impl Fn(&T) -> usize,
    make: impl FnOnce() -> T,
) -> Arc<T> {
    let key = (Arc::as_ptr(base) as usize, sets_hash(sets));
    let same_base = |w: &Weak<Props>| w.upgrade().is_some_and(|b| Arc::ptr_eq(&b, base));
    let find = |map: &AppliedMap<T>| {
        map.get(&key).and_then(|list| {
            list.iter()
                .find(|(w, s, _)| {
                    #[cfg(test)]
                    APPLIED_COMPARED.with(|c| c.set(c.get().saturating_add(1)));
                    same_base(w) && s == sets
                })
                .map(|(_, _, out)| Arc::clone(out))
        })
    };
    let mut a = memo.lock().unwrap_or_else(PoisonError::into_inner);
    if let Some(out) = find(&a.new) {
        return out;
    }
    // Made in the generation before: kept in the newest one from now on.
    let out = match find(&a.old) {
        Some(out) => out,
        None => Arc::new(make()),
    };
    let cost = cost(&out).saturating_add(set_bytes(sets));
    // One past a generation's budget by itself is not kept (it is made again when next asked
    // for), so what the two generations hold stays within twice the budget.
    if cost > MAX_APPLIED_BYTES {
        return out;
    }
    if a.trees >= MAX_APPLIED || a.bytes.saturating_add(cost) > MAX_APPLIED_BYTES {
        a.old = std::mem::take(&mut a.new);
        a.trees = 0;
        a.bytes = 0;
    }
    a.trees = a.trees.saturating_add(1);
    a.bytes = a.bytes.saturating_add(cost);
    let list = a.new.entry(key).or_default();
    list.retain(|(w, _, _)| same_base(w));
    list.push((Arc::downgrade(base), sets.clone(), Arc::clone(&out)));
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A form of `n` fields and its document, each field with a kept tooltip change.
    fn form_with_kept_changes(n: usize) -> (pdfcraft_cos::Document, crate::model::Template, crate::script::Overrides) {
        let fields: String = (0..n).map(|i| format!(r#"<field name="f{i}" w="2in" h="0.3in"><ui><textEdit/></ui></field>"#)).collect();
        let xdp = format!(
            r#"<?xml version="1.0" encoding="UTF-8"?>
<xdp:xdp xmlns:xdp="http://ns.adobe.com/xdp/">
<template xmlns="http://www.xfa.org/schema/xfa-template/3.3/"><subform name="form" layout="tb">
<pageSet><pageArea name="p"><contentArea x="0.5in" y="0.5in" w="7.5in" h="10in"/><medium short="8.5in" long="11in"/></pageArea></pageSet>
{fields}</subform></template></xdp:xdp>"#
        );
        let mut doc = pdfcraft_cos::Document::open(Arc::new(crate::fixtures::shell(&xdp))).unwrap();
        let tpl = crate::script::template_of(&doc).unwrap();
        let mut ov = crate::script::Overrides::default();
        for i in 0..n {
            assert_eq!(ov.set_prop(&format!("form[0].f{i}[0]"), "assist#0/toolTip#0", None, &format!("Tip {i}")), Ok(true));
        }
        crate::script::set_overrides(&mut doc, &ov).unwrap();
        (doc, tpl, ov)
    }

    /// Who owns which SOM path is worked out once per template, not for each path asked about
    /// or each property change read from a file: asking about 10,000 of the 20,000 fields of
    /// one unnamed subform takes one look-up per segment of each path, however wide the form,
    /// and each is its template object's alone (named, none is shared). Counted, not timed.
    #[test]
    fn who_owns_which_path_is_worked_out_once_per_template() {
        let fields: String = (0..20_000).map(|i| format!(r#"<field name="f{i}" w="2in" h="0.3in"><ui><textEdit/></ui></field>"#)).collect();
        let xdp = format!(
            r#"<?xml version="1.0" encoding="UTF-8"?>
<xdp:xdp xmlns:xdp="http://ns.adobe.com/xdp/">
<template xmlns="http://www.xfa.org/schema/xfa-template/3.3/"><subform name="form" layout="tb">
<pageSet><pageArea name="p"><contentArea x="0.5in" y="0.5in" w="7.5in" h="10in"/><medium short="8.5in" long="11in"/></pageArea></pageSet>
<subform layout="tb">{fields}</subform></subform></template></xdp:xdp>"#
        );
        let doc = pdfcraft_cos::Document::open(Arc::new(crate::fixtures::shell(&xdp))).unwrap();
        let tpl = crate::script::template_of(&doc).unwrap();
        let _ = (crate::script::take_owners_made(), crate::script::take_owners_looked());
        for i in 0..10_000 {
            assert!(crate::som_made_once(&tpl, &format!("form[0].f{i}[0]")), "f{i}");
        }
        assert!(!crate::som_is_shared(&tpl, "form[0].f19999[0]"));
        assert_eq!(crate::script::take_owners_made(), 1, "made once for the template");
        // One owner per path: each field's name is its own.
        assert_eq!(crate::script::take_owners_looked(), 10_000 + 1);
        // A clone of the template (a template laid out again) starts its own.
        let again = tpl.clone();
        assert!(!crate::som_is_shared(&again, "form[0].f0[0]"));
        assert_eq!(crate::script::take_owners_made(), 1);
    }

    /// Once read, the property changes a document keeps are not read again for the next event:
    /// none are, whether the form keeps 2 or 400, until they change; then they are read once.
    /// Counted, not timed.
    #[test]
    fn kept_changes_are_read_once_however_many_the_form_keeps() {
        for n in [2, 400] {
            let (mut doc, tpl, mut ov) = form_with_kept_changes(n);
            let _ = crate::script::take_kept_read();
            assert_eq!(crate::script::overrides_of(&doc, &tpl), ov);
            assert_eq!(crate::script::take_kept_read(), n, "the first read reads them");
            for _ in 0..3 {
                assert_eq!(crate::script::overrides_of(&doc, &tpl), ov);
                assert_eq!(crate::script::take_kept_read(), 0, "{n} kept changes, read again");
            }
            // A change: the document holds other changes, read once, then kept again.
            let before = doc.clone();
            assert_eq!(ov.set_prop("form[0].f0[0]", "assist#0/toolTip#0", None, "Changed"), Ok(true));
            crate::script::set_overrides(&mut doc, &ov).unwrap();
            assert_eq!(crate::script::overrides_of(&doc, &tpl), ov);
            assert_eq!(crate::script::take_kept_read(), n);
            assert_eq!(crate::script::overrides_of(&doc, &tpl), ov);
            assert_eq!(crate::script::take_kept_read(), 0);
            // Written and read back from the file (as the working file is after an edit): the
            // same changes, so none is checked again.
            let written = pdfcraft_cos::write_incremental(&doc, &pdfcraft_cos::SaveOptions::default()).unwrap();
            let reread = pdfcraft_cos::Document::open(Arc::new(written)).unwrap();
            assert_eq!(crate::script::overrides_of(&reread, &tpl), ov);
            assert_eq!(crate::script::take_kept_read(), 0);
            // Back to the document as it was (an undo): its own changes, not the memo's.
            let old = crate::script::overrides_of(&before, &tpl);
            assert_eq!(old, crate::script::overrides(&before));
            assert_ne!(old, ov);
        }
    }

    /// The object of property changes is written over only when it is one the document added
    /// for them: one the file names (the catalog here) or another object added since is left
    /// alone, and the changes go in an object of their own.
    #[test]
    fn a_props_reference_to_another_object_is_never_written_over() {
        use pdfcraft_cos::{Dict, Object};
        let (mut doc, _, mut ov) = form_with_kept_changes(2);
        let props_ref = |d: &pdfcraft_cos::Document| {
            let root = d.root().unwrap();
            let acro = d.get(root).as_dict().and_then(|c| c.get(b"AcroForm")).map(|a| d.resolve(a)).unwrap();
            let kept = acro.as_dict().and_then(|a| a.get(crate::script::OVERRIDES_KEY)).map(|o| d.resolve(o)).unwrap();
            kept.as_dict().and_then(|k| k.get(b"Props")).and_then(Object::as_ref).unwrap()
        };
        // Its own object, written again in place.
        let own = props_ref(&doc);
        assert_eq!(ov.set_prop("form[0].f0[0]", "assist#0/toolTip#0", None, "Again"), Ok(true));
        crate::script::set_overrides(&mut doc, &ov).unwrap();
        assert_eq!(props_ref(&doc), own);
        let annot = doc.add(Object::Dict(Dict::from_iter([(b"Type".to_vec(), Object::Name(b"Annot".to_vec()))])));
        let catalog = doc.root().unwrap();
        for target in [catalog, annot] {
            let before = doc.get(target);
            // The file's /Props names `target`.
            let root = doc.root().unwrap();
            let mut acro =
                doc.get(root).as_dict().and_then(|c| c.get(b"AcroForm")).map(|a| doc.resolve(a)).and_then(|a| a.as_dict().cloned()).unwrap();
            let mut kept = acro.get(crate::script::OVERRIDES_KEY).map(|o| doc.resolve(o)).and_then(|o| o.as_dict().cloned()).unwrap();
            kept.set(b"Props".to_vec(), Object::Ref(target));
            acro.set(crate::script::OVERRIDES_KEY.to_vec(), Object::Dict(kept));
            doc.update_dict(root, |c| c.set(b"AcroForm".to_vec(), Object::Dict(acro))).unwrap();
            crate::script::set_overrides(&mut doc, &ov).unwrap();
            if target == catalog {
                assert!(doc.get(target).as_dict().is_some_and(|c| c.get(b"Pages").is_some()), "the catalog is whole");
            } else {
                assert_eq!(doc.get(target), before);
            }
            assert_ne!(props_ref(&doc), target);
            assert_eq!(crate::script::overrides(&doc), ov);
        }
    }

    /// Two documents of the form holding other changes, read in turn (an undo and a redo):
    /// each gives its own.
    #[test]
    fn documents_holding_other_changes_each_read_their_own() {
        let (doc, tpl, ov) = form_with_kept_changes(3);
        let (other_doc, other_tpl, other) = form_with_kept_changes(4);
        for _ in 0..3 {
            assert_eq!(crate::script::overrides_of(&doc, &tpl), ov);
            assert_eq!(crate::script::overrides_of(&other_doc, &other_tpl), other);
        }
    }

    /// The live form built again for the next event gives each changed object the property tree
    /// it had: none is made again, whether the form keeps 2 changes or 400. Counted, not timed.
    #[test]
    fn the_live_form_built_again_makes_no_changed_tree_again() {
        for n in [2, 400] {
            let (doc, tpl, _) = form_with_kept_changes(n);
            let ov = crate::script::overrides_of(&doc, &tpl);
            let _ = take_applied_made();
            let first = crate::script::form_tree(&tpl, None, &ov);
            assert_eq!(take_applied_made(), n);
            let again = crate::script::form_tree(&tpl, None, &ov);
            assert_eq!(take_applied_made(), 0, "{n} kept changes");
            assert_eq!(first.root, again.root);
        }
    }

    /// Fields whose kept changes reach more than their tree (`mandatory`, kept as the
    /// validation's `nullTest`) aren't read again when the live form is built again: none is
    /// made again, whether the form keeps 2 such changes or 400. Counted, not timed.
    #[test]
    fn the_live_form_built_again_reads_no_changed_field_again() {
        for n in [2, 400] {
            let (_, tpl, _) = form_with_kept_changes(n);
            let mut ov = crate::script::Overrides::default();
            for i in 0..n {
                assert_eq!(ov.set_prop(&format!("form[0].f{i}[0]"), "validate#0", Some("nullTest"), "error"), Ok(true));
            }
            let _ = take_fields_made();
            let first = crate::script::form_tree(&tpl, None, &ov);
            let made_first = take_fields_made();
            let again = crate::script::form_tree(&tpl, None, &ov);
            assert_eq!(first.root, again.root);
            assert!(first.root.children.iter().zip(&again.root.children).all(|(a, b)| Arc::ptr_eq(&a.props, &b.props)), "the same trees");
            assert_eq!((made_first, take_fields_made()), (n, 0), "{n} kept changes");
        }
    }

    /// One object given new changes event after event (a script that sets a tooltip from what
    /// was typed) keeps at most two generations of trees, not one for every value it was
    /// ever given.
    #[test]
    fn an_object_changed_again_and_again_keeps_the_memo_bounded() {
        let memo = Memo::default();
        let base = Arc::new(Props::default());
        let long = "x".repeat(16 << 10);
        for i in 0..3_000 {
            let sets = BTreeMap::from([("assist#0/toolTip#0".to_string(), format!("{long}{i}"))]);
            let _ = applied(&memo, &base, &sets);
        }
        let (_, bytes) = applied_held(&memo);
        assert!(bytes <= 2 * MAX_APPLIED_BYTES, "{bytes} bytes held");
    }

    /// A result that is past a generation's budget by itself is handed back and not kept: kept,
    /// it would put the memo past its bound alone, and two of them past twice the bound. One
    /// that fits is kept and found again.
    #[test]
    fn a_result_past_the_budget_by_itself_is_not_kept() {
        let memo: Mutex<Applied<u8>> = Mutex::default();
        let base = Arc::new(Props::default());
        let sets = |v: &str| BTreeMap::from([("assist#0/toolTip#0".to_string(), v.to_string())]);
        let held = |memo: &Mutex<Applied<u8>>| {
            let a = memo.lock().unwrap_or_else(PoisonError::into_inner);
            (a.new.len(), a.old.len(), a.trees, a.bytes)
        };
        let made = std::cell::Cell::new(0);
        let make = || {
            made.set(made.get() + 1);
            7u8
        };
        let small = remembered(&memo, &base, &sets("small"), |_| 100, make);
        let kept = held(&memo);
        assert_eq!((kept.0, kept.1, kept.2), (1, 0, 1), "{kept:?}");
        for _ in 0..2 {
            assert_eq!(*remembered(&memo, &base, &sets("large"), |_| MAX_APPLIED_BYTES, make), 7);
        }
        assert_eq!(held(&memo), kept, "neither kept nor the cause of a new generation");
        assert_eq!(made.get(), 3, "made each time it is asked for");
        assert!(Arc::ptr_eq(&small, &remembered(&memo, &base, &sets("small"), |_| 100, make)));
        assert_eq!(made.get(), 3);
    }

    /// Many instances of one object, each with its own changes (rows a script numbers): a
    /// look-up compares the entries with its own changes, not every instance's. Counted.
    #[test]
    fn instances_with_their_own_changes_are_found_without_comparing_the_others() {
        let memo = Memo::default();
        let base = Arc::new(Props::default());
        let all: Vec<_> = (0..2_000).map(|i| BTreeMap::from([("assist#0/toolTip#0".to_string(), format!("Row {i}"))])).collect();
        let _ = take_applied_compared();
        let first: Vec<_> = all.iter().map(|s| applied(&memo, &base, s)).collect();
        let again: Vec<_> = all.iter().map(|s| applied(&memo, &base, s)).collect();
        assert!(first.iter().zip(&again).all(|(a, b)| Arc::ptr_eq(a, b)));
        let compared = take_applied_compared();
        assert!(compared <= 2 * all.len(), "{compared} entries compared");
    }

    /// The object of property changes this document added is freed when no change is left, so
    /// an incremental save doesn't write the changes nothing refers to any more.
    #[test]
    fn no_changes_left_frees_the_object_that_held_them() {
        let (mut doc, _, _) = form_with_kept_changes(2);
        let mut ov = crate::script::Overrides::default();
        ov.presence.insert("form[0].f1[0]".into(), "hidden".into());
        crate::script::set_overrides(&mut doc, &ov).unwrap();
        let written = pdfcraft_cos::write_incremental(&doc, &pdfcraft_cos::SaveOptions::default()).unwrap();
        assert!(!written.windows(5).any(|w| w == b"Tip 0"), "the changes are written though nothing refers to them");
    }

    /// Review r5 (D1): a path's changes named by reference, or a value named by one, are not
    /// followed (they are written in the object of changes itself): each is counted as refused,
    /// the ones the object holds are read, and what was read stands for the object in the memo
    /// whatever the referenced objects become.
    #[test]
    fn changes_named_by_reference_are_not_followed() {
        use pdfcraft_cos::{Dict, Object, PdfString};
        let (mut doc, tpl, _) = form_with_kept_changes(3);
        let tip = |tip: &str| Object::String(PdfString::text(tip));
        let sets = |value: Object| Object::Dict(Dict::from_iter([(b"assist#0/toolTip#0".to_vec(), value)]));
        let entry = doc.add(sets(tip("By reference")));
        let value = doc.add(tip("Value by reference"));
        let root = doc.root().unwrap();
        let mut acro = doc.get(root).as_dict().and_then(|c| c.get(b"AcroForm")).map(|a| doc.resolve(a)).and_then(|a| a.as_dict().cloned()).unwrap();
        let mut kept = acro.get(crate::script::OVERRIDES_KEY).map(|o| doc.resolve(o)).and_then(|o| o.as_dict().cloned()).unwrap();
        let props = doc.add(Object::Dict(Dict::from_iter([
            (b"form[0].f0[0]".to_vec(), Object::Ref(entry)),
            (b"form[0].f1[0]".to_vec(), sets(Object::Ref(value))),
            (b"form[0].f2[0]".to_vec(), sets(tip("Held"))),
        ])));
        kept.set(b"Props".to_vec(), Object::Ref(props));
        acro.set(crate::script::OVERRIDES_KEY.to_vec(), Object::Dict(kept));
        doc.update_dict(root, |c| c.set(b"AcroForm".to_vec(), Object::Dict(acro))).unwrap();
        let _ = crate::script::take_kept_read();
        let read = crate::script::overrides_of(&doc, &tpl);
        let tips: Vec<_> = read.props().iter().map(|(som, m)| (som.as_str(), m.get("assist#0/toolTip#0").map(String::as_str))).collect();
        assert_eq!(tips, [("form[0].f2[0]", Some("Held"))]);
        assert_eq!(read.dropped, 2, "the two named by reference, counted for a note");
        assert_eq!(crate::script::take_kept_read(), 1);
        // The referenced objects change: the object of changes is the same, and so is what it
        // gives, taken from the memo.
        doc.set(entry, sets(tip("Changed")));
        doc.set(value, tip("Changed"));
        assert_eq!(crate::script::overrides_of(&doc, &tpl), read);
        assert_eq!(crate::script::take_kept_read(), 0);
        // And read without the memo, the same.
        assert_eq!(crate::script::overrides(&doc), read);
    }

    /// Review r2 (H1): a field the memo keeps counts its own copies of its item lists (outside
    /// the object's property budget) against the generation's bytes, so fields with long lists
    /// can't hold more than the memo says.
    #[test]
    fn a_kept_field_counts_its_item_lists() {
        let entries: String = (0..1_000).map(|i| format!("<text>{}{i:04}</text>", "x".repeat(200))).collect();
        let xdp = format!(
            r#"<template xmlns="http://www.xfa.org/schema/xfa-template/3.3/"><subform name="form" layout="tb">
<pageSet><pageArea name="p"><contentArea x="0.5in" y="0.5in" w="7.5in" h="10in"/><medium short="8.5in" long="11in"/></pageArea></pageSet>
<field name="pick" w="2in" h="0.3in"><ui><choiceList/></ui><items>{entries}</items><items save="1">{entries}</items></field></subform></template>"#
        );
        let tpl = crate::parse(&xdp).unwrap().0;
        let Some(crate::model::Node::Field(f)) = tpl.root.children.first() else { panic!() };
        assert_eq!(f.items.len(), 1_000);
        let sets = BTreeMap::from([("validate#0@nullTest".to_string(), "error".to_string())]);
        let made = restyled_field(&tpl.memo, f, &sets);
        assert!(made.required);
        // The field's own lists, past its tree (which holds the entries too).
        let lists = tree_bytes(&made.common.props) + 2 * 1_000 * 204;
        assert!(fields_bytes(&tpl.memo) >= lists, "{} < {lists}", fields_bytes(&tpl.memo));
    }

    /// Review r5 (D4): so does it count its own copy of its value (a long default text) and its
    /// caption's, which its tree need not hold; the layout's readings are counted by the same.
    #[test]
    fn a_kept_field_counts_its_value_and_its_captions() {
        let (text, label) = ("v".repeat(60_000), "c".repeat(40_000));
        let xdp = format!(
            r#"<template xmlns="http://www.xfa.org/schema/xfa-template/3.3/"><subform name="form" layout="tb">
<pageSet><pageArea name="p"><contentArea x="0.5in" y="0.5in" w="7.5in" h="10in"/><medium short="8.5in" long="11in"/></pageArea></pageSet>
<field name="note" w="2in" h="0.3in"><ui><textEdit/></ui><caption><value><text>{label}</text></value></caption><value><text>{text}</text></value></field></subform></template>"#
        );
        let tpl = crate::parse(&xdp).unwrap().0;
        let Some(crate::model::Node::Field(f)) = tpl.root.children.first() else { panic!() };
        assert_eq!((value_bytes(&f.value), f.caption.as_ref().map(|c| value_bytes(&c.value))), (60_000, Some(40_000)));
        let sets = BTreeMap::from([("validate#0@nullTest".to_string(), "error".to_string())]);
        let made = restyled_field(&tpl.memo, f, &sets);
        // The texts are past what the tree keeps of an object (`parse::MAX_OBJECT_PROP_BYTES`).
        assert!(tree_bytes(&made.common.props) < 40_000);
        let own = tree_bytes(&made.common.props) + 100_000;
        assert!(fields_bytes(&tpl.memo) >= own, "{} < {own}", fields_bytes(&tpl.memo));
    }
}
