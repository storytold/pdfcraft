//! Marked content whose paint a redaction removed loses the replacement text (`/ActualText`, `/Alt`,
//! `/E`, `/T`) its property list gives it, by a copy of that list without it written in place of
//! the name. A large list named by many such blocks would be copied once for each: past
//! `MAX_PROPERTY_COPIES` in a run, the run is refused rather than the content grown that much.

use super::*;
use crate::sanitize::{Hidden, remove_hidden};

/// A page whose property list `/M0` (object 7) gives replacement text and `junk` bytes besides, and
/// `blocks` marked blocks naming it, each drawing a glyph at the top of the page (`render` sets
/// their render mode), and a word at the bottom.
fn blocks_naming_one_list(blocks: usize, junk: usize, render: &str) -> Document {
    let block = format!("/Span /M0 BDC BT /F1 10 Tf {render} 20 260 Td (S) Tj ET EMC ");
    let content = format!("{} BT /F1 10 Tf 20 20 Td (PUBLIC) Tj ET", block.repeat(blocks));
    let list = format!("<< /ActualText (SECRET) /Junk ({}) >>", "x".repeat(junk));
    one_page(content.as_bytes(), "/Properties << /M0 7 0 R >>", vec![list.into_bytes()])
}

/// 300 blocks naming a list of 100 KiB would take 30 MB of copies: Apply Redactions refuses the
/// page and changes nothing.
#[test]
fn a_large_property_list_many_redacted_blocks_name_is_refused_not_copied() {
    let mut doc = blocks_naming_one_list(300, 100 << 10, "");
    mark(&mut doc, 0, &[[0.0, 200.0, 300.0, 300.0]], "");
    let before = write_full(&doc, &SaveOptions::default()).unwrap();
    assert_eq!(apply(&mut doc, None).unwrap_err(), RedactError::PropertyCopies(1));
    assert_eq!(write_full(&doc, &SaveOptions::default()).unwrap(), before, "nothing changed");
}

/// Ten blocks naming it take 1 MB: each is given a copy without the replacement text, as before.
#[test]
fn a_property_list_a_few_redacted_blocks_name_is_copied_without_its_replacement_text() {
    let mut doc = blocks_naming_one_list(10, 100 << 10, "");
    mark(&mut doc, 0, &[[0.0, 200.0, 300.0, 300.0]], "");
    let report = apply(&mut doc, None).unwrap();
    assert_eq!(report.replacement_text, 10);
    let c = content(&doc, 0);
    assert!(!c.contains("SECRET") && c.contains("PUBLIC"), "{}", &c[..c.len().min(400)]);
}

/// Remove Hidden Information refuses the same: hidden text in 300 blocks naming the list.
#[test]
fn removing_hidden_text_in_many_blocks_naming_a_large_list_is_refused() {
    let mut doc = blocks_naming_one_list(300, 100 << 10, "3 Tr");
    assert_eq!(remove_hidden(&mut doc, &[Hidden::HiddenText]).unwrap_err(), RedactError::PropertyCopies(1));
}

/// A page past the allowance whose content also holds bytes the parser can't read is refused for
/// the copies, by Remove Hidden Information as by Apply Redactions, which looks at them first.
#[test]
fn copies_past_the_allowance_beside_bytes_the_parser_cannot_read_are_refused_for_the_copies() {
    let doc = |render: &str| {
        let block = format!("/Span /M0 BDC BT /F1 10 Tf {render} 20 260 Td (S) Tj ET EMC ");
        let content = format!("{} BT /F1 10 Tf 20 20 Td [(PUBLIC) )] TJ ET", block.repeat(300));
        let list = format!("<< /ActualText (SECRET) /Junk ({}) >>", "x".repeat(100 << 10));
        one_page(content.as_bytes(), "/Properties << /M0 7 0 R >>", vec![list.into_bytes()])
    };
    let mut marked = doc("");
    mark(&mut marked, 0, &[[0.0, 200.0, 300.0, 300.0]], "");
    assert_eq!(apply(&mut marked, None).unwrap_err(), RedactError::PropertyCopies(1));
    assert_eq!(remove_hidden(&mut doc("3 Tr"), &[Hidden::HiddenText]).unwrap_err(), RedactError::PropertyCopies(1));
}

/// The copies are counted across the page's forms: the 300 blocks drawn by a form without
/// resources of its own, which names the page's list, are refused the same.
#[test]
fn copies_in_a_form_count_for_its_page() {
    let block = "/Span /M0 BDC BT /F1 10 Tf 20 260 Td (S) Tj ET EMC ".repeat(300);
    let list = format!("<< /ActualText (SECRET) /Junk ({}) >>", "x".repeat(100 << 10));
    let form = stream("/Type /XObject /Subtype /Form /BBox [0 0 300 300]", block.as_bytes());
    let mut doc = one_page(
        b"/Fm Do BT /F1 10 Tf 20 20 Td (PUBLIC) Tj ET",
        "/Properties << /M0 7 0 R >> /XObject << /Fm 8 0 R >>",
        vec![list.into_bytes(), form],
    );
    mark(&mut doc, 0, &[[0.0, 200.0, 300.0, 300.0]], "");
    assert_eq!(apply(&mut doc, None).unwrap_err(), RedactError::PropertyCopies(1));
}

/// A list given by reference in the content itself (`/Span 7 0 R BDC`, which content shouldn't
/// hold, but the parser reads) is copied into each block too, and counted the same: 300 blocks
/// giving one of 100 KiB are refused, by Apply Redactions and by Remove Hidden Information.
#[test]
fn a_property_list_given_by_reference_in_the_content_is_counted() {
    let doc = |render: &str| {
        let block = format!("/Span 7 0 R BDC BT /F1 10 Tf {render} 20 260 Td (S) Tj ET EMC ");
        let content = format!("{} BT /F1 10 Tf 20 20 Td (PUBLIC) Tj ET", block.repeat(300));
        let list = format!("<< /ActualText (SECRET) /Junk ({}) >>", "x".repeat(100 << 10));
        one_page(content.as_bytes(), "", vec![list.into_bytes()])
    };
    let mut marked = doc("");
    mark(&mut marked, 0, &[[0.0, 200.0, 300.0, 300.0]], "");
    assert_eq!(apply(&mut marked, None).unwrap_err(), RedactError::PropertyCopies(1));
    assert_eq!(remove_hidden(&mut doc("3 Tr"), &[Hidden::HiddenText]).unwrap_err(), RedactError::PropertyCopies(1));
}

/// The allowance is the run's, not each page's: two pages, each copying 100 blocks' worth of a
/// list of 100 KiB, under the allowance alone, are refused on the second.
#[test]
fn copies_are_counted_across_the_pages_of_a_run() {
    let doc = |render: &str| {
        let block = format!("/Span /M0 BDC BT /F1 10 Tf {render} 20 260 Td (S) Tj ET EMC ");
        let content = format!("{} BT /F1 10 Tf 20 20 Td (PUBLIC) Tj ET", block.repeat(100));
        let list = format!("<< /ActualText (SECRET) /Junk ({}) >>", "x".repeat(100 << 10));
        let page = "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 300 300] /Contents 4 0 R /Resources << /Font << /F1 5 0 R >> /Properties << /M0 7 0 R >> >> >>";
        pdf(vec![
            b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
            b"<< /Type /Pages /Kids [3 0 R 8 0 R] /Count 2 >>".to_vec(),
            page.as_bytes().to_vec(),
            stream("", content.as_bytes()),
            FONT.replace("95 0 R", "6 0 R").into_bytes(),
            widths(),
            list.into_bytes(),
            page.as_bytes().to_vec(),
        ])
    };
    let mut marked = doc("");
    for page in 0..2 {
        mark(&mut marked, page, &[[0.0, 200.0, 300.0, 300.0]], "");
    }
    assert_eq!(apply(&mut marked, None).unwrap_err(), RedactError::PropertyCopies(2));
    assert_eq!(remove_hidden(&mut doc("3 Tr"), &[Hidden::HiddenText]).unwrap_err(), RedactError::PropertyCopies(2));
}

/// A list given by reference is stripped once for the page, however many times a form naming it
/// is drawn, and one written in `/Properties` once for a stream however many blocks name it; and
/// one with no replacement text is never copied at all. Each copy was made again
/// for every form drawn, before the allowance was looked at.
#[test]
fn a_property_list_is_stripped_once_however_often_a_form_names_it() {
    let doc = |list: &str| {
        let form = stream(
            "/Type /XObject /Subtype /Form /BBox [0 0 300 300] /Resources << /Font << /F1 5 0 R >> /Properties << /M0 7 0 R >> >>",
            b"/Span /M0 BDC BT /F1 10 Tf 20 260 Td (S) Tj ET EMC",
        );
        let content = format!("{} BT /F1 10 Tf 20 20 Td (PUBLIC) Tj ET", "/Fm Do ".repeat(200));
        one_page(content.as_bytes(), "/XObject << /Fm 8 0 R >>", vec![list.as_bytes().to_vec(), form])
    };
    for (list, strips) in [("<< /ActualText (SECRET) /Lang (en) >>", 1), ("<< /Lang (en) >>", 0)] {
        let mut marked = doc(list);
        mark(&mut marked, 0, &[[0.0, 200.0, 300.0, 300.0]], "");
        let before = crate::interp::strips();
        apply(&mut marked, None).unwrap();
        assert_eq!(crate::interp::strips() - before, strips, "{list}");
    } // Written in the page's own `/Properties` and named by 200 blocks of one stream: once too.
    let block = "/Span /M0 BDC BT /F1 10 Tf 20 260 Td (S) Tj ET EMC ".repeat(200);
    let mut marked = one_page(block.as_bytes(), "/Properties << /M0 << /ActualText (SECRET) >> >>", vec![]);
    mark(&mut marked, 0, &[[0.0, 200.0, 300.0, 300.0]], "");
    let before = crate::interp::strips();
    apply(&mut marked, None).unwrap();
    assert_eq!(crate::interp::strips() - before, 1, "a list written in /Properties");
}

/// Once a copy would pass the allowance, nothing more is stripped: a form with a list of 6 MiB
/// written in its own `/Properties`, drawn ten times, strips it three times (two copies, then the
/// one past the allowance), not once more for each form drawn after the refusal.
#[test]
fn stripping_stops_once_the_allowance_is_spent() {
    let list = format!("<< /ActualText (SECRET) /Junk ({}) >>", "x".repeat(6 << 20));
    let form = stream(
        &format!("/Type /XObject /Subtype /Form /BBox [0 0 300 300] /Resources << /Font << /F1 5 0 R >> /Properties << /M0 {list} >> >>"),
        b"/Span /M0 BDC BT /F1 10 Tf 20 260 Td (S) Tj ET EMC",
    );
    let content = format!("{} BT /F1 10 Tf 20 20 Td (PUBLIC) Tj ET", "/Fm Do ".repeat(10));
    let mut doc = one_page(content.as_bytes(), "/XObject << /Fm 7 0 R >>", vec![form]);
    mark(&mut doc, 0, &[[0.0, 200.0, 300.0, 300.0]], "");
    let before = crate::interp::strips();
    assert_eq!(apply(&mut doc, None).unwrap_err(), RedactError::PropertyCopies(1));
    assert_eq!(crate::interp::strips() - before, 3);
}

/// An operand that is neither a name, a reference nor a list (`/Span 5 BDC`) gives no list, and
/// shares nothing with a list kept under the empty name: the block naming `/` still loses its
/// replacement text. Filed under that name, the first left the second's text in place.
#[test]
fn a_block_with_no_list_does_not_hide_the_one_named_by_the_empty_name() {
    let content = b"/Span 5 BDC BT /F1 10 Tf 20 260 Td (A) Tj ET EMC /Span / BDC BT /F1 10 Tf 20 240 Td (SECRET) Tj ET EMC";
    let mut doc = one_page(content, "/Properties << / << /ActualText (LEAK_EMPTY) >> >>", vec![]);
    mark(&mut doc, 0, &[[0.0, 200.0, 300.0, 300.0]], "");
    assert_eq!(apply(&mut doc, None).unwrap().replacement_text, 1);
    assert!(!super::surviving_references::full_save_holds(&doc, b"LEAK_EMPTY"));
}

/// The proof's snapshot strips no list: it reads nothing stripping writes, and on its own scratch
/// copy of each page it had a fresh allowance for each.
#[test]
fn the_proofs_snapshot_strips_no_property_list() {
    let mut doc = blocks_naming_one_list(10, 1 << 10, "");
    mark(&mut doc, 0, &[[0.0, 200.0, 300.0, 300.0]], "");
    let before = crate::interp::strips();
    crate::verify::Snapshot::capture(&doc, None).unwrap();
    assert_eq!(crate::interp::strips() - before, 0);
}
