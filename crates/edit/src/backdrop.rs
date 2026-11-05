//! A page's own backdrop: an opaque white fill (or a picture) covering the whole visible page
//! before anything else is drawn. Many writers paint one (browsers' Save as PDF, print drivers;
//! scans are a page-sized picture), and it hides anything placed behind the content, so a
//! background added behind such a page never shows.
//!
//! A white fill can be replaced: the one fill operator becomes `n` (end the path without
//! painting), which looks the same on white paper. The page then gets a copy of the content
//! stream that held it, tagged `/PCBackdrop << /Index n /Op (f) /Before <SHA-256> /Was [num gen] >>`:
//! how many of the copy's operators come before the `n`, the operator it stands in for, a hash of
//! those operators (their values, not their spacing: redaction writes a stream out again with
//! other whitespace), and the number of the stream it was copied from. The stream itself is left
//! alone (other pages may share it). [`restore`] turns the fill back on when the background is
//! removed, even after the page was edited or redacted, as long as the operators before the fill
//! are unchanged and the fill would again be the first thing painted, covering the page.
//! Otherwise the fill stays off, and [`backdrops_left_off`] names those pages. A picture can't be
//! replaced without losing it, so it is only reported.
//!
//! The tag holds no page content and no reference that keeps another object alive (`/Was` is two
//! numbers, not a reference): a reference to the original stream would keep text in the file
//! that a redaction, Remove Hidden Information or Edit Text removed from the copy.
//!
//! Detection is deliberately strict: only one rectangle (`re`), in DeviceGray, DeviceRGB or
//! DeviceCMYK, opaque, unmasked, normally blended and with the identity transfer function as the
//! graphics state stands after every `gs` so far (each sets only its own keys), with an
//! axis-aligned transform, filled before any other painting, and covering the crop box.
//! Anything else is real content and stays.

use pdfcraft_content::{Matrix, Op, Pieces};
use pdfcraft_cos::{DecodeBudget, Dict, Document, ObjRef, Object, PdfString, Stream};
use sha2::{Digest, Sha256};

use crate::{EditError, MarkKind};

/// The dictionary key of a backdrop copy's tag.
const TAG: &[u8] = b"PCBackdrop";
/// The fill operators a backdrop can be painted with, and so the only ones a tag may restore.
const FILLS: [&[u8]; 3] = [b"f", b"F", b"f*"];

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
/// ones only), their joined bytes, where each starts in them, and the fill operator's joined
/// byte range.
struct Found {
    refs: Vec<(usize, ObjRef)>,
    pieces: Pieces,
    starts: Vec<usize>,
    op: std::ops::Range<usize>,
}

impl Found {
    /// The piece holding the fill operator, and the operator's offset in it.
    fn place(&self) -> Option<(usize, usize)> {
        let piece = self.pieces.piece_of(self.op.start);
        Some((piece, self.op.start.checked_sub(*self.starts.get(piece)?)?))
    }
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
    paint: Paint,
}

/// The graphics-state parameters (ExtGState, ISO 32000-2 §8.4.5) that decide whether a fill
/// paints its colour as given. A `gs` sets only the keys its dictionary has, so each is kept on
/// its own: a later dictionary that leaves one out keeps the earlier value.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Paint {
    /// Non-stroking alpha (`ca`) is 1.
    full_alpha: bool,
    /// No soft mask (`SMask` is `/None`).
    no_soft_mask: bool,
    /// The blend mode (`BM`) is Normal or Compatible.
    normal_blend: bool,
    /// The transfer function (`TR`, or `TR2`, which takes precedence) is the identity: a
    /// function could turn white into any colour.
    identity_transfer: bool,
}

impl Paint {
    const INITIAL: Paint = Paint { full_alpha: true, no_soft_mask: true, normal_blend: true, identity_transfer: true };

    /// A fill paints its colour exactly, opaque.
    fn plain(self) -> bool {
        self.full_alpha && self.no_soft_mask && self.normal_blend && self.identity_transfer
    }
}

/// The backdrop `page` (0-based) paints, if any. Finding the page walks the page tree, so to
/// check many pages use [`page_backdrops`], which walks it once.
pub fn page_backdrop(doc: &Document, page: usize) -> Result<Option<Backdrop>, EditError> {
    let all = pdfcraft_model::pages(doc);
    let p = all.get(page).ok_or(EditError::NoSuchPage(page))?;
    Ok(scan(doc, p, None).map(|(b, _)| b))
}

/// The backdrop each of `pages` (0-based) paints, in the same order, walking the page tree once.
/// A page past the end has none, and doesn't hide what the others paint: a page count taken
/// elsewhere (the viewer's) can disagree with the page tree's on a malformed document.
pub fn page_backdrops(doc: &Document, pages: &[usize]) -> Vec<Option<Backdrop>> {
    let all = pdfcraft_model::pages(doc);
    pages.iter().map(|&p| all.get(p).and_then(|page| scan(doc, page, None)).map(|(b, _)| b)).collect()
}

/// Pages (0-based, ascending) whose own white backdrop [`replace`] turned off and [`restore`]
/// couldn't turn back on: a tagged copy is still in their content, with no background over it.
/// They look the same on white paper.
pub fn backdrops_left_off(doc: &Document) -> Vec<usize> {
    let tag_of = |o: &Object| crate::place::stream_tag(doc, o);
    let copy = |o: &Object| doc.resolve(o).as_dict().is_some_and(|d| d.get(TAG).is_some());
    pdfcraft_model::pages(doc)
        .iter()
        .enumerate()
        .filter(|(_, p)| {
            let list: Vec<Object> = match p.dict.get(b"Contents").map(|c| doc.resolve(c)).as_deref() {
                Some(Object::Array(a)) => a.clone(),
                Some(_) => p.dict.get(b"Contents").cloned().into_iter().collect(),
                None => Vec::new(),
            };
            list.iter().any(copy) && !list.iter().any(|o| tag_of(o).as_deref() == Some(MarkKind::Background.tag()))
        })
        .map(|(i, _)| i)
        .collect()
}

/// Replace `page`'s white backdrop fill, if it has one (see the module doc). Returns whether it
/// did; when it did, `page` no longer matches the document. A picture backdrop is left alone.
pub(crate) fn replace(doc: &mut Document, page: &pdfcraft_model::Page) -> Result<bool, EditError> {
    let Some((Backdrop::Fill, Some(found))) = scan(doc, page, None) else { return Ok(false) };
    let Some((piece, at)) = found.place() else { return Ok(false) };
    let op = found.pieces.data().get(found.op.clone()).unwrap_or_default().to_vec();
    let edited = found.pieces.splice([(found.op.clone(), b"n".to_vec())]);
    let (Some(&(_, original)), Some(bytes)) = (found.refs.get(piece), edited.get(piece)) else { return Ok(false) };
    // The `n` among the copy's own operators. A copy whose fill can't be found that way again is
    // not made: there would be no safe way back.
    let parsed = pdfcraft_content::parse(head(bytes));
    let Some(index) = parsed.ops.iter().position(|o| o.span.start == at && o.op == b"n") else { return Ok(false) };
    let Some(before) = parsed.ops.get(..index) else { return Ok(false) };
    let mut note = Dict::new();
    note.set(b"Index".to_vec(), Object::Int(i64::try_from(index).unwrap_or(i64::MAX)));
    note.set(b"Op".to_vec(), Object::String(PdfString::literal(op)));
    note.set(b"Before".to_vec(), Object::String(PdfString { bytes: ops_hash(before), hex: true }));
    note.set(b"Was".to_vec(), Object::Array(vec![Object::Int(original.num.into()), Object::Int(original.generation.into())]));
    // The copy keeps the original's dictionary (unknown entries included) less what describes
    // the old encoding; `scan` refused streams whose bytes live in an external file (`/F`).
    let Object::Stream(source) = &*doc.get(original) else { return Ok(false) };
    let mut dict = source.dict.clone();
    for key in [&b"Length"[..], b"DL", TAG] {
        dict.remove(key);
    }
    dict.set(TAG.to_vec(), Object::Dict(note));
    let copy = doc.add(Object::Stream(Stream::flate(dict, bytes)));
    // scan accepts only one execution of this stream on the page. Other pages keep the
    // original stream; ambiguous repeated executions are never rewritten.
    let mut list = crate::place::contents(doc, page)?;
    let mut replaced = false;
    for entry in list.iter_mut().filter(|o| matches!(o, Object::Ref(r) if *r == original)) {
        *entry = Object::Ref(copy);
        replaced = true;
    }
    if replaced {
        doc.update_dict(page.obj, |d| d.set(b"Contents".to_vec(), Object::Array(list)))?;
    }
    Ok(replaced)
}

/// Turn the fills [`replace`] turned off on `page` back on, where [`restored_copy`] finds that
/// safe; a copy listed in several slots is restored in each. Returns how many slots it restored.
/// A copy it can't restore stays as it is, tag and all, so [`backdrops_left_off`] names its page.
pub(crate) fn restore(doc: &mut Document, page: &pdfcraft_model::Page) -> Result<usize, EditError> {
    let mut list = crate::place::contents(doc, page)?;
    // Per copy: the stream that takes its place, or `None` when it stays.
    let mut done: Vec<(ObjRef, Option<ObjRef>)> = Vec::new();
    let mut restored = 0;
    for entry in list.iter_mut() {
        let Object::Ref(copy) = *entry else { continue };
        let with = match done.iter().find(|(c, _)| *c == copy) {
            Some(&(_, with)) => with,
            None => {
                let with = restored_copy(doc, page, copy).map(|(bytes, dict, was)| {
                    // The stream it was copied from, while that is still here with these bytes
                    // (other pages may share it): the page is then exactly as it was. `/Was` is
                    // only a number, and a full save renumbers objects, so the bytes decide.
                    let same = |w: &ObjRef| match &*doc.get(*w) {
                        Object::Stream(s) => {
                            [TAG, &b"PCMark"[..], b"Subtype"].iter().all(|k| s.dict.get(k).is_none())
                                && DecodeBudget::new(SCAN_BYTES, 0).decode_strict(s).ok().as_deref() == Some(bytes.as_slice())
                        }
                        _ => false,
                    };
                    match was.filter(same) {
                        Some(w) => w,
                        None => doc.add(Object::Stream(Stream::flate(dict, &bytes))),
                    }
                });
                done.push((copy, with));
                with
            }
        };
        if let Some(with) = with {
            *entry = Object::Ref(with);
            restored += 1;
        }
    }
    if restored > 0 {
        doc.update_dict(page.obj, |d| d.set(b"Contents".to_vec(), Object::Array(list)))?;
    }
    Ok(restored)
}

/// `copy` (a stream [`replace`] wrote for `page`) with its fill turned back on: the bytes, its
/// dictionary without the tag, and the stream it was copied from. `None` when it isn't such a
/// copy, or when turning the fill on could paint over something: the operators before the fill
/// changed since (their hash no longer matches), or the page would no longer paint the fill
/// first, covering it. What comes after the fill (an edit, a redaction) is kept as it is.
fn restored_copy(doc: &Document, page: &pdfcraft_model::Page, copy: ObjRef) -> Option<(Vec<u8>, Dict, Option<ObjRef>)> {
    let obj = doc.get(copy);
    let Object::Stream(s) = &*obj else { return None };
    let note = s.dict.get(TAG).map(|n| doc.resolve(n))?;
    let note = note.as_dict()?;
    let index = usize::try_from(note.get(b"Index")?.as_int()?).ok()?;
    let op = note.get(b"Op")?.as_string()?.bytes.as_slice();
    if !FILLS.contains(&op) {
        return None;
    }
    let hash = note.get(b"Before")?.as_string()?;
    let bytes = DecodeBudget::new(SCAN_BYTES, 0).decode_strict(s).ok()?;
    let parsed = pdfcraft_content::parse(head(&bytes));
    // The `n` standing in for the fill, after exactly the operators that came before it.
    let fill = parsed.ops.get(index)?;
    if fill.op != b"n" || !fill.operands.is_empty() || ops_hash(parsed.ops.get(..index)?) != hash.bytes {
        return None;
    }
    let mut out = Vec::with_capacity(bytes.len().saturating_add(op.len()));
    out.extend_from_slice(bytes.get(..fill.span.start)?);
    out.extend_from_slice(op);
    out.extend_from_slice(bytes.get(fill.span.end..)?);
    let (Backdrop::Fill, Some(found)) = scan(doc, page, Some((copy, &out)))? else { return None };
    let (piece, offset) = found.place()?;
    if found.refs.get(piece).map(|&(_, r)| r) != Some(copy) || offset != fill.span.start || found.op.len() != op.len() {
        return None;
    }
    let mut dict = s.dict.clone();
    for key in [&b"Length"[..], b"DL", TAG] {
        dict.remove(key);
    }
    let was = match note.get(b"Was").and_then(Object::as_array).map(Vec::as_slice) {
        Some([num, generation]) => Some(ObjRef { num: u32::try_from(num.as_int()?).ok()?, generation: u16::try_from(generation.as_int()?).ok()? }),
        _ => None,
    };
    Some((out, dict, was))
}

/// As much of a content stream as a backdrop could be in: it has to come first.
fn head(bytes: &[u8]) -> &[u8] {
    bytes.get(..bytes.len().min(SCAN_BYTES)).unwrap_or_default()
}

/// A SHA-256 of what `ops` are, not how they are written: keywords and operand values, with
/// numbers in one form (`1`, `1.0` and `1.00000` are the same), so a stream written out again
/// with other spacing or number formatting still matches.
fn ops_hash(ops: &[Op]) -> Vec<u8> {
    let mut h = Sha256::new();
    for op in ops {
        for o in &op.operands {
            canonical(o, &mut h, 0);
        }
        if let Some((dict, data)) = &op.inline {
            canonical_dict(dict, &mut h, 0);
            bytes(b'i', data, &mut h);
        }
        bytes(b'k', &op.op, &mut h);
    }
    h.finalize().to_vec()
}

/// How deep [`canonical`] follows nested arrays and dictionaries; deeper ones count as equal.
const CANONICAL_DEPTH: usize = 32;

fn canonical(o: &Object, h: &mut Sha256, depth: usize) {
    if depth > CANONICAL_DEPTH {
        h.update(b"~");
        return;
    }
    match o {
        Object::Null => h.update(b"z"),
        Object::Bool(b) => h.update(if *b { b"T" } else { b"F" }),
        Object::Int(i) => number(&i.to_string(), h),
        Object::Real(r) if *r == 0.0 => number("0", h),
        Object::Real(r) => number(&format!("{r:.5}"), h),
        Object::String(s) => bytes(b's', &s.bytes, h),
        Object::Name(n) => bytes(b'/', n, h),
        Object::Array(a) => {
            h.update(b"[");
            for x in a {
                canonical(x, h, depth.saturating_add(1));
            }
            h.update(b"]");
        }
        Object::Dict(d) => canonical_dict(d, h, depth),
        Object::Stream(_) | Object::Ref(_) => h.update(b"?"),
    }
}

fn canonical_dict(d: &Dict, h: &mut Sha256, depth: usize) {
    h.update(b"<");
    for (k, v) in d.iter() {
        bytes(b'/', k, h);
        canonical(v, h, depth.saturating_add(1));
    }
    h.update(b">");
}

/// A number written with up to five decimals, without trailing zeros.
fn number(text: &str, h: &mut Sha256) {
    let t = if text.contains('.') { text.trim_end_matches('0').trim_end_matches('.') } else { text };
    bytes(b'#', t.as_bytes(), h);
}

/// A typed, length-prefixed value, so no two different sequences hash the same way.
fn bytes(kind: u8, b: &[u8], h: &mut Sha256) {
    h.update([kind]);
    h.update(u64::try_from(b.len()).unwrap_or(u64::MAX).to_le_bytes());
    h.update(b);
}

/// Find the backdrop, and for a fill, where it is. With `swap`, that stream reads as those bytes
/// instead of its own (to check a change before making it).
fn scan(doc: &Document, p: &pdfcraft_model::Page, swap: Option<(ObjRef, &[u8])>) -> Option<(Backdrop, Option<Found>)> {
    let crop = p.crop(doc);
    let c = p.dict.get(b"Contents")?;
    let list: Vec<Object> = match &*doc.resolve(c) {
        Object::Array(a) if a.len() <= SCAN_OPS => a.clone(),
        Object::Array(_) => return None,
        _ => vec![c.clone()],
    };
    // The page's own content: untagged streams (marks and the q/Q wrapper carry /PCMark). Read
    // only as far as a backdrop could be.
    let mut refs = Vec::new();
    let mut data = Vec::new();
    let mut starts = Vec::new();
    let mut total = 0usize;
    let mut budget = DecodeBudget::new(SCAN_BYTES, 0);
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
        // Bytes in an external file (`/F`) replace the stream's own: what it draws is unknown here.
        if s.dict.get(b"F").is_some() {
            return None;
        }
        let bytes = match swap {
            Some((with, b)) if with == *r => {
                if b.len() > SCAN_BYTES.saturating_sub(total) {
                    return None;
                }
                budget.decode_strict(&Stream::from_raw(Dict::new(), b.to_vec())).ok()?
            }
            // Tolerant decoding can return an empty or partial stream after corruption.
            // That cannot prove which paint preceded the candidate backdrop.
            _ => budget.decode_strict(s).ok()?,
        };
        if bytes.len() > SCAN_BYTES.saturating_sub(total).saturating_sub(1) {
            return None;
        }
        starts.push(total);
        total += bytes.len() + 1;
        refs.push((slot, *r));
        data.push(bytes);
    }
    let pieces = Pieces::join(&data);
    let parsed = pdfcraft_content::parse(head(pieces.data()));
    let res = resources(doc, &p.dict);
    let found = walk(doc, &res, crop, &parsed.ops);
    // A match proves the graphics state for one execution only. Repeated references may
    // paint real artwork under a different color or transform: leave them untouched.
    if let Some((Backdrop::Fill, Some(op))) = &found {
        let piece = pieces.piece_of(op.start);
        let (_, reference) = refs.get(piece)?;
        if list.iter().filter(|o| matches!(o, Object::Ref(r) if r == reference)).count() != 1 {
            return None;
        }
    }
    found.map(|(b, op)| match (b, op) {
        (Backdrop::Fill, Some(op)) => (b, Some(Found { refs, pieces, starts, op })),
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
    let mut st = State { ctm: Matrix::IDENTITY, clip: Some([f64::MIN, f64::MIN, f64::MAX, f64::MAX]), white: false, space: 1, paint: Paint::INITIAL };
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
            b"gs" => apply_state(doc, res, op.name(0)?, &mut st.paint)?,
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
                return (covers && st.white && st.paint.plain() && !clip_next).then(|| (Backdrop::Fill, Some(op.span.clone())));
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
        && st.paint.plain()
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

/// Apply the named ExtGState's keys that [`Paint`] tracks; the keys it doesn't have leave
/// `paint` as it was. `None` when the dictionary or one of those values can't be read.
fn apply_state(doc: &Document, res: &Dict, name: &[u8], paint: &mut Paint) -> Option<()> {
    let gs = sub(doc, res, b"ExtGState", name)?;
    let gs = gs.as_dict()?;
    if let Some(alpha) = gs.get(b"ca") {
        paint.full_alpha = doc.resolve(alpha).as_f64()? >= 0.999;
    }
    if let Some(mask) = gs.get(b"SMask") {
        paint.no_soft_mask = doc.resolve(mask).as_name() == Some(b"None");
    }
    if let Some(blend) = gs.get(b"BM") {
        paint.normal_blend = matches!(doc.resolve(blend).as_name(), Some(b"Normal" | b"Compatible"));
    }
    // TR2 takes precedence over TR in the same dictionary (§8.4.5, Table 57). Only the names
    // are read: a function (or one per colourant) is taken to change white.
    if let Some(transfer) = gs.get(b"TR2") {
        paint.identity_transfer = matches!(doc.resolve(transfer).as_name(), Some(b"Identity" | b"Default"));
    } else if let Some(transfer) = gs.get(b"TR") {
        paint.identity_transfer = doc.resolve(transfer).as_name() == Some(b"Identity");
    }
    Some(())
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
