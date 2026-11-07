//! Every link in the navigation document and `toc.ncx` must land on an id that exists in the
//! chapter it names, wherever on a page a chapter starts.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::collections::HashMap;

use super::tests::{block, entries, meta, page, parse};
use super::*;

fn files_of(book: &[u8]) -> HashMap<String, String> {
    entries(book).into_iter().map(|(n, _, d)| (n, String::from_utf8_lossy(&d).into_owned())).collect()
}

/// Every `href` of the navigation document and `src` of the NCX, as (file, fragment).
fn links(files: &HashMap<String, String>) -> Vec<(String, String)> {
    let mut out = Vec::new();
    let nav = files["OEBPS/nav.xhtml"].as_str();
    let n = parse(nav).unwrap();
    for e in n.descendants().filter(|e| e.has_tag_name("a")) {
        out.push(e.attribute("href").unwrap().to_string());
    }
    let ncx = files["OEBPS/toc.ncx"].as_str();
    let c = parse(ncx).unwrap();
    for e in c.descendants().filter(|e| e.has_tag_name("content")) {
        out.push(e.attribute("src").unwrap().to_string());
    }
    out.into_iter().map(|l| l.split_once('#').map_or((l.clone(), String::new()), |(f, a)| (f.to_string(), a.to_string()))).collect()
}

fn assert_links_resolve(book: &[u8]) {
    let files = files_of(book);
    for (file, fragment) in links(&files) {
        let body = files.get(&format!("OEBPS/{file}")).unwrap_or_else(|| panic!("{file} is in the archive"));
        assert!(body.contains(&format!("id=\"{fragment}\"")), "{file}#{fragment} names an id that exists");
    }
}

#[test]
fn a_heading_after_body_text_on_its_page_still_has_its_links_resolve() {
    let p1 = page(vec![block("Opening", 24.0, 700.0), block("First page body.", 11.0, 650.0)]);
    let p2 = page(vec![block("Body before the heading.", 11.0, 700.0), block("Second chapter", 24.0, 650.0), block("Its body.", 11.0, 600.0)]);
    let book = epub(&[p1, p2], &meta(), &[], &Tags::default());
    assert_links_resolve(&book);
}

#[test]
fn two_headings_on_one_page_both_resolve() {
    let p1 = page(vec![
        block("Opening", 24.0, 700.0),
        block("Body.", 11.0, 650.0),
        block("Middle", 24.0, 600.0),
        block("More body.", 11.0, 550.0),
        block("Ending", 24.0, 500.0),
        block("Last body.", 11.0, 450.0),
    ]);
    let book = epub(&[p1], &meta(), &[], &Tags::default());
    assert_links_resolve(&book);
    let files = files_of(&book);
    let nav = &files["OEBPS/nav.xhtml"];
    assert!(nav.contains("chapter-3.xhtml#h-3\">Ending</a>"), "{nav}");
}

#[test]
fn bookmarks_to_a_page_that_starts_a_chapter_mid_page_resolve() {
    let p1 = page(vec![block("Opening", 24.0, 700.0), block("Body.", 11.0, 650.0)]);
    let p2 = page(vec![block("Text first.", 11.0, 700.0), block("Next", 24.0, 650.0)]);
    let toc = vec![TocEntry { title: "Page two".into(), level: 1, page: 1 }];
    let book = epub(&[p1, p2], &meta(), &toc, &Tags::default());
    assert_links_resolve(&book);
}
