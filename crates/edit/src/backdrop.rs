//! A page's own backdrop: an opaque white fill (or a picture) covering the whole visible page
//! before anything else is drawn. Many writers paint one (browsers' Save as PDF, print drivers;
//! scans are a page-sized picture), and it hides anything placed behind the content, so a
//! background added behind such a page never shows.
//!
//! A white fill can be replaced: the one fill operator becomes `n` (end the path without
//! painting), which looks the same on white paper. The page then gets a copy of the content
//! stream that held it, tagged `/PCBackdrop << /Original ref /At n /Len n >>`, and the stream
//! itself is left alone (other pages may share it). [`restore`] puts the original back when the
//! background is removed, unless the copy has been edited since. A picture can't be replaced
//! without losing it, so it is only reported.
//!
//! Detection is deliberately strict: only one rectangle (`re`), in DeviceGray, DeviceRGB or
//! DeviceCMYK, opaque in the graphics state, with an axis-aligned transform, filled before any
//! other painting, and covering the crop box. Anything else is real content and stays.

use pdfcraft_content::{Matrix, Op, Pieces};
use pdfcraft_cos::{Dict, Document, ObjRef, Object, Stream};

use crate::EditError;

/// How much of a page's content is read looking for a backdrop: it has to come first.
const SCAN_BYTES: usize = 64 * 1024;
/// How many operators are walked before giving up.
const SCAN_OPS: usize = 4096;
/// How far (in points) a backdrop may fall short of the crop box and still count as covering it.
const COVER_EPS: f64 = 1.0;
/// Components at least this close to white (per channel) count as white: a near-white fill
/// (`0.99 g`) is a backdrop too, and replacing it changes the page by at most 2%.
const WHITE: f64 = 0.98;

/// What paints a page's backdrop.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Backdrop {
    /// An opaque white fill: replaceable.
    Fill,
    /// A picture covering the page (a scan): a background behind it stays hidden.
    Image,
}

/// Where a replaceable fill is: the content streams (references in `/Contents` order, untagged
/// ones only), their joined bytes, and the fill operator's joined byte range.
struct Found {
    refs: Vec<(usize, ObjRef)>,
    pieces: Pieces,
    op: std::ops::Range<usize>,
}

#[derive(Clone)]
struct State {
    ctm: Matrix,
    /// Clip in default user space, when it is a known rectangle; `None` = unknown shape.
    clip: Option<[f64; 4]>,
    /// The fill colour is white.
    white: bool,
    /// Non-stroking colour space components (1, 3, 4), or 0 for a space we don't read.
    space: usize,
    opaque: bool,
}

/// The backdrop `page` (0-based) paints, if any. Finding the page walks the page tree, so to
/// check many pages use [`page_backdrops`], which walks it once.
pub fn page_backdrop(doc: &Document, page: usize) -> Result<Option<Backdrop>, EditError> {
    let all = pdfcraft_model::pages(doc);
    let p = all.get(page).ok_or(EditError::NoSuchPage(page))?;
    Ok(scan(doc, p).map(|(b, _)| b))
}

/// The backdrop each of `pages` (0-based) paints, in the same order, walking the page tree once.
/// A page past the end has none, and doesn't hide what the others paint: a page count taken
/// elsewhere (the viewer's) can disagree with the page tree's on a malformed document.
pub fn page_backdrops(doc: &Document, pages: &[usize]) -> Vec<Option<Backdrop>> {
    let all = pdfcraft_model::pages(doc);
    pages.iter().map(|&p| all.get(p).and_then(|page| scan(doc, page)).map(|(b, _)| b)).collect()
}

/// Replace `page`'s white backdrop fill, if it has one (see the module doc). Returns whether it
/// did; when it did, `page` no longer matches the document. A picture backdrop is left alone.
pub(crate) fn replace(doc: &mut Document, page: &pdfcraft_model::Page) -> Result<bool, EditError> {
    let Some((Backdrop::Fill, Some(found))) = scan(doc, page) else { return Ok(false) };
    let edited = found.pieces.splice([(found.op.clone(), b"n".to_vec())]);
    let piece = found.pieces.piece_of(found.op.start);
    let (Some(&(slot, original)), Some(bytes)) = (found.refs.get(piece), edited.get(piece)) else { return Ok(false) };
    // Where the operator sits in that piece: the joined offset less the pieces before it.
    let before: usize = (0..piece).filter_map(|i| edited.get(i)).map(|p| p.len() + 1).sum();
    let at = found.op.start.saturating_sub(before);
    let len = found.op.end.saturating_sub(found.op.start);
    let mut note = Dict::new();
    note.set(b"Original".to_vec(), Object::Ref(original));
    note.set(b"At".to_vec(), Object::Int(i64::try_from(at).unwrap_or(i64::MAX)));
    note.set(b"Len".to_vec(), Object::Int(i64::try_from(len).unwrap_or(i64::MAX)));
    let mut dict = Dict::new();
    dict.set(b"PCBackdrop".to_vec(), Object::Dict(note));
    let copy = doc.add(Object::Stream(Stream::flate(dict, bytes)));
    let mut list = crate::contents(doc, page)?;
    let Some(entry) = list.get_mut(slot) else { return Ok(false) };
    *entry = Object::Ref(copy);
    doc.update_dict(page.obj, |d| d.set(b"Contents".to_vec(), Object::Array(list)))?;
    Ok(true)
}

/// Put back the original content streams [`replace`] copied on `page`, where the copy is still
/// exactly what it wrote. Returns how many it restored.
pub(crate) fn restore(doc: &mut Document, page: &pdfcraft_model::Page) -> Result<usize, EditError> {
    let mut list = crate::contents(doc, page)?;
    let mut restored = 0;
    for entry in list.iter_mut() {
        if let Some(original) = unchanged_copy_of(doc, entry) {
            *entry = Object::Ref(original);
            restored += 1;
        }
    }
    if restored > 0 {
        doc.update_dict(page.obj, |d| d.set(b"Contents".to_vec(), Object::Array(list)))?;
    }
    Ok(restored)
}

/// The original of a backdrop copy whose bytes are still the original's with the fill turned to
/// `n`; `None` for anything else (including a copy edited since, which is kept).
fn unchanged_copy_of(doc: &Document, entry: &Object) -> Option<ObjRef> {
    let copy = doc.resolve(entry);
    let Object::Stream(copy) = &*copy else { return None };
    let note = copy.dict.get(b"PCBackdrop").map(|n| doc.resolve(n))?;
    let note = note.as_dict()?;
    let Some(Object::Ref(original)) = note.get(b"Original") else { return None };
    let at = usize::try_from(note.get(b"At")?.as_int()?).ok()?;
    let len = usize::try_from(note.get(b"Len")?.as_int()?).ok()?;
    let orig = doc.get(*original);
    let Object::Stream(orig) = &*orig else { return None };
    let source = orig.decoded().ok()?;
    let mut expect = source.get(..at)?.to_vec();
    expect.push(b'n');
    expect.extend_from_slice(source.get(at.checked_add(len)?..)?);
    (copy.decoded().ok()? == expect).then_some(*original)
}

/// Find the backdrop, and for a fill, where it is.
fn scan(doc: &Document, p: &pdfcraft_model::Page) -> Option<(Backdrop, Option<Found>)> {
    let crop = p.crop(doc);
    let c = p.dict.get(b"Contents")?;
    let list: Vec<Object> = match &*doc.resolve(c) {
        Object::Array(a) => a.clone(),
        _ => vec![c.clone()],
    };
    // The page's own content: untagged streams (marks and the q/Q wrapper carry /PCMark). Read
    // only as far as a backdrop could be.
    let mut refs = Vec::new();
    let mut data = Vec::new();
    let mut total = 0usize;
    for (slot, o) in list.iter().enumerate() {
        if total >= SCAN_BYTES {
            break;
        }
        let Object::Ref(r) = o else { return None };
        let obj = doc.get(*r);
        let Object::Stream(s) = &*obj else { continue };
        if s.dict.get(b"PCMark").is_some() {
            continue;
        }
        let bytes = s.decoded().unwrap_or_default();
        total = total.saturating_add(bytes.len()).saturating_add(1);
        refs.push((slot, *r));
        data.push(bytes);
    }
    let pieces = Pieces::join(&data);
    let head = pieces.data().get(..pieces.data().len().min(SCAN_BYTES)).unwrap_or_default();
    let parsed = pdfcraft_content::parse(head);
    let res = resources(doc, &p.dict);
    let found = walk(doc, &res, crop, &parsed.ops);
    found.map(|(b, op)| match (b, op) {
        (Backdrop::Fill, Some(op)) => (b, Some(Found { refs, pieces, op })),
        _ => (b, None),
    })
}

fn resources(doc: &Document, page: &Dict) -> Dict {
    page.get(b"Resources").map(|r| doc.resolve(r)).and_then(|r| r.as_dict().cloned()).unwrap_or_default()
}

fn sub(doc: &Document, res: &Dict, kind: &[u8], name: &[u8]) -> Option<std::sync::Arc<Object>> {
    let d = res.get(kind).map(|d| doc.resolve(d))?;
    let o = d.as_dict()?.get(name)?.clone();
    Some(doc.resolve(&o))
}

/// Walk the operators until the first one that paints; a backdrop is that one, or nothing.
fn walk(doc: &Document, res: &Dict, crop: [f64; 4], ops: &[Op]) -> Option<(Backdrop, Option<std::ops::Range<usize>>)> {
    let mut st = State { ctm: Matrix::IDENTITY, clip: Some([f64::MIN, f64::MIN, f64::MAX, f64::MAX]), white: false, space: 1, opaque: true };
    let mut saved: Vec<State> = Vec::new();
    // Current path: its rectangles in user space, or `None` once it is anything else.
    let mut path: Option<Vec<[f64; 4]>> = Some(Vec::new());
    let mut clip_next = false;
    for op in ops.iter().take(SCAN_OPS) {
        match op.op.as_slice() {
            b"q" => {
                if saved.len() >= 64 {
                    return None;
                }
                saved.push(st.clone());
            }
            b"Q" => st = saved.pop()?,
            b"cm" => {
                let m = Matrix::from_operands(op.operands.get(op.operands.len().checked_sub(6)?..)?)?;
                if !m.0.iter().all(|v| v.is_finite()) {
                    return None;
                }
                st.ctm = m.then(&st.ctm);
            }
            b"gs" => st.opaque = opaque_state(doc, res, op.name(0)?)?,
            b"g" => set_colour(&mut st, 1, op),
            b"rg" => set_colour(&mut st, 3, op),
            b"k" => set_colour(&mut st, 4, op),
            b"cs" => {
                st.space = match op.name(0)? {
                    b"DeviceGray" => 1,
                    b"DeviceRGB" => 3,
                    b"DeviceCMYK" => 4,
                    _ => 0,
                };
                // The initial colour of these spaces is black.
                st.white = false;
            }
            b"sc" | b"scn" => {
                let n = st.space;
                set_colour(&mut st, n, op)
            }
            b"re" => {
                let [x, y, w, h] = op.nums::<4>()?;
                let finite = [x, y, w, h].iter().all(|v| v.is_finite());
                match (&mut path, finite && axis_aligned(&st.ctm)) {
                    (Some(rects), true) => rects.push(st.ctm.bbox([x.min(x + w), y.min(y + h), x.max(x + w), y.max(y + h)])),
                    _ => path = None,
                }
            }
            b"m" | b"l" | b"c" | b"v" | b"y" | b"h" => path = None,
            b"W" | b"W*" => clip_next = true,
            b"n" => {
                if clip_next {
                    st.clip = clipped(st.clip, path.as_deref());
                }
                path = Some(Vec::new());
                clip_next = false;
            }
            b"f" | b"F" | b"f*" => {
                // One rectangle: with two, a fill rule can cut a hole where content shows.
                let covers = match path.as_deref() {
                    Some([r]) => st.clip.and_then(|c| intersect(*r, c)).is_some_and(|r| pdfcraft_content::contains(r, crop, COVER_EPS)),
                    _ => false,
                };
                return (covers && st.white && st.opaque && !clip_next).then(|| (Backdrop::Fill, Some(op.span.clone())));
            }
            b"Do" => {
                let xo = sub(doc, res, b"XObject", op.name(0)?)?;
                let Object::Stream(s) = &*xo else { return None };
                // A stencil or masked picture lets what is behind it show.
                let masked = [&b"SMask"[..], b"Mask"].iter().any(|k| s.dict.get(k).is_some())
                    || s.dict.get(b"ImageMask").is_some_and(|m| matches!(*doc.resolve(m), Object::Bool(true)));
                let image = s.dict.name(b"Subtype") == Some(b"Image") && !masked;
                return covers_unit_square(&st, crop, image).then_some((Backdrop::Image, None));
            }
            b"BI" => {
                let masked = op
                    .inline
                    .as_ref()
                    .is_some_and(|(d, _)| [&b"IM"[..], b"ImageMask"].iter().any(|k| d.get(k).is_some_and(|m| matches!(m, Object::Bool(true)))));
                return covers_unit_square(&st, crop, !masked).then_some((Backdrop::Image, None));
            }
            // Anything else that paints comes first: no backdrop.
            b"S" | b"s" | b"B" | b"B*" | b"b" | b"b*" | b"sh" | b"Tj" | b"TJ" | b"'" | b"\"" | b"EI" | b"d0" | b"d1" => return None,
            _ => {}
        }
    }
    None
}

/// A picture placed by the CTM (onto the unit square) covering the crop box, opaque, unclipped.
fn covers_unit_square(st: &State, crop: [f64; 4], image: bool) -> bool {
    image
        && st.opaque
        && axis_aligned(&st.ctm)
        && st.clip.and_then(|c| intersect(st.ctm.bbox([0.0, 0.0, 1.0, 1.0]), c)).is_some_and(|r| pdfcraft_content::contains(r, crop, COVER_EPS))
}

fn set_colour(st: &mut State, components: usize, op: &Op) {
    st.space = components;
    let vals: Vec<f64> = op.operands.iter().filter_map(Object::as_f64).collect();
    st.white = vals.len() == components
        && match components {
            1 | 3 => vals.iter().all(|v| *v >= WHITE),
            4 => vals.iter().all(|v| *v <= 1.0 - WHITE),
            _ => false,
        };
}

/// Does the named ExtGState keep painting opaque and normal? `None` when it can't be read.
fn opaque_state(doc: &Document, res: &Dict, name: &[u8]) -> Option<bool> {
    let gs = sub(doc, res, b"ExtGState", name)?;
    let gs = gs.as_dict()?;
    let alpha = gs.get(b"ca").map(|a| doc.resolve(a).as_f64()).unwrap_or(Some(1.0))?;
    let soft = gs.get(b"SMask").is_some_and(|m| doc.resolve(m).as_name() != Some(b"None"));
    let blend = gs.get(b"BM").is_some_and(|b| !matches!(doc.resolve(b).as_name(), Some(b"Normal" | b"Compatible")));
    Some(alpha >= 0.999 && !soft && !blend)
}

/// Rotations by multiples of 90° and scales only: rectangles stay rectangles.
fn axis_aligned(m: &Matrix) -> bool {
    let [a, b, c, d, ..] = m.0;
    (b.abs() < 1e-9 && c.abs() < 1e-9) || (a.abs() < 1e-9 && d.abs() < 1e-9)
}

fn intersect(a: [f64; 4], b: [f64; 4]) -> Option<[f64; 4]> {
    let r = [a[0].max(b[0]), a[1].max(b[1]), a[2].min(b[2]), a[3].min(b[3])];
    (r.iter().all(|v| v.is_finite()) && r[2] > r[0] && r[3] > r[1]).then_some(r)
}

/// The clip after intersecting with a path: known only when the path is one rectangle.
fn clipped(clip: Option<[f64; 4]>, path: Option<&[[f64; 4]]>) -> Option<[f64; 4]> {
    match path {
        Some([r]) => intersect(clip?, *r).or(Some([0.0, 0.0, 0.0, 0.0])),
        _ => None,
    }
}

#[cfg(test)]
#[path = "backdrop_tests.rs"]
mod tests;
