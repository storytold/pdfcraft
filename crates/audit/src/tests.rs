//! Tests build their own tiny PDFs in code (contributor-original fixtures): no binary files,
//! no Adobe material, exact xref offsets computed by the builder below.

use std::sync::Arc;

use pdfcraft_cos::Document;

use crate::{AuditFinding, FindingKind, audit_redactions};

/// Minimal PDF assembler: objects are numbered 1..=n in order.
struct Builder {
    objects: Vec<Vec<u8>>,
}

impl Builder {
    fn new() -> Self {
        Builder { objects: Vec::new() }
    }

    fn add(&mut self, body: &str) -> usize {
        self.objects.push(body.as_bytes().to_vec());
        self.objects.len()
    }

    fn add_stream(&mut self, data: &[u8]) -> usize {
        self.add_stream_with(data, "")
    }

    fn add_stream_with(&mut self, data: &[u8], dict_extra: &str) -> usize {
        let mut body = format!("<< /Length {} {dict_extra} >>\nstream\n", data.len()).into_bytes();
        body.extend_from_slice(data);
        body.extend_from_slice(b"\nendstream");
        self.objects.push(body);
        self.objects.len()
    }

    fn build(self) -> Vec<u8> {
        let mut out = b"%PDF-1.7\n".to_vec();
        let mut offsets = Vec::new();
        for (i, obj) in self.objects.iter().enumerate() {
            offsets.push(out.len());
            out.extend_from_slice(format!("{} 0 obj\n", i + 1).as_bytes());
            out.extend_from_slice(obj);
            out.extend_from_slice(b"\nendobj\n");
        }
        let xref_at = out.len();
        let n = self.objects.len() + 1;
        out.extend_from_slice(format!("xref\n0 {n}\n").as_bytes());
        out.extend_from_slice(b"0000000000 65535 f \n");
        for off in offsets {
            out.extend_from_slice(format!("{off:010} 00000 n \n").as_bytes());
        }
        out.extend_from_slice(b"trailer\n<< ");
        out.extend_from_slice(format!("/Size {n} /Root 1 0 R").as_bytes());
        out.extend_from_slice(b" >>\nstartxref\n");
        out.extend_from_slice(format!("{xref_at}\n").as_bytes());
        out.extend_from_slice(b"%%EOF\n");
        out
    }
}

const FONT: &str = "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>";

/// One page with `content` as its stream; `page_extra` is appended inside the page dict.
fn one_page(content: &[u8], page_extra: &str) -> Vec<u8> {
    let mut b = Builder::new();
    b.add("<< /Type /Catalog /Pages 2 0 R >>");
    b.add("<< /Type /Pages /Kids [3 0 R] /Count 1 >>");
    let stream_num = 4;
    b.add(&format!(
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Contents {stream_num} 0 R /Resources << /Font << /F1 5 0 R >> >> {page_extra} >>"
    ));
    b.add_stream(content);
    b.add(FONT);
    b.build()
}

/// One page with two content streams (`/Contents [4 0 R 5 0 R]`).
fn two_streams(s1: &[u8], s2: &[u8]) -> Vec<u8> {
    let mut b = Builder::new();
    b.add("<< /Type /Catalog /Pages 2 0 R >>");
    b.add("<< /Type /Pages /Kids [3 0 R] /Count 1 >>");
    b.add("<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Contents [4 0 R 5 0 R] /Resources << /Font << /F1 6 0 R >> >> >>");
    b.add_stream(s1);
    b.add_stream(s2);
    b.add(FONT);
    b.build()
}

/// One page whose content stream claims `/FlateDecode` but holds garbage: decoding must fail.
fn bad_filter_page() -> Vec<u8> {
    let mut b = Builder::new();
    b.add("<< /Type /Catalog /Pages 2 0 R >>");
    b.add("<< /Type /Pages /Kids [3 0 R] /Count 1 >>");
    b.add("<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Contents 4 0 R /Resources << /Font << /F1 5 0 R >> >> >>");
    b.add_stream_with(b"this is not deflated data", "/Filter /FlateDecode");
    b.add(FONT);
    b.build()
}

fn open(bytes: Vec<u8>) -> Document {
    Document::open(Arc::new(bytes)).expect("test pdf must open")
}

#[test]
fn faux_redaction_is_found() {
    let content = b"BT /F1 24 Tf 100 700 Td (SECRET DATA 123) Tj ET\n0 g\n100 688 220 32 re f\n";
    let findings = audit_redactions(&open(one_page(content, ""))).expect("audit must run");
    assert_eq!(findings.len(), 1, "{findings:?}");
    let f = &findings[0];
    assert_eq!(f.page, 0);
    assert_eq!(f.kind, FindingKind::CoveredText);
    assert!(f.covered_text.contains("SECRET"), "covered_text was {:?}", f.covered_text);
}

#[test]
fn identity_matrix_between_rectangle_and_fill_still_covers_text() {
    // A no-op `cm` inside the path must not drop the pending rectangle (#1333).
    for cover in [&b"0 g\n100 688 220 32 re 1 0 0 1 0 0 cm f\n"[..], b"0 g\n1 0 0 1 0 0 cm 100 688 220 32 re f\n"] {
        let mut content = b"BT /F1 24 Tf 100 700 Td (SECRET DATA 123) Tj ET\n".to_vec();
        content.extend_from_slice(cover);
        let findings = audit_redactions(&open(one_page(&content, ""))).expect("audit must run");
        assert_eq!(findings.len(), 1, "{:?}: {findings:?}", String::from_utf8_lossy(cover));
        assert_eq!(findings[0].kind, FindingKind::CoveredText);
        assert_eq!(findings[0].rect, [100.0, 688.0, 320.0, 720.0]);
        assert_eq!(findings[0].covered_text, "SECRET DATA 123");
    }
}

#[test]
fn clean_document_has_no_findings() {
    let content = b"BT /F1 24 Tf 100 700 Td (nothing hidden here) Tj ET\n";
    let findings = audit_redactions(&open(one_page(content, ""))).expect("audit must run");
    assert!(findings.is_empty(), "{findings:?}");
}

#[test]
fn black_box_without_text_is_not_a_finding() {
    // A filled rectangle with no text under it is decoration, not a faux redaction.
    let content = b"BT /F1 24 Tf 100 100 Td (far away) Tj ET\n0 g\n400 600 100 40 re f\n";
    let findings = audit_redactions(&open(one_page(content, ""))).expect("audit must run");
    assert!(findings.is_empty(), "{findings:?}");
}

#[test]
fn translucent_cover_is_not_flagged() {
    // A rectangle drawn with a translucent ExtGState does not hide the text.
    let content = b"/GS1 gs\nBT /F1 24 Tf 100 700 Td (SECRET DATA 123) Tj ET\n0 g\n100 688 220 32 re f\n";
    let page_extra = "/Resources << /Font << /F1 5 0 R >> /ExtGState << /GS1 6 0 R >> >>";
    let mut b = Builder::new();
    b.add("<< /Type /Catalog /Pages 2 0 R >>");
    b.add("<< /Type /Pages /Kids [3 0 R] /Count 1 >>");
    b.add(&format!("<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Contents 4 0 R {page_extra} >>"));
    b.add_stream(content);
    b.add(FONT);
    b.add("<< /Type /ExtGState /ca 0.5 >>");
    let findings = audit_redactions(&open(b.build())).expect("audit must run");
    assert!(findings.is_empty(), "{findings:?}");
}

#[test]
fn unapplied_redact_mark_is_found() {
    let content = b"BT /F1 24 Tf 100 700 Td (DO NOT SHARE) Tj ET\n";
    let page_extra = "/Annots [6 0 R]";
    let mut b = Builder::new();
    b.add("<< /Type /Catalog /Pages 2 0 R >>");
    b.add("<< /Type /Pages /Kids [3 0 R] /Count 1 >>");
    b.add(&format!("<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Contents 4 0 R /Resources << /Font << /F1 5 0 R >> >> {page_extra} >>"));
    b.add_stream(content);
    b.add(FONT);
    b.add("<< /Type /Annot /Subtype /Redact /Rect [95 685 305 725] /C [1 0 0] >>");
    let findings = audit_redactions(&open(b.build())).expect("audit must run");
    assert_eq!(findings.len(), 1, "{findings:?}");
    assert_eq!(findings[0].kind, FindingKind::UnappliedMark);
    assert!(findings[0].covered_text.contains("DO NOT SHARE"));
}

/// #2662: a box over one number of a long line hides a few percent of the line, and all of the
/// number. Found as a cover, and as an unapplied mark over the same word.
#[test]
fn a_cover_over_one_word_of_a_long_line_is_found() {
    // Helvetica 11: "The contract value is " ends near x 132, "98765" spans about 132-163.
    let line = b"BT /F1 11 Tf 30 100 Td (The contract value is 98765 and the rest of this long line continues here.) Tj ET
";
    let mut content = line.to_vec();
    content.extend_from_slice(
        b"0 g
131 96 33 14 re f
",
    );
    let findings = audit_redactions(&open(one_page(&content, ""))).expect("audit must run");
    assert_eq!(findings.len(), 1, "{findings:?}");
    assert_eq!(findings[0].kind, FindingKind::CoveredText);
    assert!(findings[0].covered_text.contains("98765"), "{findings:?}");

    let mut b = Builder::new();
    b.add("<< /Type /Catalog /Pages 2 0 R >>");
    b.add("<< /Type /Pages /Kids [3 0 R] /Count 1 >>");
    b.add("<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Contents 4 0 R /Resources << /Font << /F1 5 0 R >> >> /Annots [6 0 R] >>");
    b.add_stream(line);
    b.add(FONT);
    b.add("<< /Type /Annot /Subtype /Redact /Rect [131 96 164 110] >>");
    let findings = audit_redactions(&open(b.build())).expect("audit must run");
    assert_eq!(findings.len(), 1, "{findings:?}");
    assert_eq!(findings[0].kind, FindingKind::UnappliedMark);
}

/// A box that only grazes the words beside it (less than half of each) is not a cover.
#[test]
fn a_box_grazing_a_word_is_not_flagged() {
    let content = b"BT /F1 11 Tf 30 100 Td (The contract value is 98765 and the rest of this long line continues here.) Tj ET
0 g
131 96 6 14 re f
";
    let findings = audit_redactions(&open(one_page(content, ""))).expect("audit must run");
    assert!(findings.is_empty(), "{findings:?}");
}

#[test]
fn opaque_filled_square_annotation_over_text_is_found() {
    let content = b"BT /F1 24 Tf 100 700 Td (SECRET DATA 123) Tj ET\n";
    let mut b = Builder::new();
    b.add("<< /Type /Catalog /Pages 2 0 R >>");
    b.add("<< /Type /Pages /Kids [3 0 R] /Count 1 >>");
    b.add("<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Contents 4 0 R /Resources << /Font << /F1 5 0 R >> >> /Annots [6 0 R] >>");
    b.add_stream(content);
    b.add(FONT);
    b.add("<< /Type /Annot /Subtype /Square /Rect [95 685 325 725] /C [0 0 0] /IC [0 0 0] >>");
    let findings = audit_redactions(&open(b.build())).expect("audit must run");
    assert_eq!(findings.len(), 1, "{findings:?}");
    assert_eq!(findings[0].kind, FindingKind::CoveredText);
    assert_eq!(findings[0].rect, [95.0, 685.0, 325.0, 725.0]);
    assert_eq!(findings[0].covered_text, "SECRET DATA 123");
}

/// A Hidden Square (`/F 2`) is never drawn, so the text under it is not a finding.
#[test]
fn hidden_square_annotation_is_not_a_finding() {
    let content = b"BT /F1 24 Tf 100 700 Td (SECRET DATA 123) Tj ET\n";
    let mut b = Builder::new();
    b.add("<< /Type /Catalog /Pages 2 0 R >>");
    b.add("<< /Type /Pages /Kids [3 0 R] /Count 1 >>");
    b.add("<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Contents 4 0 R /Resources << /Font << /F1 5 0 R >> >> /Annots [6 0 R] >>");
    b.add_stream(content);
    b.add(FONT);
    b.add("<< /Type /Annot /Subtype /Square /Rect [95 685 325 725] /C [0 0 0] /IC [0 0 0] /F 2 >>");
    let findings = audit_redactions(&open(b.build())).expect("audit must run");
    assert!(findings.is_empty(), "{findings:?}");
}

/// A NoView Square (`/F 32`) is never drawn on screen, so the text under it is not a finding.
#[test]
fn no_view_square_annotation_is_not_a_finding() {
    let content = b"BT /F1 24 Tf 100 700 Td (SECRET DATA 123) Tj ET\n";
    let mut b = Builder::new();
    b.add("<< /Type /Catalog /Pages 2 0 R >>");
    b.add("<< /Type /Pages /Kids [3 0 R] /Count 1 >>");
    b.add("<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Contents 4 0 R /Resources << /Font << /F1 5 0 R >> >> /Annots [6 0 R] >>");
    b.add_stream(content);
    b.add(FONT);
    b.add("<< /Type /Annot /Subtype /Square /Rect [95 685 325 725] /C [0 0 0] /IC [0 0 0] /F 32 >>");
    let findings = audit_redactions(&open(b.build())).expect("audit must run");
    assert!(findings.is_empty(), "{findings:?}");
}

/// A Print-only Square (`/F 4`) is still drawn on screen, so the text under it is a finding.
#[test]
fn print_flag_square_annotation_is_still_a_finding() {
    let content = b"BT /F1 24 Tf 100 700 Td (SECRET DATA 123) Tj ET\n";
    let mut b = Builder::new();
    b.add("<< /Type /Catalog /Pages 2 0 R >>");
    b.add("<< /Type /Pages /Kids [3 0 R] /Count 1 >>");
    b.add("<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Contents 4 0 R /Resources << /Font << /F1 5 0 R >> >> /Annots [6 0 R] >>");
    b.add_stream(content);
    b.add(FONT);
    b.add("<< /Type /Annot /Subtype /Square /Rect [95 685 325 725] /C [0 0 0] /IC [0 0 0] /F 4 >>");
    let findings = audit_redactions(&open(b.build())).expect("audit must run");
    assert_eq!(findings.len(), 1, "{findings:?}");
    assert_eq!(findings[0].kind, FindingKind::CoveredText);
}

#[test]
fn background_before_text_is_not_flagged() {
    // An opaque dark rectangle painted *before* the text is a background, not a cover.
    let content = b"0 g\n100 688 220 32 re f\nBT /F1 24 Tf 100 700 Td (SECRET DATA 123) Tj ET\n";
    let findings = audit_redactions(&open(one_page(content, ""))).expect("audit must run");
    assert!(findings.is_empty(), "{findings:?}");
}

#[test]
fn light_shading_after_text_is_not_flagged() {
    // Light table-style shading painted after the text leaves it readable: not a cover.
    let content = b"BT /F1 24 Tf 100 700 Td (SECRET DATA 123) Tj ET\n0.9 g\n100 688 220 32 re f\n";
    let findings = audit_redactions(&open(one_page(content, ""))).expect("audit must run");
    assert!(findings.is_empty(), "{findings:?}");
}

#[test]
fn graphics_state_carries_across_streams() {
    // The first stream sets a white fill after the text; the second stream's rectangle over
    // the text is therefore white, not the default black: no finding. Without the carry fix
    // the rectangle would default to black and flag.
    let s1 = b"BT /F1 24 Tf 100 700 Td (SECRET DATA 123) Tj ET\n1 g\n";
    let s2 = b"100 688 220 32 re f\n";
    let findings = audit_redactions(&open(two_streams(s1, s2))).expect("audit must run");
    assert!(findings.is_empty(), "{findings:?}");
}

#[test]
fn audit_fails_closed_on_undecodable_stream() {
    // A content stream that cannot be decoded is an audit error, never an empty finding list.
    let err = audit_redactions(&open(bad_filter_page())).expect_err("audit must fail closed");
    let msg = err.to_string();
    assert!(msg.contains("decoded"), "unexpected error: {msg}");
}

/// A form XObject that draws itself nests without end. The walk can't follow it to the bottom, so
/// the audit fails rather than report the page clean with a possible cover left unseen.
#[test]
fn self_drawing_form_fails_the_audit() {
    let mut b = Builder::new();
    b.add("<< /Type /Catalog /Pages 2 0 R >>");
    b.add("<< /Type /Pages /Kids [3 0 R] /Count 1 >>");
    b.add("<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Contents 4 0 R /Resources << /Font << /F1 6 0 R >> /XObject << /X 5 0 R >> >> >>");
    b.add_stream(b"BT /F1 24 Tf 100 700 Td (SECRET) Tj ET\n/X Do\n");
    b.add_stream_with(
        b"0 g\n100 688 220 32 re f\n/X Do\n",
        "/Type /XObject /Subtype /Form /BBox [0 0 612 792] /Resources << /XObject << /X 5 0 R >> >>",
    );
    b.add(FONT);
    let result = audit_redactions(&open(b.build()));
    assert!(matches!(result, Err(crate::AuditError::TooDeep)), "{result:?}");
}

/// An applied redaction leaves the words around it and a `TJ` adjustment over the gap where the
/// text was, then the redaction box. At the end of a line, the box covers most of the line's box
/// (which spans the gap) but none of its words: nothing is hidden, so nothing is flagged.
#[test]
fn applied_redaction_at_the_end_of_a_line_is_not_flagged() {
    let content = b"BT /F1 11 Tf 72 742 Td [(Mail the team:) -11354] TJ ET\n0 g\n144.138 739.8 121.814 11 re f\n";
    let findings = audit_redactions(&open(one_page(content, ""))).expect("audit must run");
    assert!(findings.is_empty(), "{findings:?}");
    // The same gap between two words, wider than the words beside it.
    let content = b"BT /F1 11 Tf 72 742 Td [(Card:) -14000 (ok)] TJ ET\n0 g\n101 739.8 150 11 re f\n";
    let findings = audit_redactions(&open(one_page(content, ""))).expect("audit must run");
    assert!(findings.is_empty(), "{findings:?}");
}

/// The control: the same box over text still drawn under it is a faux redaction, at the end of a
/// line as in the middle of a short one.
#[test]
fn faux_redaction_at_the_end_of_a_line_is_found() {
    let content = b"BT /F1 11 Tf 72 742 Td (Mail the team: someone@example.com) Tj ET\n0 g\n144.138 739.8 121.814 11 re f\n";
    let findings = audit_redactions(&open(one_page(content, ""))).expect("audit must run");
    assert_eq!(findings.len(), 1, "{findings:?}");
    assert_eq!(findings[0].covered_text, "Mail the team: someone@example.com");
    let content = b"BT /F1 11 Tf 72 742 Td [(Card:) -300 (4111111111111111) -300 (ok)] TJ ET\n0 g\n101 739.8 100 11 re f\n";
    let findings = audit_redactions(&open(one_page(content, ""))).expect("audit must run");
    assert_eq!(findings.len(), 1, "{findings:?}");
}

/// A cover over a short value after a long label hides less than 40% of the line's words, but the
/// whole of the words it covers, and most of the line's box: still a faux redaction. The value
/// follows the label in the same `TJ` after a wide gap, or in a second run moved by `Td`.
#[test]
fn faux_redaction_over_a_short_value_after_a_long_label_is_found() {
    let content = b"BT /F1 11 Tf 72 742 Td [(Social insurance number:) -8000 (123 456 789)] TJ ET\n0 g\n200 739.8 150 11 re f\n";
    let findings = audit_redactions(&open(one_page(content, ""))).expect("audit must run");
    assert_eq!(findings.len(), 1, "{findings:?}");
    assert_eq!(findings[0].covered_text, "Social insurance number: 123 456 789");
    let content = b"BT /F1 11 Tf 72 742 Td (Account:) Tj 70 0 Td (4111) Tj ET\n0 g\n115 739.8 55 11 re f\n";
    let findings = audit_redactions(&open(one_page(content, ""))).expect("audit must run");
    assert_eq!(findings.len(), 1, "{findings:?}");
    assert!(findings[0].covered_text.contains("4111"), "{findings:?}");
}

/// Text set with a negative font size in a mirrored text matrix runs its glyphs backwards: a
/// cover over the address it draws is still found.
#[test]
fn faux_redaction_over_mirrored_text_is_found() {
    let content = b"BT /F1 -11 Tf -1 0 0 1 300 742 Tm (Mail the team: someone@example.com) Tj ET\n0 g\n365 733 130 14 re f\n";
    let findings = audit_redactions(&open(one_page(content, ""))).expect("audit must run");
    assert_eq!(findings.len(), 1, "{findings:?}");
    assert_eq!(findings[0].covered_text, "Mail the team: someone@example.com");
}

/// A cover over less than half a value must retain the historical line-box detection.
#[test]
fn faux_redaction_over_a_partial_value_after_a_long_label_is_found() {
    partial_value_is_found(false);
}

#[test]
fn faux_redaction_over_a_partial_value_after_a_long_label_on_a_rotated_page_is_found() {
    partial_value_is_found(true);
}

fn partial_value_is_found(rotated: bool) {
    let bytes = one_page(
        b"BT /F1 11 Tf 72 742 Td [(Social insurance number:) -8000 (123456789)] TJ ET\n0 g\n193 738 111 16 re f\n",
        if rotated { "/Rotate 90" } else { "" },
    );
    let findings = audit_redactions(&open(bytes)).expect("audit must run");
    assert_eq!(findings.len(), 1, "partial value, rotated={rotated}");
}

/// Keep the historic detections with several covers, lines and text-spacing states.
#[test]
fn faux_redactions_with_multiple_covers_lines_and_spacing_are_found() {
    // The second box of the first case hides most of the number (less than 40% of the line): a
    // finding of its own since covers are also measured per word (#2662).
    let cases: [(&[u8], usize); 4] = [
        (b"BT /F1 11 Tf 72 742 Td (Account: 123456789) Tj ET\n0 g\n72 738 65 16 re f\n140 738 40 16 re f\n", 2),
        (b"BT /F1 11 Tf 72 742 Td (Account: 123456789) Tj 0 -15 Td (Account: 987654321) Tj ET\n0 g\n115 723 100 30 re f\n", 1),
        (b"BT /F1 11 Tf 8 Tc 72 742 Td (Account: 1234) Tj ET\n0 g\n145 738 90 16 re f\n", 1),
        (b"BT /F1 11 Tf 250 Tz 72 742 Td (Account: 1234) Tj ET\n0 g\n100 738 160 16 re f\n", 1),
    ];
    for (content, expected) in cases {
        let findings = audit_redactions(&open(one_page(content, ""))).expect("audit must run");
        assert_eq!(findings.len(), expected, "{}: {findings:?}", String::from_utf8_lossy(content));
    }
}

#[test]
fn rectangle_drawn_as_a_path_covers_text_like_re() {
    // The same cover as `re`, built from `m`/`l` (closed with `h`, a repeated start point, or
    // only by the fill itself), or in a scaled user space: all are faux redactions (#1510).
    // So are `re` with a redundant `h`, and either path painted with `b`/`b*` (#1817).
    let covers: [&[u8]; 10] = [
        b"0 g\n100 688 220 32 re f\n",
        b"0 g\n100 688 220 32 re h f\n",
        b"0 g\n100 688 220 32 re b\n",
        b"0 g\n100 688 220 32 re h b*\n",
        b"0 g\n100 688 m 320 688 l 320 720 l 100 720 l b\n",
        b"0 g\n100 688 m 320 688 l 320 720 l 100 720 l h b*\n",
        b"0 g\n100 688 m 320 688 l 320 720 l 100 720 l h f\n",
        b"0 g\n100 688 m 320 688 l 320 720 l 100 720 l 100 688 l h f\n",
        b"0 g\n100 688 m 100 720 l 320 720 l 320 688 l f\n",
        b"0 g\nq 2 0 0 2 0 0 cm 50 344 m 160 344 l 160 360 l 50 360 l h f Q\n",
    ];
    for cover in covers {
        let mut content = b"BT /F1 24 Tf 100 700 Td (SECRET DATA 123) Tj ET\n".to_vec();
        content.extend_from_slice(cover);
        let findings = audit_redactions(&open(one_page(&content, ""))).expect("audit must run");
        assert_eq!(findings.len(), 1, "{:?}: {findings:?}", String::from_utf8_lossy(cover));
        assert_eq!(findings[0].kind, FindingKind::CoveredText);
        assert_eq!(findings[0].rect, [100.0, 688.0, 320.0, 720.0]);
        assert_eq!(findings[0].covered_text, "SECRET DATA 123");
    }
}

#[test]
fn non_rectangular_path_is_not_a_cover() {
    // Each path's bounding box covers the text, but the shape itself does not hide it all.
    let covers: [&[u8]; 7] = [
        // A triangle.
        b"0 g\n100 688 m 320 688 l 320 720 l h f\n",
        // A triangle painted with `b`, and a trapezoid with `b*`.
        b"0 g\n100 688 m 320 688 l 320 720 l b\n",
        b"0 g\n100 688 m 320 688 l 320 720 l 140 720 l b*\n",
        // A triangle, then a line from its start point after `h`: not a fourth corner.
        b"0 g\n100 688 m 320 688 l 320 720 l h 100 720 l f\n",
        // A trapezoid: four corners, one slanted side.
        b"0 g\n100 688 m 320 688 l 320 720 l 140 720 l h f\n",
        // A rectangle rotated 5 degrees about the text by the CTM.
        b"0 g\nq 0.9962 0.0872 -0.0872 0.9962 62.15 -15.63 cm 100 688 m 320 688 l 320 720 l 100 720 l h f Q\n",
        // Two subpaths filled together: even-odd leaves the inner one as a hole.
        b"0 g\n90 680 m 330 680 l 330 730 l 90 730 l h 100 688 m 320 688 l 320 720 l 100 720 l h f*\n",
    ];
    for cover in covers {
        let mut content = b"BT /F1 24 Tf 100 700 Td (SECRET DATA 123) Tj ET\n".to_vec();
        content.extend_from_slice(cover);
        let findings = audit_redactions(&open(one_page(&content, ""))).expect("audit must run");
        assert!(findings.is_empty(), "{:?}: {findings:?}", String::from_utf8_lossy(cover));
    }
}

#[test]
fn finding_kind_ids_are_stable() {
    // The string IDs are the crate's public vocabulary (CLI output, future machine-readable
    // formats). They must not change; add a variant instead of renaming.
    assert_eq!(FindingKind::CoveredText.id(), "covered-text");
    assert_eq!(FindingKind::UnappliedMark.id(), "unapplied-mark");
}

#[test]
fn covered_text_finding_is_fully_determined() {
    // Golden test: the whole finding, not just its presence, is pinned.
    let content = b"BT /F1 24 Tf 100 700 Td (SECRET DATA 123) Tj ET\n0 g\n100 688 220 32 re f\n";
    let findings = audit_redactions(&open(one_page(content, ""))).expect("audit must run");
    assert_eq!(
        findings,
        vec![AuditFinding {
            page: 0,
            kind: FindingKind::CoveredText,
            rect: [100.0, 688.0, 320.0, 720.0],
            covered_text: "SECRET DATA 123".to_string(),
        }]
    );
}

#[test]
fn unapplied_mark_finding_is_fully_determined() {
    // Golden test for the second finding class, with its exact mark rectangle.
    let content = b"BT /F1 24 Tf 100 700 Td (DO NOT SHARE) Tj ET\n";
    let page_extra = "/Annots [6 0 R]";
    let mut b = Builder::new();
    b.add("<< /Type /Catalog /Pages 2 0 R >>");
    b.add("<< /Type /Pages /Kids [3 0 R] /Count 1 >>");
    b.add(&format!("<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Contents 4 0 R /Resources << /Font << /F1 5 0 R >> >> {page_extra} >>"));
    b.add_stream(content);
    b.add(FONT);
    b.add("<< /Type /Annot /Subtype /Redact /Rect [95 685 305 725] /C [1 0 0] >>");
    let findings = audit_redactions(&open(b.build())).expect("audit must run");
    assert_eq!(
        findings,
        vec![AuditFinding { page: 0, kind: FindingKind::UnappliedMark, rect: [95.0, 685.0, 305.0, 725.0], covered_text: "DO NOT SHARE".to_string() }]
    );
}

/// A page whose content draws a chain of `n` nested form XObjects. The innermost form draws
/// nothing, so a successful audit reports no findings.
fn nested_forms(n: usize) -> Vec<u8> {
    let mut b = Builder::new();
    b.add("<< /Type /Catalog /Pages 2 0 R >>");
    b.add("<< /Type /Pages /Kids [3 0 R] /Count 1 >>");
    // Object 3: page; object 4: content; objects 5..5+n: the form chain.
    b.add("<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Contents 4 0 R /Resources << /XObject << /F1 5 0 R >> >> >>");
    b.add_stream(b"/F1 Do");
    for i in 1..=n {
        let obj = 4 + i;
        if i < n {
            let next = obj + 1;
            let name = format!("/F{}", i + 1);
            b.add_stream_with(
                format!("/{name} Do", name = &name[1..]).as_bytes(),
                &format!(
                    "/Type /XObject /Subtype /Form /BBox [0 0 612 792] /Resources << /XObject << /F{i} {next} 0 R >> >>",
                    i = i + 1,
                    next = next
                ),
            );
        } else {
            b.add_stream_with(b"q Q", "/Type /XObject /Subtype /Form /BBox [0 0 612 792]");
        }
        debug_assert_eq!(b.objects.len(), obj);
    }
    b.build()
}

#[test]
fn forms_nested_at_depth_limit_audit_clean() {
    // 32 nested forms is the deepest walk the audit follows: it must succeed and report
    // the page clean rather than fail.
    let findings = audit_redactions(&open(nested_forms(32))).expect("audit must run");
    assert!(findings.is_empty(), "{findings:?}");
}

#[test]
fn forms_nested_past_depth_limit_fail_closed() {
    // The 33rd nesting level cannot be followed: the audit fails rather than report the
    // page clean with a possible cover left unseen.
    let result = audit_redactions(&open(nested_forms(33)));
    assert!(matches!(result, Err(crate::AuditError::TooDeep)), "{result:?}");
}

#[test]
fn comment_across_streams_neither_invents_text_nor_a_cover() {
    let doc = open(two_streams(b"BT /F1 24 Tf 72 700 Td (Visible text) Tj ET % note", b"0 g 70 690 200 40 re f"));
    assert!(audit_redactions(&doc).unwrap().is_empty(), "commented cover must not be reported");
    let doc = open(two_streams(b"BT /F1 24 Tf 72 700 Td (Before comment) Tj ET % note", b"BT /F1 24 Tf 72 650 Td (After comment) Tj ET"));
    let text: String = pdfcraft_edit::text_lines(&doc, 0).unwrap().into_iter().map(|line| line.text).collect();
    assert!(!text.contains("After comment"), "editor exposed phantom text: {text}");
}

#[test]
fn reading_blocks_respect_stream_comments_and_split_operands() {
    let doc = open(two_streams(b"BT /F1 24 Tf 72 700 Td (Before comment) Tj ET % note", b"BT /F1 24 Tf 72 650 Td (After comment) Tj ET"));
    let text: String = pdfcraft_edit::reading_blocks(&doc, 0).unwrap().into_iter().map(|block| block.editor_text).collect();
    assert_eq!(text, "Before comment");
    let doc = open(two_streams(b"BT /F1 24 Tf 72 700 Td (Split operand)", b"Tj ET"));
    let text: String = pdfcraft_edit::reading_blocks(&doc, 0).unwrap().into_iter().map(|block| block.editor_text).collect();
    assert_eq!(text, "Split operand");
    // Marked content opened in one stream applies to text in the next: its /ActualText replaces
    // the glyphs, as when the streams are read joined.
    for (s1, s2) in [
        (b"BT /F1 24 Tf 72 700 Td /Span <</ActualText (Replacement text)".as_slice(), b">> BDC (Glyphs) Tj EMC ET".as_slice()),
        (b"BT /F1 24 Tf 72 700 Td /Span <</ActualText (Replacement text)>> BDC", b"(Glyphs) Tj EMC ET"),
    ] {
        let doc = open(two_streams(s1, s2));
        let text: String = pdfcraft_edit::reading_blocks(&doc, 0).unwrap().into_iter().map(|block| block.editor_text).collect();
        assert_eq!(text, "Replacement text", "{}", String::from_utf8_lossy(s1));
    }
}

/// The current path is not graphics state: a rectangle built before `Q` and filled after it
/// paints the box, so it is a cover.
#[test]
fn rectangle_filled_after_restore_is_a_cover() {
    let content = b"BT /F1 24 Tf 100 700 Td (SECRET DATA 123) Tj ET\n0 g\nq 100 688 220 32 re Q f\n";
    let findings = audit_redactions(&open(one_page(content, ""))).expect("audit must run");
    assert_eq!(findings.len(), 1, "{findings:?}");
    assert!(findings[0].covered_text.contains("SECRET"), "{findings:?}");
}

/// A `/Subtype` may be an indirect reference to the name, like any other value: a `/Redact`
/// annotation and a covering form XObject named that way are still checked.
#[test]
fn indirect_subtype_is_resolved() {
    let mut b = Builder::new();
    b.add("<< /Type /Catalog /Pages 2 0 R >>");
    b.add("<< /Type /Pages /Kids [3 0 R] /Count 1 >>");
    b.add("<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Contents 4 0 R /Resources << /Font << /F1 5 0 R >> >> /Annots [6 0 R] >>");
    b.add_stream(b"BT /F1 24 Tf 100 700 Td (DO NOT SHARE) Tj ET\n");
    b.add(FONT);
    b.add("<< /Type /Annot /Subtype 7 0 R /Rect [95 685 305 725] >>");
    b.add("/Redact");
    let findings = audit_redactions(&open(b.build())).expect("audit must run");
    assert_eq!(findings.len(), 1, "{findings:?}");
    assert_eq!(findings[0].kind, FindingKind::UnappliedMark);

    let mut b = Builder::new();
    b.add("<< /Type /Catalog /Pages 2 0 R >>");
    b.add("<< /Type /Pages /Kids [3 0 R] /Count 1 >>");
    b.add("<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Contents 4 0 R /Resources << /Font << /F1 6 0 R >> /XObject << /X 5 0 R >> >> >>");
    b.add_stream(b"BT /F1 24 Tf 100 700 Td (SECRET) Tj ET\n/X Do\n");
    b.add_stream_with(b"0 g\n100 688 220 32 re f\n", "/Type /XObject /Subtype 7 0 R /BBox [0 0 612 792]");
    b.add(FONT);
    b.add("/Form");
    let findings = audit_redactions(&open(b.build())).expect("audit must run");
    assert_eq!(findings.len(), 1, "{findings:?}");
    assert_eq!(findings[0].kind, FindingKind::CoveredText);
}

/// Text drawn after a cover on the same baseline joins the covered text in one line. The cover
/// still hides the words painted before it.
#[test]
fn later_text_on_the_line_does_not_clear_an_earlier_cover() {
    let content = b"0 g\nBT /F1 24 Tf 100 700 Td (SECRET) Tj ET\n98 688 94 32 re f\nBT /F1 24 Tf 200 700 Td (tail) Tj ET\n";
    let doc = open(one_page(content, ""));
    let lines = pdfcraft_edit::text_lines(&doc, 0).expect("text");
    assert_eq!(lines.len(), 1, "the runs join one line: {:?}", lines.iter().map(|l| &l.text).collect::<Vec<_>>());
    let findings = audit_redactions(&doc).expect("audit must run");
    assert_eq!(findings.len(), 1, "{findings:?}");
    assert!(findings[0].covered_text.contains("SECRET"), "{findings:?}");
}

/// #4164: a cover over part of a line reports the words under it, not the words of the same
/// line painted beside it, which stay visible.
#[test]
fn covered_text_leaves_out_the_words_beside_the_cover() {
    let covered = |content: &[u8]| {
        let findings = audit_redactions(&open(one_page(content, ""))).expect("audit must run");
        findings.into_iter().map(|f| f.covered_text).collect::<Vec<_>>()
    };
    assert_eq!(covered(b"0 g\nBT /F1 24 Tf 100 700 Td (SECRET) Tj ET\n98 688 94 32 re f\nBT /F1 24 Tf 200 700 Td (tail) Tj ET\n"), ["SECRET"]);
    // A word shown in two runs with nothing between them stays one word (review on #4198).
    assert_eq!(covered(b"0 g\nBT /F1 24 Tf 100 700 Td (SE) Tj (CRET) Tj ET\n98 688 94 32 re f\nBT /F1 24 Tf 200 700 Td (tail) Tj ET\n"), ["SECRET"]);

    // The same for an unapplied redaction mark over the first run only. A mark over both runs
    // reports both, with the space the line has between them (`tail` is set apart here).
    for (rect, expected) in [("[98 688 192 720]", "SECRET"), ("[98 688 310 720]", "SECRET tail")] {
        let mut b = Builder::new();
        b.add("<< /Type /Catalog /Pages 2 0 R >>");
        b.add("<< /Type /Pages /Kids [3 0 R] /Count 1 >>");
        b.add("<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Contents 4 0 R /Resources << /Font << /F1 5 0 R >> >> /Annots [6 0 R] >>");
        b.add_stream(b"BT /F1 24 Tf 100 700 Td (SECRET) Tj ET\nBT /F1 24 Tf 250 700 Td (tail) Tj ET\n");
        b.add(FONT);
        b.add(&format!("<< /Type /Annot /Subtype /Redact /Rect {rect} >>"));
        let findings = audit_redactions(&open(b.build())).expect("audit must run");
        assert_eq!(findings.iter().map(|f| (f.kind.clone(), f.covered_text.as_str())).collect::<Vec<_>>(), [(FindingKind::UnappliedMark, expected)]);
    }
}

/// Many covers over the same long line repeat its text in every finding: a small file must not
/// expand into an unbounded report. Past the budget the audit fails rather than truncate.
#[test]
fn covered_text_across_findings_is_bounded() {
    let text = "X".repeat(60_000);
    let mut content = format!("BT /F1 1 Tf 0 700 Td ({text}) Tj ET\n0 g\n").into_bytes();
    for i in 0..1_200 {
        content.extend_from_slice(format!("-{i} 690 50000 20 re f\n").as_bytes());
    }
    let result = audit_redactions(&open(one_page(&content, "")));
    assert!(matches!(result, Err(crate::AuditError::TooMuchOutput)), "{:?}", result.map(|f| f.len()));
}

/// Covers compared against lines are bounded per page, so content that is quadratic in its
/// covers and lines fails the audit instead of running for hours. A line costs one comparison per
/// word: 20 covers against 20 two-word lines are 800.
#[test]
fn cover_and_line_comparisons_are_bounded() {
    let mut content = Vec::new();
    for i in 0..20 {
        content.extend_from_slice(format!("BT /F1 12 Tf 72 {} Td (Line {i}) Tj ET\n", 700 - 14 * i).as_bytes());
    }
    for _ in 0..20 {
        content.extend_from_slice(b"1 g 0 0 10 10 re f\n");
    }
    let doc = open(one_page(&content, ""));
    let limits = crate::Limits { comparisons: 799, covered_text: usize::MAX };
    assert!(matches!(crate::audit(&doc, limits), Err(crate::AuditError::TooManyComparisons)));
    let limits = crate::Limits { comparisons: 800, covered_text: usize::MAX };
    assert!(crate::audit(&doc, limits).expect("within the budget").is_empty());
}

/// A run that draws no word still costs a comparison: the covered text walks every run of a
/// covered line, so a word followed by many `( ) Tj` must not be checked for free (review on
/// #4198).
#[test]
fn runs_without_words_count_against_the_comparison_bound() {
    let mut content = b"BT /F1 24 Tf 100 700 Td (SECRET) Tj".to_vec();
    content.extend_from_slice(&b" ( ) Tj".repeat(50));
    content.extend_from_slice(
        b" ET
0 g
98 688 94 32 re f
",
    );
    let doc = open(one_page(&content, ""));
    let lines = pdfcraft_edit::glyph_text_lines(&doc, &pdfcraft_model::pages(&doc)[0]).expect("text");
    assert_eq!(lines.iter().map(|l| (l.ink.len(), l.runs().count())).collect::<Vec<_>>(), [(1, 51)]);
    let limits = crate::Limits { comparisons: 50, covered_text: usize::MAX };
    assert!(matches!(crate::audit(&doc, limits), Err(crate::AuditError::TooManyComparisons)));
    let limits = crate::Limits { comparisons: 51, covered_text: usize::MAX };
    assert_eq!(crate::audit(&doc, limits).expect("within the budget").len(), 1);
}

/// Covered text is document text: a document whose permissions withhold copying reports where
/// its covers are but not what they hide, unless it was opened with the owner password.
#[test]
fn copy_restricted_documents_withhold_covered_text() {
    let content = b"BT /F1 24 Tf 100 700 Td (SECRET DATA 123) Tj ET\n0 g\n100 688 220 32 re f\n";
    let mut doc = open(one_page(content, ""));
    doc.set_encryption(&pdfcraft_cos::NewEncryption {
        algorithm: pdfcraft_cos::Algorithm::Aes256,
        user_password: "",
        owner_password: "owner",
        permissions: -1 ^ 0b1_0000,
        encrypt_metadata: true,
        seed: [7; 32],
    })
    .expect("encrypt");
    let bytes = Arc::new(pdfcraft_cos::write_full(&doc, &Default::default()).expect("save"));
    let user = Document::open_with_password(bytes.clone(), None).expect("open");
    let findings = audit_redactions(&user).expect("audit must run");
    assert_eq!(findings.len(), 1, "the cover is still reported: {findings:?}");
    assert_eq!(findings[0].covered_text, "");
    // The output bound counts the text reported, so withheld text costs nothing.
    let nothing = crate::Limits { comparisons: usize::MAX, covered_text: 0 };
    assert_eq!(crate::audit(&user, nothing).expect("withheld text is not output").len(), 1);
    let owner = Document::open_with_password(bytes, Some("owner")).expect("open");
    let findings = audit_redactions(&owner).expect("audit must run");
    assert_eq!(findings.len(), 1, "{findings:?}");
    assert_eq!(findings[0].covered_text, "SECRET DATA 123");
    let exact = crate::Limits { comparisons: usize::MAX, covered_text: "SECRET DATA 123".len() };
    assert!(crate::audit(&owner, exact).is_ok());
    let short = crate::Limits { covered_text: exact.covered_text - 1, ..exact };
    assert!(matches!(crate::audit(&owner, short), Err(crate::AuditError::TooMuchOutput)));
}

/// Withheld text is never assembled: covers over one long line would otherwise copy the line once
/// per cover, a volume neither the comparison nor the output bound charges.
#[test]
fn withheld_covered_text_is_not_assembled() {
    const PAGE: [f64; 4] = [0.0, 0.0, 612.0, 792.0];
    let doc = open(one_page(b"BT /F1 24 Tf 100 700 Td (SECRET DATA 123) Tj ET\n", ""));
    let lines = pdfcraft_edit::text_lines(&doc, 0).expect("text");
    let mut budget = crate::Budget { limits: crate::Limits::DEFAULT, comparisons: 0, page_cost: 0, covered_text: 0 };
    budget.start_page(&lines);
    let withheld = budget.hidden_text(&lines, false, &PAGE, |_| true).expect("within the budget");
    assert_eq!(withheld.map(|t| t.capacity()), Some(0), "a finding, with no text built for it");
    assert_eq!(budget.hidden_text(&lines, false, &PAGE, |_| false).expect("within the budget"), None);
    let copied = budget.hidden_text(&lines, true, &PAGE, |_| true).expect("within the budget");
    assert_eq!(copied.as_deref(), Some("SECRET DATA 123"));
}

/// `/ActualText` is metadata the file's author controls: an empty or substitute replacement must
/// not hide the glyphs a cover sits on.
#[test]
fn a_cover_over_text_with_empty_or_substitute_actual_text_is_found() {
    for actual in ["()", "(nothing here)"] {
        let content =
            format!("/Span << /ActualText {actual} >> BDC\nBT /F1 24 Tf 100 700 Td (SECRET DATA 123) Tj ET\nEMC\n0 g\n100 688 220 32 re f\n");
        let findings = audit_redactions(&open(one_page(content.as_bytes(), ""))).expect("audit must run");
        assert_eq!(findings.len(), 1, "{actual}: {findings:?}");
        assert!(findings[0].covered_text.contains("SECRET"), "{actual}: covered_text was {:?}", findings[0].covered_text);
    }
}

#[test]
fn repeated_stream_references_share_one_decode_budget() {
    use pdfcraft_cos::{Dict, Document, Object, Stream};
    // One 1 MiB stream of spaces (a kilobyte on disk), named 300 times: by the page's /Contents,
    // and by 300 draws of a form holding it.
    let mut doc = Document::new_empty();
    let spaces = doc.add(Stream::flate(Dict::new(), &vec![b' '; 1 << 20]));
    let contents: Dict = [(b"Contents".to_vec(), Object::Array(vec![Object::Ref(spaces); 300]))].into_iter().collect();
    assert!(crate::cover::filled_rects(&doc, &contents).is_err(), "repeated content streams");

    let form_dict: Dict = [(b"Subtype".to_vec(), Object::Name(b"Form".to_vec()))].into_iter().collect();
    let form = doc.add(Stream::flate(form_dict, &vec![b' '; 1 << 20]));
    let draws = doc.add(Stream::from_raw(Dict::new(), "/X Do ".repeat(300).into_bytes()));
    let xobjects: Dict = [(b"X".to_vec(), Object::Ref(form))].into_iter().collect();
    let resources: Dict = [(b"XObject".to_vec(), Object::Dict(xobjects))].into_iter().collect();
    let page: Dict = [(b"Contents".to_vec(), Object::Ref(draws)), (b"Resources".to_vec(), Object::Dict(resources))].into_iter().collect();
    assert!(crate::cover::filled_rects(&doc, &page).is_err(), "repeated form draws");
}

#[test]
fn form_entries_past_the_page_allowance_fail_the_audit() {
    use pdfcraft_cos::{Dict, Document, Object, Stream};
    // An empty form decodes to no bytes, so only the count of entries bounds drawing it again.
    let mut doc = Document::new_empty();
    let form_dict: Dict = [(b"Subtype".to_vec(), Object::Name(b"Form".to_vec()))].into_iter().collect();
    let form = doc.add(Stream::from_raw(form_dict, Vec::new()));
    let draws = doc.add(Stream::from_raw(Dict::new(), "/X Do ".repeat(pdfcraft_cos::MAX_PAGE_FORMS + 1).into_bytes()));
    let xobjects: Dict = [(b"X".to_vec(), Object::Ref(form))].into_iter().collect();
    let resources: Dict = [(b"XObject".to_vec(), Object::Dict(xobjects))].into_iter().collect();
    let page: Dict = [(b"Contents".to_vec(), Object::Ref(draws)), (b"Resources".to_vec(), Object::Dict(resources))].into_iter().collect();
    assert!(matches!(crate::cover::filled_rects(&doc, &page), Err(crate::AuditError::TooManyForms)));
}

const IMAGE_TEXT: &[u8] = b"BT /F1 24 Tf 72 700 Td (Hello secret world) Tj ET\n";
const IMAGE_DRAW: &[u8] = b"q 72 0 0 28 130 692 cm /Im Do Q\n";

/// Build a synthetic Letter page with text, an image XObject, reusable graphics states and
/// optional extra streams for forms or soft masks, using stable object numbers.
fn image_page(content: &[u8], image: &[u8], image_dict: &str, extras: &[(&[u8], &str)]) -> Vec<u8> {
    let mut b = Builder::new();
    b.add("<< /Type /Catalog /Pages 2 0 R >>");
    b.add("<< /Type /Pages /Kids [3 0 R] /Count 1 >>");
    b.add("<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Contents 4 0 R /Resources << /Font << /F1 5 0 R >> /XObject << /Im 6 0 R /Fm 7 0 R >> /ExtGState << /Half << /ca 0.5 >> /Empty << /ca 0 >> /Other << /CA 0.5 >> /Soft << /SMask << /S /Alpha >> >> /Normal << /SMask /None /BM /Normal >> /Screen << /BM /Screen >> /Over << /op true >> /NoOver << /op false >> >> >> >>");
    b.add_stream(content);
    b.add(FONT);
    b.add_stream_with(image, &format!("/Type /XObject /Subtype /Image {image_dict}"));
    for &(data, dict) in extras {
        b.add_stream_with(data, dict);
    }
    b.build()
}
/// Build an image-audit page whose image is one 8-bit DeviceGray sample.
fn gray_image(content: &[u8], sample: u8) -> Vec<u8> {
    image_page(content, &[sample], "/Width 1 /Height 1 /BitsPerComponent 8 /ColorSpace /DeviceGray", &[])
}
/// Concatenate content fragments without inserting separators; callers supply token boundaries.
fn joined(parts: &[&[u8]]) -> Vec<u8> {
    parts.concat()
}

/// Image XObjects, stencil masks and inline images from #3117 are reported as covers while
/// the hidden text remains extractable, with the expected user-space cover rectangle.
#[test]
fn image_covers_from_issue_3117_are_found() {
    let content = joined(&[IMAGE_TEXT, IMAGE_DRAW]);
    for bytes in [
        gray_image(&content, 0),
        image_page(&content, &[0], "/Width 1 /Height 1 /ImageMask true /BitsPerComponent 1", &[]),
        one_page(&joined(&[IMAGE_TEXT, b"q 72 0 0 28 130 692 cm BI /W 1 /H 1 /CS /G /BPC 8 ID \x00 EI Q"]), ""),
    ] {
        let doc = open(bytes);
        assert!(pdfcraft_edit::text_lines(&doc, 0).unwrap().iter().any(|l| l.text.contains("secret")));
        let f = audit_redactions(&doc).unwrap();
        assert_eq!(f.len(), 1, "{f:?}");
        assert!(f[0].covered_text.contains("secret"));
        assert_eq!(f[0].rect, [130.0, 692.0, 202.0, 720.0]);
    }
}
/// Images behind text, high-contrast white samples and unpainted stencil bits are not covers.
#[test]
fn image_backgrounds_white_samples_and_empty_stencils_are_not_covers() {
    let content = joined(&[IMAGE_TEXT, IMAGE_DRAW]);
    for bytes in [
        gray_image(&joined(&[IMAGE_DRAW, IMAGE_TEXT]), 0),
        gray_image(&content, 255),
        image_page(&content, &[255], "/Width 1 /Height 1 /ImageMask true /BitsPerComponent 1", &[]),
    ] {
        assert!(audit_redactions(&open(bytes)).unwrap().is_empty());
    }
}
/// Stencil Decode polarity selects painted bits and the current fill determines contrast;
/// ordinary image samples retain their own colour regardless of the current fill.
#[test]
fn image_stencil_decode_and_fill_are_respected() {
    let content = joined(&[IMAGE_TEXT, IMAGE_DRAW]);
    for (sample, decode, expected) in [(0, "[0 1]", 1), (255, "[0 1]", 0), (0, "[1 0]", 0), (255, "[1 0]", 1)] {
        let bytes = image_page(&content, &[sample], &format!("/Width 1 /Height 1 /ImageMask true /BitsPerComponent 1 /Decode {decode}"), &[]);
        assert_eq!(audit_redactions(&open(bytes)).unwrap().len(), expected);
    }
    let content = joined(&[IMAGE_TEXT, b"1 g\n", IMAGE_DRAW]);
    let bytes = image_page(&content, &[0], "/Width 1 /Height 1 /ImageMask true /BitsPerComponent 1", &[]);
    assert!(audit_redactions(&open(bytes)).unwrap().is_empty());
    // Ordinary images ignore the current nonstroking colour.
    assert_eq!(audit_redactions(&open(gray_image(&content, 0))).unwrap().len(), 1);
}
/// Reflection, rotation, composed transforms and shear use actual image polygons, so a thin
/// diagonal whose bounding box overlaps text does not become a false cover.
#[test]
fn image_ctm_and_exact_rotated_geometry_are_respected() {
    for draw in
        [b"q -72 0 0 28 202 692 cm /Im Do Q".as_slice(), b"q 0 28 -72 0 202 692 cm /Im Do Q", b"q 1 0 0 1 130 692 cm 72 0 0 28 0 0 cm /Im Do Q"]
    {
        assert_eq!(audit_redactions(&open(gray_image(&joined(&[IMAGE_TEXT, draw]), 0))).unwrap().len(), 1);
    }
    // A thin diagonal's bounding box covers the word, its actual painted area does not.
    let draw = b"q 72 28 -0.1 0.1 130 692 cm /Im Do Q";
    assert!(audit_redactions(&open(gray_image(&joined(&[IMAGE_TEXT, draw]), 0))).unwrap().is_empty());
    // A sheared image that really covers the word is still detected.
    let draw = b"q 100 0 10 40 120 685 cm /Im Do Q";
    assert_eq!(audit_redactions(&open(gray_image(&joined(&[IMAGE_TEXT, draw]), 0))).unwrap().len(), 1);
}
/// Image detection intersects active and nested clips and restores them with `Q`, retaining
/// real overlaps and discarding clipped-out or insufficient painted coverage.
#[test]
fn image_clips_apply_only_to_images_and_restore_with_q() {
    for clip in [b"0 0 10 10 re W n\n".as_slice(), b"130 692 1 28 re W* n\n", b"0 0 10 10 re W n 100 100 10 10 re W n\n"] {
        let content = joined(&[IMAGE_TEXT, b"q ", clip, IMAGE_DRAW, b" Q"]);
        assert!(audit_redactions(&open(gray_image(&content, 0))).unwrap().is_empty());
    }
    let content = joined(&[IMAGE_TEXT, b"q 0 0 10 10 re W n Q\n", IMAGE_DRAW]);
    assert_eq!(audit_redactions(&open(gray_image(&content, 0))).unwrap().len(), 1);
    let content = joined(&[IMAGE_TEXT, b"q 130 692 72 28 re W n\n", IMAGE_DRAW, b"Q"]);
    assert_eq!(audit_redactions(&open(gray_image(&content, 0))).unwrap().len(), 1);
}
/// Nonstroking alpha survives ExtGStates that omit it and is restored by `Q`; transparent
/// and translucent images follow the existing policy of not counting as opaque covers.
#[test]
fn image_alpha_retains_unspecified_extgstate_entries() {
    for prefix in [b"/Half gs ".as_slice(), b"/Empty gs ", b"/Half gs /Other gs "] {
        assert!(audit_redactions(&open(gray_image(&joined(&[IMAGE_TEXT, prefix, IMAGE_DRAW]), 0))).unwrap().is_empty());
    }
    let content = joined(&[IMAGE_TEXT, b"q /Half gs Q ", IMAGE_DRAW]);
    assert_eq!(audit_redactions(&open(gray_image(&content, 0))).unwrap().len(), 1);
}
/// Images reached through forms obey the form BBox and matrix and can cover earlier page text.
#[test]
fn image_form_matrix_bbox_and_paint_order() {
    let draw = b"q 72 0 0 28 130 692 cm /Im Do Q";
    for (bbox, expected) in [("[0 0 10 10]", 0), ("[130 692 202 720]", 1)] {
        // Forms must have their own resources; Im references the page's image object.
        let form = format!("/Subtype /Form /BBox {bbox} /Resources << /XObject << /Im 6 0 R >> >>");
        let content = joined(&[IMAGE_TEXT, b"/Fm Do"]);
        let bytes = image_page(&content, &[0], "/Width 1 /Height 1 /ColorSpace /DeviceGray /BitsPerComponent 8", &[(draw, &form)]);
        assert_eq!(audit_redactions(&open(bytes)).unwrap().len(), expected);
    }
    let form = "/Subtype /Form /BBox [0 0 1 1] /Matrix [72 0 0 28 130 692] /Resources << /XObject << /Im 6 0 R >> >>";
    let bytes =
        image_page(&joined(&[IMAGE_TEXT, b"/Fm Do"]), &[0], "/Width 1 /Height 1 /ColorSpace /DeviceGray /BitsPerComponent 8", &[(b"/Im Do", form)]);
    assert_eq!(audit_redactions(&open(bytes)).unwrap().len(), 1);
}
/// Sample rows run top to bottom, and only fully opaque soft-mask samples contribute area;
/// coverage of the upper and lower halves is measured against the word's actual position.
#[test]
fn image_sample_rows_and_partial_soft_masks() {
    let content = joined(&[IMAGE_TEXT, IMAGE_DRAW]);
    let image = "/Width 1 /Height 2 /ColorSpace /DeviceGray /BitsPerComponent 8 /SMask 7 0 R";
    let mask = "/Subtype /Image /Width 1 /Height 2 /ColorSpace /DeviceGray /BitsPerComponent 8";
    for (alpha, expected) in [([255, 255], 1), ([0, 0], 0), ([128, 128], 0), ([0, 255], 0)] {
        let bytes = image_page(&content, &[0, 0], image, &[(&alpha, mask)]);
        assert_eq!(audit_redactions(&open(bytes)).unwrap().len(), expected);
    }
    // Top half is black. It covers enough of the word; the bottom half alone does not.
    for (data, expected) in [([0, 255], 1), ([255, 0], 0)] {
        let bytes = image_page(&content, &data, "/Width 1 /Height 2 /ColorSpace /DeviceGray /BitsPerComponent 8", &[]);
        assert_eq!(audit_redactions(&open(bytes)).unwrap().len(), expected);
    }
}
/// Unsupported formats, excessive or truncated samples, complex clips, transparency and
/// optional content fail the audit instead of producing an incomplete clean result.
#[test]
fn image_incomplete_analysis_is_an_error() {
    let content = joined(&[IMAGE_TEXT, IMAGE_DRAW]);
    for dict in [
        "/Width 1 /Height 1 /ColorSpace /CalGray /BitsPerComponent 8",
        "/Width 1 /Height 1 /ColorSpace /DeviceGray /BitsPerComponent 8 /Filter /DCTDecode",
        "/Width 1000001 /Height 1 /ColorSpace /DeviceGray /BitsPerComponent 8",
        "/Width 2 /Height 1 /ColorSpace /DeviceGray /BitsPerComponent 8",
        "/Width 1 /Height 1 /ColorSpace /DeviceGray /BitsPerComponent 8 /Interpolate true",
        "/Width 1 /Height 1 /ColorSpace /DeviceGray /BitsPerComponent 8 /Mask [0 0]",
    ] {
        assert!(audit_redactions(&open(image_page(&content, &[0], dict, &[]))).is_err(), "{dict}");
    }
    for prefix in [b"0 0 m 10 10 l W n ".as_slice(), b"/Soft gs ", b"/OC /Hidden BDC "] {
        assert!(audit_redactions(&open(gray_image(&joined(&[IMAGE_TEXT, prefix, IMAGE_DRAW]), 0))).is_err());
    }
    let bytes = one_page(b"BI /W 1 /H 1 /BPC 8 /CS /G ID truncated", "");
    assert!(audit_redactions(&open(bytes)).is_err());
}

/// Inline abbreviations and lossless filters decode correctly alongside packed samples,
/// inverted Decode arrays and supported DeviceRGB/DeviceCMYK image colours.
#[test]
fn image_inline_filters_decode_and_packed_samples() {
    for image in [b"BI /W 1 /H 1 /CS /G /BPC 8 /F /AHx ID 00> EI".as_slice(), b"BI /W 1 /H 1 /IM true /BPC 1 /D [1 0] ID \xff EI"] {
        let content = joined(&[IMAGE_TEXT, b"q 72 0 0 28 130 692 cm ", image, b" Q"]);
        assert_eq!(audit_redactions(&open(one_page(&content, ""))).unwrap().len(), 1);
    }
    let content = joined(&[IMAGE_TEXT, IMAGE_DRAW]);
    for (data, dict) in [
        (&[0, 0, 0][..], "/Width 1 /Height 1 /ColorSpace /DeviceRGB /BitsPerComponent 8"),
        (&[0, 0, 0, 255][..], "/Width 1 /Height 1 /ColorSpace /DeviceCMYK /BitsPerComponent 8"),
        (&[0][..], "/Width 1 /Height 1 /ColorSpace /DeviceGray /BitsPerComponent 1"),
        (&[255][..], "/Width 1 /Height 1 /ColorSpace /DeviceGray /BitsPerComponent 8 /Decode [1 0]"),
    ] {
        assert_eq!(audit_redactions(&open(image_page(&content, data, dict, &[]))).unwrap().len(), 1);
    }
}
/// Image comparisons and stored coverage vertices consume bounded work and error on excess.
#[test]
fn image_comparisons_and_coverage_are_bounded() {
    let content = joined(&[IMAGE_TEXT, IMAGE_DRAW]);
    let doc = open(gray_image(&content, 0));
    let limits = crate::Limits { comparisons: 1, covered_text: usize::MAX };
    assert!(matches!(crate::audit(&doc, limits), Err(crate::AuditError::TooManyComparisons)));
    let doc = open(image_page(&content, &vec![0; 100_001], "/Width 1 /Height 100001 /BitsPerComponent 8 /ColorSpace /DeviceGray", &[]));
    assert!(audit_redactions(&doc).is_err());
}
/// Compound clipping paths fail as unsupported, while simple convex polygon clips are
/// evaluated exactly for both included and excluded image coverage.
#[test]
fn image_compound_clipping_requires_complete_analysis() {
    let content = joined(&[IMAGE_TEXT, b"0 0 10 10 re 130 692 72 28 re W n ", IMAGE_DRAW]);
    assert!(audit_redactions(&open(gray_image(&content, 0))).is_err());
    for (path, expected) in [(b"0 0 m 10 0 l 10 10 l 0 10 l h W n ".as_slice(), 0), (b"130 692 m 202 692 l 202 720 l 130 720 l h W n ", 1)] {
        assert_eq!(audit_redactions(&open(gray_image(&joined(&[IMAGE_TEXT, path, IMAGE_DRAW]), 0))).unwrap().len(), expected);
    }
}

/// Joined text lines retain each run's paint order: images cover earlier overlapped glyphs,
/// and do not hide text painted after the image merely because its line starts earlier.
#[test]
fn image_paint_order_is_checked_per_run_of_a_joined_line() {
    // Geometry joins these separate text-showing operators into one line. The image precedes
    // every glyph it overlaps, even though it follows the line's first operator.
    let content = joined(&[b"BT /F1 24 Tf 72 700 Td (Hello ) Tj ET\n", IMAGE_DRAW, b"BT /F1 24 Tf 133.344 700 Td (secret world) Tj ET"]);
    assert!(audit_redactions(&open(gray_image(&content, 0))).unwrap().is_empty());
    let content = joined(&[b"BT /F1 24 Tf 72 700 Td (Hello secret ) Tj ET\n", IMAGE_DRAW, b"BT /F1 24 Tf 205.368 700 Td (world) Tj ET"]);
    assert_eq!(audit_redactions(&open(gray_image(&content, 0))).unwrap().len(), 1);
}

/// A page combining images with text inside forms errors because that text is not fully audited.
#[test]
fn image_with_unaudited_form_text_is_incomplete() {
    let content = b"/Fm Do\n";
    let form = "/Subtype /Form /BBox [0 0 612 792] /Resources << /Font << /F1 5 0 R >> /XObject << /Im 6 0 R >> >>";
    let bytes =
        image_page(content, &[0], "/Width 1 /Height 1 /ColorSpace /DeviceGray /BitsPerComponent 8", &[(&joined(&[IMAGE_TEXT, IMAGE_DRAW]), form)]);
    assert!(audit_redactions(&open(bytes)).is_err());
}
/// Optional-content scope follows marked-content boundaries, independently of graphics-state
/// save/restore; an image inside it requires further analysis, even after `Q`.
#[test]
fn image_optional_content_state_does_not_follow_q_and_q_restore() {
    let content = joined(&[IMAGE_TEXT, b"q /OC /L1 BDC Q ", IMAGE_DRAW, b"EMC"]);
    assert!(audit_redactions(&open(gray_image(&content, 0))).is_err());
    let content = joined(&[IMAGE_TEXT, b"/OC /L1 BDC EMC ", IMAGE_DRAW]);
    assert_eq!(audit_redactions(&open(gray_image(&content, 0))).unwrap().len(), 1);
}
/// Overflowing or out-of-range transforms fail image analysis before geometry can be trusted.
#[test]
fn image_extreme_geometry_is_incomplete() {
    for draw in [b"q 1e300 0 0 28 130 692 cm /Im Do Q".as_slice(), b"q 1000000000000 0 0 28 130 692 cm /Im Do Q"] {
        assert!(audit_redactions(&open(gray_image(&joined(&[IMAGE_TEXT, draw]), 0))).is_err());
    }
}

/// Excessive graphics-state saves return an incomplete-audit error rather than grow unbounded.
#[test]
fn image_graphics_state_stack_is_bounded() {
    let content = joined(&[IMAGE_TEXT, &b"q ".repeat(4097), IMAGE_DRAW]);
    assert!(audit_redactions(&open(gray_image(&content, 0))).is_err());
}

/// Disconnected dark samples and stencil bits contribute their combined painted area,
/// excluding light samples and holes rather than treating the image bounds as opaque.
#[test]
fn image_sparse_samples_are_measured_as_a_union() {
    let content = joined(&[IMAGE_TEXT, IMAGE_DRAW]);
    for (data, expected) in [([0, 255, 0], 1), ([0, 255, 255], 0), ([255, 255, 255], 0)] {
        let bytes = image_page(&content, &data, "/Width 3 /Height 1 /ColorSpace /DeviceGray /BitsPerComponent 8", &[]);
        assert_eq!(audit_redactions(&open(bytes)).unwrap().len(), expected);
    }
    let bytes = image_page(&content, &[0b01000000], "/Width 3 /Height 1 /ImageMask true /BitsPerComponent 1", &[]);
    assert_eq!(audit_redactions(&open(bytes)).unwrap().len(), 1);
}
/// Split content streams share transforms and paint order for inline images, detecting
/// images after text while leaving images painted before it unflagged.
#[test]
fn image_state_and_order_carry_across_content_streams() {
    let first = joined(&[IMAGE_TEXT, b"q 72 0 0 28 130 692 cm\n"]);
    let second = b"BI /W 1 /H 1 /CS /G /BPC 8 ID \x00 EI Q";
    assert_eq!(audit_redactions(&open(two_streams(&first, second))).unwrap().len(), 1);
    let before = joined(&[b"q 72 0 0 28 130 692 cm\n", second]);
    assert!(audit_redactions(&open(two_streams(&before, IMAGE_TEXT))).unwrap().is_empty());
}

/// Unsupported blend, soft-mask and overprint states fail closed; explicit resets to normal
/// opaque compositing restore the ability to report an image cover.
#[test]
fn image_compositing_requires_opaque_normal_paint() {
    for prefix in [b"/Screen gs ".as_slice(), b"/Over gs "] {
        assert!(audit_redactions(&open(gray_image(&joined(&[IMAGE_TEXT, prefix, IMAGE_DRAW]), 0))).is_err());
    }
    for prefix in [b"/Screen gs /Normal gs ".as_slice(), b"/Soft gs /Normal gs ", b"/Over gs /NoOver gs "] {
        assert_eq!(audit_redactions(&open(gray_image(&joined(&[IMAGE_TEXT, prefix, IMAGE_DRAW]), 0))).unwrap().len(), 1);
    }
}
