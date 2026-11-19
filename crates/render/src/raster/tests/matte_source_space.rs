//! A soft mask's `/Matte` is removed in the image's own colour space, before the colours are
//! converted (ISO 32000-1 §11.6.5.3): #7251.

use super::default_colour_spaces::{icc, s15f16, xyz};
use crate::{PageRenderer, RenderConfig, RenderRequest, RenderedPage, RequestKind};
use std::sync::Arc;

/// A page with two 80-point squares on blue, each an image of `width` × 1 pixels in `space`
/// with a soft mask of the same size: on the left `straight` with no `/Matte`, on the right
/// `preblended` with `/Matte [matte]` and the extra image entries `extra` (a `/Decode`, say).
/// `objects`, numbered from 9, are further objects of the page.
fn render_with(
    space: &str,
    width: u32,
    (straight, preblended, mask): (&[u8], &[u8], &[u8]),
    matte: &str,
    extra: &str,
    objects: &[Vec<u8>],
) -> RenderedPage {
    let image = format!("/Type /XObject /Subtype /Image /Width {width} /Height 1 /BitsPerComponent 8");
    let content = "0 0 1 rg 0 0 240 120 re f q 80 0 0 80 20 20 cm /Straight Do Q q 80 0 0 80 140 20 cm /Matted Do Q";
    let stream = |n: u32, dict: String, data: &[u8]| {
        let mut out = format!("{n} 0 obj << {dict} /Length {} >> stream\n", data.len()).into_bytes();
        out.extend_from_slice(data);
        out.extend_from_slice(b"\nendstream endobj\n");
        out
    };
    let mut pdf = b"%PDF-1.7
1 0 obj << /Type /Catalog /Pages 2 0 R >> endobj
2 0 obj << /Type /Pages /Kids [3 0 R] /Count 1 >> endobj
3 0 obj << /Type /Page /Parent 2 0 R /MediaBox [0 0 240 120] /Resources << /XObject << /Straight 4 0 R /Matted 6 0 R >> >> /Contents 8 0 R >> endobj
"
    .to_vec();
    pdf.extend(stream(4, format!("{image} /ColorSpace {space} /SMask 5 0 R"), straight));
    pdf.extend(stream(5, format!("{image} /ColorSpace /DeviceGray"), mask));
    pdf.extend(stream(6, format!("{image} /ColorSpace {space} {extra} /SMask 7 0 R"), preblended));
    pdf.extend(stream(7, format!("{image} /ColorSpace /DeviceGray /Matte [{matte}]"), mask));
    pdf.extend(stream(8, String::new(), content.as_bytes()));
    for object in objects {
        pdf.extend_from_slice(object);
    }
    pdf.extend_from_slice(b"trailer << /Root 1 0 R >>\n%%EOF");
    let mut r = PageRenderer::new(Arc::new(pdf), RenderConfig::default());
    let p = r.render(RenderRequest { page: 0, kind: RequestKind::Pixels, tile: None, scale: 1.0, tag: 0, ..Default::default() });
    assert!(p.error.is_none(), "{space}: {:?}", p.error);
    p
}

/// A 1 × 1 image in `space` of `components` components a pixel.
fn render(space: &str, components: usize, straight: u8, preblended: u8, mask: u8, matte: &str, extra: &str) -> RenderedPage {
    render_with(space, 1, (&vec![straight; components], &vec![preblended; components], &[mask]), matte, extra, &[])
}

/// Object `n`, a stream with the entries `dict` and the content `data`.
fn stream_object(n: u32, dict: &str, data: &[u8]) -> Vec<u8> {
    let mut out = format!("{n} 0 obj << {dict} /Length {} >> stream\n", data.len()).into_bytes();
    out.extend_from_slice(data);
    out.extend_from_slice(b"\nendstream endobj\n");
    out
}

/// The left (straight) and right (preblended) squares' colours.
fn squares(p: &RenderedPage) -> (Vec<u8>, Vec<u8>) {
    let (left, right) = squares_at(p, &[30]);
    (left[0].clone(), right[0].clone())
}

/// The left (straight) and right (preblended) squares' colours at the points `offsets` from
/// their left edge, mid height.
fn squares_at(p: &RenderedPage, offsets: &[u32]) -> (Vec<Vec<u8>>, Vec<Vec<u8>>) {
    let px = |x: u32, y: u32| p.rgba[((y * p.width + x) * 4) as usize..][..3].to_vec();
    (offsets.iter().map(|&o| px(20 + o, 60)).collect(), offsets.iter().map(|&o| px(140 + o, 60)).collect())
}

fn close(a: &[u8], b: &[u8]) -> bool {
    a.iter().zip(b).all(|(a, b)| a.abs_diff(*b) <= 2)
}

/// The issue's fixture: CalGray with `/Gamma 4`, white at half alpha on the left, and on the
/// right the same white preblended with a black matte, 128 = 0 + 128/255 × (255 − 0). Both
/// are the same colour; removing the matte after the gamma drew the right one darker
/// (77,77,204 where 128,128,255 is right).
#[test]
fn a_matte_is_removed_before_a_calgray_image_is_converted() {
    let space = "[/CalGray << /WhitePoint [.9505 1 1.089] /Gamma 4 >>]";
    let (left, right) = squares(&render(space, 1, 255, 128, 128, "0", ""));
    assert!(close(&left, &[128, 128, 255]), "straight: {left:?}");
    assert!(close(&right, &left), "preblended {right:?}, straight {left:?}");
}

/// The issue's second control: CalGray's default gamma and a white matte,
/// 127 = 255 + 128/255 × (0 − 255), so the right square is black at half alpha too.
#[test]
fn a_white_matte_is_removed_before_a_linear_calgray_image_is_converted() {
    let space = "[/CalGray << /WhitePoint [.9505 1 1.089] >>]";
    let (left, right) = squares(&render(space, 1, 0, 127, 128, "1", ""));
    assert!(close(&left, &[0, 0, 127]), "straight: {left:?}");
    assert!(close(&right, &left), "preblended {right:?}, straight {left:?}");
}

/// The matte is removed from the decoded components, after `/Decode`: with `[1 0]` the byte
/// 127 is the component 128/255, white preblended with black at half alpha.
#[test]
fn a_matte_is_removed_after_the_decode_array() {
    let space = "[/CalGray << /WhitePoint [.9505 1 1.089] /Gamma 4 >>]";
    let (left, right) = squares(&render(space, 1, 255, 127, 128, "0", "/Decode [1 0]"));
    assert!(close(&right, &left), "preblended {right:?}, straight {left:?}");
}

/// Device spaces, whose conversion is no conversion, draw as they did.
#[test]
fn a_matte_in_a_device_space_is_removed_as_before() {
    for (space, components, straight, preblended, matte) in
        [("/DeviceGray", 1, 255, 128, "0"), ("/DeviceRGB", 3, 255, 128, "0 0 0"), ("/DeviceRGB", 3, 0, 127, "1 1 1")]
    {
        let (left, right) = squares(&render(space, components, straight, preblended, 128, matte, ""));
        assert!(close(&right, &left), "{space}: preblended {right:?}, straight {left:?}");
    }
}

/// An ICC v2 Lab profile whose `A2B0` lookup (`lut8Type`, ICC.1 §6.5.7) is the identity: the
/// device colour is CIELAB itself, so L* 50, a* 0, b* 0 draws as the mid grey sRGB gives it.
fn lab_profile() -> Vec<u8> {
    let mut lut = [b"mft1".as_slice(), &[0; 4], &[3, 3, 2, 0]].concat();
    // The matrix, only used for XYZ input: identity.
    for value in [1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0] {
        lut.extend_from_slice(&s15f16(value));
    }
    // An identity input table per component.
    for _ in 0..3 {
        lut.extend(0..=255_u8);
    }
    // The grid, two points per component, the last varying fastest. Lab is encoded as L* · 2.55,
    // a* + 128 and b* + 128.
    for point in 0..8 {
        for shift in [2, 1, 0] {
            lut.push(if (point >> shift) & 1 == 1 { 255 } else { 0 });
        }
    }
    // An identity output table per component.
    for _ in 0..3 {
        lut.extend(0..=255_u8);
    }
    icc(b"scnr", b"Lab ", b"Lab ", &[(*b"wtpt", xyz([0.9642, 1.0, 0.8249])), (*b"A2B0", lut)])
}

/// An ICC `/Lab` profile's `/Decode` array gives its components in the profile's own encoding,
/// while its conversion takes physical L\*a\*b\*: an opaque mask, which takes the image off the
/// byte path, draws the colours it draws with no mask at all.
#[test]
fn an_opaque_matte_leaves_an_icc_lab_image_as_it_was() {
    let objects = [stream_object(9, "/N 3", &lab_profile())];
    let samples = [128, 128, 128];
    let (left, right) = squares(&render_with("[/ICCBased 9 0 R]", 1, (&samples, &samples, &[255]), "50 0 0", "", &objects));
    assert!(close(&left, &[119, 119, 119]), "straight: {left:?}");
    assert!(close(&right, &left), "preblended {right:?}, straight {left:?}");
}

/// A tint transform may give fewer components than its alternate space takes. The matte is
/// removed from each pixel's own components, and each converted on its own, so a short result
/// shifts no other pixel's colour.
#[test]
fn a_short_tint_result_shifts_no_other_pixel() {
    // Tint below half gives two components, at or above it three: red.
    let program = b"{ dup 0.5 lt { pop 0 0 } { pop 1 0 0 } ifelse }";
    let function = "/FunctionType 4 /Domain [0 1] /Range [0 1 0 1 0 1]";
    // More pixels than the 8-bit table has entries, so the straight image takes it: tint 0 on
    // the left half, tint 1 on the right.
    let data: Vec<u8> = [vec![0; 256], vec![255; 256]].concat();
    let mask = vec![255_u8; 512];
    let space = "[/Separation /Spot /DeviceRGB 9 0 R]".to_owned();
    let page = render_with(&space, 512, (&data, &data, &mask), "0", "", &[stream_object(9, function, program)]);
    let (left, right) = squares_at(&page, &[30, 50]);
    assert_eq!(left, [[0, 0, 0], [255, 0, 0]], "tint 0, two components: black; tint 1: red");
    assert_eq!(right, left, "preblended {right:?}, straight {left:?}");
}

/// A matte no longer takes a spot-colour image off patch 28's table: a colour is converted once,
/// however many pixels have it.
#[test]
fn a_matted_spot_colour_image_converts_each_colour_once() {
    const SIDE: usize = 512;
    // Tint t → RGB (1 − t, t, 0).
    let program = b"{ dup 1 exch sub exch 0 }";
    let function = "/FunctionType 4 /Domain [0 1] /Range [0 1 0 1 0 1]";
    let data = vec![0_u8; SIDE * SIDE];
    let mask = vec![255_u8; SIDE * SIDE];
    let space = "[/Separation /Spot /DeviceRGB 9 0 R]".to_owned();
    let objects = [stream_object(9, function, program)];
    let before = hayro::hayro_interpret::color::tint_transform_runs();
    let page = render_with(&space, SIDE as u32, (&data, &data, &mask), "0", "", &objects);
    let runs = hayro::hayro_interpret::color::tint_transform_runs() - before;
    assert_eq!(
        runs,
        257,
        "256 for the straight image's table and one for the matted image's one colour, not one for each of its {} pixels",
        SIDE * SIDE
    );
    let (left, right) = squares(&page);
    assert!(close(&right, &left), "preblended {right:?}, straight {left:?}");
}
