//! Rectangular clips wait until a draw needs them (vendored hayro patch 33): a CAD export that
//! wraps each object in a clip of its own viewport draws the same with and without waiting, and
//! puts far fewer clips on the rasterizer.

use super::*;

/// A page of `content` (240 × 240 pt) with a Helvetica `F1`, a 4 × 4 image `Im0`, a 25%-opacity
/// graphics state `G0`, a graphics state `G1` with a luminosity soft mask, a group form `X0` and
/// an axial shading `Sh0`.
fn page(content: &str) -> Vec<u8> {
    let pixels = "A".repeat(4 * 4 * 3);
    let form = "0 0 1 rg 0 0 40 40 re f";
    let mask = "1 g 0 0 120 240 re f";
    format!(
        "%PDF-1.4
1 0 obj << /Type /Catalog /Pages 2 0 R >> endobj
2 0 obj << /Type /Pages /Kids [3 0 R] /Count 1 >> endobj
3 0 obj << /Type /Page /Parent 2 0 R /MediaBox [0 0 240 240] /Contents 4 0 R /Resources << /Font << /F1 5 0 R >> /XObject << /Im0 6 0 R /X0 7 0 R >> /ExtGState << /G0 8 0 R /G1 9 0 R >> /Shading << /Sh0 11 0 R >> >> >> endobj
4 0 obj << /Length {} >> stream
{content}
endstream endobj
5 0 obj << /Type /Font /Subtype /Type1 /BaseFont /Helvetica >> endobj
6 0 obj << /Type /XObject /Subtype /Image /Width 4 /Height 4 /ColorSpace /DeviceRGB /BitsPerComponent 8 /Length {} >> stream
{pixels}
endstream endobj
7 0 obj << /Type /XObject /Subtype /Form /BBox [0 0 40 40] /Group << /S /Transparency >> /Length {} >> stream
{form}
endstream endobj
8 0 obj << /ca 0.25 >> endobj
9 0 obj << /SMask << /S /Luminosity /G 10 0 R >> >> endobj
10 0 obj << /Type /XObject /Subtype /Form /BBox [0 0 240 240] /Group << /S /Transparency /CS /DeviceGray >> /Length {} >> stream
{mask}
endstream endobj
11 0 obj << /ShadingType 2 /ColorSpace /DeviceRGB /Coords [160 0 230 0] /Function << /FunctionType 2 /Domain [0 1] /C0 [1 1 0] /C1 [0 0.5 1] /N 1 >> >> endobj
trailer << /Root 1 0 R >>
%%EOF",
        content.len(),
        pixels.len(),
        form.len(),
        mask.len()
    )
    .into_bytes()
}

/// `pdf`'s page rendered at `scale` with clips waiting or not, and how many clips reached the
/// rasterizer.
fn render(pdf: &[u8], scale: f32, wait: bool) -> (RenderedPage, u64) {
    let mut r = PageRenderer::new(Arc::new(pdf.to_vec()), RenderConfig::default());
    hayro::set_defer_clips(wait);
    let before = hayro::device_clips();
    let out = r.render(RenderRequest { scale, ..Default::default() });
    let clips = hayro::device_clips() - before;
    hayro::set_defer_clips(true);
    assert!(out.error.is_none(), "{:?}", out.error);
    (out, clips)
}

/// Every kind of draw under a waiting clip: inside it, crossing it, past it entirely, nested
/// clips with a later draw crossing only the outer one, a clip that isn't a rectangle, a mitred
/// corner reaching past a clip's edge, and text, an image, a group, a translucent fill, a fill
/// under a soft mask and a shading under a waiting clip.
const MIXED: &str = "q 20 20 200 200 re W n
q 30 30 40 40 re W n 1 0 0 rg 35 35 30 30 re f Q
q 30 80 40 40 re W n 0 0 0 RG 2 w 20 100 m 80 100 l S Q
q 30 130 40 40 re W n 0 0.6 0 rg 100 130 20 20 re f Q
q 80 30 60 60 re W n q 90 40 20 20 re W n 0 0 1 rg 92 42 16 16 re f Q 1 0.5 0 rg 70 50 30 10 re f Q
q 80 100 m 140 100 l 110 150 l h W n 0.5 0 0.5 rg 80 100 60 50 re f Q
q 150 30 60 40 re W n BT /F1 18 Tf 152 40 Td (Hg) Tj ET Q
q 150 80 60 60 re W n q 40 0 0 40 160 90 cm /Im0 Do Q Q
q 150 150 60 60 re W n q 1 0 0 1 160 160 cm /X0 Do Q Q
q 30 180 60 30 re W n /G0 gs 0 0 0 rg 35 185 50 20 re f Q
q 100 140 40 30 re W n 0 0 0 RG 6 w 0 j 10 M 105 145 m 120 167 l 135 145 l S Q
q 100 180 50 40 re W n /G1 gs 1 0 0 rg 105 185 40 30 re f Q
q 160 215 60 20 re W n /Sh0 sh Q
q 30 30 40 40 re W n 0 0.5 0.5 rg 25 25 50 50 re 40 40 20 20 re f* Q
Q";

#[test]
fn waiting_clips_change_no_pixel() {
    let pdf = page(MIXED);
    // Shrunk, at size and enlarged: the paint boxes and clips land on other pixel edges.
    for scale in [1.0, 2.0, 0.37, 3.0] {
        let (waited, _) = render(&pdf, scale, true);
        let (applied, _) = render(&pdf, scale, false);
        assert!(waited.rgba == applied.rgba, "scale {scale}: the page changed");
    }
}

/// One rectangle clip per object, each object inside it (as CAD exports draw): the page's own
/// crop clip aside, none reaches the rasterizer.
#[test]
fn objects_inside_their_own_clips_put_none_on_the_rasterizer() {
    let mut content = String::from("q 5 5 230 230 re W n\n");
    for i in 0..400 {
        let (x, y) = (10 + (i % 20) * 11, 10 + (i / 20) * 11);
        // Round joins, so a stroke's reach is its half width, not ten of them (a miter's limit).
        content.push_str(&format!(
            "q {x} {y} 10 10 re W n 0 0 0 rg {} {} 6 6 re f 0 0 0 RG 0.5 w 1 j {} {} m {} {} l S Q\n",
            x + 2,
            y + 2,
            x + 3,
            y + 5,
            x + 7,
            y + 5
        ));
    }
    content.push('Q');
    let pdf = page(&content);
    let (waited, clips) = render(&pdf, 2.0, true);
    let (applied, all) = render(&pdf, 2.0, false);
    assert!(waited.rgba == applied.rgba, "the page changed");
    // The page's own clip (its crop box) and the outer one come on top of the objects' 400.
    assert!(all >= 401, "without waiting every clip is applied: {all}");
    assert_eq!(all - clips, 401, "waiting, none of the objects' clips is, nor the outer one: {clips} of {all}");
}
