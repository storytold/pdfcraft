//! Contributor-original synthetic files: a page tree that leads to no page, while the file holds
//! a page that names that tree as its `/Parent` (vendored hayro-syntax patch 59).
//!
//! ISO 32000-2 §7.7.3.2 has every page reached from the root of the page tree. When the walk from
//! the root reaches none (the root's only kid is the root itself, as in PDFBOX-4623
//! `loop_in_page_tree.pdf`), the page dictionaries found by scanning the file are the pages,
//! with what they inherit through `/Parent` (§7.7.3.4). Only dictionaries that say they are pages
//! count, and not when the walk stopped at a kid it could not read: that tree names pages the walk
//! did not reach.
use pdfcraft_render::{PageRenderer, PageText, RenderConfig, RenderRequest, RequestKind, inspect};
use std::sync::Arc;

/// A PDF of `objs`, numbered from 1, with a cross-reference table and object 1 as the catalog.
fn pdf_of(objs: &[String]) -> Arc<Vec<u8>> {
    let mut out = b"%PDF-1.7\n".to_vec();
    let mut offsets = Vec::new();
    for (i, o) in objs.iter().enumerate() {
        offsets.push(out.len());
        out.extend_from_slice(format!("{} 0 obj\n{o}\nendobj\n", i + 1).as_bytes());
    }
    let xref = out.len();
    out.extend_from_slice(format!("xref\n0 {}\n0000000000 65535 f \n", objs.len() + 1).as_bytes());
    for o in offsets {
        out.extend_from_slice(format!("{o:010} 00000 n \n").as_bytes());
    }
    out.extend_from_slice(format!("trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n", objs.len() + 1).as_bytes());
    Arc::new(out)
}

const CONTENT: &str = "BT /F1 24 Tf 20 100 Td (Hello) Tj ET";

/// Objects 1-6: the catalog, a root whose `/Kids` are `kids` and which carries the media box and
/// the font, a page under it, the font, the page's content, and a note (an annotation with a
/// `/Contents` text string, which is no page).
fn file(kids: &str) -> Arc<Vec<u8>> {
    pdf_of(&[
        "<< /Type /Catalog /Pages 2 0 R >>".into(),
        format!("<< /Type /Pages /Kids [{kids}] /Count 1 /MediaBox [0 0 300 200] /Resources << /Font << /F1 4 0 R >> >> >>"),
        "<< /Type /Page /Parent 2 0 R /Contents 5 0 R >>".into(),
        "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica /Encoding /WinAnsiEncoding >>".into(),
        format!("<< /Length {} >>\nstream\n{CONTENT}\nendstream", CONTENT.len()),
        "<< /Type /Annot /Subtype /Text /Rect [0 0 20 20] /Contents (a note) >>".into(),
    ])
}

fn text_of_page_one(bytes: Arc<Vec<u8>>) -> String {
    let mut r = PageRenderer::new(bytes, RenderConfig::default());
    let page = r.render(RenderRequest { page: 0, kind: RequestKind::Text, tile: None, scale: 1.0, tag: 0, preview: false });
    assert!(page.error.is_none(), "page 1 is read: {:?}", page.error);
    page.text.map(|t: Arc<PageText>| t.plain_text()).unwrap_or_default()
}

/// The root's only kid is the root: the walk ends at once, having reached no page. The page that
/// names the root as its parent is found, with the root's media box and font, and the note is not
/// taken for a second page although it has `/Contents`.
#[cfg_attr(target_arch = "wasm32", wasm_bindgen_test::wasm_bindgen_test)]
#[cfg_attr(not(target_arch = "wasm32"), test)]
fn a_page_tree_whose_root_is_its_own_kid_gives_way_to_the_page_found_by_scanning() {
    let bytes = file("2 0 R");
    let info = inspect(bytes.clone(), None).expect("the file opens");
    assert_eq!(info.pages.len(), 1, "one page: the note is not a page");
    let page = &info.pages[0];
    assert_eq!((page.width, page.height), (300.0, 200.0), "the media box comes from the page's /Parent");
    assert!(text_of_page_one(bytes).contains("Hello"), "the page draws with the font its /Parent holds");
}

/// An empty `/Kids`: the same.
#[cfg_attr(target_arch = "wasm32", wasm_bindgen_test::wasm_bindgen_test)]
#[cfg_attr(not(target_arch = "wasm32"), test)]
fn an_empty_page_tree_gives_way_to_the_page_found_by_scanning() {
    let info = inspect(file(""), None).expect("the file opens");
    assert_eq!(info.pages.iter().map(|p| (p.width, p.height)).collect::<Vec<_>>(), [(300.0, 200.0)]);
}

/// A tree whose walk stops at a kid that is not there (object 9) has not shown that it leads to no
/// page, so the scan does not stand in for it: the orphan page 7 (a size no tree page has) is never
/// one of the pages. Page 3, listed after the missing kid, may or may not be reached by the walk
/// (#2660), but nothing else is shown.
#[cfg_attr(target_arch = "wasm32", wasm_bindgen_test::wasm_bindgen_test)]
#[cfg_attr(not(target_arch = "wasm32"), test)]
fn a_walk_stopped_by_a_missing_kid_is_not_replaced_by_the_scan() {
    let bytes = pdf_of(&[
        "<< /Type /Catalog /Pages 2 0 R >>".into(),
        "<< /Type /Pages /Kids [9 0 R 3 0 R] /Count 2 >>".into(),
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 300 200] /Contents 5 0 R >>".into(),
        "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>".into(),
        format!("<< /Length {} >>\nstream\n{CONTENT}\nendstream", CONTENT.len()),
        "<< /Length 0 >>\nstream\n\nendstream".into(),
        "<< /Type /Page /MediaBox [0 0 500 500] /Contents 6 0 R >>".into(),
    ]);
    let sizes: Vec<(f32, f32)> = inspect(bytes, None).map(|i| i.pages.iter().map(|p| (p.width, p.height)).collect()).unwrap_or_default();
    assert!(sizes.iter().all(|s| *s == (300.0, 200.0)), "only the tree's page, if any: {sizes:?}");
}
