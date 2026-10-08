//! Editing bookmarks (the document outline, ISO 32000-2 §12.3.3), execution plan M4.6.
//!
//! Bookmarks are addressed by their **path**: child indices from the top level, so `[0, 2]` is
//! the third child of the first top-level bookmark. Paths follow `/First`–`/Next` order, which
//! is also the order viewers (and our inspector) list them in.
//!
//! Edits are surgical: they relink only the affected siblings (`/Parent`, `/Prev`, `/Next`,
//! `/First`, `/Last`) and recompute `/Count` values, and an object is rewritten only when one of
//! its values changes. Everything else on an item (colour, style, actions, unknown keys) is kept.
//! Walks are cycle- and size-safe; a broken outline is never followed forever.

use std::collections::HashSet;

use pdfcraft_cos::{Dict, Document, ObjRef, Object, PdfString};

use crate::{OrganizeError, walk};

/// Hard cap on items visited (broken or hostile outlines).
const MAX_ITEMS: usize = 100_000;

/// A bookmark as stored in the file.
#[derive(Clone, Debug, PartialEq)]
pub struct Bookmark {
    pub obj: ObjRef,
    pub title: String,
    /// Shown expanded (a positive `/Count`) when it has children.
    pub open: bool,
    pub children: Vec<Bookmark>,
}

#[derive(Debug, thiserror::Error, PartialEq)]
pub enum OutlineError {
    #[error("there is no bookmark at {0:?}")]
    NoSuchBookmark(Vec<usize>),
    #[error("a bookmark cannot be moved inside itself")]
    IntoItself,
    #[error("the title must not be empty")]
    EmptyTitle,
    #[error("{0}")]
    Organize(#[from] OrganizeError),
    #[error("{0}")]
    Cos(#[from] pdfcraft_cos::CosError),
}

type Result<T> = std::result::Result<T, OutlineError>;

fn outline_root(doc: &Document) -> Option<ObjRef> {
    let root = doc.root()?;
    doc.get(root).as_dict()?.reference(b"Outlines")
}

/// The children of `parent` (an item or the outline root), in order. Cycle-safe.
fn children_of(doc: &Document, parent: ObjRef, seen: &mut HashSet<ObjRef>) -> Vec<ObjRef> {
    let mut out = Vec::new();
    let mut next = doc.get(parent).as_dict().and_then(|d| d.reference(b"First"));
    while let Some(r) = next {
        if seen.len() >= MAX_ITEMS || !seen.insert(r) {
            break;
        }
        let item = doc.get(r);
        let Some(d) = item.as_dict() else { break };
        out.push(r);
        next = d.reference(b"Next");
    }
    out
}

/// Every bookmark item, parents before their children. Cycle-safe.
pub(crate) fn items(doc: &Document) -> Vec<ObjRef> {
    let Some(root) = outline_root(doc) else { return Vec::new() };
    let (mut seen, mut out, mut stack) = (HashSet::new(), Vec::new(), vec![root]);
    while let Some(parent) = stack.pop() {
        let kids = children_of(doc, parent, &mut seen);
        stack.extend(kids.iter().copied());
        out.extend(kids);
    }
    out
}

/// The bookmark tree.
pub fn bookmarks(doc: &Document) -> Vec<Bookmark> {
    let Some(root) = outline_root(doc) else { return Vec::new() };
    let mut seen = HashSet::from([root]);
    fn build(doc: &Document, parent: ObjRef, seen: &mut HashSet<ObjRef>) -> Vec<Bookmark> {
        children_of(doc, parent, seen)
            .into_iter()
            .map(|r| {
                let item = doc.get(r);
                let d = item.as_dict().cloned().unwrap_or_default();
                let title = d.get(b"Title").map(|t| doc.resolve(t)).and_then(|t| t.as_string().map(|s| s.to_text())).unwrap_or_default();
                let open = d.int(b"Count").is_some_and(|c| c > 0);
                Bookmark { obj: r, title, open, children: build(doc, r, seen) }
            })
            .collect()
    }
    build(doc, root, &mut seen)
}

/// The object of the bookmark at `path`, and its parent object (the outline root for top-level).
fn resolve_path(doc: &Document, path: &[usize]) -> Result<(ObjRef, ObjRef)> {
    let root = outline_root(doc).ok_or_else(|| OutlineError::NoSuchBookmark(path.to_vec()))?;
    if path.is_empty() {
        return Err(OutlineError::NoSuchBookmark(Vec::new()));
    }
    let mut parent = root;
    let mut seen = HashSet::from([root]);
    for (depth, i) in path.iter().enumerate() {
        let kids = children_of(doc, parent, &mut seen);
        let r = *kids.get(*i).ok_or_else(|| OutlineError::NoSuchBookmark(path.to_vec()))?;
        if depth + 1 == path.len() {
            return Ok((r, parent));
        }
        parent = r;
    }
    // The loop returns on the last index.
    Err(OutlineError::NoSuchBookmark(path.to_vec()))
}

/// The object that holds children at `parent_path` (`[]` = the outline root, created if needed).
fn container(doc: &mut Document, parent_path: &[usize]) -> Result<ObjRef> {
    if parent_path.is_empty() {
        if let Some(r) = outline_root(doc) {
            return Ok(r);
        }
        let mut o = Dict::new();
        o.set(b"Type".to_vec(), Object::name("Outlines"));
        let r = doc.add(Object::Dict(o));
        let root = doc.root().ok_or(OrganizeError::NoPageTree)?;
        doc.update_dict(root, |c| c.set(b"Outlines".to_vec(), Object::Ref(r)))?;
        return Ok(r);
    }
    Ok(resolve_path(doc, parent_path)?.0)
}

/// Set `key` on the dictionary `r` (or remove it with `None`) only if it changes.
fn put(doc: &mut Document, r: ObjRef, key: &[u8], value: Option<Object>) -> Result<()> {
    let current = doc.get(r).as_dict().and_then(|d| d.get(key).cloned());
    if current == value {
        return Ok(());
    }
    doc.update_dict(r, |d| match value {
        Some(v) => d.set(key.to_vec(), v),
        None => {
            d.remove(key);
        }
    })?;
    Ok(())
}

/// Make `kids` the children of `parent`, in order: rewires `/Parent`, `/Prev`, `/Next`,
/// `/First` and `/Last`.
fn relink(doc: &mut Document, parent: ObjRef, kids: &[ObjRef]) -> Result<()> {
    for (i, k) in kids.iter().enumerate() {
        put(doc, *k, b"Parent", Some(Object::Ref(parent)))?;
        put(doc, *k, b"Prev", i.checked_sub(1).map(|p| Object::Ref(kids[p])))?;
        put(doc, *k, b"Next", kids.get(i + 1).map(|n| Object::Ref(*n)))?;
    }
    put(doc, parent, b"First", kids.first().map(|k| Object::Ref(*k)))?;
    put(doc, parent, b"Last", kids.last().map(|k| Object::Ref(*k)))?;
    Ok(())
}

/// Recompute every `/Count` (§12.3.3: open items count their visible descendants, closed items
/// the negative of what opening them would show; the root counts all visible items).
fn recount(doc: &mut Document) -> Result<()> {
    let Some(root) = outline_root(doc) else { return Ok(()) };
    let mut seen = HashSet::from([root]);
    // Returns the number of items visible below `node` when `node` is open.
    fn visit(doc: &mut Document, node: ObjRef, is_root: bool, seen: &mut HashSet<ObjRef>) -> Result<i64> {
        let kids = children_of(doc, node, seen);
        let mut visible = 0i64;
        for k in &kids {
            let below = visit(doc, *k, false, seen)?;
            let open = doc.get(*k).as_dict().and_then(|d| d.int(b"Count")).is_some_and(|c| c > 0);
            visible += 1 + if open { below } else { 0 };
        }
        let count = if kids.is_empty() {
            None
        } else if is_root || doc.get(node).as_dict().and_then(|d| d.int(b"Count")).is_some_and(|c| c > 0) {
            Some(visible)
        } else {
            Some(-visible)
        };
        put(doc, node, b"Count", count.map(Object::Int))?;
        Ok(visible)
    }
    visit(doc, root, true, &mut seen)?;
    Ok(())
}

fn destination(doc: &Document, page: usize) -> Result<Object> {
    let pages = walk(doc)?;
    let (p, _) = pages.get(page).ok_or(OrganizeError::NoSuchPage(page))?;
    // `/XYZ null null null`: go to the page, keeping the reader's zoom (what Acrobat writes for
    // a new bookmark when the view has no specific position).
    Ok(Object::Array(vec![Object::Ref(*p), Object::name("XYZ"), Object::Null, Object::Null, Object::Null]))
}

/// Add a bookmark titled `title` that goes to `page` (0-based), as child `index` of
/// `parent_path` (`[]` = top level; an index past the end appends). Returns its path.
pub fn add_bookmark(doc: &mut Document, parent_path: &[usize], index: usize, title: &str, page: usize) -> Result<Vec<usize>> {
    if title.trim().is_empty() {
        return Err(OutlineError::EmptyTitle);
    }
    let dest = destination(doc, page)?;
    let parent = container(doc, parent_path)?;
    let mut kids = children_of(doc, parent, &mut HashSet::from([parent]));
    let mut d = Dict::new();
    d.set(b"Title".to_vec(), Object::String(PdfString::text(title)));
    d.set(b"Dest".to_vec(), dest);
    let item = doc.add(Object::Dict(d));
    let at = index.min(kids.len());
    kids.insert(at, item);
    relink(doc, parent, &kids)?;
    // Show the new bookmark: open its parent (the root is always open).
    if !parent_path.is_empty() && doc.get(parent).as_dict().and_then(|d| d.int(b"Count")).is_none_or(|c| c <= 0) {
        put(doc, parent, b"Count", Some(Object::Int(1)))?;
    }
    recount(doc)?;
    let mut path = parent_path.to_vec();
    path.push(at);
    Ok(path)
}

/// Change a bookmark's title.
pub fn rename_bookmark(doc: &mut Document, path: &[usize], title: &str) -> Result<()> {
    if title.trim().is_empty() {
        return Err(OutlineError::EmptyTitle);
    }
    let (r, _) = resolve_path(doc, path)?;
    put(doc, r, b"Title", Some(Object::String(PdfString::text(title))))
}

/// Point a bookmark at `page` (0-based), replacing its destination or GoTo action.
pub fn set_bookmark_page(doc: &mut Document, path: &[usize], page: usize) -> Result<()> {
    let dest = destination(doc, page)?;
    let (r, _) = resolve_path(doc, path)?;
    put(doc, r, b"Dest", Some(dest))?;
    put(doc, r, b"A", None) // `/Dest` and `/A` must not both be present (§12.3.3)
}

/// Remove a bookmark and everything under it. The removed items are unlinked (a full save drops
/// them; an incremental save leaves the old objects unreferenced).
pub fn delete_bookmark(doc: &mut Document, path: &[usize]) -> Result<()> {
    let (r, parent) = resolve_path(doc, path)?;
    let mut kids = children_of(doc, parent, &mut HashSet::from([parent]));
    kids.retain(|k| *k != r);
    relink(doc, parent, &kids)?;
    recount(doc)
}

/// Move the bookmark at `from` to child `index` of `to_parent` (counted after removing it from
/// its old place; past the end appends). Returns its new path.
pub fn move_bookmark(doc: &mut Document, from: &[usize], to_parent: &[usize], index: usize) -> Result<Vec<usize>> {
    if to_parent.starts_with(from) {
        return Err(OutlineError::IntoItself);
    }
    let (r, old_parent) = resolve_path(doc, from)?;
    let mut old = children_of(doc, old_parent, &mut HashSet::from([old_parent]));
    old.retain(|k| *k != r);
    relink(doc, old_parent, &old)?;
    // Removing `from` shifts later siblings: adjust a target path that runs through them.
    let mut to_parent = to_parent.to_vec();
    let depth = from.len() - 1;
    if to_parent.len() > depth && to_parent[..depth] == from[..depth] && to_parent[depth] > from[depth] {
        to_parent[depth] -= 1;
    }
    let parent = container(doc, &to_parent)?;
    let mut kids = children_of(doc, parent, &mut HashSet::from([parent]));
    let at = index.min(kids.len());
    kids.insert(at, r);
    relink(doc, parent, &kids)?;
    recount(doc)?;
    to_parent.push(at);
    Ok(to_parent)
}

/// Expand or collapse a bookmark that has children.
pub fn set_bookmark_open(doc: &mut Document, path: &[usize], open: bool) -> Result<()> {
    let (r, _) = resolve_path(doc, path)?;
    let count = doc.get(r).as_dict().and_then(|d| d.int(b"Count")).unwrap_or(0);
    if count != 0 && (count > 0) != open {
        put(doc, r, b"Count", Some(Object::Int(-count)))?;
        recount(doc)?;
    }
    Ok(())
}
