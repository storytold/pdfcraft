use std::collections::BTreeMap;
use std::sync::Arc;

use super::*;
use crate::{DocId, Session};

/// A one-page document with `content` and two fonts: `F1` Helvetica, `F2` Helvetica-Bold, and
/// `F3` a subset of a face no computer has, with glyphs for `a` and `b` only.
fn pdf(content: &str) -> Vec<u8> {
    let objs: Vec<String> = vec![
        "<< /Type /Catalog /Pages 2 0 R >>".into(),
        "<< /Type /Pages /Kids [3 0 R] /Count 1 >>".into(),
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 595 842] /Contents 4 0 R /Resources << /Font << /F1 5 0 R /F2 6 0 R /F3 7 0 R >> >> >>".into(),
        format!("<< /Length {} >>\nstream\n{content}\nendstream", content.len()),
        "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica /Encoding /WinAnsiEncoding >>".into(),
        "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica-Bold /Encoding /WinAnsiEncoding >>".into(),
        "<< /Type /Font /Subtype /TrueType /BaseFont /ABCDEF+NoSuchFace /FirstChar 97 /LastChar 99 /Widths [500 520 0] /Encoding /WinAnsiEncoding >>"
            .into(),
    ];
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
    out
}

fn open(bytes: Vec<u8>) -> (Session, DocId) {
    let mut s = Session::new().with_clock(|| 1_700_000_000);
    let id = s.open("replace.pdf", None, Arc::new(bytes), None).expect("opens");
    (s, id)
}

fn query(find: &str, with: &str) -> Replace {
    Replace { find: find.into(), with: with.into(), ..Default::default() }
}

/// Plan and apply; the plan's counts.
fn replace_all(s: &mut Session, id: DocId, q: &Replace) -> Replacement {
    let pages: Vec<usize> = (0..s.get(id).unwrap().info.pages.len()).collect();
    let plan = s.get(id).unwrap().plan_replace(&pages, q, Which::All, None).unwrap();
    if let Some(edit) = plan.edit.clone() {
        s.apply(id, edit).unwrap();
    }
    plan
}

fn texts(s: &Session, id: DocId, page: usize) -> Vec<String> {
    s.get(id).unwrap().text_blocks(page).into_iter().map(|b| b.text).collect()
}

/// The page text as the find bar reads it (the text layer's glyphs).
fn page_text(s: &Session, id: DocId, page: usize) -> pdfcraft_render::PageText {
    let doc = s.get(id).unwrap();
    let mut r = pdfcraft_render::PageRenderer::new(doc.bytes.clone(), pdfcraft_render::RenderConfig::default());
    let out = r.render(pdfcraft_render::RenderRequest { page, kind: pdfcraft_render::RequestKind::Text, scale: 1.0, ..Default::default() });
    out.text.map(|t| t.as_ref().clone()).unwrap_or_default()
}

#[test]
fn a_replaced_word_keeps_its_paragraphs_font_and_size() {
    let (mut s, id) = open(pdf("BT /F1 14 Tf 72 700 Td (The quick brown fox) Tj ET"));
    let plan = replace_all(&mut s, id, &query("quick", "slow"));
    assert_eq!((plan.replaced, plan.pages.clone(), plan.skipped_total()), (1, vec![0], 0));
    assert_eq!(texts(&s, id, 0), ["The slow brown fox"]);
    let lines = s.get(id).unwrap().text_lines(0);
    assert_eq!(lines.len(), 1);
    assert_eq!((lines[0].font.as_str(), lines[0].base_font.as_str(), lines[0].size), ("F1", "Helvetica", 14.0));
    assert_eq!(s.get(id).unwrap().can_undo(), Some("Replace all"));
}

/// A red word and a bold name inside a paragraph keep their own style when words around them
/// are replaced; a replacement inside the red word stays red.
#[test]
fn a_mixed_style_paragraph_keeps_the_other_runs_styles() {
    let content = "BT /F1 12 Tf 72 700 Td 0 g (Please pay the invoice ) Tj 1 0 0 rg (now) Tj 0 g (, before the end of the month,) Tj ET
BT /F1 12 Tf 72 686 Td 0 g (as agreed with ) Tj /F2 12 Tf (Ada Lovelace) Tj /F1 12 Tf (.) Tj ET";
    let (mut s, id) = open(pdf(content));
    assert_eq!(texts(&s, id, 0).len(), 1, "one paragraph of five runs");
    let q = Replace { whole_words: true, ..query("the", "THE") };
    let plan = replace_all(&mut s, id, &q);
    assert_eq!(plan.replaced, 3);
    let plan = replace_all(&mut s, id, &query("now", "today"));
    assert_eq!(plan.replaced, 1);
    assert_eq!(texts(&s, id, 0), ["Please pay THE invoice today, before THE end of THE month, as agreed with Ada Lovelace."]);
    let lines = s.get(id).unwrap().text_lines(0);
    let run = |word: &str| lines.iter().find(|l| l.text.contains(word)).unwrap_or_else(|| panic!("no run with {word:?}: {lines:?}"));
    assert_eq!(run("today").color, [1.0, 0.0, 0.0], "the replacement takes the red run it lands in");
    assert_eq!(run("today").text.trim(), "today", "and only it is red");
    assert_eq!(run("Ada").base_font, "Helvetica-Bold");
    assert_eq!(run("Ada").text.trim(), "Ada Lovelace");
    assert_eq!(run("invoice").color, [0.0, 0.0, 0.0]);
    assert_eq!(run("invoice").base_font, "Helvetica");
}

#[test]
fn replace_all_across_pages_is_one_undo_step() {
    let (mut s, id) = open(crate::tests::fixture(3));
    let before = s.get(id).unwrap().bytes.clone();
    let plan = replace_all(&mut s, id, &query("page", "Sheet"));
    assert_eq!((plan.replaced, plan.pages.clone()), (3, vec![0, 1, 2]));
    assert!(matches!(&plan.edit, Some(Edit::Batch { edits, .. }) if edits.len() == 3));
    for p in 0..3 {
        assert_eq!(texts(&s, id, p), [format!("Sheet {}", p + 1)]);
    }
    s.undo(id).unwrap();
    for p in 0..3 {
        assert_eq!(texts(&s, id, p), [format!("Page {}", p + 1)]);
    }
    assert_eq!(s.get(id).unwrap().can_undo(), None, "one step undid all three pages");
    assert_eq!(s.get(id).unwrap().bytes, before, "the original bytes are back");
}

#[test]
fn whole_words_and_case_follow_the_find_options() {
    let content = "BT /F1 12 Tf 72 700 Td (cat Cats concat CAT) Tj ET";
    let count = |q: Replace| {
        let (s, id) = open(pdf(content));
        s.get(id).unwrap().plan_replace(&[0], &q, Which::All, None).unwrap().replaced
    };
    assert_eq!(count(query("cat", "dog")), 4);
    assert_eq!(count(Replace { whole_words: true, ..query("cat", "dog") }), 2);
    assert_eq!(count(Replace { case_sensitive: true, ..query("cat", "dog") }), 2);
    assert_eq!(count(Replace { case_sensitive: true, whole_words: true, ..query("CAT", "dog") }), 1);
    let (mut s, id) = open(pdf(content));
    replace_all(&mut s, id, &Replace { whole_words: true, ..query("cat", "dog") });
    assert_eq!(texts(&s, id, 0), ["dog Cats concat dog"]);
    // An empty replacement deletes the match.
    replace_all(&mut s, id, &query(" concat", ""));
    assert_eq!(texts(&s, id, 0), ["dog Cats dog"]);
}

#[test]
fn a_match_spanning_two_paragraphs_is_skipped_and_counted() {
    let content = "BT /F1 12 Tf 72 700 Td (The first ends here) Tj ET BT /F1 12 Tf 72 500 Td (second starts there) Tj ET";
    let (mut s, id) = open(pdf(content));
    assert_eq!(texts(&s, id, 0).len(), 2);
    let plan = replace_all(&mut s, id, &query("here second", "x"));
    assert_eq!(plan.replaced, 0);
    assert_eq!(plan.skipped, BTreeMap::from([(Skip::SpansParagraphs, 1)]));
    assert!(plan.edit.is_none());
    assert_eq!(texts(&s, id, 0), ["The first ends here", "second starts there"]);
}

/// The subset in `F3` has glyphs for `a` and `b` only, from a face no computer has: a `c` would
/// be drawn in Helvetica instead. That paragraph is left alone and counted, the other replaced.
#[test]
fn a_character_the_font_cannot_draw_is_skipped_and_counted() {
    let content = "BT /F3 12 Tf 72 700 Td (ab ba) Tj ET BT /F1 12 Tf 72 500 Td (ab in Helvetica) Tj ET";
    let (mut s, id) = open(pdf(content));
    let before = texts(&s, id, 0);
    assert_eq!(before, ["ab ba", "ab in Helvetica"]);
    let plan = replace_all(&mut s, id, &query("ab", "cb"));
    assert_eq!(plan.replaced, 1);
    assert_eq!(plan.skipped, BTreeMap::from([(Skip::MissingGlyphs, 1)]));
    assert_eq!(texts(&s, id, 0), ["ab ba", "cb in Helvetica"]);
    let lines = s.get(id).unwrap().text_lines(0);
    assert!(lines.iter().filter(|l| l.text.contains("ab ba")).all(|l| l.font == "F3"), "the subset paragraph keeps its font: {lines:?}");
    // What the subset can draw still goes in.
    let plan = replace_all(&mut s, id, &query("ab ba", "ba ab"));
    assert_eq!((plan.replaced, plan.skipped_total()), (1, 0));
    assert_eq!(texts(&s, id, 0)[0], "ba ab");
}

#[test]
fn invisible_text_over_a_scan_is_skipped() {
    let (mut s, id) = open(pdf("BT 3 Tr /F1 12 Tf 72 700 Td (recognized words) Tj ET"));
    let plan = replace_all(&mut s, id, &query("words", "text"));
    assert_eq!(plan.skipped, BTreeMap::from([(Skip::ScannedText, 1)]));
    assert!(plan.edit.is_none());
}

/// What the find bar counts on a page beyond what the paragraphs hold is reported as not
/// editable text (a form field, a comment), so the counts add up to what was highlighted.
#[test]
fn matches_the_paragraphs_dont_hold_are_counted_as_not_editable() {
    let (s, id) = open(pdf("BT /F1 12 Tf 72 700 Td (one cat) Tj ET"));
    let found = BTreeMap::from([(0, 3)]);
    let plan = s.get(id).unwrap().plan_replace(&[0], &query("cat", "dog"), Which::All, Some(&found)).unwrap();
    assert_eq!(plan.replaced, 1);
    assert_eq!(plan.skipped, BTreeMap::from([(Skip::NotEditable, 2)]));
}

/// Replace (one match): the highlighted one, found by where the find bar's glyphs are, even when
/// the paragraph holds the word twice.
#[test]
fn the_highlighted_match_is_the_one_replaced() {
    let (mut s, id) = open(pdf("BT /F1 12 Tf 72 700 Td (cat and cat) Tj ET BT /F1 12 Tf 72 500 Td (no match) Tj ET"));
    let text = page_text(&s, id, 0);
    let found = text.find("cat");
    assert_eq!(found.len(), 2);
    let centre = |g: usize| {
        let r = text.glyphs[g].rect;
        [(r[0] + r[2]) / 2.0, (r[1] + r[3]) / 2.0]
    };
    let second = &found[1];
    let which = Which::At { page: 0, start: centre(second.start), end: centre(second.end - 1) };
    let plan = s.get(id).unwrap().plan_replace(&[0], &query("cat", "dog"), which, None).unwrap();
    assert_eq!(plan.replaced, 1);
    assert!(matches!(&plan.edit, Some(Edit::EditTextBlock { block: 0, .. })), "one paragraph edit, not a batch: {:?}", plan.edit);
    s.apply(id, plan.edit.unwrap()).unwrap();
    assert_eq!(texts(&s, id, 0), ["cat and dog", "no match"]);
    // A point in no paragraph is not editable text.
    let away = Which::At { page: 0, start: [500.0, 20.0], end: [510.0, 20.0] };
    let plan = s.get(id).unwrap().plan_replace(&[0], &query("cat", "dog"), away, None).unwrap();
    assert_eq!(plan.skipped, BTreeMap::from([(Skip::NotEditable, 1)]));
}

#[test]
fn first_replaces_only_the_first_match_in_document_order() {
    let (mut s, id) = open(crate::tests::fixture(2));
    let plan = s.get(id).unwrap().plan_replace(&[0, 1], &query("page", "Sheet"), Which::First, None).unwrap();
    assert_eq!((plan.replaced, plan.pages.clone()), (1, vec![0]));
    s.apply(id, plan.edit.unwrap()).unwrap();
    assert_eq!((texts(&s, id, 0), texts(&s, id, 1)), (vec!["Sheet 1".to_string()], vec!["Page 2".to_string()]));
}

#[test]
fn nothing_to_find_is_refused() {
    let (s, id) = open(crate::tests::fixture(1));
    assert!(matches!(s.get(id).unwrap().plan_replace(&[0], &query("  ", "x"), Which::All, None), Err(EditError::Invalid(_))));
}
