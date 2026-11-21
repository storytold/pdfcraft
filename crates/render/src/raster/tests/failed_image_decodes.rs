//! A picture that doesn't decode is decoded once however often a page draws it (vendored
//! hayro-interpret patch 91). The decoded-image cache kept only decodes that succeeded, so a
//! damaged picture, or a JPX or JBIG2 stream its decoder refuses, was decoded again at every
//! draw: a page drawing it a thousand times decoded it a thousand times. The failure is kept for
//! the same key now, and every draw still reports it as it did.

use super::image_cache_pdf;
use hayro::hayro_interpret::font::Glyph;
use hayro::hayro_interpret::hayro_syntax::Pdf;
use hayro::hayro_interpret::{
    BlendMode, ClipPath, Context, Device, GlyphDrawMode, Image, InterpreterCache, InterpreterSettings, InterpreterWarning, Paint, PathDrawMode,
    SoftMask, TransformExt, interpret_page,
};
use kurbo::{Affine, BezPath, Rect};
use std::io::Write;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

/// What a page handed its device: the raster images with their pixels, those of them with a
/// mask, and the stencil masks with theirs. Every image is asked for at the size it is drawn
/// at, as the renderer does.
#[derive(Default)]
struct Draws {
    rasters: usize,
    masked: usize,
    stencils: usize,
}

impl<'a> Device<'a> for Draws {
    fn set_soft_mask(&mut self, _: Option<SoftMask<'a>>) {}
    fn set_blend_mode(&mut self, _: BlendMode) {}
    fn draw_path(&mut self, _: &BezPath, _: Affine, _: &Paint<'a>, _: &PathDrawMode) {}
    fn push_clip_path(&mut self, _: &ClipPath) {}
    fn push_transparency_group(&mut self, _: f32, _: Option<SoftMask<'a>>, _: BlendMode) {}
    fn draw_glyph(&mut self, _: &Glyph<'a>, _: Affine, _: Affine, _: &Paint<'a>, _: &GlyphDrawMode) {}
    fn draw_image(&mut self, image: Image<'a, '_>, transform: Affine) {
        let [a, b, c, d, ..] = transform.as_coeffs();
        let target = ((a.hypot(b) * f64::from(image.width())).ceil() as u32, (c.hypot(d) * f64::from(image.height())).ceil() as u32);
        match image {
            Image::Raster(r) => r.with_rgba(
                |_, alpha| {
                    self.rasters += 1;
                    self.masked += usize::from(alpha.is_some());
                },
                Some(target),
            ),
            Image::Stencil(s) => s.with_stencil(|_, _| self.stencils += 1, Some(target)),
        }
    }
    fn pop_clip_path(&mut self) {}
    fn pop_transparency_group(&mut self) {}
}

/// What drawing `/name` five times at one size, and then the picture that decodes (`/G`) once
/// on the right, did: the decodes the cache ran, the image decode failures reported, and what
/// the device was handed.
struct Outcome {
    decodes: usize,
    failures: usize,
    draws: Draws,
}

fn draw_five_times(name: &str) -> Outcome {
    draw_five_times_at(name, [20; 5])
}

/// [`draw_five_times`], drawing `/name` the five times at `sizes` (units square).
fn draw_five_times_at(name: &str, sizes: [u32; 5]) -> Outcome {
    let flate = |data: &[u8]| {
        let mut z = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::fast());
        z.write_all(data).unwrap();
        z.finish().unwrap()
    };
    let pixels = flate(&(0..64 * 64 * 3).map(|i| (i % 251) as u8).collect::<Vec<_>>());
    let image = |filter: &str, extra: &str, len: usize| {
        format!(
            "<< /Type /XObject /Subtype /Image /Width 64 /Height 64 /ColorSpace /DeviceRGB /BitsPerComponent 8 /Filter /{filter} {extra}/Length {len} >>"
        )
    };
    let good = image("FlateDecode", "", pixels.len());
    // A JPEG 2000 picture whose codestream is four zero bytes.
    let bad_jpx = image("JPXDecode", "", 4);
    // A picture whose soft mask is such a JPEG 2000 stream.
    let bad_mask = image("FlateDecode", "/SMask 9 0 R ", pixels.len());
    // A stencil mask whose JBIG2 data is four zero bytes.
    let bad_stencil = "<< /Type /XObject /Subtype /Image /Width 64 /Height 64 /ImageMask true /Filter /JBIG2Decode /Length 4 >>";
    let jpx_mask =
        "<< /Type /XObject /Subtype /Image /Width 64 /Height 64 /ColorSpace /DeviceGray /BitsPerComponent 8 /Filter /JPXDecode /Length 4 >>";
    let content = [0, 24, 48, 72, 96].iter().zip(sizes).map(|(x, s)| format!("q {s} 0 0 {s} {x} 10 cm /{name} Do Q")).collect::<Vec<_>>().join(" ")
        + " q 10 0 0 10 118 10 cm /G Do Q";
    let bytes = image_cache_pdf(
        &content,
        "<< /XObject << /G 5 0 R /J 6 0 R /S 7 0 R /M 8 0 R >> >>",
        &[(&good, &pixels), (&bad_jpx, b"\0\0\0\0"), (&bad_mask, &pixels), (bad_stencil, b"\0\0\0\0"), (jpx_mask, b"\0\0\0\0")],
    );
    let parsed = Pdf::new(Arc::new(bytes)).expect("parses");
    let page = &parsed.pages()[0];
    let cache = InterpreterCache::new();
    let failures = Arc::new(AtomicUsize::new(0));
    let others = Arc::new(Mutex::new(Vec::new()));
    let settings = InterpreterSettings {
        warning_sink: {
            let (failures, others) = (failures.clone(), others.clone());
            Arc::new(move |warning| match warning {
                InterpreterWarning::ImageDecodeFailure => {
                    failures.fetch_add(1, Ordering::Relaxed);
                }
                other => others.lock().unwrap().push(format!("{other:?}")),
            })
        },
        ..InterpreterSettings::default()
    };
    let initial = page.initial_transform(true).to_kurbo();
    let mut ctx = Context::new(initial, Rect::new(0.0, 0.0, 130.0, 70.0), &cache, page.xref(), settings);
    let mut draws = Draws::default();
    interpret_page(page, &mut ctx, &mut draws);
    assert!(others.lock().unwrap().is_empty(), "/{name} reported {:?}", others.lock().unwrap());
    Outcome { decodes: cache.decoded_image_decodes(), failures: failures.load(Ordering::Relaxed), draws }
}

/// A JPEG 2000 picture that doesn't decode, drawn five times at one size, is decoded once, and
/// each of the five draws still reports the failure.
#[test]
fn a_picture_that_does_not_decode_is_decoded_once_however_often_it_is_drawn() {
    let out = draw_five_times("J");
    assert_eq!(out.draws.rasters, 1, "only the picture after it drew");
    assert_eq!(out.decodes, 2, "decoded again at every draw");
    assert_eq!(out.failures, 5, "every draw reports the failure, as before");
}

/// A JPEG 2000 picture decodes at a reduced resolution for the size it is drawn at, so its
/// failure is kept for that size: drawn at two sizes, it is decoded once at each, also when it
/// comes back to the first size after the second.
#[test]
fn a_picture_that_does_not_decode_is_decoded_once_at_each_size() {
    let out = draw_five_times_at("J", [20, 20, 10, 10, 20]);
    assert_eq!(out.decodes, 3, "once at each of its two sizes, and the picture after it");
    assert_eq!(out.failures, 5, "every draw reports the failure");
}

/// A JBIG2 stencil mask that doesn't decode, drawn five times, is decoded once, and each draw
/// still reports the failure: it isn't the page's JBIG2 allowance that refused it, which a
/// later page would have afresh.
#[test]
fn a_stencil_mask_that_does_not_decode_is_decoded_once_however_often_it_is_drawn() {
    let out = draw_five_times("M");
    assert_eq!((out.draws.stencils, out.draws.rasters), (0, 1), "only the picture after it drew");
    assert_eq!(out.decodes, 2, "decoded again at every draw");
    assert_eq!(out.failures, 5, "every draw reports the failure, as before");
}

/// A picture whose soft mask doesn't decode draws without it at every draw, decoded once, and
/// the mask's failure is reported once, as it was (the picture decodes; only its mask fails).
#[test]
fn a_picture_whose_soft_mask_does_not_decode_draws_unmasked_decoded_once() {
    let out = draw_five_times("S");
    assert_eq!((out.draws.rasters, out.draws.masked), (6, 0), "drawn five times without its mask, and the picture after it");
    assert_eq!(out.decodes, 2);
    assert_eq!(out.failures, 1);
}

/// A picture that decodes is still decoded once and handed to every draw.
#[test]
fn a_picture_that_decodes_is_still_decoded_once_and_drawn_every_time() {
    let out = draw_five_times("G");
    assert_eq!(out.draws.rasters, 6);
    assert_eq!(out.decodes, 1);
    assert_eq!(out.failures, 0);
}

/// A JBIG2 picture the page's allowance left out isn't kept as a failure: the next page drawing
/// it starts with an allowance of its own and draws it, from the same cache.
#[test]
fn a_picture_left_out_for_the_pages_allowance_draws_on_the_next_page() {
    use super::{RenderWarning, black_jbig2_page, jbig2_segment};
    use crate::raster::{parse, render_page, take_warnings, warning_settings};
    use crate::{RenderConfig, RenderRequest, RequestKind};
    use hayro::RenderCache;

    let image = |number: u8, id: u8| {
        let mut stream = black_jbig2_page();
        jbig2_segment(&mut stream, 1, 62, &[], &[0, 0, 0, 0, id]);
        let mut object = format!(
            "{number} 0 obj << /Type /XObject /Subtype /Image /Width 16 /Height 2 /BitsPerComponent 1 /ColorSpace /DeviceGray /Filter /JBIG2Decode /Length {} >> stream\n",
            stream.len()
        )
        .into_bytes();
        object.extend_from_slice(&stream);
        object.extend_from_slice(b"\nendstream endobj\n");
        object
    };
    let page = |number: u8, content: u8| {
        format!("{number} 0 obj << /Type /Page /Parent 2 0 R /MediaBox [0 0 40 40] /Contents {content} 0 R /Resources << /XObject << /A 7 0 R /B 8 0 R >> >> >> endobj\n").into_bytes()
    };
    let content = |number: u8, text: &str| format!("{number} 0 obj << /Length {} >> stream\n{text}\nendstream endobj\n", text.len()).into_bytes();
    let mut pdf =
        b"%PDF-1.7\n1 0 obj << /Type /Catalog /Pages 2 0 R >> endobj\n2 0 obj << /Type /Pages /Kids [3 0 R 4 0 R] /Count 2 >> endobj\n".to_vec();
    pdf.extend(page(3, 5));
    pdf.extend(page(4, 6));
    // The first page spends its allowance on A and has nothing left for B; the second draws A,
    // decoded already, and then B.
    pdf.extend(content(5, "q 20 0 0 40 0 0 cm /A Do Q q 20 0 0 40 20 0 cm /B Do Q"));
    pdf.extend(content(6, "q 20 0 0 40 0 0 cm /A Do Q q 20 0 0 40 20 0 cm /B Do Q"));
    pdf.extend(image(7, 1));
    pdf.extend(image(8, 2));
    pdf.extend_from_slice(b"trailer << /Root 1 0 R >>\n%%EOF\n");
    let pdf = parse(&Arc::new(pdf), None).expect("parses");
    /// A page's pixels and the warnings it reported.
    type Drawn = (Vec<u8>, Vec<RenderWarning>);
    let black = |(rgba, _): &Drawn, x: usize| match &rgba[((20 * 40 + x) * 4)..][..4] {
        [0, 0, 0, 255] => true,
        [255, 255, 255, 255] => false,
        other => panic!("neither an image nor the page: {other:?}"),
    };
    // Both pages, or the first alone, through one cache, as a render worker keeps it.
    let pages = |work: u64, count: usize| {
        let config = RenderConfig { jbig2_work: Some(work), ..RenderConfig::default() };
        let cache = RenderCache::new();
        (0..count)
            .map(|page| {
                let warnings = Arc::new(Mutex::new(Vec::new()));
                let settings = warning_settings(&config.settings(), warnings.clone());
                let req = RenderRequest { page, kind: RequestKind::Pixels, tile: None, scale: 1.0, tag: 0, ..Default::default() };
                let (_, _, rgba, _) = render_page(&pdf, &cache, &settings, &config, req).expect("renders");
                (rgba[..].to_vec(), take_warnings(&warnings))
            })
            .collect::<Vec<Drawn>>()
    };
    // The smallest allowance that draws A on the first page. A larger allowance draws at least as
    // much, so a binary search over the first page alone finds it in about ten draws.
    let draws_a = |work: u64| pages(work, 1).first().is_some_and(|first| black(first, 10));
    let candidates: Vec<u64> = (1..1_000).collect();
    let one = candidates.get(candidates.partition_point(|work| !draws_a(*work))).copied().expect("a small image is cheap");
    let Ok([first, second]) = <[Drawn; 2]>::try_from(pages(one, 2)) else { panic!("two pages") };
    assert_eq!((black(&first, 10), black(&first, 30)), (true, false), "the first page had enough for A alone");
    assert!(first.1.contains(&RenderWarning::ContentTruncated), "{:?}", first.1);
    assert!(black(&second, 10) && black(&second, 30), "A, and B left out of the first page, draw on the second");
    assert!(second.1.is_empty(), "{:?}", second.1);
}

/// A JBIG2 mask its decoding work refuses: four typical-row regions of 16,384 × 16,384 take more
/// than a stream may.
fn jbig2_mask_past_its_work() -> Vec<u8> {
    use super::{black_jbig2_page, jbig2_segment, jbig2_white_region};
    let mut mask = black_jbig2_page();
    for number in 1..=4 {
        jbig2_segment(&mut mask, number, 38, &[], &jbig2_white_region(16384, 16384)); // immediate generic region
    }
    mask
}

/// What drawing 64 × 64 pictures under JBIG2 soft masks did, with the page's JBIG2 allowance
/// `work`: `/P` under `mask` (`/Width` × `/Height` `size`) and `/W` under
/// [`jbig2_mask_past_its_work`], each of `draws` a name and a size (units square). The decodes the
/// cache ran, how many times content was said to be left out, and what the device was handed.
fn draw_under_jbig2_masks(mask: &[u8], size: (u32, u32), draws: &[(&str, u32)], work: u64) -> (usize, usize, Draws) {
    let pixels = vec![0; 64 * 64 * 3];
    let picture = |mask: u8| {
        format!(
            "<< /Type /XObject /Subtype /Image /Width 64 /Height 64 /ColorSpace /DeviceRGB /BitsPerComponent 8 /SMask {mask} 0 R /Length {} >>",
            pixels.len()
        )
    };
    let soft_mask = |(width, height): (u32, u32), data: &[u8]| {
        format!(
            "<< /Type /XObject /Subtype /Image /Width {width} /Height {height} /ColorSpace /DeviceGray /BitsPerComponent 1 /Filter /JBIG2Decode /Length {} >>",
            data.len()
        )
    };
    let past_its_work = jbig2_mask_past_its_work();
    let content = draws.iter().map(|(name, s)| format!("q {s} 0 0 {s} 10 10 cm /{name} Do Q")).collect::<Vec<_>>().join(" ");
    let bytes = image_cache_pdf(
        &content,
        "<< /XObject << /P 5 0 R /W 7 0 R >> >>",
        &[(&picture(6), &pixels), (&soft_mask(size, mask), mask), (&picture(8), &pixels), (&soft_mask((16, 2), &past_its_work), &past_its_work)],
    );
    let parsed = Pdf::new(Arc::new(bytes)).expect("parses");
    let page = &parsed.pages()[0];
    let cache = InterpreterCache::new();
    let truncated = Arc::new(AtomicUsize::new(0));
    let others = Arc::new(Mutex::new(Vec::new()));
    let settings = InterpreterSettings {
        warning_sink: {
            let (truncated, others) = (truncated.clone(), others.clone());
            Arc::new(move |warning| match warning {
                InterpreterWarning::ContentTruncated => {
                    truncated.fetch_add(1, Ordering::Relaxed);
                }
                other => others.lock().unwrap().push(format!("{other:?}")),
            })
        },
        jbig2_work: work,
        ..InterpreterSettings::default()
    };
    let initial = page.initial_transform(true).to_kurbo();
    let mut ctx = Context::new(initial, Rect::new(0.0, 0.0, 130.0, 70.0), &cache, page.xref(), settings);
    let mut device = Draws::default();
    interpret_page(page, &mut ctx, &mut device);
    assert!(others.lock().unwrap().is_empty(), "reported {:?}", others.lock().unwrap());
    (cache.decoded_image_decodes(), truncated.load(Ordering::Relaxed), device)
}

/// A picture whose JBIG2 soft mask a decoder limit refuses isn't drawn (patch 73), and the limit
/// is the stream's own: decoding it again refuses it again. So the refusal is kept with the picture
/// that went with it (patch 91), and a page drawing it a hundred times decodes them once, as for
/// one draw. Kept as content left out, every draw still says so. The refusal wasn't kept, and the
/// picture, of any size, was decoded again at every draw: 2.45 s for a hundred draws of a 4,096 ×
/// 4,096 JPX picture, against 0.23 s under a mask the file damaged. A mask larger than its picture
/// is averaged down for each size it is drawn at, from one kept at an intermediate size (patch 82),
/// so a second size finds the refusal kept there and keeps it for that size too, also after
/// another picture's mask was refused for its work, which isn't kept.
#[test]
fn a_picture_whose_jbig2_mask_a_limit_refuses_is_decoded_once_however_often_it_is_drawn() {
    use super::{black_jbig2_page, black_jbig2_page_of, jbig2_segment, wide_jbig2_dictionary};
    let mut dictionary = black_jbig2_page();
    jbig2_segment(&mut dictionary, 1, 0, &[], &wide_jbig2_dictionary(65535, &[2048, 2049]));
    let masks = [
        ("past the decoder's bound on a side", black_jbig2_page_of(65536, 2), (65536, 2)),
        ("past the renderer's bound on pixels", black_jbig2_page_of(65535, 65535), (65535, 65535)),
        ("whose dictionary is past its bound", dictionary, (16, 2)),
    ];
    let work = InterpreterSettings::default().jbig2_work;
    for (what, mask, size) in masks {
        let (once, _, _) = draw_under_jbig2_masks(&mask, size, &[("P", 20)], work);
        let (decodes, truncated, draws) = draw_under_jbig2_masks(&mask, size, &[("P", 20); 100], work);
        assert_eq!(draws.rasters, 0, "a mask {what} leaves the picture out");
        assert_eq!(decodes, once, "a mask {what}: a hundred draws decode as one does");
        assert_eq!(truncated, 100, "a mask {what}: every draw says content was left out");
        // With no page allowance to run out of, so that the mask past its work is refused by its
        // own limit and the page's allowance doesn't stop what is kept.
        let each = [("P", 20), ("W", 20), ("P", 40)];
        let (once_each, _, _) = draw_under_jbig2_masks(&mask, size, &each, u64::MAX);
        let (decodes, _, draws) = draw_under_jbig2_masks(&mask, size, &[&each[..], &[("P", 40); 2]].concat(), u64::MAX);
        assert_eq!(draws.rasters, 0, "a mask {what} leaves the picture out at either size");
        assert_eq!(decodes, once_each, "a mask {what}: drawn again at its second size, decoded as once at each");
    }
}

/// A JBIG2 mask refused for the work its decoding takes isn't kept: that work may be what its page
/// had left, which another page has afresh. So the picture under it is decoded at every draw, here
/// with no page allowance to run out of, where the stream's own limit is what refuses it.
#[test]
fn a_picture_whose_jbig2_mask_is_refused_for_its_work_is_decoded_at_every_draw() {
    let mask = super::black_jbig2_page();
    let (once, _, _) = draw_under_jbig2_masks(&mask, (16, 2), &[("W", 20)], u64::MAX);
    let (decodes, truncated, draws) = draw_under_jbig2_masks(&mask, (16, 2), &[("W", 20); 3], u64::MAX);
    assert_eq!(draws.rasters, 0, "the picture is left out");
    assert_eq!(decodes, 3 * once, "decoded at every draw");
    assert_eq!(truncated, 3, "every draw says content was left out");
}
