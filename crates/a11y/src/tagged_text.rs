//! The document's text in tag order: what Export ▸ Text from tags writes.
//!
//! The structure tree is read from the root, each element's `/K` in order, so an element's own
//! marked content and its child elements interleave as listed. Text that no tag holds (artifacts
//! and untagged content) is left out. `/ActualText` replaces the text of the element it is on;
//! `/Alt` replaces the content of a Figure, Formula or Form and, on any other element, stands in
//! only when the element marks no text. Block elements start a line; the cells of a table row are
//! separated by tabs; other pieces are joined by a space.

use std::collections::{HashMap, HashSet};

use pdfcraft_cos::{Dict, Document, ObjRef, Object};
use pdfcraft_model::Page;

use crate::alt::{standard, tree_root};
use crate::headings::mcid_texts_up_to;

const MAX_VISITS: usize = 1_000_000;
const MAX_DEPTH: usize = 256;
/// Bytes of text one export gathers (also bounds the replacement strings it decodes).
const MAX_TEXT: usize = 64 << 20;

/// The text of a tagged document, in tag order.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TaggedText {
    /// Lines of text, ending with a line break (empty when the tags mark no text).
    pub text: String,
    /// Content the tags hold that this reader can't read yet, so it is not in `text`: marked
    /// content in a form XObject (`/Stm`), form fields and comments (`/OBJR` to a widget, or to
    /// an annotation other than a link that has `/Contents`), and marked content whose page isn't
    /// known.
    pub unread: usize,
    /// The tree is larger or deeper than one export reads, so `text` stops early.
    pub truncated: bool,
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum TaggedTextError {
    #[error("the document has no tags")]
    Untagged,
}

/// What separates the next piece of text from the last one; a larger one wins.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum Gap {
    Space,
    Tab,
    Line,
}

struct Reader<'a> {
    doc: &'a Document,
    role_map: Dict,
    pages: &'a [Page],
    page_index: HashMap<ObjRef, usize>,
    seen: HashSet<ObjRef>,
    visits: usize,
    /// Per page: the text each MCID shows (read once per page).
    texts: HashMap<usize, HashMap<i64, String>>,
    gap: Gap,
    /// For each open table row, how many cells it has started.
    rows: Vec<usize>,
    /// A cell after the first in its row started and has put no text yet: its first text follows
    /// a tab, even after a block inside the cell asked for a line.
    cell_tab: bool,
    out: TaggedText,
}

/// The document's text in tag order (see the module documentation).
pub fn tagged_text(doc: &Document) -> Result<TaggedText, TaggedTextError> {
    let root = tree_root(doc).ok_or(TaggedTextError::Untagged)?;
    let pages = pdfcraft_model::pages(doc);
    let mut r = Reader {
        doc,
        role_map: root.get(b"RoleMap").map(|o| doc.resolve(o)).and_then(|o| o.as_dict().cloned()).unwrap_or_default(),
        pages: &pages,
        page_index: pages.iter().enumerate().map(|(i, p)| (p.obj, i)).collect(),
        seen: HashSet::new(),
        visits: 0,
        texts: HashMap::new(),
        gap: Gap::Space,
        rows: Vec::new(),
        cell_tab: false,
        out: TaggedText::default(),
    };
    if let Some(k) = root.get(b"K") {
        r.walk(k, None, 0);
    }
    if !r.out.text.is_empty() {
        r.out.text.push('\n');
    }
    Ok(r.out)
}

/// Standard types that start a line of their own (ISO 32000-2 §14.8.4; PDF 1.7's grouping
/// elements included).
fn is_block(ty: &[u8]) -> bool {
    matches!(
        ty,
        b"Document"
            | b"DocumentFragment"
            | b"Part"
            | b"Art"
            | b"Sect"
            | b"Div"
            | b"Aside"
            | b"BlockQuote"
            | b"Caption"
            | b"TOC"
            | b"TOCI"
            | b"Index"
            | b"P"
            | b"H"
            | b"H1"
            | b"H2"
            | b"H3"
            | b"H4"
            | b"H5"
            | b"H6"
            | b"Title"
            | b"L"
            | b"LI"
            | b"Table"
            | b"THead"
            | b"TBody"
            | b"TFoot"
            | b"TR"
            | b"Note"
            | b"FENote"
            | b"Figure"
            | b"Formula"
    )
}

impl Reader<'_> {
    fn full(&self) -> bool {
        self.out.text.len() >= MAX_TEXT
    }

    fn walk(&mut self, k: &Object, pg: Option<ObjRef>, depth: usize) {
        if self.full() {
            self.out.truncated = true;
            return;
        }
        self.visits += 1;
        if depth > MAX_DEPTH || self.visits > MAX_VISITS {
            self.out.truncated = true;
            return;
        }
        match k {
            Object::Ref(r) => {
                if !self.seen.insert(*r) {
                    return;
                }
                let target = self.doc.get(*r);
                self.walk(&target, pg, depth + 1);
            }
            Object::Array(a) => a.iter().for_each(|x| self.walk(x, pg, depth + 1)),
            Object::Int(n) => self.mark(pg, *n),
            Object::Dict(d) if d.contains(b"S") => self.element(d, d.get(b"Pg").and_then(Object::as_ref).or(pg), depth),
            Object::Dict(d) => {
                let pg = d.get(b"Pg").and_then(Object::as_ref).or(pg);
                if d.contains(b"Stm") {
                    // Marked content inside a form XObject: its own MCID namespace.
                    self.out.unread += 1;
                } else if let Some(n) = d.get(b"MCID").and_then(Object::as_int) {
                    self.mark(pg, n);
                } else if let Some(obj) = d.get(b"Obj").map(|o| self.doc.resolve(o))
                    && obj
                        .as_dict()
                        .is_some_and(|a| a.name(b"Subtype") == Some(b"Widget") || (a.name(b"Subtype") != Some(b"Link") && a.contains(b"Contents")))
                {
                    // A form field's value or a comment's text: not read here. A link carries its
                    // text in marked content next to the reference.
                    self.out.unread += 1;
                }
            }
            _ => {}
        }
    }

    fn element(&mut self, d: &Dict, pg: Option<ObjRef>, depth: usize) {
        let ty = d.name(b"S").map(|s| standard(self.doc, &self.role_map, s)).unwrap_or_default();
        let block = is_block(&ty);
        let row = ty.as_slice() == b"TR";
        if block {
            self.gap(Gap::Line);
        } else if matches!(ty.as_slice(), b"TD" | b"TH") {
            self.gap(Gap::Tab);
            if let Some(cells) = self.rows.last_mut() {
                *cells += 1;
                self.cell_tab = *cells > 1;
            }
        }
        if row {
            self.rows.push(0);
        }
        // An empty /ActualText is still a replacement: the element reads as nothing.
        if let Some(t) = self.attr(d, b"ActualText") {
            self.put(&t);
        } else {
            let alt = self.attr(d, b"Alt").filter(|t| !t.trim().is_empty());
            match alt {
                Some(a) if matches!(ty.as_slice(), b"Figure" | b"Formula" | b"Form") => self.put(&a),
                _ => {
                    let before = self.out.text.len();
                    if let Some(kids) = d.get(b"K") {
                        self.walk(kids, pg, depth + 1);
                    }
                    if self.out.text.len() == before
                        && let Some(a) = alt
                    {
                        self.put(&a);
                    }
                }
            }
        }
        if row {
            self.rows.pop();
            self.cell_tab = false;
        }
        if block {
            self.gap(Gap::Line);
        }
    }

    fn mark(&mut self, pg: Option<ObjRef>, mcid: i64) {
        let Some(page) = pg.and_then(|p| self.page_index.get(&p).copied()) else {
            self.out.unread += 1;
            return;
        };
        let (doc, pages) = (self.doc, self.pages);
        let texts = match self.texts.entry(page) {
            std::collections::hash_map::Entry::Occupied(e) => e.into_mut(),
            std::collections::hash_map::Entry::Vacant(e) => {
                // Each id's whole text, up to what one export gathers; more is reported.
                let (texts, cut) = pages.get(page).map(|p| mcid_texts_up_to(doc, &p.dict, MAX_TEXT)).unwrap_or_default();
                self.out.truncated |= cut;
                e.insert(texts)
            }
        };
        let text = texts.get(&mcid).cloned();
        if let Some(text) = text {
            self.put(&text);
        }
    }

    fn gap(&mut self, gap: Gap) {
        self.gap = self.gap.max(gap);
    }

    /// Append a piece of text after the pending gap (nothing for blank text).
    fn put(&mut self, text: &str) {
        let text = text.trim();
        if text.is_empty() {
            return;
        }
        if !self.out.text.is_empty() {
            self.out.text.push(match self.gap {
                _ if self.cell_tab => '\t',
                Gap::Space => ' ',
                Gap::Tab => '\t',
                Gap::Line => '\n',
            });
        }
        self.out.text.push_str(text);
        self.gap = Gap::Space;
        self.cell_tab = false;
    }

    /// The text of the string `key` holds, unless it would not fit what is left of [`MAX_TEXT`]
    /// (then the export stops there).
    fn attr(&mut self, d: &Dict, key: &[u8]) -> Option<String> {
        let o = self.doc.resolve(d.get(key)?);
        let s = o.as_string()?;
        // A text string decodes to at most three bytes per raw byte.
        if s.bytes.len().saturating_mul(3) > MAX_TEXT.saturating_sub(self.out.text.len()) {
            self.out.truncated = true;
            return None;
        }
        Some(s.to_text())
    }
}
