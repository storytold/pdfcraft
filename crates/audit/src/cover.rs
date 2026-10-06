//! Walk page content streams and collect the opaque filled rectangles they draw.
//!
//! This is the geometric half of the faux-redaction audit: a rectangle filled with an opaque
//! color (`re` + `f`) is the classic way a "redaction" is faked. Graphics state tracked here is
//! deliberately small: the CTM stack (`q`/`Q`/`cm`), the non-stroking color (`g`/`rg`/`k`) and
//! the non-stroking alpha from ExtGState (`gs` reading `/ca`). Form XObjects are followed
//! read-only with a depth cap. Anything fancier (arbitrary filled paths, shadings, patterns) is
//! out of scope and documented as such.

use printcraft_content::{Matrix, parse};
use printcraft_cos::{Dict, Document, Object};

/// Maximum form-XObject nesting followed (cyclic references are cut by the cap).
const MAX_DEPTH: usize = 8;
/// Bound on total operators walked per page, so a hostile stream cannot spin forever.
const MAX_OPS: usize = 2_000_000;

/// One opaque filled rectangle, in page user space.
#[derive(Clone, Debug, PartialEq)]
pub struct FilledRect {
    /// `[x0, y0, x1, y1]`, normalized so `x0 <= x1` and `y0 <= y1`.
    pub rect: [f64; 4],
    /// The fill color as sRGB-ish `[r, g, b]` (CMYK is converted approximately).
    pub rgb: [f64; 3],
}

#[derive(Clone)]
struct Gs {
    ctm: Matrix,
    fill: [f64; 3],
    alpha: f64,
}

impl Default for Gs {
    fn default() -> Self {
        Gs { ctm: Matrix::IDENTITY, fill: [0.0, 0.0, 0.0], alpha: 1.0 }
    }
}

struct Walker<'a> {
    doc: &'a Document,
    out: Vec<FilledRect>,
    ops_seen: usize,
}

impl<'a> Walker<'a> {
    fn walk_stream(&mut self, data: &[u8], base_ctm: &Matrix, resources: &Dict, depth: usize) {
        if depth > MAX_DEPTH {
            return;
        }
        let parsed = parse(data);
        let mut stack: Vec<Gs> = Vec::new();
        let mut gs = Gs { ctm: *base_ctm, ..Gs::default() };
        // A pending `re` rectangle, in the user space active when it was built.
        let mut pending: Option<[f64; 4]> = None;
        for op in &parsed.ops {
            self.ops_seen += 1;
            if self.ops_seen > MAX_OPS {
                return;
            }
            match op.op.as_slice() {
                b"q" => stack.push(gs.clone()),
                b"Q" => {
                    if let Some(g) = stack.pop() {
                        gs = g;
                    }
                    pending = None;
                }
                b"cm" => {
                    if let Some(m) = Matrix::from_operands(&op.operands)
                        && m.0.iter().all(|v| v.is_finite())
                    {
                        gs.ctm = m.then(&gs.ctm);
                    }
                    pending = None;
                }
                b"g" => {
                    if let Some(v) = op.num(0).filter(|v| v.is_finite()) {
                        let v = v.clamp(0.0, 1.0);
                        gs.fill = [v, v, v];
                    }
                }
                b"rg" => {
                    if let Some([r, g, b]) = op.nums::<3>()
                        && [r, g, b].iter().all(|v| v.is_finite())
                    {
                        gs.fill = [r.clamp(0.0, 1.0), g.clamp(0.0, 1.0), b.clamp(0.0, 1.0)];
                    }
                }
                b"k" => {
                    if let Some([c, m, y, k]) = op.nums::<4>()
                        && [c, m, y, k].iter().all(|v| v.is_finite())
                    {
                        // Naive CMYK -> RGB; good enough to tell black from white.
                        let (c, m, y, k) = (c.clamp(0.0, 1.0), m.clamp(0.0, 1.0), y.clamp(0.0, 1.0), k.clamp(0.0, 1.0));
                        gs.fill = [(1.0 - c) * (1.0 - k), (1.0 - m) * (1.0 - k), (1.0 - y) * (1.0 - k)];
                    }
                }
                b"gs" => {
                    if let Some(name) = op.name(0) {
                        gs.alpha = ext_alpha(self.doc, resources, name);
                    }
                }
                b"re" => {
                    pending = op.nums::<4>().and_then(|[x, y, w, h]| {
                        if [x, y, w, h].iter().all(|v| v.is_finite()) && w != 0.0 && h != 0.0 {
                            let r = gs.ctm.bbox([x.min(x + w), y.min(y + h), x.max(x + w), y.max(y + h)]);
                            (r[2] > r[0] && r[3] > r[1]).then_some(r)
                        } else {
                            None
                        }
                    });
                }
                b"f" | b"F" | b"f*" | b"B" | b"B*" => {
                    if let Some(r) = pending.take()
                        && gs.alpha >= 1.0
                    {
                        self.out.push(FilledRect { rect: r, rgb: gs.fill });
                    }
                    // Any other path construction clears the pending rectangle.
                }
                b"m" | b"l" | b"c" | b"v" | b"y" | b"h" | b"S" | b"s" | b"n" | b"W" | b"W*" => {
                    pending = None;
                }
                b"Do" => {
                    if let Some(name) = op.name(0) {
                        self.walk_xobject(name, &gs, resources, depth);
                    }
                    pending = None;
                }
                _ => {}
            }
        }
    }

    /// Follow a form XObject read-only, composing its matrix onto the current CTM.
    fn walk_xobject(&mut self, name: &[u8], gs: &Gs, resources: &Dict, depth: usize) {
        let xobjects = resources.get(b"XObject").map(|o| self.doc.resolve(o)).and_then(|o| o.as_dict().cloned()).unwrap_or_default();
        let Some(entry) = xobjects.get(name) else { return };
        let resolved = self.doc.resolve(entry);
        let Object::Stream(stream) = &*resolved else { return };
        let dict = &stream.dict;
        if dict.name(b"Subtype") != Some(b"Form") {
            return;
        }
        let matrix = dict
            .get(b"Matrix")
            .map(|o| self.doc.resolve(o))
            .and_then(|o| o.as_array().map(|a| a.to_vec()))
            .and_then(|a| {
                let v: Vec<f64> = a.iter().filter_map(|o| self.doc.resolve(o).as_f64()).collect();
                (v.len() == 6 && v.iter().all(|x| x.is_finite())).then(|| Matrix([v[0], v[1], v[2], v[3], v[4], v[5]]))
            })
            .unwrap_or(Matrix::IDENTITY);
        let sub_resources = dict.get(b"Resources").map(|o| self.doc.resolve(o)).and_then(|o| o.as_dict().cloned()).unwrap_or_default();
        let data = stream.decoded().unwrap_or_default();
        // The XObject matrix maps form space into the user space active at `Do`.
        self.walk_stream(&data, &matrix.then(&gs.ctm), &sub_resources, depth + 1);
    }
}

/// The non-stroking alpha (`/ca`) of a named ExtGState, defaulting to opaque.
fn ext_alpha(doc: &Document, resources: &Dict, name: &[u8]) -> f64 {
    let alpha = resources
        .get(b"ExtGState")
        .map(|o| doc.resolve(o))
        .and_then(|o| o.as_dict().cloned())
        .and_then(|d| d.get(name).map(|o| doc.resolve(o)))
        .and_then(|o| o.as_dict().cloned())
        .and_then(|d| d.get(b"ca").map(|o| doc.resolve(o)).and_then(|o| o.as_f64()));
    alpha.filter(|a| a.is_finite()).map(|a| a.clamp(0.0, 1.0)).unwrap_or(1.0)
}

/// Every opaque filled rectangle drawn by a page's content (and its form XObjects).
pub fn filled_rects(doc: &Document, page: &Dict) -> Vec<FilledRect> {
    let resources = page.get(b"Resources").map(|o| doc.resolve(o)).and_then(|o| o.as_dict().cloned()).unwrap_or_default();
    let contents: Vec<Object> = match page.get(b"Contents") {
        None => Vec::new(),
        Some(c) => match &*doc.resolve(c) {
            Object::Array(a) => a.clone(),
            _ => vec![c.clone()],
        },
    };
    let mut w = Walker { doc, out: Vec::new(), ops_seen: 0 };
    for c in &contents {
        if w.ops_seen > MAX_OPS {
            break;
        }
        if let Object::Stream(s) = &*doc.resolve(c) {
            let data = s.decoded().unwrap_or_default();
            w.walk_stream(&data, &Matrix::IDENTITY, &resources, 0);
        }
    }
    w.out
}
