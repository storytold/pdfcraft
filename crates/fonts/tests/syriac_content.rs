//! Syriac shaping must use OpenType and preserve logical cluster ownership.
use pdfcraft_fonts::{CRAFT_FONTS, content_text, document_syriac_font, shape_syriac, source_shaping};
use skrifa::{FontRef, raw::TableProvider};

/// Skip optional-font checks locally but fail when Syriac qualification requires the input.
fn available() -> bool {
    let present = document_syriac_font().is_some();
    assert!(present || std::env::var_os("PDFCRAFT_REQUIRE_SYRIAC").is_none(), "qualification requires the Syriac craft-fonts input");
    present
}

/// A source face without GSUB must be refused even when its cmap remains readable.
#[test]
fn source_without_gsub_is_refused_before_unjoined_text_can_be_written() {
    if !available() {
        return;
    }
    let face = document_syriac_font().unwrap();
    let mut bytes = face.bytes.to_vec();
    // Keep every table's offsets intact, but remove GSUB from the SFNT directory.
    // Parsing glyphs and cmap must still succeed, so refusal pins the contextual guard.
    let count = u16::from_be_bytes(bytes[4..6].try_into().unwrap()) as usize;
    let mut removed = false;
    for record in bytes[12..].chunks_exact_mut(16).take(count) {
        if record.get(..4) == Some(b"GSUB") {
            record[..4].copy_from_slice(b"TEST");
            removed = true;
        }
    }
    assert!(removed);
    let font = FontRef::new(&bytes).unwrap();
    assert!(font.gsub().is_err());
    assert!(font.cmap().is_ok());
    let error = source_shaping::shape(&bytes, "\u{0712}\u{0712}").unwrap_err();
    assert!(error.contains("contextual substitution"), "{error}");
}

/// Verify contextual glyph changes, mark ownership and valid shaping in all three faces.
#[test]
fn joining_marks_and_all_three_source_faces_use_real_layout() {
    if !available() {
        return;
    }
    let single = shape_syriac("\u{0712}", true).unwrap();
    let joined = shape_syriac("\u{0712}\u{0712}", true).unwrap();
    let isolated_id = single[0].glyphs[0].0;
    assert!(joined.iter().flat_map(|cluster| &cluster.glyphs).any(|(id, _)| *id != isolated_id));
    let marked = shape_syriac("\u{0712}\u{0730}", true).unwrap();
    assert_eq!(marked.iter().map(|cluster| cluster.text.as_str()).collect::<String>(), "\u{0712}\u{0730}");
    assert!(marked.iter().map(|cluster| cluster.glyphs.len()).sum::<usize>() >= 2);
    let faces: Vec<_> = CRAFT_FONTS.iter().filter(|face| face.covers("Syrc")).collect();
    assert_eq!(faces.len(), 3);
    for face in faces {
        let run = source_shaping::shape(face.bytes, "ABC 12 \u{0712}\u{0730}\u{0712}").unwrap();
        assert!(run.advance.is_finite() && run.advance > 0.0);
        assert!(run.glyphs.iter().all(|glyph| glyph.position.iter().all(|p| p.is_finite())));
    }
    assert!(content_text::wrap("\u{0860}", 12.0, 200.0).is_err(), "the selected releases lack the supplement");
}

/// Verify wrapped bidi direction, attached comb marks, digit order and input bounds.
#[test]
fn wrapping_keeps_paragraph_direction_and_comb_keeps_marks_and_digits() {
    if !available() {
        return;
    }
    let rows = content_text::wrap("\u{0712}\u{0730}\u{0712} ABC 12", 12.0, 20.0).unwrap();
    assert!(rows.len() > 1);
    assert!(rows.iter().all(|row| row.rtl));
    assert!(rows.iter().any(|row| row.text == "12"));
    let cells = content_text::comb_cells("\u{0712}\u{0730} 12").unwrap();
    assert!(cells.iter().any(|cell| cell.text == "\u{0712}\u{0730}"));
    let one = cells.iter().position(|cell| cell.text == "1").unwrap();
    let two = cells.iter().position(|cell| cell.text == "2").unwrap();
    assert_eq!(two, one + 1);
    assert!(content_text::wrap("\u{0712}", f64::NAN, 100.0).is_err());
    assert!(content_text::comb_cells(&"\u{0712}".repeat(4097)).is_err());
}
