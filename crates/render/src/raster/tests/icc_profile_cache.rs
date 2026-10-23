//! Two `/ICCBased` profiles whose stream dictionaries are byte for byte the same (`/N 3` and one
//! `/Length`) each draw through their own profile (vendored hayro-interpret patch 48). hayro
//! cached the colour space it read from a profile stream by the dictionary's bytes alone, so
//! the second profile drew through whichever was read first.
//!
//! The profiles are synthetic ones built here: a linear profile with the sRGB colorants, and the
//! same profile with red and blue swapped, which draws red as blue. They have the same length.

use super::{image_cache_pdf, render_first_page, rgb_at};

/// The width in pixels of the 130 × 70 pt page `image_cache_pdf` makes, rendered at 72 dpi.
const WIDTH: usize = 130;

const RED: [u8; 3] = [255, 0, 0];
const BLUE: [u8; 3] = [0, 0, 255];

/// An s15Fixed16Number (ICC.1 §4.6).
fn s15f16(value: f64) -> [u8; 4] {
    ((value * 65536.0).round() as i32).to_be_bytes()
}

/// sRGB's red, green and blue colorants, adapted to D50 as sRGB profiles record them.
const SRGB_COLORANTS: [[f64; 3]; 3] = [[0.4361, 0.2225, 0.0139], [0.3851, 0.7169, 0.0971], [0.1431, 0.0606, 0.7141]];

/// An ICC v2 RGB display profile (ICC.1:2001-04) with linear tone curves, whose red, green and
/// blue take the sRGB colorants `primaries` names: `[0, 1, 2]` is linear sRGB, and `[2, 1, 0]`
/// draws red as blue. The device model is left blank: hayro takes a profile whose model reads
/// `sRGB` for standard sRGB and skips its transform. Every profile is 300 bytes long.
fn rgb_profile(primaries: [usize; 3]) -> Vec<u8> {
    let mut tags: Vec<([u8; 4], Vec<u8>)> = Vec::new();
    for (signature, primary) in [*b"rXYZ", *b"gXYZ", *b"bXYZ"].into_iter().zip(primaries) {
        let [x, y, z] = SRGB_COLORANTS[primary];
        tags.push((signature, [b"XYZ ".as_slice(), &[0; 4], &s15f16(x), &s15f16(y), &s15f16(z)].concat()));
    }
    for signature in [*b"rTRC", *b"gTRC", *b"bTRC"] {
        // A `curveType` with no entries: the identity.
        tags.push((signature, [b"curv".as_slice(), &[0; 8]].concat()));
    }
    let start = 128 + 4 + 12 * tags.len();
    let (mut table, mut data) = (Vec::new(), Vec::new());
    for (signature, tag) in &tags {
        table.extend_from_slice(signature);
        table.extend_from_slice(&u32::try_from(start + data.len()).unwrap().to_be_bytes());
        table.extend_from_slice(&u32::try_from(tag.len()).unwrap().to_be_bytes());
        data.extend_from_slice(tag);
    }
    let mut profile = vec![0; 128];
    profile[0..4].copy_from_slice(&u32::try_from(start + data.len()).unwrap().to_be_bytes());
    profile[8..12].copy_from_slice(&[2, 0x10, 0, 0]);
    profile[12..16].copy_from_slice(b"mntr");
    profile[16..20].copy_from_slice(b"RGB ");
    profile[20..24].copy_from_slice(b"XYZ ");
    profile[36..40].copy_from_slice(b"acsp");
    // The illuminant of the profile connection space, D50.
    for (i, value) in [0.9642, 1.0, 0.8249].into_iter().enumerate() {
        profile[68 + 4 * i..72 + 4 * i].copy_from_slice(&s15f16(value));
    }
    profile.extend_from_slice(&u32::try_from(tags.len()).unwrap().to_be_bytes());
    profile.extend(table);
    profile.extend(data);
    profile
}

/// An `/ICCBased` space whose profile stream is written directly in the space's array, not as
/// an object of its own (hayro reads such a stream when its data is plain tokens): the profile
/// as ASCIIHex digits, each a token of its own, in upper case (a lone `f` reads as a broken
/// `false`, and the page's dictionary with it). Its dictionary reports the object holding the
/// array, which is the page for every one of them.
fn direct_space(profile: &[u8]) -> String {
    let hex: String = profile.iter().flat_map(|b| [b >> 4, b & 0xf]).map(|d| format!("{d:X} ")).collect();
    format!("[/ICCBased << /N 3 /Filter /ASCIIHexDecode /Length {} >> stream\n{hex}\nendstream]", hex.len())
}

/// A page that fills six 20 pt squares, side by side, with `1 0 0 sc` (red) in the spaces `/A`
/// (object 5, linear sRGB), `/B` (object 6, red and blue swapped), `/A2` (object 5 again,
/// through an array of other bytes, so the profile is looked up again), `/A3` (object 7, a copy
/// of object 5), `/C` and `/D` (the two profiles again, each a stream written directly in its
/// array). Objects 5, 6 and 7 have the same dictionary, and so do the two direct streams.
fn page() -> Vec<u8> {
    let (srgb, swapped) = (rgb_profile([0, 1, 2]), rgb_profile([2, 1, 0]));
    assert_eq!(srgb.len(), swapped.len(), "the profiles have the same length, so the same dictionary");
    assert_ne!(srgb, swapped);
    let dict = format!("<< /N 3 /Length {} >>", srgb.len());
    let resources = format!(
        "<< /ColorSpace << /A [/ICCBased 5 0 R] /B [/ICCBased 6 0 R] /A2 [ /ICCBased 5 0 R ] /A3 [/ICCBased 7 0 R] /C {} /D {} >> >>",
        direct_space(&srgb),
        direct_space(&swapped)
    );
    let content = ["A", "B", "A2", "A3", "C", "D"]
        .iter()
        .enumerate()
        .map(|(i, name)| format!("/{name} cs 1 0 0 sc {} 20 20 30 re f", 2 + 21 * i))
        .collect::<Vec<_>>()
        .join("\n");
    image_cache_pdf(&content, &resources, &[(&dict, &srgb), (&dict, &swapped), (&dict, &srgb)])
}

#[track_caller]
fn assert_near(got: [u8; 3], want: [u8; 3], what: &str) {
    assert!(got.iter().zip(want).all(|(got, want)| got.abs_diff(want) <= 6), "{what}: got {got:?}, expected about {want:?}");
}

/// Each square is drawn through its own profile: red through linear sRGB, blue through the
/// swapped one, whether the profile is an object (5 and 6) or written in its array (`/C` and
/// `/D`), and object 5 looked up again and its copy (object 7) still draw red.
#[test]
fn icc_profiles_with_identical_dictionaries_draw_their_own_colours() {
    let rendered = render_first_page(page());
    assert_eq!((rendered.width as usize, rendered.height), (WIDTH, 70));
    let expected = [
        ("/A, object 5, linear sRGB", RED),
        ("/B, object 6, red and blue swapped", BLUE),
        ("/A2, object 5 again", RED),
        ("/A3, object 7, a copy of object 5", RED),
        ("/C, linear sRGB written in its array", RED),
        ("/D, red and blue swapped written in its array", BLUE),
    ];
    for (i, (what, want)) in expected.into_iter().enumerate() {
        assert_near(rgb_at(&rendered.rgba, WIDTH, 12 + 21 * i, 35), want, what);
    }
}

/// The profiles are still cached: the four different ones (objects 5 and 6, and the two written
/// in their arrays) are read once each, and neither object 5 looked up again nor its copy, object
/// 7, is read again. A second render through the same cache reads none.
#[test]
fn icc_profiles_used_again_are_read_once() {
    use hayro::hayro_interpret::font::Glyph;
    use hayro::hayro_interpret::hayro_syntax::Pdf;
    use hayro::hayro_interpret::{
        BlendMode, ClipPath, Context, Device, GlyphDrawMode, Image, InterpreterCache, InterpreterSettings, Paint, PathDrawMode, SoftMask,
        TransformExt, interpret_page,
    };
    use kurbo::{Affine, BezPath, Rect};
    use std::sync::Arc;

    /// Draws nothing: only the colour spaces the page selects matter here.
    struct Nothing;
    impl<'a> Device<'a> for Nothing {
        fn set_soft_mask(&mut self, _: Option<SoftMask<'a>>) {}
        fn set_blend_mode(&mut self, _: BlendMode) {}
        fn draw_path(&mut self, _: &BezPath, _: Affine, _: &Paint<'a>, _: &PathDrawMode) {}
        fn push_clip_path(&mut self, _: &ClipPath) {}
        fn push_transparency_group(&mut self, _: f32, _: Option<SoftMask<'a>>, _: BlendMode) {}
        fn draw_glyph(&mut self, _: &Glyph<'a>, _: Affine, _: Affine, _: &Paint<'a>, _: &GlyphDrawMode) {}
        fn draw_image(&mut self, _: Image<'a, '_>, _: Affine) {}
        fn pop_clip_path(&mut self) {}
        fn pop_transparency_group(&mut self) {}
    }

    let pdf = Pdf::new(Arc::new(page())).expect("parses");
    let page = &pdf.pages()[0];
    let cache = InterpreterCache::new();
    for render in ["first", "second"] {
        let initial = page.initial_transform(true).to_kurbo();
        let mut ctx = Context::new(initial, Rect::new(0.0, 0.0, 130.0, 70.0), &cache, page.xref(), InterpreterSettings::default());
        interpret_page(page, &mut ctx, &mut Nothing);
        assert_eq!(cache.icc_profile_reads(), 4, "after the {render} render: each different profile is read once");
    }
}
