//! What the crate keeps alongside a parsed template for as long as it lives, so that the work of
//! an event doesn't grow with the property changes the form already holds:
//!
//! - the property changes last read from a document of the form
//!   ([`crate::script::overrides_of`]), with the object they were read from: an event whose
//!   document holds the same object takes them from here instead of checking every one of them
//!   again;
//! - the property trees those changes give the template's objects ([`applied`]): the live form
//!   is built for each event, and handing back the tree it had keeps the script engine's copy of
//!   it too (that copy is kept by tree).
//!
//! Neither is part of the template: a clone starts empty, and two compare equal. A poisoned lock
//! only means a panic elsewhere while it was held; both are memos, so their contents stay valid.

use std::collections::{BTreeMap, HashMap};
use std::sync::{Arc, Mutex, PoisonError, Weak};

use pdfcraft_cos::Object;

use crate::model::Props;

/// The property changes as kept, by SOM path then property key (see
/// [`crate::script::Overrides::props`]).
pub(crate) type PropMap = BTreeMap<String, BTreeMap<String, String>>;

/// The property changes read from one object of a document: what was kept of them, how many
/// changes and bytes that is, and how many were not kept.
#[derive(Clone)]
pub(crate) struct KeptProps {
    pub(crate) props: Arc<PropMap>,
    pub(crate) count: usize,
    pub(crate) bytes: usize,
    pub(crate) dropped: usize,
}

/// Most trees [`applied`] keeps; past them it starts again. A form keeps at most
/// [`crate::restyle::MAX_PROP_OVERRIDES`] changes, so this holds every changed object of a form.
const MAX_APPLIED: usize = 20_000;

/// One tree [`applied`] made: the tree it was made from (to tell a tree that is gone from one
/// now at its address), the changes, and the result.
type AppliedEntry = (Weak<Props>, BTreeMap<String, String>, Arc<Props>);

/// See the module documentation.
#[derive(Default)]
pub struct Memo {
    /// The object the property changes were last read from, and what was read.
    kept: Mutex<Option<(Arc<Object>, KeptProps)>>,
    applied: Mutex<HashMap<usize, Vec<AppliedEntry>>>,
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
}

#[cfg(test)]
thread_local! {
    /// Property trees [`applied`] made (rather than handed back), on this thread.
    static APPLIED_MADE: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

/// How many property trees [`applied`] has made on this thread, since the last call.
#[cfg(test)]
pub(crate) fn take_applied_made() -> usize {
    APPLIED_MADE.with(|c| c.replace(0))
}

/// `base` with the changes `sets` applied ([`crate::restyle::apply_props`]), the same `Arc` each
/// time the template's memo is asked for the same tree and changes. Kept by the tree's
/// address, checked against the tree itself so a freed tree's address can't match.
pub(crate) fn applied(memo: &Memo, base: &Arc<Props>, sets: &BTreeMap<String, String>) -> Arc<Props> {
    let key = Arc::as_ptr(base) as usize;
    let same_base = |w: &Weak<Props>| w.upgrade().is_some_and(|b| Arc::ptr_eq(&b, base));
    {
        let map = memo.applied.lock().unwrap_or_else(PoisonError::into_inner);
        if let Some(out) = map.get(&key).and_then(|list| list.iter().find(|(w, s, _)| same_base(w) && s == sets)).map(|(_, _, out)| Arc::clone(out)) {
            return out;
        }
    }
    #[cfg(test)]
    APPLIED_MADE.with(|c| c.set(c.get().saturating_add(1)));
    let out = Arc::new(crate::restyle::apply_props(base, sets).0);
    let mut map = memo.applied.lock().unwrap_or_else(PoisonError::into_inner);
    if map.len() >= MAX_APPLIED {
        map.clear();
    }
    let list = map.entry(key).or_default();
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
        let (other_doc, _, other) = form_with_kept_changes(4);
        for _ in 0..3 {
            assert_eq!(crate::script::overrides_of(&doc, &tpl), ov);
            assert_eq!(crate::script::overrides_of(&other_doc, &tpl), other);
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
}
