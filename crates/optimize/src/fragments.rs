//! Merge image fragments: some PDF printers draw a page picture as hundreds or thousands of
//! images one pixel row high, stacked edge to edge. Each fragment is too small to resample, and
//! every JPEG fragment carries its own headers, so a one-page document can weigh megabytes and
//! no image setting makes it smaller. Here the fragments are put back together into one image,
//! which the image settings then resample and recompress like any other.
//!
//! What counts as a run of fragments, and is merged:
//! - consecutive `q … cm /Name Do Q` groups in a page's content stream, with nothing else
//!   between them, each drawing an image upright (no rotation, skew or flip);
//! - the images are 8-bit grey or RGB in the same colour space, without masks or a decode array
//!   that changes anything, in a codec PdfCraft decodes;
//! - they tile one rectangle edge to edge with no gap or overlap — stacked top to bottom, placed
//!   side by side, or arranged in a grid of rows and columns (in any drawing order), at least
//!   [`MIN_FRAGMENTS`] of them.
//!
//! The merged image keeps the finest resolution of its fragments both ways, so nothing is lost
//! until the image settings downsample it; it is stored losslessly. A page whose content
//! stream can't be read back exactly is left alone. The fragments are then unused, and Clean Up
//! drops them.

use pdfcraft_content::{Matrix, Op, parse, serialize_ops};
use pdfcraft_cos::{DecodeBudget, Dict, Document, ObjRef, Object, Stream};

use crate::images::{components, content_bytes, decodable, page_resources, pixels, resample};
use crate::{OptimizeError, Report, Stage};

/// Fewest stacked images treated as fragments of one picture.
pub const MIN_FRAGMENTS: usize = 4;

/// Most pixels a merged image may have (memory).
const MAX_PIXELS: u64 = 80_000_000;

/// Fragment groups a page's search for runs may read per content operator, and at least. A run
/// that is not one picture is searched again from each of its groups, so without a limit images
/// drawn over one another take time quadratic in their number; past it, the rest of the page is
/// left as it is.
const SCAN_PER_OP: usize = 16;
const MIN_SCAN: usize = 1 << 16;

#[cfg(test)]
thread_local! {
    /// Fragment groups read, for the test that keeps a page's search linear in its content.
    static GROUP_READS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

/// One fragment: its image, its box in the coordinates the run is drawn in, and its size.
struct Fragment {
    image: ObjRef,
    rect: [f64; 4],
    width: u32,
    height: u32,
}

/// The ops `q`, one or more `cm`, `Do`, `Q` starting at `i`: the index after them, the matrix
/// and the name drawn.
fn group_at(ops: &[Op], i: usize) -> Option<(usize, Matrix, Vec<u8>)> {
    #[cfg(test)]
    GROUP_READS.with(|c| c.set(c.get().saturating_add(1)));
    if !ops.get(i)?.is("q") {
        return None;
    }
    let mut j = i + 1;
    let mut m = Matrix([1.0, 0.0, 0.0, 1.0, 0.0, 0.0]);
    while let Some(op) = ops.get(j).filter(|o| o.is("cm")) {
        m = Matrix::from_operands(&op.operands)?.then(&m);
        j += 1;
    }
    let draw = ops.get(j).filter(|o| o.is("Do"))?;
    let name = draw.name(0)?.to_vec();
    ops.get(j + 1).filter(|o| o.is("Q"))?;
    (j > i + 1).then_some((j + 2, m, name))
}

/// The image `name` draws, if it can be a fragment: its reference, size, colour space and
/// component count.
fn fragment_image(doc: &Document, xobjects: &Dict, name: &[u8]) -> Option<(ObjRef, u32, u32, Object, usize)> {
    let r = xobjects.get(name).and_then(Object::as_ref)?;
    let obj = doc.get(r);
    let Object::Stream(s) = &*obj else { return None };
    let d = &s.dict;
    if d.name(b"Subtype") != Some(b"Image") || d.int(b"BitsPerComponent") != Some(8) || d.contains(b"Mask") || d.contains(b"SMask") {
        return None;
    }
    if matches!(d.get(b"ImageMask"), Some(Object::Bool(true))) || !decodable(doc, s)? {
        return None;
    }
    let cs = d.get(b"ColorSpace")?.clone();
    let n = components(doc, &cs)?;
    let (w, h) = (u32::try_from(d.int(b"Width")?).ok()?, u32::try_from(d.int(b"Height")?).ok()?);
    (w > 0 && h > 0).then_some((r, w, h, cs, n))
}

/// Whether `a` and `b` share their top and bottom edges within row tolerance.
fn same_y(a: &Fragment, b: &Fragment) -> bool {
    let tol = 1e-3 * (a.rect[3] - a.rect[1]).min(b.rect[3] - b.rect[1]);
    (a.rect[1] - b.rect[1]).abs() <= tol && (a.rect[3] - b.rect[3]).abs() <= tol
}

/// Whether `run` tiles one rectangle edge to edge (stacked top to bottom, placed side by side,
/// or arranged in a grid of rows and columns). Sorts it from the top row down, and left to right
/// within each row. Returns that exact row partition: tolerance-based membership is not
/// transitive, so assembly must not regroup after the horizontal sort changes each row's anchor.
fn stacked(run: &mut [Fragment]) -> Option<Vec<std::ops::Range<usize>>> {
    if run.is_empty()
        || run.iter().any(|f| {
            let [x0, y0, x1, y1] = f.rect;
            let (w, h) = (x1 - x0, y1 - y0);
            !(x0.is_finite() && y0.is_finite() && x1.is_finite() && y1.is_finite() && w.is_finite() && w > 0.0 && h.is_finite() && h > 0.0)
        })
    {
        return None;
    }
    run.sort_by(|a, b| b.rect[3].total_cmp(&a.rect[3]));
    let mut row_bounds: Vec<[f64; 4]> = Vec::new();
    let mut rows = Vec::new();
    let mut start = 0;
    while start < run.len() {
        let mut end = start + 1;
        while end < run.len() && same_y(&run[start], &run[end]) {
            end += 1;
        }
        let row = &mut run[start..end];
        row.sort_by(|a, b| a.rect[0].total_cmp(&b.rect[0]));
        if !row.windows(2).all(|w| match w {
            [a, b] => (a.rect[2] - b.rect[0]).abs() <= 0.01 * (a.rect[2] - a.rect[0]).min(b.rect[2] - b.rect[0]),
            _ => true,
        }) {
            return None;
        }
        let (Some(first), Some(last)) = (row.first(), row.last()) else { return None };
        row_bounds.push([first.rect[0], first.rect[1], last.rect[2], first.rect[3]]);
        rows.push(start..end);
        start = end;
    }
    let (Some(&[x0, _, x1, y1]), Some(&[_, y0, _, _])) = (row_bounds.first(), row_bounds.last()) else { return None };
    let (width, height) = (x1 - x0, y1 - y0);
    if !(width.is_finite() && width > 0.0 && height.is_finite() && height > 0.0) {
        return None;
    }
    let tol_x = 1e-3 * width.max(1.0);
    if row_bounds.iter().any(|r| (r[0] - x0).abs() > tol_x || (r[2] - x1).abs() > tol_x) {
        return None;
    }
    row_bounds
        .windows(2)
        .all(|w| match w {
            [a, b] => (a[1] - b[3]).abs() <= 0.01 * (a[3] - a[1]).min(b[3] - b[1]),
            _ => true,
        })
        .then_some(rows)
}

/// The fragment groups from `i` on (at most `limit`) while they form a picture drawn row by row
/// or strip by strip in one direction, up to a gap or a jump.
fn drawn_in_sequence(ops: &[Op], i: usize, limit: usize, xobjects: &Dict, doc: &Document) -> Vec<Fragment> {
    let mut run: Vec<Fragment> = Vec::new();
    let mut row_x = [0.0f64, 0.0];
    let mut cur_x = [0.0f64, 0.0];
    let mut cur_y = [0.0f64, 0.0];
    let mut first_row = true;
    let mut right: Option<bool> = None;
    let mut down: Option<bool> = None;
    let mut complete = 0usize;
    let mut j = i;
    while run.len() < limit {
        let Some((next, m, name)) = group_at(ops, j) else { break };
        let Some((image, width, height, ..)) = fragment_image(doc, xobjects, &name) else { break };
        let [a, _, _, d, e, f] = m.0;
        let f2 = Fragment { image, rect: [e, f, e + a, f + d], width, height };
        if let Some(prev) = run.last() {
            let tol_y = 1e-3 * (cur_y[1] - cur_y[0]).min(f2.rect[3] - f2.rect[1]);
            let same_row = (cur_y[0] - f2.rect[1]).abs() <= tol_y && (cur_y[1] - f2.rect[3]).abs() <= tol_y;
            if same_row {
                let tol_x = 0.01 * (prev.rect[2] - prev.rect[0]).min(f2.rect[2] - f2.rect[0]);
                let step_right = (f2.rect[0] - prev.rect[2]).abs() <= tol_x;
                let step_left = (prev.rect[0] - f2.rect[2]).abs() <= tol_x;
                match (right, step_right, step_left) {
                    (None | Some(true), true, _) => {
                        right = Some(true);
                        cur_x[1] = f2.rect[2];
                    }
                    (None | Some(false), _, true) => {
                        right = Some(false);
                        cur_x[0] = f2.rect[0];
                    }
                    _ => break,
                }
                if first_row {
                    row_x = cur_x;
                    complete = run.len() + 1;
                } else {
                    let tol_span = 1e-3 * (row_x[1] - row_x[0]).max(1.0);
                    if cur_x[0] < row_x[0] - tol_span || cur_x[1] > row_x[1] + tol_span {
                        break;
                    }
                    if (cur_x[0] - row_x[0]).abs() <= tol_span && (cur_x[1] - row_x[1]).abs() <= tol_span {
                        complete = run.len() + 1;
                    }
                }
            } else {
                let tol_span = 1e-3 * (row_x[1] - row_x[0]).max(1.0);
                if (cur_x[0] - row_x[0]).abs() > tol_span || (cur_x[1] - row_x[1]).abs() > tol_span {
                    break;
                }
                let tol_v = 0.01 * (cur_y[1] - cur_y[0]).min(f2.rect[3] - f2.rect[1]);
                let below = (cur_y[0] - f2.rect[3]).abs() <= tol_v;
                let above = (f2.rect[1] - cur_y[1]).abs() <= tol_v;
                match (down, below, above) {
                    (None | Some(true), true, _) => down = Some(true),
                    (None | Some(false), _, true) => down = Some(false),
                    _ => break,
                }
                let starts_left = (f2.rect[0] - row_x[0]).abs() <= tol_span && f2.rect[2] <= row_x[1] + tol_span;
                let starts_right = (f2.rect[2] - row_x[1]).abs() <= tol_span && f2.rect[0] >= row_x[0] - tol_span;
                if starts_left && starts_right {
                    right = None;
                    complete = run.len() + 1;
                } else if starts_left {
                    right = Some(true);
                } else if starts_right {
                    right = Some(false);
                } else {
                    break;
                }
                first_row = false;
                cur_x = [f2.rect[0], f2.rect[2]];
                cur_y = [f2.rect[1], f2.rect[3]];
            }
        } else {
            row_x = [f2.rect[0], f2.rect[2]];
            cur_x = row_x;
            cur_y = [f2.rect[1], f2.rect[3]];
            complete = 1;
        }
        run.push(f2);
        j = next;
    }
    run.truncate(complete);
    run
}

/// The op index after the first `count` fragment groups from `i`.
fn run_end(ops: &[Op], i: usize, count: usize) -> usize {
    let mut j = i;
    for _ in 0..count {
        match group_at(ops, j) {
            Some((next, ..)) => j = next,
            None => break,
        }
    }
    j
}

/// The run's pixels as one image, top row first: `(pixels, width, height)`.
fn merge(doc: &Document, run: &[Fragment], partition: &[std::ops::Range<usize>], n: usize) -> Option<(Vec<u8>, u32, u32)> {
    let (first, last) = (run.first()?, run.last()?);
    let (left, right, top, bottom) = (first.rect[0], last.rect[2], first.rect[3], last.rect[1]);
    let rows: Vec<&[Fragment]> = partition.iter().map(|range| run.get(range.clone())).collect::<Option<_>>()?;
    // Pixels per unit along each axis: the finest fragment's.
    let density_x = run.iter().map(|f| f.width as f64 / (f.rect[2] - f.rect[0])).fold(0.0, f64::max);
    let density_y = run.iter().map(|f| f.height as f64 / (f.rect[3] - f.rect[1])).fold(0.0, f64::max);
    let width = if rows.len() == run.len() { f64::from(run.iter().map(|f| f.width).max()?) } else { ((right - left) * density_x).round() };
    let height = if rows.len() == 1 { f64::from(run.iter().map(|f| f.height).max()?) } else { ((top - bottom) * density_y).round() };
    if !(width.is_finite() && width >= 1.0 && width <= f64::from(u32::MAX)) {
        return None;
    }
    if !(height.is_finite() && height >= 1.0 && height <= f64::from(u32::MAX)) {
        return None;
    }
    // In range and whole, so the casts are exact.
    let (width, height) = (width as u32, height as u32);
    if u64::from(width) * u64::from(height) > MAX_PIXELS {
        return None;
    }
    let stride = (width as usize).checked_mul(n)?;
    let mut out = vec![255u8; stride.checked_mul(height as usize)?];
    let mut r0 = 0u32;
    for (row_idx, row) in rows.iter().enumerate() {
        let r1 = if row_idx + 1 == rows.len() { height } else { (((top - row.first()?.rect[1]) * density_y).round().max(0.0) as u32).min(height) };
        if r1 <= r0 {
            return None;
        }
        let th = r1 - r0;
        let mut c0 = 0u32;
        for (col_idx, f) in row.iter().enumerate() {
            let c1 = if col_idx + 1 == row.len() { width } else { (((f.rect[2] - left) * density_x).round().max(0.0) as u32).min(width) };
            if c1 <= c0 {
                return None;
            }
            let tw = c1 - c0;
            let obj = doc.get(f.image);
            let Object::Stream(s) = &*obj else { return None };
            let px = pixels(doc, s, n)?;
            let px = if (f.width, f.height) == (tw, th) { px } else { resample(&px, n, f.width, f.height, tw, th)? };
            let tile_row = (tw as usize).checked_mul(n)?;
            let col_off = (c0 as usize).checked_mul(n)?;
            for r in 0..th as usize {
                let dst = (r0 as usize + r) * stride + col_off;
                let src = r * tile_row;
                out.get_mut(dst..dst + tile_row)?.copy_from_slice(px.get(src..src + tile_row)?);
            }
            c0 = c1;
        }
        r0 = r1;
    }
    Some((out, width, height))
}

/// A name for the merged image that `xobjects` doesn't use yet.
fn fresh_name(xobjects: &Dict, k: &mut usize) -> Vec<u8> {
    loop {
        *k += 1;
        let name = format!("PCMerged{k}").into_bytes();
        if !xobjects.contains(&name) {
            return name;
        }
    }
}

/// Add `name` → `image` to the page's image resources, wherever they live.
fn add_xobject(doc: &mut Document, page: ObjRef, name: &[u8], image: ObjRef) -> Result<(), OptimizeError> {
    let res = doc.get(page).as_dict().and_then(|d| d.get(b"Resources").cloned());
    let put = |d: &mut Dict| {
        let mut x = d.get(b"XObject").and_then(Object::as_dict).cloned().unwrap_or_default();
        x.set(name.to_vec(), Object::Ref(image));
        d.set(b"XObject".to_vec(), Object::Dict(x));
    };
    match res {
        Some(Object::Ref(r)) => match doc.get(r).as_dict().and_then(|d| d.reference(b"XObject")) {
            Some(x) => doc.update_dict(x, |d| d.set(name.to_vec(), Object::Ref(image)))?,
            None => doc.update_dict(r, put)?,
        },
        Some(Object::Dict(d)) => match d.reference(b"XObject") {
            Some(x) => doc.update_dict(x, |d| d.set(name.to_vec(), Object::Ref(image)))?,
            None => doc.update_dict(page, |p| {
                let mut d = d.clone();
                put(&mut d);
                p.set(b"Resources".to_vec(), Object::Dict(d));
            })?,
        },
        // Inherited (or none): the page gets its own copy with the new image.
        _ => {
            let mut d = page_resources(doc, page).unwrap_or_default();
            if let Some(x) = d.reference(b"XObject") {
                let x = doc.get(x).as_dict().cloned().unwrap_or_default();
                d.set(b"XObject".to_vec(), Object::Dict(x));
            }
            put(&mut d);
            doc.update_dict(page, |p| p.set(b"Resources".to_vec(), Object::Dict(d)))?;
        }
    }
    Ok(())
}

/// Merge the fragment runs of one page; returns how many fragments were merged.
fn merge_page(doc: &mut Document, page: ObjRef, report: &mut Report, progress: &mut dyn FnMut(Stage) -> bool) -> Result<usize, OptimizeError> {
    let Some(contents) = doc.get(page).as_dict().and_then(|d| d.get(b"Contents").cloned()) else { return Ok(0) };
    // A page whose content isn't all known is left alone: rewriting it would drop the rest.
    let Some(data) = content_bytes(doc, &contents, &mut DecodeBudget::page()) else { return Ok(0) };
    let parsed = parse(&data);
    if parsed.skipped > 0 {
        return Ok(0);
    }
    let resources = page_resources(doc, page).unwrap_or_default();
    let mut xobjects = resources.get(b"XObject").map(|x| doc.resolve(x)).and_then(|x| x.as_dict().cloned()).unwrap_or_default();
    let ops = parsed.ops;
    let mut out: Vec<Op> = Vec::with_capacity(ops.len());
    let (mut i, mut merged, mut k) = (0, 0, 0);
    let budget = ops.len().saturating_mul(SCAN_PER_OP).max(MIN_SCAN);
    let mut scanned = 0usize;
    while i < ops.len() {
        if scanned > budget {
            out.extend(ops.get(i..).unwrap_or_default().iter().cloned());
            break;
        }
        // The longest run of fragment groups from here with one colour space.
        let mut run = Vec::new();
        let mut kind: Option<(Object, usize)> = None;
        let mut j = i;
        while let Some((next, m, name)) = group_at(&ops, j) {
            let Some((image, width, height, cs, n)) = fragment_image(doc, &xobjects, &name) else { break };
            let [a, b, c, d, e, f] = m.0;
            if b.abs() > 1e-9 || c.abs() > 1e-9 || !(a > 0.0 && d > 0.0) {
                break;
            }
            match &kind {
                Some((k_cs, k_n)) if *k_cs != cs || *k_n != n => break,
                None => kind = Some((cs, n)),
                _ => {}
            }
            run.push(Fragment { image, rect: [e, f, e + a, f + d], width, height });
            j = next;
        }
        scanned = scanned.saturating_add(run.len()).saturating_add(1);
        // Not one picture as a whole: a prefix drawn row by row or strip by strip may be.
        let mut rows = if run.len() >= MIN_FRAGMENTS { stacked(&mut run) } else { None };
        if run.len() >= MIN_FRAGMENTS && rows.is_none() {
            run = drawn_in_sequence(&ops, i, run.len(), &xobjects, doc);
            j = run_end(&ops, i, run.len());
            scanned = scanned.saturating_add(run.len().saturating_mul(2)).saturating_add(2);
            rows = stacked(&mut run);
        }
        let Some(((cs, n), rows)) = kind.zip(rows).filter(|_| run.len() >= MIN_FRAGMENTS) else {
            // Not a run: keep this op and look again from the next one.
            if let Some(op) = ops.get(i) {
                out.push(op.clone());
            }
            i += 1;
            continue;
        };
        if !progress(Stage::Fragments) {
            return Err(OptimizeError::Cancelled);
        }
        let Some((px, w, h)) = merge(doc, &run, &rows, n) else {
            out.extend(ops.get(i..j).unwrap_or_default().iter().cloned());
            i = j;
            continue;
        };
        let mut dict = Dict::new();
        dict.set(b"Type".to_vec(), Object::name("XObject"));
        dict.set(b"Subtype".to_vec(), Object::name("Image"));
        dict.set(b"Width".to_vec(), Object::Int(i64::from(w)));
        dict.set(b"Height".to_vec(), Object::Int(i64::from(h)));
        dict.set(b"ColorSpace".to_vec(), cs);
        dict.set(b"BitsPerComponent".to_vec(), Object::Int(8));
        let image = doc.add(Object::Stream(Stream::flate(dict, &px)));
        let name = fresh_name(&xobjects, &mut k);
        xobjects.set(name.clone(), Object::Ref(image));
        add_xobject(doc, page, &name, image)?;
        let (Some(first), Some(last)) = (run.first(), run.last()) else { break };
        let [x0, y0, x1, y1] = [first.rect[0], last.rect[1], last.rect[2], first.rect[3]];
        let num = Object::Real;
        out.push(Op::new("q", Vec::new()));
        out.push(Op::new("cm", vec![num(x1 - x0), Object::Int(0), Object::Int(0), num(y1 - y0), num(x0), num(y0)]));
        out.push(Op::new("Do", vec![Object::Name(name)]));
        out.push(Op::new("Q", Vec::new()));
        merged += run.len();
        report.merged_images += 1;
        i = j;
    }
    if merged > 0 {
        let stream = doc.add(Object::Stream(Stream::flate(Dict::new(), &serialize_ops(&out))));
        doc.update_dict(page, |p| p.set(b"Contents".to_vec(), Object::Ref(stream)))?;
    }
    Ok(merged)
}

/// Merge consecutive tiled image fragments across `pages`.
pub(crate) fn run(doc: &mut Document, pages: &[ObjRef], report: &mut Report, progress: &mut dyn FnMut(Stage) -> bool) -> Result<(), OptimizeError> {
    for p in pages {
        report.fragments_merged += merge_page(doc, *p, report, progress)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use pdfcraft_cos::{Dict, Document, Object, Stream};

    use super::*;

    /// Merged dimensions exceeding `u32::MAX` are rejected before allocating pixel buffers.
    #[test]
    fn a_merged_height_beyond_u32_is_refused_before_allocating() {
        // Two fragments, one pixel row each, the top one 2^-48 high: the merged image would be
        // 2^16 × 2^48 pixels, whose product wraps to 0 in u64.
        let image = ObjRef { num: 1, generation: 0 };
        let edge = 1.0 - (-48f64).exp2();
        let vertical = [
            Fragment { image, rect: [0.0, edge, 1.0, 1.0], width: 1 << 16, height: 1 },
            Fragment { image, rect: [0.0, 0.0, 1.0, edge], width: 1 << 16, height: 1 },
        ];
        assert!(merge(&Document::new_empty(), &vertical, &[0..1, 1..2], 3).is_none());
        let horizontal = [
            Fragment { image, rect: [0.0, 0.0, edge, 1.0], width: 1, height: 1 << 16 },
            Fragment { image, rect: [edge, 0.0, 1.0, 1.0], width: 1, height: 1 << 16 },
        ];
        assert!(merge(&Document::new_empty(), &horizontal, std::slice::from_ref(&(0..2)), 3).is_none());
    }

    /// Even an inconsistent partition must not silently omit a row or tile from its report.
    #[test]
    fn assembly_refuses_collapsed_output_rows_and_tiles() {
        let mut doc = Document::new_empty();
        let mut dict = Dict::new();
        dict.set(b"Width".to_vec(), Object::Int(10));
        dict.set(b"Height".to_vec(), Object::Int(10));
        dict.set(b"BitsPerComponent".to_vec(), Object::Int(8));
        dict.set(b"ColorSpace".to_vec(), Object::name("DeviceGray"));
        let image = doc.add(Object::Stream(Stream::flate(dict, &[80; 100])));
        let mut run: Vec<_> = (0..4)
            .map(|col| Fragment { image, rect: [10.0 * f64::from(col), 0.0, 10.0 * f64::from(col + 1), 10.0], width: 10, height: 10 })
            .collect();
        // The first supposed row consumes the entire height; the second cannot be discarded.
        assert!(merge(&doc, &run, &[0..2, 2..4], 1).is_none());
        // Likewise, a tile whose right edge does not advance cannot simply vanish.
        if let Some(tile) = run.get_mut(1) {
            tile.rect = [0.0, 0.0, 10.0, 10.0];
        }
        assert!(merge(&doc, &run, std::slice::from_ref(&(0..4)), 1).is_none());
    }

    /// Many images drawn on top of each other are not one picture; looking for runs among them reads
    /// a bounded number of groups per operator, not the rest of the page from every group.
    #[test]
    fn rejected_runs_read_a_bounded_number_of_groups() {
        let mut doc = Document::new_empty();
        let mut image = Dict::new();
        for (k, v) in
            [("Subtype", Object::name("Image")), ("Width", Object::Int(1)), ("Height", Object::Int(1)), ("BitsPerComponent", Object::Int(8))]
        {
            image.set(k.as_bytes().to_vec(), v);
        }
        image.set(b"ColorSpace".to_vec(), Object::name("DeviceGray"));
        let image = doc.add(Object::Stream(Stream::from_raw(image, vec![0])));
        let groups = 4000;
        let content = doc.add(Object::Stream(Stream::flate(Dict::new(), "q 10 0 0 10 0 0 cm /Im Do Q\n".repeat(groups).as_bytes())));
        let mut xobjects = Dict::new();
        xobjects.set(b"Im".to_vec(), Object::Ref(image));
        let mut resources = Dict::new();
        resources.set(b"XObject".to_vec(), Object::Dict(xobjects));
        let mut page = Dict::new();
        page.set(b"Contents".to_vec(), Object::Ref(content));
        page.set(b"Resources".to_vec(), Object::Dict(resources));
        let page = doc.add(Object::Dict(page));
        GROUP_READS.with(|c| c.set(0));
        let merged = merge_page(&mut doc, page, &mut Report::default(), &mut |_| true);
        assert_eq!(merged.ok(), Some(0));
        let reads = GROUP_READS.with(|c| c.get());
        assert!(reads <= 40 * 4 * groups, "{reads} group reads");
    }

    /// Side-by-side vertical strips and 2D tile grids merge into one image XObject while invalid
    /// or separated runs are handled per run.
    #[test]
    fn side_by_side_and_grid_fragments_merge_into_one_image() {
        let tile = |doc: &mut Document, w: u32, h: u32, px: Vec<u8>| {
            let mut d = Dict::new();
            d.set(b"Type".to_vec(), Object::name("XObject"));
            d.set(b"Subtype".to_vec(), Object::name("Image"));
            d.set(b"Width".to_vec(), Object::Int(i64::from(w)));
            d.set(b"Height".to_vec(), Object::Int(i64::from(h)));
            d.set(b"BitsPerComponent".to_vec(), Object::Int(8));
            d.set(b"ColorSpace".to_vec(), Object::name("DeviceGray"));
            doc.add(Object::Stream(Stream::flate(d, &px)))
        };
        let build_page = |doc: &mut Document, xobjects: Dict, ops: &str| {
            let content = doc.add(Object::Stream(Stream::flate(Dict::new(), ops.as_bytes())));
            let mut resources = Dict::new();
            resources.set(b"XObject".to_vec(), Object::Dict(xobjects));
            let mut page = Dict::new();
            page.set(b"Contents".to_vec(), Object::Ref(content));
            page.set(b"Resources".to_vec(), Object::Dict(resources));
            doc.add(Object::Dict(page))
        };

        // Four vertical strips placed side by side (drawn out of order), alternating 1×3 and 2×3
        // pixels so the merged image keeps the finest horizontal resolution (8×3).
        let mut doc = Document::new_empty();
        let mut xobjects = Dict::new();
        for c in 0..4u8 {
            let w = if c % 2 == 0 { 1 } else { 2 };
            let im = tile(&mut doc, w, 3, vec![c * 50; (w * 3) as usize]);
            xobjects.set(format!("C{c}").into_bytes(), Object::Ref(im));
        }
        let ops = [2usize, 0, 3, 1].into_iter().map(|c| format!("q 10 0 0 30 {} 100 cm /C{c} Do Q\n", 15.0 + c as f64 * 10.0)).collect::<String>();
        let page = build_page(&mut doc, xobjects, &ops);
        let mut report = Report::default();
        let merged = merge_page(&mut doc, page, &mut report, &mut |_| true).unwrap();
        assert_eq!((merged, report.merged_images), (4, 1));
        let xo = doc
            .get(page)
            .as_dict()
            .and_then(|d| d.get(b"Resources"))
            .and_then(Object::as_dict)
            .and_then(|d| d.get(b"XObject"))
            .and_then(Object::as_dict)
            .cloned()
            .unwrap();
        let merged_ref = xo.get(b"PCMerged1").and_then(Object::as_ref).unwrap();
        let Object::Stream(s) = &*doc.get(merged_ref) else { panic!("expected stream") };
        assert_eq!((s.dict.int(b"Width"), s.dict.int(b"Height")), (Some(8), Some(3)));
        assert_eq!(s.decoded().unwrap(), [0, 0, 50, 50, 100, 100, 150, 150].repeat(3));
        let c_ref = doc.get(page).as_dict().and_then(|d| d.reference(b"Contents")).unwrap();
        let Object::Stream(cs) = &*doc.get(c_ref) else { panic!("expected contents") };
        let parsed = parse(&cs.decoded().unwrap());
        let cm = parsed.ops.iter().find(|o| o.is("cm")).and_then(|o| Matrix::from_operands(&o.operands)).unwrap();
        assert_eq!(cm.0, [40.0, 0.0, 0.0, 30.0, 15.0, 100.0]);

        // A 2×3 grid of tiles (2 rows, 3 columns), each 2×2 pixels, drawn in mixed order.
        let mut doc = Document::new_empty();
        let mut xobjects = Dict::new();
        for r in 0..2u8 {
            for c in 0..3u8 {
                let v = r * 100 + c * 20;
                let im = tile(&mut doc, 2, 2, vec![v; 4]);
                xobjects.set(format!("T{r}{c}").into_bytes(), Object::Ref(im));
            }
        }
        let ops = [(1u8, 2u8), (0, 1), (1, 0), (0, 2), (0, 0), (1, 1)]
            .into_iter()
            .map(|(r, c)| {
                let x = 5.0 + f64::from(c) * 10.0;
                let y = 50.0 + f64::from(1 - r) * 10.0;
                format!("q 10 0 0 10 {x} {y} cm /T{r}{c} Do Q\n")
            })
            .collect::<String>();
        let page = build_page(&mut doc, xobjects, &ops);
        let mut report = Report::default();
        let merged = merge_page(&mut doc, page, &mut report, &mut |_| true).unwrap();
        assert_eq!((merged, report.merged_images), (6, 1));
        let xo = doc
            .get(page)
            .as_dict()
            .and_then(|d| d.get(b"Resources"))
            .and_then(Object::as_dict)
            .and_then(|d| d.get(b"XObject"))
            .and_then(Object::as_dict)
            .cloned()
            .unwrap();
        let Object::Stream(s) = &*doc.get(xo.get(b"PCMerged1").and_then(Object::as_ref).unwrap()) else { panic!("expected stream") };
        assert_eq!((s.dict.int(b"Width"), s.dict.int(b"Height")), (Some(6), Some(4)));
        let mut expected = Vec::new();
        expected.extend([0, 0, 20, 20, 40, 40].repeat(2));
        expected.extend([100, 100, 120, 120, 140, 140].repeat(2));
        assert_eq!(s.decoded().unwrap(), expected);

        // Two side-by-side runs of 4 strips separated by a horizontal gap merge into 2 images,
        // whether drawn left to right or right to left.
        for right_to_left in [false, true] {
            let mut doc = Document::new_empty();
            let mut xobjects = Dict::new();
            let im = tile(&mut doc, 2, 2, vec![80; 4]);
            xobjects.set(b"S".to_vec(), Object::Ref(im));
            let xs: Vec<f64> = if right_to_left {
                vec![110.0, 100.0, 90.0, 80.0, 30.0, 20.0, 10.0, 0.0]
            } else {
                vec![0.0, 10.0, 20.0, 30.0, 80.0, 90.0, 100.0, 110.0]
            };
            let ops = xs.into_iter().map(|x| format!("q 10 0 0 20 {x} 0 cm /S Do Q\n")).collect::<String>();
            let page = build_page(&mut doc, xobjects, &ops);
            let mut report = Report::default();
            let merged = merge_page(&mut doc, page, &mut report, &mut |_| true).unwrap();
            assert_eq!((merged, report.merged_images), (8, 2), "right_to_left={right_to_left}");
        }

        // A 2×2 grid followed immediately by a lone fifth tile below only the first column merges
        // the 4 grid tiles and leaves the fifth tile alone.
        let mut doc = Document::new_empty();
        let mut xobjects = Dict::new();
        let im = tile(&mut doc, 2, 2, vec![90; 4]);
        xobjects.set(b"G".to_vec(), Object::Ref(im));
        let ops = [(0.0, 20.0), (10.0, 20.0), (0.0, 10.0), (10.0, 10.0), (0.0, 0.0)]
            .into_iter()
            .map(|(x, y)| format!("q 10 0 0 10 {x} {y} cm /G Do Q\n"))
            .collect::<String>();
        let page = build_page(&mut doc, xobjects, &ops);
        let mut report = Report::default();
        let merged = merge_page(&mut doc, page, &mut report, &mut |_| true).unwrap();
        assert_eq!((merged, report.merged_images), (4, 1));

        // Overlapping side-by-side strips or rows of different widths are left alone.
        for bad_ops in [
            "q 10 0 0 20 0 0 cm /G Do Q\nq 10 0 0 20 5 0 cm /G Do Q\nq 10 0 0 20 15 0 cm /G Do Q\nq 10 0 0 20 25 0 cm /G Do Q\n",
            "q 10 0 0 10 0 10 cm /G Do Q\nq 10 0 0 10 10 10 cm /G Do Q\nq 10 0 0 10 0 0 cm /G Do Q\nq 15 0 0 10 10 0 cm /G Do Q\n",
        ] {
            let mut doc = Document::new_empty();
            let mut xobjects = Dict::new();
            let im = tile(&mut doc, 2, 2, vec![90; 4]);
            xobjects.set(b"G".to_vec(), Object::Ref(im));
            let page = build_page(&mut doc, xobjects, bad_ops);
            let mut report = Report::default();
            let merged = merge_page(&mut doc, page, &mut report, &mut |_| true).unwrap();
            assert_eq!((merged, report.merged_images), (0, 0));
        }
    }
}
