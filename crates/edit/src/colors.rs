//! Print production ▸ Convert colors: page content colours to grayscale or CMYK.
//!
//! Device colours set by the colour operators (ISO 32000-2 §8.6.8: `g`/`G`, `rg`/`RG`, `k`/`K`,
//! and `cs`/`CS` with a device colour space followed by `sc`/`scn`/`SC`/`SCN`) are rewritten in
//! the target space. The conversions are the simple device formulas, not a colour-managed
//! (ICC) conversion: luminance `0.3·R + 0.59·G + 0.11·B` for grey, and full grey-component
//! replacement for CMYK (`K = 1 − max(R, G, B)`). Named colour spaces (ICC-based, spot colours,
//! patterns), shadings, form XObjects and annotation appearances are left as they are; images
//! are converted by the optimize crate.

use pdfcraft_content::{Op, Pieces, num};
use pdfcraft_cos::{Document, Object};

use crate::EditError;

/// What Convert colors converts to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ColorTarget {
    Gray,
    Cmyk,
}

impl ColorTarget {
    pub fn key(self) -> &'static str {
        match self {
            ColorTarget::Gray => "gray",
            ColorTarget::Cmyk => "cmyk",
        }
    }

    pub fn from_key(s: &str) -> Option<Self> {
        match s.to_ascii_lowercase().as_str() {
            "gray" | "grey" | "grayscale" | "greyscale" => Some(ColorTarget::Gray),
            "cmyk" => Some(ColorTarget::Cmyk),
            _ => None,
        }
    }
}

fn clamp(v: f64) -> f64 {
    if v.is_finite() { v.clamp(0.0, 1.0) } else { 0.0 }
}

/// A converted component, clamped and rounded to 6 places (so white stays exactly 1).
fn tidy(v: f64) -> f64 {
    (clamp(v) * 1e6).round() / 1e6
}

/// RGB to grey (luminance).
pub fn rgb_to_gray(r: f64, g: f64, b: f64) -> f64 {
    tidy(0.3 * clamp(r) + 0.59 * clamp(g) + 0.11 * clamp(b))
}

/// RGB to CMYK with full grey-component replacement.
pub fn rgb_to_cmyk(r: f64, g: f64, b: f64) -> [f64; 4] {
    let (r, g, b) = (clamp(r), clamp(g), clamp(b));
    let k = 1.0 - r.max(g).max(b);
    if k >= 1.0 {
        return [0.0, 0.0, 0.0, 1.0];
    }
    let d = 1.0 - k;
    [(1.0 - r - k) / d, (1.0 - g - k) / d, (1.0 - b - k) / d, k].map(tidy)
}

/// CMYK to grey.
pub fn cmyk_to_gray(c: f64, m: f64, y: f64, k: f64) -> f64 {
    tidy(1.0 - (0.3 * clamp(c) + 0.59 * clamp(m) + 0.11 * clamp(y) + clamp(k)).min(1.0))
}

/// A device colour space by its components, or `None` for any other space.
fn device(name: &[u8]) -> Option<usize> {
    match name {
        b"DeviceGray" | b"G" => Some(1),
        b"DeviceRGB" | b"RGB" => Some(3),
        b"DeviceCMYK" | b"CMYK" => Some(4),
        _ => None,
    }
}

/// A colour of `n` components in the target space: the operator suffix's components.
fn convert(v: &[f64], target: ColorTarget) -> Option<Vec<f64>> {
    Some(match (v, target) {
        ([g], ColorTarget::Gray) => vec![clamp(*g)],
        ([r, g, b], ColorTarget::Gray) => vec![rgb_to_gray(*r, *g, *b)],
        ([c, m, y, k], ColorTarget::Gray) => vec![cmyk_to_gray(*c, *m, *y, *k)],
        ([g], ColorTarget::Cmyk) => vec![0.0, 0.0, 0.0, 1.0 - clamp(*g)],
        ([r, g, b], ColorTarget::Cmyk) => rgb_to_cmyk(*r, *g, *b).to_vec(),
        ([c, m, y, k], ColorTarget::Cmyk) => vec![clamp(*c), clamp(*m), clamp(*y), clamp(*k)],
        _ => return None,
    })
}

fn nums(op: &Op) -> Option<Vec<f64>> {
    op.operands.iter().map(Object::as_f64).collect()
}

/// The operators to put in place of `op`, or `None` to keep it. `fill`/`stroke` track the
/// current device colour space (`Some(n)`) or a non-device one (`None`).
fn rewrite(op: &Op, target: ColorTarget, fill: &mut Option<usize>, stroke: &mut Option<usize>) -> Option<Vec<Op>> {
    let (set_op, space_op, space) = match target {
        ColorTarget::Gray => (["g", "G"], ["cs", "CS"], "DeviceGray"),
        ColorTarget::Cmyk => (["k", "K"], ["cs", "CS"], "DeviceCMYK"),
    };
    let out_n = if target == ColorTarget::Gray { 1 } else { 4 };
    let colour = |v: Vec<f64>, stroking: bool| -> Vec<Op> { vec![Op::new(set_op[usize::from(stroking)], v.into_iter().map(num).collect())] };
    match op.op.as_slice() {
        b"g" | b"rg" | b"k" | b"G" | b"RG" | b"K" => {
            let stroking = op.op.first().is_some_and(u8::is_ascii_uppercase);
            let n = match op.op.as_slice() {
                b"g" | b"G" => 1,
                b"rg" | b"RG" => 3,
                _ => 4,
            };
            // These operators also set the device colour space.
            if stroking {
                *stroke = Some(out_n);
            } else {
                *fill = Some(out_n);
            }
            let v = nums(op)?;
            if v.len() != n || n == out_n {
                return None;
            }
            Some(colour(convert(&v, target)?, stroking))
        }
        b"cs" | b"CS" => {
            let stroking = op.is("CS");
            let n = op.operands.first().and_then(Object::as_name).and_then(device);
            let slot = if stroking { stroke } else { fill };
            match n {
                Some(n) if n != out_n => {
                    *slot = Some(n);
                    Some(vec![Op::new(space_op[usize::from(stroking)], vec![Object::name(space)])])
                }
                Some(n) => {
                    *slot = Some(n);
                    None
                }
                None => {
                    *slot = None;
                    None
                }
            }
        }
        b"sc" | b"scn" | b"SC" | b"SCN" => {
            let stroking = op.op.first().is_some_and(u8::is_ascii_uppercase);
            let n = (if stroking { *stroke } else { *fill })?;
            if n == out_n {
                return None;
            }
            let v = nums(op)?;
            if v.len() != n {
                return None;
            }
            let v = convert(&v, target)?;
            Some(vec![Op::new(if stroking { "SC" } else { "sc" }, v.into_iter().map(num).collect())])
        }
        _ => None,
    }
}

/// The rewrites of `ops` (operator index → replacement). After a rewritten `cs`, the content is
/// in the target space but its `sc` operands are still in the old one, so the tracked space keeps
/// the old component count and each `sc` is converted from it.
fn plan(ops: &[Op], target: ColorTarget) -> Vec<(usize, Vec<Op>)> {
    let (mut fill, mut stroke) = (Some(1), Some(1));
    let mut stack: Vec<(Option<usize>, Option<usize>)> = Vec::new();
    let mut out = Vec::new();
    for (i, op) in ops.iter().enumerate() {
        match op.op.as_slice() {
            b"q" => {
                if stack.len() < 256 {
                    stack.push((fill, stroke));
                }
            }
            b"Q" => {
                if let Some((f, s)) = stack.pop() {
                    fill = f;
                    stroke = s;
                }
            }
            _ => {
                if let Some(new) = rewrite(op, target, &mut fill, &mut stroke) {
                    out.push((i, new));
                }
            }
        }
    }
    out
}

/// Convert the device colours of `pages` (0-based) to `target`. Returns how many colour
/// operators changed.
pub fn convert_colors(doc: &mut Document, pages: &[usize], target: ColorTarget) -> Result<usize, EditError> {
    let count = pdfcraft_model::pages(doc).len();
    if let Some(p) = pages.iter().find(|p| **p >= count) {
        return Err(EditError::NoSuchPage(*p));
    }
    let mut changed = 0_usize;
    for &p in pages {
        let page = crate::text::page_dict(doc, p)?;
        let streams = crate::text::content_streams(doc, &page.dict)?;
        if streams.is_empty() {
            continue;
        }
        let pieces = Pieces::join(&streams.iter().map(|(_, data)| data.as_slice()).collect::<Vec<_>>());
        let ops = pieces.parse().ops;
        let fixes = plan(&ops, target);
        if fixes.is_empty() {
            continue;
        }
        changed = changed.saturating_add(fixes.len());
        let mut next = fixes.into_iter().peekable();
        let new = crate::text::splice(&pieces, &ops, |i| match next.peek() {
            Some((at, _)) if *at == i => next.next().map_or((Vec::new(), true), |(_, ops)| (ops, false)),
            _ => (Vec::new(), true),
        });
        let contents = crate::text::rewritten_contents(doc, &streams, new);
        doc.update_dict(page.obj, |d| d.set(b"Contents".to_vec(), contents))?;
    }
    Ok(changed)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(src: &str, target: ColorTarget) -> String {
        let ops = pdfcraft_content::parse(src.as_bytes()).ops;
        let fixes = plan(&ops, target);
        let mut next = fixes.into_iter().peekable();
        let mut out: Vec<Op> = Vec::new();
        for (i, op) in ops.iter().enumerate() {
            match next.peek() {
                Some((at, _)) if *at == i => out.extend(next.next().map(|(_, o)| o).unwrap_or_default()),
                _ => out.push(op.clone()),
            }
        }
        String::from_utf8(pdfcraft_content::serialize_ops(&out)).unwrap().split_whitespace().collect::<Vec<_>>().join(" ")
    }

    #[test]
    fn device_colours_become_grey() {
        assert_eq!(run("1 0 0 rg 0 0 1 RG 0 0 0 1 k 0.5 g", ColorTarget::Gray), "0.3 g 0.11 G 0 g 0.5 g");
        // A device space set with cs: the space and its sc follow.
        assert_eq!(run("/DeviceRGB cs 0 1 0 sc", ColorTarget::Gray), "/DeviceGray cs 0.59 sc");
        // A named space (ICC, spot) is left alone, and so is its scn.
        assert_eq!(run("/CS0 cs 0.2 0.4 0.6 scn", ColorTarget::Gray), "/CS0 cs 0.2 0.4 0.6 scn");
        // q/Q restore the space.
        assert_eq!(run("q /CS0 cs Q 1 1 1 rg", ColorTarget::Gray), "q /CS0 cs Q 1 g");
    }

    #[test]
    fn device_colours_become_cmyk() {
        assert_eq!(run("1 0 0 rg", ColorTarget::Cmyk), "0 1 1 0 k");
        assert_eq!(run("0.25 G", ColorTarget::Cmyk), "0 0 0 0.75 K");
        assert_eq!(run("0 0 0 rg 0.1 0.2 0.3 0.4 k", ColorTarget::Cmyk), "0 0 0 1 k 0.1 0.2 0.3 0.4 k");
    }

    #[test]
    fn conversions_stay_in_range() {
        assert_eq!(rgb_to_cmyk(f64::NAN, 2.0, -1.0), [1.0, 0.0, 1.0, 0.0], "NaN and out-of-range values clamp");
        assert_eq!(cmyk_to_gray(1.0, 1.0, 1.0, 1.0), 0.0);
        assert_eq!(rgb_to_gray(1.0, 1.0, 1.0), 1.0);
    }
}
