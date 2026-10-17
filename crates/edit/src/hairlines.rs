//! Fix hairlines (Acrobat's Print production ▸ Fix Hairlines; Fix hairlines): strokes thinner
//! than a printer holds - zero-width lines, and lines whose width on the page, after every
//! transformation, is below a threshold - are given a chosen width. Page content and the Form
//! XObjects it draws are rewritten; Type 3 glyphs and tiling patterns on request.

use pdfcraft_content::{Op, parse, serialize_ops};
use pdfcraft_cos::{Dict, Document, ObjRef, Object, Stream};
use std::collections::HashSet;

use crate::{EditError, check, n, page_list};

/// The stream `r` holds, if it is one.
fn stream_of(doc: &Document, r: ObjRef) -> Option<Stream> {
    match &*doc.get(r) {
        Object::Stream(s) => Some(s.clone()),
        _ => None,
    }
}

/// What counts as a hairline and what it becomes, in points on the page.
#[derive(Clone, Debug, PartialEq)]
pub struct Hairlines {
    /// Lines narrower than this (points, after the page's transformations) are replaced;
    /// zero-width lines always are. Acrobat's default is 0.003 in.
    pub narrower_than: f64,
    /// The width they are given (points on the page). Acrobat's default is 0.01 in.
    pub width: f64,
    /// Also the strokes inside Type 3 font glyphs.
    pub type3: bool,
    /// Also the strokes inside tiling patterns.
    pub patterns: bool,
}

impl Default for Hairlines {
    fn default() -> Self {
        Self { narrower_than: 0.003 * 72.0, width: 0.01 * 72.0, type3: false, patterns: false }
    }
}

/// What a run changed.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Fixed {
    /// Strokes given a new width.
    pub lines: usize,
    /// Pages on which something changed.
    pub pages: usize,
}

/// The uniform scale a matrix applies to lengths (the geometric mean of its two axes).
fn scale_of(m: [f64; 6]) -> f64 {
    (m[0] * m[3] - m[1] * m[2]).abs().sqrt()
}

fn mul(a: [f64; 6], b: [f64; 6]) -> [f64; 6] {
    // a then b.
    [
        a[0] * b[0] + a[1] * b[2],
        a[0] * b[1] + a[1] * b[3],
        a[2] * b[0] + a[3] * b[2],
        a[2] * b[1] + a[3] * b[3],
        a[4] * b[0] + a[5] * b[2] + b[4],
        a[4] * b[1] + a[5] * b[3] + b[5],
    ]
}

/// Rewrite one content stream drawn under `base` (the CTM when it starts, scale only matters).
/// `forms` collects the Form XObjects it draws, with the CTM each is drawn under, for the
/// caller to rewrite in turn. Returns the new bytes when anything changed, and the count.
fn rewrite(
    data: &[u8],
    base: [f64; 6],
    h: &Hairlines,
    resources: Option<&Dict>,
    doc: &Document,
    forms: &mut Vec<(ObjRef, [f64; 6])>,
) -> (Option<Vec<u8>>, usize) {
    let parsed = parse(data);
    let mut ops: Vec<Op> = parsed.ops;
    let mut fixed = 0usize;
    let mut changed = false;
    // The graphics state: CTM and line width, with the q/Q stack.
    let mut ctm = base;
    let mut width = 1.0f64;
    let mut stack: Vec<([f64; 6], f64)> = Vec::new();
    // The `w` that set the current width, to rewrite it in place when a stroke needs it.
    let mut width_op: Option<usize> = None;
    let mut out: Vec<Op> = Vec::with_capacity(ops.len() + 8);
    let xobjects = resources.and_then(|r| r.get(b"XObject")).map(|x| doc.resolve(x)).and_then(|x| x.as_dict().cloned());
    for op in ops.drain(..) {
        match op.op.as_slice() {
            b"q" => {
                stack.push((ctm, width));
                if stack.len() > 512 {
                    stack.remove(0);
                }
            }
            b"Q" => {
                if let Some((c, w)) = stack.pop() {
                    ctm = c;
                    width = w;
                }
                width_op = None;
            }
            b"cm" => {
                if let Some(m) = op.nums::<6>() {
                    ctm = mul(m, ctm);
                }
            }
            b"w" => {
                if let Some([w]) = op.nums::<1>() {
                    width = w;
                    width_op = Some(out.len());
                }
            }
            b"Do" => {
                if let Some(Object::Name(name)) = op.operands.first()
                    && let Some(x) = xobjects.as_ref().and_then(|x| x.get(name))
                    && let Some(r) = x.as_ref()
                    && stream_of(doc, r).is_some_and(|s| s.dict.name(b"Subtype") == Some(b"Form"))
                {
                    forms.push((r, ctm));
                }
            }
            b"S" | b"s" | b"B" | b"B*" | b"b" | b"b*" => {
                let scale = scale_of(ctm);
                let on_page = width * scale;
                let thin = width <= 0.0 || (scale > 0.0 && on_page < h.narrower_than);
                if thin && scale > 0.0 {
                    let new_w = h.width / scale;
                    match width_op {
                        // The `w` that set this width is rewritten, so every stroke under it
                        // changes with one operator.
                        Some(i) if out.get(i).is_some_and(|o| o.is("w")) => {
                            out[i] = Op::new("w", vec![Object::Real(new_w)]);
                        }
                        _ => {
                            out.push(Op::new("w", vec![Object::Real(new_w)]));
                            width_op = Some(out.len() - 1);
                        }
                    }
                    width = new_w;
                    fixed += 1;
                    changed = true;
                }
            }
            _ => {}
        }
        out.push(op);
    }
    (changed.then(|| serialize_ops(&out)), fixed)
}

/// Fix the hairlines on `pages` (0-based). Each page's content, and each Form XObject it draws
/// (once, under the transformation of its first use), is rewritten where a stroke is thinner
/// than `h.narrower_than` points on the page.
pub fn fix_hairlines(doc: &mut Document, pages: &[usize], h: &Hairlines) -> Result<Fixed, EditError> {
    let all = page_list(doc);
    check(pages, all.len())?;
    if !(h.narrower_than.is_finite() && h.narrower_than >= 0.0 && h.width.is_finite() && h.width > 0.0) {
        return Err(EditError::Invalid("the hairline widths must be positive numbers of points".into()));
    }
    let mut result = Fixed::default();
    let mut done_forms: HashSet<ObjRef> = HashSet::new();
    for &i in pages {
        let page = page_list(doc)[i].clone();
        let resources = page.dict.get(b"Resources").map(|r| doc.resolve(r)).and_then(|r| r.as_dict().cloned());
        let mut forms: Vec<(ObjRef, [f64; 6])> = Vec::new();
        let mut page_fixed = 0usize;
        // The page's streams, each rewritten on its own (state carries across them).
        let entries: Vec<Object> = match page.dict.get(b"Contents").map(|c| doc.resolve(c)) {
            Some(c) => match &*c {
                Object::Array(a) => a.clone(),
                _ => page.dict.get(b"Contents").cloned().into_iter().collect(),
            },
            None => Vec::new(),
        };
        let joined: Vec<u8> = entries
            .iter()
            .filter_map(|e| e.as_ref())
            .filter_map(|r| stream_of(doc, r).and_then(|s| s.decoded().ok()))
            .flat_map(|d| {
                let mut v = d;
                v.push(b'\n');
                v
            })
            .collect();
        let (new, fixed) = rewrite(&joined, [1.0, 0.0, 0.0, 1.0, 0.0, 0.0], h, resources.as_ref(), doc, &mut forms);
        if let Some(bytes) = new {
            let stream = doc.add(Object::Stream(Stream::flate(Dict::new(), &bytes)));
            doc.update_dict(page.obj, |d| d.set(b"Contents".to_vec(), Object::Ref(stream)))?;
            page_fixed += fixed;
        }
        // The forms the page draws, and the forms they draw.
        while let Some((r, ctm)) = forms.pop() {
            if !done_forms.insert(r) {
                continue;
            }
            let Some(s) = stream_of(doc, r) else { continue };
            let Ok(data) = s.decoded() else { continue };
            let matrix = s.dict.get(b"Matrix").map(|m| doc.resolve(m)).and_then(|m| m.as_array().cloned()).and_then(|a| {
                let v: Vec<f64> = a.iter().filter_map(|x| doc.resolve(x).as_f64()).collect();
                <[f64; 6]>::try_from(v).ok()
            });
            let base = match matrix {
                Some(m) => mul(m, ctm),
                None => ctm,
            };
            let res = s.dict.get(b"Resources").map(|x| doc.resolve(x)).and_then(|x| x.as_dict().cloned());
            let mut inner: Vec<(ObjRef, [f64; 6])> = Vec::new();
            let (new, fixed) = rewrite(&data, base, h, res.as_ref(), doc, &mut inner);
            forms.extend(inner);
            if let Some(bytes) = new {
                let mut dict = s.dict.clone();
                dict.remove(b"Length");
                dict.remove(b"Filter");
                dict.remove(b"DecodeParms");
                doc.set(r, Object::Stream(Stream::flate(dict, &bytes)));
                page_fixed += fixed;
            }
        }
        if h.type3 || h.patterns {
            page_fixed += fix_in_resources(doc, resources.as_ref(), h, &mut done_forms)?;
        }
        if page_fixed > 0 {
            result.lines += page_fixed;
            result.pages += 1;
        }
    }
    Ok(result)
}

/// Strokes inside Type 3 glyph procedures and tiling patterns of `resources`, on request. Their
/// drawing scale is the font matrix or the pattern matrix (the text size and placement are
/// not known here, so the glyph's own scale stands in).
fn fix_in_resources(doc: &mut Document, resources: Option<&Dict>, h: &Hairlines, done: &mut HashSet<ObjRef>) -> Result<usize, EditError> {
    let Some(res) = resources else { return Ok(0) };
    let mut fixed = 0usize;
    let mut targets: Vec<(ObjRef, [f64; 6])> = Vec::new();
    if h.type3
        && let Some(fonts) = res.get(b"Font").map(|f| doc.resolve(f)).and_then(|f| f.as_dict().cloned())
    {
        for (_, f) in fonts.iter() {
            let fd = doc.resolve(f);
            let Some(fd) = fd.as_dict() else { continue };
            if fd.name(b"Subtype") != Some(b"Type3") {
                continue;
            }
            let fm = fd.get(b"FontMatrix").map(|m| doc.resolve(m)).and_then(|m| m.as_array().cloned()).and_then(|a| {
                let v: Vec<f64> = a.iter().filter_map(|x| doc.resolve(x).as_f64()).collect();
                <[f64; 6]>::try_from(v).ok()
            });
            // Glyph space → text space at a nominal 10 pt.
            let base = fm.map(|m| mul(m, [10.0, 0.0, 0.0, 10.0, 0.0, 0.0])).unwrap_or([0.01, 0.0, 0.0, 0.01, 0.0, 0.0]);
            if let Some(procs) = fd.get(b"CharProcs").map(|p| doc.resolve(p)).and_then(|p| p.as_dict().cloned()) {
                for (_, g) in procs.iter() {
                    if let Some(r) = g.as_ref() {
                        targets.push((r, base));
                    }
                }
            }
        }
    }
    if h.patterns
        && let Some(pats) = res.get(b"Pattern").map(|p| doc.resolve(p)).and_then(|p| p.as_dict().cloned())
    {
        for (_, p) in pats.iter() {
            let Some(r) = p.as_ref() else { continue };
            let Some(s) = stream_of(doc, r) else { continue };
            if s.dict.int(b"PatternType") != Some(1) {
                continue;
            }
            let pm = s.dict.get(b"Matrix").map(|m| doc.resolve(m)).and_then(|m| m.as_array().cloned()).and_then(|a| {
                let v: Vec<f64> = a.iter().filter_map(|x| doc.resolve(x).as_f64()).collect();
                <[f64; 6]>::try_from(v).ok()
            });
            targets.push((r, pm.unwrap_or([1.0, 0.0, 0.0, 1.0, 0.0, 0.0])));
        }
    }
    for (r, base) in targets {
        if !done.insert(r) {
            continue;
        }
        let Some(s) = stream_of(doc, r) else { continue };
        let Ok(data) = s.decoded() else { continue };
        let res = s.dict.get(b"Resources").map(|x| doc.resolve(x)).and_then(|x| x.as_dict().cloned());
        let mut inner = Vec::new();
        let (new, n_fixed) = rewrite(&data, base, h, res.as_ref(), doc, &mut inner);
        if let Some(bytes) = new {
            let mut dict = s.dict.clone();
            dict.remove(b"Length");
            dict.remove(b"Filter");
            dict.remove(b"DecodeParms");
            doc.set(r, Object::Stream(Stream::flate(dict, &bytes)));
            fixed += n_fixed;
        }
    }
    let _ = n;
    Ok(fixed)
}
