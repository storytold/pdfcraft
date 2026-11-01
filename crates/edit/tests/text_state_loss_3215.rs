//! Synthetic reproductions of #3215 and #3825; only the attributed OFL Inter atomicity test embeds a font.
use std::sync::Arc;

use pdfcraft_cos::{Document, SaveOptions, write_full};
use pdfcraft_edit::text::{replace_block, replace_line, text_blocks, text_lines};

/// Build a single-page PDF from synthetic operators, with standard-font metrics and no embedded font.
fn fixture(content: &str) -> Document {
    let objects = [
        "<< /Type /Catalog /Pages 2 0 R >>".to_string(),
        "<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_string(),
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Contents 4 0 R /Resources << /Font << /F1 5 0 R >> >> >>".to_string(),
        format!("<< /Length {} >>\nstream\n{content}\nendstream", content.len()),
        "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica /Encoding /WinAnsiEncoding >>".to_string(),
    ];
    let mut bytes = b"%PDF-1.7\n".to_vec();
    let mut offsets = Vec::new();
    for (i, object) in objects.iter().enumerate() {
        offsets.push(bytes.len());
        bytes.extend_from_slice(format!("{} 0 obj\n{object}\nendobj\n", i + 1).as_bytes());
    }
    let xref = bytes.len();
    bytes.extend_from_slice(format!("xref\n0 {}\n0000000000 65535 f \n", objects.len() + 1).as_bytes());
    for offset in offsets {
        bytes.extend_from_slice(format!("{offset:010} 00000 n \n").as_bytes());
    }
    bytes.extend_from_slice(format!("trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n", objects.len() + 1).as_bytes());
    Document::open(Arc::new(bytes)).unwrap()
}

/// Measure the final word's horizontal ink extent, independent of source-run line grouping.
fn untouched_word_width(doc: &Document) -> f64 {
    let lines = text_lines(doc, 0).unwrap();
    let ink = lines.iter().flat_map(|line| &line.ink).last().unwrap();
    ink[2] - ink[0]
}

/// Edit only the earlier word through the selected API, then verify the later word's width after full save/reopen.
fn check_untouched_word(content: &str, old: &str, new: &str, paragraph: bool) {
    let mut doc = fixture(content);
    let before = untouched_word_width(&doc);
    let lines = text_lines(&doc, 0).unwrap();
    let blocks = text_blocks(&doc, 0).unwrap();
    eprintln!(
        "before: lines={:?}, blocks={:?}, untouched width={before:.6}",
        lines.iter().map(|l| (&l.text, &l.ink)).collect::<Vec<_>>(),
        blocks.iter().map(|b| &b.editor_text).collect::<Vec<_>>()
    );
    if paragraph {
        replace_block(&mut doc, 0, 0, &blocks[0].editor_text.replacen(old, new, 1)).unwrap();
    } else {
        replace_line(&mut doc, 0, 0, &lines[0].text.replacen(old, new, 1)).unwrap();
    }
    let saved = write_full(&doc, &SaveOptions::default()).unwrap();
    hayro_syntax::Pdf::new(saved.clone()).expect("saved PDF parses independently");
    let reopened = Document::open(Arc::new(saved)).unwrap();
    let after = untouched_word_width(&reopened);
    let after_lines = text_lines(&reopened, 0).unwrap();
    let edited_text = after_lines.iter().map(|line| line.text.as_str()).collect::<String>();
    assert!(edited_text.contains(new) && !edited_text.contains(old), "edit was saved: {edited_text}");
    eprintln!(
        "after: lines={:?}, untouched width={after:.6}, delta={:.6}",
        after_lines.iter().map(|l| (&l.text, &l.ink)).collect::<Vec<_>>(),
        after - before
    );
    assert!(
        (after - before).abs() < 0.001,
        "editing only {old:?} to {new:?} changed untouched word width from {before:.6} to {after:.6} (paragraph={paragraph})"
    );
}

#[test]
fn line_edit_keeps_later_horizontal_scale_3215() {
    check_untouched_word("BT /F1 24 Tf 72 700 Td (Plain) Tj 60 Tz ( condensed) Tj ET", "Plain", "Plxin", false);
}

#[test]
fn paragraph_edit_keeps_later_horizontal_scale_3215() {
    check_untouched_word("BT /F1 24 Tf 72 700 Td (Plain) Tj 60 Tz ( condensed) Tj ET", "Plain", "Plxin", true);
}

#[test]
fn line_edit_keeps_later_character_spacing_3215() {
    check_untouched_word("BT /F1 24 Tf 72 700 Td (Tight) Tj 3 Tc ( spaced) Tj ET", "Tight", "Tigxt", false);
}

#[test]
fn paragraph_edit_keeps_later_character_spacing_3215() {
    check_untouched_word("BT /F1 24 Tf 72 700 Td (Tight) Tj 3 Tc ( spaced) Tj ET", "Tight", "Tigxt", true);
}

#[test]
fn unchanged_character_spacing_and_scale_keep_one_line_3215() {
    for state in ["", "3 Tc", "60 Tz", "3 Tc 60 Tz"] {
        let mut doc = fixture(&format!("BT /F1 24 Tf {state} 72 700 Td (Plain) Tj {state} ( condensed) Tj ET"));
        assert_eq!(text_lines(&doc, 0).unwrap().len(), 1, "same state: {state}");
        let blocks = text_blocks(&doc, 0).unwrap();
        assert_eq!(blocks.len(), 1);
        assert_eq!(blocks[0].editor_text, "Plain condensed");
        let before = untouched_word_width(&doc);
        replace_line(&mut doc, 0, 0, "Plxin condensed").unwrap();
        let reopened = Document::open(Arc::new(write_full(&doc, &SaveOptions::default()).unwrap())).unwrap();
        assert_eq!(text_lines(&reopened, 0).unwrap().len(), 1, "saved same state: {state}");
        assert!((untouched_word_width(&reopened) - before).abs() < 0.001, "same state: {state}");
    }
}

#[test]
fn restored_character_spacing_and_scale_keep_each_run_3215() {
    for (changed, restored) in [("60 Tz", "100 Tz"), ("3 Tc", "0 Tc")] {
        for paragraph in [false, true] {
            let mut doc = fixture(&format!("BT /F1 24 Tf 72 700 Td (Plain) Tj {changed} ( middle) Tj {restored} ( last) Tj ET"));
            let widths =
                |doc: &Document| text_lines(doc, 0).unwrap().iter().flat_map(|line| &line.ink).skip(1).map(|ink| ink[2] - ink[0]).collect::<Vec<_>>();
            let before = widths(&doc);
            assert_eq!(before.len(), 2);
            if paragraph {
                let block = text_blocks(&doc, 0).unwrap().remove(0);
                replace_block(&mut doc, 0, 0, &block.editor_text.replace("Plain", "Plxin")).unwrap();
            } else {
                let line = text_lines(&doc, 0).unwrap().remove(0);
                replace_line(&mut doc, 0, 0, &line.text.replace("Plain", "Plxin")).unwrap();
            }
            let reopened = Document::open(Arc::new(write_full(&doc, &SaveOptions::default()).unwrap())).unwrap();
            let after = widths(&reopened);
            let edited_text = text_blocks(&reopened, 0).unwrap()[0].editor_text.clone();
            assert!(edited_text.contains("Plxin") && !edited_text.contains("Plain"), "edit was saved: {edited_text}");
            assert_eq!(after.len(), before.len());
            assert!(before.iter().zip(after).all(|(a, b)| (a - b).abs() < 0.001), "state {changed}, paragraph={paragraph}");
        }
    }
}

#[test]
fn encoded_spaces_with_different_state_stay_in_the_paragraph_3215() {
    for (changed, restored) in [("20 Tz", "100 Tz"), ("-5 Tc", "0 Tc")] {
        let mut doc = fixture(&format!("BT /F1 24 Tf 72 700 Td (Hello) Tj {changed} ( ) Tj {restored} (world) Tj ET"));
        assert_eq!(text_blocks(&doc, 0).unwrap()[0].editor_text, "Hello world", "whitespace state: {changed}");
        replace_block(&mut doc, 0, 0, "Hallo world").unwrap();
        let reopened = Document::open(Arc::new(write_full(&doc, &SaveOptions::default()).unwrap())).unwrap();
        assert_eq!(text_blocks(&reopened, 0).unwrap()[0].editor_text, "Hallo world", "saved whitespace state: {changed}");
    }
}

const CONTENT: &str = "BT /F1 24 Tf 72 700 Td 0.001 Tc -0.002 Tw (Section 1 ) Tj 0 Tc 0 Tw (- ) Tj (Main Heading) Tj ET";
const BEFORE: &str = "Section 1 - Main Heading";
const AFTER: &str = "Section 2 - Main Heading";

/// Capture public source-line text and diagnostic row geometry.
fn snapshot(doc: &Document) -> String {
    let lines = text_lines(doc, 0).unwrap();
    let blocks = text_blocks(doc, 0).unwrap();
    eprintln!(
        "lines={:?}; blocks={:?}",
        lines.iter().map(|line| (&line.text, &line.ink)).collect::<Vec<_>>(),
        blocks.iter().map(|block| &block.editor_text).collect::<Vec<_>>()
    );
    lines.iter().map(|line| line.text.as_str()).collect()
}

/// Full-save and independently parse before reopening for post-edit assertions.
fn reopen(doc: &Document) -> Document {
    let bytes = write_full(doc, &SaveOptions::default()).unwrap();
    hayro_syntax::Pdf::new(bytes.clone()).expect("saved PDF parses independently");
    Document::open(Arc::new(bytes)).unwrap()
}

#[test]
fn jitter_ordinary_heading_stays_one_line() {
    let doc = fixture(CONTENT);
    assert_eq!(snapshot(&doc), BEFORE);
    assert_eq!(text_lines(&doc, 0).unwrap().len(), 1, "tiny Tc jitter must not fragment this ordinary heading");
}

#[test]
fn jitter_full_line_replacement_does_not_duplicate_suffix() {
    let mut doc = fixture(CONTENT);
    assert_eq!(snapshot(&doc), BEFORE);
    replace_line(&mut doc, 0, 0, AFTER).unwrap();
    let reopened = reopen(&doc);
    assert_eq!(snapshot(&reopened), AFTER, "full-line edit duplicated the retained suffix");
}

#[test]
fn jitter_current_source_run_replacement_control() {
    let mut doc = fixture(CONTENT);
    let line = text_lines(&doc, 0).unwrap().remove(0);
    replace_line(&mut doc, 0, 0, &line.text.replacen("Section 1", "Section 2", 1)).unwrap();
    assert_eq!(snapshot(&reopen(&doc)), AFTER);
}

#[test]
fn jitter_paragraph_replacement_control() {
    let mut doc = fixture(CONTENT);
    let blocks = text_blocks(&doc, 0).unwrap();
    assert_eq!(blocks.len(), 1);
    assert_eq!(blocks[0].editor_text, BEFORE);
    replace_block(&mut doc, 0, 0, AFTER).unwrap();
    let reopened = reopen(&doc);
    assert_eq!(text_lines(&reopened, 0).unwrap().len(), 1, "paragraph save must not fragment the public line");
    snapshot(&reopened);
    assert_eq!(text_blocks(&reopened, 0).unwrap()[0].editor_text, AFTER);
}

/// Read every content stream in order for an independent effective-state oracle.
fn content_bytes(doc: &Document) -> Vec<u8> {
    use pdfcraft_cos::Object;
    let page = pdfcraft_model::pages(doc).remove(0);
    let contents = doc.resolve(page.dict.get(b"Contents").unwrap());
    let objects = match &*contents {
        Object::Array(items) => items.clone(),
        other => vec![other.clone()],
    };
    objects
        .iter()
        .flat_map(|object| match &*doc.resolve(object) {
            Object::Stream(stream) => stream.decoded().unwrap(),
            _ => Vec::new(),
        })
        .collect()
}

/// Effective Tc/Tw/Tz/Tf size for each ASCII character, without the edit crate's run grouping.
fn character_states(doc: &Document) -> Vec<(char, f64, f64, f64, f64)> {
    use pdfcraft_cos::Object;
    let mut state = (0.0, 0.0, 100.0, 0.0);
    let mut stack = Vec::new();
    let mut result = Vec::new();
    for op in pdfcraft_content::parse(&content_bytes(doc)).ops {
        match op.op.as_slice() {
            b"q" => stack.push(state),
            b"Q" => state = stack.pop().unwrap(),
            b"Tc" => state.0 = op.num(0).unwrap(),
            b"Tw" => state.1 = op.num(0).unwrap(),
            b"Tz" => state.2 = op.num(0).unwrap(),
            b"Tf" => state.3 = op.num(1).unwrap(),
            b"Tj" | b"'" => {
                let text = op.operands.first().and_then(Object::as_string).unwrap().to_text();
                result.extend(text.chars().map(|c| (c, state.0, state.1, state.2, state.3)));
            }
            b"TJ" => {
                for string in op.operands.iter().filter_map(Object::as_array).flatten().filter_map(Object::as_string) {
                    result.extend(string.to_text().chars().map(|c| (c, state.0, state.1, state.2, state.3)));
                }
            }
            b"\"" => {
                state.1 = op.num(0).unwrap();
                state.0 = op.num(1).unwrap();
                result.extend(
                    op.operands.get(2).and_then(Object::as_string).unwrap().to_text().chars().map(|c| (c, state.0, state.1, state.2, state.3)),
                );
            }
            _ => {}
        }
    }
    result
}

/// Replace the initial A while preserving every later source character's exact spacing state.
fn check_exact_states(content: &str, paragraph: bool) {
    let mut doc = fixture(content);
    let before = character_states(&doc);
    let before_ink: Vec<_> = text_lines(&doc, 0).unwrap().into_iter().flat_map(|line| line.ink).collect();
    let source = text_blocks(&doc, 0).unwrap().remove(0).editor_text;
    let replacement = source.replacen('A', "B", 1);
    let width = untouched_word_width(&doc);
    if paragraph {
        replace_block(&mut doc, 0, 0, &replacement).unwrap();
    } else {
        replace_line(&mut doc, 0, 0, &replacement).unwrap();
    }
    let reopened = reopen(&doc);
    let after = character_states(&reopened);
    assert_eq!(after.first().unwrap().0, 'B');
    assert_eq!(after.len(), before.len(), "source character count, paragraph={paragraph}");
    assert!(
        after.iter().skip(1).zip(before.iter().skip(1)).all(|(a, b)| a == b),
        "exact authored state survives, paragraph={paragraph}; first mismatch {:?}",
        after.iter().zip(&before).enumerate().skip(1).find(|(_, (a, b))| a != b)
    );
    let after_ink: Vec<_> = text_lines(&reopened, 0).unwrap().into_iter().flat_map(|line| line.ink).collect();
    assert_eq!(after_ink.len(), before_ink.len());
    for (before, after) in before_ink.iter().zip(&after_ink) {
        for (a, b) in before.iter().zip(after) {
            assert!((a - b).abs() <= 1e-9 * a.abs().max(1.0), "every run retains its position: {before:?} -> {after:?}");
        }
    }
    assert!((untouched_word_width(&reopened) - width).abs() <= 1e-6 * width.abs().max(1e-6), "untouched glyph geometry, paragraph={paragraph}");
}

#[test]
fn jitter_exact_states_survive_long_repeated_and_magnified_runs() {
    let mut repeated = String::from("BT /F1 24 Tf 72 700 Td (A) Tj ");
    for _ in 0..64 {
        repeated.push_str("0.0000001 Tc (x) Tj -0.0000001 Tc (y) Tj 0 Tc (z) Tj ");
    }
    repeated.push_str("ET");
    let cases = [
        "BT /F1 24 Tf 72 700 Td (A ) Tj 0.0000001 Tc (tail) Tj ET".to_string(),
        format!("BT /F1 1 Tf 72 700 Td (A ) Tj 0.0000001 Tc ({}) Tj ET", "x".repeat(2000)),
        repeated,
        "q 100000 0 0 1 0 0 cm BT /F1 24 Tf 0.00001 0 0 1 0.00072 700 Tm (A ) Tj 0.0000001 Tc (tail) Tj ET Q".into(),
        "q 100000 0 0 100000 0 0 cm BT /F1 0.00024 Tf 0.00072 0.007 Td (A ) Tj 0.0000001 Tc (tail) Tj ET Q".into(),
        "BT /F1 24 Tf 72 700 Td (A ) Tj 99.9999999 Tz (tail) Tj ET".into(),
        "BT /F1 24 Tf 72 700 Td (A ) Tj -0.001 Tc (tail) Tj ET".into(),
        "BT /F1 0.00024 Tf 72 700 Td (A ) Tj 0.0000001 Tc (tail) Tj ET".into(),
        "BT /F1 24 Tf 0.0000000001 0 0 1 72 700 Tm (A ) Tj 0.0000001 Tc (tail) Tj ET".into(),
    ];
    for (index, content) in cases.iter().enumerate() {
        for paragraph in [false, true] {
            eprintln!("exact state case {index}, paragraph={paragraph}");
            check_exact_states(content, paragraph);
        }
    }
}

#[test]
fn mixed_line_edit_keeps_relative_same_bt_neighbor_and_does_not_wrap() {
    for prefix in ["72 700 Td", "24 TL 72 724 Td T*"] {
        let content = format!("BT /F1 24 Tf {prefix} (A ) Tj 3 Tc (tail) Tj /F1 30 Tf ( NEXT) Tj ET");
        let mut doc = fixture(&content);
        let before = text_lines(&doc, 0).unwrap();
        assert_eq!(before.len(), 2);
        let next = before[1].rect;
        let replacement = format!("B {}tail", "wide ".repeat(80));
        replace_line(&mut doc, 0, 0, &replacement).unwrap();
        let reopened = reopen(&doc);
        let after = text_lines(&reopened, 0).unwrap();
        assert_eq!(after.len(), 2, "line editing never wraps");
        assert_eq!(after[0].text, replacement);
        assert_eq!(after[1].text, " NEXT");
        let advance = after[0].rect[2] - before[0].rect[2];
        assert!((after[1].rect[0] - next[0] - advance).abs() < 0.001, "following same-BT text follows the edited advance");
        assert_eq!(after[1].baseline()[1], before[1].baseline()[1]);
    }
}

#[test]
fn mixed_paragraph_edit_preserves_public_selection_and_other_paragraphs() {
    // Explicit normal leading makes the 70-point gaps paragraph breaks. Without TL, the
    // repeated gaps intentionally count as one widely led paragraph in upstream grouping.
    let mut doc = fixture("BT /F1 24 Tf 24 TL 72 700 Td (Before) Tj 0 -70 Td (A ) Tj 0.001 Tc (tail) Tj 0 Tc 0 -70 Td (After) Tj ET");
    let before = text_lines(&doc, 0).unwrap();
    assert_eq!(text_blocks(&doc, 0).unwrap().len(), 3);
    replace_block(&mut doc, 0, 1, "B tail").unwrap();
    let reopened = reopen(&doc);
    let after = text_lines(&reopened, 0).unwrap();
    assert_eq!(after.len(), 3);
    assert_eq!(after[0].text, before[0].text);
    assert_eq!(after[0].rect, before[0].rect);
    assert_eq!(after[2].text, before[2].text);
    assert_eq!(after[2].rect, before[2].rect);
    assert_eq!(text_blocks(&reopened, 0).unwrap()[1].editor_text, "B tail");
    replace_line(&mut doc, 0, 1, "C tail").unwrap();
    assert_eq!(text_blocks(&reopen(&doc), 0).unwrap()[1].editor_text, "C tail");
}

#[test]
fn unsupported_mixed_positioning_refuses_without_mutating() {
    let infinity = "9".repeat(400);
    let cases = [
        "BT /F1 24 Tf 72 700 Td (A ) Tj 1 0 0 1 100 700 Tm 3 Tc (tail) Tj ET".into(),
        "BT /F1 24 Tf 0 1 -1 0 300 300 Tm (A ) Tj 3 Tc (tail) Tj ET".into(),
        "BT /F1 24 Tf 1 0 0.2 1 72 700 Tm (A ) Tj 3 Tc (tail) Tj ET".into(),
        "BT /F1 24 Tf 72 700 Td (A ) Tj -30 Tc (tail) Tj ET".into(),
        format!("BT /F1 24 Tf 72 700 Td (A ) Tj {infinity} Tc (tail) Tj ET"),
        "BT /F1 24 Tf 72 700 Td (A ) Tj 3 Tc 3 Tr (tail) Tj ET".into(),
        "BT /F1 24 Tf 72 700 Td (A ) Tj 3 Tc 1 Ts (tail) Tj ET".into(),
        "BT /F1 24 Tf 72 700 Td (A ) Tj 3 Tr (hidden) Tj 0 Tr ( visible) Tj 0.001 Tc (tail) Tj ET".into(),
    ];
    for (index, content) in cases.iter().enumerate() {
        for paragraph in [false, true] {
            let mut doc = fixture(content);
            let before = write_full(&doc, &SaveOptions::default()).unwrap();
            let result = if paragraph { replace_block(&mut doc, 0, 0, "B tail") } else { replace_line(&mut doc, 0, 0, "B tail") };
            assert!(result.is_err(), "case {index}, paragraph={paragraph}");
            assert_eq!(write_full(&doc, &SaveOptions::default()).unwrap(), before, "refusal changes nothing");
        }
    }
}

#[test]
fn mixed_paragraph_relative_scoped_neighbor_refuses_without_mutating() {
    let mut doc = fixture("BT /F1 24 Tf 72 700 Td /P BMC (A ) Tj 0.001 Tc (tail) Tj EMC /P BMC ( NEXT) Tj EMC ET");
    assert_eq!(text_blocks(&doc, 0).unwrap().len(), 2);
    let before = write_full(&doc, &SaveOptions::default()).unwrap();
    assert!(replace_block(&mut doc, 0, 0, "B tail").is_err());
    assert_eq!(write_full(&doc, &SaveOptions::default()).unwrap(), before);
    replace_line(&mut doc, 0, 0, "B tail").unwrap();
    assert_eq!(text_lines(&reopen(&doc), 0).unwrap().len(), 2);
}

#[test]
fn mixed_paragraph_uses_semantic_duplicate_selection_only() {
    for copy in [
        "BT /F1 24 Tf 0 Tc 72 700 Td (A tail) Tj ET",
        "BT /F1 24 Tf 0 Tc 94.68 700 Td (tail) Tj ET",
        "BT /F1 24 Tf 0 Tc 4 Tr 94.68 700 Td (tail) Tj ET",
    ] {
        let mut doc = fixture(&format!("BT /F1 24 Tf 72 700 Td (A ) Tj 0.001 Tc (tail) Tj ET {copy}"));
        assert_eq!(text_blocks(&doc, 0).unwrap().len(), 2);
        let before = text_lines(&doc, 0).unwrap();
        replace_block(&mut doc, 0, 0, "B tail").unwrap();
        let reopened = reopen(&doc);
        let after = text_lines(&reopened, 0).unwrap();
        if copy.contains("(A tail)") {
            assert_eq!(after.len(), 1, "full visible duplicate is replaced together with its semantic target");
            assert_eq!(after[0].text, "B tail");
        } else {
            assert_eq!(after.len(), 2, "private fragments do not newly select a partial-text neighbour");
            assert_eq!(after[1].text, before[1].text);
            assert_eq!(after[1].rect, before[1].rect);
        }
    }
}

#[test]
fn mixed_line_quote_entry_restores_original_following_state() {
    for first in ["(A ) '", "-0.002 0.001 (A ) \""] {
        let mut doc = fixture(&format!("BT /F1 24 Tf 24 TL 72 724 Td {first} 3 Tc (tail) Tj /F1 30 Tf ( NEXT) Tj ET"));
        let before = character_states(&doc);
        replace_line(&mut doc, 0, 0, "B tail").unwrap();
        let reopened = reopen(&doc);
        assert_eq!(text_lines(&reopened, 0).unwrap()[0].text, "B tail");
        assert_eq!(&character_states(&reopened)[1..], &before[1..]);
    }
}

#[test]
fn mixed_paragraph_fallback_failure_is_atomic() {
    use pdfcraft_cos::{Dict, ObjRef, Object, Stream};
    let mut doc = fixture("BT /F1 24 Tf 72 700 Td (A ) Tj 0.001 Tc (tail) Tj ET");
    // The repo's attributed OFL Inter program deliberately has more glyphs than this
    // synthetic WinAnsi source encoding, forcing full-program completion for the first č.
    let program = doc.add(Object::Stream(Stream::from_raw(Dict::new(), include_bytes!("../../../assets/fonts/Inter-Regular.ttf").to_vec())));
    let mut descriptor = Dict::new();
    descriptor.set(b"FontFile2".to_vec(), Object::Ref(program));
    doc.update_dict(ObjRef::new(5, 0), |font| {
        font.set(b"FontDescriptor".to_vec(), Object::Dict(descriptor));
    })
    .unwrap();
    let before = write_full(&doc, &SaveOptions::default()).unwrap();
    let mut control = doc.clone();
    replace_block(&mut control, 0, 0, "č tail").unwrap();
    assert!(write_full(&control, &SaveOptions::default()).unwrap().len() > before.len(), "control completes the source font");
    assert!(replace_block(&mut doc, 0, 0, "č\u{10ffff} tail").is_err());
    assert_eq!(write_full(&doc, &SaveOptions::default()).unwrap(), before, "a later unsupported glyph rolls back earlier font allocation");
    for replacement in ["א tail", "क tail", "B\u{0301} tail"] {
        assert!(replace_block(&mut doc, 0, 0, replacement).is_err());
        assert_eq!(write_full(&doc, &SaveOptions::default()).unwrap(), before);
    }
}

/// Independent per-glyph pen oracle for these synthetic horizontal source programs.
fn glyph_positions(doc: &Document) -> Vec<(char, f64, f64)> {
    use pdfcraft_content::Matrix;
    use pdfcraft_cos::Object;
    let page = pdfcraft_model::pages(doc).remove(0);
    let resources = doc.dict(page.dict.get(b"Resources").unwrap()).unwrap();
    let fonts = doc.dict(resources.get(b"Font").unwrap()).unwrap();
    let mut state = (Matrix::IDENTITY, String::new(), 0.0, 0.0, 0.0, 1.0, 0.0);
    let mut stack = Vec::new();
    let mut tm = Matrix::IDENTITY;
    let mut tlm = tm;
    let mut out = Vec::new();
    for op in pdfcraft_content::parse(&content_bytes(doc)).ops {
        match op.op.as_slice() {
            b"q" => stack.push(state.clone()),
            b"Q" => state = stack.pop().unwrap(),
            b"cm" => state.0 = Matrix(op.nums::<6>().unwrap()).then(&state.0),
            b"BT" => {
                tm = Matrix::IDENTITY;
                tlm = tm;
            }
            b"Tf" => {
                state.1 = String::from_utf8(op.name(0).unwrap().to_vec()).unwrap();
                state.2 = op.num(1).unwrap();
            }
            b"Tc" => state.3 = op.num(0).unwrap(),
            b"Tw" => state.4 = op.num(0).unwrap(),
            b"Tz" => state.5 = op.num(0).unwrap() / 100.0,
            b"TL" => state.6 = op.num(0).unwrap(),
            b"Tm" => {
                tm = Matrix(op.nums::<6>().unwrap());
                tlm = tm;
            }
            b"Td" | b"TD" => {
                let [x, y] = op.nums::<2>().unwrap();
                if op.is("TD") {
                    state.6 = -y;
                }
                tlm = Matrix([1.0, 0.0, 0.0, 1.0, x, y]).then(&tlm);
                tm = tlm;
            }
            b"T*" => {
                tlm = Matrix([1.0, 0.0, 0.0, 1.0, 0.0, -state.6]).then(&tlm);
                tm = tlm;
            }
            b"Tj" | b"TJ" | b"'" | b"\"" => {
                if matches!(op.op.as_slice(), b"'" | b"\"") {
                    if op.is("\"") {
                        state.4 = op.num(0).unwrap();
                        state.3 = op.num(1).unwrap();
                    }
                    tlm = Matrix([1.0, 0.0, 0.0, 1.0, 0.0, -state.6]).then(&tlm);
                    tm = tlm;
                }
                let metrics = pdfcraft_fonts::pdf::Metrics::from_dict(doc, &doc.dict(fonts.get(state.1.as_bytes()).unwrap()).unwrap());
                let pieces = if op.is("TJ") {
                    op.operands.last().and_then(Object::as_array).unwrap().clone()
                } else {
                    vec![op.operands.last().unwrap().clone()]
                };
                for piece in pieces {
                    if let Object::String(string) = piece {
                        for (code, len) in metrics.codes(&string.bytes) {
                            let text = metrics.text_of(code).unwrap();
                            let point = tm.then(&state.0).apply(0.0, 0.0);
                            out.extend(text.chars().map(|ch| (ch, point.0, point.1)));
                            let advance =
                                (metrics.width(code) * state.2 + state.3 + if metrics.is_space(code, len) { state.4 } else { 0.0 }) * state.5;
                            tm = Matrix([1.0, 0.0, 0.0, 1.0, advance, 0.0]).then(&tm);
                        }
                    } else {
                        let advance = -piece.as_f64().unwrap() / 1000.0 * state.2 * state.5;
                        tm = Matrix([1.0, 0.0, 0.0, 1.0, advance, 0.0]).then(&tm);
                    }
                }
            }
            _ => {}
        }
    }
    out
}

/// Read serialized TJ values without relying on edited text grouping.
fn tj_numbers(doc: &Document) -> Vec<f64> {
    pdfcraft_content::parse(&content_bytes(doc))
        .ops
        .iter()
        .filter(|op| op.is("TJ"))
        .flat_map(|op| op.operands.iter().filter_map(pdfcraft_cos::Object::as_array).flatten().filter_map(pdfcraft_cos::Object::as_f64))
        .collect()
}

/// A/B and 1/2 have equal Helvetica advances, so every original glyph position must survive.
fn check_positioned_geometry(content: &str, old: &str, new: &str, paragraph: bool) {
    let mut doc = fixture(content);
    let before = glyph_positions(&doc);
    let source = if paragraph { text_blocks(&doc, 0).unwrap()[0].editor_text.clone() } else { text_lines(&doc, 0).unwrap()[0].text.clone() };
    let replacement = source.replacen(old, new, 1);
    assert_ne!(replacement, source);
    if paragraph {
        replace_block(&mut doc, 0, 0, &replacement).unwrap();
    } else {
        replace_line(&mut doc, 0, 0, &replacement).unwrap();
    }
    let after = reopen(&doc);
    let states = character_states(&doc);
    let saved_states = character_states(&after);
    assert_eq!(states, saved_states, "full-save state precision");
    let points = glyph_positions(&after);
    assert_eq!(before.len(), points.len());
    if paragraph {
        assert_eq!(text_blocks(&after, 0).unwrap()[0].editor_text, replacement);
    } else {
        assert_eq!(text_lines(&after, 0).unwrap()[0].text, replacement);
    }
    for (index, (a, b)) in before.iter().zip(&points).enumerate() {
        assert!((a.1 - b.1).abs() < 1e-8 && (a.2 - b.2).abs() < 1e-8, "glyph {index}: {a:?} -> {b:?}, paragraph={paragraph}");
    }
    let mut repeated = after;
    if paragraph {
        replace_block(&mut repeated, 0, 0, &source).unwrap();
    } else {
        replace_line(&mut repeated, 0, 0, &source).unwrap();
    }
    for (a, b) in before.iter().zip(glyph_positions(&reopen(&repeated))) {
        assert_eq!(a.0, b.0);
        assert!((a.1 - b.1).abs() < 1e-8 && (a.2 - b.2).abs() < 1e-8, "second edit keeps source geometry");
    }
}

#[test]
fn kerned_word_jitter_edits_preserve_every_glyph() {
    let content = "BT /F1 24 Tf 72 700 Td 0.001 Tc -0.002 Tw [(Sect) 3 (ion 1 )] TJ 0 Tc 0 Tw (- ) Tj (Main Heading) Tj ET";
    for paragraph in [false, true] {
        check_positioned_geometry(content, "1", "2", paragraph);
    }
}

#[test]
fn kerned_word_jitter_preview_matches_apply_without_mutating_source() {
    let mut doc = fixture("BT /F1 24 Tf 72 700 Td 0.001 Tc -0.002 Tw [(Sect) 3 (ion 1 )] TJ 0 Tc 0 Tw (- ) Tj (Main Heading) Tj ET");
    let saved = write_full(&doc, &SaveOptions::default()).unwrap();
    let draft = "Section 2 - Main Heading";
    let layout = pdfcraft_edit::text::text_edit_layout(&doc, 0, 0, draft, &Default::default()).unwrap();
    assert_eq!(write_full(&doc, &SaveOptions::default()).unwrap(), saved, "preview must not mutate the source");
    assert_eq!(layout.source_rect, text_blocks(&doc, 0).unwrap()[0].rect);
    assert!(!layout.overflow);
    replace_block(&mut doc, 0, 0, draft).unwrap();
    let after = reopen(&doc);
    let blocks = text_blocks(&after, 0).unwrap();
    assert_eq!(blocks.len(), 1);
    assert_eq!(blocks[0].editor_text, draft);
    assert_eq!(layout.draft_rect, Some(blocks[0].rect), "preview bounds must match the applied, saved edit");
}

#[test]
fn positioned_mixed_runs_keep_signed_numeric_only_and_sentinel_gaps() {
    let cases = [
        "BT /F1 24 Tf 72 700 Td (A ) Tj 3 Tc [(t) -100 (ai) 100 (l)] TJ ET",
        "BT /F1 24 Tf 72 700 Td (A ) Tj 60 Tz [3] TJ 100 Tz 0.001 Tc (tail) Tj ET",
        "BT /F1 24 Tf 72 700 Td [7 (X A )] TJ [] TJ [()] TJ 0.001 Tc [(tail) -8] TJ ET",
        "q 100000 0 0 1 0 0 cm BT /F1 24 Tf 0.00001 0 0 1 0.00072 700 Tm (A ) Tj 0.0000001 Tc [(ta) 0.00001 (il)] TJ ET Q",
        r"BT /F1 24 Tf 72 700 Td (A ) Tj 0.001 Tc [(caf\351) 3 (teria)] TJ ET",
        "BT /F1 24 Tf 72 700 Td (A ) Tj 0.001 Tc [(ta) 3 (il)] TJ /F1 30 Tf ( NEXT) Tj ET",
    ];
    for (index, content) in cases.iter().enumerate() {
        for paragraph in [false, true] {
            if index == 5 && paragraph {
                continue;
            } // Its unselected same-BT neighbour remains unsupported for paragraph replacement.
            check_positioned_geometry(content, "A", "B", paragraph);
        }
    }
}

#[test]
fn positioned_boundaries_follow_retained_glyph_identity_not_inherited_style() {
    for paragraph in [false, true] {
        for (replacement, expected) in [("ab rail", vec![10.0]), ("axb tail", vec![]), ("a tail", vec![]), ("zb tail", vec![])] {
            let mut doc = fixture("BT /F1 24 Tf 72 700 Td [(a) 10 (b)] TJ 0.001 Tc ( tail) Tj ET");
            if paragraph {
                replace_block(&mut doc, 0, 0, replacement).unwrap();
            } else {
                replace_line(&mut doc, 0, 0, replacement).unwrap();
            }
            let numbers = tj_numbers(&reopen(&doc));
            assert_eq!(numbers.len(), expected.len(), "{replacement}, paragraph={paragraph}: {numbers:?}");
            assert!(numbers.iter().zip(expected).all(|(a, b)| (a - b).abs() < 1e-10));
        }
        for (replacement, expected) in [("X ab rail", vec![5.0, 6.0]), ("zX ab tail", vec![6.0]), ("X ab tailz", vec![5.0])] {
            let mut doc = fixture("BT /F1 24 Tf 72 700 Td [5 (X ab)] TJ 0.001 Tc [( tail) 6] TJ ET");
            if paragraph {
                replace_block(&mut doc, 0, 0, replacement).unwrap();
            } else {
                replace_line(&mut doc, 0, 0, replacement).unwrap();
            }
            let numbers = tj_numbers(&reopen(&doc));
            assert_eq!(numbers.len(), expected.len(), "sentinel {replacement}, paragraph={paragraph}: {numbers:?}");
            assert!(numbers.iter().zip(expected).all(|(a, b)| (a - b).abs() < 1e-10));
        }
    }
}

#[test]
fn positioned_paragraph_wrap_drops_only_separated_pair_gap() {
    let mut doc = fixture("BT /F1 24 Tf 72 700 Td [(A ) -100 (tail)] TJ 0.001 Tc ( end) Tj ET");
    let replacement = format!("{}w1de A tail end", "wide ".repeat(7));
    replace_block(&mut doc, 0, 0, &replacement).unwrap();
    let after = reopen(&doc);
    let positions = glyph_positions(&after);
    let tail = positions.iter().find(|point| point.0 == 't').copied().unwrap();
    assert!((tail.1 - 72.0).abs() < 1e-8 && tail.2 < 700.0, "wrapped tail starts at the row origin: {tail:?}");
    assert!(tj_numbers(&after).is_empty(), "the old pair no longer shares a row");
}

#[test]
fn positioned_source_limits_and_formatting_refuse_atomically() {
    use pdfcraft_edit::text::{BlockStyle, rewrite_block};
    let many = "1 -1 ".repeat(50_001);
    for content in [
        format!("BT /F1 24 Tf 72 700 Td (A ) Tj 0.001 Tc [(tail) {many}] TJ ET"),
        "BT /F1 24 Tf 72 700 Td [(A) 3 ( tail)] TJ 0.001 Tc ( end) Tj ET".to_string(),
    ] {
        let mut doc = fixture(&content);
        let before = write_full(&doc, &SaveOptions::default()).unwrap();
        if content.len() > 100_000 {
            assert!(replace_line(&mut doc, 0, 0, "B tail").is_err());
        } else {
            assert!(rewrite_block(&mut doc, 0, 0, Some("B tail end"), &BlockStyle { size: Some(30.0), ..BlockStyle::default() }).is_err());
        }
        assert_eq!(write_full(&doc, &SaveOptions::default()).unwrap(), before);
    }
}

#[test]
fn positioned_source_alias_codes_and_inferred_tracking_refuse_atomically() {
    use pdfcraft_cos::{Dict, Object};
    for paragraph in [false, true] {
        let mut alias = fixture("BT /F1 24 Tf 72 700 Td (A ) Tj 0.001 Tc [(B) 3 (tail)] TJ ET");
        alias
            .update_dict(pdfcraft_cos::ObjRef::new(5, 0), |font| {
                let mut encoding = Dict::new();
                encoding.set(b"BaseEncoding".to_vec(), Object::name("WinAnsiEncoding"));
                encoding.set(b"Differences".to_vec(), Object::Array(vec![Object::Int(65), Object::name("A"), Object::name("A")]));
                font.set(b"Encoding".to_vec(), Object::Dict(encoding));
                font.set(b"FirstChar".to_vec(), Object::Int(65));
                font.set(b"LastChar".to_vec(), Object::Int(66));
                font.set(b"Widths".to_vec(), Object::Array(vec![Object::Int(500), Object::Int(1000)]));
            })
            .unwrap();
        let tracking = fixture("BT /F1 24 Tf 72 700 Td 0.001 Tc [(A) -300 (B) -300 (C) -300 (D)] TJ 0 Tc (X) Tj ET");
        for mut doc in [alias, tracking] {
            let before = write_full(&doc, &SaveOptions::default()).unwrap();
            let result = if paragraph { replace_block(&mut doc, 0, 0, "ABCX") } else { replace_line(&mut doc, 0, 0, "ABCX") };
            assert!(result.is_err());
            assert_eq!(write_full(&doc, &SaveOptions::default()).unwrap(), before);
        }
    }
}

#[test]
fn positioned_long_runs_and_outside_numeric_prefix_keep_geometry() {
    let long = format!("BT /F1 1 Tf 72 700 Td (A ) Tj 0.001 Tc [{}] TJ ET", "(x) 0.00001 ".repeat(2000));
    let prefix = "BT /F1 24 Tf 72 700 Td [3] TJ (A ) Tj 0.001 Tc [(ta) 3 (il)] TJ ET";
    let anisotropic = "q 100 0 0 1 0 0 cm BT /F1 1 Tf 0.72 700 Td (A ) Tj 0.001 Tc [(ta) -3 (il)] TJ ET Q";
    let tiny_vertical = "q 1000 0 0 1 0 0 cm BT /F1 0.024 Tf 0.072 700 Td (A ) Tj 0.000001 Tc [(ta) 3 (il)] TJ ET Q";
    for content in [long.as_str(), prefix, anisotropic, tiny_vertical] {
        for paragraph in [false, true] {
            check_positioned_geometry(content, "A", "B", paragraph);
        }
    }
}

#[test]
fn positioned_multi_style_row_refuses_without_reassigning_boundary_ownership() {
    let mut doc = fixture("BT /F1 24 Tf 72 700 Td (Other ) Tj 1 0 0 rg 0.001 Tc [(A) 3 ( tail)] TJ 0 Tc ( end) Tj ET");
    assert!(text_lines(&doc, 0).unwrap().len() > 1);
    assert_eq!(text_blocks(&doc, 0).unwrap().len(), 1);
    let before = write_full(&doc, &SaveOptions::default()).unwrap();
    assert!(replace_block(&mut doc, 0, 0, "Oth A tail end").is_err());
    assert_eq!(write_full(&doc, &SaveOptions::default()).unwrap(), before);
}

#[test]
fn positioned_quote_entry_and_other_row_keep_geometry() {
    let cases = [
        "BT /F1 24 Tf 24 TL 72 724 Td (A ) ' 0.001 Tc [(ta) 3 (il)] TJ ET",
        r#"BT /F1 24 Tf 24 TL 72 724 Td 0.002 0.001 (A ) " 0.002 Tc [(ta) 3 (il)] TJ ET"#,
        "BT /F1 24 Tf 24 TL 72 700 Td (A ) Tj 3 Tc (tail) Tj 0 Tc T* [(Other) -100 (row)] TJ ET",
    ];
    for content in cases {
        for paragraph in [false, true] {
            check_positioned_geometry(content, "A", "B", paragraph);
        }
    }
}

#[test]
fn cached_text_session_keeps_mixed_spacing_across_repeated_drafts() {
    let mut doc = fixture("BT /F1 24 Tf 72 700 Td 0.001 Tc -0.002 Tw [(Sect) 3 (ion 1 )] TJ 0 Tc 0 Tw (- ) Tj (Main Heading) Tj ET");
    let saved = write_full(&doc, &SaveOptions::default()).unwrap();
    let session = pdfcraft_edit::text::TextSession::open(&doc, 0, 0).unwrap();
    let draft = "Section 2 - Main Heading";
    let first = session.layout(draft, &Default::default()).unwrap();
    session.layout("Section 3 - Main Heading", &Default::default()).unwrap();
    let repeated = session.layout(draft, &Default::default()).unwrap();
    assert_eq!(first.draft_rect, repeated.draft_rect);
    assert_eq!(write_full(&doc, &SaveOptions::default()).unwrap(), saved);
    session.commit(&mut doc, Some(draft), &Default::default()).unwrap();
    let after = reopen(&doc);
    let blocks = text_blocks(&after, 0).unwrap();
    assert_eq!(blocks[0].editor_text, draft);
    assert_eq!(first.draft_rect, Some(blocks[0].rect));
    let changed = write_full(&doc, &SaveOptions::default()).unwrap();
    assert!(session.commit(&mut doc, Some("Section 4 - Main Heading"), &Default::default()).is_err());
    assert_eq!(write_full(&doc, &SaveOptions::default()).unwrap(), changed);
}

#[test]
fn mixed_spacing_underlines_restate_inner_text_paint_after_save() {
    use pdfcraft_cos::{Dict, ObjRef, Object};
    use pdfcraft_edit::text::{BlockStyle, rewrite_block};
    let mut doc = fixture("BT /GS1 gs /F1 24 Tf 72 700 Td (A ) Tj 0.001 Tc 95 Tz (tail) Tj ET");
    doc.update_dict(ObjRef::new(3, 0), |page| {
        let Object::Dict(resources) = page.get_mut(b"Resources").unwrap() else { panic!("resources") };
        let mut paint = Dict::new();
        paint.set(b"CA".to_vec(), Object::Real(0.25));
        let mut states = Dict::new();
        states.set(b"GS1".to_vec(), Object::Dict(paint));
        resources.set(b"ExtGState".to_vec(), Object::Dict(states));
    })
    .unwrap();
    rewrite_block(&mut doc, 0, 0, Some("B tail"), &BlockStyle { underline: Some(true), ..Default::default() }).unwrap();
    let saved = reopen(&doc);
    assert_no_graphics_stack_inside_text(&saved);
    let mut state = None;
    let mut stack = Vec::new();
    let mut strokes = 0;
    for op in pdfcraft_content::parse(&content_bytes(&saved)).ops {
        match op.op.as_slice() {
            b"q" => stack.push(state.clone()),
            b"Q" => state = stack.pop().unwrap(),
            b"gs" => state = op.name(0).map(<[u8]>::to_vec),
            b"S" => {
                assert_eq!(state.as_deref(), Some(b"GS1".as_slice()), "underline retains source stroke alpha");
                strokes += 1;
            }
            _ => {}
        }
    }
    assert!(strokes >= 2, "both private spacing runs have underlines");
    assert_eq!(text_blocks(&saved, 0).unwrap()[0].editor_text, "B tail");
}

#[test]
fn sequential_text_objects_keep_exact_spacing_in_paragraph_only() {
    let content = "BT /F1 12 Tf 1 0 0 1 72 700 Tm (for company ) Tj ET \
        BT /F1 12 Tf 1 0 0 1 141.36 700 Tm (A) Tj ET \
        BT 0.0001 Tc /F1 12 Tf 1 0 0 1 149.364 700 Tm (EROSOL) Tj ET";
    let mut doc = fixture(content);
    let before = write_full(&doc, &SaveOptions::default()).unwrap();
    assert!(replace_line(&mut doc, 0, 0, "for companies AEROSOL").is_err(), "line splices cannot consume later absolute matrices");
    assert_eq!(write_full(&doc, &SaveOptions::default()).unwrap(), before);
    replace_block(&mut doc, 0, 0, "for companies AEROSOL").unwrap();
    let saved = reopen(&doc);
    assert_eq!(text_blocks(&saved, 0).unwrap()[0].editor_text, "for companies AEROSOL");
    let states = character_states(&saved);
    let text: String = states.iter().map(|s| s.0).collect();
    let at = text.find("EROSOL").unwrap();
    assert!(states[at..at + 6].iter().all(|s| s.1 == 0.0001), "authored tiny spacing remains exact: {states:?}");

    let mut manual = fixture(&content.replace("(A) Tj ET", "(A) Tj 1 0 0 1 149.364 700 Tm (X) Tj ET"));
    let before = write_full(&manual, &SaveOptions::default()).unwrap();
    assert!(replace_block(&mut manual, 0, 0, "replacement").is_err());
    assert_eq!(write_full(&manual, &SaveOptions::default()).unwrap(), before);
}

fn assert_no_graphics_stack_inside_text(doc: &Document) {
    let mut text = false;
    for op in pdfcraft_content::parse(&content_bytes(doc)).ops {
        match op.op.as_slice() {
            b"BT" => {
                assert!(!text, "nested text object");
                text = true;
            }
            b"ET" => {
                assert!(text, "unmatched end text");
                text = false;
            }
            b"q" | b"Q" => assert!(!text, "graphics stack operator inside text object"),
            _ => {}
        }
    }
    assert!(!text);
}

#[test]
fn exact_rows_keep_graphics_isolation_outside_text_objects() {
    for content in ["BT /F1 24 Tf 72 700 Td (A ) Tj 0.001 Tc 95 Tz (tail) Tj ET", "BT /F1 24 Tf 72 700 Td (A ) Tj 0.001 Tc [(ta) 3 (il)] TJ ET"] {
        let mut doc = fixture(content);
        replace_block(&mut doc, 0, 0, "B tail").unwrap();
        let after = reopen(&doc);
        assert_no_graphics_stack_inside_text(&after);
        assert_eq!(text_blocks(&after, 0).unwrap()[0].editor_text, "B tail");
        replace_block(&mut doc, 0, 0, "A tail").unwrap();
        assert_no_graphics_stack_inside_text(&reopen(&doc));
    }
}

#[test]
fn surplus_tj_arrays_follow_the_drawn_last_operand() {
    for paragraph in [false, true] {
        check_positioned_geometry("BT /F1 24 Tf 72 700 Td (A ) Tj 0.001 Tc [(ta) 3 (il)] [(ta) -3 (il)] TJ ET", "A", "B", paragraph);
    }
}

#[test]
fn incompatible_exact_row_paint_refuses_atomically() {
    let mut doc = fixture("BT /F1 24 Tf 72 700 Td (A ) Tj 0.001 Tc (tail) Tj 1 Tr ( end) Tj ET");
    assert_eq!(text_blocks(&doc, 0).unwrap().len(), 1);
    let saved = write_full(&doc, &SaveOptions::default()).unwrap();
    assert!(replace_block(&mut doc, 0, 0, "B tail end").is_err());
    assert_eq!(write_full(&doc, &SaveOptions::default()).unwrap(), saved);
}

#[test]
fn positioned_source_graphics_stack_refuses_without_assuming_text_matrix_semantics() {
    for paragraph in [false, true] {
        let mut doc = fixture("BT /F1 24 Tf 72 700 Td (A ) Tj q 0.001 Tc [(ta) 3 (il)] TJ Q ( end) Tj ET");
        let saved = write_full(&doc, &SaveOptions::default()).unwrap();
        let result = if paragraph { replace_block(&mut doc, 0, 0, "B tail end") } else { replace_line(&mut doc, 0, 0, "B tail end") };
        assert!(result.is_err(), "inner q/Q matrix semantics are not supported by exact source mapping");
        assert_eq!(write_full(&doc, &SaveOptions::default()).unwrap(), saved);
    }
}

#[test]
fn exact_spacing_justification_refuses_atomically() {
    use pdfcraft_edit::{
        added::Align,
        text::{BlockStyle, rewrite_block},
    };
    let mut doc = fixture("BT /F1 12 Tf 72 700 Td (Alpha ) Tj 0.001 Tc (beta gamma) Tj ET");
    let before = write_full(&doc, &SaveOptions::default()).unwrap();
    let error =
        rewrite_block(&mut doc, 0, 0, Some("Alpha beta delta"), &BlockStyle { align: Some(Align::Justify), width: Some(80.0), ..Default::default() })
            .unwrap_err();
    assert!(error.to_string().contains("justification"), "{error}");
    assert_eq!(write_full(&doc, &SaveOptions::default()).unwrap(), before);
}

#[test]
fn exact_spacing_soft_wrap_preserves_consumed_spaces_after_reopen() {
    use pdfcraft_edit::text::{BlockStyle, rewrite_block};
    let mut doc = fixture("BT /F1 12 Tf 72 700 Td (Alpha  ) Tj 0.001 Tc (beta gamma) Tj ET");
    rewrite_block(&mut doc, 0, 0, Some("Alpha  beta delta"), &BlockStyle { width: Some(45.0), ..Default::default() }).unwrap();
    let saved = reopen(&doc);
    let blocks = text_blocks(&saved, 0).unwrap();
    assert_eq!(blocks.len(), 1);
    assert!(blocks[0].rows.len() > 1, "fixture must wrap");
    assert_eq!(blocks[0].reflow_text, "Alpha  beta delta");
    assert_no_graphics_stack_inside_text(&saved);
}

/// A following font-resource switch must survive an edit of mixed-spacing text in the same BT.
#[test]
fn mixed_spacing_keeps_following_font_resource_after_save() {
    use pdfcraft_cos::{Dict, ObjRef, Object};
    let mut doc = fixture("BT /F1 24 Tf 72 700 Td (A ) Tj 3 Tc 60 Tz (tail) Tj /F2 30 Tf ( NEXT) Tj ET");
    let second_font = doc.add(Object::Dict(Dict::from_iter([
        (b"Type".to_vec(), Object::name("Font")),
        (b"Subtype".to_vec(), Object::name("Type1")),
        (b"BaseFont".to_vec(), Object::name("Courier")),
        (b"Encoding".to_vec(), Object::name("WinAnsiEncoding")),
    ])));
    doc.update_dict(ObjRef::new(3, 0), |page| {
        page.set(
            b"Resources".to_vec(),
            Object::Dict(Dict::from_iter([(
                b"Font".to_vec(),
                Object::Dict(Dict::from_iter([(b"F1".to_vec(), Object::Ref(ObjRef::new(5, 0))), (b"F2".to_vec(), Object::Ref(second_font))])),
            )])),
        );
    })
    .unwrap();
    let before = text_lines(&doc, 0).unwrap();
    assert_eq!(before.len(), 2);
    assert_eq!((before[1].font.as_str(), before[1].base_font.as_str()), ("F2", "Courier"));
    let states = character_states(&doc);
    replace_line(&mut doc, 0, 0, "B longer tail").unwrap();
    let saved = reopen(&doc);
    let after = text_lines(&saved, 0).unwrap();
    assert_eq!(after.len(), 2);
    assert_eq!(after[0].text, "B longer tail");
    assert_eq!((after[1].text.as_str(), after[1].font.as_str(), after[1].base_font.as_str()), (" NEXT", "F2", "Courier"));
    assert_eq!(after[1].size, 30.0);
    assert_eq!(after[1].baseline()[1], before[1].baseline()[1]);
    let advance = after[0].rect[2] - before[0].rect[2];
    assert!((after[1].rect[0] - before[1].rect[0] - advance).abs() < 0.001);
    assert!(((after[1].rect[2] - after[1].rect[0]) - (before[1].rect[2] - before[1].rect[0])).abs() < 0.001);
    assert_eq!(character_states(&saved).into_iter().rev().take(5).collect::<Vec<_>>(), states.into_iter().rev().take(5).collect::<Vec<_>>());
}

/// A synthetic restricted subset has only a/b; mixed-spacing line edits cannot substitute c.
#[test]
fn mixed_spacing_subset_missing_glyph_refuses_without_mutation() {
    use pdfcraft_cos::{ObjRef, Object};
    let mut doc = fixture("BT /F1 24 Tf 72 700 Td (a) Tj 3 Tc (b) Tj ET");
    doc.update_dict(ObjRef::new(5, 0), |font| {
        font.set(b"Subtype".to_vec(), Object::name("TrueType"));
        font.set(b"BaseFont".to_vec(), Object::name("ABCDEF+SyntheticSans"));
        font.set(b"FirstChar".to_vec(), Object::Int(97));
        font.set(b"LastChar".to_vec(), Object::Int(99));
        font.set(b"Widths".to_vec(), Object::Array(vec![Object::Int(500), Object::Int(520), Object::Int(0)]));
    })
    .unwrap();
    let lines = text_lines(&doc, 0).unwrap();
    assert_eq!(lines.len(), 1);
    assert_eq!(lines[0].text, "ab");
    let before = write_full(&doc, &SaveOptions::default()).unwrap();
    let mut control = doc.clone();
    replace_line(&mut control, 0, 0, "bb").unwrap();
    assert_eq!(text_lines(&reopen(&control), 0).unwrap()[0].text, "bb");
    let error = replace_line(&mut doc, 0, 0, "ac").unwrap_err();
    assert!(error.to_string().contains("source font that can encode"), "{error}");
    assert_eq!(write_full(&doc, &SaveOptions::default()).unwrap(), before);
    assert_eq!(text_lines(&reopen(&doc), 0).unwrap()[0].text, "ab");
}
