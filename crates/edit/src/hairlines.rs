//! Fix hairlines (Acrobat's Print production ▸ Fix Hairlines; Fix hairlines): strokes thinner
//! than a printer holds - zero-width lines, and lines whose width on the page, after every
//! transformation, is below a threshold - are given a chosen width. Page content and the Form
//! XObjects it draws are rewritten; Type 3 glyphs and tiling patterns on request.
//!
//! A form is rewritten for the scale and line width it is drawn under. One drawn under several
//! (or drawn by a page that is not being fixed) is rewritten as a copy for each use that needs
//! one, and the resources of what draws it point at the copy, so no other use changes.

use pdfcraft_content::{Op, parse, serialize_ops};
use pdfcraft_cos::{Dict, Document, Name, ObjRef, Object, Stream};
use std::collections::{HashMap, HashSet};

use crate::page_streams::PageStreams;
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

/// The graphics state a stream leaves for the next one on the same page: CTM and line width,
/// with the q/Q stack, and whether a path is still being built.
#[derive(Clone, Debug)]
struct State {
    ctm: [f64; 6],
    width: f64,
    stack: Vec<([f64; 6], f64)>,
    in_path: bool,
}

impl State {
    /// A stream drawn under `base` with line width `width` in effect.
    fn new(base: [f64; 6], width: f64) -> Self {
        Self { ctm: base, width, stack: Vec::new(), in_path: false }
    }
}

/// All a form's rewrite depends on: the scale it is drawn under and the line width in effect
/// at its `Do` (as bits, so it can be a map key).
type Key = (u64, u64);

fn key_of(ctm: [f64; 6], width: f64) -> Key {
    (scale_of(ctm).to_bits(), width.to_bits())
}

/// A Form XObject a stream draws: the resource name it is drawn by, the object, and the CTM
/// and line width at its `Do`.
struct Drawn {
    name: Name,
    r: ObjRef,
    ctm: [f64; 6],
    width: f64,
}

/// The names a stream's `Do`s are given instead, by the name they had and the key they are
/// drawn under.
type Renames = HashMap<(Name, Key), Name>;

/// How deep forms drawing forms are followed, and how many form rewrites one run makes; past
/// either, a form is drawn as it is.
const MAX_DEPTH: usize = 32;
const MAX_FORMS: usize = 4096;

/// The width `/name gs` sets, when its ExtGState has an `/LW`.
fn line_width_of(doc: &Document, gstates: Option<&Dict>, name: &[u8]) -> Option<f64> {
    let gs = doc.resolve(gstates?.get(name)?);
    let lw = gs.as_dict()?.get(b"LW")?;
    doc.resolve(lw).as_f64().filter(|w| w.is_finite())
}

/// Rewrite one content stream drawn under `state` (the CTM when it starts, scale only
/// matters), leaving in it the state at the stream's end. A thin stroke gets a `w` of its own
/// before its path begins (graphics-state operators are not allowed inside a path) and the
/// old width back after it is painted, so nothing else drawn changes width.
/// `forms` collects the Form XObjects it draws, with the CTM and width each is drawn under,
/// for the caller to rewrite in turn; a `Do` listed in `renames` draws the name given there.
/// Returns the new bytes when anything changed, and the count.
fn rewrite(
    data: &[u8],
    state: &mut State,
    h: &Hairlines,
    resources: Option<&Dict>,
    doc: &Document,
    forms: &mut Vec<Drawn>,
    renames: &Renames,
) -> (Option<Vec<u8>>, usize) {
    let parsed = parse(data);
    let mut ops: Vec<Op> = parsed.ops;
    let mut fixed = 0usize;
    let mut changed = false;
    // Where in `out` the path being built began (None when it began in an earlier stream).
    let mut path_start: Option<usize> = None;
    let mut out: Vec<Op> = Vec::with_capacity(ops.len() + 8);
    let dict_of = |key: &[u8]| resources.and_then(|r| r.get(key)).map(|x| doc.resolve(x)).and_then(|x| x.as_dict().cloned());
    let xobjects = dict_of(&b"XObject"[..]);
    let gstates = dict_of(&b"ExtGState"[..]);
    for mut op in ops.drain(..) {
        match op.op.as_slice() {
            b"q" => {
                state.stack.push((state.ctm, state.width));
                if state.stack.len() > 512 {
                    state.stack.remove(0);
                }
            }
            b"Q" => {
                if let Some((c, w)) = state.stack.pop() {
                    state.ctm = c;
                    state.width = w;
                }
            }
            b"cm" => {
                if let Some(m) = op.nums::<6>() {
                    state.ctm = mul(m, state.ctm);
                }
            }
            b"w" => {
                if let Some([w]) = op.nums::<1>() {
                    state.width = w;
                }
            }
            b"gs" => {
                // An ExtGState's /LW sets the width as `w` does (so the width put back after
                // a fixed stroke is the right one).
                if let Some(Object::Name(name)) = op.operands.first()
                    && let Some(w) = line_width_of(doc, gstates.as_ref(), name)
                {
                    state.width = w;
                }
            }
            b"m" | b"re" => {
                if !state.in_path {
                    state.in_path = true;
                    path_start = Some(out.len());
                }
            }
            b"Do" => {
                if let Some(Object::Name(name)) = op.operands.first()
                    && let Some(x) = xobjects.as_ref().and_then(|x| x.get(name))
                    && let Some(r) = x.as_ref()
                    && matches!(&*doc.get(r), Object::Stream(s) if s.dict.name(b"Subtype") == Some(b"Form"))
                {
                    let name = name.clone();
                    if !renames.is_empty()
                        && let Some(to) = renames.get(&(name.clone(), key_of(state.ctm, state.width)))
                        && let Some(first) = op.operands.first_mut()
                    {
                        *first = Object::Name(to.clone());
                        changed = true;
                    }
                    forms.push(Drawn { name, r, ctm: state.ctm, width: state.width });
                }
            }
            b"S" | b"s" | b"B" | b"B*" | b"b" | b"b*" => {
                let scale = scale_of(state.ctm);
                let on_page = state.width * scale;
                let thin = state.width <= 0.0 || (scale > 0.0 && on_page < h.narrower_than);
                // A path begun in an earlier stream cannot get its `w` before it here; it is
                // left as it is rather than given an operator inside the path.
                let at = match path_start {
                    Some(i) => Some(i),
                    None if !state.in_path => Some(out.len()),
                    None => None,
                };
                state.in_path = false;
                path_start = None;
                if thin
                    && scale > 0.0
                    && let Some(at) = at
                {
                    let new_w = h.width / scale;
                    out.insert(at, Op::new("w", vec![Object::Real(new_w)]));
                    out.push(op);
                    out.push(Op::new("w", vec![Object::Real(state.width)]));
                    fixed += 1;
                    changed = true;
                    continue;
                }
            }
            b"f" | b"F" | b"f*" | b"n" => {
                state.in_path = false;
                path_start = None;
            }
            _ => {}
        }
        out.push(op);
    }
    (changed.then(|| serialize_ops(&out)), fixed)
}

/// What a run keeps across pages.
struct Run {
    /// Each form already rewritten under a key, and the object drawn for it there.
    done: HashMap<(ObjRef, Key), ObjRef>,
    /// The forms drawn where this run does not rewrite: they are never changed in place.
    shared: HashSet<ObjRef>,
    /// The forms already drawn as themselves: `true` when rewritten in place, `false` when
    /// left as they were. Any other use that needs something else gets a copy.
    used: HashMap<ObjRef, bool>,
    /// Each form's stream as it was before the run.
    original: HashMap<ObjRef, Stream>,
    /// Form rewrites left.
    budget: usize,
}

/// The XObjects (forms and images, and those the forms draw) that the pages not in `pages`
/// draw, and those any page's annotation appearances draw (this run does not rewrite them):
/// rewriting one of those in place would change what they show.
fn drawn_elsewhere(doc: &Document, pages: &[usize]) -> HashSet<ObjRef> {
    let mut seen: HashSet<ObjRef> = HashSet::new();
    let mut todo: Vec<Object> = Vec::new();
    let xobjects_of = |res: Option<&Object>, todo: &mut Vec<Object>| {
        if let Some(xo) = res
            .map(|r| doc.resolve(r))
            .and_then(|r| r.as_dict().and_then(|d| d.get(b"XObject")).map(|x| doc.resolve(x)))
            .and_then(|x| x.as_dict().cloned())
        {
            todo.extend(xo.iter().map(|(_, o)| o.clone()));
        }
    };
    for (i, page) in page_list(doc).into_iter().enumerate() {
        if !pages.contains(&i) {
            xobjects_of(page.dict.get(b"Resources"), &mut todo);
        }
        let annots = page.dict.get(b"Annots").map(|a| doc.resolve(a)).and_then(|a| a.as_array().cloned()).unwrap_or_default();
        for a in annots {
            let Some(ap) = doc.resolve(&a).as_dict().and_then(|d| d.get(b"AP").cloned()) else { continue };
            let Some(ap) = doc.resolve(&ap).as_dict().cloned() else { continue };
            for (_, v) in ap.iter() {
                match &*doc.resolve(v) {
                    Object::Dict(states) => todo.extend(states.iter().map(|(_, s)| s.clone())),
                    _ => todo.push(v.clone()),
                }
            }
        }
    }
    while let Some(o) = todo.pop() {
        let Some(r) = o.as_ref() else { continue };
        if !seen.insert(r) {
            continue;
        }
        let res = match &*doc.get(r) {
            Object::Stream(s) => s.dict.get(b"Resources").cloned(),
            _ => None,
        };
        xobjects_of(res.as_ref(), &mut todo);
    }
    seen
}

/// A name for another copy drawn by `name` that `xobjects` does not hold yet.
fn fresh_name(xobjects: &Dict, name: &[u8]) -> Name {
    let mut i = 1usize;
    loop {
        let mut candidate = name.to_vec();
        candidate.extend_from_slice(format!("_hl{i}").as_bytes());
        if !xobjects.contains(&candidate) {
            return candidate;
        }
        i = i.saturating_add(1);
    }
}

/// Rewrite one drawing context - a page's streams, or a form's - that starts in `start`, and
/// the forms it draws. Returns each stream's new bytes (`None` where it did not change), the
/// strokes fixed (in the forms too), and the `/XObject` dictionary for the context's resources
/// when its names must now point at other objects.
fn fix_context(
    doc: &mut Document,
    run: &mut Run,
    h: &Hairlines,
    data: &[Vec<u8>],
    start: &State,
    resources: Option<&Dict>,
    depth: usize,
) -> (Vec<Option<Vec<u8>>>, usize, Option<Dict>) {
    let none = Renames::new();
    let mut drawn: Vec<Drawn> = Vec::new();
    let mut state = start.clone();
    let mut new: Vec<Option<Vec<u8>>> = Vec::with_capacity(data.len());
    let mut fixed = 0usize;
    for d in data {
        let (bytes, count) = rewrite(d, &mut state, h, resources, doc, &mut drawn, &none);
        fixed += count;
        new.push(bytes);
    }
    if drawn.is_empty() {
        return (new, fixed, None);
    }
    let Some(xo) = resources.and_then(|r| r.get(b"XObject")).map(|x| doc.resolve(x)).and_then(|x| x.as_dict().cloned()) else {
        return (new, fixed, None);
    };
    // The object each (name, key) is drawn as, and each name's objects in order of first use.
    let mut chosen: HashMap<(Name, Key), ObjRef> = HashMap::new();
    let mut targets: Vec<(Name, Vec<ObjRef>)> = Vec::new();
    for d in drawn {
        let k = key_of(d.ctm, d.width);
        if chosen.contains_key(&(d.name.clone(), k)) {
            continue;
        }
        let (to, count) = fix_form(doc, run, h, d.r, d.ctm, d.width, depth + 1);
        fixed += count;
        chosen.insert((d.name.clone(), k), to);
        match targets.iter_mut().find(|(name, _)| *name == d.name) {
            Some((_, list)) => {
                if !list.contains(&to) {
                    list.push(to);
                }
            }
            None => targets.push((d.name, vec![to])),
        }
    }
    // The first object keeps the name; each other one gets a name of its own, and the `Do`s
    // drawing it are renamed.
    let mut repointed = xo;
    let mut changed = false;
    let mut renames = Renames::new();
    for (name, list) in &targets {
        for (i, to) in list.iter().enumerate() {
            let at = if i == 0 { name.clone() } else { fresh_name(&repointed, name) };
            if repointed.get(&at).and_then(Object::as_ref) != Some(*to) {
                repointed.set(at.clone(), Object::Ref(*to));
                changed = true;
            }
            if i > 0 {
                for ((was, k), t) in &chosen {
                    if was == name && t == to {
                        renames.insert((was.clone(), *k), at.clone());
                    }
                }
            }
        }
    }
    if !renames.is_empty() {
        // The same rewrite again, with the renamed `Do`s.
        let mut state = start.clone();
        let mut again: Vec<Drawn> = Vec::new();
        new = data.iter().map(|d| rewrite(d, &mut state, h, resources, &*doc, &mut again, &renames).0).collect();
    }
    (new, fixed, changed.then_some(repointed))
}

/// The object `r` is drawn as under `ctm` with line width `width` in effect, rewritten for
/// them, and the strokes that rewrite fixed. It is `r` itself, rewritten in place, unless
/// something else draws `r` differently; then it is a copy.
fn fix_form(doc: &mut Document, run: &mut Run, h: &Hairlines, r: ObjRef, ctm: [f64; 6], width: f64, depth: usize) -> (ObjRef, usize) {
    let k = key_of(ctm, width);
    if let Some(&to) = run.done.get(&(r, k)) {
        return (to, 0);
    }
    if depth > MAX_DEPTH || run.budget == 0 {
        return (r, 0);
    }
    run.budget -= 1;
    // Marked before rewriting, so a form that draws itself ends.
    run.done.insert((r, k), r);
    let s = match run.original.get(&r) {
        Some(s) => s.clone(),
        None => {
            let Some(s) = stream_of(doc, r) else { return (r, 0) };
            run.original.insert(r, s.clone());
            s
        }
    };
    let Ok(data) = s.decoded() else { return (r, 0) };
    let matrix = s.dict.get(b"Matrix").map(|m| doc.resolve(m)).and_then(|m| m.as_array().cloned()).and_then(|a| {
        let v: Vec<f64> = a.iter().filter_map(|x| doc.resolve(x).as_f64()).collect();
        <[f64; 6]>::try_from(v).ok()
    });
    let base = match matrix {
        Some(m) => mul(m, ctm),
        None => ctm,
    };
    let res = s.dict.get(b"Resources").map(|x| doc.resolve(x)).and_then(|x| x.as_dict().cloned());
    let (new, fixed, xo) = fix_context(doc, run, h, std::slice::from_ref(&data), &State::new(base, width), res.as_ref(), depth);
    let bytes = new.into_iter().next().flatten();
    let to = if bytes.is_none() && xo.is_none() {
        if run.used.get(&r) == Some(&true) {
            // `r` was rewritten in place for another use; this one draws it as it was.
            doc.add(Object::Stream(s))
        } else {
            run.used.insert(r, false);
            r
        }
    } else {
        let mut form = s;
        if let Some(xo) = xo {
            let mut res = res.unwrap_or_default();
            res.set(b"XObject".to_vec(), Object::Dict(xo));
            form.dict.set(b"Resources".to_vec(), Object::Dict(res));
        }
        if let Some(bytes) = bytes {
            let mut dict = form.dict;
            for key in [&b"Length"[..], b"Filter", b"DecodeParms"] {
                dict.remove(key);
            }
            form = Stream::flate(dict, &bytes);
        }
        if run.shared.contains(&r) || run.used.contains_key(&r) {
            doc.add(Object::Stream(form))
        } else {
            doc.set(r, Object::Stream(form));
            run.used.insert(r, true);
            r
        }
    };
    run.done.insert((r, k), to);
    (to, fixed)
}

/// Fix the hairlines on `pages` (0-based). Each page's content, and each Form XObject it draws
/// (for the scale and line width it is drawn under), is rewritten where a stroke is thinner
/// than `h.narrower_than` points on the page. A form drawn under more than one, or drawn by a
/// page not in `pages` or by an annotation's appearance, is rewritten as a copy for the uses
/// that need one, and the page's (or drawing form's) resources point at the copy; a resource
/// dictionary other pages share is not changed. Type 3 glyphs and tiling patterns are rewritten
/// in place, once, at a nominal scale (see `fix_in_resources`).
pub fn fix_hairlines(doc: &mut Document, pages: &[usize], h: &Hairlines) -> Result<Fixed, EditError> {
    let all = page_list(doc);
    check(pages, all.len())?;
    if !(h.narrower_than.is_finite() && h.narrower_than >= 0.0 && h.width.is_finite() && h.width > 0.0) {
        return Err(EditError::Invalid("the hairline widths must be positive numbers of points".into()));
    }
    let mut result = Fixed::default();
    let mut run =
        Run { done: HashMap::new(), shared: drawn_elsewhere(doc, pages), used: HashMap::new(), original: HashMap::new(), budget: MAX_FORMS };
    let mut done_glyphs: HashSet<ObjRef> = HashSet::new();
    for &i in pages {
        let Some(page) = page_list(doc).get(i).cloned() else { continue };
        let resources = page.dict.get(b"Resources").map(|r| doc.resolve(r)).and_then(|r| r.as_dict().cloned());
        let mut page_fixed = 0usize;
        // The page's streams, each rewritten on its own (state carries across them), and the
        // forms they draw. A page with a stream that cannot be read is left alone.
        if let Some(streams) = PageStreams::read(doc, &page.dict) {
            let start = State::new([1.0, 0.0, 0.0, 1.0, 0.0, 0.0], 1.0);
            let (new, fixed, xo) = fix_context(doc, &mut run, h, &streams.data, &start, resources.as_ref(), 0);
            page_fixed += fixed;
            streams.write(doc, page.obj, new)?;
            if let Some(xo) = xo {
                // This page's own resources point at the copies; a resource dictionary other
                // pages share is not changed.
                let mut res = resources.clone().unwrap_or_default();
                res.set(b"XObject".to_vec(), Object::Dict(xo));
                doc.update_dict(page.obj, |d| d.set(b"Resources".to_vec(), Object::Dict(res)))?;
            }
        }
        if h.type3 || h.patterns {
            page_fixed += fix_in_resources(doc, resources.as_ref(), h, &mut done_glyphs)?;
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
/// not known here, so the glyph's own scale stands in). They are rewritten in place, once per
/// run: a pattern's space is set by its matrix whatever CTM it is painted under, and a glyph
/// is measured at a nominal size, so there is no per-use scale to make a copy for. The forms
/// they draw are not followed.
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
    let none = Renames::new();
    for (r, base) in targets {
        if !done.insert(r) {
            continue;
        }
        let Some(s) = stream_of(doc, r) else { continue };
        let Ok(data) = s.decoded() else { continue };
        let res = s.dict.get(b"Resources").map(|x| doc.resolve(x)).and_then(|x| x.as_dict().cloned());
        let mut inner = Vec::new();
        let (new, n_fixed) = rewrite(&data, &mut State::new(base, 1.0), h, res.as_ref(), doc, &mut inner, &none);
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
