//! pdfcraft-organize — page and document-structure edits (L4).
//!
//! Every operation mutates a `pdfcraft_cos::Document` (copy-on-write), so callers snapshot the
//! document before an edit for undo. Operations never drop data they do not understand.
//!
//! Page-tree strategy: before restructuring, inheritable page attributes (`Resources`,
//! `MediaBox`, `CropBox`, `Rotate` — ISO 32000-2 §7.7.3.4) are copied onto each page, then the
//! tree is rebuilt as a single flat `/Pages` node. Intermediate nodes become unreachable (a full
//! save drops them; an incremental save leaves them untouched). Flat trees are valid for any
//! page count and are what most producers write for small documents.

#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

use pdfcraft_cos::{Dict, Document, ObjRef, Object, PdfString, XrefEntry};

mod boxes;
mod dedupe;
mod import;
mod labels;
mod outline;
mod pdfx;
mod prune;
mod transition;
pub mod view;
mod xmp;

pub use boxes::{BoxSpec, PageBox, page_boxes, set_page_box};
pub use dedupe::dedupe_resources;
pub use import::{
    CombineOutcome, Combiner, FieldConflicts, ImportOutcome, Run, SplitBy, combine, combine_field_conflicts, combine_grouped, combine_grouped_with,
    combine_selected, combine_selected_with, combine_with, extract_pages, field_name_conflicts, import_pages, import_pages_with, insert_file_pages,
    page_as_form, split, split_ranges,
};
pub use labels::{LabelRange, LabelStyle, PageLabels, number_pages, page_label_ranges, page_labels, set_page_label_ranges};
pub use outline::{
    Bookmark, BookmarkPage, OutlineEntry, OutlineError, add_bookmark, add_bookmark_tree, bookmark_page, bookmarks, delete_bookmark, move_bookmark,
    rename_bookmark, set_bookmark_open, set_bookmark_page, top_level_bookmarks,
};
pub use transition::{TRANSITION_DIRECTIONS, TRANSITION_STYLES, Transition, TransitionSpec, set_transition};
pub use view::{InitialView, displays_doc_title, initial_view, set_initial_view};

#[derive(Debug, thiserror::Error, PartialEq)]
pub enum OrganizeError {
    #[error("the document has no page tree")]
    NoPageTree,
    /// Holds the 0-based index; the message names the 1-based page a user counts (#1550).
    #[error("page {} does not exist", .0.saturating_add(1))]
    NoSuchPage(usize),
    #[error("a document must keep at least one page")]
    WouldRemoveAllPages,
    #[error("{0}")]
    InvalidBox(String),
    #[error("{0}")]
    Invalid(String),
    #[error("{0}")]
    Cos(#[from] pdfcraft_cos::CosError),
}

const INHERITABLE: [&[u8]; 4] = [b"Resources", b"MediaBox", b"CropBox", b"Rotate"];

/// A leaf page in document order.
#[derive(Clone, Debug, PartialEq)]
pub struct PageRef {
    pub obj: ObjRef,
}

fn pages_root(doc: &Document) -> Result<ObjRef, OrganizeError> {
    let root = doc.root().ok_or(OrganizeError::NoPageTree)?;
    let catalog = doc.get(root);
    catalog.as_dict().and_then(|d| d.reference(b"Pages")).ok_or(OrganizeError::NoPageTree)
}

/// All leaf pages in order, with their effective inherited attributes.
fn walk(doc: &Document) -> Result<Vec<(ObjRef, Dict)>, OrganizeError> {
    let root = pages_root(doc)?;
    let mut out = Vec::new();
    let mut seen = std::collections::HashSet::new();
    // (node, inherited attributes from ancestors)
    let mut stack: Vec<(ObjRef, Dict)> = vec![(root, Dict::new())];
    while let Some((node, inherited)) = stack.pop() {
        if !seen.insert(node) || seen.len() > 1_000_000 {
            continue; // cycles in broken trees
        }
        let obj = doc.get(node);
        let Some(d) = obj.as_dict() else { continue };
        let mut attrs = inherited.clone();
        for k in INHERITABLE {
            if let Some(v) = d.get(k) {
                attrs.set(k.to_vec(), v.clone());
            }
        }
        let is_pages = d.name(b"Type") == Some(b"Pages") || (d.contains(b"Kids") && d.name(b"Type") != Some(b"Page"));
        if is_pages {
            let kids = d.get(b"Kids").map(|k| doc.resolve(k)).and_then(|k| k.as_array().cloned()).unwrap_or_default();
            for k in kids.iter().rev() {
                if let Some(r) = k.as_ref() {
                    stack.push((r, attrs.clone()));
                }
            }
        } else {
            out.push((node, attrs));
        }
    }
    Ok(out)
}

/// Leaf pages in document order.
pub fn pages(doc: &Document) -> Result<Vec<PageRef>, OrganizeError> {
    Ok(walk(doc)?.into_iter().map(|(obj, _)| PageRef { obj }).collect())
}

/// Effective inherited page rotation, clockwise in degrees.
pub fn page_rotation(doc: &Document, index: usize) -> Result<i64, OrganizeError> {
    let pages = walk(doc)?;
    let (_, attrs) = pages.get(index).ok_or(OrganizeError::NoSuchPage(index))?;
    Ok(resolved_rotation(doc, attrs).rem_euclid(360))
}

pub fn page_count(doc: &Document) -> Result<usize, OrganizeError> {
    Ok(walk(doc)?.len())
}

fn resolved_rotation(doc: &Document, attrs: &Dict) -> i64 {
    attrs.get(b"Rotate").and_then(|value| doc.resolve(value).as_int()).unwrap_or(0)
}

/// Rebuild the page tree as one flat node with `order` as its kids (attributes pushed down).
fn rebuild(doc: &mut Document, order: &[(ObjRef, Dict)]) -> Result<(), OrganizeError> {
    let root = pages_root(doc)?;
    for (page, inherited) in order {
        doc.update_dict(*page, |d| {
            for k in INHERITABLE {
                if !d.contains(k)
                    && let Some(v) = inherited.get(k)
                {
                    d.set(k.to_vec(), v.clone());
                }
            }
            d.set(b"Parent".to_vec(), Object::Ref(root));
        })?;
    }
    doc.update_dict(root, |d| {
        d.set(b"Type".to_vec(), Object::name("Pages"));
        d.set(b"Kids".to_vec(), Object::Array(order.iter().map(|(r, _)| Object::Ref(*r)).collect()));
        d.set(b"Count".to_vec(), Object::Int(order.len() as i64));
        // Attributes now live on the pages; leaving them here would be harmless but misleading.
        for k in INHERITABLE {
            d.remove(k);
        }
        d.remove(b"Parent");
    })?;
    Ok(())
}

/// Gather the document's `/Type /Page` objects into a new page tree when the catalog leads to no
/// page (its `/Pages` is missing or points at a lost object). Readers then find the pages by
/// scanning the file, in file order, and edits must address those same pages. Returns how many
/// pages were gathered; 0 leaves the document unchanged.
pub fn recover_page_tree(doc: &mut Document) -> Result<usize, OrganizeError> {
    if walk(doc).is_ok_and(|pages| !pages.is_empty()) {
        return Ok(0);
    }
    let catalog = doc.root().ok_or(OrganizeError::NoPageTree)?;
    let position = |num: u32| match doc.xref_entry(num) {
        Some(XrefEntry::InFile { offset, .. }) => (offset, 0),
        Some(XrefEntry::InStream { stream, index }) => match doc.xref_entry(stream) {
            Some(XrefEntry::InFile { offset, .. }) => (offset, index),
            _ => (u64::MAX, index),
        },
        _ => (u64::MAX, 0),
    };
    let mut found: Vec<((u64, u32), ObjRef)> = doc
        .scan_objects()
        .filter(|(_, o)| o.as_dict().is_some_and(|d| d.name(b"Type") == Some(b"Page")))
        .map(|(r, _)| (position(r.num), r))
        .collect();
    if found.is_empty() {
        return Ok(0);
    }
    found.sort();
    let root = doc.add(Dict::new());
    doc.update_dict(catalog, |d| d.set(b"Pages".to_vec(), Object::Ref(root)))?;
    let order: Vec<(ObjRef, Dict)> = found.into_iter().map(|(_, r)| (r, Dict::new())).collect();
    rebuild(doc, &order)?;
    Ok(order.len())
}

fn check(indices: &[usize], n: usize) -> Result<(), OrganizeError> {
    match indices.iter().find(|i| **i >= n) {
        // A viewer still shows pages a lost page tree leaves out (#1656): "page 1 does not
        // exist" would contradict it.
        Some(_) if n == 0 => Err(OrganizeError::Invalid("the document's page tree lists no pages: the file is damaged".into())),
        Some(i) => Err(OrganizeError::NoSuchPage(*i)),
        None => Ok(()),
    }
}

/// Rotate pages by a multiple of 90° (positive = clockwise), adjusting `/Rotate` (§7.7.3.3).
pub fn rotate_pages(doc: &mut Document, indices: &[usize], degrees: i64) -> Result<(), OrganizeError> {
    if degrees % 90 != 0 {
        return Err(OrganizeError::Invalid("rotation must be a multiple of 90 degrees".into()));
    }
    let all = walk(doc)?;
    check(indices, all.len())?;
    let delta = degrees.rem_euclid(360);
    for &i in indices {
        let (page, inherited) = &all[i];
        // Repeated indices must see earlier edits instead of the page walk's snapshot.
        let current_page = doc.get(*page);
        let current_attrs = current_page.as_dict().filter(|attrs| attrs.get(b"Rotate").is_some()).unwrap_or(inherited);
        let current = resolved_rotation(doc, current_attrs);
        let new = (current.rem_euclid(360) + delta).rem_euclid(360);
        doc.update_dict(*page, |d| d.set(b"Rotate".to_vec(), Object::Int(new)))?;
    }
    Ok(())
}

/// Delete pages. Refuses to delete every page.
pub fn delete_pages(doc: &mut Document, indices: &[usize]) -> Result<(), OrganizeError> {
    let all = walk(doc)?;
    check(indices, all.len())?;
    let keep: Vec<(ObjRef, Dict)> = all.iter().enumerate().filter(|(i, _)| !indices.contains(i)).map(|(_, p)| p.clone()).collect();
    if keep.is_empty() {
        return Err(OrganizeError::WouldRemoveAllPages);
    }
    rebuild(doc, &keep)?;
    labels::drop_deleted_pages(doc, indices)?;
    // The deleted pages go for good: bookmarks, links and widgets that still point at one would
    // otherwise keep it, with its content, in the saved file. Their form widgets leave the form,
    // and bookmarks and links that went to them lose that destination.
    let gone: Vec<ObjRef> = all.iter().map(|(r, _)| *r).filter(|r| !keep.iter().any(|(k, _)| k == r)).collect();
    let mut widgets = Vec::new();
    for r in &gone {
        let annots = doc.get(*r).as_dict().and_then(|d| d.get(b"Annots").cloned()).map(|a| doc.resolve(&a).as_array().cloned().unwrap_or_default());
        for a in annots.unwrap_or_default() {
            if let Some(w) = a.as_ref().filter(|w| doc.get(*w).as_dict().is_some_and(|d| d.name(b"Subtype") == Some(b"Widget"))) {
                widgets.push(w);
            }
        }
    }
    drop_widgets(doc, widgets)?;
    drop_destinations_to(doc, &gone, &keep)?;
    for r in gone {
        doc.free(r);
    }
    Ok(())
}

/// Bookmarks, and links on the `keep` pages, that go to one of the `gone` pages lose that
/// destination (/Dest, or a GoTo /A), rather than pointing at nothing; so does the catalog's /OpenAction.
fn drop_destinations_to(doc: &mut Document, gone: &[ObjRef], keep: &[(ObjRef, Dict)]) -> Result<(), OrganizeError> {
    let dead_names = drop_named_destinations(doc, gone)?;
    let mut holders = outline::items(doc);
    for (p, _) in keep {
        let annots = doc.get(*p).as_dict().and_then(|d| d.get(b"Annots").cloned()).map(|a| doc.resolve(&a).as_array().cloned().unwrap_or_default());
        holders.extend(
            annots
                .unwrap_or_default()
                .iter()
                .filter_map(Object::as_ref)
                .filter(|a| doc.get(*a).as_dict().is_some_and(|d| d.name(b"Subtype") == Some(b"Link"))),
        );
    }
    let to_gone = |o: Option<&Object>| {
        let Some(o) = o.map(|o| doc.resolve(o)) else { return false };
        match &*o {
            Object::Name(n) => dead_names.contains(n),
            Object::String(s) => dead_names.contains(&s.bytes),
            _ => goes_to(doc, &o, gone),
        }
    };
    let mut dead: Vec<(ObjRef, &[u8])> = Vec::new();
    for h in holders {
        let Some(d) = doc.get(h).as_dict().cloned() else { continue };
        if to_gone(d.get(b"Dest")) {
            dead.push((h, b"Dest"));
        }
        let action = d.get(b"A").map(|a| doc.resolve(a)).and_then(|a| a.as_dict().cloned());
        if action.is_some_and(|a| a.name(b"S") == Some(b"GoTo") && to_gone(a.get(b"D"))) {
            dead.push((h, b"A"));
        }
    }
    // The catalog's /OpenAction is either a destination array or a GoTo action; one that opens a
    // deleted page is dropped, as a bookmark's would be.
    if let Some(root) = doc.root() {
        let open = doc.get(root).as_dict().and_then(|c| c.get(b"OpenAction").map(|o| doc.resolve(o)));
        let opens_gone = match open.as_deref() {
            Some(Object::Dict(a)) => a.name(b"S") == Some(b"GoTo") && to_gone(a.get(b"D")),
            other => to_gone(other),
        };
        if opens_gone {
            dead.push((root, b"OpenAction"));
        }
    }
    for (h, key) in dead {
        doc.update_dict(h, |d| {
            d.remove(key);
        })?;
    }
    Ok(())
}

/// True when a destination (an array, or a dictionary whose /D is one) opens one of the `gone` pages.
fn goes_to(doc: &Document, dest: &Object, gone: &[ObjRef]) -> bool {
    let dest = doc.resolve(dest);
    let array = match dest.as_dict() {
        Some(d) => d.get(b"D").map(|d| doc.resolve(d)),
        None => Some(dest.clone()),
    };
    array.and_then(|a| a.as_array().and_then(|a| a.first()).and_then(Object::as_ref)).is_some_and(|r| gone.contains(&r))
}

/// Remove the named destinations (the catalog's /Names /Dests tree and the legacy /Dests
/// dictionary) that open one of the `gone` pages. Returns the removed names, so that the links
/// using them can lose their destination too. Names of surviving pages are left alone.
fn drop_named_destinations(doc: &mut Document, gone: &[ObjRef]) -> Result<Vec<Vec<u8>>, OrganizeError> {
    let mut dead: Vec<Vec<u8>> = Vec::new();
    let Some(root) = doc.root() else { return Ok(dead) };
    let Some(catalog) = doc.get(root).as_dict().cloned() else { return Ok(dead) };
    match catalog.get(b"Dests").cloned() {
        Some(Object::Ref(r)) => {
            if let Some(mut d) = doc.get(r).as_dict().cloned()
                && prune_legacy(doc, &mut d, gone, &mut dead)
            {
                doc.update_dict(r, |x| *x = d)?;
            }
        }
        Some(Object::Dict(mut d)) => {
            let changed = prune_legacy(doc, &mut d, gone, &mut dead);
            if changed {
                doc.update_dict(root, |c| c.set(b"Dests".to_vec(), Object::Dict(d)))?;
            }
        }
        _ => {}
    }
    match catalog.get(b"Names").cloned() {
        Some(Object::Ref(n)) => {
            let tree = doc.get(n).as_dict().and_then(|d| d.get(b"Dests").cloned());
            if let Some(new) = tree.and_then(|t| prune_tree_slot(doc, &t, gone, &mut dead).transpose()).transpose()? {
                doc.update_dict(n, |d| d.set(b"Dests".to_vec(), Object::Dict(new)))?;
            }
        }
        Some(Object::Dict(mut names)) => {
            let tree = names.get(b"Dests").cloned();
            if let Some(new) = tree.and_then(|t| prune_tree_slot(doc, &t, gone, &mut dead).transpose()).transpose()? {
                names.set(b"Dests".to_vec(), Object::Dict(new));
                doc.update_dict(root, |c| c.set(b"Names".to_vec(), Object::Dict(names)))?;
            }
        }
        _ => {}
    }
    Ok(dead)
}

/// Drop the entries of a legacy /Dests dictionary that go to a `gone` page. True when any went.
fn prune_legacy(doc: &Document, dests: &mut Dict, gone: &[ObjRef], dead: &mut Vec<Vec<u8>>) -> bool {
    let keys: Vec<Vec<u8>> = dests.iter().filter(|(_, v)| goes_to(doc, v, gone)).map(|(k, _)| k.clone()).collect();
    for k in &keys {
        dests.remove(k);
    }
    dead.extend(keys.iter().cloned());
    !keys.is_empty()
}

/// Prune a name tree held in `slot` (a reference, or a dictionary written inline). A reference is
/// updated in place and yields `None`; an inline dictionary that changed is handed back.
fn prune_tree_slot(doc: &mut Document, slot: &Object, gone: &[ObjRef], dead: &mut Vec<Vec<u8>>) -> Result<Option<Dict>, OrganizeError> {
    let mut seen = Vec::new();
    match slot {
        Object::Ref(r) => {
            prune_tree_ref(doc, *r, gone, dead, &mut seen)?;
            Ok(None)
        }
        Object::Dict(d) => {
            let mut node = d.clone();
            Ok(prune_tree_node(doc, &mut node, gone, dead, &mut seen)?.then_some(node))
        }
        _ => Ok(None),
    }
}

fn prune_tree_ref(doc: &mut Document, r: ObjRef, gone: &[ObjRef], dead: &mut Vec<Vec<u8>>, seen: &mut Vec<ObjRef>) -> Result<(), OrganizeError> {
    if seen.contains(&r) {
        return Ok(());
    }
    seen.push(r);
    let Some(mut node) = doc.get(r).as_dict().cloned() else { return Ok(()) };
    if prune_tree_node(doc, &mut node, gone, dead, seen)? {
        doc.update_dict(r, |d| *d = node)?;
    }
    Ok(())
}

/// Drop the /Names pairs of one name-tree node that go to a `gone` page, then do the same for its
/// /Kids. True when `node` itself changed (kids held by reference are written back here).
fn prune_tree_node(
    doc: &mut Document,
    node: &mut Dict,
    gone: &[ObjRef],
    dead: &mut Vec<Vec<u8>>,
    seen: &mut Vec<ObjRef>,
) -> Result<bool, OrganizeError> {
    let mut changed = false;
    if let Some(pairs) = node.get(b"Names").map(|n| doc.resolve(n)).and_then(|n| n.as_array().cloned()) {
        let mut kept = Vec::with_capacity(pairs.len());
        for pair in pairs.chunks(2) {
            match pair {
                [key, value] if goes_to(doc, value, gone) => {
                    if let Some(s) = doc.resolve(key).as_string() {
                        dead.push(s.bytes.clone());
                    }
                }
                _ => kept.extend_from_slice(pair),
            }
        }
        if kept.len() != pairs.len() {
            node.set(b"Names".to_vec(), Object::Array(kept));
            changed = true;
        }
    }
    let kids = node.get(b"Kids").map(|k| doc.resolve(k)).and_then(|k| k.as_array().cloned()).unwrap_or_default();
    for kid in kids {
        if let Some(r) = kid.as_ref() {
            prune_tree_ref(doc, r, gone, dead, seen)?;
        }
    }
    Ok(changed)
}

/// Take widgets out of the form: out of their field's /Kids, or out of /AcroForm /Fields when
/// they are fields themselves. A field left without kids goes too.
fn drop_widgets(doc: &mut Document, mut widgets: Vec<ObjRef>) -> Result<(), OrganizeError> {
    let unlink = |list: Option<&Object>, w: ObjRef| -> Option<Vec<Object>> {
        let list = list.and_then(Object::as_array)?;
        list.iter().any(|o| o.as_ref() == Some(w)).then(|| list.iter().filter(|o| o.as_ref() != Some(w)).cloned().collect())
    };
    while let Some(w) = widgets.pop() {
        if let Some(parent) = doc.get(w).as_dict().and_then(|d| d.reference(b"Parent")) {
            let kids = doc.get(parent).as_dict().and_then(|d| unlink(d.get(b"Kids").map(|k| doc.resolve(k)).as_deref(), w));
            if let Some(kids) = kids {
                if kids.is_empty() {
                    widgets.push(parent);
                }
                doc.update_dict(parent, |d| d.set(b"Kids".to_vec(), Object::Array(kids)))?;
            }
            continue;
        }
        let Some(root) = doc.root() else { continue };
        match doc.get(root).as_dict().and_then(|c| c.get(b"AcroForm").cloned()) {
            Some(Object::Ref(form)) => {
                if let Some(fields) = doc.get(form).as_dict().and_then(|f| unlink(f.get(b"Fields").map(|x| doc.resolve(x)).as_deref(), w)) {
                    doc.update_dict(form, |f| f.set(b"Fields".to_vec(), Object::Array(fields)))?;
                }
            }
            Some(Object::Dict(mut form)) => {
                if let Some(fields) = unlink(form.get(b"Fields").map(|x| doc.resolve(x)).as_deref(), w) {
                    form.set(b"Fields".to_vec(), Object::Array(fields));
                    doc.update_dict(root, |c| c.set(b"AcroForm".to_vec(), Object::Dict(form)))?;
                }
            }
            _ => {}
        }
    }
    Ok(())
}

/// Move the pages at `indices` (kept in their relative order) so they start at position `to`
/// in the resulting document.
pub fn move_pages(doc: &mut Document, indices: &[usize], to: usize) -> Result<(), OrganizeError> {
    let all = walk(doc)?;
    check(indices, all.len())?;
    let mut sorted = indices.to_vec();
    sorted.sort_unstable();
    sorted.dedup();
    let moving: Vec<(ObjRef, Dict)> = sorted.iter().map(|i| all[*i].clone()).collect();
    let mut rest: Vec<(ObjRef, Dict)> = all.iter().enumerate().filter(|(i, _)| !sorted.contains(i)).map(|(_, p)| p.clone()).collect();
    let at = to.min(rest.len());
    rest.splice(at..at, moving);
    rebuild(doc, &rest)
}

/// Reverse the order of the pages at `indices` among the positions they occupy; every other
/// page stays where it is. Fewer than two distinct pages is refused, since nothing would change.
pub fn reverse_pages(doc: &mut Document, indices: &[usize]) -> Result<(), OrganizeError> {
    let mut all = walk(doc)?;
    check(indices, all.len())?;
    let mut slots = indices.to_vec();
    slots.sort_unstable();
    slots.dedup();
    if slots.len() < 2 {
        return Err(OrganizeError::Invalid("reversing needs at least two pages".into()));
    }
    let pages: Vec<(ObjRef, Dict)> = slots.iter().filter_map(|i| all.get(*i).cloned()).collect();
    for (slot, page) in slots.iter().zip(pages.into_iter().rev()) {
        if let Some(entry) = all.get_mut(*slot) {
            *entry = page;
        }
    }
    rebuild(doc, &all)
}

/// Duplicate pages: copies of `indices` (in order) are inserted at `at` in the page order
/// (clamped to the document's end), or after the last of them when `at` is `None`, sharing
/// fonts and images with the originals.
pub fn duplicate_pages(doc: &mut Document, indices: &[usize], at: Option<usize>) -> Result<(), OrganizeError> {
    let n = page_count(doc)?;
    check(indices, n)?;
    let Some(&last) = indices.iter().max() else { return Ok(()) };
    let mut order = indices.to_vec();
    order.sort_unstable();
    order.dedup();
    let src = doc.clone();
    import_pages(doc, &src, &order, at.unwrap_or(last + 1)).map(|_| ())
}

/// Replace Pages: the content of `targets` (in order) is replaced by that of `src_pages` of
/// `src`. As in Acrobat, only what the page shows changes (contents, resources and page boxes);
/// the original pages' links, comments, form widgets and the bookmarks pointing at them stay.
pub fn replace_pages(doc: &mut Document, targets: &[usize], src: &Document, src_pages: &[usize]) -> Result<(), OrganizeError> {
    let n = page_count(doc)?;
    check(targets, n)?;
    if targets.len() != src_pages.len() || targets.is_empty() {
        return Err(OrganizeError::Invalid(format!("{} pages can't replace {}", src_pages.len(), targets.len())));
    }
    let imported = import_pages(doc, src, src_pages, n)?;
    let all = walk(doc)?;
    const SHOWN: [&[u8]; 8] = [b"Contents", b"Resources", b"MediaBox", b"CropBox", b"BleedBox", b"TrimBox", b"ArtBox", b"Rotate"];
    for (t, new) in targets.iter().zip(&imported) {
        // The imported page with its inherited attributes resolved.
        let (_, inherited) = all.iter().find(|(r, _)| r == new).cloned().unwrap_or((*new, Dict::new()));
        let src_dict = doc.get(*new).as_dict().cloned().unwrap_or_default();
        let target = all[*t].0;
        doc.update_dict(target, |d| {
            for k in SHOWN {
                match src_dict.get(k).or_else(|| inherited.get(k)) {
                    Some(v) => d.set(k.to_vec(), v.clone()),
                    None => {
                        d.remove(k);
                    }
                }
            }
        })?;
    }
    // Drop the temporary copies (always the last pages now).
    delete_pages(doc, &(n..n + imported.len()).collect::<Vec<_>>())
}

/// Insert a blank page of `width × height` points at position `at` (0 = before the first page).
pub fn insert_blank_page(doc: &mut Document, at: usize, width: f64, height: f64) -> Result<ObjRef, OrganizeError> {
    let mut all = walk(doc)?;
    let root = pages_root(doc)?;
    let mut page = Dict::new();
    page.set(b"Type".to_vec(), Object::name("Page"));
    page.set(b"Parent".to_vec(), Object::Ref(root));
    page.set(b"MediaBox".to_vec(), Object::Array(vec![0.into(), 0.into(), Object::Real(width), Object::Real(height)]));
    page.set(b"Resources".to_vec(), Object::Dict(Dict::new()));
    let r = doc.add(page);
    let at = at.min(all.len());
    all.insert(at, (r, Dict::new()));
    rebuild(doc, &all)?;
    Ok(r)
}

/// Document information keys editable in Document Properties ▸ Description.
pub const INFO_KEYS: [&str; 4] = ["Title", "Author", "Subject", "Keywords"];

/// The document-information entries the spec names (ISO 32000-2 table 349). Any other key in
/// `/Info` is a custom property (Document Properties ▸ Custom).
pub const STANDARD_INFO_KEYS: [&str; 9] = ["Title", "Author", "Subject", "Keywords", "Creator", "Producer", "CreationDate", "ModDate", "Trapped"];

/// Longest custom property name accepted, in bytes: the name length every reader is asked to
/// accept (§7.3.5, Annex C).
pub const MAX_CUSTOM_KEY_LEN: usize = 127;

/// The custom document properties: every `/Info` entry whose key the spec does not name and whose
/// value is text, in the dictionary's order, as (name, text), the text without NUL or whitespace
/// padding and entries that are only padding left out. At most 256 are listed.
pub fn custom_info(doc: &Document) -> Vec<(String, String)> {
    let Some(info) = doc.trailer().get(b"Info").map(|o| doc.resolve(o)) else { return Vec::new() };
    let Some(d) = info.as_dict() else { return Vec::new() };
    d.iter()
        .filter(|(k, _)| !STANDARD_INFO_KEYS.iter().any(|s| s.as_bytes() == k.as_slice()))
        .filter_map(|(k, v)| {
            // Padding (\0s and whitespace, in any mix) is not part of the value, and a value that
            // is only padding is no property. Filtered before the cap, so blank entries can't
            // crowd real ones out.
            let text = doc.resolve(v).as_string()?.to_text();
            let text = text.trim_matches(|c: char| c == '\0' || c.is_whitespace());
            (!text.is_empty()).then(|| (String::from_utf8_lossy(k).into_owned(), text.to_owned()))
        })
        .take(256)
        .collect()
}

/// Whether `name` may be a custom property's key: not one the spec names (those are the
/// Description fields), not empty, at most [`MAX_CUSTOM_KEY_LEN`] bytes, and without the
/// characters a name cannot carry unescaped (whitespace, the delimiters `()<>[]{}/%` and `#`),
/// so every reader shows the same name. Acrobat refuses the standard and empty names too.
pub fn check_custom_key(name: &str) -> Result<(), OrganizeError> {
    if name.is_empty() {
        return Err(OrganizeError::Invalid("a custom property needs a name".into()));
    }
    if STANDARD_INFO_KEYS.contains(&name) {
        return Err(OrganizeError::Invalid(format!("{name} is a standard document property, not a custom one")));
    }
    if name.len() > MAX_CUSTOM_KEY_LEN {
        return Err(OrganizeError::Invalid(format!("a custom property name is at most {MAX_CUSTOM_KEY_LEN} bytes")));
    }
    if let Some(c) = name.chars().find(|c| c.is_whitespace() || c.is_control() || "()<>[]{}/%#".contains(*c)) {
        return Err(OrganizeError::Invalid(format!("a custom property name can't contain {c:?}")));
    }
    Ok(())
}

/// Read a document-information entry as text.
pub fn info(doc: &Document, key: &str) -> Option<String> {
    let info = doc.trailer().get(b"Info").map(|o| doc.resolve(o))?;
    info.as_dict()?.get(key.as_bytes()).and_then(|v| doc.resolve(v).as_string().map(|s| s.to_text()))
}

/// Set (or clear, with an empty value) a document-information entry.
///
/// Documents with XMP metadata also carry Title, Author, Subject, Keywords, Creator and Producer
/// in the XMP packet, and viewers that read XMP first show its value: the packet's property is
/// changed (or removed) to match (see `xmp`).
pub fn set_info(doc: &mut Document, key: &str, value: &str) -> Result<(), OrganizeError> {
    if !STANDARD_INFO_KEYS.contains(&key) {
        check_custom_key(key)?;
    }
    let value = value.trim();
    let entry = (!value.is_empty()).then(|| Object::String(PdfString::text(value)));
    set_info_entry(doc, key.as_bytes(), entry)?;
    xmp::sync_info(doc, key, value)
}

/// Set (or remove, with `None`) a document-information entry of any type.
fn set_info_entry(doc: &mut Document, key: &[u8], entry: Option<Object>) -> Result<(), OrganizeError> {
    match doc.trailer().get(b"Info").cloned() {
        Some(Object::Ref(r)) if doc.get(r).as_dict().is_some() => {
            doc.update_dict(r, |d| match entry {
                Some(v) => d.set(key.to_vec(), v),
                None => {
                    d.remove(key);
                }
            })?;
        }
        _ => {
            let Some(v) = entry else { return Ok(()) };
            let mut d = match doc.trailer().get(b"Info") {
                Some(Object::Dict(d)) => d.clone(),
                _ => Dict::new(),
            };
            d.set(key.to_vec(), v);
            let r = doc.add(d);
            doc.trailer_mut().set(b"Info".to_vec(), Object::Ref(r));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests;
