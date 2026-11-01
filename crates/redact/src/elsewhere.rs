//! Places [`crate::apply`] leaves as they are that may still hold the removed text (#2448
//! review): bookmark titles, attachments, the comments that remain, and XMP on objects other than
//! the catalog and the pages. Removing them is Remove hidden information's job (its bookmarks,
//! attachments, comments and metadata categories); applying redactions only counts them, so the
//! user can be told. Nothing here changes the document.
//!
//! The text is matched as the metadata check matches it ([`crate::metadata::Search`]), with a
//! work limit of its own. Every walk is bounded: outline items and name-tree nodes are each read
//! once (a seen-set for indirect ones) up to a cap, and the XMP walk is Remove hidden
//! information's own.

use std::collections::HashSet;

use pdfcraft_cos::{Document, ObjRef, Object};

use crate::metadata::Search;

/// Places that may still hold the removed text after [`crate::apply`], which it doesn't change.
/// Remove hidden information removes them. Each count is of places, not of occurrences.
///
/// When the removed text couldn't be searched for (`unsearched`: the text of a removed glyph
/// isn't known, a form was removed unread, or there was too much removed text to collect or
/// search for, as for the metadata check), every place holding any text is counted, since any
/// of them may hold it. A count may also include places that couldn't be read or that lie past
/// a walk's cap (one for each walk cut short), which count as holding it, and, once the search's
/// work limit runs out, every place not yet checked that holds any text.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Elsewhere {
    /// Bookmarks (outline items) whose title holds it.
    pub bookmarks: usize,
    /// Embedded files whose name in the document's list of attachments (`/EmbeddedFiles`),
    /// file name (`/F`, `/UF`) or description (`/Desc`) holds it, and file attachment
    /// annotations whose file's name or description does.
    pub attachments: usize,
    /// Annotations left on the pages (not form field widgets or pop-ups) whose text
    /// (`/Contents`, rich text `/RC`, subject `/Subj`) or author (`/T`) holds it.
    pub comments: usize,
    /// XMP streams on objects other than the catalog and the pages (images, fonts, form
    /// XObjects, annotations…) whose text holds it, or that can't be read.
    pub xmp: usize,
    /// The removed text couldn't be searched for, so the counts are of every place that holds
    /// any text.
    pub unsearched: bool,
}

impl Elsewhere {
    /// All the places counted.
    pub fn total(&self) -> usize {
        self.bookmarks.saturating_add(self.attachments).saturating_add(self.comments).saturating_add(self.xmp)
    }
}

/// The most outline items read.
const MAX_OUTLINE_ITEMS: usize = 100_000;
/// The most name-tree nodes read, and how deep the tree is followed.
const MAX_NAME_TREE_NODES: usize = 100_000;
const MAX_NAME_TREE_DEPTH: usize = 32;

/// Count the places that hold the `removed` text (see [`Elsewhere`]). `complete` is false when
/// the collection of removed text was cut short.
pub(crate) fn find(doc: &Document, removed: &[String], complete: bool) -> Elsewhere {
    let search = Search::of(removed, complete);
    if search.is_nothing() {
        return Elsewhere::default();
    }
    // A blank value holds nothing, even when everything counts.
    let hit =
        |o: Option<&Object>| o.and_then(|o| doc.resolve(o).as_string().map(|s| s.to_text())).is_some_and(|t| !t.trim().is_empty() && search.text(&t));
    Elsewhere {
        bookmarks: bookmarks(doc, &hit),
        attachments: attachments(doc, &hit),
        comments: comments(doc, &search, &hit),
        xmp: xmp(doc, &search),
        unsearched: search.is_everything(),
    }
}

/// Outline items whose `/Title` holds the text.
fn bookmarks(doc: &Document, hit: &dyn Fn(Option<&Object>) -> bool) -> usize {
    let Some(root) = doc.root() else { return 0 };
    let Some(first) =
        doc.get(root).as_dict().and_then(|c| c.get(b"Outlines").cloned()).and_then(|o| doc.resolve(&o).as_dict()?.get(b"First").cloned())
    else {
        return 0;
    };
    let (mut n, mut read) = (0usize, 0usize);
    let mut seen = HashSet::new();
    let mut next = vec![first];
    while let Some(o) = next.pop() {
        if let Object::Ref(r) = &o
            && !seen.insert(*r)
        {
            continue;
        }
        let Some(item) = doc.resolve(&o).as_dict().cloned() else { continue };
        if read >= MAX_OUTLINE_ITEMS {
            // Items past the cap aren't read: count one for them.
            return n.saturating_add(1);
        }
        read += 1;
        n += usize::from(hit(item.get(b"Title")));
        next.extend([b"Next".as_slice(), b"First"].into_iter().filter_map(|k| item.get(k).cloned()));
    }
    n
}

/// Embedded files in the `/EmbeddedFiles` name tree whose name, file name or description holds
/// the text, and file attachment annotations whose file's do.
fn attachments(doc: &Document, hit: &dyn Fn(Option<&Object>) -> bool) -> usize {
    let file = |spec: Option<&Object>| {
        spec.and_then(|s| doc.resolve(s).as_dict().cloned()).is_some_and(|d| [b"F".as_slice(), b"UF", b"Desc"].iter().any(|k| hit(d.get(k))))
    };
    let mut n = 0usize;
    let tree = doc
        .root()
        .and_then(|r| doc.get(r).as_dict()?.get(b"Names").cloned())
        .and_then(|names| doc.resolve(&names).as_dict()?.get(b"EmbeddedFiles").cloned());
    if let Some(tree) = tree {
        let mut seen = HashSet::new();
        let mut next = vec![(tree, 0usize)];
        let mut read = 0usize;
        while let Some((o, depth)) = next.pop() {
            if let Object::Ref(r) = &o
                && !seen.insert(*r)
            {
                continue;
            }
            let Some(node) = doc.resolve(&o).as_dict().cloned() else { continue };
            if read >= MAX_NAME_TREE_NODES || depth > MAX_NAME_TREE_DEPTH {
                // A node past the cap isn't read: count one for what it holds.
                n = n.saturating_add(1);
                break;
            }
            read += 1;
            if let Some(leaves) = node.get(b"Names").map(|l| doc.resolve(l)).and_then(|l| l.as_array().cloned()) {
                for pair in leaves.chunks(2) {
                    n += usize::from(hit(pair.first()) || file(pair.get(1)));
                }
            }
            if let Some(kids) = node.get(b"Kids").map(|k| doc.resolve(k)).and_then(|k| k.as_array().cloned()) {
                next.extend(kids.into_iter().map(|k| (k, depth.saturating_add(1))));
            }
        }
    }
    for page in pdfcraft_model::pages(doc) {
        for a in crate::annots_of(doc, &page.dict) {
            let Some(a) = doc.resolve(&a).as_dict().cloned() else { continue };
            if a.name(b"Subtype") == Some(b"FileAttachment".as_slice()) {
                n += usize::from(file(a.get(b"FS")));
            }
        }
    }
    n
}

/// Annotations on the pages, other than form field widgets and pop-ups (which show their
/// parent's text), whose text or author holds it.
fn comments(doc: &Document, search: &Search, hit: &dyn Fn(Option<&Object>) -> bool) -> usize {
    let mut seen = HashSet::new();
    let mut n = 0usize;
    for page in pdfcraft_model::pages(doc) {
        for a in crate::annots_of(doc, &page.dict) {
            if let Some(r) = a.as_ref()
                && !seen.insert(r)
            {
                continue;
            }
            let Some(a) = doc.resolve(&a).as_dict().cloned() else { continue };
            if matches!(a.name(b"Subtype"), Some(b"Widget" | b"Popup")) {
                continue;
            }
            // Rich text is XHTML: its text is searched, not its markup. A rich text stream that
            // can't be read counts as holding it.
            let rich = a.get(b"RC").is_some_and(|o| rich_text(doc, o).is_none_or(|t| !t.trim().is_empty() && search.text(&t)));
            n += usize::from(rich || [b"Contents".as_slice(), b"T", b"Subj"].iter().any(|k| hit(a.get(k))));
        }
    }
    n
}

/// The text of a rich text string or stream (`/RC`), markup left out; `None` when a stream
/// can't be read (strictly, as UTF-8, within the XMP limit).
fn rich_text(doc: &Document, o: &Object) -> Option<String> {
    let xml = match &*doc.resolve(o) {
        Object::String(s) => s.to_text(),
        Object::Stream(s) if s.filters().len() <= 1 => String::from_utf8(s.decoded_first_filter_strict(crate::metadata::MAX_XMP_BYTES).ok()?).ok()?,
        Object::Stream(_) => return None,
        _ => return Some(String::new()),
    };
    Some(crate::metadata::xml_values(&xml).join(" "))
}

/// XMP streams on anything reachable from the catalog other than the catalog's and the pages'
/// own, each counted once, that hold the text or can't be read.
fn xmp(doc: &Document, search: &Search) -> usize {
    let scrubbed: HashSet<ObjRef> = crate::metadata::catalog_and_pages(doc).into_iter().collect();
    let Ok(values) = crate::sanitize::owned_values(doc, b"Metadata") else {
        // A graph too deep to walk: some XMP may be there, unread.
        return 1;
    };
    let mut seen = HashSet::new();
    let mut n = 0usize;
    for (holder, own, v) in values {
        if own && scrubbed.contains(&holder) {
            continue;
        }
        if let Some(r) = v.as_ref()
            && !seen.insert(r)
        {
            continue;
        }
        // An entry that isn't a stream holds no XMP.
        if !matches!(&*doc.resolve(&v), Object::Stream(_)) {
            continue;
        }
        n += usize::from(search.xmp(doc, &v));
    }
    n
}
