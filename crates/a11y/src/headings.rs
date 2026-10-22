//! The document's headings (H, H1–H6 after the role map) with the text they mark: what New
//! Bookmarks from Structure turns into bookmarks.

use std::collections::{HashMap, HashSet};

use pdfcraft_content::Matrix;
use pdfcraft_cos::{Dict, Document, ObjRef, Object};
use pdfcraft_fonts::pdf::Metrics;
use pdfcraft_model::Page;

use crate::alt::{page_content, standard, tree_root};

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
    out: Vec<Heading>,
}

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
        let doc = self.doc;
        let attr = |key: &[u8]| d.get(key).map(|o| doc.resolve(o)).and_then(|o| o.as_string().map(|s| s.to_text())).filter(|t| !t.trim().is_empty());
        let title = match attr(b"ActualText") {
            Some(t) => Some(t),
            None => self.marked_text(&marks, page).or_else(|| attr(b"Alt")).or_else(|| attr(b"T")),
        };
        let Some(title) = title else { return };
        let title: String = title.split_whitespace().collect::<Vec<_>>().join(" ").chars().take(MAX_TITLE).collect();
        self.out.push(Heading { obj, level, title, page });
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

fn sub(doc: &Document, d: &Dict, key: &[u8]) -> Dict {
    d.get(key).map(|x| doc.resolve(x)).and_then(|x| x.as_dict().cloned()).unwrap_or_default()
}

/// The text each marked-content id shows on a page (text drawn directly in the page content).
///
/// Word breaks come from where the glyphs are, as on the page: a glyph that starts on another
/// line, more than 0.2 em past the end of the previous glyph of the same id, or back over more
/// than an em from it, starts a new word (the em is the font size times |`Tz`|; see [`new_word`]).
/// Text-positioning operators alone don't break words: producers that place every glyph with its
/// own `Td` (Chromium) would otherwise read as "C h a p t e r".
pub(crate) fn mcid_texts(doc: &Document, page: &Dict) -> HashMap<i64, String> {
    let res = sub(doc, page, b"Resources");
    let (fonts, props) = (sub(doc, &res, b"Font"), sub(doc, &res, b"Properties"));
    let mut metrics: HashMap<Vec<u8>, Metrics> = HashMap::new();
    let mut stack: Vec<Option<i64>> = Vec::new();
    let mut out: HashMap<i64, String> = HashMap::new();
    // Where each id's last glyph ended, in user space.
    let mut ends: HashMap<i64, (f64, f64)> = HashMap::new();
    let mut gs = TextState::default();
    let mut saved: Vec<TextState> = Vec::new();
    let mut unsaved = 0usize;
    let (mut tm, mut tlm) = (Matrix::IDENTITY, Matrix::IDENTITY);
    for op in pdfcraft_content::parse(&page_content(doc, page)).ops {
        let id = stack.iter().rev().find_map(|m| *m);
        match op.op.as_slice() {
            b"BMC" => stack.push(None),
            b"BDC" => stack.push(match op.operands.get(1) {
                Some(Object::Dict(d)) => d.get(b"MCID").and_then(Object::as_int),
                Some(Object::Name(n)) => {
                    props.get(n).map(|x| doc.resolve(x)).and_then(|x| x.as_dict().and_then(|d| d.get(b"MCID").and_then(Object::as_int)))
                }
                _ => None,
            }),
            b"EMC" => {
                stack.pop();
            }
            b"q" if saved.len() < MAX_SAVED => saved.push(gs.clone()),
            b"q" => unsaved += 1,
            b"Q" if unsaved > 0 => unsaved -= 1,
            b"Q" => {
                if let Some(saved) = saved.pop() {
                    gs = saved;
                }
            }
            b"cm" => {
                if let Some(m) = op.nums::<6>() {
                    gs.ctm = Matrix(m).then(&gs.ctm);
                }
            }
            b"Tf" => {
                gs.size = op.num(1).unwrap_or(gs.size);
                gs.font = op.name(0).map(<[u8]>::to_vec);
                if let Some(name) = op.name(0)
                    && !metrics.contains_key(name)
                    && let Some(f) = fonts.get(name).map(|f| doc.resolve(f)).and_then(|f| f.as_dict().cloned())
                {
                    metrics.insert(name.to_vec(), Metrics::from_dict(doc, &f));
                }
            }
            b"Tc" => gs.char_spacing = op.num(0).unwrap_or(0.0),
            b"Tw" => gs.word_spacing = op.num(0).unwrap_or(0.0),
            b"Tz" => gs.scale = op.num(0).unwrap_or(100.0) / 100.0,
            b"TL" => gs.leading = op.num(0).unwrap_or(0.0),
            b"BT" => (tm, tlm) = (Matrix::IDENTITY, Matrix::IDENTITY),
            b"Td" | b"TD" => {
                if let Some([x, y]) = op.nums::<2>() {
                    if op.op == b"TD" {
                        gs.leading = -y;
                    }
                    tlm = Matrix::translate(x, y).then(&tlm);
                    tm = tlm;
                }
            }
            b"Tm" => {
                if let Some(m) = op.nums::<6>() {
                    (tm, tlm) = (Matrix(m), Matrix(m));
                }
            }
            b"T*" => {
                tlm = Matrix::translate(0.0, -gs.leading).then(&tlm);
                tm = tlm;
            }
            _ => {}
        }
        let shows = matches!(op.op.as_slice(), b"Tj" | b"TJ" | b"'" | b"\"");
        let Some(m) = gs.font.as_ref().and_then(|f| metrics.get(f)).filter(|_| shows) else { continue };
        if op.op != b"Tj" && op.op != b"TJ" {
            if op.op == b"\"" {
                gs.word_spacing = op.num(0).unwrap_or(gs.word_spacing);
                gs.char_spacing = op.num(1).unwrap_or(gs.char_spacing);
            }
            tlm = Matrix::translate(0.0, -gs.leading).then(&tlm);
            tm = tlm;
        }
        let pieces: Vec<&Object> = match op.op.as_slice() {
            // A surplus array (`[(A)] [(B)] TJ`) is malformed, but the operator still takes the
            // one immediately before it, as the renderer and every other reader here do (#3569).
            b"TJ" => op.operands.last().and_then(Object::as_array).map(|a| a.iter().collect()).unwrap_or_default(),
            _ => op.operands.last().into_iter().collect(),
        };
        for piece in pieces {
            let s = match piece {
                Object::String(s) => s,
                other => {
                    // A TJ adjustment moves the next glyph; whether it breaks a word is decided there.
                    let dx = -other.as_f64().unwrap_or(0.0) / 1000.0 * gs.size * gs.scale;
                    tm = Matrix::translate(dx, 0.0).then(&tm);
                    continue;
                }
            };
            for (code, len) in m.codes(&s.bytes) {
                let trm = tm.then(&gs.ctm);
                if let Some(id) = id
                    && let Some(glyph) = m.text_of(code)
                {
                    let text = out.entry(id).or_default();
                    if text.len() < MAX_TITLE * 4 {
                        let breaks = ends.get(&id).is_some_and(|end| new_word(&trm, *end, gs.size, gs.scale));
                        if breaks && !text.is_empty() && !text.ends_with(' ') && !glyph.starts_with(' ') {
                            text.push(' ');
                        }
                        text.push_str(&glyph);
                    }
                }
                let advance = m.width(code) * gs.size + gs.char_spacing + if m.is_space(code, len) { gs.word_spacing } else { 0.0 };
                tm = Matrix::translate(advance * gs.scale, 0.0).then(&tm);
                if let Some(id) = id {
                    ends.insert(id, tm.then(&gs.ctm).apply(0.0, 0.0));
                }
            }
        }
    }
    out
}

/// How deep `q` nests before the text state stops being saved (a hostile stream can nest millions).
const MAX_SAVED: usize = 256;

/// The graphics state that places glyphs: the CTM, the font and the text state parameters.
#[derive(Clone)]
struct TextState {
    ctm: Matrix,
    font: Option<Vec<u8>>,
    size: f64,
    char_spacing: f64,
    word_spacing: f64,
    scale: f64,
    leading: f64,
}

impl Default for TextState {
    fn default() -> Self {
        TextState { ctm: Matrix::IDENTITY, font: None, size: 0.0, char_spacing: 0.0, word_spacing: 0.0, scale: 1.0, leading: 0.0 }
    }
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
