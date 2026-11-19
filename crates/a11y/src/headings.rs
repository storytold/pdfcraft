//! The document's headings (H, H1–H6 after the role map) with the text they mark: what New
//! Bookmarks from Structure turns into bookmarks.

use std::collections::{HashMap, HashSet};

use pdfcraft_content::Matrix;
use pdfcraft_cos::{Dict, Document, ObjRef, Object};
use pdfcraft_model::Page;

use crate::alt::{standard, tree_root};

const MAX_HEADINGS: usize = 10_000;
const MAX_VISITS: usize = 1_000_000;
const MAX_DEPTH: usize = 256;
/// Characters kept of a heading's title.
const MAX_TITLE: usize = 256;

/// A heading element.
#[derive(Clone, Debug, PartialEq)]
pub struct Heading {
    /// The structure element (`None` for a direct element, which can't be referenced).
    pub obj: Option<ObjRef>,
    /// 1 for H1 … 6 for H6; a plain H takes the number of sections (Sect, Part, Art) around it.
    pub level: u8,
    pub title: String,
    /// 0-based page of its content.
    pub page: usize,
}

struct Walker<'a> {
    doc: &'a Document,
    role_map: Dict,
    pages: &'a [Page],
    page_index: HashMap<ObjRef, usize>,
    seen: HashSet<ObjRef>,
    visits: usize,
    /// Per page: the text each MCID shows (read once per page).
    texts: HashMap<usize, HashMap<i64, String>>,
    /// What is left of [`MAX_TITLE_SOURCE_BYTES`].
    title_left: usize,
    out: Vec<Heading>,
}

/// Bytes of `/ActualText`, `/Alt` and `/T` strings the headings of one document decode in all.
/// Titles are cut to [`MAX_TITLE`] characters, but many headings can name one large string, and
/// each would otherwise be decoded in full first.
const MAX_TITLE_SOURCE_BYTES: usize = 32 << 20;

/// The headings, in document order. Headings without text or a page are left out.
pub fn headings(doc: &Document) -> Vec<Heading> {
    let Some(root) = tree_root(doc) else { return Vec::new() };
    let pages = pdfcraft_model::pages(doc);
    let mut w = Walker {
        doc,
        role_map: root.get(b"RoleMap").map(|o| doc.resolve(o)).and_then(|o| o.as_dict().cloned()).unwrap_or_default(),
        pages: &pages,
        page_index: pages.iter().enumerate().map(|(i, p)| (p.obj, i)).collect(),
        seen: HashSet::new(),
        visits: 0,
        texts: HashMap::new(),
        title_left: MAX_TITLE_SOURCE_BYTES,
        out: Vec::new(),
    };
    if let Some(k) = root.get(b"K") {
        w.walk(k, None, 0, 0);
    }
    w.out
}

impl Walker<'_> {
    /// Whether `k` may be visited (bounded depth and size, each object once).
    fn enter(&mut self, k: &Object, depth: usize) -> bool {
        self.visits += 1;
        depth <= MAX_DEPTH && self.visits <= MAX_VISITS && self.out.len() < MAX_HEADINGS && !matches!(k, Object::Ref(r) if !self.seen.insert(*r))
    }

    fn element_of(&self, k: &Object) -> Option<(Option<ObjRef>, Dict)> {
        let d = match k {
            Object::Ref(r) => self.doc.get(*r).as_dict().cloned()?,
            Object::Dict(d) => d.clone(),
            _ => return None,
        };
        d.contains(b"S").then(|| (k.as_ref(), d))
    }

    fn walk(&mut self, k: &Object, pg: Option<ObjRef>, sections: u8, depth: usize) {
        if !self.enter(k, depth) {
            return;
        }
        if let Object::Array(a) = k {
            a.iter().for_each(|x| self.walk(x, pg, sections, depth + 1));
            return;
        }
        let Some((obj, d)) = self.element_of(k) else { return };
        let pg = d.get(b"Pg").and_then(Object::as_ref).or(pg);
        let ty = d.name(b"S").map(|s| standard(self.doc, &self.role_map, s)).unwrap_or_default();
        let level = match ty.as_slice() {
            [b'H', n @ b'1'..=b'6'] => Some(n - b'0'),
            b"H" => Some(sections.max(1)),
            _ => None,
        };
        if let Some(level) = level {
            self.heading(obj, &d, pg, level, depth);
            return;
        }
        let sections = if matches!(ty.as_slice(), b"Sect" | b"Part" | b"Art") { sections.saturating_add(1) } else { sections };
        if let Some(kids) = d.get(b"K") {
            self.walk(kids, pg, sections, depth + 1);
        }
    }

    /// The marked content under `k`: (page, MCID) pairs in order.
    fn marks(&mut self, k: &Object, pg: Option<ObjRef>, depth: usize, out: &mut Vec<(Option<ObjRef>, i64)>) {
        if !self.enter(k, depth) {
            return;
        }
        match k {
            Object::Int(n) => out.push((pg, *n)),
            Object::Array(a) => a.iter().for_each(|x| self.marks(x, pg, depth + 1, out)),
            Object::Dict(m) if m.name(b"Type") == Some(b"MCR") => {
                if let Some(n) = m.get(b"MCID").and_then(Object::as_int) {
                    out.push((m.get(b"Pg").and_then(Object::as_ref).or(pg), n));
                }
            }
            _ => {
                if let Some((_, d)) = self.element_of(k)
                    && let Some(kids) = d.get(b"K")
                {
                    self.marks(kids, d.get(b"Pg").and_then(Object::as_ref).or(pg), depth + 1, out);
                }
            }
        }
    }

    fn heading(&mut self, obj: Option<ObjRef>, d: &Dict, pg: Option<ObjRef>, level: u8, depth: usize) {
        let mut marks = Vec::new();
        if let Some(kids) = d.get(b"K") {
            self.marks(kids, pg, depth + 1, &mut marks);
        }
        let Some(page) = pg.or_else(|| marks.iter().find_map(|m| m.0)).and_then(|p| self.page_index.get(&p).copied()) else { return };
        let title = match self.attr(d, b"ActualText") {
            Some(t) => Some(t),
            None => match self.marked_text(&marks, page) {
                Some(t) => Some(t),
                None => self.attr(d, b"Alt").or_else(|| self.attr(d, b"T")),
            },
        };
        let Some(title) = title else { return };
        let title: String = title.split_whitespace().collect::<Vec<_>>().join(" ").chars().take(MAX_TITLE).collect();
        self.out.push(Heading { obj, level, title, page });
    }

    /// The non-blank text of the string `key` holds, charged against the title allowance by the
    /// longer of its raw and decoded size (a string that does not fit is not decoded).
    fn attr(&mut self, d: &Dict, key: &[u8]) -> Option<String> {
        let o = self.doc.resolve(d.get(key)?);
        let s = o.as_string()?;
        // A text string decodes to at most three bytes per raw byte, so one whose worst case does
        // not fit the remaining allowance is never decoded: the raw check alone let a string at
        // the raw limit decode to three times it before the allowance was reduced.
        if s.bytes.len().saturating_mul(3) > self.title_left {
            return None;
        }
        let text = s.to_text();
        self.title_left = self.title_left.saturating_sub(text.len().max(s.bytes.len()));
        Some(text).filter(|t| !t.trim().is_empty())
    }

    /// The text the marked content shows (`page`: where marks without a page of their own are).
    fn marked_text(&mut self, marks: &[(Option<ObjRef>, i64)], page: usize) -> Option<String> {
        let mut parts = Vec::new();
        // The title is cut to MAX_TITLE characters later: stop gathering well before a heading
        // that repeats one mark many times can pile up gigabytes.
        let mut gathered = 0usize;
        for (p, id) in marks {
            if gathered >= MAX_TITLE * 4 {
                break;
            }
            let page = p.and_then(|p| self.page_index.get(&p).copied()).unwrap_or(page);
            if let Some(text) = self.texts_of(page).get(id) {
                gathered = gathered.saturating_add(text.len()).saturating_add(1);
                parts.push(text.clone());
            }
        }
        Some(parts.join(" ")).filter(|t| !t.trim().is_empty())
    }

    fn texts_of(&mut self, page: usize) -> &HashMap<i64, String> {
        let (doc, pages) = (self.doc, self.pages);
        self.texts.entry(page).or_insert_with(|| pages.get(page).map(|p| mcid_texts(doc, &p.dict)).unwrap_or_default())
    }
}

/// The text each marked-content id shows on a page (text drawn directly in the page content).
///
/// Word breaks come from where the glyphs are, as on the page: a glyph that starts on another
/// line, more than 0.2 em past the end of the previous glyph of the same id, or back over more
/// than an em from it, starts a new word (the em is the font size times |`Tz`|; see [`new_word`]).
/// Text-positioning operators alone don't break words: producers that place every glyph with its
/// own `Td` (Chromium) would otherwise read as "C h a p t e r".
pub(crate) fn mcid_texts(doc: &Document, page: &Dict) -> HashMap<i64, String> {
    let mut out: HashMap<i64, String> = HashMap::new();
    // Where each id's last glyph ended, in user space.
    let mut ends: HashMap<i64, (f64, f64)> = HashMap::new();
    crate::glyphs::for_each_glyph(doc, page, |g| {
        let Some(id) = g.id else { return };
        // A glyph's text is only built while its id still takes more.
        if out.get(&id).is_none_or(|t| t.len() < MAX_TITLE * 4)
            && let Some(glyph) = g.metrics.text_of(g.code)
        {
            let text = out.entry(id).or_default();
            let breaks = ends.get(&id).is_some_and(|end| new_word(&g.trm, *end, g.size, g.scale));
            if breaks && !text.is_empty() && !text.ends_with(' ') && !glyph.starts_with(' ') {
                text.push(' ');
            }
            text.push_str(&glyph);
        }
        ends.insert(id, g.end);
    });
    out
}

/// Does a glyph drawn with text rendering matrix `trm` (text space → user space) start a new word
/// after a glyph that ended at `end` (user space)? Yes on another line (more than half the absolute
/// font size up or down), more than 0.2 horizontal em ahead, or back over more than a horizontal em.
/// The horizontal em is the absolute value of font size times horizontal scale (`Tz`); "ahead" follows
/// the glyph advance, backwards when the size or `Tz` is negative. `Tz` never scales the vertical threshold.
fn new_word(trm: &Matrix, end: (f64, f64), font_size: f64, horizontal_scale: f64) -> bool {
    let advance = font_size * horizontal_scale;
    let em = advance.abs();
    let start = trm.apply(0.0, 0.0);
    let [a, b, c, d, _, _] = trm.0;
    let Some(back) = Matrix([a, b, c, d, 0.0, 0.0]).invert() else { return true };
    let (dx, dy) = back.apply(start.0 - end.0, start.1 - end.1);
    let ahead = if advance < 0.0 { -dx } else { dx };
    !(ahead.is_finite() && dy.is_finite()) || dy.abs() > font_size.abs() * 0.5 || ahead > em * 0.2 || ahead < -em
}
