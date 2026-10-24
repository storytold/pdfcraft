//! Copying pages between documents: the basis of Combine Files, Insert Pages from File,
//! Extract Pages and Split.
//!
//! A page is deep-copied with everything it references (content streams, resources, fonts,
//! images, annotations), renumbered into the destination. Objects shared between the copied
//! pages (a font used on every page) are copied once. The copy never follows `/Parent` from a
//! page, so the source page tree does not come along. It rewires what points *back* into the
//! document:
//! - an annotation's `/P` points at its new page;
//! - link and GoTo destinations that target a copied page point at the copy. Links to pages that
//!   were not copied are dropped, so they don't dangle;
//! - form-field widgets keep their field hierarchy, and the top-level fields are registered in the
//!   destination `/AcroForm`;
//! - structure-tree links (`/StructParent(s)`) and article beads (`/B`) are removed, because the
//!   source structure tree is not copied;
//! - named destinations are resolved against the source's name tree and become explicit ones;
//! - layers (optional content groups) used by the pages are registered in the destination's
//!   `/OCProperties`, keeping their default on/off state.

use std::collections::{HashMap, HashSet};

use pdfcraft_cos::{Dict, Document, ObjRef, Object};

use crate::{INHERITABLE, OrganizeError, pages_root, rebuild, walk};

/// Keys that must not be followed when copying: they point back up or across the source
/// document, and following them would drag in unrelated objects.
const SKIP_ON_PAGE: &[&[u8]] = &[b"Parent", b"B", b"StructParents", b"Thumb", b"PieceInfo"];
const SKIP_ON_ANNOT: &[&[u8]] = &[b"StructParent", b"P"];

/// How deep a copy follows references and nested arrays and dictionaries, counted together.
/// Real documents stay well under this (a page's resources, fonts and descriptors, nested
/// form XObjects and field trees are a few levels each); a chain of thousands of indirect
/// objects, each shallow enough for the parser, would otherwise exhaust the stack (#1800).
const MAX_COPY_DEPTH: usize = 256;

/// Keys that stay on the widget when a combined field/widget is split (#1810): the annotation's
/// appearance, placement and page. Field state (name, value, flags, default appearance) moves to
/// the new parent field, where the widgets inherit it.
const WIDGET_KEYS: &[&[u8]] = &[
    b"Type",
    b"Subtype",
    b"Rect",
    b"P",
    b"AP",
    b"AS",
    b"MK",
    b"BS",
    b"Border",
    b"C",
    b"CA",
    b"F",
    b"H",
    b"R",
    b"NM",
    b"M",
    b"StructParent",
    b"StructParents",
    b"OC",
    b"Tabs",
    b"A",
    b"Contents",
    b"BM",
    b"Lang",
    b"AF",
];

struct Copier<'a> {
    src: &'a Document,
    /// Current recursion depth, bounded by [`MAX_COPY_DEPTH`].
    depth: usize,
    map: HashMap<ObjRef, ObjRef>,
    /// Source page → destination page, for rewriting `/P` and destinations.
    pages: HashMap<ObjRef, ObjRef>,
    /// Destination objects that are annotation dictionaries (fixed up after copying).
    annots: Vec<ObjRef>,
    /// Top-level form fields reached through widgets.
    fields: Vec<ObjRef>,
    /// Optional content groups copied: (source, destination).
    ocgs: Vec<(ObjRef, ObjRef)>,
}

impl<'a> Copier<'a> {
    fn new(src: &'a Document, map: HashMap<ObjRef, ObjRef>, pages: HashMap<ObjRef, ObjRef>) -> Self {
        Copier { src, depth: 0, map, pages, annots: Vec::new(), fields: Vec::new(), ocgs: Vec::new() }
    }

    /// Copy an indirect object (once), returning its destination reference.
    fn copy_ref(&mut self, dst: &mut Document, r: ObjRef) -> Result<ObjRef, OrganizeError> {
        if let Some(d) = self.map.get(&r) {
            return Ok(*d);
        }
        // Reserve the number first so cycles (field ↔ widget) terminate.
        let new = dst.add(Object::Null);
        self.map.insert(r, new);
        let obj = self.src.get(r);
        if obj.as_dict().is_some_and(|d| d.name(b"Type") == Some(b"OCG")) {
            self.ocgs.push((r, new));
        }
        let copied = self.copy_value(dst, &obj, Some(new))?;
        dst.set(new, copied);
        Ok(new)
    }

    /// One level deeper into the object graph, or the refusal when there are too many.
    fn descend(&mut self) -> Result<(), OrganizeError> {
        if self.depth >= MAX_COPY_DEPTH {
            return Err(OrganizeError::Invalid(format!("the pages refer to objects nested more than {MAX_COPY_DEPTH} levels deep")));
        }
        self.depth += 1;
        Ok(())
    }

    fn copy_value(&mut self, dst: &mut Document, o: &Object, this: Option<ObjRef>) -> Result<Object, OrganizeError> {
        if !matches!(o, Object::Ref(_) | Object::Array(_) | Object::Dict(_) | Object::Stream(_)) {
            return Ok(o.clone());
        }
        self.descend()?;
        let copied = match o {
            Object::Ref(r) => self.copy_ref(dst, *r).map(Object::Ref),
            Object::Array(a) => a.iter().map(|x| self.copy_value(dst, x, None)).collect::<Result<Vec<_>, _>>().map(Object::Array),
            Object::Dict(d) => self.copy_dict(dst, d, this).map(Object::Dict),
            Object::Stream(s) => self.copy_dict(dst, &s.dict, None).map(|dict| {
                let mut s = s.clone();
                s.dict = dict;
                Object::Stream(s)
            }),
            other => Ok(other.clone()),
        };
        self.depth = self.depth.saturating_sub(1);
        copied
    }

    fn copy_dict(&mut self, dst: &mut Document, d: &Dict, this: Option<ObjRef>) -> Result<Dict, OrganizeError> {
        let is_annot = d.name(b"Type") == Some(b"Annot") || (d.contains(b"Subtype") && d.contains(b"Rect") && !d.contains(b"Type"));
        let is_field = d.contains(b"FT") || (d.contains(b"T") && d.contains(b"Kids"));
        let mut out = Dict::new();
        for (k, v) in d.iter() {
            if is_annot && SKIP_ON_ANNOT.contains(&k.as_slice()) {
                continue;
            }
            // Destinations are rewritten afterwards (they may target pages copied later).
            if is_annot && (k.as_slice() == b"Dest" || k.as_slice() == b"A") {
                out.set(k.clone(), v.clone());
                continue;
            }
            out.set(k.clone(), self.copy_value(dst, v, None)?);
        }
        if let Some(this) = this {
            if is_annot {
                self.annots.push(this);
            }
            // A field (or widget) without a parent is a top-level field.
            if is_field && !d.contains(b"Parent") {
                self.fields.push(this);
            }
        }
        Ok(out)
    }

    /// Map a source destination (explicit array or reference to one) to the destination
    /// document; `None` if it targets a page that was not copied.
    fn map_dest(&self, dest: &Object) -> Option<Object> {
        self.map_dest_depth(dest, 0)
    }

    fn map_dest_depth(&self, dest: &Object, depth: u8) -> Option<Object> {
        if depth > 4 {
            return None;
        }
        match dest {
            Object::Name(n) => self.map_dest_depth(&named_dest(self.src, n)?, depth + 1),
            Object::String(s) => self.map_dest_depth(&named_dest(self.src, &s.bytes)?, depth + 1),
            Object::Ref(_) => self.map_dest_depth(&self.src.resolve(dest), depth + 1),
            Object::Dict(d) => self.map_dest_depth(d.get(b"D")?, depth + 1),
            Object::Array(a) => {
                let page = a.first()?.as_ref()?;
                let new_page = *self.pages.get(&page)?;
                let mut a = a.clone();
                a[0] = Object::Ref(new_page);
                Some(Object::Array(a))
            }
            _ => None,
        }
    }

    /// Rewrite `/P`, `/Dest` and GoTo actions on copied annotations.
    fn fix_annotations(&mut self, dst: &mut Document, annot_pages: &HashMap<ObjRef, ObjRef>) -> Result<(), OrganizeError> {
        for &a in &std::mem::take(&mut self.annots) {
            let src_dict = dst.get(a).as_dict().cloned();
            let Some(d) = src_dict else { continue };
            let mut d = d;
            if let Some(p) = annot_pages.get(&a) {
                d.set(b"P".to_vec(), Object::Ref(*p));
            }
            if let Some(dest) = d.get(b"Dest").cloned() {
                let resolved = self.src.resolve(&dest);
                match self.map_dest(&resolved) {
                    Some(nd) => d.set(b"Dest".to_vec(), nd),
                    None => {
                        d.remove(b"Dest");
                    }
                }
            }
            if let Some(action) = d.get(b"A").cloned() {
                let action = self.src.resolve(&action);
                if let Some(ad) = action.as_dict() {
                    if ad.name(b"S") == Some(b"GoTo") {
                        let target = ad.get(b"D").map(|x| self.src.resolve(x));
                        match target.and_then(|t| self.map_dest(&t)) {
                            Some(nd) => {
                                let mut na = Dict::new();
                                na.set(b"S".to_vec(), Object::name("GoTo"));
                                na.set(b"D".to_vec(), nd);
                                d.set(b"A".to_vec(), Object::Dict(na));
                            }
                            None => {
                                d.remove(b"A");
                            }
                        }
                    } else {
                        // Other actions (URI, Launch, JavaScript…) carry no page references. The
                        // copier lends its map out and takes it back with the new copies in it,
                        // so actions sharing an object share its one copy.
                        let mut c = Copier::new(self.src, std::mem::take(&mut self.map), HashMap::new());
                        let copied = c.copy_value(dst, &Object::Dict(ad.clone()), None);
                        self.map = c.map;
                        d.set(b"A".to_vec(), copied?);
                    }
                }
            }
            dst.set(a, Object::Dict(d));
        }
        Ok(())
    }
}

/// Copy `o` from `src` into `dst`, reusing the copies listed in `map` (source → destination), so
/// an object the copied pages already brought along (an ICC profile) is not stored twice.
pub(crate) fn copy_object(dst: &mut Document, src: &Document, o: &Object, map: HashMap<ObjRef, ObjRef>) -> Result<Object, OrganizeError> {
    Copier::new(src, map, HashMap::new()).copy_value(dst, o, None)
}

/// How colliding fully-qualified field names are resolved when pages are imported.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum FieldConflicts {
    /// The incoming field's own `/T` is renamed to the first free `Name-1`, `Name-2`… variant,
    /// so it stays its own field (the default; Combine and Insert from File use it).
    #[default]
    Rename,
    /// The older behaviour: a field that arrives with a taken name becomes one field with the
    /// one already there. Duplicate, Replace, Extract and Split always use it.
    Merge,
}

/// What [`import_pages_with`] did beyond placing the pages.
pub struct ImportOutcome {
    /// The new pages, in order.
    pub pages: Vec<ObjRef>,
    /// Every field renamed to keep its name separate: old fully-qualified name → new one.
    pub renamed_fields: Vec<(String, String)>,
}

/// Copy pages `src_pages` (0-based, in the given order) of `src` into `dst`, inserting them at
/// position `at`, resolving field-name collisions per `conflicts`. Returns the new page
/// references in order and what the conflict resolution renamed.
///
/// Resources identical to ones already in `dst` (the same fonts, images, colour profiles) are
/// shared rather than stored twice (see `dedupe`).
pub fn import_pages_with(
    dst: &mut Document,
    src: &Document,
    src_pages: &[usize],
    at: usize,
    conflicts: FieldConflicts,
) -> Result<ImportOutcome, OrganizeError> {
    let before: HashSet<u32> = dst.object_numbers().into_iter().collect();
    let imported = match import_pages_mapped(dst, src, src_pages, at, conflicts) {
        Ok(imported) => imported,
        Err(e) => {
            // A refusal while copying (a source too deep to follow) has only added objects:
            // take them back, so the destination is as it was.
            for r in added_objects(dst, &before) {
                dst.free(r);
            }
            return Err(e);
        }
    };
    let created = added_objects(dst, &before);
    crate::dedupe::dedupe_resources(dst, &created, true);
    Ok(ImportOutcome { pages: imported.pages, renamed_fields: imported.renamed_fields })
}

/// The objects in `dst` that are not among `before`, the numbers it held before an import: the
/// ones the import added. They are not always numbered above the others: when no number is left
/// above the highest (the document holds object 4294967295, or its `/Size` is exhausted),
/// [`Document::add`] fills free numbers below it.
fn added_objects(dst: &Document, before: &HashSet<u32>) -> Vec<ObjRef> {
    dst.object_numbers().into_iter().filter(|n| !before.contains(n)).map(|n| ObjRef::new(n, dst.generation(n))).collect()
}

/// Insert Pages from File: [`import_pages_with`], and the inserted file's bookmarks come along
/// under one new bookmark titled `title` (the file's name) at its first inserted page, as Combine
/// Files does for each file. That bookmark goes among the top-level bookmarks where the pages went:
/// before the first one that points after them. Bookmarks to pages left out lose their destination;
/// a file without bookmarks adds none.
pub fn insert_file_pages(
    dst: &mut Document,
    src: &Document,
    title: &str,
    src_pages: &[usize],
    at: usize,
    conflicts: FieldConflicts,
) -> Result<ImportOutcome, OrganizeError> {
    let before: HashSet<u32> = dst.object_numbers().into_iter().collect();
    let imported = import_pages_mapped(dst, src, src_pages, at, conflicts)?;
    let created = added_objects(dst, &before);
    crate::dedupe::dedupe_resources(dst, &created, true);
    let outline = source_outline(src, imported.page_map);
    if let Some(first) = imported.pages.first()
        && !outline.is_empty()
    {
        let after = at.min(crate::page_count(dst)?.saturating_sub(imported.pages.len())).saturating_add(imported.pages.len());
        insert_file_bookmark(dst, &Mark { title: title.to_string(), page: *first, outline }, after)?;
    }
    Ok(ImportOutcome { pages: imported.pages, renamed_fields: imported.renamed_fields })
}

/// Add `mark` (a file's bookmark holding its own, collapsed) to the top level of `doc`'s outline,
/// before the first top-level bookmark that points at page `after` (0-based) or later.
fn insert_file_bookmark(doc: &mut Document, mark: &Mark, after: usize) -> Result<(), OrganizeError> {
    let index = crate::outline::top_level_bookmarks(doc).iter().position(|b| b.page.is_some_and(|p| p >= after));
    let root = crate::outline::container(doc, &[])?;
    let item = doc.add(Object::Null);
    let mut d = Dict::new();
    d.set(b"Title".to_vec(), Object::String(pdfcraft_cos::PdfString::text(&mark.title)));
    d.set(b"Dest".to_vec(), Object::Array(vec![Object::Ref(mark.page), Object::name("Fit")]));
    if let Some((f, l, n)) = write_outline_level(doc, item, &mark.outline)? {
        d.set(b"First".to_vec(), Object::Ref(f));
        d.set(b"Last".to_vec(), Object::Ref(l));
        d.set(b"Count".to_vec(), Object::Int(-(n as i64))); // collapsed
    }
    doc.set(item, Object::Dict(d));
    let mut top = crate::outline::children(doc, root);
    top.insert(index.unwrap_or(top.len()).min(top.len()), item);
    crate::outline::relink_and_recount(doc, root, &top)?;
    Ok(())
}

/// Copy pages `src_pages` (0-based, in the given order) of `src` into `dst`, inserting them at
/// position `at`. Returns the new page references in order.
///
/// Fields keep their names ([`FieldConflicts::Merge`]): this is what Duplicate and Replace
/// Pages use, where the incoming fields are copies of the document's own and renaming them
/// would silently change what the form exports. Combine and Insert from File, which bring in
/// another document's fields, use [`import_pages_with`] and [`FieldConflicts::Rename`].
///
/// Resources identical to ones already in `dst` (the same fonts, images, colour profiles) are
/// shared rather than stored twice (see `dedupe`).
pub fn import_pages(dst: &mut Document, src: &Document, src_pages: &[usize], at: usize) -> Result<Vec<ObjRef>, OrganizeError> {
    import_pages_with(dst, src, src_pages, at, FieldConflicts::Merge).map(|o| o.pages)
}

/// What `import_pages_mapped` copied.
struct Imported {
    /// The new pages, in order.
    pages: Vec<ObjRef>,
    /// Source page → destination page.
    page_map: HashMap<ObjRef, ObjRef>,
    /// Every source object copied → its copy.
    objects: HashMap<ObjRef, ObjRef>,
    /// Fields renamed to keep their names separate (old → new fully-qualified name).
    renamed_fields: Vec<(String, String)>,
}

/// `import_pages`, also returning what was copied from where.
fn import_pages_mapped(
    dst: &mut Document,
    src: &Document,
    src_pages: &[usize],
    at: usize,
    conflicts: FieldConflicts,
) -> Result<Imported, OrganizeError> {
    let source = walk(src)?;
    if let Some(bad) = src_pages.iter().find(|i| **i >= source.len()) {
        return Err(OrganizeError::NoSuchPage(*bad));
    }
    let mut existing = walk(dst)?;
    let root = pages_root(dst)?;
    let mut copier = Copier::new(src, HashMap::new(), HashMap::new());
    // Allocate every destination page first, so destinations between copied pages resolve.
    let targets: Vec<(ObjRef, ObjRef, Dict)> = src_pages
        .iter()
        .map(|&i| {
            let (page, inherited) = &source[i];
            let new = match copier.pages.get(page) {
                Some(n) => *n, // the same page requested twice gets a second copy below
                None => dst.add(Object::Null),
            };
            copier.pages.insert(*page, new);
            (*page, new, inherited.clone())
        })
        .collect();
    let mut annot_pages = HashMap::new();
    let mut new_pages = Vec::new();
    let mut done: HashMap<ObjRef, ObjRef> = HashMap::new();
    for (page, new, inherited) in targets {
        // A page listed twice: make a second, independent page object sharing resources, and
        // with copies of its own annotations (an annotation belongs to one page: its `/P`, its
        // popup, a widget's place). Destinations still map to the pages copied.
        let repeat = done.contains_key(&page);
        let new = if repeat { dst.add(Object::Null) } else { new };
        let mut own = repeat.then(|| Copier::new(src, HashMap::new(), copier.pages.clone()));
        let src_dict = src.get(page).as_dict().cloned().unwrap_or_default();
        let resources = resources_with_ancestors(src, &src_dict);
        let mut d = Dict::new();
        for (k, v) in src_dict.iter() {
            if SKIP_ON_PAGE.contains(&k.as_slice()) || (k.as_slice() == b"Resources" && resources.is_some()) {
                continue;
            }
            if k.as_slice() == b"Annots" {
                let list = src.resolve(v).as_array().cloned().unwrap_or_default();
                let mut out = Vec::new();
                for a in list {
                    // On a repeated page a form field's widget becomes one more widget of the
                    // same field (copied already with the page's first copy): one field, one
                    // value, shown on both pages. Copying the field again would make two fields
                    // of one name, filled in separately.
                    let resolved = src.resolve(&a);
                    let field = resolved.as_dict().filter(|d| d.name(b"Subtype") == Some(b"Widget")).and_then(|d| d.reference(b"Parent"));
                    if repeat
                        && let Some(parent) = field
                        && let Some(copied) = copier.map.get(&parent).copied()
                    {
                        let r = dst.add(Object::Null);
                        let c = copier.copy_value(dst, &resolved, Some(r))?;
                        dst.set(r, c);
                        add_kid(dst, copied, r)?;
                        annot_pages.insert(r, new);
                        out.push(Object::Ref(r));
                        continue;
                    }
                    // A widget that is its own field (no parent to hang a second widget on): on
                    // its first copy it is split into a parent field and a widget child (#1810);
                    // every later copy of the page gets one more widget child of that field, so
                    // the pages no longer share one widget whose `/P` names only the last page.
                    let own_field = resolved.as_dict().is_some_and(|d| d.name(b"Subtype") == Some(b"Widget") && !d.contains(b"Parent"));
                    if own_field
                        && let Some(first) = a.as_ref().and_then(|r| copier.map.get(&r).copied())
                        && let Some(parent) = dst.get(first).as_dict().and_then(|d| d.reference(b"Parent"))
                    {
                        let r = dst.add(Object::Null);
                        let c = copier.copy_value(dst, &resolved, Some(r))?;
                        let Object::Dict(d) = c else { continue };
                        let (_, mut d) = field_and_widget(dst, &d);
                        d.set(b"Parent".to_vec(), Object::Ref(parent));
                        // The new widget is not a field root of its own.
                        copier.fields.retain(|f| *f != r);
                        dst.set(r, Object::Dict(d));
                        add_kid(dst, parent, r)?;
                        annot_pages.insert(r, new);
                        out.push(Object::Ref(r));
                        continue;
                    }
                    let annots = if own_field { &mut copier } else { own.as_mut().unwrap_or(&mut copier) };
                    // Direct (inline) annotation dictionaries become indirect objects, so every
                    // copied annotation can be fixed up the same way.
                    let r = match a {
                        Object::Ref(r) => annots.copy_ref(dst, r)?,
                        Object::Dict(_) => {
                            let r = dst.add(Object::Null);
                            let c = annots.copy_value(dst, &a, Some(r))?;
                            dst.set(r, c);
                            r
                        }
                        _ => continue,
                    };
                    if own_field && copier.fields.contains(&r) {
                        split_own_field(dst, &mut copier.fields, r);
                    }
                    annot_pages.insert(r, new);
                    out.push(Object::Ref(r));
                }
                d.set(b"Annots".to_vec(), Object::Array(out));
                continue;
            }
            d.set(k.clone(), copier.copy_value(dst, v, None)?);
        }
        if let Some(r) = resources {
            let r = copier.copy_value(dst, &r, None)?;
            d.set(b"Resources".to_vec(), r);
        }
        for k in INHERITABLE {
            if !d.contains(k)
                && let Some(v) = inherited.get(k)
            {
                let v = copier.copy_value(dst, v, None)?;
                d.set(k.to_vec(), v);
            }
        }
        d.set(b"Type".to_vec(), Object::name("Page"));
        d.set(b"Parent".to_vec(), Object::Ref(root));
        dst.set(new, Object::Dict(d));
        // The repeat's own annotations are fixed up, its fields and layers registered, with the rest.
        if let Some(own) = own {
            copier.annots.extend(own.annots);
            copier.fields.extend(own.fields);
            copier.ocgs.extend(own.ocgs);
        }
        done.insert(page, new);
        new_pages.push(new);
    }
    copier.fix_annotations(dst, &annot_pages)?;
    let renamed_fields = match conflicts {
        FieldConflicts::Rename => resolve_field_conflicts(dst, &copier.fields)?,
        FieldConflicts::Merge => Vec::new(),
    };
    register_fields(dst, &copier.fields)?;
    register_layers(dst, src, &copier.ocgs)?;
    let at = at.min(existing.len());
    let inserted: Vec<(ObjRef, Dict)> = new_pages.iter().map(|r| (*r, Dict::new())).collect();
    existing.splice(at..at, inserted);
    rebuild(dst, &existing)?;
    Ok(Imported { pages: new_pages, page_map: copier.pages, objects: copier.map, renamed_fields })
}

/// A page's `/Resources` with the names that only its page-tree ancestors list, or `None` when
/// they add nothing to the dictionary it would have anyway (its own, or else its nearest
/// ancestor's). Readers (ours included) look a resource name up the page and then up the tree,
/// so a page that lists its font but inherits its images still shows them. A copy hangs from
/// another tree, so it must list them itself or it draws nothing of them (#1628). The page's
/// entry wins over an ancestor's, and a nearer ancestor's over a farther one's.
fn resources_with_ancestors(src: &Document, page: &Dict) -> Option<Object> {
    let mut merged = page.get(b"Resources").map(|r| src.dict(r).unwrap_or_default());
    let mut added = false;
    let mut seen = std::collections::HashSet::new();
    let mut parent = page.reference(b"Parent");
    // Bounded: a damaged tree's `/Parent` links can loop or run deep.
    while let Some(r) = parent
        && seen.insert(r)
        && seen.len() <= 256
    {
        let node = src.get(r);
        let Some(node) = node.as_dict() else { break };
        parent = node.reference(b"Parent");
        let Some(theirs) = node.get(b"Resources").and_then(|r| src.dict(r)) else { continue };
        let Some(mine) = merged.as_mut() else {
            merged = Some(theirs);
            continue;
        };
        for (category, entries) in theirs.iter() {
            match (mine.get(category).map(|c| src.dict(c)), src.dict(entries)) {
                (None, _) => {
                    mine.set(category.clone(), entries.clone());
                    added = true;
                }
                (Some(Some(mut names)), Some(entries)) => {
                    let before = names.len();
                    for (name, value) in entries.iter() {
                        if !names.contains(name) {
                            names.set(name.clone(), value.clone());
                        }
                    }
                    if names.len() > before {
                        mine.set(category.clone(), Object::Dict(names));
                        added = true;
                    }
                }
                // `/ProcSet` and the like: the page's own value stands.
                _ => {}
            }
        }
    }
    merged.filter(|_| added).map(Object::Dict)
}

/// Look up a named destination in the source (`/Dests` dictionary or `/Names /Dests` tree).
fn named_dest(src: &Document, name: &[u8]) -> Option<Object> {
    let catalog = src.get(src.root()?).as_dict().cloned()?;
    if let Some(dests) = catalog.get(b"Dests").map(|d| src.resolve(d))
        && let Some(v) = dests.as_dict().and_then(|d| d.get(name))
    {
        return Some(src.resolve(v).as_ref().clone());
    }
    let names = src.resolve(catalog.get(b"Names")?);
    let tree = names.as_dict()?.get(b"Dests")?.clone();
    name_tree_lookup(src, &tree, name, 0, &mut HashSet::new())
}

/// Name trees are read once per node: a node listed twice (or a cycle) is skipped.
fn name_tree_lookup(src: &Document, node: &Object, key: &[u8], depth: u8, seen: &mut HashSet<ObjRef>) -> Option<Object> {
    if depth > 32 || node.as_ref().is_some_and(|r| !seen.insert(r)) {
        return None;
    }
    #[cfg(test)]
    crate::work(1);
    let node = src.resolve(node);
    let d = node.as_dict()?;
    if let Some(names) = d.get(b"Names").map(|n| src.resolve(n)).and_then(|n| n.as_array().cloned()) {
        for pair in names.chunks(2) {
            if let [k, v] = pair
                && src.resolve(k).as_string().is_some_and(|s| s.bytes == key)
            {
                return Some(src.resolve(v).as_ref().clone());
            }
        }
    }
    let kids = d.get(b"Kids").map(|k| src.resolve(k)).and_then(|k| k.as_array().cloned()).unwrap_or_default();
    kids.iter().find_map(|k| name_tree_lookup(src, k, key, depth + 1, seen))
}

/// All (key, value) pairs of a name tree, in order, each node read once.
pub(crate) fn name_tree_entries(src: &Document, node: &Object, depth: u8, seen: &mut HashSet<ObjRef>, out: &mut Vec<(Vec<u8>, Object)>) {
    if depth > 32 || node.as_ref().is_some_and(|r| !seen.insert(r)) {
        return;
    }
    #[cfg(test)]
    crate::work(1);
    let node = src.resolve(node);
    let Some(d) = node.as_dict() else { return };
    if let Some(names) = d.get(b"Names").map(|n| src.resolve(n)).and_then(|n| n.as_array().cloned()) {
        for pair in names.chunks(2) {
            if let [k, v] = pair
                && let Some(k) = src.resolve(k).as_string()
            {
                out.push((k.bytes.clone(), v.clone()));
            }
        }
    }
    for k in d.get(b"Kids").map(|k| src.resolve(k)).and_then(|k| k.as_array().cloned()).unwrap_or_default() {
        name_tree_entries(src, &k, depth + 1, seen, out);
    }
}

/// Register copied optional content groups in the destination's `/OCProperties`, keeping each
/// layer's default visibility from the source.
fn register_layers(dst: &mut Document, src: &Document, ocgs: &[(ObjRef, ObjRef)]) -> Result<(), OrganizeError> {
    if ocgs.is_empty() {
        return Ok(());
    }
    let src_off: Vec<Object> = src
        .root()
        .and_then(|r| src.get(r).as_dict().cloned())
        .and_then(|c| c.get(b"OCProperties").map(|o| src.resolve(o)))
        .and_then(|o| o.as_dict().and_then(|d| d.get(b"D")).map(|d| src.resolve(d)))
        .and_then(|d| d.as_dict().and_then(|d| d.get(b"OFF")).map(|o| src.resolve(o)))
        .and_then(|o| o.as_array().cloned())
        .unwrap_or_default();
    let root = dst.root().ok_or(OrganizeError::NoPageTree)?;
    let catalog = dst.get(root).as_dict().cloned().ok_or(OrganizeError::NoPageTree)?;
    let (props_ref, mut props) = match catalog.get(b"OCProperties") {
        Some(Object::Ref(r)) => (Some(*r), dst.get(*r).as_dict().cloned().unwrap_or_default()),
        Some(Object::Dict(d)) => (None, d.clone()),
        _ => (None, Dict::new()),
    };
    let list = |d: &Dict, k: &[u8], doc: &Document| d.get(k).map(|v| doc.resolve(v)).and_then(|v| v.as_array().cloned()).unwrap_or_default();
    let mut all = list(&props, b"OCGs", dst);
    let mut config = props.get(b"D").map(|d| dst.resolve(d)).and_then(|d| d.as_dict().cloned()).unwrap_or_default();
    let (mut on, mut off, mut order) = (list(&config, b"ON", dst), list(&config, b"OFF", dst), list(&config, b"Order", dst));
    for (from, to) in ocgs {
        let r = Object::Ref(*to);
        if all.contains(&r) {
            continue;
        }
        all.push(r.clone());
        order.push(r.clone());
        if src_off.contains(&Object::Ref(*from)) { off.push(r) } else { on.push(r) }
    }
    props.set(b"OCGs".to_vec(), Object::Array(all));
    config.set(b"ON".to_vec(), Object::Array(on));
    config.set(b"OFF".to_vec(), Object::Array(off));
    config.set(b"Order".to_vec(), Object::Array(order));
    props.set(b"D".to_vec(), Object::Dict(config));
    match props_ref {
        Some(r) => dst.set(r, Object::Dict(props)),
        None => dst.update_dict(root, |c| c.set(b"OCProperties".to_vec(), Object::Dict(props)))?,
    }
    Ok(())
}

/// Rename the top-level fields about to be registered in `dst` whose fully-qualified names
/// (`parent.T.child`) are already taken, so each imported field stays its own field instead of
/// blurring into the destination's field of the same name (fields of one fully-qualified name
/// are one field). A colliding node's own `/T` becomes the first free `Name-1`, `Name-2`…
/// variant, so everything below it keeps its place in the tree. Returns what was renamed
/// (old fully-qualified name → new one), in order.
fn resolve_field_conflicts(dst: &mut Document, fields: &[ObjRef]) -> Result<Vec<(String, String)>, OrganizeError> {
    let mut taken = taken_field_names(dst)?;
    let planned = plan_renames(dst, fields, &mut taken);
    let mut renamed = Vec::new();
    for (node, new_t, old, new) in planned {
        dst.update_dict(node, |d| d.set(b"T".to_vec(), Object::String(pdfcraft_cos::PdfString::text(&new_t))))?;
        renamed.push((old, new));
    }
    Ok(renamed)
}

/// What importing `src` into `dst` would rename, computed without changing either document:
/// each fully-qualified name of `src`'s fields that `dst` already uses (or that an earlier
/// field of `src` itself uses) is renamed to the first free variant, exactly as the import
/// would. Unreadable structure previews as nothing: the import itself will say why it failed.
pub fn field_name_conflicts(dst: &Document, src: &Document) -> Vec<(String, String)> {
    let Ok(mut taken) = taken_field_names(dst) else { return Vec::new() };
    let Ok(roots) = field_roots(src) else { return Vec::new() };
    plan_renames(src, &roots, &mut taken).into_iter().map(|(_, _, old, new)| (old, new)).collect()
}

/// What combining `sources`, in order, would rename in each one: the whole list is simulated
/// with the same first-free-variant rule the combine uses, so a later file's preview sees the
/// earlier files' renames. Unreadable structure previews as nothing (the combine itself will
/// say why it fails); non-colliding names still count as taken for the files after them.
pub fn combine_field_conflicts(sources: &[&Document]) -> Vec<Vec<(String, String)>> {
    let mut taken = std::collections::HashSet::new();
    sources
        .iter()
        .map(|src| match field_roots(src) {
            Ok(roots) => plan_renames(src, &roots, &mut taken).into_iter().map(|(_, _, old, new)| (old, new)).collect(),
            Err(_) => Vec::new(),
        })
        .collect()
}

/// Walk field trees `roots` in `doc` and plan the renames that would keep every name separate
/// from `taken`: each node's fully-qualified name joins `taken` (so a later field of the same
/// import collides with an earlier one), and a node already taken is renamed to the first free
/// variant of its own `/T`. Names are compared and renamed as decoded text, not as stored
/// bytes, so one name written in two encodings (PDFDocEncoding, UTF-16BE) is one name.
/// Read-only: returns (node, new `/T`, old name, new name). Cycle- and depth-safe.
fn plan_renames(doc: &Document, roots: &[ObjRef], taken: &mut std::collections::HashSet<String>) -> Vec<(ObjRef, String, String, String)> {
    let mut planned = Vec::new();
    let mut walk = RenameWalk::default();
    for &root in roots {
        plan_renames_under(doc, root, "", taken, &mut walk, 0, &mut planned);
    }
    planned
}

/// What one [`plan_renames`] call remembers: the nodes it visited, and for each colliding name
/// the suffix to try next.
#[derive(Default)]
struct RenameWalk {
    seen: HashSet<ObjRef>,
    next: HashMap<String, u32>,
}

/// One node of [`plan_renames`].
fn plan_renames_under(
    doc: &Document,
    node: ObjRef,
    prefix: &str,
    taken: &mut std::collections::HashSet<String>,
    walk: &mut RenameWalk,
    depth: u8,
    planned: &mut Vec<(ObjRef, String, String, String)>,
) {
    if depth > 32 || !walk.seen.insert(node) {
        return;
    }
    let Some(d) = doc.get(node).as_dict().cloned() else { return };
    let mut name = d.get(b"T").and_then(|t| doc.resolve(t).as_string().map(|s| s.to_text())).unwrap_or_default();
    if !name.is_empty() {
        let fqn = qualified(prefix, &name);
        if taken.contains(&fqn) {
            // `taken` only grows, so every suffix below the last one handed out for this name is
            // still taken: resume there instead of at 1.
            let mut n = walk.next.get(&fqn).copied().unwrap_or(1);
            let mut variant;
            while {
                #[cfg(test)]
                crate::work(1);
                variant = format!("{name}-{n}");
                taken.contains(&qualified(prefix, &variant))
            } {
                n = n.saturating_add(1);
            }
            walk.next.insert(fqn.clone(), n.saturating_add(1));
            let new_fqn = qualified(prefix, &variant);
            planned.push((node, variant.clone(), fqn, new_fqn.clone()));
            taken.insert(new_fqn);
            name = variant;
        } else {
            taken.insert(fqn);
        }
    }
    let prefix = qualified_optional(prefix, &name);
    let kids = d.get(b"Kids").map(|k| doc.resolve(k)).and_then(|k| k.as_array().cloned()).unwrap_or_default();
    for kid in kids.iter().filter_map(|k| k.as_ref()) {
        plan_renames_under(doc, kid, &prefix, taken, walk, depth + 1, planned);
    }
}

/// The field names in use in `dst`: every name in its form, and every name hanging only off a
/// page's widgets — a field the form never lists still owns its name.
fn taken_field_names(dst: &Document) -> Result<std::collections::HashSet<String>, OrganizeError> {
    let mut taken = std::collections::HashSet::new();
    let mut seen = std::collections::HashSet::new();
    for root in field_roots(dst)? {
        collect_names(dst, root, "", &mut seen, 0, &mut taken);
    }
    Ok(taken)
}

/// A document's field trees, wherever they are rooted: the `/AcroForm /Fields` entries, and
/// the fields behind widgets on pages (climbed to their root, for forms that never registered
/// them).
fn field_roots(doc: &Document) -> Result<Vec<ObjRef>, OrganizeError> {
    let mut roots: Vec<ObjRef> = doc
        .root()
        .and_then(|r| doc.get(r).as_dict().cloned())
        .and_then(|c| c.get(b"AcroForm").map(|f| doc.resolve(f)))
        .and_then(|f| f.as_dict().cloned())
        .and_then(|f| f.get(b"Fields").map(|l| doc.resolve(l)))
        .and_then(|l| l.as_array().cloned())
        .unwrap_or_default()
        .iter()
        .filter_map(|f| f.as_ref())
        .collect();
    for (page, _) in walk(doc)? {
        let Some(d) = doc.get(page).as_dict().cloned() else { continue };
        let annots = d.get(b"Annots").map(|a| doc.resolve(a)).and_then(|a| a.as_array().cloned()).unwrap_or_default();
        for a in annots.iter() {
            let Some(w) = doc.resolve(a).as_dict().cloned() else { continue };
            if w.name(b"Subtype") != Some(b"Widget") {
                continue;
            }
            // Walk up to the field the widget belongs to, however deep it sits; a widget that
            // is its own field is its own root.
            let Some(mut root) = a.as_ref() else { continue };
            let mut up = 0;
            while let Some(parent) = doc.get(root).as_dict().and_then(|p| p.reference(b"Parent")) {
                if up >= 32 || root == parent {
                    break;
                }
                root = parent;
                up += 1;
            }
            roots.push(root);
        }
    }
    Ok(roots)
}

/// Record the fully-qualified names of one field tree. Cycle- and depth-safe.
fn collect_names(
    dst: &Document,
    node: ObjRef,
    prefix: &str,
    seen: &mut std::collections::HashSet<ObjRef>,
    depth: u8,
    out: &mut std::collections::HashSet<String>,
) {
    if depth > 32 || !seen.insert(node) {
        return;
    }
    let Some(d) = dst.get(node).as_dict().cloned() else { return };
    let name = d.get(b"T").and_then(|t| dst.resolve(t).as_string().map(|s| s.to_text())).unwrap_or_default();
    let prefix = qualified_optional(prefix, &name);
    if !name.is_empty() {
        out.insert(prefix.clone());
    }
    let kids = d.get(b"Kids").map(|k| dst.resolve(k)).and_then(|k| k.as_array().cloned()).unwrap_or_default();
    for kid in kids.iter().filter_map(|k| k.as_ref()) {
        collect_names(dst, kid, &prefix, seen, depth + 1, out);
    }
}

/// `prefix.name`, or `name` at the top level.
fn qualified(prefix: &str, name: &str) -> String {
    if prefix.is_empty() { name.to_string() } else { format!("{prefix}.{name}") }
}

/// The prefix a node's children hang under: its own name, when it has one.
fn qualified_optional(prefix: &str, name: &str) -> String {
    if name.is_empty() { prefix.to_string() } else { qualified(prefix, name) }
}

/// Add `kid` to `field`'s `/Kids`, in the array object itself when `/Kids` is one, so the field
/// keeps its other widgets.
fn add_kid(dst: &mut Document, field: ObjRef, kid: ObjRef) -> Result<(), OrganizeError> {
    match dst.get(field).as_dict().and_then(|f| f.get(b"Kids").cloned()) {
        Some(Object::Ref(list)) => {
            let mut kids = dst.get(list).as_array().cloned().unwrap_or_default();
            kids.push(Object::Ref(kid));
            dst.set(list, Object::Array(kids));
        }
        _ => dst.update_dict(field, |f| {
            let mut kids = f.get(b"Kids").and_then(|k| k.as_array()).cloned().unwrap_or_default();
            kids.push(Object::Ref(kid));
            f.set(b"Kids".to_vec(), Object::Array(kids));
        })?,
    }
    Ok(())
}

/// PdfCraft patch (#1810): split a widget that doubles as its own field into a parent field
/// holding the field keys (name, value, flags…) and a widget child holding the annotation keys
/// (rectangle, appearance, page). The parent takes the widget's place among the top-level fields;
/// later copies of the page add widget children of it, so every page has its own widget and the
/// field stays one.
fn split_own_field(dst: &mut Document, fields: &mut [ObjRef], widget: ObjRef) {
    let Some(d) = dst.get(widget).as_dict().cloned() else { return };
    let (mut field, mut child) = field_and_widget(dst, &d);
    let parent = dst.add(Object::Null);
    field.set(b"Kids".to_vec(), Object::Array(vec![Object::Ref(widget)]));
    child.set(b"Parent".to_vec(), Object::Ref(parent));
    dst.set(parent, Object::Dict(field));
    dst.set(widget, Object::Dict(child));
    if let Some(entry) = fields.iter_mut().find(|f| **f == widget) {
        *entry = parent;
    }
}

/// A combined field/widget has two additional-action namespaces (ISO 32000-1 tables 194/196).
/// Partition them as well as the ordinary entries, including when copying a later widget.
fn field_and_widget(doc: &Document, d: &Dict) -> (Dict, Dict) {
    let (mut field, mut child) = (Dict::new(), Dict::new());
    for (k, v) in d.iter() {
        if k.as_slice() == b"AA"
            && let Some(actions) = doc.resolve(v).as_dict().cloned()
        {
            let (mut field_actions, mut widget_actions) = (Dict::new(), Dict::new());
            for (trigger, action) in actions.iter() {
                if [b"E".as_slice(), b"X", b"D", b"U", b"Fo", b"Bl", b"PO", b"PC", b"PV", b"PI"].contains(&trigger.as_slice()) {
                    widget_actions.set(trigger.clone(), action.clone());
                } else {
                    field_actions.set(trigger.clone(), action.clone());
                }
            }
            if field_actions.iter().next().is_some() {
                field.set(k.clone(), Object::Dict(field_actions));
            }
            if widget_actions.iter().next().is_some() {
                child.set(k.clone(), Object::Dict(widget_actions));
            }
        } else if WIDGET_KEYS.contains(&k.as_slice()) {
            child.set(k.clone(), v.clone());
        } else {
            field.set(k.clone(), v.clone());
        }
    }
    (field, child)
}

/// Add top-level fields to the destination's `/AcroForm /Fields` (creating the form if needed).
fn register_fields(dst: &mut Document, fields: &[ObjRef]) -> Result<(), OrganizeError> {
    if fields.is_empty() {
        return Ok(());
    }
    let root = dst.root().ok_or(OrganizeError::NoPageTree)?;
    let catalog = dst.get(root).as_dict().cloned().ok_or(OrganizeError::NoPageTree)?;
    let (form_ref, mut form) = match catalog.get(b"AcroForm") {
        Some(Object::Ref(r)) => (Some(*r), dst.get(*r).as_dict().cloned().unwrap_or_default()),
        Some(Object::Dict(d)) => (None, d.clone()),
        _ => (None, Dict::new()),
    };
    let mut list = form.get(b"Fields").map(|f| dst.resolve(f)).and_then(|f| f.as_array().cloned()).unwrap_or_default();
    let mut listed: HashSet<ObjRef> = list.iter().filter_map(Object::as_ref).collect();
    for f in fields {
        #[cfg(test)]
        crate::work(1);
        if listed.insert(*f) {
            list.push(Object::Ref(*f));
        }
    }
    form.set(b"Fields".to_vec(), Object::Array(list));
    match form_ref {
        Some(r) => dst.set(r, Object::Dict(form)),
        None => {
            let r = dst.add(form);
            dst.update_dict(root, |c| c.set(b"AcroForm".to_vec(), Object::Ref(r)))?;
        }
    }
    Ok(())
}

/// A new document containing copies of `pages` from `src` (Extract Pages / Split).
/// Document information (title, author…) is carried over, and so is the print standard the
/// source declares (PDF/X: its output intents and identification, #263), and the bookmarks that
/// go to the copied pages.
pub fn extract_pages(src: &Document, pages: &[usize]) -> Result<Document, OrganizeError> {
    let mut out = Document::new_empty();
    // One source: nothing to deduplicate. Its fields keep their names (a source with two
    // same-named field roots stays one field, as it was), so Extract and Split never rename.
    let imported = import_pages_mapped(&mut out, src, pages, 0, FieldConflicts::Merge)?;
    // The bookmarks of these pages come along, mapped to the copies (#4020).
    let outline = kept_bookmarks(source_outline(src, imported.page_map));
    if !outline.is_empty() {
        let root = crate::outline::container(&mut out, &[])?;
        if let Some((first, last, _)) = write_outline_level(&mut out, root, &outline)? {
            out.update_dict(root, |d| {
                d.set(b"First".to_vec(), Object::Ref(first));
                d.set(b"Last".to_vec(), Object::Ref(last));
            })?;
            let top = crate::outline::children(&out, root);
            crate::outline::relink_and_recount(&mut out, root, &top)?;
        }
    }
    for key in crate::INFO_KEYS {
        if let Some(v) = crate::info(src, key) {
            crate::set_info(&mut out, key, &v)?;
        }
    }
    // After the title: the XMP packet repeats it.
    crate::pdfx::carry(&mut out, Some(src), &crate::pdfx::PrintStandard::of(src), imported.objects)?;
    // The pages share their source's resource dictionary, so the part would otherwise carry every
    // XObject the source lists — images and all (#204). Keep only what these pages draw.
    crate::prune::prune_unused_xobjects(&mut out)?;
    Ok(out)
}

/// How to split a document.
#[derive(Clone, Debug, PartialEq)]
pub enum SplitBy {
    /// Every `n` pages.
    PageCount(usize),
    /// Before each of these 0-based page indices (e.g. `[3, 7]` → 0–2, 3–6, 7–end).
    Before(Vec<usize>),
}

/// The page ranges (0-based, end-exclusive) a split produces.
pub fn split_ranges(page_count: usize, by: &SplitBy) -> Vec<std::ops::Range<usize>> {
    let mut cuts: Vec<usize> = match by {
        SplitBy::PageCount(n) => (1..).map(|i| i * (*n).max(1)).take_while(|c| *c < page_count).collect(),
        SplitBy::Before(v) => v.iter().copied().filter(|c| *c > 0 && *c < page_count).collect(),
    };
    cuts.sort_unstable();
    cuts.dedup();
    let mut ranges = Vec::new();
    let mut start = 0;
    for c in cuts.into_iter().chain(std::iter::once(page_count)) {
        if c > start {
            ranges.push(start..c);
            start = c;
        }
    }
    ranges
}

/// Split a document into several new documents.
pub fn split(src: &Document, by: &SplitBy) -> Result<Vec<Document>, OrganizeError> {
    let n = crate::page_count(src)?;
    split_ranges(n, by).into_iter().map(|r| extract_pages(src, &r.collect::<Vec<_>>())).collect()
}

/// What combining produced, beyond the document: the fields renamed to keep colliding names
/// separate (old fully-qualified name → new one), in order across all sources.
pub struct CombineOutcome {
    /// The combined document.
    pub doc: Document,
    /// Every field renamed while combining.
    pub renamed_fields: Vec<(String, String)>,
}

/// Combine whole documents, in order, into a new document. Each source gets a top-level
/// bookmark (its `title`) pointing to its first page, like Acrobat's Combine Files.
/// The source's own bookmarks are nested (collapsed) under its entry, and its document-level
/// attachments are carried over. When every source declares the same print standard (PDF/X
/// with the same output intent), so does the result.
pub fn combine(sources: &[(&str, &Document)]) -> Result<Document, OrganizeError> {
    combine_with(sources, FieldConflicts::Rename).map(|o| o.doc)
}

/// [`combine`] reporting what the conflict resolution renamed.
pub fn combine_with(sources: &[(&str, &Document)], conflicts: FieldConflicts) -> Result<CombineOutcome, OrganizeError> {
    let all: Vec<(&str, &Document, Option<&[usize]>)> = sources.iter().map(|(t, d)| (*t, *d, None)).collect();
    combine_selected_with(&all, conflicts)
}

/// Combine Files with chosen pages: each source contributes `pages` (0-based, in that order;
/// `None` for all of them). Bookmarks that point at pages left out lose their destination.
pub fn combine_selected(sources: &[(&str, &Document, Option<&[usize]>)]) -> Result<Document, OrganizeError> {
    combine_selected_with(sources, FieldConflicts::Rename).map(|o| o.doc)
}

/// [`combine_selected`] reporting what the conflict resolution renamed.
pub fn combine_selected_with(sources: &[(&str, &Document, Option<&[usize]>)], conflicts: FieldConflicts) -> Result<CombineOutcome, OrganizeError> {
    let mut combiner = Combiner::with_field_conflicts(conflicts);
    for (title, src, chosen) in sources {
        combiner.add(title, src, *chosen)?;
    }
    combiner.finish_with_renames()
}

/// Combine Files one source at a time, with the same result as [`combine_selected`]. [`Combiner::add`]
/// copies a source's pages into the result and keeps only what the rest of the combine needs: its
/// bookmarks (read and mapped to the copied pages), its attachments (copied), and its document only
/// when it declares output intents. So each source can be dropped as soon as it is added, and a long
/// list holds one source at a time rather than all of them.
pub struct Combiner {
    out: Document,
    marks: Vec<Mark>,
    attachments: Vec<(Vec<u8>, Object)>,
    /// The first source's print standard, and what its pages brought along.
    standard: Option<Standard>,
    /// Whether every source so far declares the first source's print standard.
    agreed: bool,
    /// How colliding field names are resolved as each source is added.
    field_conflicts: FieldConflicts,
    /// What adding sources renamed (old → new fully-qualified field name), in order.
    renamed_fields: Vec<(String, String)>,
}

/// The first source's print standard (see [`Combiner`]).
struct Standard {
    declared: crate::pdfx::PrintStandard,
    /// The source objects the copied pages brought along, and their copies.
    objects: HashMap<ObjRef, ObjRef>,
    /// The source itself, kept only for its output intents, which are copied from it.
    source: Option<Document>,
}

impl Default for Combiner {
    fn default() -> Self {
        Self::new()
    }
}

impl Combiner {
    /// A combine with no sources yet, renaming colliding field names as sources are added.
    pub fn new() -> Self {
        Self::with_field_conflicts(FieldConflicts::Rename)
    }

    /// [`Combiner::new`] resolving colliding field names as `conflicts` says.
    pub fn with_field_conflicts(field_conflicts: FieldConflicts) -> Self {
        Self {
            out: Document::new_empty(),
            marks: Vec::new(),
            attachments: Vec::new(),
            standard: None,
            agreed: true,
            field_conflicts,
            renamed_fields: Vec::new(),
        }
    }

    /// What adding sources has renamed so far (old → new fully-qualified field name).
    pub fn renamed_fields(&self) -> &[(String, String)] {
        &self.renamed_fields
    }

    /// Add `src`'s `pages` (0-based, in that order; `None` for all of them) after the pages added so far,
    /// under a bookmark titled `title`. `src` need not outlive the call.
    pub fn add(&mut self, title: &str, src: &Document, pages: Option<&[usize]>) -> Result<(), OrganizeError> {
        let n = crate::page_count(src)?;
        let pages: Vec<usize> = match pages {
            Some(p) => {
                if let Some(bad) = p.iter().find(|i| **i >= n) {
                    return Err(OrganizeError::NoSuchPage(*bad));
                }
                p.to_vec()
            }
            None => (0..n).collect(),
        };
        let at = crate::page_count(&self.out)?;
        let imported = import_pages_mapped(&mut self.out, src, &pages, at, self.field_conflicts)?;
        self.renamed_fields.extend(imported.renamed_fields);
        if let Some(first) = imported.pages.first() {
            let outline = source_outline(src, imported.page_map);
            self.marks.push(Mark { title: title.to_string(), page: *first, outline });
        }
        collect_attachments(&mut self.out, src, &mut self.attachments)?;
        let declared = crate::pdfx::PrintStandard::of(src);
        match self.standard.as_ref().map(|first| first.declared.same_as(&declared)) {
            Some(same) => self.agreed &= same,
            None => {
                let source = declared.declares_output_intents().then(|| src.clone());
                self.standard = Some(Standard { declared, objects: imported.objects, source });
            }
        }
        Ok(())
    }

    /// The combined document: the pages as added, the print standard of the first source when every
    /// source declares the same one, shared resources stored once, then the bookmarks and attachments.
    pub fn finish(self) -> Result<Document, OrganizeError> {
        self.finish_with_renames().map(|o| o.doc)
    }

    /// [`Combiner::finish`] reporting what the combine renamed.
    pub fn finish_with_renames(self) -> Result<CombineOutcome, OrganizeError> {
        let Combiner { mut out, marks, attachments, standard, agreed, renamed_fields, .. } = self;
        if let Some(first) = standard.filter(|_| agreed) {
            crate::pdfx::carry(&mut out, first.source.as_ref(), &first.declared, first.objects)?;
        }
        // Sources often share fonts, images and profiles (or are the same file): store them once.
        let all: Vec<ObjRef> = out.object_numbers().into_iter().map(|n| ObjRef::new(n, out.generation(n))).collect();
        crate::dedupe::dedupe_resources(&mut out, &all, false);
        add_outline(&mut out, &marks)?;
        set_attachments(&mut out, attachments)?;
        Ok(CombineOutcome { doc: out, renamed_fields })
    }
}

/// One run of pages for [`combine_grouped`]: the file it comes from (`group`: runs with the same
/// group are one file split around, and give the same document), its title, the document, and
/// its pages (0-based, in that order; `None` for all of them).
pub type Run<'a> = (usize, &'a str, &'a Document, Option<&'a [usize]>);

/// Combine Files where a file's pages may be split into several runs with other files' pages
/// between them. Each file (group) is copied once, all its pages together, then the pages are put
/// in the runs' order: its links between its own pages, its form fields and its attachments stay
/// whole, and it gets one top-level bookmark (the title of its first run) at its first page, its
/// own bookmarks nested. Otherwise as [`combine_selected`] (print standard, shared resources).
pub fn combine_grouped(runs: &[Run<'_>]) -> Result<Document, OrganizeError> {
    combine_grouped_with(runs, FieldConflicts::Rename).map(|o| o.doc)
}

/// [`combine_grouped`] reporting what the conflict resolution renamed.
pub fn combine_grouped_with(runs: &[Run<'_>], conflicts: FieldConflicts) -> Result<CombineOutcome, OrganizeError> {
    // Each file's pages, in the order its runs list them, and each run's place in that list.
    struct File<'a> {
        title: &'a str,
        src: &'a Document,
        pages: Vec<usize>,
    }
    let mut files: Vec<(usize, File<'_>)> = Vec::new();
    let mut slices: Vec<(usize, usize, usize)> = Vec::with_capacity(runs.len());
    for (group, title, src, chosen) in runs {
        let at = match files.iter().position(|(g, _)| g == group) {
            Some(at) => at,
            None => {
                files.push((*group, File { title, src, pages: Vec::new() }));
                files.len() - 1
            }
        };
        let Some((_, file)) = files.get_mut(at) else { continue };
        let n = crate::page_count(file.src)?;
        let pages: Vec<usize> = match chosen {
            Some(p) => {
                if let Some(bad) = p.iter().find(|i| **i >= n) {
                    return Err(OrganizeError::NoSuchPage(*bad));
                }
                p.to_vec()
            }
            None => (0..n).collect(),
        };
        slices.push((at, file.pages.len(), pages.len()));
        file.pages.extend(pages);
    }
    let mut out = Document::new_empty();
    let mut marks = Vec::new();
    let mut attachments = Vec::new();
    let mut copies: Vec<Vec<ObjRef>> = Vec::with_capacity(files.len());
    let mut renamed_fields = Vec::new();
    let mut standard: Option<(crate::pdfx::PrintStandard, &Document, HashMap<ObjRef, ObjRef>)> = None;
    let mut agreed = true;
    for (_, file) in &files {
        let at = crate::page_count(&out)?;
        let imported = import_pages_mapped(&mut out, file.src, &file.pages, at, conflicts)?;
        copies.push(imported.pages.clone());
        renamed_fields.extend(imported.renamed_fields);
        if let Some(first) = imported.pages.first() {
            marks.push(Mark { title: file.title.to_string(), page: *first, outline: source_outline(file.src, imported.page_map) });
        }
        collect_attachments(&mut out, file.src, &mut attachments)?;
        let declared = crate::pdfx::PrintStandard::of(file.src);
        match &standard {
            None => standard = Some((declared, file.src, imported.objects)),
            Some((first, _, _)) => agreed &= first.same_as(&declared),
        }
    }
    // The pages in the runs' order. A file's first run comes before its others, and its pages
    // were copied in run order, so its bookmark (its first copied page) is its first page shown.
    let mut order: Vec<(ObjRef, Dict)> = Vec::new();
    for (file, start, len) in slices {
        let run = copies.get(file).and_then(|c| c.get(start..start.saturating_add(len))).unwrap_or_default();
        order.extend(run.iter().map(|r| (*r, Dict::new())));
    }
    rebuild(&mut out, &order)?;
    if let Some((declared, src, objects)) = standard.filter(|_| agreed) {
        crate::pdfx::carry(&mut out, Some(src), &declared, objects)?;
    }
    let all: Vec<ObjRef> = out.object_numbers().into_iter().map(|n| ObjRef::new(n, out.generation(n))).collect();
    crate::dedupe::dedupe_resources(&mut out, &all, false);
    add_outline(&mut out, &marks)?;
    set_attachments(&mut out, attachments)?;
    Ok(CombineOutcome { doc: out, renamed_fields })
}

/// Copy a source's document-level attachments (`/Names /EmbeddedFiles`) into `dst`.
fn collect_attachments(dst: &mut Document, src: &Document, out: &mut Vec<(Vec<u8>, Object)>) -> Result<(), OrganizeError> {
    let Some(tree) = src
        .root()
        .and_then(|r| src.get(r).as_dict().cloned())
        .and_then(|c| c.get(b"Names").map(|n| src.resolve(n)))
        .and_then(|n| n.as_dict().and_then(|d| d.get(b"EmbeddedFiles").cloned()))
    else {
        return Ok(());
    };
    let mut entries = Vec::new();
    name_tree_entries(src, &tree, 0, &mut HashSet::new(), &mut entries);
    let mut copier = Copier::new(src, HashMap::new(), HashMap::new());
    for (k, v) in entries {
        let v = copier.copy_value(dst, &v, None)?;
        out.push((k, v));
    }
    Ok(())
}

/// Write attachments as a flat, sorted `/EmbeddedFiles` name tree; duplicate names get " (2)"…
pub(crate) fn set_attachments(dst: &mut Document, mut entries: Vec<(Vec<u8>, Object)>) -> Result<(), OrganizeError> {
    if entries.is_empty() {
        return Ok(());
    }
    // Names are compared and numbered as text: a key written as UTF-16 (a name with an accent
    // or Chinese letters) must not get ASCII bytes appended to it.
    let mut seen = std::collections::HashSet::new();
    for (k, _) in entries.iter_mut() {
        let base = pdfcraft_cos::PdfString::literal(k.clone()).to_text();
        let mut text = base.clone();
        let mut n = 2;
        while !seen.insert(text.clone()) {
            text = format!("{base} ({n})");
            n += 1;
        }
        if text != base {
            *k = pdfcraft_cos::PdfString::text(&text).bytes;
        }
    }
    entries.sort_by(|a, b| a.0.cmp(&b.0));
    let mut names = Vec::new();
    for (k, v) in entries {
        names.push(Object::String(pdfcraft_cos::PdfString::literal(k)));
        names.push(v);
    }
    let mut tree = Dict::new();
    tree.set(b"Names".to_vec(), Object::Array(names));
    let tree = dst.add(tree);
    let root = dst.root().ok_or(OrganizeError::NoPageTree)?;
    // `/Names` may be an indirect dictionary (Acrobat writes it so): update it where it is, so
    // its other trees (/Dests, /JavaScript…) stay.
    if let Some(Object::Ref(names)) = dst.get(root).as_dict().and_then(|c| c.get(b"Names").cloned())
        && dst.get(names).as_dict().is_some()
    {
        dst.update_dict(names, |n| n.set(b"EmbeddedFiles".to_vec(), Object::Ref(tree)))?;
        return Ok(());
    }
    dst.update_dict(root, |c| {
        let mut n = match c.get(b"Names") {
            Some(Object::Dict(d)) => d.clone(),
            _ => Dict::new(),
        };
        n.set(b"EmbeddedFiles".to_vec(), Object::Ref(tree));
        c.set(b"Names".to_vec(), Object::Dict(n));
    })?;
    Ok(())
}

/// A source's entry in the combined outline (see [`add_outline`]): its title, its first page in the
/// result, and its own bookmarks, read from the source when it was added.
struct Mark {
    title: String,
    page: ObjRef,
    outline: Vec<OutlineNode>,
}

/// One of a source's bookmarks, read and ready to write: its title, colour and flags, its destination
/// in the result (`None` when the page it points at was not copied), whether it is open, and its own
/// bookmarks.
struct OutlineNode {
    title: Option<Object>,
    colour: Option<Object>,
    flags: Option<Object>,
    dest: Option<Object>,
    open: bool,
    children: Vec<OutlineNode>,
}

/// The bookmarks of `src`, with destinations mapped through `pages` (source page to copied page). They
/// are read while `src` is at hand, so the source itself need not be kept.
fn source_outline(src: &Document, pages: HashMap<ObjRef, ObjRef>) -> Vec<OutlineNode> {
    let first = src
        .root()
        .and_then(|r| src.get(r).as_dict().cloned())
        .and_then(|c| c.get(b"Outlines").map(|o| src.resolve(o)))
        .and_then(|o| o.as_dict().and_then(|d| d.reference(b"First")));
    let mapper = Copier::new(src, HashMap::new(), pages);
    let mut budget = 10_000usize;
    outline_level(&mapper, first, 0, &mut budget)
}

/// `nodes` without the bookmarks whose page was not copied (no destination left), unless a bookmark
/// under them stays. [`outline_level`] already bounds the depth.
fn kept_bookmarks(nodes: Vec<OutlineNode>) -> Vec<OutlineNode> {
    nodes
        .into_iter()
        .filter_map(|mut n| {
            n.children = kept_bookmarks(n.children);
            (n.dest.is_some() || !n.children.is_empty()).then_some(n)
        })
        .collect()
}

/// One level of bookmarks from `first` (its siblings), each with its children. Cycle- and size-safe:
/// at most `budget` bookmarks are read in all, and nesting stops at 32 levels.
fn outline_level(m: &Copier<'_>, first: Option<ObjRef>, depth: u8, budget: &mut usize) -> Vec<OutlineNode> {
    if depth > 32 {
        return Vec::new();
    }
    let mut seen = std::collections::HashSet::new();
    let mut made = Vec::new();
    let mut cur = first;
    while let Some(r) = cur {
        if !seen.insert(r) || *budget == 0 {
            break;
        }
        *budget -= 1;
        let Some(item) = m.src.get(r).as_dict().cloned() else { break };
        cur = item.reference(b"Next");
        let title = item.get(b"Title").map(|t| m.src.resolve(t).as_ref().clone());
        let colour = item.get(b"C").map(|v| m.src.resolve(v).as_ref().clone());
        let flags = item.get(b"F").map(|v| m.src.resolve(v).as_ref().clone());
        let target = item.get(b"Dest").cloned().or_else(|| {
            let a = m.src.resolve(item.get(b"A")?);
            let a = a.as_dict()?;
            (a.name(b"S") == Some(b"GoTo")).then(|| a.get(b"D").cloned()).flatten()
        });
        let dest = target.and_then(|t| m.map_dest(&t));
        let open = item.int(b"Count").is_some_and(|c| c > 0);
        let children = outline_level(m, item.reference(b"First"), depth + 1, budget);
        made.push(OutlineNode { title, colour, flags, dest, open, children });
    }
    made
}

/// Write `nodes` as one level under `parent`: each bookmark is created, then its children, depth first
/// and in order, so the object numbers come out as they always did. Returns the level's first and last
/// item and how many items it has (`None` when it is empty).
fn write_outline_level(dst: &mut Document, parent: ObjRef, nodes: &[OutlineNode]) -> Result<Option<(ObjRef, ObjRef, usize)>, OrganizeError> {
    let mut made: Vec<ObjRef> = Vec::new();
    for node in nodes {
        let new = dst.add(Object::Null);
        let mut d = Dict::new();
        if let Some(t) = &node.title {
            d.set(b"Title".to_vec(), t.clone());
        }
        if let Some(c) = &node.colour {
            d.set(b"C".to_vec(), c.clone());
        }
        if let Some(f) = &node.flags {
            d.set(b"F".to_vec(), f.clone());
        }
        if let Some(dest) = &node.dest {
            d.set(b"Dest".to_vec(), dest.clone());
        }
        d.set(b"Parent".to_vec(), Object::Ref(parent));
        if let Some(prev) = made.last() {
            d.set(b"Prev".to_vec(), Object::Ref(*prev));
            dst.update_dict(*prev, |p| p.set(b"Next".to_vec(), Object::Ref(new)))?;
        }
        if let Some((f, l, n)) = write_outline_level(dst, new, &node.children)? {
            d.set(b"First".to_vec(), Object::Ref(f));
            d.set(b"Last".to_vec(), Object::Ref(l));
            // Keep the source's open/closed state.
            d.set(b"Count".to_vec(), Object::Int(if node.open { n as i64 } else { -(n as i64) }));
        }
        dst.set(new, Object::Dict(d));
        made.push(new);
    }
    Ok(match (made.first(), made.last()) {
        (Some(first), Some(last)) => Some((*first, *last, made.len())),
        _ => None,
    })
}

/// Replace the outline with one bookmark per source (title → its first page), each holding a collapsed
/// copy of that source's own bookmarks with destinations mapped to the copied pages.
fn add_outline(doc: &mut Document, marks: &[Mark]) -> Result<(), OrganizeError> {
    if marks.is_empty() {
        return Ok(());
    }
    let outlines = doc.add(Object::Null);
    let items: Vec<ObjRef> = marks.iter().map(|_| doc.add(Object::Null)).collect();
    let (Some(&first_item), Some(&last_item)) = (items.first(), items.last()) else { return Ok(()) };
    for (i, (mark, r)) in marks.iter().zip(&items).enumerate() {
        let mut d = Dict::new();
        d.set(b"Title".to_vec(), Object::String(pdfcraft_cos::PdfString::text(&mark.title)));
        d.set(b"Parent".to_vec(), Object::Ref(outlines));
        d.set(b"Dest".to_vec(), Object::Array(vec![Object::Ref(mark.page), Object::name("Fit")]));
        if i > 0 {
            d.set(b"Prev".to_vec(), Object::Ref(items[i - 1]));
        }
        if let Some(next) = items.get(i + 1) {
            d.set(b"Next".to_vec(), Object::Ref(*next));
        }
        if let Some((f, l, n)) = write_outline_level(doc, *r, &mark.outline)? {
            d.set(b"First".to_vec(), Object::Ref(f));
            d.set(b"Last".to_vec(), Object::Ref(l));
            d.set(b"Count".to_vec(), Object::Int(-(n as i64))); // collapsed
        }
        doc.set(*r, Object::Dict(d));
    }
    let mut o = Dict::new();
    o.set(b"Type".to_vec(), Object::name("Outlines"));
    o.set(b"First".to_vec(), Object::Ref(first_item));
    o.set(b"Last".to_vec(), Object::Ref(last_item));
    o.set(b"Count".to_vec(), Object::Int(items.len() as i64));
    doc.set(outlines, Object::Dict(o));
    let root = doc.root().ok_or(OrganizeError::NoPageTree)?;
    doc.update_dict(root, |c| {
        c.set(b"Outlines".to_vec(), Object::Ref(outlines));
        c.set(b"PageMode".to_vec(), Object::name("UseOutlines"));
    })?;
    Ok(())
}

/// Page `page` of `src` as a form XObject in `dst` (its content and resources, no annotations),
/// for backgrounds and watermarks taken from a PDF. The form's `/BBox` is the page's crop box
/// and its `/Matrix` undoes the page rotation, so it draws upright as displayed. Returns the
/// form and its displayed size in points.
pub fn page_as_form(dst: &mut Document, src: &Document, page: usize) -> Result<(ObjRef, (f64, f64)), OrganizeError> {
    use pdfcraft_cos::Stream;
    let all = walk(src)?;
    // `walk` gives the inheritable attributes (resources, boxes, rotation); the content is
    // the page's own.
    let (pr, d) = all.get(page).cloned().ok_or(OrganizeError::NoSuchPage(page))?;
    let own = src.get(pr).as_dict().cloned().unwrap_or_default();
    let rect = |k: &[u8]| -> Option<[f64; 4]> {
        let a = src.resolve(d.get(k)?);
        let v: Vec<f64> = a.as_array()?.iter().filter_map(|x| src.resolve(x).as_f64()).collect();
        (v.len() == 4).then(|| [v[0].min(v[2]), v[1].min(v[3]), v[0].max(v[2]), v[1].max(v[3])])
    };
    let bbox = rect(b"CropBox").or_else(|| rect(b"MediaBox")).unwrap_or([0.0, 0.0, 612.0, 792.0]);
    let (w, h) = (bbox[2] - bbox[0], bbox[3] - bbox[1]);
    let rotate = d.get(b"Rotate").and_then(|r| src.resolve(r).as_int()).unwrap_or(0).rem_euclid(360);
    // Content: the page's streams, decoded and joined.
    let content = match own.get(b"Contents") {
        Some(c) => crate::prune::content_bytes(src, c).map_err(|e| OrganizeError::Invalid(format!("page {} content: {e}", page + 1)))?,
        None => Vec::new(),
    };
    let mut copier = Copier::new(src, HashMap::new(), HashMap::new());
    let resources = match d.get(b"Resources") {
        Some(r) => copier.copy_value(dst, r, None)?,
        None => Object::Dict(Dict::new()),
    };
    register_layers(dst, src, &copier.ocgs)?;
    let mut fd = Dict::new();
    fd.set(b"Type".to_vec(), Object::name("XObject"));
    fd.set(b"Subtype".to_vec(), Object::name("Form"));
    fd.set(b"BBox".to_vec(), Object::Array(bbox.iter().map(|v| Object::Real(*v)).collect()));
    // Map the crop box to (0, 0)–(w, h) as displayed (turning a rotated page upright).
    let m: [f64; 6] = match rotate {
        90 => [0.0, -1.0, 1.0, 0.0, -bbox[1], bbox[2]],
        180 => [-1.0, 0.0, 0.0, -1.0, bbox[2], bbox[3]],
        270 => [0.0, 1.0, -1.0, 0.0, bbox[3], -bbox[0]],
        _ => [1.0, 0.0, 0.0, 1.0, -bbox[0], -bbox[1]],
    };
    fd.set(b"Matrix".to_vec(), Object::Array(m.iter().map(|v| Object::Real(*v)).collect()));
    fd.set(b"Resources".to_vec(), resources);
    let r = dst.add(Object::Stream(Stream::flate(fd, &content)));
    let size = if rotate == 90 || rotate == 270 { (h, w) } else { (w, h) };
    Ok((r, size))
}
