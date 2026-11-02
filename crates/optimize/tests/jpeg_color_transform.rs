//! Optimize keeps a JPEG's colours whatever its colour transform says (#1201).
//!
//! A JPEG with three or four components can store its samples as they are or converted to YCbCr
//! (YCCK). An Adobe APP14 marker in the JPEG says which; without one, the filter's
//! `/ColorTransform` does (ISO 32000-2 §7.4.8, default 1 for three components). Each image here
//! is drawn by PdfCraft's own renderer before and after Optimize, and the two must show the same
//! colours, up to what a JPEG re-encode loses.

use std::sync::Arc;

use pdfcraft_cos::{Dict, Document, ObjRef, Object, SaveOptions, Stream, write_full};
use pdfcraft_optimize::{OptimizeError, Settings, Stage, optimize, optimize_with_progress};

/// A photo-like RGB picture: colourful gradients with a little texture, so the colour transform
/// matters everywhere and a medium-quality JPEG is smaller than a maximum-quality one.
fn photo(w: u32, h: u32) -> Vec<u8> {
    let mut px = Vec::with_capacity((w * h * 3) as usize);
    for y in 0..h {
        for x in 0..w {
            let t = ((x ^ y) & 7) as u8;
            px.push((x * 255 / w) as u8 ^ t);
            px.push((y * 255 / h) as u8);
            px.push((255 - (x + y) * 127 / (w + h)) as u8 ^ (t << 2));
        }
    }
    px
}

fn rgb_jpeg(px: &[u8], w: u32, h: u32) -> Vec<u8> {
    let mut out = Vec::new();
    image::codecs::jpeg::JpegEncoder::new_with_quality(&mut out, 100).encode(px, w, h, image::ExtendedColorType::Rgb8).unwrap();
    out
}

/// The JPEG's segments before its first scan: (marker, start of the marker, end of the segment).
fn segments(jpeg: &[u8]) -> Vec<(u8, usize, usize)> {
    assert_eq!(&jpeg[..2], &[0xFF, 0xD8]);
    let (mut i, mut out) = (2, Vec::new());
    loop {
        assert_eq!(jpeg[i], 0xFF, "a marker at {i}");
        let marker = jpeg[i + 1];
        let end = i + 2 + u16::from_be_bytes([jpeg[i + 2], jpeg[i + 3]]) as usize;
        out.push((marker, i, end));
        if marker == 0xDA {
            return out;
        }
        i = end;
    }
}

/// `jpeg` with an Adobe APP14 segment whose transform flag is `transform` (0: samples stored as
/// they are, 1: YCbCr, 2: YCCK).
fn with_adobe(jpeg: &[u8], transform: u8) -> Vec<u8> {
    assert!(segments(jpeg).iter().all(|s| s.0 != 0xEE), "no Adobe segment yet");
    let mut v = jpeg[..2].to_vec();
    v.extend_from_slice(&[0xFF, 0xEE, 0, 14, b'A', b'd', b'o', b'b', b'e', 0, 100, 0, 0, 0, 0, transform]);
    v.extend_from_slice(&jpeg[2..]);
    v
}

/// `jpeg` with its three components named R, G and B (in the frame and scan headers), which
/// JPEG decoders read as samples stored untransformed.
fn with_rgb_ids(jpeg: &[u8]) -> Vec<u8> {
    let mut v = jpeg.to_vec();
    let mut ids = Vec::new();
    for (marker, i, _) in segments(jpeg) {
        match marker {
            0xC0 => {
                assert_eq!(v[i + 9], 3);
                for k in 0..3 {
                    ids.push(v[i + 10 + 3 * k]);
                    v[i + 10 + 3 * k] = b"RGB"[k];
                }
            }
            0xDA => {
                for k in 0..v[i + 4] as usize {
                    let id = v[i + 5 + 2 * k];
                    v[i + 5 + 2 * k] = b"RGB"[ids.iter().position(|&x| x == id).unwrap()];
                }
            }
            _ => {}
        }
    }
    v
}

/// A baseline JPEG `w` × `h` (multiples of 8) with four components, every 8 × 8 block one flat
/// colour `block(bx, by)` as stored, and an Adobe APP14 segment if `adobe` is given.
fn four_component_jpeg(w: u16, h: u16, adobe: Option<u8>, block: impl Fn(u16, u16) -> [u8; 4]) -> Vec<u8> {
    fn put(bits: &mut Vec<bool>, value: u32, n: u32) {
        bits.extend((0..n).rev().map(|i| value >> i & 1 == 1));
    }
    let mut v = vec![0xFF, 0xD8];
    if let Some(t) = adobe {
        v.extend_from_slice(&[0xFF, 0xEE, 0, 14, b'A', b'd', b'o', b'b', b'e', 0, 100, 0, 0, 0, 0, t]);
    }
    // Every quantiser 1: a flat block's DC coefficient is 8 × (sample − 128), exactly.
    v.extend_from_slice(&[0xFF, 0xDB, 0, 67, 0]);
    v.extend_from_slice(&[1; 64]);
    v.extend_from_slice(&[0xFF, 0xC0, 0, 20, 8]);
    v.extend_from_slice(&h.to_be_bytes());
    v.extend_from_slice(&w.to_be_bytes());
    v.push(4);
    for id in 1..=4 {
        v.extend_from_slice(&[id, 0x11, 0]);
    }
    // DC table: categories 0 to 11 with four-bit codes equal to the category. AC table: EOB, "0".
    v.extend_from_slice(&[0xFF, 0xC4, 0, 31, 0x00, 0, 0, 0, 12, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0]);
    v.extend(0..12u8);
    v.extend_from_slice(&[0xFF, 0xC4, 0, 20, 0x10, 1, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0]);
    v.extend_from_slice(&[0xFF, 0xDA, 0, 14, 4, 1, 0, 2, 0, 3, 0, 4, 0, 0, 63, 0]);
    let mut bits = Vec::new();
    let mut pred = [0i32; 4];
    for by in 0..h / 8 {
        for bx in 0..w / 8 {
            for (c, s) in block(bx, by).into_iter().enumerate() {
                let dc = (i32::from(s) - 128) * 8;
                let diff = dc - pred[c];
                pred[c] = dc;
                let category = 32 - diff.unsigned_abs().leading_zeros();
                put(&mut bits, category, 4);
                put(&mut bits, if diff < 0 { (diff + (1 << category) - 1) as u32 } else { diff as u32 }, category);
                put(&mut bits, 0, 1);
            }
        }
    }
    bits.resize(bits.len().div_ceil(8) * 8, true);
    for byte in bits.chunks(8).map(|b| b.iter().fold(0u8, |a, &x| a << 1 | u8::from(x))) {
        v.push(byte);
        if byte == 0xFF {
            v.push(0);
        }
    }
    v.extend_from_slice(&[0xFF, 0xD9]);
    v
}

/// An image XObject holding `jpeg`.
fn jpeg_image(doc: &mut Document, w: u32, h: u32, colour_space: &str, jpeg: Vec<u8>, parms: Parms) -> ObjRef {
    let mut d = Dict::new();
    d.set(b"Type".to_vec(), Object::name("XObject"));
    d.set(b"Subtype".to_vec(), Object::name("Image"));
    d.set(b"Width".to_vec(), Object::Int(w as i64));
    d.set(b"Height".to_vec(), Object::Int(h as i64));
    d.set(b"BitsPerComponent".to_vec(), Object::Int(8));
    d.set(b"ColorSpace".to_vec(), Object::name(colour_space));
    let ct = |c: i64| {
        let mut p = Dict::new();
        p.set(b"ColorTransform".to_vec(), Object::Int(c));
        Object::Dict(p)
    };
    match parms {
        Parms::None => d.set(b"Filter".to_vec(), Object::name("DCTDecode")),
        Parms::Dict(c) => {
            d.set(b"Filter".to_vec(), Object::name("DCTDecode"));
            d.set(b"DecodeParms".to_vec(), ct(c));
        }
        Parms::Array(c) => {
            d.set(b"Filter".to_vec(), Object::Array(vec![Object::name("DCTDecode")]));
            d.set(b"DecodeParms".to_vec(), Object::Array(vec![ct(c)]));
        }
    }
    doc.add(Object::Stream(Stream::from_raw(d, jpeg)))
}

/// How an image's `/DecodeParms` gives its `/ColorTransform`.
#[derive(Clone, Copy, Debug)]
enum Parms {
    None,
    /// `/Filter /DCTDecode /DecodeParms << /ColorTransform c >>`.
    Dict(i64),
    /// `/Filter [/DCTDecode] /DecodeParms [<< /ColorTransform c >>]`.
    Array(i64),
}

/// A document with one page, `w` × `h` pt, whose content is `content` drawing `images`.
fn one_page(doc: &mut Document, w: u32, h: u32, images: &[(String, ObjRef)], content: &str) {
    let pages = doc.root().and_then(|r| doc.get(r).as_dict().and_then(|d| d.reference(b"Pages"))).unwrap();
    let mut x = Dict::new();
    for (name, r) in images {
        x.set(name.as_bytes().to_vec(), Object::Ref(*r));
    }
    let mut res = Dict::new();
    res.set(b"XObject".to_vec(), Object::Dict(x));
    let c = doc.add(Object::Stream(Stream::from_raw(Dict::new(), content.as_bytes().to_vec())));
    let mut p = Dict::new();
    p.set(b"Type".to_vec(), Object::name("Page"));
    p.set(b"Parent".to_vec(), Object::Ref(pages));
    p.set(b"MediaBox".to_vec(), Object::Array(vec![0.into(), 0.into(), (w as i64).into(), (h as i64).into()]));
    p.set(b"Resources".to_vec(), Object::Dict(res));
    p.set(b"Contents".to_vec(), Object::Ref(c));
    let r = doc.add(Object::Dict(p));
    doc.update_dict(pages, |d| {
        d.set(b"Kids".to_vec(), Object::Array(vec![Object::Ref(r)]));
        d.set(b"Count".to_vec(), Object::Int(1));
    })
    .unwrap();
}

/// A document whose one page shows `jpeg` (`w` × `h` px) at `ppi`.
fn showing(w: u32, h: u32, ppi: u32, colour_space: &str, jpeg: Vec<u8>, parms: Parms) -> (Document, ObjRef) {
    let mut doc = Document::new_empty();
    let img = jpeg_image(&mut doc, w, h, colour_space, jpeg, parms);
    let (pw, ph) = (w * 72 / ppi, h * 72 / ppi);
    one_page(&mut doc, pw, ph, &[("Im".to_string(), img)], &format!("q {pw} 0 0 {ph} 0 0 cm /Im Do Q"));
    (doc, img)
}

fn stream(doc: &Document, r: ObjRef) -> Stream {
    match &*doc.get(r) {
        Object::Stream(s) => s.clone(),
        _ => panic!("not a stream"),
    }
}

/// The page as PdfCraft's renderer draws it, `scale` device pixels per point: RGB rows.
fn render(doc: &Document, scale: f32) -> (u32, u32, Vec<u8>) {
    let bytes = write_full(doc, &SaveOptions::default()).unwrap();
    let mut r = pdfcraft_render::PageRenderer::new(Arc::new(bytes), pdfcraft_render::RenderConfig::default());
    let out = r.render(pdfcraft_render::RenderRequest { page: 0, scale, ..Default::default() });
    assert!(out.error.is_none(), "{:?}", out.error);
    (out.width, out.height, out.rgba.chunks(4).flat_map(|p| [p[0], p[1], p[2]]).collect())
}

/// The mean difference per colour component between two renders of one page, leaving out a
/// two-pixel border (where the picture's edge is smoothed).
fn mean_difference(a: &(u32, u32, Vec<u8>), b: &(u32, u32, Vec<u8>)) -> f64 {
    assert_eq!((a.0, a.1), (b.0, b.1), "renders of one page size");
    let (w, h) = (a.0 as usize, a.1 as usize);
    let (mut sum, mut count) = (0u64, 0u64);
    for y in 2..h - 2 {
        for x in 2..w - 2 {
            for c in 0..3 {
                let i = (y * w + x) * 3 + c;
                sum += u64::from(a.2[i].abs_diff(b.2[i]));
                count += 1;
            }
        }
    }
    sum as f64 / count as f64
}

/// Re-encoding a JPEG at medium quality moves a component by about this much on average; a
/// colour transform applied or dropped wrongly moves it by tens.
const TOLERANCE: f64 = 4.0;

#[derive(Clone, Copy, Debug, PartialEq)]
enum Outcome {
    /// Re-encoded (smaller), as a plain YCbCr JPEG with no `/DecodeParms`.
    Reencoded,
    /// Left exactly as it was.
    Untouched,
}

#[test]
fn optimize_keeps_the_colours_of_an_rgb_jpeg_whatever_its_colour_transform() {
    use Outcome::*;
    let (w, h) = (128, 128);
    let plain = rgb_jpeg(&photo(w, h), w, h);
    // What the cases rely on: one JPEG read with and without its colour transform shows very
    // different colours, so a transform lost or added by Optimize can't hide in the tolerance.
    let transformed = render(&showing(w, h, 72, "DeviceRGB", plain.clone(), Parms::None).0, 1.0);
    let as_stored = render(&showing(w, h, 72, "DeviceRGB", plain.clone(), Parms::Dict(0)).0, 1.0);
    let apart = mean_difference(&transformed, &as_stored);
    assert!(apart > 10.0 * TOLERANCE, "the fixture tells the two readings apart: {apart:.1}");

    let mut failures = Vec::new();
    for (what, adobe, rgb_ids, parms, expect) in [
        ("no Adobe marker, no /ColorTransform", None, false, Parms::None, Reencoded),
        ("no Adobe marker, /ColorTransform 1", None, false, Parms::Dict(1), Reencoded),
        ("no Adobe marker, /ColorTransform 0", None, false, Parms::Dict(0), Reencoded),
        ("no Adobe marker, /ColorTransform 0 in a filter array", None, false, Parms::Array(0), Reencoded),
        ("Adobe YCbCr marker", Some(1), false, Parms::None, Reencoded),
        ("Adobe YCbCr marker, /ColorTransform 1", Some(1), false, Parms::Dict(1), Reencoded),
        ("Adobe untransformed marker", Some(0), false, Parms::None, Reencoded),
        ("Adobe untransformed marker, /ColorTransform 0", Some(0), false, Parms::Dict(0), Reencoded),
        // The marker overrides /ColorTransform in the standard, but readers differ (PdfCraft's
        // renderer lets /ColorTransform 0 win): the image is left as it is.
        ("Adobe YCbCr marker, /ColorTransform 0", Some(1), false, Parms::Dict(0), Untouched),
        ("Adobe untransformed marker, /ColorTransform 1", Some(0), false, Parms::Dict(1), Untouched),
        // Components named R, G and B: JPEG decoders read them untransformed, the standard's
        // default transforms them. Only an explicit /ColorTransform 0 settles it.
        ("components R, G, B, no /ColorTransform", None, true, Parms::None, Untouched),
        ("components R, G, B, /ColorTransform 0", None, true, Parms::Dict(0), Reencoded),
    ] {
        let mut jpeg = plain.clone();
        if rgb_ids {
            jpeg = with_rgb_ids(&jpeg);
        }
        if let Some(t) = adobe {
            jpeg = with_adobe(&jpeg, t);
        }
        let (mut doc, img) = showing(w, h, 72, "DeviceRGB", jpeg, parms);
        let original = stream(&doc, img);
        let before = render(&doc, 1.0);
        // Reduce File Size: an image at 72 ppi isn't resampled, only recompressed as JPEG 60.
        let report = optimize(&mut doc, &Settings::default()).unwrap();
        let after = render(&doc, 1.0);
        let new = stream(&doc, img);
        let outcome = if new == original {
            Untouched
        } else {
            assert_eq!(new.dict.name(b"Filter"), Some(&b"DCTDecode"[..]), "{what}");
            assert!(!new.dict.contains(b"DecodeParms"), "{what}: a plain JPEG has no colour transform to state");
            assert_eq!(report.images_recompressed, 1, "{what}");
            Reencoded
        };
        let difference = mean_difference(&before, &after);
        if outcome != expect || difference > TOLERANCE {
            failures.push(format!("{what}: {outcome:?} (expected {expect:?}), colours moved by {difference:.1}"));
        }
    }
    assert!(failures.is_empty(), "\n{}", failures.join("\n"));
}

#[test]
fn optimize_leaves_cmyk_jpegs_as_they_are_with_or_without_an_adobe_marker() {
    // 256 px drawn at 288 ppi: Reduce File Size would resample a colour space it handles.
    let (w, h) = (256u16, 256u16);
    let block = |bx: u16, by: u16| [(bx * 8) as u8, (by * 8) as u8, (255 - (bx + by) * 4) as u8, ((bx ^ by) * 8) as u8];
    for (adobe, parms) in [
        (None, Parms::None),
        (None, Parms::Dict(0)),
        (None, Parms::Dict(1)),
        (Some(0), Parms::None),
        (Some(0), Parms::Dict(1)),
        (Some(2), Parms::None),
        (Some(2), Parms::Dict(0)),
    ] {
        let what = format!("Adobe marker {adobe:?}, {parms:?}");
        let jpeg = four_component_jpeg(w, h, adobe, block);
        let (mut doc, img) = showing(u32::from(w), u32::from(h), 288, "DeviceCMYK", jpeg, parms);
        let original = doc.get(img);
        let before = render(&doc, 4.0);
        optimize(&mut doc, &Settings::default()).unwrap();
        assert_eq!(doc.get(img), original, "{what}: left as it is");
        assert!(before == render(&doc, 4.0), "{what}: the page looks the same");
    }
}

#[test]
fn merging_jpeg_strips_keeps_their_colour_transform() {
    // A page picture printed as 16 JPEG strips stored untransformed (/ColorTransform 0, no
    // Adobe marker): the merged image must show the strips' colours, not their samples converted
    // from YCbCr.
    let (w, h, rows) = (128u32, 128u32, 8u32);
    let px = photo(w, h);
    let mut doc = Document::new_empty();
    let mut images = Vec::new();
    let mut content = String::new();
    for k in 0..h / rows {
        let band = &px[(k * rows * w * 3) as usize..((k + 1) * rows * w * 3) as usize];
        let img = jpeg_image(&mut doc, w, rows, "DeviceRGB", rgb_jpeg(band, w, rows), Parms::Dict(0));
        images.push((format!("S{k}"), img));
        content += &format!("q {w} 0 0 {rows} 0 {} cm /S{k} Do Q\n", h - (k + 1) * rows);
    }
    one_page(&mut doc, w, h, &images, &content);
    let before = render(&doc, 1.0);
    let report = optimize(&mut doc, &Settings::default()).unwrap();
    assert_eq!(report.merged_images, 1, "{report:?}");
    let difference = mean_difference(&before, &render(&doc, 1.0));
    assert!(difference <= TOLERANCE, "colours moved by {difference:.1}");
}

/// `jpeg` with `bytes` right after its SOI marker.
fn after_soi(jpeg: &[u8], bytes: &[u8]) -> Vec<u8> {
    let mut v = jpeg[..2].to_vec();
    v.extend_from_slice(bytes);
    v.extend_from_slice(&jpeg[2..]);
    v
}

#[test]
fn a_jpeg_header_that_takes_more_than_a_few_steps_to_read_is_left_as_it_is() {
    // Decoders skip stray bytes between segments and any number of fill bytes before a marker,
    // so a stream can make a header scan walk megabytes. Optimize reads the header again for
    // every group of fragments it tries: it stops at the first stray byte, or after a few
    // thousand steps, and leaves such an image as it is.
    let (w, h) = (128, 128);
    let plain = rgb_jpeg(&photo(w, h), w, h);
    for (what, jpeg, expect) in [
        ("three fill bytes before the first segment", after_soi(&plain, &[0xFF; 3]), Outcome::Reencoded),
        ("one stray byte before the first segment", after_soi(&plain, &[0]), Outcome::Untouched),
        ("8 MB of stray bytes before the first segment", after_soi(&plain, &vec![0; 8_000_000]), Outcome::Untouched),
        ("10,000 fill bytes before the first segment", after_soi(&plain, &[0xFF; 10_000]), Outcome::Untouched),
    ] {
        let (mut doc, img) = showing(w, h, 72, "DeviceRGB", jpeg, Parms::None);
        let original = stream(&doc, img);
        optimize(&mut doc, &Settings::default()).unwrap();
        let outcome = if stream(&doc, img) == original { Outcome::Untouched } else { Outcome::Reencoded };
        assert_eq!(outcome, expect, "{what}");
    }
}

#[test]
fn cancelling_works_from_the_first_call_with_a_jpeg_drawn_many_times() {
    // One JPEG with 8 MB of stray bytes after SOI, drawn 64 times at the same place (the case
    // reported on #3012). Fragment merging reads its header for each group it tries, before the
    // first progress call. The header scan stops at the first stray byte, so that call is the
    // first image, and cancelling there stops Optimize.
    let (w, h) = (128, 8);
    let jpeg = after_soi(&rgb_jpeg(&photo(w, h), w, h), &vec![0; 8_000_000]);
    let mut doc = Document::new_empty();
    let img = jpeg_image(&mut doc, w, h, "DeviceRGB", jpeg, Parms::None);
    let content = format!(
        "q {w} 0 0 {h} 0 0 cm /Im Do Q
"
    )
    .repeat(64);
    one_page(&mut doc, w, h, &[("Im".to_string(), img)], &content);
    let mut stages = Vec::new();
    let result = optimize_with_progress(&mut doc, &Settings::default(), &mut |stage| {
        stages.push(stage);
        false
    });
    assert!(matches!(result, Err(OptimizeError::Cancelled)), "{result:?}");
    assert_eq!(stages, [Stage::Images { done: 0, total: 1 }]);
}
