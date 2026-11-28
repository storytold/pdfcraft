//! The sampled side: images (ISO 32000-1 §8.9.5), `Indexed` lookup tables (§8.6.6.3) and axial
//! and radial shadings (§8.7.4.5.3, §8.7.4.5.4), each mapped through the space's tint transform.

use std::collections::HashMap;

use pdfcraft_cos::{Dict, ObjRef, Object, PdfString, Stream};

use super::tint::Func;
use super::{Conversion, Spaces, SpotsConverted, device_cmyk, real};

/// Linear pieces a converted shading's colour is made of.
pub const SHADING_SEGMENTS: usize = 64;
/// Largest converted image, in output bytes.
const MAX_OUTPUT: usize = 256 << 20;
/// Largest lookup table read.
const MAX_TABLE: usize = 256 * 32;
/// Distinct sample values remembered while converting one image.
const MAX_CACHE: usize = 1 << 16;

fn int(spaces: &Spaces<'_>, d: &Dict, key: &[u8]) -> Option<i64> {
    d.get(key).and_then(|o| spaces.doc.resolve(o).as_int())
}

fn numbers(spaces: &Spaces<'_>, o: Option<&Object>) -> Option<Vec<f64>> {
    let o = spaces.doc.resolve(o?);
    o.as_array().map(|a| a.iter().filter_map(|v| spaces.doc.resolve(v).as_f64()).collect())
}

fn to_byte(v: f64) -> u8 {
    (v.clamp(0.0, 1.0) * 255.0).round() as u8
}

/// An image drawn in a converted space, as 8-bit `DeviceCMYK`; `None` when its space does not
/// convert or it is refused.
pub(super) fn image(spaces: &mut Spaces<'_>, at: ObjRef, s: &Stream, report: &mut SpotsConverted) -> Option<Stream> {
    let cs = s.dict.get(b"ColorSpace")?.clone();
    let c = spaces.lookup(&cs, Some(at), report)?;
    let conv = spaces.conversions.get(c)?;
    let refuse = |report: &mut SpotsConverted, why: &str| {
        report.refuse(Some(at), conv.ink(), format!("the image {why}"));
        None
    };
    if s.dict.get(b"Mask").is_some_and(|m| spaces.doc.resolve(m).as_array().is_some()) {
        return refuse(report, "has a colour-key mask given in its spot tints");
    }
    let (Some(w), Some(h), Some(bpc)) = (int(spaces, &s.dict, b"Width"), int(spaces, &s.dict, b"Height"), int(spaces, &s.dict, b"BitsPerComponent"))
    else {
        return refuse(report, "has no readable size or bits per component");
    };
    let size = usize::try_from(w).ok().zip(usize::try_from(h).ok()).filter(|(w, h)| *w > 0 && *h > 0);
    let bpc = u32::try_from(bpc).ok().filter(|b| matches!(b, 1 | 2 | 4 | 8 | 16));
    let (Some((w, h)), Some(bpc)) = (size, bpc) else {
        return refuse(report, "has a size or bits per component that is not valid");
    };
    let Some(_) = w.checked_mul(h).and_then(|p| p.checked_mul(4)).filter(|n| *n <= MAX_OUTPUT) else {
        return refuse(report, &format!("would be larger than {} MiB as CMYK", MAX_OUTPUT >> 20));
    };
    let Ok(data) = s.decoded_within(MAX_OUTPUT) else {
        return refuse(report, "data is not decoded here (a picture codec, or damaged data)");
    };
    let decode = numbers(spaces, s.dict.get(b"Decode")).filter(|d| d.len() == 2 * conv.n).unwrap_or_else(|| [0.0, 1.0].repeat(conv.n));
    let Some(out) = convert_samples(conv, &data, (w, h, bpc), &decode) else {
        return refuse(report, "data is shorter than its size, or its tint transform fails");
    };
    let mut dict = s.dict.clone();
    for k in [&b"Length"[..], b"Filter", b"DecodeParms", b"Decode", b"DL"] {
        dict.remove(k);
    }
    dict.set(b"ColorSpace".to_vec(), device_cmyk());
    dict.set(b"BitsPerComponent".to_vec(), Object::Int(8));
    Some(Stream::flate(dict, &out))
}

/// The image's samples as 8-bit CMYK.
fn convert_samples(conv: &Conversion, data: &[u8], (w, h, bpc): (usize, usize, u32), decode: &[f64]) -> Option<Vec<u8>> {
    let n = conv.n;
    let row_bits = w.checked_mul(n)?.checked_mul(bpc as usize)?;
    let row = row_bits.div_ceil(8);
    if data.len() < row.checked_mul(h)? {
        return None;
    }
    let max = f64::from((1u32 << bpc) - 1);
    let mut cache: HashMap<Vec<u16>, [u8; 4]> = HashMap::new();
    let mut out = Vec::new();
    let mut samples = vec![0u16; n];
    let mut tints = vec![0.0; n];
    for y in 0..h {
        let line = data.get(y * row..(y + 1) * row)?;
        for x in 0..w {
            for (i, s) in samples.iter_mut().enumerate() {
                *s = sample(line, (x * n + i) * bpc as usize, bpc)?;
            }
            if let Some(px) = cache.get(&samples) {
                out.extend_from_slice(px);
                continue;
            }
            for (i, t) in tints.iter_mut().enumerate() {
                let (lo, hi) = (decode.get(2 * i).copied().unwrap_or(0.0), decode.get(2 * i + 1).copied().unwrap_or(1.0));
                *t = lo + f64::from(*samples.get(i)?) * (hi - lo) / max;
            }
            let px = conv.cmyk(&tints)?.map(to_byte);
            if cache.len() >= MAX_CACHE {
                cache.clear();
            }
            cache.insert(samples.clone(), px);
            out.extend_from_slice(&px);
        }
    }
    Some(out)
}

/// The `bpc`-bit sample starting `bit` bits into `line`.
fn sample(line: &[u8], bit: usize, bpc: u32) -> Option<u16> {
    let byte = *line.get(bit / 8)?;
    Some(match bpc {
        16 => u16::from_be_bytes([byte, *line.get(bit / 8 + 1)?]),
        8 => u16::from(byte),
        _ => {
            let shift = 8 - bpc - (bit % 8) as u32;
            u16::from((byte >> shift) & ((1u8 << bpc) - 1))
        }
    })
}

/// `[/Indexed base hival lookup]` over a converted base, as `[/Indexed /DeviceCMYK hival
/// lookup']`; `None` for any other array.
pub(super) fn indexed(spaces: &mut Spaces<'_>, items: &[Object], at: Option<ObjRef>, report: &mut SpotsConverted) -> Option<Object> {
    let doc = spaces.doc;
    if doc.resolve(items.first()?).as_name() != Some(b"Indexed") {
        return None;
    }
    let c = spaces.lookup(items.get(1)?, at, report)?;
    let conv = spaces.conversions.get(c)?;
    let refuse = |report: &mut SpotsConverted| {
        report.refuse(at, conv.ink(), "its indexed lookup table is short or its tint transform fails".into());
        None
    };
    let Some(hival) = doc.resolve(items.get(2)?).as_int().and_then(|v| usize::try_from(v).ok()).filter(|v| *v <= 255) else {
        return refuse(report);
    };
    let table = match &*doc.resolve(items.get(3)?) {
        Object::String(s) => Some(s.bytes.clone()),
        Object::Stream(s) => s.decoded_within(MAX_TABLE).ok(),
        _ => None,
    };
    let Some(table) = table else { return refuse(report) };
    let mut out = Vec::with_capacity((hival + 1) * 4);
    for entry in 0..=hival {
        let tints: Option<Vec<f64>> = table.get(entry * conv.n..(entry + 1) * conv.n).map(|b| b.iter().map(|v| f64::from(*v) / 255.0).collect());
        let Some(cmyk) = tints.and_then(|t| conv.cmyk(&t)) else { return refuse(report) };
        out.extend(cmyk.map(to_byte));
    }
    let hival = items.get(2)?.clone();
    Some(Object::Array(vec![Object::Name(b"Indexed".to_vec()), device_cmyk(), hival, Object::String(PdfString { bytes: out, hex: true })]))
}

/// Convert an axial or radial shading in place; true when it changed.
pub(super) fn shading(spaces: &mut Spaces<'_>, d: &mut Dict, at: Option<ObjRef>, report: &mut SpotsConverted) -> bool {
    let Some(cs) = d.get(b"ColorSpace").cloned() else {
        return false;
    };
    let Some(c) = spaces.lookup(&cs, at, report) else {
        return false;
    };
    let Some(conv) = spaces.conversions.get(c) else {
        return false;
    };
    let refuse = |report: &mut SpotsConverted, why: String| {
        report.refuse(at, conv.ink(), why);
        false
    };
    let kind = int(spaces, d, b"ShadingType").unwrap_or(0);
    if !matches!(kind, 2 | 3) {
        return refuse(report, format!("a type {kind} shading paints in it; only axial and radial shadings are converted"));
    }
    let functions: Result<Vec<Func>, String> = match d.get(b"Function").map(|o| spaces.doc.resolve(o)).as_deref() {
        Some(Object::Array(fs)) => fs.iter().map(|f| Func::load(spaces.doc, f)).collect(),
        Some(_) => d.get(b"Function").map(|f| Func::load(spaces.doc, f)).into_iter().collect(),
        None => Err("it has no function".into()),
    };
    let functions = match functions {
        Ok(f) => f,
        Err(e) => return refuse(report, format!("a shading's function cannot be read: {e}")),
    };
    let domain = numbers(spaces, d.get(b"Domain")).unwrap_or_else(|| vec![0.0, 1.0]);
    let (Some(&t0), Some(&t1)) = (domain.first(), domain.get(1)) else {
        return refuse(report, "a shading's domain cannot be read".into());
    };
    if !(t0.is_finite() && t1.is_finite() && t1 > t0) {
        return refuse(report, "a shading's domain is not an increasing range".into());
    }
    let Some(stitched) = stitch(&functions, conv, (t0, t1)) else {
        return refuse(report, "a shading's function cannot be evaluated".into());
    };
    if let Some(bg) = numbers(spaces, d.get(b"Background")) {
        match conv.cmyk(&bg) {
            Some(cmyk) => d.set(b"Background".to_vec(), Object::Array(cmyk.iter().map(|v| real(*v)).collect())),
            None => {
                d.remove(b"Background");
            }
        }
    }
    d.set(b"ColorSpace".to_vec(), device_cmyk());
    d.set(b"Function".to_vec(), stitched);
    true
}

/// The shading's colour at `t`: one n-output function, or n one-output functions (§8.7.4.5.3).
fn colour_at(functions: &[Func], n: usize, t: f64) -> Option<Vec<f64>> {
    let out: Vec<f64> = if let [f] = functions {
        f.eval(&[t])?
    } else {
        functions.iter().map(|f| f.eval(&[t]).and_then(|o| o.first().copied())).collect::<Option<_>>()?
    };
    (out.len() == n).then_some(out)
}

/// A stitching function (§7.10.4) of [`SHADING_SEGMENTS`] linear pieces over `t0..t1`, through
/// the shading's colour mapped to CMYK.
fn stitch(functions: &[Func], conv: &Conversion, (t0, t1): (f64, f64)) -> Option<Object> {
    let reals = |v: &[f64]| Object::Array(v.iter().map(|x| real(*x)).collect());
    let at = |i: usize| t0 + (t1 - t0) * i as f64 / SHADING_SEGMENTS as f64;
    let points: Vec<[f64; 4]> =
        (0..=SHADING_SEGMENTS).map(|i| colour_at(functions, conv.n, at(i)).and_then(|c| conv.cmyk(&c))).collect::<Option<_>>()?;
    let mut pieces = Vec::with_capacity(SHADING_SEGMENTS);
    for pair in points.windows(2) {
        let [c0, c1] = pair else { continue };
        let mut piece = Dict::new();
        piece.set(b"FunctionType".to_vec(), Object::Int(2));
        piece.set(b"Domain".to_vec(), reals(&[0.0, 1.0]));
        piece.set(b"C0".to_vec(), reals(c0));
        piece.set(b"C1".to_vec(), reals(c1));
        piece.set(b"N".to_vec(), Object::Int(1));
        pieces.push(Object::Dict(piece));
    }
    let bounds: Vec<f64> = (1..SHADING_SEGMENTS).map(at).collect();
    let mut f = Dict::new();
    f.set(b"FunctionType".to_vec(), Object::Int(3));
    f.set(b"Domain".to_vec(), Object::Array(vec![Object::Real(t0), Object::Real(t1)]));
    f.set(b"Functions".to_vec(), Object::Array(pieces));
    f.set(b"Bounds".to_vec(), Object::Array(bounds.into_iter().map(Object::Real).collect()));
    f.set(b"Encode".to_vec(), reals(&[0.0, 1.0].repeat(SHADING_SEGMENTS)));
    Some(Object::Dict(f))
}
