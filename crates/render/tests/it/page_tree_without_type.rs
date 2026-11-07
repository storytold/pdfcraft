//! Contributor-original synthetic page trees whose intermediate nodes have `/Kids` and `/Count`
//! but no `/Type` (ISO 32000-2 §7.7.3.2 requires `/Type /Pages`; writers leave it out). A node is
//! told from a page by its `/Kids` (a page has none, §7.7.3.3), so the pages under such nodes are
//! all read, with the attributes they inherit, and a leaf without `/Type` is still a page.
use pdfcraft_render::{PageRenderer, RenderConfig, RenderRequest, RequestKind, inspect};
use std::sync::Arc;

/// The words drawn on the pages, in page order.
const WORDS: [&str; 5] = ["ONE", "TWO", "THREE", "FOUR", "FIVE"];

/// Root (typed) → [node 3, page FIVE]; node 3 (no `/Type`, `/MediaBox` 300 × 200) → [node 4,
/// page FOUR (no `/Type` either)]; node 4 (no `/Type`) → [ONE, TWO (`/Type /Page` and an empty
/// `/Kids`), THREE]. Page FIVE has its own 400 × 400 box and a text annotation.
fn pdf() -> Arc<Vec<u8>> {
    let font = "<< /Font << /F1 << /Type /Font /Subtype /Type1 /BaseFont /Helvetica >> >> >>";
    let page = |typed: bool, parent: u32, content: u32, extra: &str| {
        let ty = if typed { "/Type /Page " } else { "" };
        format!("<< {ty}/Parent {parent} 0 R /Resources {font} /Contents {content} 0 R {extra}>>").into_bytes()
    };
    let mut objects: Vec<Vec<u8>> = vec![
        b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
        b"<< /Type /Pages /Kids [3 0 R 10 0 R] /Count 5 /MediaBox [0 0 612 792] >>".to_vec(),
        b"<< /Parent 2 0 R /Kids [4 0 R 9 0 R] /Count 4 /MediaBox [0 0 300 200] >>".to_vec(),
        b"<< /Parent 3 0 R /Kids [6 0 R 7 0 R 8 0 R] /Count 3 >>".to_vec(),
        // 5: unused, keeps the numbering below readable.
        b"null".to_vec(),
        page(true, 4, 11, ""),
        // A `/Type /Page` with a stray `/Kids` is still a page.
        page(true, 4, 12, "/Kids [] "),
        page(true, 4, 13, ""),
        page(false, 3, 14, ""),
        page(true, 2, 15, "/MediaBox [0 0 400 400] /Annots [<< /Type /Annot /Subtype /Text /Rect [10 10 30 30] /Contents (note) >>] "),
    ];
    for word in WORDS {
        objects.push(stream(format!("BT /F1 24 Tf 20 100 Td ({word}) Tj ET").as_bytes()));
    }
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
    Arc::new(out)
}

fn stream(body: &[u8]) -> Vec<u8> {
    let mut s = format!("<< /Length {} >>\nstream\n", body.len()).into_bytes();
    s.extend_from_slice(body);
    s.extend_from_slice(b"\nendstream");
    s
}

fn request(page: usize, kind: RequestKind) -> RenderRequest {
    RenderRequest { page, kind, tile: None, scale: 1.0, tag: 0, preview: false }
}

#[test]
fn pages_under_nodes_without_a_type_are_all_read_with_their_text_and_ink() {
    let mut renderer = PageRenderer::new(pdf(), RenderConfig::default());
    assert_eq!(renderer.page_count(), WORDS.len());
    for (page, word) in WORDS.iter().enumerate() {
        let text = renderer.render(request(page, RequestKind::Text));
        assert!(text.error.is_none(), "page {} text error: {:?}", page + 1, text.error);
        assert_eq!(text.text.expect("page text").plain_text().trim(), *word, "page {}", page + 1);
        let pixels = renderer.render(request(page, RequestKind::Pixels));
        assert!(pixels.error.is_none(), "page {} render error: {:?}", page + 1, pixels.error);
        let ink = pixels.rgba.as_chunks::<4>().0.iter().filter(|[r, g, b, _]| *r < 128 && *g < 128 && *b < 128).count();
        assert!(ink > 20, "page {} draws {word}: {ink} dark pixels", page + 1);
    }
}

#[test]
fn inspection_numbers_the_same_pages_and_their_inherited_boxes() {
    let info = inspect(pdf(), None).expect("inspect");
    let sizes: Vec<(f32, f32)> = info.pages.iter().map(|p| (p.width, p.height)).collect();
    // ONE to FOUR inherit node 3's box through node 4; FIVE has its own.
    assert_eq!(sizes, [(300.0, 200.0), (300.0, 200.0), (300.0, 200.0), (300.0, 200.0), (400.0, 400.0)]);
    let notes: Vec<usize> = info.annotations.iter().filter(|a| a.subtype == "Text").map(|a| a.page).collect();
    assert_eq!(notes, [4], "the annotation is on page FIVE, the fifth page");
}
