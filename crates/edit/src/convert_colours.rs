//! Convert colours (Acrobat's Print production ▸ Convert Colors): every colour
//! operator in the page content and the artwork it draws is rewritten in a target space -
//! grayscale, RGB or CMYK - by the standard device formulas (no ICC profile is applied), black
//! preserved on request; pictures are re-encoded through a helper the caller supplies.

use pdfcraft_content::{Op, parse, serialize_ops};
use pdfcraft_cos::{Dict, Document, ObjRef, Object, Stream};
use std::collections::{HashMap, HashSet};

use crate::colour::{Space, rgb, space_of};
use crate::page_streams::PageStreams;
use crate::{EditError, check, page_list};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ColourTarget {
    Gray,
    Rgb,
    Cmyk,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ConvertColours {
    pub target: ColourTarget,
    /// Pure black stays 100% K (or gray 0) rather than a mix.
    pub preserve_black: bool,
    /// Re-encode pictures too (through the caller's helper).
    pub images: bool,
    /// The appearance streams of the pages' comments and form fields too.
    pub annotations: bool,
}

impl Default for ConvertColours {
    fn default() -> Self {
        Self { target: ColourTarget::Gray, preserve_black: true, images: true, annotations: false }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Converted {
    /// Colour operators rewritten.
    pub colours: usize,
    /// Pictures re-encoded.
    pub images: usize,
    /// Pages on which something changed.
    pub pages: usize,
}

/// Re-encode one image stream in the target space, if it can be: the caller's (the optimizer
/// knows how to decode and encode pictures; the edit crate does not).
pub type ImageRecolour<'a> = dyn FnMut(&Document, &Stream, ColourTarget) -> Option<Stream> + 'a;

fn luma(c: [f64; 3]) -> f64 {
    (0.299 * c[0] + 0.587 * c[1] + 0.114 * c[2]).clamp(0.0, 1.0)
}

/// sRGB → the target's components.
pub fn components(target: ColourTarget, c: [f64; 3], preserve_black: bool) -> Vec<f64> {
    let black = c.iter().all(|v| *v < 0.02);
    match target {
        ColourTarget::Gray => vec![if black && preserve_black { 0.0 } else { luma(c) }],
        ColourTarget::Rgb => c.to_vec(),
        ColourTarget::Cmyk => {
            if black && preserve_black {
                return vec![0.0, 0.0, 0.0, 1.0];
            }
            let k = 1.0 - c.iter().cloned().fold(0.0, f64::max);
            if k >= 0.999 {
                return vec![0.0, 0.0, 0.0, 1.0];
            }
            let f = |v: f64| ((1.0 - v - k) / (1.0 - k)).clamp(0.0, 1.0);
            vec![f(c[0]), f(c[1]), f(c[2]), k]
        }
    }
}

fn op_for(target: ColourTarget, stroke: bool) -> &'static str {
    match (target, stroke) {
        (ColourTarget::Gray, false) => "g",
        (ColourTarget::Gray, true) => "G",
        (ColourTarget::Rgb, false) => "rg",
        (ColourTarget::Rgb, true) => "RG",
        (ColourTarget::Cmyk, false) => "k",
        (ColourTarget::Cmyk, true) => "K",
    }
}

/// Whether a `cs` operand names DeviceCMYK itself (directly or through a `/ColorSpace`
/// resource), whose initial colour is black, unlike an ICC CMYK space's all-zero white.
fn device_cmyk(doc: &Document, operand: &Object, cs_res: &Dict) -> bool {
    let is_device = |o: &Object| match &*doc.resolve(o) {
        Object::Name(n) => matches!(n.as_slice(), b"DeviceCMYK" | b"CMYK"),
        Object::Array(a) => a.len() == 1 && a.first().and_then(Object::as_name).is_some_and(|n| matches!(n, b"DeviceCMYK" | b"CMYK")),
        _ => false,
    };
    match operand.as_name() {
        Some(n) => is_device(operand) || cs_res.get(n).is_some_and(is_device),
        None => is_device(operand),
    }
}

fn r3(v: f64) -> Object {
    Object::Real((v * 1000.0).round() / 1000.0)
}

/// The current fill and stroke spaces (None = a device space set by g/rg/k, which the
/// converted operator sets again), with the q/Q stack: what one content stream leaves for the
/// next on the same page.
#[derive(Clone, Debug, Default)]
struct State {
    fill: Option<Space>,
    stroke: Option<Space>,
    stack: Vec<(Option<Space>, Option<Space>)>,
}

/// Rewrite one content stream's colour operators, starting from `state` and leaving in it the
/// state at the stream's end. `cs_res` is its `/ColorSpace` resource dictionary. Returns the
/// new bytes when anything changed, and how many operators.
fn rewrite(data: &[u8], state: &mut State, doc: &Document, cs_res: &Dict, settings: &ConvertColours) -> (Option<Vec<u8>>, usize) {
    let mut ops: Vec<Op> = parse(data).ops;
    let mut count = 0usize;
    let mut out: Vec<Op> = Vec::with_capacity(ops.len());
    let State { fill, stroke, stack } = state;
    let numbers = |op: &Op| -> Vec<f64> { op.operands.iter().filter_map(Object::as_f64).collect() };
    let convert = |c: [f64; 3], is_stroke: bool| -> Op {
        Op::new(op_for(settings.target, is_stroke), components(settings.target, c, settings.preserve_black).into_iter().map(r3).collect())
    };
    for op in ops.drain(..) {
        match op.op.as_slice() {
            b"q" => {
                stack.push((fill.clone(), stroke.clone()));
                if stack.len() > 512 {
                    stack.remove(0);
                }
                out.push(op);
            }
            b"Q" => {
                if let Some((f, s)) = stack.pop() {
                    *fill = f;
                    *stroke = s;
                }
                out.push(op);
            }
            b"cs" | b"CS" => {
                let is_stroke = op.op.as_slice() == b"CS";
                let space = op.operands.first().map(|o| space_of(doc, o, cs_res, 0)).unwrap_or(Space::Gray);
                let pattern = space == Space::Pattern;
                if is_stroke {
                    *stroke = Some(space);
                } else {
                    *fill = Some(space);
                }
                if pattern {
                    // Patterns stay what they are (their own content is not converted here).
                    out.push(op);
                } else {
                    // The space's initial colour, now in the target: black (a tint at full,
                    // DeviceCMYK's 0 0 0 1, ISO 32000-1 8.6.5.5).
                    let space = if is_stroke { stroke.clone() } else { fill.clone() }.unwrap_or(Space::Gray);
                    let initial = match space {
                        Space::Tint { .. } => rgb(&space, &[1.0; 8]),
                        Space::Cmyk if op.operands.first().is_some_and(|o| device_cmyk(doc, o, cs_res)) => rgb(&space, &[0.0, 0.0, 0.0, 1.0]),
                        _ => rgb(&space, &[]),
                    };
                    out.push(convert(initial, is_stroke));
                    count += 1;
                }
            }
            b"g" | b"rg" | b"k" | b"G" | b"RG" | b"K" => {
                let is_stroke = op.op[0].is_ascii_uppercase();
                let space = match op.op.to_ascii_lowercase().as_slice() {
                    b"g" => Space::Gray,
                    b"rg" => Space::Rgb,
                    _ => Space::Cmyk,
                };
                let already = matches!(
                    (settings.target, &space),
                    (ColourTarget::Gray, Space::Gray) | (ColourTarget::Rgb, Space::Rgb) | (ColourTarget::Cmyk, Space::Cmyk)
                );
                if is_stroke {
                    *stroke = None;
                } else {
                    *fill = None;
                }
                if already {
                    out.push(op);
                } else {
                    let c = rgb(&space, &numbers(&op));
                    out.push(convert(c, is_stroke));
                    count += 1;
                }
            }
            b"sc" | b"scn" | b"SC" | b"SCN" => {
                let is_stroke = op.op[0].is_ascii_uppercase();
                let space = if is_stroke { stroke.clone() } else { fill.clone() };
                match space {
                    Some(Space::Pattern) | None if op.operands.iter().any(|o| matches!(o, Object::Name(_))) => out.push(op),
                    None => {
                        // sc after g/rg/k: the device space those set; read the numbers by count.
                        let n = numbers(&op);
                        let space = match n.len() {
                            1 => Space::Gray,
                            4 => Space::Cmyk,
                            _ => Space::Rgb,
                        };
                        out.push(convert(rgb(&space, &n), is_stroke));
                        count += 1;
                    }
                    Some(Space::Pattern) => out.push(op),
                    Some(space) => {
                        out.push(convert(rgb(&space, &numbers(&op)), is_stroke));
                        count += 1;
                    }
                }
            }
            _ => out.push(op),
        }
    }
    ((count > 0).then(|| serialize_ops(&out)), count)
}

fn stream_of(doc: &Document, r: ObjRef) -> Option<Stream> {
    match &*doc.get(r) {
        Object::Stream(s) => Some(s.clone()),
        _ => None,
    }
}

fn cs_resources(doc: &Document, resources: Option<&Dict>) -> Dict {
    resources.and_then(|r| r.get(b"ColorSpace")).map(|c| doc.resolve(c)).and_then(|c| c.as_dict().cloned()).unwrap_or_default()
}

/// What a run keeps across pages: each XObject already handled, and the object that now
/// stands for it (itself, or the converted copy made because a page not being converted draws
/// it too); and the XObjects those other pages draw.
struct Run {
    done: HashMap<ObjRef, ObjRef>,
    shared: HashSet<ObjRef>,
}

/// The XObjects (forms and images, and those the forms draw), and appearance streams, that
/// the pages not in `pages` draw: converting one of those in place would change those pages.
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
        if pages.contains(&i) {
            continue;
        }
        xobjects_of(page.dict.get(b"Resources"), &mut todo);
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

/// Store a converted XObject: in place, or, when another page draws `r` too, as a new object
/// the caller points to instead. Returns the object that now stands for `r`.
fn put(doc: &mut Document, run: &mut Run, r: ObjRef, obj: Object) -> ObjRef {
    let to = if run.shared.contains(&r) {
        doc.add(obj)
    } else {
        doc.set(r, obj);
        r
    };
    run.done.insert(r, to);
    to
}

/// Convert the XObjects of `resources`: forms rewritten (and their own XObjects after them),
/// images through `recolour`. Each object is converted once; one that a page not being
/// converted draws is converted as a copy, and the returned `/XObject` dictionary, when there
/// is one, points at the copies (the caller puts it in `resources`' place). When `may_copy` is
/// false the caller cannot do that, so such objects are left alone.
fn convert_xobjects(
    doc: &mut Document,
    resources: Option<&Dict>,
    settings: &ConvertColours,
    recolour: &mut ImageRecolour<'_>,
    run: &mut Run,
    result: &mut Converted,
    may_copy: bool,
) -> Option<Dict> {
    let xo = resources.and_then(|r| r.get(b"XObject")).map(|x| doc.resolve(x)).and_then(|x| x.as_dict().cloned())?;
    let mut repointed = xo.clone();
    let mut changed = false;
    for (name, o) in xo.iter() {
        let Some(r) = o.as_ref() else { continue };
        if let Some(&to) = run.done.get(&r) {
            if to != r {
                repointed.set(name.clone(), Object::Ref(to));
                changed = true;
            }
            continue;
        }
        if !may_copy && run.shared.contains(&r) {
            continue;
        }
        // Marked before converting, so a form that draws itself ends.
        run.done.insert(r, r);
        let Some(s) = stream_of(doc, r) else { continue };
        let stored = match s.dict.name(b"Subtype") {
            Some(b"Form") => {
                let res = s.dict.get(b"Resources").map(|x| doc.resolve(x)).and_then(|x| x.as_dict().cloned());
                let cs = cs_resources(doc, res.as_ref());
                let (new, n) = match s.decoded() {
                    Ok(data) => rewrite(&data, &mut State::default(), doc, &cs, settings),
                    Err(_) => (None, 0),
                };
                let inner = convert_xobjects(doc, res.as_ref(), settings, recolour, run, result, true);
                if new.is_none() && inner.is_none() {
                    None
                } else {
                    let mut form = s.clone();
                    if let Some(xo) = inner {
                        let mut res = res.unwrap_or_default();
                        res.set(b"XObject".to_vec(), Object::Dict(xo));
                        form.dict.set(b"Resources".to_vec(), Object::Dict(res));
                    }
                    if let Some(bytes) = new {
                        let mut dict = form.dict;
                        for k in [&b"Length"[..], b"Filter", b"DecodeParms"] {
                            dict.remove(k);
                        }
                        form = Stream::flate(dict, &bytes);
                        result.colours += n;
                    }
                    Some(put(doc, run, r, Object::Stream(form)))
                }
            }
            Some(b"Image") if settings.images => match recolour(doc, &s, settings.target) {
                Some(new) => {
                    result.images += 1;
                    Some(put(doc, run, r, Object::Stream(new)))
                }
                None => None,
            },
            _ => None,
        };
        if let Some(to) = stored
            && to != r
        {
            repointed.set(name.clone(), Object::Ref(to));
            changed = true;
        }
    }
    changed.then_some(repointed)
}

/// Convert the colours on `pages` (0-based). An object that pages not in `pages` also draw is
/// converted as a copy for these pages, so the others keep their colours.
pub fn convert_colours(
    doc: &mut Document,
    pages: &[usize],
    settings: &ConvertColours,
    recolour: &mut ImageRecolour<'_>,
) -> Result<Converted, EditError> {
    let all = page_list(doc);
    check(pages, all.len())?;
    let mut result = Converted::default();
    let mut run = Run { done: HashMap::new(), shared: drawn_elsewhere(doc, pages) };
    for &i in pages {
        let Some(page) = page_list(doc).get(i).cloned() else { continue };
        let resources = page.dict.get(b"Resources").map(|r| doc.resolve(r)).and_then(|r| r.as_dict().cloned());
        let cs = cs_resources(doc, resources.as_ref());
        let before = (result.colours, result.images);
        // Each content stream on its own (state carries across them); a page with a stream
        // that cannot be read keeps its content as it is.
        if let Some(streams) = PageStreams::read(doc, &page.dict) {
            let mut state = State::default();
            let mut new: Vec<Option<Vec<u8>>> = Vec::with_capacity(streams.data.len());
            for data in &streams.data {
                let (bytes, n) = rewrite(data, &mut state, doc, &cs, settings);
                result.colours += n;
                new.push(bytes);
            }
            streams.write(doc, page.obj, new)?;
        }
        if let Some(xo) = convert_xobjects(doc, resources.as_ref(), settings, recolour, &mut run, &mut result, true) {
            // This page's own resources point at the copies; a resource dictionary other pages
            // share is not changed.
            let mut res = resources.clone().unwrap_or_default();
            res.set(b"XObject".to_vec(), Object::Dict(xo));
            doc.update_dict(page.obj, |d| d.set(b"Resources".to_vec(), Object::Dict(res)))?;
        }
        if settings.annotations {
            let annots: Vec<ObjRef> = page
                .dict
                .get(b"Annots")
                .map(|a| doc.resolve(a))
                .and_then(|a| a.as_array().cloned())
                .unwrap_or_default()
                .iter()
                .filter_map(|a| a.as_ref())
                .collect();
            for a in annots {
                let Some(ap) =
                    doc.get(a).as_dict().and_then(|d| d.get(b"AP").cloned()).map(|ap| doc.resolve(&ap)).and_then(|ap| ap.as_dict().cloned())
                else {
                    continue;
                };
                // Every appearance stream (and the states of a /N dictionary) as a form.
                let mut streams: Vec<ObjRef> = Vec::new();
                for (_, v) in ap.iter() {
                    match &*doc.resolve(v) {
                        Object::Stream(_) => streams.extend(v.as_ref()),
                        Object::Dict(states) => streams.extend(states.iter().filter_map(|(_, s)| s.as_ref())),
                        _ => {}
                    }
                }
                let mut fake = Dict::new();
                let mut xo = Dict::new();
                for (k, r) in streams.iter().enumerate() {
                    xo.set(format!("A{k}").into_bytes(), Object::Ref(*r));
                }
                fake.set(b"XObject".to_vec(), Object::Dict(xo));
                // An appearance stream other pages draw is left alone (there is no resource
                // dictionary here to point at a copy).
                convert_xobjects(doc, Some(&fake), settings, recolour, &mut run, &mut result, false);
            }
        }
        if (result.colours, result.images) != before {
            result.pages += 1;
        }
    }
    Ok(result)
}
