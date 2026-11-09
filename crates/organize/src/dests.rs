//! Named destinations (ISO 32000-2 §12.3.2.4): Acrobat's Destinations panel. A named destination
//! is a name a link, a bookmark or a URL (`file.pdf#nameddest=…`) can go to instead of a page; the
//! name stands for an explicit destination (`[page /XYZ left top zoom]`) or a dictionary whose
//! `/D` is one. Since PDF 1.2 they live in the catalog's `/Names /Dests` name tree (§7.9.6); PDF
//! 1.1 kept them in the catalog's `/Dests` dictionary, which readers still honour, so both are
//! read and edited here.
//!
//! A destination is addressed by its name as text. An edit applies to every entry of that name,
//! in the tree and in the old dictionary alike, so the one a reader finds first (the tree's) and
//! one it would fall back to never disagree. Edits rewrite the tree as one flat, sorted node, as
//! attachments are written; they are refused, changing nothing, when the tree is nested deeper or
//! is larger than it is read, since rewriting it would then drop the entries left unread.
//!
//! The document's own links, bookmarks and actions that name a destination keep working: a
//! rename gives them the new name, and a delete gives them the place the name stood for. Links
//! in other files and URLs cannot be reached, and go nowhere after either.

use std::collections::{HashMap, HashSet};

use pdfcraft_cos::{Dict, Document, ObjRef, Object, PdfString};

use crate::{OrganizeError, walk};

/// Deepest name-tree node read (the root is 0).
const TREE_DEPTH: usize = 32;

/// Most name-tree nodes read.
const TREE_NODES: usize = 100_000;

/// Most actions followed along one action's `/Next` chain (ISO 32000-2 §12.6.2).
const NEXT_DEPTH: usize = 64;

/// Longest destination name, in characters.
pub const MAX_DESTINATION_NAME: usize = 1024;

/// A named destination as the Destinations panel lists it.
#[derive(Clone, Debug, PartialEq)]
pub struct NamedDestination {
    /// The name as text.
    pub name: String,
    /// The 0-based page it goes to, when that page is in this document.
    pub page: Option<usize>,
    /// Where on that page it goes: the destination's type (`XYZ`, `Fit`, `FitH`…) and the numbers
    /// after it, in PDF user space (`None` for `null` or anything that isn't a number).
    pub view: Option<(Vec<u8>, Vec<Option<f64>>)>,
}

/// A name-tree entry: its key as written (a string, or, by some producers, a name), the key's
/// bytes, and its value.
struct Entry {
    key: Object,
    bytes: Vec<u8>,
    value: Object,
}

/// The `/Names /Dests` tree's entries in tree order, and whether all of them were read.
struct TreeEntries {
    entries: Vec<Entry>,
    complete: bool,
}

/// The catalog's `/Names` dictionary, if it has one.
fn names_dict(doc: &Document) -> Option<Dict> {
    let root = doc.root()?;
    let names = doc.get(root).as_dict()?.get(b"Names").cloned()?;
    doc.resolve(&names).as_dict().cloned()
}

/// Every entry of the `/Names /Dests` tree, read iteratively: each indirect node once, at most
/// [`TREE_NODES`] nodes, none deeper than [`TREE_DEPTH`].
fn tree_entries(doc: &Document) -> TreeEntries {
    let mut out = TreeEntries { entries: Vec::new(), complete: true };
    let Some(tree) = names_dict(doc).and_then(|n| n.get(b"Dests").cloned()) else { return out };
    let mut stack = vec![(tree, 0usize)];
    let mut seen: HashSet<ObjRef> = HashSet::new();
    let mut read = 0usize;
    while let Some((raw, depth)) = stack.pop() {
        if depth > TREE_DEPTH || read >= TREE_NODES {
            out.complete = false;
            continue;
        }
        // A node listed twice, or a cycle, is read once.
        if let Object::Ref(r) = raw
            && !seen.insert(r)
        {
            continue;
        }
        read += 1;
        let node = doc.resolve(&raw);
        let Some(d) = node.as_dict() else { continue };
        if let Some(pairs) = d.get(b"Names").map(|n| doc.resolve(n))
            && let Some(pairs) = pairs.as_array()
        {
            for pair in pairs.chunks(2) {
                if let [k, v] = pair {
                    let key = doc.resolve(k);
                    let bytes = match &*key {
                        Object::String(s) => s.bytes.clone(),
                        Object::Name(n) => n.clone(),
                        _ => continue,
                    };
                    out.entries.push(Entry { key: (*key).clone(), bytes, value: v.clone() });
                }
            }
        }
        if let Some(kids) = d.get(b"Kids").map(|k| doc.resolve(k))
            && let Some(kids) = kids.as_array()
        {
            // Pushed in reverse, so they are read in order.
            stack.extend(kids.iter().rev().map(|kid| (kid.clone(), depth + 1)));
        }
    }
    out
}

/// The catalog's PDF 1.1 `/Dests` dictionary, if it has one.
fn legacy_dict(doc: &Document) -> Option<Dict> {
    let root = doc.root()?;
    let dests = doc.get(root).as_dict()?.get(b"Dests").cloned()?;
    doc.resolve(&dests).as_dict().cloned()
}

/// A destination key as text: a text string's characters (a name's bytes read the same way, UTF-8
/// when they are).
fn key_text(key: &[u8]) -> String {
    PdfString::literal(key.to_vec()).to_text()
}

/// Whether `key` reads as `name`.
fn key_is(key: &[u8], name: &str) -> bool {
    PdfString::literal(key.to_vec()).text_eq(name)
}

/// The explicit destination array a named destination stands for: itself, or the `/D` of a
/// dictionary (ISO 32000-2 §12.3.2.4). A name standing for another name is not followed: that
/// is not a destination, and following it would look the whole tree up again for every entry.
fn explicit(doc: &Document, value: &Object) -> Option<Vec<Object>> {
    let value = doc.resolve(value);
    let array = match value.as_dict() {
        Some(d) => doc.resolve(d.get(b"D")?),
        None => value,
    };
    array.as_array().cloned()
}

/// The named destinations of `doc`, sorted by name: the tree's first, then the old dictionary's
/// that the tree doesn't have. A name found twice is listed once, as a reader finds it (the tree's
/// first entry).
pub fn named_destinations(doc: &Document) -> Vec<NamedDestination> {
    let pages: HashMap<ObjRef, usize> = walk(doc).unwrap_or_default().into_iter().enumerate().map(|(i, (r, _))| (r, i)).collect();
    let page_count = pages.len();
    let mut listed: HashSet<String> = HashSet::new();
    let mut out = Vec::new();
    let tree = tree_entries(doc).entries.into_iter().map(|e| (e.bytes, e.value));
    let legacy: Vec<(Vec<u8>, Object)> = legacy_dict(doc).map(|d| d.iter().map(|(k, v)| (k.clone(), v.clone())).collect()).unwrap_or_default();
    for (key, value) in tree.chain(legacy) {
        let name = key_text(&key);
        if !listed.insert(name.clone()) {
            continue;
        }
        let dest = explicit(doc, &value);
        let page = dest.as_deref().and_then(|d| match d.first()? {
            Object::Ref(r) => pages.get(r).copied(),
            // Some producers write a page number instead of the page.
            Object::Int(n) => usize::try_from(*n).ok().filter(|&p| p < page_count),
            _ => None,
        });
        let view = dest.as_deref().and_then(|d| crate::outline::view_of_explicit(doc, d));
        out.push(NamedDestination { name, page, view });
    }
    out.sort_by(|a, b| a.name.cmp(&b.name));
    out
}

/// A name a destination may be given: not blank, at most [`MAX_DESTINATION_NAME`] characters.
fn check_name(name: &str) -> Result<(), OrganizeError> {
    if name.trim().is_empty() {
        return Err(OrganizeError::Invalid("a destination needs a name".into()));
    }
    if name.chars().count() > MAX_DESTINATION_NAME {
        return Err(OrganizeError::Invalid(format!("a destination name can be at most {MAX_DESTINATION_NAME} characters")));
    }
    Ok(())
}

/// Whether any destination of `doc` is called `name`.
fn exists(doc: &Document, entries: &TreeEntries, name: &str) -> bool {
    entries.entries.iter().any(|e| key_is(&e.bytes, name)) || legacy_dict(doc).is_some_and(|d| d.iter().any(|(k, _)| key_is(k, name)))
}

/// The tree's entries, refused when some could not be read (rewriting would drop them).
fn editable_entries(doc: &Document) -> Result<TreeEntries, OrganizeError> {
    let entries = tree_entries(doc);
    if !entries.complete {
        return Err(OrganizeError::Invalid(format!(
            "the document's named destinations are nested more than {TREE_DEPTH} levels deep or spread over more than {TREE_NODES} parts, so they cannot be edited without losing some"
        )));
    }
    Ok(entries)
}

/// `/XYZ left top null` on `page` (0-based): the user-space point (`left`, `top`) at the window's
/// top-left corner at the reader's zoom, as a new bookmark goes; a coordinate left out keeps the
/// reader's position that way.
fn target(doc: &Document, page: usize, left: Option<f64>, top: Option<f64>) -> Result<Object, OrganizeError> {
    if ![left, top].iter().flatten().all(|v| v.is_finite()) {
        return Err(OrganizeError::Invalid("a destination's position must be a finite number".into()));
    }
    let pages = walk(doc)?;
    let (p, _) = pages.get(page).ok_or(OrganizeError::NoSuchPage(page))?;
    Ok(crate::outline::go_to_at(*p, left, top))
}

/// Write `entries` as the `/Names /Dests` tree: one node, sorted by key bytes (a stable sort, so
/// of equal keys the one a reader found first stays first), each key written as it was. No
/// entries removes the tree, and the catalog's `/Names` when nothing else is left in it.
fn write_tree(doc: &mut Document, mut entries: Vec<Entry>) -> Result<(), OrganizeError> {
    let root = doc.root().ok_or(OrganizeError::NoPageTree)?;
    let names_slot = doc.get(root).as_dict().and_then(|c| c.get(b"Names").cloned());
    let tree = if entries.is_empty() {
        None
    } else {
        entries.sort_by(|a, b| a.bytes.cmp(&b.bytes));
        let names = entries.into_iter().flat_map(|e| [e.key, e.value]).collect();
        let mut node = Dict::new();
        node.set(b"Names".to_vec(), Object::Array(names));
        Some(Object::Ref(doc.add(node)))
    };
    let put = |n: &mut Dict| match tree {
        Some(t) => n.set(b"Dests".to_vec(), t),
        None => {
            n.remove(b"Dests");
        }
    };
    match names_slot {
        // An indirect `/Names` is updated where it is, so its other trees stay.
        Some(Object::Ref(r)) if doc.get(r).as_dict().is_some() => {
            let mut empty = false;
            doc.update_dict(r, |n| {
                put(n);
                empty = n.is_empty();
            })?;
            if empty {
                doc.update_dict(root, |c| {
                    c.remove(b"Names");
                })?;
            }
        }
        other => {
            let mut names = match other {
                Some(Object::Dict(d)) => d,
                _ => Dict::new(),
            };
            put(&mut names);
            doc.update_dict(root, |c| {
                if names.is_empty() {
                    c.remove(b"Names");
                } else {
                    c.set(b"Names".to_vec(), Object::Dict(names));
                }
            })?;
        }
    }
    Ok(())
}

/// Change the old `/Dests` dictionary with `f`, where it is (an indirect dictionary in place, an
/// inline one in the catalog). Nothing happens when there is none or `f` changes nothing.
fn edit_legacy(doc: &mut Document, f: impl FnOnce(&mut Dict) -> bool) -> Result<(), OrganizeError> {
    let root = doc.root().ok_or(OrganizeError::NoPageTree)?;
    let Some(slot) = doc.get(root).as_dict().and_then(|c| c.get(b"Dests").cloned()) else { return Ok(()) };
    let Some(mut dests) = doc.resolve(&slot).as_dict().cloned() else { return Ok(()) };
    if !f(&mut dests) {
        return Ok(());
    }
    match slot {
        Object::Ref(r) => doc.set(r, Object::Dict(dests)),
        _ => doc.update_dict(root, |c| c.set(b"Dests".to_vec(), Object::Dict(dests)))?,
    }
    Ok(())
}

/// The keys of the old dictionary that read as `name`.
fn legacy_keys(dests: &Dict, name: &str) -> Vec<Vec<u8>> {
    dests.iter().filter(|(k, _)| key_is(k, name)).map(|(k, _)| k.clone()).collect()
}

/// `value` sent to `dest` instead: a dictionary keeps its other entries (a structure destination
/// `/SD`, say) with `/D` replaced; anything else becomes `dest`.
fn retarget(doc: &Document, value: &Object, dest: &Object) -> Object {
    match doc.resolve(value).as_dict() {
        Some(d) => {
            let mut d = d.clone();
            d.set(b"D".to_vec(), dest.clone());
            Object::Dict(d)
        }
        None => dest.clone(),
    }
}

/// Destinations panel ▸ New Destination: name the view at (`left`, `top`) on `page` (0-based)
/// `name`, at the reader's zoom. Refused when a destination already has that name.
pub fn add_named_destination(doc: &mut Document, name: &str, page: usize, left: Option<f64>, top: Option<f64>) -> Result<(), OrganizeError> {
    check_name(name)?;
    let dest = target(doc, page, left, top)?;
    let mut entries = editable_entries(doc)?;
    if exists(doc, &entries, name) {
        return Err(OrganizeError::Invalid(format!("a destination named {name} already exists")));
    }
    let key = PdfString::text(name);
    entries.entries.push(Entry { bytes: key.bytes.clone(), key: Object::String(key), value: dest });
    write_tree(doc, entries.entries)
}

/// Destinations panel ▸ Set Destination: send the destination `name` to the view at (`left`,
/// `top`) on `page` (0-based).
pub fn set_named_destination(doc: &mut Document, name: &str, page: usize, left: Option<f64>, top: Option<f64>) -> Result<(), OrganizeError> {
    let dest = target(doc, page, left, top)?;
    let mut entries = editable_entries(doc)?;
    if !exists(doc, &entries, name) {
        return Err(no_such(name));
    }
    let mut in_tree = false;
    for e in entries.entries.iter_mut().filter(|e| key_is(&e.bytes, name)) {
        e.value = retarget(doc, &e.value, &dest);
        in_tree = true;
    }
    if in_tree {
        write_tree(doc, entries.entries)?;
    }
    let values: Vec<(Vec<u8>, Object)> = legacy_dict(doc)
        .map(|d| legacy_keys(&d, name).into_iter().filter_map(|k| d.get(&k).map(|v| (k, retarget(doc, v, &dest)))).collect())
        .unwrap_or_default();
    edit_legacy(doc, |d| {
        for (k, v) in &values {
            d.set(k.clone(), v.clone());
        }
        !values.is_empty()
    })
}

/// Destinations panel ▸ Rename: call the destination `name` `new_name`, keeping where it goes.
/// Refused when another destination already has the new name. The document's links, bookmarks
/// and actions that named it name it by the new name.
pub fn rename_named_destination(doc: &mut Document, name: &str, new_name: &str) -> Result<(), OrganizeError> {
    check_name(new_name)?;
    let mut entries = editable_entries(doc)?;
    if !exists(doc, &entries, name) {
        return Err(no_such(name));
    }
    if exists(doc, &entries, new_name) {
        return Err(OrganizeError::Invalid(format!("a destination named {new_name} already exists")));
    }
    // A string key in the tree is a text string; a name key (the old dictionary's, or a tree's
    // written as a name) is UTF-8, as names are (ISO 32000-2 §7.3.5).
    let tree_key = PdfString::text(new_name).bytes;
    let name_key = new_name.as_bytes().to_vec();
    let mut old_keys: HashSet<Vec<u8>> = HashSet::new();
    let mut in_tree = false;
    for e in entries.entries.iter_mut().filter(|e| key_is(&e.bytes, name)) {
        let (key, bytes) = match &e.key {
            Object::Name(_) => (Object::Name(name_key.clone()), name_key.clone()),
            Object::String(s) => (Object::String(PdfString { bytes: tree_key.clone(), hex: s.hex }), tree_key.clone()),
            _ => (Object::String(PdfString::literal(tree_key.clone())), tree_key.clone()),
        };
        old_keys.insert(std::mem::replace(&mut e.bytes, bytes));
        e.key = key;
        in_tree = true;
    }
    if in_tree {
        write_tree(doc, entries.entries)?;
    }
    let legacy = legacy_dict(doc).map(|d| legacy_keys(&d, name)).unwrap_or_default();
    let in_legacy = !legacy.is_empty();
    old_keys.extend(legacy.iter().cloned());
    edit_legacy(doc, |d| {
        for k in &legacy {
            if let Some(v) = d.remove(k) {
                d.set(name_key.clone(), v);
            }
        }
        in_legacy
    })?;
    // A string names a tree entry and a name an entry of the old dictionary (§12.3.2.4); a
    // reference keeps its kind unless the destination is only in the other place.
    rewrite_references(doc, &old_keys, &|found| {
        let as_string = match found {
            Object::String(_) => in_tree,
            _ => !in_legacy,
        };
        let hex = matches!(found, Object::String(s) if s.hex);
        Some(if as_string { Object::String(PdfString { bytes: tree_key.clone(), hex }) } else { Object::Name(name_key.clone()) })
    })
}

/// Destinations panel ▸ Delete: remove the destination `name`. The document's links, bookmarks
/// and actions that named it go where it went instead.
pub fn delete_named_destination(doc: &mut Document, name: &str) -> Result<(), OrganizeError> {
    let mut entries = editable_entries(doc)?;
    if !exists(doc, &entries, name) {
        return Err(no_such(name));
    }
    let legacy = legacy_dict(doc);
    let legacy_named = legacy.as_ref().map(|d| legacy_keys(d, name)).unwrap_or_default();
    // Where a reader went: the tree's first entry of the name, else the old dictionary's.
    let went = entries
        .entries
        .iter()
        .find(|e| key_is(&e.bytes, name))
        .map(|e| e.value.clone())
        .or_else(|| legacy_named.first().and_then(|k| legacy.as_ref()?.get(k).cloned()))
        .and_then(|v| explicit(doc, &v));
    let mut old_keys: HashSet<Vec<u8>> = legacy_named.iter().cloned().collect();
    let before = entries.entries.len();
    entries.entries.retain(|e| {
        let keep = !key_is(&e.bytes, name);
        if !keep {
            old_keys.insert(e.bytes.clone());
        }
        keep
    });
    if entries.entries.len() != before {
        write_tree(doc, entries.entries)?;
    }
    edit_legacy(doc, |d| {
        for k in &legacy_named {
            d.remove(k);
        }
        !legacy_named.is_empty()
    })?;
    // One that went nowhere leaves its references going nowhere, as they did.
    match went {
        Some(dest) => rewrite_references(doc, &old_keys, &|_| Some(Object::Array(dest.clone()))),
        None => Ok(()),
    }
}

fn no_such(name: &str) -> OrganizeError {
    OrganizeError::Invalid(format!("there is no destination named {name}"))
}

/// Give every reference to a named destination with one of `keys` (a link's or bookmark's
/// `/Dest`, or the `/D` of a GoTo action) what `replace` makes of it. The actions looked at are
/// those of bookmarks, of the annotations on the pages (`/A` and `/AA`), of the pages (`/AA`) and
/// of the catalog (`/OpenAction`, `/AA`), with the actions they chain to by `/Next`. Each
/// indirect object is visited once in each role, so shared and cyclic actions are safe, and only
/// the entries that change are written back.
fn rewrite_references(doc: &mut Document, keys: &HashSet<Vec<u8>>, replace: &dyn Fn(&Object) -> Option<Object>) -> Result<(), OrganizeError> {
    if keys.is_empty() {
        return Ok(());
    }
    let mut holders = crate::outline::items(doc);
    for (page, _) in walk(doc)? {
        holders.push(page);
        let annots = doc.get(page).as_dict().and_then(|d| d.get(b"Annots").cloned()).map(|a| doc.resolve(&a));
        if let Some(annots) = annots.as_deref().and_then(Object::as_array) {
            holders.extend(annots.iter().filter_map(Object::as_ref));
        }
    }
    let mut r = Rewriter { keys, replace, seen: HashSet::new() };
    for h in holders {
        r.holder(doc, h)?;
    }
    let Some(root) = doc.root() else { return Ok(()) };
    if !r.seen.insert((root, Role::Holder)) {
        return Ok(());
    }
    let Some(catalog) = doc.get(root).as_dict().cloned() else { return Ok(()) };
    let mut changes = Vec::new();
    if let Some(aa) = catalog.get(b"AA")
        && let Some(new) = r.triggers(doc, aa)?
    {
        changes.push((b"AA".to_vec(), new));
    }
    // The opening action is a destination or an action (ISO 32000-2 Table 29).
    if let Some(open) = catalog.get(b"OpenAction") {
        let new = match r.dest(doc, open) {
            Some(new) => Some(new),
            None => r.action(doc, open, 0)?,
        };
        if let Some(new) = new {
            changes.push((b"OpenAction".to_vec(), new));
        }
    }
    write_changes(doc, root, changes)
}

/// What an indirect object is visited as: one object can be all three in a hostile file.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
enum Role {
    /// A bookmark, annotation, page or the catalog.
    Holder,
    /// An additional-actions (`/AA`) dictionary.
    Triggers,
    Action,
}

/// Set `changes` on the dictionary `r` as it is now, so that a change an inner visit made to the
/// same object is kept.
fn write_changes(doc: &mut Document, r: ObjRef, changes: Vec<(Vec<u8>, Object)>) -> Result<(), OrganizeError> {
    if !changes.is_empty() {
        doc.update_dict(r, |d| {
            for (k, v) in changes {
                d.set(k, v);
            }
        })?;
    }
    Ok(())
}

/// The walk [`rewrite_references`] makes.
struct Rewriter<'a> {
    keys: &'a HashSet<Vec<u8>>,
    replace: &'a dyn Fn(&Object) -> Option<Object>,
    seen: HashSet<(ObjRef, Role)>,
}

impl Rewriter<'_> {
    /// `dest` replaced, when it names one of the keys.
    fn dest(&self, doc: &Document, dest: &Object) -> Option<Object> {
        let found = doc.resolve(dest);
        let named = match &*found {
            Object::String(s) => self.keys.contains(&s.bytes),
            Object::Name(n) => self.keys.contains(n),
            _ => false,
        };
        if named { (self.replace)(&found) } else { None }
    }

    /// A bookmark, annotation or page: its `/Dest`, `/A` and `/AA`.
    fn holder(&mut self, doc: &mut Document, h: ObjRef) -> Result<(), OrganizeError> {
        if !self.seen.insert((h, Role::Holder)) {
            return Ok(());
        }
        let Some(d) = doc.get(h).as_dict().cloned() else { return Ok(()) };
        let mut changes = Vec::new();
        if let Some(new) = d.get(b"Dest").and_then(|v| self.dest(doc, v)) {
            changes.push((b"Dest".to_vec(), new));
        }
        if let Some(a) = d.get(b"A")
            && let Some(new) = self.action(doc, a, 0)?
        {
            changes.push((b"A".to_vec(), new));
        }
        if let Some(aa) = d.get(b"AA")
            && let Some(new) = self.triggers(doc, aa)?
        {
            changes.push((b"AA".to_vec(), new));
        }
        write_changes(doc, h, changes)
    }

    /// An `/AA` dictionary's actions: an indirect dictionary is changed in place; an inline one
    /// that changed is handed back.
    fn triggers(&mut self, doc: &mut Document, aa: &Object) -> Result<Option<Object>, OrganizeError> {
        if let Object::Ref(r) = aa
            && !self.seen.insert((*r, Role::Triggers))
        {
            return Ok(None);
        }
        let Some(triggers) = doc.resolve(aa).as_dict().cloned() else { return Ok(None) };
        let mut changes = Vec::new();
        for (k, v) in triggers.iter() {
            if let Some(new) = self.action(doc, v, 0)? {
                changes.push((k.clone(), new));
            }
        }
        inline_or_in_place(doc, aa, triggers, changes)
    }

    /// An action, and those it chains to: an indirect one is changed in place; an inline one
    /// that changed is handed back.
    fn action(&mut self, doc: &mut Document, action: &Object, depth: usize) -> Result<Option<Object>, OrganizeError> {
        if depth > NEXT_DEPTH {
            return Ok(None);
        }
        let d = match action {
            Object::Ref(r) if self.seen.insert((*r, Role::Action)) => doc.get(*r).as_dict().cloned(),
            Object::Dict(d) => Some(d.clone()),
            _ => None,
        };
        let Some(d) = d else { return Ok(None) };
        let mut changes = Vec::new();
        // GoToR and GoToE name destinations in other files: those are not this document's.
        if d.name(b"S") == Some(b"GoTo")
            && let Some(new) = d.get(b"D").and_then(|v| self.dest(doc, v))
        {
            changes.push((b"D".to_vec(), new));
        }
        match d.get(b"Next") {
            Some(Object::Array(items)) => {
                let mut items = items.clone();
                let mut any = false;
                for item in &mut items {
                    if let Some(new) = self.action(doc, item, depth + 1)? {
                        *item = new;
                        any = true;
                    }
                }
                if any {
                    changes.push((b"Next".to_vec(), Object::Array(items)));
                }
            }
            Some(next) => {
                if let Some(new) = self.action(doc, next, depth + 1)? {
                    changes.push((b"Next".to_vec(), new));
                }
            }
            None => {}
        }
        inline_or_in_place(doc, action, d, changes)
    }
}

/// `changes` made to `held`, the dictionary `slot` holds: written in place when `slot` is a
/// reference, else handed back as the changed inline dictionary (`None` when nothing changed).
fn inline_or_in_place(doc: &mut Document, slot: &Object, mut held: Dict, changes: Vec<(Vec<u8>, Object)>) -> Result<Option<Object>, OrganizeError> {
    if changes.is_empty() {
        return Ok(None);
    }
    if let Object::Ref(r) = slot {
        write_changes(doc, *r, changes)?;
        return Ok(None);
    }
    for (k, v) in changes {
        held.set(k, v);
    }
    Ok(Some(Object::Dict(held)))
}
