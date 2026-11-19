//! The glyphs a page's own content shows, each with the marked-content id it is in, where it is
//! drawn and how far it advances: what heading titles, tag text and tag boxes are read from.

use std::collections::HashMap;
use std::rc::Rc;

use pdfcraft_content::{Matrix, Op};
use pdfcraft_cos::{Dict, Document, ObjRef, Object};
use pdfcraft_fonts::pdf::Metrics;

use crate::alt::page_content;

/// How deep `q` nests before the text state stops being saved (a hostile stream can nest millions).
const MAX_SAVED: usize = 256;

/// One glyph shown on the page.
pub(crate) struct Glyph<'a> {
    /// The marked-content id it is in: the innermost sequence's, else the enclosing one's.
    pub id: Option<i64>,
    /// Text space (before the font size and `Tz`) to user space, at the glyph's origin.
    pub trm: Matrix,
    pub metrics: &'a Metrics,
    pub code: u32,
    pub size: f64,
    /// Horizontal scaling (`Tz` / 100).
    pub scale: f64,
    /// How far the next glyph starts, in text space before `Tz`: width, `Tc` and `Tw`.
    pub advance: f64,
}

impl Glyph<'_> {
    /// Where the next glyph starts, in user space.
    pub fn end(&self) -> (f64, f64) {
        self.trm.apply(self.advance * self.scale, 0.0)
    }

    /// The glyph's box (its advance width by the font's ascent and descent), in user space.
    pub fn bbox(&self) -> [f64; 4] {
        let m = self.metrics;
        let width = m.width(self.code) * self.size * self.scale;
        self.trm.bbox([0.0, m.descent * self.size, width, m.ascent * self.size])
    }
}

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

fn sub(doc: &Document, d: &Dict, key: &[u8]) -> Dict {
    d.get(key).map(|x| doc.resolve(x)).and_then(|x| x.as_dict().cloned()).unwrap_or_default()
}

/// The text state of a walk over one page's content.
struct Walk<'d> {
    doc: &'d Document,
    fonts: Dict,
    props: Dict,
    metrics: HashMap<Vec<u8>, Rc<Metrics>>,
    /// Built once per font object, however many resource names alias it.
    by_font: HashMap<ObjRef, Rc<Metrics>>,
    /// The id each open marked-content sequence is in: its own, else the enclosing one's.
    stack: Vec<Option<i64>>,
    gs: TextState,
    /// With each saved state, the text and text line matrices: inside a text object `q`/`Q`
    /// save and restore them too (ISO 32000-2 §9.4.1, as corrected by the PDF Association's
    /// errata, issue 368), as the renderer does.
    saved: Vec<(TextState, Matrix, Matrix)>,
    unsaved: usize,
    tm: Matrix,
    tlm: Matrix,
}

/// Calls `f` for each glyph the page's own content shows, in content order.
pub(crate) fn for_each_glyph(doc: &Document, page: &Dict, mut f: impl FnMut(&Glyph<'_>)) {
    let res = sub(doc, page, b"Resources");
    let mut walk = Walk {
        doc,
        fonts: sub(doc, &res, b"Font"),
        props: sub(doc, &res, b"Properties"),
        metrics: HashMap::new(),
        by_font: HashMap::new(),
        stack: Vec::new(),
        gs: TextState::default(),
        saved: Vec::new(),
        unsaved: 0,
        tm: Matrix::IDENTITY,
        tlm: Matrix::IDENTITY,
    };
    for op in pdfcraft_content::parse(&page_content(doc, page)).ops {
        crate::count_mark_reads(1);
        walk.state(&op);
        if matches!(op.op.as_slice(), b"Tj" | b"TJ" | b"'" | b"\"") {
            walk.show(&op, &mut f);
        }
    }
}

impl Walk<'_> {
    /// Applies an operator that changes the marked-content stack or the text state.
    fn state(&mut self, op: &Op) {
        let id = self.stack.last().copied().flatten();
        let gs = &mut self.gs;
        match op.op.as_slice() {
            b"BMC" => self.stack.push(id),
            b"BDC" => {
                let own = match op.operands.get(1) {
                    Some(Object::Dict(d)) => d.get(b"MCID").and_then(Object::as_int),
                    Some(Object::Name(n)) => {
                        self.props.get(n).map(|x| self.doc.resolve(x)).and_then(|x| x.as_dict().and_then(|d| d.get(b"MCID").and_then(Object::as_int)))
                    }
                    _ => None,
                };
                self.stack.push(own.or(id));
            }
            b"EMC" => {
                self.stack.pop();
            }
            b"q" if self.saved.len() < MAX_SAVED => self.saved.push((gs.clone(), self.tm, self.tlm)),
            b"q" => self.unsaved += 1,
            b"Q" if self.unsaved > 0 => self.unsaved -= 1,
            b"Q" => {
                if let Some(saved) = self.saved.pop() {
                    (self.gs, self.tm, self.tlm) = saved;
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
                if let Some(name) = op.name(0) {
                    self.load_font(name);
                }
            }
            b"Tc" => gs.char_spacing = op.num(0).unwrap_or(0.0),
            b"Tw" => gs.word_spacing = op.num(0).unwrap_or(0.0),
            b"Tz" => gs.scale = op.num(0).unwrap_or(100.0) / 100.0,
            b"TL" => gs.leading = op.num(0).unwrap_or(0.0),
            b"BT" => (self.tm, self.tlm) = (Matrix::IDENTITY, Matrix::IDENTITY),
            b"Td" | b"TD" => {
                if let Some([x, y]) = op.nums::<2>() {
                    if op.op == b"TD" {
                        gs.leading = -y;
                    }
                    self.tlm = Matrix::translate(x, y).then(&self.tlm);
                    self.tm = self.tlm;
                }
            }
            b"Tm" => {
                if let Some(m) = op.nums::<6>() {
                    (self.tm, self.tlm) = (Matrix(m), Matrix(m));
                }
            }
            b"T*" => {
                self.tlm = Matrix::translate(0.0, -gs.leading).then(&self.tlm);
                self.tm = self.tlm;
            }
            _ => {}
        }
    }

    /// Reads the metrics of font resource `name`, once.
    fn load_font(&mut self, name: &[u8]) {
        if self.metrics.contains_key(name) {
            return;
        }
        // Keyed by the object at the end of the reference chain, so names that reach one font
        // through different chains share its metrics (the hop limit is resolve's).
        let font = self.fonts.get(name).and_then(|o| {
            let (mut cur, mut last) = (o.clone(), None);
            for _ in 0..16 {
                let Object::Ref(r) = cur else { break };
                last = Some(r);
                cur = (*self.doc.get(r)).clone();
            }
            last
        });
        let built = match font.and_then(|r| self.by_font.get(&r)) {
            Some(m) => Some(m.clone()),
            None => self.fonts.get(name).map(|f| self.doc.resolve(f)).and_then(|f| f.as_dict().map(|d| Rc::new(Metrics::from_dict(self.doc, d)))),
        };
        if let Some(m) = built {
            if let Some(r) = font {
                self.by_font.insert(r, m.clone());
            }
            self.metrics.insert(name.to_vec(), m);
        }
    }

    /// Shows the strings of a text-showing operator, glyph by glyph.
    fn show(&mut self, op: &Op, f: &mut impl FnMut(&Glyph<'_>)) {
        let Some(m) = self.gs.font.as_ref().and_then(|font| self.metrics.get(font)).cloned() else { return };
        let gs = &mut self.gs;
        if op.op != b"Tj" && op.op != b"TJ" {
            if op.op == b"\"" {
                gs.word_spacing = op.num(0).unwrap_or(gs.word_spacing);
                gs.char_spacing = op.num(1).unwrap_or(gs.char_spacing);
            }
            self.tlm = Matrix::translate(0.0, -gs.leading).then(&self.tlm);
            self.tm = self.tlm;
        }
        let id = self.stack.last().copied().flatten();
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
                    self.tm = Matrix::translate(dx, 0.0).then(&self.tm);
                    continue;
                }
            };
            for (code, len) in m.codes(&s.bytes) {
                let advance = m.width(code) * gs.size + gs.char_spacing + if m.is_space(code, len) { gs.word_spacing } else { 0.0 };
                let glyph = Glyph { id, trm: self.tm.then(&gs.ctm), metrics: &m, code, size: gs.size, scale: gs.scale, advance };
                f(&glyph);
                self.tm = Matrix::translate(advance * gs.scale, 0.0).then(&self.tm);
            }
        }
    }
}
