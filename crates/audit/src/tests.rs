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

/// A 12 pt sentence at 72,700 that runs most of the way across the page (about 408 pt wide).
const SENTENCE: &[u8] = b"BT /F1 12 Tf 72 700 Td (The witness John Q Example lives at 12 Elm Street in Springfield and saw it) Tj ET\n";

#[test]
fn adjacent_covers_over_a_line_are_each_found() {
    // Four boxes that together black out the whole line: each one is a cover on its own.
    let mut content = SENTENCE.to_vec();
    content.extend_from_slice(b"0 g\n72 696 125 16 re f\n197 696 125 16 re f\n322 696 125 16 re f\n447 696 125 16 re f\n");
    let findings = audit_redactions(&open(one_page(&content, ""))).expect("audit must run");
    assert_eq!(findings.len(), 4, "{findings:?}");
    assert!(findings.iter().all(|f| f.kind == FindingKind::CoveredText), "{findings:?}");
}

#[test]
fn rules_painted_after_text_are_not_covers() {
    // A strikethrough through the line and a table border crossing it hide no glyph: too thin
    // in the one case, too narrow in the other.
    for rule in [&b"0 g\n72 703 400 1 re f\n"[..], b"0 g\n250 600 0.5 200 re f\n"] {
        let mut content = SENTENCE.to_vec();
        content.extend_from_slice(rule);
        let findings = audit_redactions(&open(one_page(&content, ""))).expect("audit must run");
        assert!(findings.is_empty(), "{:?}: {findings:?}", String::from_utf8_lossy(rule));
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

/// One page of `TEXT` with one annotation (object 6, `annot` is the rest of its dictionary) whose
/// appearance streams are objects 7, 8, ... (`streams`: each the rest of the stream dictionary
/// and the data).
fn annotated(annot: &str, streams: &[(&str, &[u8])]) -> Vec<u8> {
    let mut b = Builder::new();
    b.add("<< /Type /Catalog /Pages 2 0 R >>");
    b.add("<< /Type /Pages /Kids [3 0 R] /Count 1 >>");
    b.add("<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Contents 4 0 R /Resources << /Font << /F1 5 0 R >> >> /Annots [6 0 R] >>");
    b.add_stream(TEXT);
    b.add(FONT);
    b.add(&format!("<< /Type /Annot {annot} >>"));
    for (extra, data) in streams {
        b.add_stream_with(data, extra);
    }
    b.build()
}

const TEXT: &[u8] = b"BT /F1 24 Tf 100 700 Td (SECRET DATA 123) Tj ET\n";
const AP_FORM: &str = "/Type /XObject /Subtype /Form /BBox [0 0 220 32]";
const AP_FILL: &[u8] = b"0 g\n0 0 220 32 re f\n";

#[test]
fn annotation_appearance_over_text_is_found() {
    // A black rectangle drawn as a shape or highlight annotation covers the text exactly like
    // one in the page content: the viewer paints its appearance stream over the page.
    for subtype in ["Square", "Highlight"] {
        let annot = format!("/Subtype /{subtype} /Rect [100 688 320 720] /C [0 0 0] /IC [0 0 0] /F 4 /AP << /N 7 0 R >>");
        let findings = audit_redactions(&open(annotated(&annot, &[(AP_FORM, AP_FILL)]))).expect("audit must run");
        assert_eq!(findings.len(), 1, "{subtype}: {findings:?}");
        assert_eq!(findings[0].kind, FindingKind::CoveredText);
        assert_eq!(findings[0].rect, [100.0, 688.0, 320.0, 720.0]);
        assert_eq!(findings[0].covered_text, "SECRET DATA 123");
    }
}

#[test]
fn annotation_appearance_is_placed_by_rect_bbox_and_matrix() {
    // ISO 32000-2 §12.5.5: the form's /BBox goes through /Matrix, and the box that comes out is
    // scaled and moved onto /Rect. Here a 100-unit square scaled by 2 lands on the 220 × 32 rect.
    let annot = "/Subtype /Square /Rect [100 688 320 720] /F 4 /AP << /N 7 0 R >>";
    let form = "/Type /XObject /Subtype /Form /BBox [0 0 100 100] /Matrix [2 0 0 2 50 50]";
    let findings = audit_redactions(&open(annotated(annot, &[(form, b"0 g\n0 0 100 100 re f\n")]))).expect("audit must run");
    assert_eq!(findings.len(), 1, "{findings:?}");
    assert_eq!(findings[0].rect, [100.0, 688.0, 320.0, 720.0]);
    // The same square painted in the lower-left quarter of the form hides only the left half of
    // the line's x range and the bottom of its height: under half of each word, no cover.
    let findings = audit_redactions(&open(annotated(annot, &[(form, b"0 g\n0 0 50 50 re f\n")]))).expect("audit must run");
    assert!(findings.is_empty(), "{findings:?}");
}

#[test]
fn hidden_or_noview_annotation_appearance_is_not_a_cover() {
    // Flags Hidden (2) and NoView (32): the viewer does not paint it, so the text shows.
    for flags in [2, 32, 34] {
        let annot = format!("/Subtype /Square /Rect [100 688 320 720] /F {flags} /AP << /N 7 0 R >>");
        let findings = audit_redactions(&open(annotated(&annot, &[(AP_FORM, AP_FILL)]))).expect("audit must run");
        assert!(findings.is_empty(), "/F {flags}: {findings:?}");
    }
}

#[test]
fn annotation_appearance_state_selects_the_stream() {
    // /N holds one stream per appearance state; /AS names the one shown.
    let streams: &[(&str, &[u8])] = &[(AP_FORM, AP_FILL), (AP_FORM, b"")];
    let on = "/Subtype /Square /Rect [100 688 320 720] /F 4 /AS /On /AP << /N << /On 7 0 R /Off 8 0 R >> >>";
    let findings = audit_redactions(&open(annotated(on, streams))).expect("audit must run");
    assert_eq!(findings.len(), 1, "{findings:?}");
    let off = "/Subtype /Square /Rect [100 688 320 720] /F 4 /AS /Off /AP << /N << /On 7 0 R /Off 8 0 R >> >>";
    let findings = audit_redactions(&open(annotated(off, streams))).expect("audit must run");
    assert!(findings.is_empty(), "{findings:?}");
}

#[test]
fn undecodable_annotation_appearance_fails_the_audit() {
    let annot = "/Subtype /Square /Rect [100 688 320 720] /F 4 /AP << /N 7 0 R >>";
    let form = "/Type /XObject /Subtype /Form /BBox [0 0 220 32] /Filter /FlateDecode";
    let err = audit_redactions(&open(annotated(annot, &[(form, b"this is not deflated data")]))).expect_err("audit must fail closed");
    assert!(err.to_string().contains("decoded"), "unexpected error: {err}");
}

#[test]
fn background_before_text_is_not_flagged() {
    // An opaque dark rectangle painted *before* the text is a background, not a cover.
    let content = b"0 g\n100 688 220 32 re f\nBT /F1 24 Tf 100 700 Td (SECRET DATA 123) Tj ET\n";
    let findings = audit_redactions(&open(one_page(content, ""))).expect("audit must run");
    assert!(findings.is_empty(), "{findings:?}");
}

#[test]
fn opaque_cover_hides_text_whatever_the_colours() {
    // An opaque fill painted after the text hides it whether the two colours are alike or
    // not: grey, red or black text under a black box, black text under a white-out box, under
    // light table-style shading, under mid grey or under opaque yellow.
    let covers: [&[u8]; 7] = [
        b"0.4 g\nBT /F1 24 Tf 100 700 Td (SECRET DATA 123) Tj ET\n0 g\n100 688 220 32 re f\n",
        b"1 0 0 rg\nBT /F1 24 Tf 100 700 Td (SECRET DATA 123) Tj ET\n0 g\n100 688 220 32 re f\n",
        b"BT /F1 24 Tf 100 700 Td (SECRET DATA 123) Tj ET\n1 g\n100 688 220 32 re f\n",
        b"BT /F1 24 Tf 100 700 Td (SECRET DATA 123) Tj ET\n0.9 g\n100 688 220 32 re f\n",
        b"BT /F1 24 Tf 100 700 Td (SECRET DATA 123) Tj ET\n0.5 g\n100 688 220 32 re f\n",
        b"BT /F1 24 Tf 100 700 Td (SECRET DATA 123) Tj ET\n1 1 0 rg\n100 688 220 32 re f\n",
        b"BT /F1 24 Tf 100 700 Td (SECRET DATA 123) Tj ET\n0 0 0 0 k\n100 688 220 32 re f\n",
    ];
    for content in covers {
        let findings = audit_redactions(&open(one_page(content, ""))).expect("audit must run");
        assert_eq!(findings.len(), 1, "{:?}: {findings:?}", String::from_utf8_lossy(content));
        assert_eq!(findings[0].kind, FindingKind::CoveredText);
        assert_eq!(findings[0].covered_text, "SECRET DATA 123");
    }
}

/// One page whose `/ExtGState /GS1` is `gs1`, with `s2` as a second content stream when given.
fn with_gstate(s1: &[u8], s2: Option<&[u8]>, gs1: &str) -> Vec<u8> {
    let mut b = Builder::new();
    b.add("<< /Type /Catalog /Pages 2 0 R >>");
    b.add("<< /Type /Pages /Kids [3 0 R] /Count 1 >>");
    let contents = if s2.is_some() { "[4 0 R 7 0 R]" } else { "4 0 R" };
    b.add(&format!(
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Contents {contents} /Resources << /Font << /F1 5 0 R >> /ExtGState << /GS1 6 0 R >> >> >>"
    ));
    b.add_stream(s1);
    b.add(FONT);
    b.add(&format!("<< /Type /ExtGState {gs1} >>"));
    if let Some(s2) = s2 {
        b.add_stream(s2);
    }
    b.build()
}

#[test]
fn blended_highlight_is_not_a_cover() {
    // A yellow highlight drawn with a Multiply (or Darken) blend mode leaves the text visible;
    // the blend mode may be a name or an array of names. A Normal or Compatible mode is opaque.
    for (bm, hidden) in [("/BM /Multiply", false), ("/BM [/Darken /Compatible]", false), ("/BM /Normal", true), ("/BM /Compatible", true)] {
        let content = b"BT /F1 24 Tf 100 700 Td (SECRET DATA 123) Tj ET\n/GS1 gs\n1 1 0 rg\n100 688 220 32 re f\n";
        let findings = audit_redactions(&open(with_gstate(content, None, bm))).expect("audit must run");
        assert_eq!(findings.len(), usize::from(hidden), "{bm}: {findings:?}");
    }
}

#[test]
fn graphics_state_carries_across_streams() {
    // The first stream makes fills translucent after the text; the second stream's rectangle
    // over the text is therefore translucent, not the default opaque: no finding. Without the
    // carry fix the rectangle would default to opaque and flag.
    let s1 = b"BT /F1 24 Tf 100 700 Td (SECRET DATA 123) Tj ET\n/GS1 gs\n";
    let s2 = b"100 688 220 32 re f\n";
    let findings = audit_redactions(&open(with_gstate(s1, Some(s2), "/ca 0.5"))).expect("audit must run");
    assert!(findings.is_empty(), "{findings:?}");
}

#[test]
fn ext_gstate_without_an_entry_leaves_it_unchanged() {
    // ISO 32000-2 §8.4.5: an ExtGState sets only the parameters it names. A second `gs` that
    // sets nothing relevant keeps the translucency (and a blend mode) of the first.
    let mut b = Builder::new();
    b.add("<< /Type /Catalog /Pages 2 0 R >>");
    b.add("<< /Type /Pages /Kids [3 0 R] /Count 1 >>");
    b.add("<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Contents 4 0 R /Resources << /Font << /F1 5 0 R >> /ExtGState << /GS1 6 0 R /GS2 7 0 R >> >> >>");
    b.add_stream(b"BT /F1 24 Tf 100 700 Td (SECRET DATA 123) Tj ET\n/GS1 gs /GS2 gs\n0 g\n100 688 220 32 re f\n");
    b.add(FONT);
    b.add("<< /Type /ExtGState /ca 0.5 >>");
    b.add("<< /Type /ExtGState /LW 2 >>");
    let findings = audit_redactions(&open(b.build())).expect("audit must run");
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
