use super::*;

/// The decoded content of each sheet of a summary, written and read back.
fn sheets(doc: &Document) -> Vec<String> {
    let bytes = write_full(doc, &SaveOptions::default()).unwrap();
    let doc = Document::open(Arc::new(bytes)).unwrap();
    pdfcraft_model::pages(&doc)
        .iter()
        .map(|p| String::from_utf8_lossy(&decoded_contents(&doc, &p.dict, &mut DecodeBudget::page()).unwrap()).into_owned())
        .collect()
}

fn comment(page: usize, number: usize, lines: &[&str]) -> SummaryComment {
    SummaryComment {
        page,
        number,
        index: None,
        rect: [10.0, 10.0, 50.0, 50.0],
        color: Some([1.0, 0.0, 0.0]),
        lines: lines.iter().enumerate().map(|(i, l)| (usize::from(i > 1), l.to_string())).collect(),
    }
}

#[test]
fn a_summary_shows_each_commented_page_beside_its_comments() {
    let src = fixture(3);
    let comments = [comment(0, 1, &["1. Rectangle  Ada", "Check this", "Reply  Bo", "Done"]), comment(2, 1, &["1. Sticky Note  Ada"])];
    let doc = summary_with_pages(&src, "Summary of Comments on f.pdf", &comments, Connect::Lines).unwrap();
    let shown = sheets(&doc);
    assert_eq!(shown.len(), 2, "one sheet per page with comments; page 2 has none");
    let first = &shown[0];
    assert!(first.contains("/P0 Do"), "the page is drawn: {first}");
    assert!(first.contains("(Page 1)") && first.contains("(Check this)") && first.contains("(Done)"), "{first}");
    assert_eq!(first.matches(" l S").count(), 2, "the header rule and one connector line: {first}");
    assert!(shown[1].contains("(Page 3)"));
    // The page itself, as a form with its printing comments drawn.
    let pages = pdfcraft_model::pages(&doc);
    assert_eq!(pages[0].display_size(&doc), (792.0, 612.0));
    let res = doc.resolve(pages[0].dict.get(b"Resources").unwrap()).as_dict().cloned().unwrap();
    let xo = doc.resolve(res.get(b"XObject").unwrap()).as_dict().cloned().unwrap();
    assert!(xo.get(b"P0").is_some());
    assert_eq!(doc.trailer().get(b"Encrypt"), None);

    // Numbers instead of lines: the number is drawn on the page, no line.
    let doc = summary_with_pages(&src, "S", &comments, Connect::Numbers).unwrap();
    let first = &sheets(&doc)[0];
    assert_eq!(first.matches(" l S").count(), 1, "only the header rule is a line: {first}");
    assert!(first.contains("(1) Tj"), "{first}");
}

#[test]
fn comments_that_do_not_fit_continue_beside_the_same_page() {
    let src = fixture(1);
    let long = "word ".repeat(400);
    let comments: Vec<SummaryComment> = (1..=6).map(|n| comment(0, n, &["heading", long.as_str()])).collect();
    let doc = summary_with_pages(&src, "S", &comments, Connect::Lines).unwrap();
    let shown = sheets(&doc);
    assert!(shown.len() > 1, "the comments run onto more sheets");
    assert!(shown.iter().all(|s| s.contains("/P0 Do")), "each shows the page");
    assert!(shown[1].contains("(Page 1 \\(continued\\))"), "{}", shown[1]);
    // One comment taller than a sheet is split across sheets rather than lost.
    let huge = "word ".repeat(20_000);
    let doc = summary_with_pages(&src, "S", &[comment(0, 1, &["heading", huge.as_str()])], Connect::Lines).unwrap();
    let shown = sheets(&doc);
    assert!(shown.len() > 2);
    assert_eq!(shown.iter().map(|s| s.matches("(word) Tj").count() + s.matches("(word word").count()).filter(|&c| c > 0).count(), shown.len());
}

#[test]
fn no_comments_make_one_sheet_saying_so() {
    let src = fixture(2);
    // A comment on a page the document doesn't have is left out.
    let doc = summary_with_pages(&src, "S", &[comment(9, 1, &["lost"])], Connect::Lines).unwrap();
    let shown = sheets(&doc);
    assert_eq!(shown.len(), 1);
    assert!(shown[0].contains("(This document has no comments.)") && !shown[0].contains("lost"), "{}", shown[0]);
}

#[test]
fn the_mark_follows_the_comment_on_a_turned_page() {
    let mut src = fixture(1);
    let page = pdfcraft_model::pages(&src)[0].obj;
    src.update_dict(page, |d| d.set(b"Rotate".to_vec(), Object::Int(90))).unwrap();
    // A comment at the page's bottom-left corner in user space: turned clockwise, that corner is
    // at the top-left of the page as shown.
    let mut c = comment(0, 7, &["h"]);
    c.rect = [0.0, 0.0, 0.0, 0.0];
    let doc = summary_with_pages(&src, "S", &[c], Connect::Numbers).unwrap();
    let first = &sheets(&doc)[0];
    // The page (300 × 200 as shown) fills the left area's width: 348 pt wide, 232 tall, centred
    // in 516 pt from y = 36, so its top is at 36 + (516 + 232) / 2 = 410.
    let (x, y) = (36.0, 410.0);
    assert!(first.contains(&format!("{} {} m", x + 6.5, y)), "the number's circle is at the shown top-left corner: {first}");
}

#[test]
fn characters_helvetica_lacks_are_kept() {
    let src = fixture(1);
    // Cyrillic: the bundled face draws it.
    let doc = summary_with_pages(&src, "S", &[comment(0, 1, &["h", "Проверьте"])], Connect::Lines).unwrap();
    assert!(!sheets(&doc)[0].contains("[U+"));
    // Line breaks, tabs and soft hyphens lay the text out or are drawn as hyphens: they don't
    // send it to Helvetica with every letter written as a code point.
    let doc = summary_with_pages(&src, "S", &[comment(0, 1, &["h", "Проверьте\r\nэто\tco\u{ad}op"])], Connect::Lines).unwrap();
    assert!(!sheets(&doc)[0].contains("[U+"), "{}", sheets(&doc)[0]);
    // Japanese: no bundled face here, so written as code points, never dropped.
    let doc = summary_with_pages(&src, "S", &[comment(0, 1, &["h", "確認"])], Connect::Lines).unwrap();
    assert!(sheets(&doc)[0].contains("[U+78BA][U+8A8D]"));
}

#[test]
fn a_long_title_stops_before_the_page_label() {
    let src = fixture(1);
    let title = format!("Summary of Comments on {}.pdf", "x".repeat(300));
    let doc = summary_with_pages(&src, &title, &[comment(0, 1, &["h"])], Connect::Lines).unwrap();
    let first = &sheets(&doc)[0];
    assert!(first.contains("(Page 1)") && !first.contains(&"x".repeat(150)), "{first}");
}

#[test]
fn nothing_points_at_a_comment_the_page_does_not_show() {
    // Page 1's second annotation is a note without the Print flag: drawn nowhere on the page.
    let src = fixture(1);
    let mut hidden = comment(0, 1, &["h"]);
    hidden.index = Some(1);
    let first = sheets(&summary_with_pages(&src, "S", &[hidden.clone()], Connect::Lines).unwrap()).remove(0);
    assert_eq!(first.matches(" l S").count(), 1, "only the header rule: {first}");
    hidden.index = Some(0);
    let first = sheets(&summary_with_pages(&src, "S", &[hidden], Connect::Lines).unwrap()).remove(0);
    assert_eq!(first.matches(" l S").count(), 2, "the printing square is pointed at: {first}");
}
