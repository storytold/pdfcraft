//! Contributor-original synthetic pages for combined fill-and-stroke operators (`B`, `B*`, `b`,
//! `b*`) and text rendering modes 2 and 6 (`FillStroke`, `FillAndStrokeAndClip`) under
//! transparency, blend modes, and soft masks (ISO 32000-2 §11.7.4.4).

use pdfcraft_render::{PageRenderer, RenderConfig, RenderRequest, RequestKind};
use std::sync::Arc;

fn pdf_with_content(content: &str) -> Vec<u8> {
    let mask_stream = b"0.5 g 0 0 200 200 re f";
    let objects: Vec<Vec<u8>> = vec![
        b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
        b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_vec(),
        b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 200 200] \
          /Group << /S /Transparency /CS /DeviceRGB >> \
          /Contents 4 0 R \
          /Resources << \
            /Font << /F1 << /Type /Font /Subtype /Type1 /BaseFont /Helvetica-Bold >> >> \
            /ExtGState << \
              /GS50 << /Type /ExtGState /ca 0.5 /CA 0.5 >> \
              /GSStroke50 << /Type /ExtGState /ca 1.0 /CA 0.5 >> \
              /GSMul << /Type /ExtGState /BM /Multiply /ca 1.0 /CA 1.0 >> \
              /GSMask << /Type /ExtGState /SMask << /Type /Mask /S /Luminosity /G 5 0 R >> >> \
            >> \
          >> >>"
            .to_vec(),
        stream(b"", content.as_bytes()),
        stream(
            b"/Type /XObject /Subtype /Form /BBox [0 0 200 200] \
              /Group << /S /Transparency /CS /DeviceGray /I true >> ",
            mask_stream,
        ),
    ];

    let mut out = b"%PDF-1.7\n".to_vec();
    let mut offsets = Vec::new();
    for (i, object) in objects.iter().enumerate() {
        offsets.push(out.len());
        out.extend_from_slice(format!("{} 0 obj\n", i + 1).as_bytes());
        out.extend_from_slice(object);
        out.extend_from_slice(b"\nendobj\n");
    }
    let xref = out.len();
    out.extend_from_slice(format!("xref\n0 {}\n0000000000 65535 f \n", objects.len() + 1).as_bytes());
    for offset in offsets {
        out.extend_from_slice(format!("{offset:010} 00000 n \n").as_bytes());
    }
    out.extend_from_slice(format!("trailer << /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n", objects.len() + 1).as_bytes());
    out
}

fn stream(dict_entries: &[u8], body: &[u8]) -> Vec<u8> {
    let mut s = b"<< ".to_vec();
    s.extend_from_slice(dict_entries);
    s.extend_from_slice(format!("/Length {} >>\nstream\n", body.len()).as_bytes());
    s.extend_from_slice(body);
    s.extend_from_slice(b"\nendstream");
    s
}

struct Page(Vec<u8>);

impl Page {
    fn render(content: &str) -> Self {
        let mut renderer = PageRenderer::new(Arc::new(pdf_with_content(content)), RenderConfig::default());
        let page = renderer.render(RenderRequest { page: 0, kind: RequestKind::Pixels, scale: 1.0, ..Default::default() });
        assert!(page.error.is_none(), "{content}: {:?}", page.error);
        assert_eq!((page.width, page.height), (200, 200), "{content}");
        Self(page.rgba.to_vec())
    }

    fn at(&self, x: usize, y: usize) -> [u8; 3] {
        let i = ((199 - y) * 200 + x) * 4;
        match self.0.get(i..i + 3) {
            Some([r, g, b]) => [*r, *g, *b],
            _ => panic!("pixel ({x}, {y}) is out of bounds"),
        }
    }

    fn assert_close(&self, x: usize, y: usize, want: [u8; 3], label: &str) {
        let got = self.at(x, y);
        assert!(got.iter().zip(want).all(|(a, b)| a.abs_diff(b) <= 2), "{label} at ({x}, {y}): got {got:?}, expected {want:?}");
    }
}

#[test]
fn fill_stroke_path_knocks_out_fill_under_stroke() {
    // 50% red fill (`ca = 0.5`) and 50% blue 20 pt stroke (`CA = 0.5`) on a white page.
    // The stroke spans x = 30..50 along the left edge of the 40..160 square:
    // - outer half of stroke at (35, 100) is over white -> [128, 128, 255]
    // - inner half of stroke at (45, 100) knocks out the fill -> [128, 128, 255]
    // - interior fill at (100, 100) is 50% red over white -> [255, 128, 128]
    for op in ["40 40 120 120 re B", "40 40 120 120 re B*", "40 40 m 160 40 l 160 160 l 40 160 l b", "40 40 m 160 40 l 160 160 l 40 160 l b*"] {
        let page = Page::render(&format!("q /GS50 gs 1 0 0 rg 0 0 1 RG 20 w {op} Q"));
        page.assert_close(100, 100, [255, 128, 128], &format!("{op} fill center"));
        page.assert_close(35, 100, [128, 128, 255], &format!("{op} outer stroke"));
        page.assert_close(45, 100, [128, 128, 255], &format!("{op} inner stroke"));
    }

    // Even when fill alpha is 1.0 (`ca = 1.0`) and only stroke alpha is 0.5 (`CA = 0.5`),
    // the semi-transparent stroke knocks out the opaque fill beneath it and composites
    // directly with the white backdrop.
    let page = Page::render("q /GSStroke50 gs 1 0 0 rg 0 0 1 RG 20 w 40 40 120 120 re B Q");
    page.assert_close(100, 100, [255, 0, 0], "opaque fill center");
    page.assert_close(35, 100, [128, 128, 255], "CA=0.5 outer stroke");
    page.assert_close(45, 100, [128, 128, 255], "CA=0.5 inner stroke knocks out opaque fill");

    // Control: separate `f` followed by `S` are two distinct graphics objects, so the stroke
    // composites over the already-painted fill: 0.5 * (0, 0, 255) + 0.5 * (255, 128, 128) = (128, 64, 191).
    let separate = Page::render("q /GS50 gs 1 0 0 rg 0 0 1 RG 20 w 40 40 120 120 re f 40 40 120 120 re S Q");
    separate.assert_close(35, 100, [128, 128, 255], "separate outer stroke");
    separate.assert_close(45, 100, [128, 64, 191], "separate inner stroke over fill");
}

#[test]
fn fill_stroke_path_applies_blend_mode_and_soft_mask_once() {
    // Under `/BM /Multiply` on a (0.8, 0.8, 0.5) backdrop, `B` knocks out the yellow fill
    // (1.0, 0.8, 0.0) under the cyan stroke (0.0, 0.8, 1.0) before multiplying with the backdrop:
    // - fill center: (1.0 * 0.8, 0.8 * 0.8, 0.0 * 0.5) = [204, 163, 0]
    // - both halves of the stroke: (0.0 * 0.8, 0.8 * 0.8, 1.0 * 0.5) = [0, 163, 128]
    let mul = Page::render(
        "0.8 0.8 0.5 rg 0 0 200 200 re f \
         q /GSMul gs 1 0.8 0 rg 0 0.8 1 RG 20 w 40 40 120 120 re B Q",
    );
    mul.assert_close(100, 100, [204, 163, 0], "Multiply fill center");
    mul.assert_close(35, 100, [0, 163, 128], "Multiply outer stroke");
    mul.assert_close(45, 100, [0, 163, 128], "Multiply inner stroke");

    // Under a 50% luminosity `/SMask`, `B` masks the combined knockout group once:
    // - fill center: 50% red over white = [255, 128, 128]
    // - both halves of the stroke: 50% blue over white = [128, 128, 255]
    let masked = Page::render("q /GSMask gs 1 0 0 rg 0 0 1 RG 20 w 40 40 120 120 re B Q");
    masked.assert_close(100, 100, [255, 128, 128], "SMask fill center");
    masked.assert_close(35, 100, [128, 128, 255], "SMask outer stroke");
    masked.assert_close(45, 100, [128, 128, 255], "SMask inner stroke");
}

#[test]
fn text_rendering_modes_2_and_6_knock_out_fill_under_stroke() {
    for mode in [2, 6] {
        let page = Page::render(&format!("q /GS50 gs 1 0 0 rg 0 0 1 RG 12 w BT /F1 120 Tf 30 50 Td {mode} Tr (I) Tj ET Q"));
        // Both the 50% red fill ([255, 128, 128]) and the 50% blue stroke ([128, 128, 255]) over
        // white have G >= 128. With knockout, every pixel (fill, inner stroke, outer stroke, and
        // anti-aliased edges) stays at G >= 126; without knockout, the inner half of the stroke
        // composites over the fill to [128, 64, 191] (G = 64).
        let mut stroke_pixels = 0usize;
        let mut fill_pixels = 0usize;
        for &[r, g, b, _] in page.0.as_chunks::<4>().0 {
            assert!(g >= 126, "mode {mode} Tr pixel [{r}, {g}, {b}] composited stroke over fill");
            if b >= 253 && r <= 130 {
                stroke_pixels += 1;
            } else if r >= 253 && b <= 130 {
                fill_pixels += 1;
            }
        }
        assert!(stroke_pixels > 200, "mode {mode} Tr should paint stroke pixels");
        assert!(fill_pixels > 100, "mode {mode} Tr should paint interior fill pixels");
    }

    // Mode 6 (`FillAndStrokeAndClip`) also clips subsequent drawing after `ET` to the glyph outline.
    let clipped = Page::render(
        "q /GS50 gs 1 0 0 rg 0 0 1 RG 12 w BT /F1 120 Tf 30 50 Td 6 Tr (I) Tj ET \
         1.0 ca 0 1 0 rg 0 0 200 200 re f Q",
    );
    // Outside the glyph (10, 10), the green rectangle is clipped away.
    clipped.assert_close(10, 10, [255, 255, 255], "outside 6 Tr glyph clip");
}
