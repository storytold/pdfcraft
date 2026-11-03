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
const MAX_TAGGED_TEXT: usize = 32 * 1024 * 1024;
const MAX_TAGGED_WARNINGS: usize = 100;
const MAX_TAGGED_TASKS: usize = 100_000;

/// Text in structure-tree order and repairs or omissions found while reading it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TaggedText {
    pub text: String,
    pub warnings: Vec<String>,
}

fn tagged_warning(out: &mut TaggedText, warning: String) {
    if out.warnings.contains(&warning) {
        return;
    }
    if out.warnings.len() < MAX_TAGGED_WARNINGS {
        out.warnings.push(warning);
    } else if out.warnings.len() == MAX_TAGGED_WARNINGS {
        out.warnings.push("additional tagged-text warnings were omitted".into());
    }
}

/// Read reachable marked content in tree order. Artifacts are absent because they have no
/// structure-tree reference. Cycles, missing objects, and limits are reported instead of guessed.
pub fn tagged_text(doc: &Document, selected_pages: &[usize]) -> TaggedText {
    let Some(root) = tree_root(doc) else {
        return TaggedText { text: String::new(), warnings: vec!["the document has no structure tree".into()] };
    };
    let pages = pdfcraft_model::pages(doc);
    let page_index: HashMap<ObjRef, usize> = pages.iter().enumerate().map(|(i, p)| (p.obj, i)).collect();
    let selected: HashSet<usize> = selected_pages.iter().copied().collect();
    let mut texts = HashMap::<usize, HashMap<i64, String>>::new();
    let mut seen = HashSet::new();
    let objects: HashSet<u32> = doc.object_numbers().into_iter().collect();
    let mut out = TaggedText::default();
    let mut used_marks = HashSet::new();
    let mut replacement_pages = HashSet::new();
    let mut last_page = None;
    if root.get(b"K").is_none() {
        tagged_warning(&mut out, "the structure tree has no root content".into());
    }
    let mut tasks: Vec<(Object, Option<ObjRef>, usize)> = root.get(b"K").cloned().map(|k| vec![(k, None, 0)]).unwrap_or_default();
    let mut visits = 0usize;
    while let Some((item, inherited_page, depth)) = tasks.pop() {
        visits = visits.saturating_add(1);
        if visits > 1_000_000 || depth > MAX_DEPTH {
            tagged_warning(&mut out, "structure traversal stopped at its safety limit".into());
            break;
        }
        let item = match item {
            Object::Ref(r) => {
                if !objects.contains(&r.num) {
                    tagged_warning(&mut out, format!("missing structure reference {}", r.num));
                    continue;
                }
                if !seen.insert(r) {
                    tagged_warning(&mut out, format!("cyclic or repeated structure reference {}", r.num));
                    continue;
                }
                (*doc.get(r)).clone()
            }
            item => item,
        };
        match item {
            Object::Array(items) => {
                let count = items.len().min(MAX_TAGGED_TASKS.saturating_sub(tasks.len()));
                if count < items.len() {
                    tagged_warning(&mut out, "structure traversal stopped at its pending-item limit".into());
                }
                tasks.extend(items.into_iter().take(count).rev().map(|item| (item, inherited_page, depth + 1)));
            }
            Object::Int(mcid) => match inherited_page.and_then(|p| page_index.get(&p).copied()) {
                Some(page) if selected.contains(&page) => {
                    let page_text = texts
                        .entry(page)
                        .or_insert_with(|| pages.get(page).map(|p| mcid_texts_limited(doc, &p.dict, MAX_TAGGED_TEXT)).unwrap_or_default());
                    match page_text.get(&mcid) {
                        Some(text) if text.len() < MAX_TAGGED_TEXT && out.text.len().saturating_add(text.len()) <= MAX_TAGGED_TEXT => {
                            if !used_marks.insert((page, mcid)) {
                                tagged_warning(&mut out, format!("marked content {mcid} on page {} is referenced more than once", page + 1));
                                continue;
                            }
                            if last_page.is_some_and(|previous| previous != page) {
                                out.text.push('\u{c}');
                            } else if !out.text.is_empty() && !out.text.ends_with('\n') {
                                out.text.push('\n');
                            }
                            last_page = Some(page);
                            out.text.push_str(text);
                        }
                        Some(_) => {
                            tagged_warning(&mut out, "tagged text stopped at its size limit".into());
                            break;
                        }
                        None => tagged_warning(&mut out, format!("missing marked content {mcid} on page {}", page + 1)),
                    }
                }
                Some(_) => {}
                None if inherited_page.is_some() => tagged_warning(&mut out, format!("marked content {mcid} references a missing page")),
                None => tagged_warning(&mut out, format!("marked content {mcid} has no page association")),
            },
            Object::Dict(dict) => {
                let page = dict.get(b"Pg").and_then(Object::as_ref).or(inherited_page);
                if dict.name(b"Type") == Some(b"MCR") {
                    if let Some(mcid) = dict.get(b"MCID").and_then(Object::as_int) {
                        tasks.push((Object::Int(mcid), dict.get(b"Pg").and_then(Object::as_ref).or(page), depth + 1));
                    } else {
                        tagged_warning(&mut out, "marked-content reference has no MCID".into());
                    }
                    continue;
                }
                if dict.contains(b"S") {
                    if dict.name(b"S") == Some(b"Artifact") {
                        continue;
                    }
                    let replacement = [b"ActualText".as_slice(), b"Alt".as_slice()]
                        .into_iter()
                        .filter(|key| *key != b"Alt" || dict.name(b"S") == Some(b"Figure"))
                        .find_map(|key| {
                            dict.get(key)
                                .map(|value| doc.resolve(value))
                                .and_then(|value| value.as_string().map(|s| s.to_text()))
                                .filter(|s| !s.is_empty())
                        });
                    if let Some(replacement) = replacement {
                        if let Some(index) = page.and_then(|p| page_index.get(&p).copied()).filter(|p| selected.contains(p)) {
                            replacement_pages.insert(index);
                            if out.text.len().saturating_add(replacement.len()) <= MAX_TAGGED_TEXT {
                                if last_page.is_some_and(|previous| previous != index) {
                                    out.text.push('\u{c}');
                                } else if !out.text.is_empty() && !out.text.ends_with('\n') {
                                    out.text.push('\n');
                                }
                                last_page = Some(index);
                                out.text.push_str(&replacement);
                            } else {
                                tagged_warning(&mut out, "tagged text stopped at its size limit".into());
                                break;
                            }
                        } else if page.is_none() {
                            tagged_warning(&mut out, "alternate text has no page association".into());
                        }
                        continue;
                    } else if dict.get(b"K").is_none() {
                        tagged_warning(&mut out, "structure element has no content or alternate text".into());
                    }
                    if let Some(kids) = dict.get(b"K") {
                        tasks.push((kids.clone(), page, depth + 1));
                    }
                }
            }
            _ => tagged_warning(&mut out, "unsupported structure-tree content was skipped".into()),
        }
    }
    warn_unstructured_pages(doc, &pages, &selected, &mut texts, &replacement_pages, &used_marks, &mut out);
    out.warnings.sort();
    out.warnings.dedup();
    out
}

fn warn_unstructured_pages(
    doc: &Document,
    pages: &[Page],
    selected: &HashSet<usize>,
    texts: &mut HashMap<usize, HashMap<i64, String>>,
    replacement_pages: &HashSet<usize>,
    used_marks: &HashSet<(usize, i64)>,
    out: &mut TaggedText,
) {
    for page in selected {
        let page_text =
            texts.entry(*page).or_insert_with(|| pages.get(*page).map(|p| mcid_texts_limited(doc, &p.dict, MAX_TAGGED_TEXT)).unwrap_or_default());
        if !replacement_pages.contains(page) && page_text.iter().any(|(mcid, text)| !text.trim().is_empty() && !used_marks.contains(&(*page, *mcid)))
        {
            tagged_warning(out, format!("page {} contains text outside the structure tree", page + 1));
        }
    }
}

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
    mcid_texts_limited(doc, page, MAX_TITLE * 4)
}

fn mcid_texts_limited(doc: &Document, page: &Dict, max_text: usize) -> HashMap<i64, String> {
    let res = sub(doc, page, b"Resources");
    let (fonts, props) = (sub(doc, &res, b"Font"), sub(doc, &res, b"Properties"));
    let mut metrics: HashMap<Vec<u8>, Metrics> = HashMap::new();
    // The id each open marked-content sequence is in: its own, else the enclosing one's.
    let mut stack: Vec<(Option<i64>, bool)> = Vec::new();
    let mut out: HashMap<i64, String> = HashMap::new();
    // Where each id's last glyph ended, in user space.
    let mut ends: HashMap<i64, (f64, f64)> = HashMap::new();
    let mut gs = TextState::default();
    // With each saved state, the text and text line matrices: inside a text object `q`/`Q`
    // save and restore them too (ISO 32000-2 §9.4.1, as corrected by the PDF Association's
    // errata, issue 368), as the renderer does.
    let mut saved: Vec<(TextState, Matrix, Matrix)> = Vec::new();
    let mut unsaved = 0usize;
    let (mut tm, mut tlm) = (Matrix::IDENTITY, Matrix::IDENTITY);
    for op in pdfcraft_content::parse(&page_content(doc, page)).ops {
        crate::count_mark_reads(1);
        let (inherited_id, artifact) = stack.last().copied().unwrap_or((None, false));
        let id = (!artifact).then_some(inherited_id).flatten();
        match op.op.as_slice() {
            b"BMC" => stack.push((inherited_id, artifact || op.name(0) == Some(b"Artifact"))),
            b"BDC" => {
                let mcid = match op.operands.get(1) {
                    Some(Object::Dict(d)) => d.get(b"MCID").and_then(Object::as_int),
                    Some(Object::Name(n)) => {
                        props.get(n).map(|x| doc.resolve(x)).and_then(|x| x.as_dict().and_then(|d| d.get(b"MCID").and_then(Object::as_int)))
                    }
                    _ => None,
                }
                .or(inherited_id);
                stack.push((mcid, artifact || op.name(0) == Some(b"Artifact")));
            }
            b"EMC" => {
                stack.pop();
            }
            b"q" if saved.len() < MAX_SAVED => saved.push((gs.clone(), tm, tlm)),
            b"q" => unsaved += 1,
            b"Q" if unsaved > 0 => unsaved -= 1,
            b"Q" => {
                if let Some(saved) = saved.pop() {
                    (gs, tm, tlm) = saved;
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
                // A glyph's text is only built while its id still takes more.
                if let Some(id) = id
                    && out.get(&id).is_none_or(|t| t.len() < max_text)
                    && let Some(glyph) = m.text_of(code)
                {
                    let text = out.entry(id).or_default();
                    let breaks = ends.get(&id).is_some_and(|end| new_word(&trm, *end, gs.size, gs.scale));
                    if breaks && !text.is_empty() && !text.ends_with(' ') && !glyph.starts_with(' ') {
                        text.push(' ');
                    }
                    text.push_str(&glyph);
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
