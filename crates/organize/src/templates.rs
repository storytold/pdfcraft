//! Page templates (#1429): the name dictionary's `/Templates` tree names pages kept out of the
//! page tree, and its `/Pages` tree names visible pages (ISO 32000-2 §7.7.4, Table 32). Forms
//! show more pages from them through JavaScript: `Template.spawn` adds a copy of a template's
//! page, and `Template.hidden = false` moves the template's own page into the document.

use std::collections::HashSet;

use pdfcraft_cos::{Dict, Document, ObjRef, Object, PdfString};

use crate::{INHERITABLE, OrganizeError, page_count, rebuild, walk};

/// At most this many names are read from one tree: a form has a handful of templates.
const MAX_NAMES: usize = 4096;

/// Field levels followed up from a widget to its top-level field.
const MAX_FIELD_DEPTH: usize = 32;

/// A named page a form can add or show.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Template {
    pub name: String,
    /// Kept out of the page tree (`/Templates`); a visible named page (`/Pages`) is not hidden.
    pub hidden: bool,
}

/// The document's templates: the hidden ones (`/Templates`), then the named visible pages
/// (`/Pages`), each in its tree's order.
pub fn templates(doc: &Document) -> Vec<Template> {
    let named = |tree: &[u8], hidden: bool| {
        entries(doc, tree).into_iter().filter(|(_, v)| v.as_ref().is_some()).map(move |(k, _)| Template { name: text(&k), hidden })
    };
    named(b"Templates", true).chain(named(b"Pages", false)).collect()
}

/// Add a copy of template `name`'s page as page `at` (0-based, clamped to the end), as
/// JavaScript's `Template.spawn(at, rename, false)` does. With `rename`, the copy's fields are
/// put under `P<at>.<name>`, so they are fields of their own; without it, they are more widgets
/// of the template's fields and share their values. Returns where the page went.
pub fn spawn_template(doc: &mut Document, name: &str, at: usize, rename: bool) -> Result<usize, OrganizeError> {
    let page = template_page(doc, name)?;
    let at = at.min(page_count(doc)?);
    // The copy is made from a version of the document whose page tree ends with the template's
    // page, so the page importer copies it as any page: its content, resources and widgets,
    // with their fields registered in the form.
    let mut src = doc.clone();
    let mut order = walk(&src)?;
    let index = match order.iter().position(|(r, _)| *r == page) {
        Some(index) => index,
        None => {
            src.update_dict(page, |d| d.set(b"Type".to_vec(), Object::name("Page")))?;
            order.push((page, fallback_attributes(&order)));
            rebuild(&mut src, &order)?;
            order.len().saturating_sub(1)
        }
    };
    if rename {
        rename_fields(&mut src, page, &format!("P{at}"), name)?;
    }
    let before = crate::import::listed_fields(doc);
    // Renamed copies are fields of their own: a second copy spawned at the same page, whose
    // `P<at>` is taken, gets the first free `P<at>-1`… rather than a name the form already has.
    let conflicts = if rename { crate::import::FieldConflicts::Rename } else { crate::import::FieldConflicts::Merge };
    let copied = crate::import::import_pages_with(doc, &src, &[index], at, conflicts)?.pages;
    if !rename && let Some(&copy) = copied.first() {
        crate::import::share_fields_with_originals(doc, copy, &before)?;
    }
    Ok(at)
}

/// Show (`hidden` false) or hide template `name`, as JavaScript's `Template.hidden` does. A
/// shown template's page goes at the end of the document and is named in `/Pages`; a hidden
/// one leaves the page tree, page and all, and is named in `/Templates` again. Showing a
/// visible page, or hiding a hidden one, changes nothing.
pub fn set_template_hidden(doc: &mut Document, name: &str, hidden: bool) -> Result<(), OrganizeError> {
    let (from, to): (&[u8], &[u8]) = if hidden { (b"Pages", b"Templates") } else { (b"Templates", b"Pages") };
    // Rewritten from every entry, not the listing's first [`MAX_NAMES`], so no name is lost.
    let mut source = all_entries(doc, from);
    let Some(k) = source.iter().position(|(key, _)| text(key) == name) else {
        return if all_entries(doc, to).iter().any(|(key, _)| text(key) == name) {
            Ok(())
        } else {
            Err(OrganizeError::Invalid(format!("the document has no template named {name}")))
        };
    };
    let (key, value) = source.remove(k);
    let page = value.as_ref().ok_or_else(|| OrganizeError::Invalid(format!("template {name} is not a page")))?;
    let mut order = walk(doc)?;
    if hidden {
        let Some(at) = order.iter().position(|(r, _)| *r == page) else {
            return Err(OrganizeError::Invalid(format!("template {name} is not a page of the document")));
        };
        if order.len() == 1 {
            return Err(OrganizeError::WouldRemoveAllPages);
        }
        // Out of the tree, the page no longer inherits: it keeps what it showed with.
        let (_, inherited) = order.remove(at);
        doc.update_dict(page, |d| {
            for k in INHERITABLE {
                if !d.contains(k)
                    && let Some(v) = inherited.get(k)
                {
                    d.set(k.to_vec(), v.clone());
                }
            }
            d.remove(b"Parent");
            d.set(b"Type".to_vec(), Object::name("Template"));
        })?;
    } else {
        if order.iter().any(|(r, _)| *r == page) {
            return Err(OrganizeError::Invalid(format!("template {name} is already a page of the document")));
        }
        doc.update_dict(page, |d| d.set(b"Type".to_vec(), Object::name("Page")))?;
        let attributes = fallback_attributes(&order);
        order.push((page, attributes));
    }
    rebuild(doc, &order)?;
    let mut target = all_entries(doc, to);
    target.push((key, value));
    write_tree(doc, from, source)?;
    write_tree(doc, to, target)
}

/// The page template `name` names, hidden or visible.
fn template_page(doc: &Document, name: &str) -> Result<ObjRef, OrganizeError> {
    [&b"Templates"[..], b"Pages"]
        .iter()
        .flat_map(|tree| entries(doc, tree))
        .find(|(key, _)| text(key) == name)
        .and_then(|(_, v)| v.as_ref())
        .filter(|r| doc.get(*r).as_dict().is_some())
        .ok_or_else(|| OrganizeError::Invalid(format!("the document has no template named {name}")))
}

/// What a page added to the tree inherits when it has no attributes of its own: those of the
/// first page, so a template written without a media box still has one.
fn fallback_attributes(order: &[(ObjRef, Dict)]) -> Dict {
    order.first().map(|(_, inherited)| inherited.clone()).unwrap_or_default()
}

/// Put the fields of the widgets on `page` under a new field `outer`, holding one named
/// `inner`: JavaScript's renaming of a spawned page's fields (`P1.Name.Field`).
fn rename_fields(doc: &mut Document, page: ObjRef, outer: &str, inner: &str) -> Result<(), OrganizeError> {
    let annots = doc.get(page).as_dict().and_then(|d| d.get(b"Annots").cloned()).map(|a| doc.resolve(&a).as_ref().clone());
    let mut tops: Vec<ObjRef> = Vec::new();
    for annot in annots.as_ref().and_then(Object::as_array).into_iter().flatten() {
        let Some(widget) = annot.as_ref() else { continue };
        if doc.get(widget).as_dict().is_none_or(|d| d.name(b"Subtype") != Some(b"Widget")) {
            continue;
        }
        if let Some(top) = top_field(doc, widget)
            && !tops.contains(&top)
        {
            tops.push(top);
        }
    }
    if tops.is_empty() {
        return Ok(());
    }
    let named = |t: &str| {
        let mut d = Dict::new();
        d.set(b"T".to_vec(), Object::String(PdfString::text(t)));
        d
    };
    let mut middle = named(inner);
    middle.set(b"Kids".to_vec(), Object::Array(tops.iter().map(|r| Object::Ref(*r)).collect()));
    let middle = doc.add(middle);
    let mut top = named(outer);
    top.set(b"Kids".to_vec(), Object::Array(vec![Object::Ref(middle)]));
    let top = doc.add(top);
    doc.update_dict(middle, |d| d.set(b"Parent".to_vec(), Object::Ref(top)))?;
    for field in tops {
        doc.update_dict(field, |d| d.set(b"Parent".to_vec(), Object::Ref(middle)))?;
    }
    Ok(())
}

/// The top-level field `widget` belongs to (itself, when it is its own field), following
/// `/Parent` at most [`MAX_FIELD_DEPTH`] levels and never through a node twice.
fn top_field(doc: &Document, widget: ObjRef) -> Option<ObjRef> {
    let mut node = widget;
    let mut seen = HashSet::new();
    for _ in 0..MAX_FIELD_DEPTH {
        if !seen.insert(node) {
            return None;
        }
        match doc.get(node).as_dict()?.reference(b"Parent") {
            Some(parent) => node = parent,
            None => return Some(node),
        }
    }
    None
}

/// The (key, value) pairs of the name dictionary's tree `key`, at most [`MAX_NAMES`] (for listing).
fn entries(doc: &Document, key: &[u8]) -> Vec<(Vec<u8>, Object)> {
    let mut out = all_entries(doc, key);
    out.truncate(MAX_NAMES);
    out
}

/// Every (key, value) pair of the name dictionary's tree `key`, for rewriting it. The walk is
/// bounded by the tree reader's depth limit and seen-set; the entries are already in memory.
fn all_entries(doc: &Document, key: &[u8]) -> Vec<(Vec<u8>, Object)> {
    let Some(tree) = names(doc).and_then(|n| n.get(key).cloned()) else { return Vec::new() };
    let mut out = Vec::new();
    crate::import::name_tree_entries(doc, &tree, 0, &mut HashSet::new(), &mut out);
    out
}

/// The catalog's name dictionary.
fn names(doc: &Document) -> Option<Dict> {
    let catalog = doc.get(doc.root()?).as_dict()?.clone();
    doc.resolve(catalog.get(b"Names")?).as_dict().cloned()
}

/// A tree key as text (keys are PDF text strings).
fn text(key: &[u8]) -> String {
    PdfString::literal(key.to_vec()).to_text()
}

/// Write the name dictionary's tree `key` as one sorted leaf holding `entries`, or leave it out
/// when there are none. The dictionary's other trees stay where they are.
fn write_tree(doc: &mut Document, key: &[u8], mut entries: Vec<(Vec<u8>, Object)>) -> Result<(), OrganizeError> {
    entries.sort_by(|a, b| a.0.cmp(&b.0));
    let tree = (!entries.is_empty()).then(|| {
        let mut names = Vec::with_capacity(entries.len().saturating_mul(2));
        for (k, v) in entries {
            names.push(Object::String(PdfString::literal(k)));
            names.push(v);
        }
        let mut leaf = Dict::new();
        leaf.set(b"Names".to_vec(), Object::Array(names));
        doc.add(leaf)
    });
    let set = |n: &mut Dict| match tree {
        Some(t) => n.set(key.to_vec(), Object::Ref(t)),
        None => {
            n.remove(key);
        }
    };
    let root = doc.root().ok_or(OrganizeError::NoPageTree)?;
    // `/Names` may be an indirect dictionary: update it where it is.
    if let Some(Object::Ref(names)) = doc.get(root).as_dict().and_then(|c| c.get(b"Names").cloned())
        && doc.get(names).as_dict().is_some()
    {
        doc.update_dict(names, set)?;
        return Ok(());
    }
    doc.update_dict(root, |c| {
        let mut n = match c.get(b"Names") {
            Some(Object::Dict(d)) => d.clone(),
            _ => Dict::new(),
        };
        set(&mut n);
        c.set(b"Names".to_vec(), Object::Dict(n));
    })?;
    Ok(())
}

#[cfg(test)]
mod tests;
