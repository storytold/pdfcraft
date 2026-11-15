//! Contributor-original synthetic files: a page dictionary holding a stray delimiter (`)`,
//! `]`, a brace or a lone `>`) between its entries (`/Contents 6 0 R ) >>`). A dictionary is a
//! sequence of key-value pairs (ISO 32000-2 §7.3.7), so the byte is garbage between entries:
//! it is skipped and the dictionary keeps every entry around it (vendored hayro-syntax patch
//! 51). Before, the byte failed the whole dictionary, and the page-tree walk (§7.7.3) ended
//! at that page: it and every later page were missing, with no error.
use pdfcraft_render::{PageRenderer, PageText, RenderConfig, RenderRequest, RequestKind};
use std::sync::Arc;

/// The bytes that start no object and so were not skipped as garbage before.
const STRAY: [&str; 6] = [")", "]", "{", "}", ">", ") ] )"];

const PAGES: usize = 4;

/// Four 400 × 400 pages, each writing `PAGE<n>`; the second page's dictionary holds `stray`
/// after its `/Contents` entry and before `/Rotate 0`, so the entry after the byte is read too.
fn pdf(stray: &str) -> Arc<Vec<u8>> {
    pdf_with(stray, "")
}

/// As [`pdf`], with `nested` placed inside the second page's direct `/Resources` dictionary,
/// before `/Font`: a nested dictionary is stepped over by the skipping parser, not read.
fn pdf_with(stray: &str, nested: &str) -> Arc<Vec<u8>> {
    let kids: String = (0..PAGES).map(|i| format!("{} 0 R ", 4 + 2 * i)).collect();
    let mut objects: Vec<Vec<u8>> = vec![
        b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
        format!("<< /Type /Pages /Kids [{kids}] /Count {PAGES} >>").into_bytes(),
        b"<< /Font << /F1 << /Type /Font /Subtype /Type1 /BaseFont /Helvetica >> >> >>".to_vec(),
    ];
    for i in 0..PAGES {
        let (junk, resources) = if i == 1 && !nested.is_empty() {
            (stray, format!("<< {nested} /Font << /F1 << /Type /Font /Subtype /Type1 /BaseFont /Helvetica >> >> >>"))
        } else if i == 1 {
            (stray, "3 0 R".to_owned())
        } else {
            ("", "3 0 R".to_owned())
        };
        objects.push(
            format!("<< /Type /Page /Parent 2 0 R /MediaBox [0 0 400 400] /Resources {resources} /Contents {} 0 R {junk} /Rotate 0 >>", 5 + 2 * i)
                .into_bytes(),
        );
        let body = format!("BT /F1 36 Tf 50 200 Td (PAGE{}) Tj ET", i + 1);
        let mut s = format!("<< /Length {} >>\nstream\n", body.len()).into_bytes();
        s.extend_from_slice(body.as_bytes());
        s.extend_from_slice(b"\nendstream");
        objects.push(s);
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

fn request(page: usize, kind: RequestKind) -> RenderRequest {
    RenderRequest { page, kind, tile: None, scale: 1.0, tag: 0, preview: false }
}

fn text(renderer: &mut PageRenderer, page: usize) -> Arc<PageText> {
    let out = renderer.render(request(page, RequestKind::Text));
    assert!(out.error.is_none(), "page {} text error: {:?}", page + 1, out.error);
    out.text.expect("page text")
}

#[test]
fn every_page_after_a_stray_delimiter_is_in_the_page_tree() {
    for stray in STRAY {
        let renderer = PageRenderer::new(pdf(stray), RenderConfig::default());
        assert_eq!(renderer.page_count(), PAGES, "{stray:?}");
    }
}

#[test]
fn the_page_holding_the_stray_delimiter_and_the_pages_after_it_draw_their_text() {
    for stray in STRAY {
        let mut renderer = PageRenderer::new(pdf(stray), RenderConfig::default());
        for page in 0..PAGES {
            let want = format!("PAGE{}", page + 1);
            let got = text(&mut renderer, page);
            assert_eq!(got.find(&want).len(), 1, "{stray:?}: {want} in {:?}", got.plain_text());
        }
    }
}

#[test]
fn the_page_holding_the_stray_delimiter_draws_like_a_clean_one() {
    let mut clean = PageRenderer::new(pdf(""), RenderConfig::default());
    let want = clean.render(request(1, RequestKind::Pixels));
    assert!(want.error.is_none());
    assert!(want.rgba.iter().any(|&b| b != 255), "the clean page draws its text");
    for stray in STRAY {
        let mut renderer = PageRenderer::new(pdf(stray), RenderConfig::default());
        let got = renderer.render(request(1, RequestKind::Pixels));
        assert!(got.error.is_none(), "{stray:?}: {:?}", got.error);
        assert_eq!((got.width, got.height), (want.width, want.height), "{stray:?}");
        assert!(got.rgba == want.rgba, "{stray:?}: the page draws differently from the clean one");
    }
}

#[test]
fn a_stray_delimiter_in_a_nested_resources_dictionary_keeps_the_page_and_its_font() {
    for stray in STRAY {
        let mut renderer = PageRenderer::new(pdf_with("", stray), RenderConfig::default());
        assert_eq!(renderer.page_count(), PAGES, "{stray:?}");
        for page in 0..PAGES {
            let want = format!("PAGE{}", page + 1);
            let got = text(&mut renderer, page);
            assert_eq!(got.find(&want).len(), 1, "{stray:?}: {want} in {:?}", got.plain_text());
        }
    }
}

/// Stray bytes with no `>>` after them still leave the dictionary unreadable: skipping them
/// stops at the end of the data, and the walk stays bounded.
#[test]
fn an_unterminated_page_dictionary_full_of_stray_bytes_is_bounded() {
    let mut bytes = (*pdf("")).clone();
    let junk = ") ] } { > ".repeat(10_000);
    let at = bytes.windows(9).rposition(|w| w == b"/Rotate 0").expect("a page dictionary");
    // Cut the file just after the last page's `/Rotate 0`, then pile up stray bytes: no
    // `>>`, no `endobj`, no xref table. Whatever the reader recovers, it must not panic.
    bytes.truncate(at + 9);
    bytes.extend_from_slice(junk.as_bytes());
    let renderer = PageRenderer::new(Arc::new(bytes), RenderConfig::default());
    assert!(renderer.page_count() <= PAGES);
}
