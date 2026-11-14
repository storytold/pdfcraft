//! One line of text in any script as glyph outlines, for generated appearances whose text the
//! standard fonts can't draw (a stamp's label in Russian, Japanese, Hindi, Arabic, …).
//!
//! The line is split into directional runs (UAX #9, `unicode-bidi`), each run into pieces drawn
//! by the first face that has their characters, and each piece is shaped with harfrust in its
//! script and language, so ligatures, conjuncts, marks and joining forms come out as the face
//! means them. Nothing is embedded: the result is outlines, filled by the caller.

use harfrust::{Direction, Language, ShapeOptions, ShaperData, UnicodeBuffer};
use skrifa::instance::{LocationRef, Size};
use skrifa::outline::DrawSettings;
use skrifa::{FontRef, GlyphId, MetadataProvider};
use unicode_bidi::BidiInfo;

use crate::script::Flatten;
use crate::{CRAFT_FONTS, CraftFont, GlyphError, ScriptOutline};

/// Characters of one line drawn at most: a label or a "By …" line, not a paragraph.
pub const MAX_LINE_CHARS: usize = 256;

/// Outline points of one line at most, as for a typed signature.
const MAX_LINE_POINTS: usize = 262_144;

/// The faces for a line of text in `script` (ISO 15924: `"Cyrl"`, `"Jpan"`, `"Deva"`, …), best
/// first: the craft-fonts faces made for the script (bold ones first when `bold`), for Chinese,
/// Japanese and Korean then the other faces with Han characters (the Japanese Gothic first, as
/// the faces made for Chinese may be missing), then the bundled Inter (Latin,
/// Greek and Cyrillic, in every build), then every other craft-fonts face, so a name in another
/// script on the line still finds a face. Faces the script needs may be missing: check
/// [`line_missing`].
pub fn line_faces(script: &str, bold: bool) -> Vec<&'static CraftFont> {
    let made_for = |f: &CraftFont, s: &str| f.covers(s) || (s == "Kore" && f.covers("Hang"));
    let mut own: Vec<&'static CraftFont> = if script == "Jpan" {
        crate::document_japanese_fonts_for_style(false, bold)
    } else {
        CRAFT_FONTS.iter().filter(|f| made_for(f, script)).collect()
    };
    // Stable: manifest order within each weight.
    own.sort_by_key(|f| f.style.contains("Bold") != bold);
    let han = ["Hans", "Hant", "Jpan", "Kore"];
    // The Japanese Gothic faces first (stamps and labels are sans), then the other Han faces.
    let others_with_han: Vec<&'static CraftFont> = if han.contains(&script) {
        crate::document_japanese_fonts_for_style(false, bold)
            .into_iter()
            .chain(CRAFT_FONTS.iter().filter(|f| han.iter().any(|s| f.covers(s))))
            .collect()
    } else {
        Vec::new()
    };
    let mut out: Vec<&'static CraftFont> = Vec::new();
    for face in own.into_iter().chain(others_with_han).chain(crate::document_latin_fonts(bold)).chain(CRAFT_FONTS.iter()) {
        if !out.iter().any(|f| std::ptr::eq(*f, face)) {
            out.push(face);
        }
    }
    out
}

/// The first character of `text` (other than white space and control characters) that none of
/// `faces` has a glyph for, so [`line_outline`] would fail. `None` when they cover all of it.
pub fn line_missing(text: &str, faces: &[&CraftFont]) -> Option<char> {
    let fonts: Vec<FontRef> = faces.iter().filter_map(|f| FontRef::new(f.bytes).ok()).collect();
    text.chars().find(|c| !c.is_whitespace() && !c.is_control() && !fonts.iter().any(|f| has(f, *c)))
}

fn has(font: &FontRef, c: char) -> bool {
    font.charmap().map(c).is_some_and(|g| g.to_u32() != 0)
}

/// The outlines of one line of `text` at a font size of 1 (em units, baseline at y = 0), laid
/// out left to right as displayed. `lang` is the BCP 47 tag of the text's language ("ja",
/// "zh-Hant"), which picks a face's local forms. Line breaks and other control characters are
/// drawn as spaces. [`GlyphError::Missing`] when no face has a character (see [`line_missing`]),
/// [`GlyphError::TooComplex`] past [`MAX_LINE_CHARS`] or the outline budget, [`GlyphError::NoFont`]
/// without faces.
pub fn line_outline(text: &str, faces: &[&CraftFont], lang: &str) -> Result<ScriptOutline, GlyphError> {
    if text.chars().take(MAX_LINE_CHARS + 1).count() > MAX_LINE_CHARS {
        return Err(GlyphError::TooComplex);
    }
    let fonts: Vec<FontRef> = faces.iter().filter_map(|f| FontRef::new(f.bytes).ok()).collect();
    let Some(first) = fonts.first() else { return Err(GlyphError::NoFont) };
    let text: String = text.chars().map(|c| if c.is_control() { ' ' } else { c }).collect();
    let language = lang.parse::<Language>().ok();
    let loc = LocationRef::default();
    let metrics = first.metrics(Size::unscaled(), loc);
    let em = 1.0 / f64::from(metrics.units_per_em.max(1));
    let mut pen = Flatten::new(em);
    pen.max_points = MAX_LINE_POINTS;
    let bidi = BidiInfo::new(&text, None);
    for para in &bidi.paragraphs {
        let (levels, runs) = bidi.visual_runs(para, para.range.clone());
        for run in runs {
            let rtl = levels.get(run.start).is_some_and(|l| l.is_rtl());
            let piece_text = text.get(run.clone()).ok_or(GlyphError::Missing)?;
            let mut pieces = pieces(piece_text, &fonts)?;
            // The pieces of a right-to-left run are displayed last first.
            if rtl {
                pieces.reverse();
            }
            for (face, range) in pieces {
                let font = fonts.get(face).ok_or(GlyphError::Missing)?;
                let piece = piece_text.get(range).ok_or(GlyphError::Missing)?;
                draw_piece(&mut pen, font, piece, rtl, language.clone())?;
            }
        }
    }
    pen.close();
    if pen.too_complex {
        return Err(GlyphError::TooComplex);
    }
    if pen.contours.iter().flatten().any(|p| !p[0].is_finite() || !p[1].is_finite()) || !pen.dx.is_finite() {
        return Err(GlyphError::Missing);
    }
    Ok(ScriptOutline { contours: pen.contours, width: pen.dx, ascent: f64::from(metrics.ascent) * em, descent: f64::from(metrics.descent) * em })
}

/// `text` cut into pieces by face: a character stays with the face of the piece before it when
/// that face has it (marks with their base, spaces and digits with their words), else it starts a
/// piece in the first face that has it. Byte ranges into `text`, in logical order.
fn pieces(text: &str, fonts: &[FontRef]) -> Result<Vec<(usize, std::ops::Range<usize>)>, GlyphError> {
    let mut out: Vec<(usize, std::ops::Range<usize>)> = Vec::new();
    for (at, c) in text.char_indices() {
        let end = at + c.len_utf8();
        if let Some((face, range)) = out.last_mut()
            && (c.is_whitespace() || fonts.get(*face).is_some_and(|f| has(f, c)))
        {
            range.end = end;
            continue;
        }
        let face = fonts.iter().position(|f| has(f, c)).ok_or(GlyphError::Missing)?;
        out.push((face, at..end));
    }
    Ok(out)
}

/// Shape `piece` in `font` and draw it at the pen, which moves past it.
fn draw_piece(pen: &mut Flatten, font: &FontRef, piece: &str, rtl: bool, language: Option<Language>) -> Result<(), GlyphError> {
    let data = ShaperData::new(font);
    let shaper = data.shaper(font).build();
    let scale = 1.0 / f64::from(shaper.units_per_em().max(1));
    let mut buffer = UnicodeBuffer::new();
    buffer.push_str(piece);
    buffer.set_direction(if rtl { Direction::RightToLeft } else { Direction::LeftToRight });
    if let Some(language) = language {
        buffer.set_language(language);
    }
    buffer.guess_segment_properties();
    let shaped = shaper.shape(buffer, ShapeOptions::new());
    let outlines = font.outline_glyphs();
    let loc = LocationRef::default();
    let origin = pen.dx;
    let mut x = 0.0;
    for (info, pos) in shaped.glyph_infos().iter().zip(shaped.glyph_positions()) {
        if info.glyph_id == 0 {
            // A space the face has no glyph for still advances.
            if piece.get(info.cluster as usize..).and_then(|s| s.chars().next()).is_some_and(char::is_whitespace) {
                x += f64::from(pos.x_advance) * scale;
                continue;
            }
            return Err(GlyphError::Missing);
        }
        if let Some(glyph) = outlines.get(GlyphId::new(info.glyph_id)) {
            pen.close();
            pen.scale = scale;
            pen.dx = origin + x + f64::from(pos.x_offset) * scale;
            pen.dy = f64::from(pos.y_offset) * scale;
            let _ = glyph.draw(DrawSettings::unhinted(Size::unscaled(), loc), &mut *pen);
            pen.close();
            if pen.too_complex {
                return Err(GlyphError::TooComplex);
            }
        }
        x += f64::from(pos.x_advance) * scale;
    }
    pen.dx = origin + x;
    pen.dy = 0.0;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn latin() -> Vec<&'static CraftFont> {
        crate::document_latin_fonts(true)
    }

    #[test]
    fn cyrillic_greek_and_vietnamese_draw_with_the_bundled_face() {
        for text in ["ОДОБРЕНО", "ЗАТВЕРДЖЕНО ҚҰПИЯ Әже", "ΕΓΚΡΙΘΗΚΕ", "ĐÃ PHÊ DUYỆT"] {
            assert_eq!(line_missing(text, &latin()), None, "{text}");
            let o = line_outline(text, &latin(), "und").unwrap();
            assert!(!o.contours.is_empty() && o.width > 1.0, "{text}");
            let b = o.bounds();
            assert!(b.iter().all(|v| v.is_finite()) && b[3] > 0.5, "{text}: {b:?}");
        }
    }

    #[test]
    fn a_space_and_a_wider_text_move_the_pen() {
        let one = line_outline("АБ", &latin(), "ru").unwrap();
        let spaced = line_outline("А Б", &latin(), "ru").unwrap();
        assert!(spaced.width > one.width);
        assert_eq!(spaced.contours.len(), one.contours.len(), "a space draws nothing");
        assert_eq!(line_outline("", &latin(), "ru").unwrap().width, 0.0);
        assert_eq!(line_outline("\n\t", &latin(), "ru").map(|o| o.contours.len()), Ok(0), "control characters are spaces");
    }

    #[test]
    fn text_no_face_has_is_reported_not_drawn() {
        // An Egyptian hieroglyph: no face PdfCraft has or fetches draws it.
        let text = "Ada \u{13000}";
        assert_eq!(line_missing(text, &latin()), Some('\u{13000}'));
        assert_eq!(line_outline(text, &latin(), "en"), Err(GlyphError::Missing));
        assert_eq!(line_outline("A", &[], "en"), Err(GlyphError::NoFont));
    }

    #[test]
    fn overlong_lines_are_refused_whole() {
        let long = "Ж".repeat(MAX_LINE_CHARS + 1);
        assert_eq!(line_outline(&long, &latin(), "ru"), Err(GlyphError::TooComplex));
        assert!(line_outline(&"Ж".repeat(MAX_LINE_CHARS), &latin(), "ru").is_ok());
    }

    #[test]
    fn faces_for_a_script_end_with_the_bundled_face() {
        for script in ["Latn", "Cyrl", "Jpan", "Hant", "Kore", "Arab", "Deva", "Telu", "Laoo", "Thai"] {
            let faces = line_faces(script, true);
            assert!(faces.iter().any(|f| f.family == "Inter"), "{script}");
            let distinct = faces.iter().enumerate().all(|(i, f)| faces.iter().skip(i + 1).all(|g| !std::ptr::eq(*f, *g)));
            assert!(distinct, "{script}: each face once");
        }
        if CRAFT_FONTS.iter().all(|f| !f.covers("Cyrl")) {
            assert_eq!(line_faces("Cyrl", true).first().map(|f| f.style), Some("SemiBold"), "bold Inter first");
            assert_eq!(line_faces("Cyrl", false).first().map(|f| f.style), Some("Regular"));
        }
    }

    #[test]
    fn shaped_scripts_draw_with_their_craft_fonts_face() {
        // Each needs its craft-fonts face; builds without one skip that script.
        for (script, lang, text) in [
            ("Jpan", "ja", "承認済み"),
            ("Hans", "zh-Hans", "已批准"),
            ("Hant", "zh-Hant", "已核准"),
            ("Kore", "ko", "승인됨"),
            ("Arab", "ar", "تمت الموافقة"),
            ("Thai", "th", "อนุมัติแล้ว"),
            ("Deva", "hi", "स्वीकृत"),
            ("Telu", "te", "ఆమోదించబడింది"),
            ("Laoo", "lo", "ອະນຸມັດແລ້ວ"),
        ] {
            let faces = line_faces(script, true);
            if line_missing(text, &faces).is_some() {
                eprintln!("skipping {script}: no craft-fonts face for it in this build (set CRAFT_FONTS_DIR)");
                continue;
            }
            let o = line_outline(text, &faces, lang).unwrap_or_else(|e| panic!("{script}: {e:?}"));
            assert!(!o.contours.is_empty() && o.width > 0.5, "{script}");
            assert!(o.bounds().iter().all(|v| v.is_finite()), "{script}");
        }
    }

    #[test]
    fn a_right_to_left_run_keeps_a_latin_name_whole() {
        let faces = line_faces("Arab", false);
        let text = "بواسطة Ada";
        if line_missing(text, &faces).is_some() {
            eprintln!("skipping: no craft-fonts Arabic face in this build (set CRAFT_FONTS_DIR)");
            return;
        }
        let whole = line_outline(text, &faces, "ar").unwrap();
        let name = line_outline("Ada", &faces, "ar").unwrap();
        // "Ada" is displayed at the left, before the Arabic word read right to left.
        let left: Vec<_> = whole.contours.iter().filter(|c| c.iter().all(|p| p[0] < name.width + 0.05)).collect();
        assert_eq!(left.len(), name.contours.len(), "the name's glyphs come first on the line");
    }
}
