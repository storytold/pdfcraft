//! #4886: a non-isolated knockout group (`/I false /K true`) starts from the backdrop under it
//! (ISO 32000-2 §11.4.7, §11.4.8), so each element blends with the page there, in its own blend
//! mode, and replaces the elements before it under its shape. White with `/BM /Difference` over
//! magenta is green; drawn into a layer of its own starting transparent, it stayed white.

use std::sync::Arc;

use pdfcraft_render::{PageRenderer, RenderConfig, RenderRequest, RequestKind};

const MAGENTA: [u8; 3] = [255, 0, 255];
const GREEN: [u8; 3] = [0, 255, 0];
const WHITE: [u8; 3] = [255, 255, 255];

fn pdf(objects: &[String]) -> Vec<u8> {
    let mut out = b"%PDF-1.7\n".to_vec();
    let mut offsets = Vec::new();
    for (i, body) in objects.iter().enumerate() {
        offsets.push(out.len());
        out.extend_from_slice(format!("{} 0 obj\n{body}\nendobj\n", i + 1).as_bytes());
    }
    let xref = out.len();
    out.extend_from_slice(format!("xref\n0 {}\n0000000000 65535 f \n", objects.len() + 1).as_bytes());
    for offset in offsets {
        out.extend_from_slice(format!("{offset:010} 00000 n \n").as_bytes());
    }
    out.extend_from_slice(format!("trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n", objects.len() + 1).as_bytes());
    out
}

fn stream(dict: &str, content: &str) -> String {
    format!("<< {dict} /Length {} >>\nstream\n{content}\nendstream", content.len())
}

/// Renders a 200 x 200 page: magenta, then the group `/Fk` with group attributes `group` and
/// content `content`. Its resources have `/D` (`/BM /Difference`), `/N` (`/BM /Normal`), `/H`
/// (`/ca 0.5`), `/W`, a white square from 40 to 160 in a group of its own, `/Nest`, a group
/// holding a non-isolated knockout group with a white Difference square from 80 to 120, and
/// `/Img`, a white 1 x 1 image, `/Img2`, a blue 2 x 1 image whose soft mask is opaque on the
/// left and transparent on the right, `/St`, a 2 x 1 stencil mask painting its left pixel, and
/// the shading `/Sh0`, a mesh whose one triangle lies off the page, and `/Sh1`, a red mesh
/// triangle from (90, 90) to (110, 90) and (100, 110).
/// Returns a lookup of the colour at PDF point (x, y).
fn render(group: &str, content: &str) -> impl Fn(u32, u32) -> [u8; 3] {
    let gs = "/ExtGState << /D << /BM /Difference >> /H << /ca 0.5 >> /N << /BM /Normal >> >>";
    let objects = [
        "<< /Type /Catalog /Pages 2 0 R >>".to_string(),
        "<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_string(),
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 200 200] /Contents 4 0 R /Group << /S /Transparency /CS /DeviceRGB >> /Resources << /XObject << /Fk 5 0 R >> >> >>"
            .to_string(),
        stream("", "1 0 1 rg 0 0 200 200 re f /Fk Do"),
        stream(
            &format!("/Type /XObject /Subtype /Form /BBox [0 0 200 200] /Group << /S /Transparency /CS /DeviceRGB {group} >> /Resources << {gs} /XObject << /W 6 0 R /Nest 7 0 R /Img 9 0 R /Img2 10 0 R /St 12 0 R >> /Shading << /Sh0 13 0 R /Sh1 14 0 R >> >>"),
            content,
        ),
        stream("/Type /XObject /Subtype /Form /BBox [0 0 200 200] /Group << /S /Transparency >>", "1 1 1 rg 40 40 120 120 re f"),
        stream("/Type /XObject /Subtype /Form /BBox [0 0 200 200] /Group << /S /Transparency >> /Resources << /XObject << /K2 8 0 R >> >>", "/K2 Do"),
        stream(
            &format!("/Type /XObject /Subtype /Form /BBox [0 0 200 200] /Group << /S /Transparency /I false /K true >> /Resources << {gs} >>"),
            "/D gs 1 1 1 rg 80 80 40 40 re f",
        ),
        stream("/Type /XObject /Subtype /Image /Width 1 /Height 1 /ColorSpace /DeviceRGB /BitsPerComponent 8 /Filter /ASCIIHexDecode", "FFFFFF>"),
        stream(
            "/Type /XObject /Subtype /Image /Width 2 /Height 1 /ColorSpace /DeviceRGB /BitsPerComponent 8 /SMask 11 0 R /Filter /ASCIIHexDecode",
            "0000FF0000FF>",
        ),
        stream("/Type /XObject /Subtype /Image /Width 2 /Height 1 /ColorSpace /DeviceGray /BitsPerComponent 8 /Filter /ASCIIHexDecode", "FF00>"),
        stream("/Type /XObject /Subtype /Image /Width 2 /Height 1 /ImageMask true /BitsPerComponent 1 /Filter /ASCIIHexDecode", "40>"),
        stream(
            "/ShadingType 4 /ColorSpace /DeviceRGB /BitsPerCoordinate 8 /BitsPerComponent 8 /BitsPerFlag 8 /Decode [300 310 300 310 0 1 0 1 0 1] /Filter /ASCIIHexDecode",
            "000000FF0000 00FF00FF0000 0000FFFF0000>",
        ),
        stream(
            "/ShadingType 4 /ColorSpace /DeviceRGB /BitsPerCoordinate 8 /BitsPerComponent 8 /BitsPerFlag 8 /Decode [0 255 0 255 0 1 0 1 0 1] /Filter /ASCIIHexDecode",
            "005A5AFF0000 006E5AFF0000 00646EFF0000>",
        ),
    ];
    let mut renderer = PageRenderer::new(Arc::new(pdf(&objects)), RenderConfig::default());
    let page = renderer.render(RenderRequest { page: 0, kind: RequestKind::Pixels, tile: None, scale: 1.0, tag: 0, preview: false });
    assert!(page.error.is_none(), "{content}: {:?}", page.error);
    let (rgba, width) = (page.rgba.to_vec(), page.width);
    move |x, y| {
        // Pixel rows run top down.
        let offset = usize::try_from(((200 - y) * width + x) * 4).unwrap();
        [rgba[offset], rgba[offset + 1], rgba[offset + 2]]
    }
}

fn check(at: &impl Fn(u32, u32) -> [u8; 3], x: u32, y: u32, want: [u8; 3], what: &str) {
    let got = at(x, y);
    assert!(got.iter().zip(want).all(|(a, b)| a.abs_diff(b) <= 2), "{what} at ({x}, {y}): {got:?}, expected {want:?}");
}

const NON_ISOLATED: &str = "/I false /K true";
const SQUARES: &str = "/D gs 1 1 1 rg 20 20 110 110 re f 70 70 110 110 re f";

#[test]
fn a_non_isolated_knockout_group_blends_each_element_with_the_page() {
    let squares = render(NON_ISOLATED, SQUARES);
    check(&squares, 40, 40, GREEN, "first square");
    check(&squares, 160, 160, GREEN, "second square");
    // The second square blends with the page, not with the first: green, not magenta again.
    check(&squares, 100, 100, GREEN, "overlap");
    check(&squares, 190, 10, MAGENTA, "page outside the group's elements");
}

#[test]
fn the_last_element_over_a_point_decides_its_blend_mode() {
    // Normal blue at 50 % after a Difference square: blue over the page where they overlap.
    let blue_last = render(NON_ISOLATED, "/D gs 1 1 1 rg 20 20 110 110 re f /N gs 0 0 1 rg q /H gs 70 70 110 110 re f Q");
    check(&blue_last, 40, 40, GREEN, "Difference alone");
    check(&blue_last, 100, 100, [128, 0, 255], "Normal blue last");
    check(&blue_last, 160, 160, [128, 0, 255], "Normal blue alone");
    // The other way round: the Difference square replaces the blue and blends with the page.
    let white_last = render(NON_ISOLATED, "q /H gs 0 0 1 rg 20 20 110 110 re f Q /D gs 1 1 1 rg 70 70 110 110 re f");
    check(&white_last, 40, 40, [128, 0, 255], "Normal blue alone");
    check(&white_last, 100, 100, GREEN, "Difference last");
}

#[test]
fn a_nested_group_in_a_non_isolated_knockout_group_blends_with_the_page() {
    let nested = render(NON_ISOLATED, "0 0 1 rg 20 20 160 160 re f /D gs /W Do");
    check(&nested, 100, 100, GREEN, "nested group drawn with Difference over the knocked-out blue");
    check(&nested, 30, 30, [0, 0, 255], "blue beside the nested group");
}

#[test]
fn an_isolated_knockout_group_still_starts_transparent() {
    let isolated = render("/I true /K true", SQUARES);
    check(&isolated, 40, 40, WHITE, "isolated: nothing to blend with");
    check(&isolated, 100, 100, WHITE, "isolated overlap");
}

#[test]
fn an_image_and_a_partly_transparent_element_blend_with_the_page() {
    let image = render(NON_ISOLATED, "/D gs q 120 0 0 120 40 40 cm /Img Do Q");
    check(&image, 100, 100, GREEN, "white image drawn with Difference");
    check(&image, 20, 20, MAGENTA, "page beside the image");
    // Difference at 50 %: half the page, half the blend result (green).
    let half = render(NON_ISOLATED, "/D gs /H gs 1 1 1 rg 40 40 120 120 re f");
    check(&half, 100, 100, [128, 128, 128], "Difference at 50 %");
}

/// A knockout group nested in another one's element is drawn first in that element's erase
/// pass, which must leave the outer group's passes alone (review of #4886's fix).
#[test]
fn a_knockout_group_inside_an_element_keeps_the_outer_groups_passes() {
    let nested = render(NON_ISOLATED, "/D gs 1 1 1 rg 10 10 50 50 re f /N gs /Nest Do /D gs 1 1 1 rg 140 140 50 50 re f");
    check(&nested, 30, 30, GREEN, "Difference square before the nested groups");
    check(&nested, 165, 165, GREEN, "Difference square after the nested groups");
}

/// A masked image (or, below, a stencil) in another pass than its own replaces what is under its mask
/// there too: a blue image over a white Difference square stays blue where its mask is opaque,
/// and leaves the square to blend with the page where it is transparent (review of #4886's fix).
#[test]
fn a_masked_image_replaces_what_is_under_its_mask_in_every_pass() {
    let masked = render(NON_ISOLATED, "/D gs 1 1 1 rg 40 40 120 120 re f /N gs q 120 0 0 120 40 40 cm /Img2 Do Q");
    check(&masked, 60, 100, [0, 0, 255], "image over the square, mask opaque");
    check(&masked, 140, 100, GREEN, "square under the transparent half of the mask");
}

#[test]
fn a_stencil_replaces_what_is_under_it_in_every_pass() {
    let stencil = render(NON_ISOLATED, "/D gs 1 1 1 rg 40 40 120 120 re f /N gs 0 0 1 rg q 120 0 0 120 40 40 cm /St Do Q");
    check(&stencil, 60, 100, [0, 0, 255], "stencil over the square");
    check(&stencil, 140, 100, GREEN, "square beside the stencil");
}

/// A shading that paints nothing (a mesh off the page) replaces nothing in another pass either.
#[test]
fn a_shading_that_paints_nothing_clears_nothing_in_another_pass() {
    let mesh = render(NON_ISOLATED, "0 0 1 rg 20 20 160 160 re f /D gs /Sh0 sh");
    check(&mesh, 100, 100, [0, 0, 255], "blue under an off-page Difference mesh");
}

/// A shading replaces only what is under the part it paints: a small mesh triangle over a
/// square leaves the rest of the square, in its own pass, in another pass and in an isolated
/// knockout group.
#[test]
fn a_small_mesh_replaces_only_what_is_under_its_triangle() {
    let square = "0 0 1 rg 20 20 160 160 re f";
    for (group, mode) in [(NON_ISOLATED, ""), (NON_ISOLATED, "/D gs"), ("/K true", "")] {
        let content = format!("{square} {mode} /Sh1 sh");
        let mesh = render(group, &content);
        check(&mesh, 40, 40, [0, 0, 255], &format!("{group} {mode}: square beside the triangle"));
        if mode.is_empty() {
            check(&mesh, 100, 97, [255, 0, 0], &format!("{group}: triangle"));
        }
    }
}
