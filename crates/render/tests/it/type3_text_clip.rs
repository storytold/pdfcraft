//! Contributor-original synthetic pages: text in a Type 3 font in the text rendering modes
//! (ISO 32000-2 §9.3.6, Table 104). For a Type 3 font, modes 3 and 7 render nothing, the other
//! modes render the glyph descriptions, and modes 4 to 7 add nothing to the clipping path
//! (vendored hayro-interpret patch 64).
use pdfcraft_render::{PageRenderer, RenderConfig, RenderRequest, RequestKind};
use std::sync::Arc;

/// Glyph procedures in a 1000-unit glyph space (`/FontMatrix [0.001 0 0 0.001 0 0]`), named
/// by the letters that show them: the `/Differences` give them codes 65 (`A`) on, in order.
const GLYPHS: &[(&str, &[u8])] = &[
    // A: a square, x 100..900, y 0..800.
    ("A", b"1000 0 100 0 900 800 d1 100 0 800 800 re f"),
    // B: a triangle on the baseline with its apex at the top middle.
    ("B", b"1000 0 0 0 1000 1000 d1 0 0 m 1000 0 l 500 1000 l f"),
    // C: a coloured glyph (`d0`): a blue square.
    ("C", b"1000 0 d0 0 0 1 rg 100 0 800 800 re f"),
    // D: shows four of itself, nesting until the paint budget runs out.
    ("D", b"1000 0 0 0 1000 1000 d1 BT /T3 1000 Tf 0 0 Td (DDDD) Tj ET"),
];

fn pdf(content: &str) -> Vec<u8> {
    let names: String = GLYPHS.iter().map(|(n, _)| format!("/{n} ")).collect();
    let procs: String = GLYPHS.iter().enumerate().map(|(i, (n, _))| format!("/{n} {} 0 R ", 6 + i)).collect();
    let widths = vec!["1000"; GLYPHS.len()].join(" ");
    let mut objects: Vec<Vec<u8>> = vec![
        b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
        b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_vec(),
        b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 100 100] /Contents 4 0 R /Resources << /Font << /T3 5 0 R /F1 << /Type /Font /Subtype /Type1 /BaseFont /Helvetica >> >> >> >>".to_vec(),
        stream(content.as_bytes()),
        format!(
            "<< /Type /Font /Subtype /Type3 /FontBBox [0 0 1000 1000] /FontMatrix [0.001 0 0 0.001 0 0] \
             /CharProcs << {procs}>> /Encoding << /Type /Encoding /Differences [65 {names}] >> \
             /FirstChar 65 /LastChar {} /Widths [{widths}] >>",
            64 + GLYPHS.len()
        )
        .into_bytes(),
    ];
    objects.extend(GLYPHS.iter().map(|(_, body)| stream(body)));
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

fn stream(body: &[u8]) -> Vec<u8> {
    let mut s = format!("<< /Length {} >>\nstream\n", body.len()).into_bytes();
    s.extend_from_slice(body);
    s.extend_from_slice(b"\nendstream");
    s
}

/// A 100 x 100 pixel rendering of a 100 x 100 pt page; `at(x, y)` reads the pixel at page
/// coordinates (y up).
struct Page(Vec<u8>);

impl Page {
    fn render(content: &str) -> Self {
        let mut renderer = PageRenderer::new(Arc::new(pdf(content)), RenderConfig::default());
        let page = renderer.render(RenderRequest { page: 0, kind: RequestKind::Pixels, scale: 1.0, ..Default::default() });
        assert!(page.error.is_none(), "{content}: {:?}", page.error);
        assert_eq!((page.width, page.height), (100, 100), "{content}");
        Self(page.rgba.to_vec())
    }

    fn at(&self, x: usize, y: usize) -> [u8; 3] {
        let i = ((99 - y) * 100 + x) * 4;
        match self.0.get(i..i + 3) {
            Some([r, g, b]) => [*r, *g, *b],
            _ => panic!("pixel {x},{y} is off the page"),
        }
    }

    /// Whether any pixel is dark in every channel: a glyph painted in the default black.
    fn has_black(&self) -> bool {
        self.0.as_chunks::<4>().0.iter().any(|p| p[0] < 128 && p[1] < 128 && p[2] < 128)
    }
}

const RED: [u8; 3] = [255, 0, 0];
const BLUE: [u8; 3] = [0, 0, 255];
const WHITE: [u8; 3] = [255, 255, 255];

/// `ABA` at 20 pt from (10, 40): squares x 12..28 and 52..68 (y 40..56), and a triangle from
/// (30, 40) and (50, 40) up to (40, 60).
const ABA: &str = "/T3 20 Tf 10 40 Td (ABA) Tj";

#[test]
fn clip_only_type3_text_is_not_rendered_and_leaves_no_clip() {
    // The fill stops at x 60, inside the second square: a glyph painted under `7 Tr` would show
    // past it as a bar beside the box.
    let page = Page::render(&format!("BT 7 Tr {ABA} ET 1 0 0 rg 0 0 60 100 re f"));
    assert_eq!(page.at(20, 48), RED, "the fill over the first square");
    assert_eq!(page.at(5, 5), RED, "no clip: the fill covers its whole box");
    assert_eq!(page.at(29, 45), RED, "no clip between the glyphs either");
    assert_eq!(page.at(33, 57), RED, "no clip beside the triangle");
    assert_eq!(page.at(64, 48), WHITE, "the second square past the fill is not rendered");
    assert_eq!(page.at(80, 80), WHITE, "outside the fill");
    assert!(!page.has_black(), "7 Tr renders no glyph");

    // Alone, neither shape glyphs nor a coloured glyph leave a mark.
    for text in [ABA, "/T3 20 Tf 10 40 Td (C) Tj"] {
        let page = Page::render(&format!("BT 7 Tr {text} ET"));
        assert!(page.0.as_chunks::<4>().0.iter().all(|p| p[..3] == WHITE), "7 Tr {text} renders nothing");
    }
}

#[test]
fn invisible_type3_text_is_not_rendered() {
    for text in [ABA, "/T3 20 Tf 10 40 Td (C) Tj"] {
        let page = Page::render(&format!("BT 3 Tr {text} ET"));
        assert!(page.0.as_chunks::<4>().0.iter().all(|p| p[..3] == WHITE), "3 Tr {text} renders nothing");
    }
}

#[test]
fn other_modes_render_type3_glyphs_filled_and_never_clip() {
    for mode in [0, 1, 2, 4, 5, 6] {
        let page = Page::render(&format!("BT 0 0 1 rg 1 1 0 RG 6 w {mode} Tr {ABA} ET 1 0 0 rg 0 0 60 100 re f"));
        assert_eq!(page.at(64, 48), BLUE, "{mode} Tr: the glyph is rendered, in the fill colour");
        assert_eq!(page.at(70, 48), WHITE, "{mode} Tr: and not stroked");
        assert_eq!(page.at(20, 48), RED, "{mode} Tr: the later fill over the glyph");
        assert_eq!(page.at(5, 5), RED, "{mode} Tr: the later fill is not clipped");
        assert_eq!(page.at(29, 45), RED, "{mode} Tr: not clipped between the glyphs");
        assert!(!page.has_black(), "{mode} Tr");
    }
    // A coloured glyph keeps its own colour.
    let page = Page::render("BT 1 0 0 rg 4 Tr /T3 20 Tf 10 40 Td (C) Tj ET");
    assert_eq!(page.at(20, 48), BLUE);
}

#[test]
fn only_outline_glyphs_shape_a_clip_shared_with_type3_text() {
    // `I` in Helvetica at 40 pt from (60, 10): its stem covers (65, 20). The square A from
    // (10, 40) at 20 pt adds nothing to the clip and is not rendered.
    let page = Page::render("BT 7 Tr /F1 40 Tf 60 10 Td (I) Tj /T3 20 Tf -50 30 Td (A) Tj ET 1 0 0 rg 0 0 100 100 re f");
    assert_eq!(page.at(65, 20), RED, "inside the outline glyph's clip");
    assert_eq!(page.at(20, 48), WHITE, "the Type 3 square neither clips nor paints");
    assert_eq!(page.at(5, 5), WHITE, "outside the clip");
    assert!(!page.has_black());
}

#[test]
fn self_nesting_type3_text_in_a_clip_mode_stays_bounded() {
    // Every D shows four Ds: unbounded this is 4^n procedures. The paint nesting depth and the
    // page's paint budget stop it; the page must still render.
    for mode in [4, 6, 7] {
        let content = format!("BT {mode} Tr /T3 20 Tf 10 40 Td (DD) Tj ET 1 0 0 rg 0 0 100 100 re f");
        let page = Page::render(&content);
        assert_eq!(page.at(5, 5), RED, "{content}: no clip");
        assert!(!page.has_black(), "{content}");
    }
}
