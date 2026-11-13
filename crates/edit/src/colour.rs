//! Colours as sRGB through the colour space in force, for Convert colours: `g`/`rg`/`k`, and
//! `cs` + `sc`/`scn` through the space named. Device spaces, ICCBased by its component count (or
//! its Alternate), CalRGB and CalGray as their device kin, Lab through CIE XYZ with the
//! dictionary's white point, Indexed through its table, Separation and DeviceN through a Type 2
//! (exponential) tint transform into the alternate space; a sampled or PostScript tint function,
//! or a pattern, falls back to a plain darkness so nothing reads as the wrong hue.

use pdfcraft_cos::{DecodeBudget, Dict, Document, Object, Stream};

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

/// The first `max` numbers of an array: a space is read once for each resource name naming it,
/// so it keeps only the numbers its colours read, however long the array in the file.
fn nums_upto(doc: &Document, o: &Object, max: usize) -> Vec<f64> {
    doc.resolve(o).as_array().map(|a| a.iter().filter_map(|v| doc.resolve(v).as_f64()).take(max).collect()).unwrap_or_default()
}

/// The most bytes one Indexed lookup table is decoded to when text colours are read. A table
/// holds `hival + 1` entries of the base space's components (ISO 32000-2 §8.6.6.3): at most 256
/// entries of a DeviceN space's 32 colourants, 8 KiB, so this leaves room for padding.
pub(crate) const MAX_LOOKUP_DECODED: usize = 64 << 10;

/// The most bytes all the Indexed lookup tables one page's text colours select are decoded to,
/// together: a thousand RGB tables, where a form drawn again and again, or many names for one
/// table, would otherwise decode the same table each time.
pub(crate) const MAX_PAGE_LOOKUPS: usize = 1 << 20;

/// What Indexed lookup tables may still cost: `left` bytes in all, at most `cap` for one table.
/// A stream is charged what its filters decoded, and one past either limit reads as empty, so
/// its colours read as black (see [`rgb`]). A byte string is charged the part of it kept.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Lookups {
    left: usize,
    cap: usize,
}

impl Lookups {
    /// One page's allowance (see [`MAX_PAGE_LOOKUPS`]).
    pub(crate) fn page() -> Self {
        Lookups { left: MAX_PAGE_LOOKUPS, cap: MAX_LOOKUP_DECODED }
    }

    /// No allowance beyond [`Stream::decoded`]'s own limit, for each table alone.
    fn per_table() -> Self {
        Lookups { left: usize::MAX, cap: pdfcraft_cos::MAX_PAGE_DECODED }
    }

    /// The lookup table in `stream`, charged to the allowance what its filters decoded: all it
    /// was allowed when it ran past its limit, and only the work done when it failed sooner, so
    /// a few small damaged tables don't spend what the page's other tables need. Only its first
    /// `needed` bytes are kept.
    fn decode(&mut self, stream: &Stream, needed: usize) -> Vec<u8> {
        let max = self.cap.min(self.left);
        if max == 0 {
            return Vec::new();
        }
        // One byte more than `max`, so the budget holds the decode to `max` itself and charges
        // it exactly (see `DecodeBudget::decode_within`).
        let allowed = max.saturating_add(1);
        let mut budget = DecodeBudget::new(allowed, 0);
        let decoded = budget.decode_within(stream, max);
        self.left = self.left.saturating_sub(allowed.saturating_sub(budget.remaining()));
        match decoded {
            Ok(mut table) => {
                table.truncate(needed);
                table
            }
            Err(_) => Vec::new(),
        }
    }

    /// The first `needed` bytes of a lookup table given as a byte string, as far as the allowance
    /// still goes, charged to it. The string is in memory already, but each space read from it
    /// keeps a copy, so many names for one padded string would otherwise keep a full copy each.
    fn keep(&mut self, string: &[u8], needed: usize) -> Vec<u8> {
        let len = string.len().min(needed).min(self.cap).min(self.left);
        self.left = self.left.saturating_sub(len);
        string.get(..len).map(<[u8]>::to_vec).unwrap_or_default()
    }

    /// What the allowance still holds, for the tests of what a table was charged.
    #[cfg(test)]
    pub(crate) fn left(&self) -> usize {
        self.left
    }
}

/// The colour space an operand of `cs` names: a device space, or a name in the resources'
/// `/ColorSpace` dictionary, or (inside arrays) a family.
pub(crate) fn space_of(doc: &Document, operand: &Object, cs_res: &Dict, depth: u8) -> Space {
    space_within(doc, operand, cs_res, depth, &mut Lookups::per_table())
}

/// [`space_of`], with the Indexed lookup streams it decodes charged to `lookups`. What can't be
/// read falls back: an unknown name or family to DeviceGray, an ICCBased stream without `/N` or
/// a usable `/Alternate` to DeviceRGB, an Indexed space without a base over DeviceRGB.
pub(crate) fn space_within(doc: &Document, operand: &Object, cs_res: &Dict, depth: u8, lookups: &mut Lookups) -> Space {
    read_space(doc, operand, cs_res, depth, lookups, false).unwrap_or(Space::Gray)
}

/// The space an operand names when it can be read, `None` where [`space_within`] would fall
/// back: a name the resources don't hold, a family that isn't one, an ICCBased stream without
/// `/N` or a usable `/Alternate`, an Indexed space without a readable base, nesting past the
/// limit. Text colours read their operands by count for such a space, as a reader that can't
/// find it would.
pub(crate) fn readable_space(doc: &Document, operand: &Object, cs_res: &Dict, depth: u8, lookups: &mut Lookups) -> Option<Space> {
    read_space(doc, operand, cs_res, depth, lookups, true)
}

/// [`space_within`] (`strict` false: never `None`) and [`readable_space`] (`strict` true).
fn read_space(doc: &Document, operand: &Object, cs_res: &Dict, depth: u8, lookups: &mut Lookups, strict: bool) -> Option<Space> {
    // What a space that can't be read is, when not strict.
    let fallback = |space: Space| if strict { None } else { Some(space) };
    if depth > 4 {
        return fallback(Space::Gray);
    }
    let resolved = doc.resolve(operand);
    match &*resolved {
        Object::Name(n) => match n.as_slice() {
            b"DeviceGray" | b"G" | b"CalGray" => Some(Space::Gray),
            b"DeviceRGB" | b"RGB" | b"CalRGB" => Some(Space::Rgb),
            b"DeviceCMYK" | b"CMYK" => Some(Space::Cmyk),
            b"Pattern" => Some(Space::Pattern),
            other => match cs_res.get(other) {
                Some(o) => read_space(doc, o, &Dict::default(), depth + 1, lookups, strict),
                None => fallback(Space::Gray),
            },
        },
        Object::Array(a) => {
            let family = a.first().map(|f| doc.resolve(f)).and_then(|f| f.as_name().map(|n| n.to_vec())).unwrap_or_default();
            match family.as_slice() {
                b"ICCBased" => {
                    // The stream's dictionary is read where it is, not copied for each name.
                    let stream = a.get(1).map(|s| doc.resolve(s));
                    let dict = stream.as_ref().and_then(|s| match &**s {
                        Object::Stream(st) => Some(&st.dict),
                        _ => None,
                    });
                    if let Some(alt) = dict.and_then(|d| d.get(b"Alternate")) {
                        let alt = read_space(doc, alt, cs_res, depth + 1, lookups, strict);
                        // Not strict, the alternate is what the space is, readable or not; strict,
                        // an unreadable one leaves `/N` to say.
                        if alt.is_some() || !strict {
                            return alt;
                        }
                    }
                    // ISO 32000-2 §8.6.5.5: `/N` is 1, 3 or 4.
                    match dict.and_then(|d| d.int(b"N")) {
                        Some(1) => Some(Space::Gray),
                        Some(3) => Some(Space::Rgb),
                        Some(4) => Some(Space::Cmyk),
                        _ => fallback(Space::Rgb),
                    }
                }
                b"CalRGB" => Some(Space::Rgb),
                b"CalGray" => Some(Space::Gray),
                b"Lab" => {
                    // Only the numbers a white point and a range hold are read, however long the
                    // arrays are: the space is read once for each name naming it.
                    let d = a.get(1).map(|d| doc.resolve(d));
                    let d = d.as_ref().and_then(|d| d.as_dict());
                    let wp = d.and_then(|d| d.get(b"WhitePoint")).map(|w| nums_upto(doc, w, 4)).unwrap_or_default();
                    let white = match wp.as_slice() {
                        &[x, y, z] => [x, y, z],
                        _ => [0.9505, 1.0, 1.089],
                    };
                    let r = d.and_then(|d| d.get(b"Range")).map(|w| nums_upto(doc, w, 5)).unwrap_or_default();
                    let range = match r.as_slice() {
                        &[a0, a1, b0, b1] => [a0, a1, b0, b1],
                        _ => [-100.0, 100.0, -100.0, 100.0],
                    };
                    Some(Space::Lab { white, range })
                }
                b"Indexed" | b"I" => {
                    let base = match a.get(1) {
                        Some(b) => read_space(doc, b, cs_res, depth + 1, lookups, strict)?,
                        None => fallback(Space::Rgb)?,
                    };
                    let hival = a.get(2).and_then(|h| doc.resolve(h).as_f64()).unwrap_or(0.0).clamp(0.0, 255.0) as usize;
                    // The table holds `hival + 1` entries of the base's components (ISO 32000-2
                    // §8.6.6.3); bytes past them are never read, so they aren't kept.
                    let needed = hival.saturating_add(1).saturating_mul(base.components().max(1));
                    let table = match a.get(3).map(|t| doc.resolve(t)) {
                        Some(t) => match &*t {
                            Object::String(s) => lookups.keep(&s.bytes, needed),
                            Object::Stream(st) => lookups.decode(st, needed),
                            _ => Vec::new(),
                        },
                        None => Vec::new(),
                    };
                    Some(Space::Indexed { base: Box::new(base), hival, table })
                }
                b"Separation" | b"DeviceN" => {
                    // The `/None` colorant paints nothing (ISO 32000-2, 8.6.6.4): given a colour
                    // of its own it would appear, so it stays what it is, as a pattern does.
                    let is_none = |o: &Object| doc.resolve(o).as_name() == Some(b"None");
                    let none = a.get(1).is_some_and(|names| {
                        is_none(names) || doc.resolve(names).as_array().is_some_and(|names| !names.is_empty() && names.iter().all(is_none))
                    });
                    if none {
                        return Some(Space::Pattern);
                    }
                    let inputs = if family == b"Separation" {
                        1
                    } else {
                        a.get(1).map(|n| doc.resolve(n)).and_then(|n| n.as_array().map(Vec::len)).unwrap_or(1).max(1)
                    };
                    // A missing alternate reads as DeviceGray: the tint is then the ink's darkness.
                    let alt = match a.get(2) {
                        Some(b) => read_space(doc, b, cs_res, depth + 1, lookups, strict)?,
                        None => Space::Gray,
                    };
                    // The transform's outputs are the alternate space's components (ISO 32000-2
                    // §8.6.6.4, §7.10.3): C0 and C1 past them are never read, so they aren't kept.
                    let outputs = alt.components().max(1);
                    let exp = a.get(3).map(|f| doc.resolve(f)).and_then(|f| {
                        let d = match &*f {
                            Object::Dict(d) => d,
                            Object::Stream(s) => &s.dict,
                            _ => return None,
                        };
                        if d.int(b"FunctionType") != Some(2) {
                            return None;
                        }
                        let c0 = d.get(b"C0").map(|v| nums_upto(doc, v, outputs)).unwrap_or_else(|| vec![0.0]);
                        let c1 = d.get(b"C1").map(|v| nums_upto(doc, v, outputs)).unwrap_or_else(|| vec![1.0]);
                        let n = d.get(b"N").and_then(|v| doc.resolve(v).as_f64()).unwrap_or(1.0);
                        Some((c0, c1, n))
                    });
                    Some(Space::Tint { inputs, alt: Box::new(alt), exp })
                }
                b"Pattern" => Some(Space::Pattern),
                b"DeviceGray" | b"G" => Some(Space::Gray),
                b"DeviceRGB" | b"RGB" => Some(Space::Rgb),
                b"DeviceCMYK" | b"CMYK" => Some(Space::Cmyk),
                _ => fallback(Space::Gray),
            }
        }
        _ => fallback(Space::Gray),
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
            // A table without this entry (too short, or not decoded: see `Lookups`) gives black
            // whatever its base: zeros read in a CMYK base would be white.
            let Some(entry) = table.get(i.saturating_mul(n)..i.saturating_add(1).saturating_mul(n)) else {
                return [0.0, 0.0, 0.0];
            };
            let vals: Vec<f64> = entry.iter().map(|b| f64::from(*b) / 255.0).collect();
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
                // The values given, not the colourant count: a long `/DeviceN` names array
                // doesn't make each colour cost a step per name.
                let t = values.iter().take(*inputs).copied().fold(0.0, f64::max);
                let g = 1.0 - clamp01(t);
                [g, g, g]
            }
        },
        Space::Pattern => [0.0, 0.0, 0.0],
    }
}
