//! Public-API regression for explicit-direction rewrite dispatch.
//!
//! Synthetic PDF operators and CMap only: no font file or binary fixture.

use std::sync::Arc;

use pdfcraft_cos::{Document, SaveOptions, write_full};
use pdfcraft_edit::text;
use pdfcraft_fonts::TextDirection;

/// A one-page document built from synthetic objects, as in edit's unit fixtures.
fn build(objects: &[&str]) -> Document {
    let mut bytes = b"%PDF-1.7\n".to_vec();
    let mut offsets = Vec::new();
    for (index, object) in objects.iter().enumerate() {
        offsets.push(bytes.len());
        bytes.extend_from_slice(format!("{} 0 obj\n{object}\nendobj\n", index + 1).as_bytes());
    }
    let xref = bytes.len();
    bytes.extend_from_slice(format!("xref\n0 {}\n0000000000 65535 f \n", objects.len() + 1).as_bytes());
    for offset in offsets {
        bytes.extend_from_slice(format!("{offset:010} 00000 n \n").as_bytes());
    }
    bytes.extend_from_slice(format!("trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n", objects.len() + 1).as_bytes());
    Document::open(Arc::new(bytes)).unwrap()
}

/// Wrap synthetic content operators in a correctly sized PDF stream object.
fn stream(content: &str) -> String {
    format!("<< /Length {} >>\nstream\n{content}\nendstream", content.len())
}

/// The F1 Helvetica portion of the existing text_page fixture.
fn text_page(content: &str) -> Document {
    build(&[
        "<< /Type /Catalog /Pages 2 0 R >>",
        "<< /Type /Pages /Kids [3 0 R] /Count 1 >>",
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 600 800] /Contents 4 0 R /Resources << /Font << /F1 5 0 R >> >> >>",
        &stream(content),
        "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica /Encoding /WinAnsiEncoding >>",
    ])
}

/// Own-project legacy Indic fixture: letters A..G name synthetic glyph codes.
/// Its ToUnicode yields visual-order Devanagari; no Kruti Dev font is embedded.
fn devanagari_page(content: &str) -> Document {
    let cmap = "/CIDInit /ProcSet findresource begin 12 dict begin begincmap /CMapType 2 def \
                1 begincodespacerange <00> <FF> endcodespacerange 8 beginbfchar \
                <20> <0020> <41> <092C> <42> <093F> <43> <0922093C> <44> <092F> <45> <093E> <46> <0915> <47> <0924> \
                endbfchar endcmap CMapName currentdict /CMap defineresource pop end end";
    let widths: Vec<&str> = (32..=71)
        .map(|code| {
            if code == 32 {
                "250"
            } else if code >= 65 {
                "500"
            } else {
                "0"
            }
        })
        .collect();
    let font = format!(
        "<< /Type /Font /Subtype /Type1 /BaseFont /ABCDEF+KrutiDev010 /FirstChar 32 /LastChar 71 /Widths [{}] /ToUnicode 6 0 R >>",
        widths.join(" ")
    );
    build(&[
        "<< /Type /Catalog /Pages 2 0 R >>",
        "<< /Type /Pages /Kids [3 0 R] /Count 1 >>",
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 600 800] /Contents 4 0 R /Resources << /Font << /F1 5 0 R >> >> >>",
        &stream(content),
        &font,
        &stream(cmap),
    ])
}

#[test]
/// Exercise explicit Auto/LTR/RTL and retained RTL through the public dispatch.
/// General-angle and legacy visual-order Indic sources must reach their intended
/// refusal with the complete serialized document unchanged.
fn explicit_direction_dispatch_refuses_general_turns_and_visual_indic_atomically() {
    for visual_indic in [false, true] {
        for (direction, stored) in
            [(Some(TextDirection::Auto), false), (Some(TextDirection::LeftToRight), false), (Some(TextDirection::RightToLeft), false), (None, true)]
        {
            let source = if visual_indic { "BT /F1 12 Tf 40 500 Td (ABCDE) Tj ET" } else { "BT /F1 12 Tf 1 1 -1 1 300 300 Tm (Only line) Tj ET" };
            let content = if stored { format!("/Span << /PCTextDirection /RTL >> BDC {source} EMC") } else { source.to_owned() };
            let mut doc = if visual_indic { devanagari_page(&content) } else { text_page(&content) };

            let lines = text::text_lines(&doc, 0).unwrap();
            let blocks = text::text_blocks(&doc, 0).unwrap();
            assert_eq!(lines.len(), 1, "one source line must reach the dispatch decision");
            assert_eq!(blocks.len(), 1, "one paragraph must reach the dispatch decision");
            assert_eq!(blocks[0].lines, [0], "the public block geometry must describe that source line");
            assert_eq!(lines[0].visual_order, visual_indic, "the Indic fixture must reach the visual-order exclusion");
            let expected_text = if visual_indic { "\u{092C}\u{0922}\u{093C}\u{093F}\u{092F}\u{093E}" } else { "Only line" };
            assert_eq!(lines[0].text, expected_text, "the synthetic font mapping must decode the intended source");
            // TextLine::upright is private. With one source line, this public
            // baseline angle pins the upright or unsupported 45° geometry.
            let expected_rotation = if visual_indic { 0.0 } else { 45.0 };
            assert!((blocks[0].rotation - expected_rotation).abs() < 1e-6, "unexpected fixture rotation: {}", blocks[0].rotation);
            let expected_direction = if stored { TextDirection::RightToLeft } else { TextDirection::Auto };
            assert_eq!(lines[0].text_direction, expected_direction);
            assert_eq!(blocks[0].text_direction, expected_direction, "retained RTL must survive grouping into the paragraph");

            let before = write_full(&doc, &SaveOptions::default()).unwrap();
            let style = text::BlockStyle { direction, ..Default::default() };
            let error = text::rewrite_block(&mut doc, 0, 0, None, &style).unwrap_err();
            let expected =
                if visual_indic { "explicit text direction for legacy visual-order Indic text" } else { "explicit text direction for turned text" };
            assert!(error.to_string().contains(expected), "{error}");
            assert_eq!(write_full(&doc, &SaveOptions::default()).unwrap(), before, "refusal must preserve every object, not only the page stream");
        }
    }
}

/// A uniform 90-degree source row is the admitted control for general-angle refusal:
/// requested Auto/LTR/RTL or retained RTL must survive serialization without changing
/// its logical text or baseline angle.
#[test]
fn explicit_direction_dispatch_accepts_uniform_quarter_turns() {
    for (direction, stored) in
        [(Some(TextDirection::Auto), false), (Some(TextDirection::LeftToRight), false), (Some(TextDirection::RightToLeft), false), (None, true)]
    {
        let source = "BT /F1 12 Tf 0 1 -1 0 300 300 Tm (Only line) Tj ET";
        let content = if stored { format!("/Span << /PCTextDirection /RTL >> BDC {source} EMC") } else { source.to_owned() };
        let mut doc = text_page(&content);
        text::rewrite_block(&mut doc, 0, 0, None, &text::BlockStyle { direction, ..Default::default() }).unwrap();
        let doc = Document::open(Arc::new(write_full(&doc, &SaveOptions::default()).unwrap())).unwrap();
        let blocks = text::text_blocks(&doc, 0).unwrap();
        assert_eq!(blocks.len(), 1);
        assert_eq!(blocks[0].editor_text, "Only line");
        assert!((blocks[0].rotation - 90.0).abs() < 1e-6);
        assert_eq!(blocks[0].text_direction, direction.unwrap_or(TextDirection::RightToLeft));
    }
}
