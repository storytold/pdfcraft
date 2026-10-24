//! Colours as sRGB through the colour space in force, for Convert colours: `g`/`rg`/`k`, and
//! `cs` + `sc`/`scn` through the space named. Device spaces, ICCBased by its component count (or
//! its Alternate), CalRGB and CalGray as their device kin, Lab through CIE XYZ with the
//! dictionary's white point, Indexed through its table, Separation and DeviceN through a Type 2
//! (exponential) tint transform into the alternate space; a sampled or PostScript tint function,
//! or a pattern, falls back to a plain darkness so nothing reads as the wrong hue.

use pdfcraft_cos::{Dict, Document, Object};

/// A colour space as far as a fill colour needs it.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Space {
    Gray,
    Rgb,
    Cmyk,
    Lab {
        white: [f64; 3],
        range: [f64; 4],
    },
    Indexed {
        base: Box<Space>,
        hival: usize,
        table: Vec<u8>,
    },
    /// Separation and DeviceN: the number of inputs, the alternate space, and the tint
    /// transform when it is a Type 2 function (C0, C1, N).
    Tint {
        inputs: usize,
        alt: Box<Space>,
        exp: Option<(Vec<f64>, Vec<f64>, f64)>,
    },
    Pattern,
}

impl Space {
    /// How many numbers set a colour in this space.
    fn components(&self) -> usize {
        match self {
            Space::Gray => 1,
            Space::Rgb | Space::Lab { .. } => 3,
            Space::Cmyk => 4,
            Space::Indexed { .. } => 1,
            Space::Tint { inputs, .. } => *inputs,
            Space::Pattern => 0,
        }
    }
}

fn nums(doc: &Document, o: &Object) -> Vec<f64> {
    doc.resolve(o).as_array().map(|a| a.iter().filter_map(|v| doc.resolve(v).as_f64()).collect()).unwrap_or_default()
}

/// The colour space an operand of `cs` names: a device space, or a name in the resources'
/// `/ColorSpace` dictionary, or (inside arrays) a family.
pub(crate) fn space_of(doc: &Document, operand: &Object, cs_res: &Dict, depth: u8) -> Space {
    if depth > 4 {
        return Space::Gray;
    }
    let resolved = doc.resolve(operand);
    match &*resolved {
        Object::Name(n) => match n.as_slice() {
            b"DeviceGray" | b"G" | b"CalGray" => Space::Gray,
            b"DeviceRGB" | b"RGB" | b"CalRGB" => Space::Rgb,
            b"DeviceCMYK" | b"CMYK" => Space::Cmyk,
            b"Pattern" => Space::Pattern,
            other => match cs_res.get(other) {
                Some(o) => space_of(doc, o, &Dict::default(), depth + 1),
                None => Space::Gray,
            },
        },
        Object::Array(a) => {
            let family = a.first().map(|f| doc.resolve(f)).and_then(|f| f.as_name().map(|n| n.to_vec())).unwrap_or_default();
            match family.as_slice() {
                b"ICCBased" => {
                    let stream = a.get(1).map(|s| doc.resolve(s));
                    let dict = stream.as_ref().and_then(|s| match &**s {
                        Object::Stream(st) => Some(st.dict.clone()),
                        _ => None,
                    });
                    if let Some(alt) = dict.as_ref().and_then(|d| d.get(b"Alternate").cloned()) {
                        return space_of(doc, &alt, cs_res, depth + 1);
                    }
                    match dict.and_then(|d| d.int(b"N")) {
                        Some(1) => Space::Gray,
                        Some(4) => Space::Cmyk,
                        _ => Space::Rgb,
                    }
                }
                b"CalRGB" => Space::Rgb,
                b"CalGray" => Space::Gray,
                b"Lab" => {
                    let d = a.get(1).map(|d| doc.resolve(d)).and_then(|d| d.as_dict().cloned()).unwrap_or_default();
                    let wp = d.get(b"WhitePoint").map(|w| nums(doc, w)).unwrap_or_default();
                    let white = if wp.len() == 3 { [wp[0], wp[1], wp[2]] } else { [0.9505, 1.0, 1.089] };
                    let r = d.get(b"Range").map(|w| nums(doc, w)).unwrap_or_default();
                    let range = if r.len() == 4 { [r[0], r[1], r[2], r[3]] } else { [-100.0, 100.0, -100.0, 100.0] };
                    Space::Lab { white, range }
                }
                b"Indexed" | b"I" => {
                    let base = a.get(1).map(|b| space_of(doc, b, cs_res, depth + 1)).unwrap_or(Space::Rgb);
                    let hival = a.get(2).and_then(|h| doc.resolve(h).as_f64()).unwrap_or(0.0).clamp(0.0, 255.0) as usize;
                    let table = match a.get(3).map(|t| doc.resolve(t)) {
                        Some(t) => match &*t {
                            Object::String(s) => s.bytes.clone(),
                            Object::Stream(st) => st.decoded().unwrap_or_default(),
                            _ => Vec::new(),
                        },
                        None => Vec::new(),
                    };
                    Space::Indexed { base: Box::new(base), hival, table }
                }
                b"Separation" | b"DeviceN" => {
                    let inputs = if family == b"Separation" {
                        1
                    } else {
                        a.get(1).map(|n| doc.resolve(n)).and_then(|n| n.as_array().map(Vec::len)).unwrap_or(1).max(1)
                    };
                    let alt = a.get(2).map(|b| space_of(doc, b, cs_res, depth + 1)).unwrap_or(Space::Gray);
                    let exp = a.get(3).map(|f| doc.resolve(f)).and_then(|f| {
                        let d = match &*f {
                            Object::Dict(d) => d.clone(),
                            Object::Stream(s) => s.dict.clone(),
                            _ => return None,
                        };
                        if d.int(b"FunctionType") != Some(2) {
                            return None;
                        }
                        let c0 = d.get(b"C0").map(|v| nums(doc, v)).unwrap_or_else(|| vec![0.0]);
                        let c1 = d.get(b"C1").map(|v| nums(doc, v)).unwrap_or_else(|| vec![1.0]);
                        let n = d.get(b"N").and_then(|v| doc.resolve(v).as_f64()).unwrap_or(1.0);
                        Some((c0, c1, n))
                    });
                    Space::Tint { inputs, alt: Box::new(alt), exp }
                }
                b"Pattern" => Space::Pattern,
                b"DeviceGray" | b"G" => Space::Gray,
                b"DeviceRGB" | b"RGB" => Space::Rgb,
                b"DeviceCMYK" | b"CMYK" => Space::Cmyk,
                _ => Space::Gray,
            }
        }
        _ => Space::Gray,
    }
}

fn clamp01(v: f64) -> f64 {
    if v.is_finite() { v.clamp(0.0, 1.0) } else { 0.0 }
}

/// CMYK to sRGB, the usual multiplicative approximation (no ICC profile).
pub(crate) fn cmyk(c: f64, m: f64, y: f64, k: f64) -> [f64; 3] {
    let (c, m, y, k) = (clamp01(c), clamp01(m), clamp01(y), clamp01(k));
    [(1.0 - c) * (1.0 - k), (1.0 - m) * (1.0 - k), (1.0 - y) * (1.0 - k)]
}

/// CIE L*a*b* (with the dictionary's white point) to sRGB, through XYZ and the D65 sRGB matrix.
fn lab(white: [f64; 3], l: f64, a: f64, b: f64) -> [f64; 3] {
    let m = (l + 16.0) / 116.0;
    let (ll, mm, nn) = (m + a / 500.0, m, m - b / 200.0);
    let g = |x: f64| if x >= 6.0 / 29.0 { x * x * x } else { 108.0 / 841.0 * (x - 4.0 / 29.0) };
    let (xw, yw, zw) = (white[0].max(1e-6), white[1].max(1e-6), white[2].max(1e-6));
    let (x, y, z) = (xw * g(ll), yw * g(mm), zw * g(nn));
    // Scale the white point to D65 before the sRGB matrix (a plain von Kries over XYZ).
    let (x, y, z) = (x * 0.9505 / xw, y / yw, z * 1.089 / zw);
    let lin = [3.2406 * x - 1.5372 * y - 0.4986 * z, -0.9689 * x + 1.8758 * y + 0.0415 * z, 0.0557 * x - 0.2040 * y + 1.0570 * z];
    lin.map(|c| {
        let c = clamp01(c);
        if c <= 0.0031308 { 12.92 * c } else { 1.055 * c.powf(1.0 / 2.4) - 0.055 }
    })
}

/// The sRGB of `values` in `space` (missing values read as 0; a pattern reads as black).
pub(crate) fn rgb(space: &Space, values: &[f64]) -> [f64; 3] {
    let v = |i: usize| values.get(i).copied().unwrap_or(0.0);
    match space {
        Space::Gray => {
            let g = clamp01(v(0));
            [g, g, g]
        }
        Space::Rgb => [clamp01(v(0)), clamp01(v(1)), clamp01(v(2))],
        Space::Cmyk => cmyk(v(0), v(1), v(2), v(3)),
        Space::Lab { white, range } => {
            let l = v(0).clamp(0.0, 100.0);
            let a = v(1).clamp(range[0].min(range[1]), range[1].max(range[0]));
            let b = v(2).clamp(range[2].min(range[3]), range[3].max(range[2]));
            lab(*white, l, a, b)
        }
        Space::Indexed { base, hival, table } => {
            let n = base.components().max(1);
            let i = (v(0).max(0.0).round() as usize).min(*hival);
            let vals: Vec<f64> = (0..n).map(|k| table.get(i * n + k).map(|b| *b as f64 / 255.0).unwrap_or(0.0)).collect();
            // Lab tables hold the L*, a*, b* range in bytes; the others are 0..1 already.
            match &**base {
                Space::Lab { range, .. } => {
                    let scaled = [
                        vals[0] * 100.0,
                        range[0] + vals.get(1).copied().unwrap_or(0.0) * (range[1] - range[0]),
                        range[2] + vals.get(2).copied().unwrap_or(0.0) * (range[3] - range[2]),
                    ];
                    rgb(base, &scaled)
                }
                _ => rgb(base, &vals),
            }
        }
        Space::Tint { inputs, alt, exp } => match exp {
            Some((c0, c1, n)) => {
                let t = clamp01(v(0));
                let k = if *n == 1.0 { t } else { t.powf(*n) };
                let out: Vec<f64> = c0.iter().zip(c1.iter()).map(|(a, b)| a + k * (b - a)).collect();
                rgb(alt, &out)
            }
            // No usable transform: the darkness of the strongest tint, in the alternate space's ink.
            None => {
                let t = (0..*inputs).map(v).fold(0.0, f64::max);
                let g = 1.0 - clamp01(t);
                [g, g, g]
            }
        },
        Space::Pattern => [0.0, 0.0, 0.0],
    }
}
