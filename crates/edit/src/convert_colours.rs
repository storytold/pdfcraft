//! Convert colours (Acrobat's Print production ▸ Convert Colors): every colour
//! operator in the page content and the artwork it draws is rewritten in a target space -
//! grayscale, RGB or CMYK - by the standard device formulas (no ICC profile is applied), black
//! preserved on request; pictures are re-encoded through a helper the caller supplies.

use pdfcraft_content::{Op, parse, serialize_ops};
use pdfcraft_cos::{Dict, Document, ObjRef, Object, Stream};
use std::collections::HashSet;

use crate::colour::{Space, rgb, space_of};
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

fn r3(v: f64) -> Object {
    Object::Real((v * 1000.0).round() / 1000.0)
}

/// Rewrite one content stream's colour operators. `cs_res` is its `/ColorSpace` resource
/// dictionary. Returns the new bytes when anything changed, and how many operators.
fn rewrite(data: &[u8], doc: &Document, cs_res: &Dict, settings: &ConvertColours) -> (Option<Vec<u8>>, usize) {
    let mut ops: Vec<Op> = parse(data).ops;
    let mut count = 0usize;
    let mut out: Vec<Op> = Vec::with_capacity(ops.len());
    // The current fill and stroke spaces (None = a device space set by g/rg/k, which the
    // converted operator sets again), with the q/Q stack.
    let mut fill: Option<Space> = None;
    let mut stroke: Option<Space> = None;
    let mut stack: Vec<(Option<Space>, Option<Space>)> = Vec::new();
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
                    fill = f;
                    stroke = s;
                }
                out.push(op);
            }
            b"cs" | b"CS" => {
                let is_stroke = op.op.as_slice() == b"CS";
                let space = op.operands.first().map(|o| space_of(doc, o, cs_res, 0)).unwrap_or(Space::Gray);
                let pattern = space == Space::Pattern;
                if is_stroke {
                    stroke = Some(space);
                } else {
                    fill = Some(space);
                }
                if pattern {
                    // Patterns stay what they are (their own content is not converted here).
                    out.push(op);
                } else {
                    // The space's initial colour, now in the target: black (a tint at full).
                    let space = if is_stroke { stroke.clone() } else { fill.clone() }.unwrap_or(Space::Gray);
                    let initial = match space {
                        Space::Tint { .. } => rgb(&space, &[1.0; 8]),
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
                    stroke = None;
                } else {
                    fill = None;
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

/// Convert the XObjects of `resources`: forms rewritten (and their own XObjects after them),
/// images through `recolour`. `done` keeps each object to once.
fn convert_xobjects(
    doc: &mut Document,
    resources: Option<&Dict>,
    settings: &ConvertColours,
    recolour: &mut ImageRecolour<'_>,
    done: &mut HashSet<ObjRef>,
    result: &mut Converted,
) {
    let Some(xo) = resources.and_then(|r| r.get(b"XObject")).map(|x| doc.resolve(x)).and_then(|x| x.as_dict().cloned()) else { return };
    let refs: Vec<ObjRef> = xo.iter().filter_map(|(_, o)| o.as_ref()).collect();
    for r in refs {
        if !done.insert(r) {
            continue;
        }
        let Some(s) = stream_of(doc, r) else { continue };
        match s.dict.name(b"Subtype") {
            Some(b"Form") => {
                let res = s.dict.get(b"Resources").map(|x| doc.resolve(x)).and_then(|x| x.as_dict().cloned());
                let cs = cs_resources(doc, res.as_ref());
                if let Ok(data) = s.decoded() {
                    let (new, n) = rewrite(&data, doc, &cs, settings);
                    if let Some(bytes) = new {
                        let mut dict = s.dict.clone();
                        for k in [&b"Length"[..], b"Filter", b"DecodeParms"] {
                            dict.remove(k);
                        }
                        doc.set(r, Object::Stream(Stream::flate(dict, &bytes)));
                        result.colours += n;
                    }
                }
                convert_xobjects(doc, res.as_ref(), settings, recolour, done, result);
            }
            Some(b"Image") if settings.images => {
                if let Some(new) = recolour(doc, &s, settings.target) {
                    doc.set(r, Object::Stream(new));
                    result.images += 1;
                }
            }
            _ => {}
        }
    }
}

/// Convert the colours on `pages` (0-based).
pub fn convert_colours(
    doc: &mut Document,
    pages: &[usize],
    settings: &ConvertColours,
    recolour: &mut ImageRecolour<'_>,
) -> Result<Converted, EditError> {
    let all = page_list(doc);
    check(pages, all.len())?;
    let mut result = Converted::default();
    let mut done: HashSet<ObjRef> = HashSet::new();
    for &i in pages {
        let page = page_list(doc)[i].clone();
        let resources = page.dict.get(b"Resources").map(|r| doc.resolve(r)).and_then(|r| r.as_dict().cloned());
        let cs = cs_resources(doc, resources.as_ref());
        let before = (result.colours, result.images);
        let entries: Vec<ObjRef> = match page.dict.get(b"Contents").map(|c| doc.resolve(c)) {
            Some(c) => match &*c {
                Object::Array(a) => a.iter().filter_map(|e| e.as_ref()).collect(),
                _ => page.dict.get(b"Contents").and_then(|c| c.as_ref()).into_iter().collect(),
            },
            None => Vec::new(),
        };
        let joined: Vec<u8> = entries
            .iter()
            .filter_map(|r| stream_of(doc, *r).and_then(|s| s.decoded().ok()))
            .flat_map(|d| {
                let mut v = d;
                v.push(b'\n');
                v
            })
            .collect();
        let (new, n) = rewrite(&joined, doc, &cs, settings);
        if let Some(bytes) = new {
            let stream = doc.add(Object::Stream(Stream::flate(Dict::new(), &bytes)));
            doc.update_dict(page.obj, |d| d.set(b"Contents".to_vec(), Object::Ref(stream)))?;
            result.colours += n;
        }
        convert_xobjects(doc, resources.as_ref(), settings, recolour, &mut done, &mut result);
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
                convert_xobjects(doc, Some(&fake), settings, recolour, &mut done, &mut result);
            }
        }
        if (result.colours, result.images) != before {
            result.pages += 1;
        }
    }
    Ok(result)
}
