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
//! - they share their left and right edges and are stacked edge to edge with no gap or overlap
//!   (in any drawing order), at least [`MIN_FRAGMENTS`] of them.
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

/// Whether `run` tiles one rectangle top to bottom: same left and right edges, edge to edge.
/// Sorts it from the top down.
fn stacked(run: &mut [Fragment]) -> bool {
    let Some(first) = run.first() else { return false };
    let [x0, _, x1, _] = first.rect;
    let width = x1 - x0;
    if !(width.is_finite() && width > 0.0) {
        return false;
    }
    let tol = 1e-3 * width.max(1.0);
    if run.iter().any(|f| {
        (f.rect[0] - x0).abs() > tol || (f.rect[2] - x1).abs() > tol || f.rect[3].partial_cmp(&f.rect[1]) != Some(std::cmp::Ordering::Greater)
    }) {
        return false;
    }
    run.sort_by(|a, b| b.rect[3].total_cmp(&a.rect[3]));
    run.windows(2).all(|w| match w {
        [a, b] => (a.rect[1] - b.rect[3]).abs() <= 0.01 * (a.rect[3] - a.rect[1]).min(b.rect[3] - b.rect[1]),
        _ => true,
    })
}

/// The fragment groups from `i` on while each is drawn right under (or right over) the one
/// before it, in the same direction: a picture drawn row by row, up to a gap or a jump.
fn drawn_in_sequence(ops: &[Op], i: usize, xobjects: &Dict, doc: &Document) -> Vec<Fragment> {
    let mut run: Vec<Fragment> = Vec::new();
    let mut down: Option<bool> = None;
    let mut j = i;
    while let Some((next, m, name)) = group_at(ops, j) {
        let Some((image, width, height, ..)) = fragment_image(doc, xobjects, &name) else { break };
        let [a, _, _, d, e, f] = m.0;
        let f2 = Fragment { image, rect: [e, f, e + a, f + d], width, height };
        if let Some(prev) = run.last() {
            let tol = 0.01 * (prev.rect[3] - prev.rect[1]).min(f2.rect[3] - f2.rect[1]);
            let below = (prev.rect[1] - f2.rect[3]).abs() <= tol;
            let above = (f2.rect[1] - prev.rect[3]).abs() <= tol;
            let same_x = (prev.rect[0] - f2.rect[0]).abs() <= 1e-3 * a.abs().max(1.0) && (prev.rect[2] - f2.rect[2]).abs() <= 1e-3 * a.abs().max(1.0);
            match (down, below, above) {
                _ if !same_x => break,
                (None | Some(true), true, _) => down = Some(true),
                (None | Some(false), _, true) => down = Some(false),
                _ => break,
            }
        }
        run.push(f2);
        j = next;
    }
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
fn merge(doc: &Document, run: &[Fragment], n: usize) -> Option<(Vec<u8>, u32, u32)> {
    let (top, bottom) = (run.first()?.rect[3], run.last()?.rect[1]);
    let width = run.iter().map(|f| f.width).max()?;
    // Pixel rows per unit of height: the finest fragment's.
    let density = run.iter().map(|f| f.height as f64 / (f.rect[3] - f.rect[1])).fold(0.0, f64::max);
    let height = ((top - bottom) * density).round();
    if !(height.is_finite() && height >= 1.0 && height <= f64::from(u32::MAX)) {
        return None;
    }
    // In range and whole, so the cast is exact.
    let height = height as u32;
    if u64::from(width) * u64::from(height) > MAX_PIXELS {
        return None;
    }
    let row = (width as usize).checked_mul(n)?;
    let mut out = vec![255u8; row.checked_mul(height as usize)?];
    for f in run {
        let r0 = (((top - f.rect[3]) * density).round().max(0.0) as u32).min(height);
        let r1 = (((top - f.rect[1]) * density).round().max(0.0) as u32).min(height);
        if r1 <= r0 {
            continue;
        }
        let obj = doc.get(f.image);
        let Object::Stream(s) = &*obj else { return None };
        let px = pixels(s, n)?;
        let px = if (f.width, f.height) == (width, r1 - r0) { px } else { resample(&px, n, f.width, f.height, width, r1 - r0)? };
        out.get_mut(r0 as usize * row..r1 as usize * row)?.copy_from_slice(px.get(..(r1 - r0) as usize * row)?);
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
        // Not one picture as a whole: the rows drawn one under the other from the start may be.
        if run.len() >= MIN_FRAGMENTS && !stacked(&mut run) {
            run = drawn_in_sequence(&ops, i, &xobjects, doc);
            j = run_end(&ops, i, run.len());
            scanned = scanned.saturating_add(run.len().saturating_mul(2)).saturating_add(2);
        }
        let Some((cs, n)) = kind.filter(|_| run.len() >= MIN_FRAGMENTS && stacked(&mut run)) else {
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
        let Some((px, w, h)) = merge(doc, &run, n) else {
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
        let [x0, y0, x1, y1] = [first.rect[0], last.rect[1], first.rect[2], first.rect[3]];
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

    #[test]
    fn a_merged_height_beyond_u32_is_refused_before_allocating() {
        // Two fragments, one pixel row each, the top one 2^-48 high: the merged image would be
        // 2^16 × 2^48 pixels, whose product wraps to 0 in u64.
        let image = ObjRef { num: 1, generation: 0 };
        let edge = 1.0 - (-48f64).exp2();
        let run = [
            Fragment { image, rect: [0.0, edge, 1.0, 1.0], width: 1 << 16, height: 1 },
            Fragment { image, rect: [0.0, 0.0, 1.0, edge], width: 1 << 16, height: 1 },
        ];
        assert!(merge(&Document::new_empty(), &run, 3).is_none());
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
}
