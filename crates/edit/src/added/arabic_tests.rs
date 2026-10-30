//! Arabic items too big for one Type 3 font. Shaped clusters and glyph outlines are injected here,
//! so these run without the craft-fonts Arabic face. Shaping itself is covered by the face tests in
//! `tests.rs`, which run where the face is built in.

use std::cell::Cell;

use super::*;

/// Regression fingerprint for the small-item fixture: drawing bytes, font map and document
/// objects, with generated streams decoded so Flate backend selection cannot change the hash.
// Re-pinned when the standard-14 metrics became exact (F1). The Latin run in the fingerprinted
// item is "Hi " in Helvetica 12pt: the old character-class guess called H 680, i 260 and space
// 260 (14.4pt), where the published metrics are 722, 222 and 278 (14.664pt). The Arabic clusters
// that follow therefore start 0.264pt further right, which is the whole point of F1 -- the glyphs
// now land where the font says they do. Nothing about the Arabic path changed.
const SMALL_ITEM_FINGERPRINT: u64 = 9_200_256_185_091_160_371;

/// A cluster that shows one character with the single glyph `id`, at `advance` em.
fn cluster(id: u32, advance: f64) -> ShapedCluster {
    ShapedCluster { script: ShapedScript::Arabic, glyphs: vec![(id, [0.0, 0.0])], advance, text: "ب".into() }
}

/// One run of `clusters` as a line of an item at `item()`'s size.
fn line(clusters: Vec<ShapedCluster>, rtl: bool) -> LaidLine {
    let text = clusters.iter().map(|c| c.text.as_str()).collect();
    let width = clusters.iter().map(|c| c.advance).sum::<f64>() * item().size;
    (text, rtl, vec![Piece::Shaped(clusters)], width)
}

/// A glyph outline: a box on the baseline, `advance` em wide.
fn boxed(advance: f64) -> GlyphOutline {
    let contours = vec![vec![[0.0, 0.0], [advance, 0.0], [advance, 0.5], [0.0, 0.5]]];
    GlyphOutline { contours, width: advance, bbox: [0.0, 0.0, advance, 0.5] }
}

fn item() -> AddedText {
    AddedText { rect: [72.0, 600.0, 540.0, 700.0], ..AddedText::default() }
}

/// Every object of `doc`, serialised: equal snapshots mean the document did not change.
fn snapshot(doc: &Document) -> Vec<u8> {
    let mut bytes = Vec::new();
    for num in doc.object_numbers() {
        let obj = doc.get(ObjRef { num, generation: doc.generation(num) });
        pdfcraft_cos::serialize(&obj, &mut bytes);
        bytes.push(b'\n');
    }
    bytes
}

/// Serialize generated objects with decoded stream bytes so the golden does not depend on which
/// Flate backend Cargo's unified dependency features select for the test binary. Only the raw
/// bytes (and so `/Length`, which the writer takes from them) are normalised: `/Filter` and
/// `/DecodeParms` stay in the hash, so a change that stops compressing generated streams, or
/// switches filters, still fails the golden.
fn golden_snapshot(doc: &Document) -> Vec<u8> {
    let mut bytes = Vec::new();
    for num in doc.object_numbers() {
        let mut obj = (*doc.get(ObjRef { num, generation: doc.generation(num) })).clone();
        if let Object::Stream(ref mut stream) = obj {
            let decoded = stream.decoded_strict().expect("generated stream decodes");
            stream.raw = decoded.into();
        }
        pdfcraft_cos::serialize(&obj, &mut bytes);
        bytes.push(b'\n');
    }
    bytes
}

/// The fonts an item drew, in creation order, with their glyph counts.
fn fonts_of(doc: &Document, fonts: &Dict) -> Vec<(Vec<u8>, usize)> {
    fonts
        .iter()
        .map(|(name, font)| {
            let glyphs = match doc.dict(font).as_ref().and_then(|f| f.get(b"CharProcs")) {
                Some(Object::Dict(procs)) => procs.len(),
                _ => 0,
            };
            (name.clone(), glyphs)
        })
        .collect()
}

/// The font and code of every glyph the content stream shows, in drawing order.
fn glyphs_shown(out: &[u8]) -> Vec<(Vec<u8>, u8)> {
    let mut font = Vec::new();
    let mut shown = Vec::new();
    for op in pdfcraft_content::parse(out).ops {
        if op.is("Tf") {
            font = op.name(0).unwrap_or_default().to_vec();
        } else if op.is("Tj")
            && let Some(Object::String(s)) = op.operands.first()
        {
            shown.push((font.clone(), s.bytes.first().copied().unwrap_or_default()));
        }
    }
    shown
}

/// 64-bit FNV-1a: a stable fingerprint for the golden check.
fn fnv1a(bytes: &[u8]) -> u64 {
    bytes.iter().fold(0xcbf2_9ce4_8422_2325, |h, b| (h ^ u64::from(*b)).wrapping_mul(0x0100_0000_01b3))
}

#[test]
fn an_item_past_the_old_glyph_limit_gets_a_font_per_240_glyphs() {
    // Sixteen fonts of 240 glyphs was the most an item could use. This one needs 3,841 glyphs.
    let n = 16 * GLYPHS_PER_FONT + 1;
    let laid = vec![line((0..n).map(|i| cluster(i as u32 + 1, 0.5)).collect(), false)];
    let mut doc = Document::new_empty();
    let (mut fonts, mut out) = (Dict::new(), Vec::new());
    let drawn = draw_laid_shaped(&mut doc, &item(), &laid, &Dict::new(), &mut fonts, &mut out, |_, _| Ok(boxed(0.5)));
    assert!(drawn.is_ok(), "{drawn:?}");
    let made = fonts_of(&doc, &fonts);
    let mut expected = vec![GLYPHS_PER_FONT; 16];
    expected.push(1);
    assert_eq!(made.iter().map(|(_, glyphs)| *glyphs).collect::<Vec<_>>(), expected);
    let shown = glyphs_shown(&out);
    assert_eq!(shown.len(), n);
    for (k, (font, code)) in shown.iter().enumerate() {
        assert_eq!(font, &made[k / GLYPHS_PER_FONT].0, "glyph {k} is drawn from the wrong font");
        assert_eq!(usize::from(*code), k % GLYPHS_PER_FONT + 1, "glyph {k} has the wrong code");
    }
}

#[test]
fn an_outline_missing_from_a_later_font_leaves_the_document_unchanged() {
    // Two fonts' worth of glyphs; the glyph in the second font has no outline.
    let n = 2 * GLYPHS_PER_FONT;
    let laid = vec![line((0..n).map(|i| cluster(i as u32 + 1, 0.5)).collect(), false)];
    let mut doc = Document::new_empty();
    let before = snapshot(&doc);
    let missing = n as u32;
    let (mut fonts, mut out) = (Dict::new(), Vec::new());
    let drawn = draw_laid_shaped(&mut doc, &item(), &laid, &Dict::new(), &mut fonts, &mut out, |_, id| {
        if id == missing { Err(GlyphError::Missing) } else { Ok(boxed(0.5)) }
    });
    assert!(matches!(&drawn, Err(EditError::Invalid(msg)) if msg.contains("can't show")), "{drawn:?}");
    assert_eq!(snapshot(&doc), before);
    assert!(fonts.is_empty() && out.is_empty());
}

#[test]
fn an_item_below_the_old_limit_is_drawn_exactly_as_before() {
    // A left-to-right line with a Latin run, a space and a two-glyph cluster, then a right-to-left
    // line. Glyphs repeat across the two lines and share one font.
    let space = ShapedCluster { script: ShapedScript::Arabic, glyphs: vec![(6, [0.0, 0.0])], advance: 0.25, text: " ".into() };
    let ligature = ShapedCluster { script: ShapedScript::Arabic, glyphs: vec![(4, [0.0, 0.0]), (5, [-0.1, 0.2])], advance: 0.6, text: "لا".into() };
    let mixed: LaidLine =
        ("Hi ب".into(), false, vec![Piece::Latin("Hi ".into()), Piece::Shaped(vec![cluster(1, 0.5), cluster(2, 0.5), space, ligature])], 4.0);
    let rtl = line(vec![cluster(2, 0.5), cluster(9, 0.75)], true);
    let laid = vec![mixed, rtl];
    let mut doc = Document::new_empty();
    let (mut fonts, mut out) = (Dict::new(), Vec::new());
    let drawn = draw_laid_shaped(&mut doc, &item(), &laid, &Dict::new(), &mut fonts, &mut out, |_, id| Ok(boxed(0.5 + f64::from(id) * 0.01)));
    assert!(drawn.is_ok(), "{drawn:?}");
    assert_eq!(fonts.len(), 1);
    // The injected outlines are fixed; only the descriptor's family comes from craft-fonts.
    // Check that metadata separately, then normalize those two fields to the original golden.
    // ActualText is new metadata: check each line's value and remove only its added wrappers
    // before hashing. Drawing bytes and font objects still match the original golden exactly.
    let family = pdfcraft_fonts::document_arabic_font().map_or("Arabic", |face| face.family);
    for (_, font) in fonts.iter() {
        let descriptor = doc.dict(font).unwrap().get(b"FontDescriptor").and_then(Object::as_ref).unwrap();
        let dict = doc.dict(&Object::Ref(descriptor)).unwrap();
        assert_eq!(dict.name(b"FontName"), Some(family.replace(' ', "").as_bytes()));
        assert_eq!(dict.get(b"FontFamily").and_then(Object::as_string).unwrap().bytes, family.as_bytes());
        doc.update_dict(descriptor, |d| {
            d.set(b"FontName".to_vec(), Object::name("Arabic"));
            d.set(b"FontFamily".to_vec(), Object::String(PdfString::literal(b"Arabic".to_vec())));
        })
        .unwrap();
    }
    let mut bytes = out.clone();
    for (line, ..) in &laid {
        let hex: String = line.encode_utf16().map(|u| format!("{u:04X}")).collect();
        let header = format!("/Span <</ActualText <FEFF{hex}>>> BDC\n").into_bytes();
        let start = bytes.windows(header.len()).position(|w| w == header).expect("each line has its logical text");
        bytes.drain(start..start + header.len());
        let end = bytes.windows(4).position(|w| w == b"EMC\n").expect("each ActualText span closes");
        bytes.drain(end..end + 4);
    }
    pdfcraft_cos::serialize(&Object::Dict(fonts.clone()), &mut bytes);
    bytes.extend(golden_snapshot(&doc));
    assert_eq!(fnv1a(&bytes), SMALL_ITEM_FINGERPRINT, "small-item output changed");
}

#[test]
fn long_text_with_few_distinct_glyphs_needs_one_font() {
    // 5,000 clusters that repeat three glyphs: one font, and one outline read per glyph.
    let clusters: Vec<ShapedCluster> = (0..5_000u32).map(|i| cluster(1 + i % 3, 0.5)).collect();
    let laid = vec![line(clusters, false)];
    let reads = Cell::new(0usize);
    let mut doc = Document::new_empty();
    let (mut fonts, mut out) = (Dict::new(), Vec::new());
    let drawn = draw_laid_shaped(&mut doc, &item(), &laid, &Dict::new(), &mut fonts, &mut out, |_, _| {
        reads.set(reads.get() + 1);
        Ok(boxed(0.5))
    });
    assert!(drawn.is_ok(), "{drawn:?}");
    assert_eq!(fonts_of(&doc, &fonts).iter().map(|(_, glyphs)| *glyphs).collect::<Vec<_>>(), [3]);
    assert_eq!(reads.get(), 3);
    assert_eq!(glyphs_shown(&out).len(), 5_000);
}

#[test]
fn distinct_clusters_past_the_old_limit_share_their_glyph_outlines() {
    // 3,841 different clusters (their advances differ) built from four glyphs: four outlines are read.
    let clusters: Vec<ShapedCluster> = (0..3_841u32)
        .map(|i| ShapedCluster {
            script: ShapedScript::Arabic,
            glyphs: vec![(1 + i % 4, [0.0, 0.0])],
            advance: 0.5 + f64::from(i) * 1e-6,
            text: "ب".into(),
        })
        .collect();
    let laid = vec![line(clusters, false)];
    let reads = Cell::new(0usize);
    let mut doc = Document::new_empty();
    let (mut fonts, mut out) = (Dict::new(), Vec::new());
    let drawn = draw_laid_shaped(&mut doc, &item(), &laid, &Dict::new(), &mut fonts, &mut out, |_, _| {
        reads.set(reads.get() + 1);
        Ok(boxed(0.5))
    });
    assert!(drawn.is_ok(), "{drawn:?}");
    assert_eq!(fonts.len(), 17);
    assert_eq!(reads.get(), 4);
    assert_eq!(glyphs_shown(&out).len(), 3_841);
}

#[test]
fn hostile_clusters_draw_empty_or_fail_without_changing_the_document() {
    // A cluster with no glyphs and no text draws nothing, but it is still one glyph of a font.
    let empty = ShapedCluster { script: ShapedScript::Arabic, glyphs: Vec::new(), advance: 0.0, text: String::new() };
    let laid = vec![line(vec![empty, cluster(1, 0.5)], false)];
    let mut doc = Document::new_empty();
    let (mut fonts, mut out) = (Dict::new(), Vec::new());
    let drawn = draw_laid_shaped(&mut doc, &item(), &laid, &Dict::new(), &mut fonts, &mut out, |_, _| Ok(boxed(0.5)));
    assert!(drawn.is_ok(), "{drawn:?}");
    assert_eq!(glyphs_shown(&out).len(), 2);
    // A glyph id the face doesn't have fails the item before anything is written.
    let before = snapshot(&doc);
    let laid = vec![line(vec![cluster(1, 0.5), cluster(u32::MAX, 0.5)], false)];
    let (mut fonts, mut out) = (Dict::new(), Vec::new());
    let drawn = draw_laid_shaped(&mut doc, &item(), &laid, &Dict::new(), &mut fonts, &mut out, |_, id| {
        if id == u32::MAX { Err(GlyphError::Missing) } else { Ok(boxed(0.5)) }
    });
    assert!(drawn.is_err());
    assert_eq!(snapshot(&doc), before);
    assert!(fonts.is_empty() && out.is_empty());
}

#[test]
fn a_font_name_the_page_already_uses_is_not_reused() {
    let laid = vec![line(vec![cluster(1, 0.5)], false)];
    let mut doc = Document::new_empty();
    let (mut fonts, mut out) = (Dict::new(), Vec::new());
    let drawn = draw_laid_shaped(&mut doc, &item(), &laid, &Dict::new(), &mut fonts, &mut out, |_, _| Ok(boxed(0.5)));
    assert!(drawn.is_ok(), "{drawn:?}");
    let first = fonts_of(&doc, &fonts)[0].0.clone();
    // The same item on a page whose resources already use that name.
    let mut taken = Dict::new();
    taken.set(first.clone(), Object::Null);
    let mut doc = Document::new_empty();
    let (mut fonts, mut out) = (Dict::new(), Vec::new());
    let drawn = draw_laid_shaped(&mut doc, &item(), &laid, &taken, &mut fonts, &mut out, |_, _| Ok(boxed(0.5)));
    assert!(drawn.is_ok(), "{drawn:?}");
    let second = fonts_of(&doc, &fonts)[0].0.clone();
    assert_ne!(second, first);
    assert!(second.starts_with(b"PCAr"), "{}", String::from_utf8_lossy(&second));
}

#[test]
fn wrapping_keeps_indentation_repeated_spaces_and_logical_text() {
    for text in ["  A  B  ", "   ", "A  B C  D", "  مرحبا  بالعالم  "] {
        let t = AddedText { text: text.into(), rect: [0.0, 0.0, 55.0, 100.0], ..item() };
        assert_eq!(lines(&t).concat(), text, "wrapping must retain every original character");
    }
}

#[test]
fn an_overwide_arabic_word_wraps_without_detaching_marks() {
    if pdfcraft_fonts::document_arabic_font().is_none() {
        return;
    }
    let text = "بَ".repeat(32);
    let t = AddedText { text: text.clone(), rect: [0.0, 0.0, 42.0, 100.0], ..item() };
    let wrapped = wrapped(&t);
    assert!(wrapped.len() > 1, "a long Arabic word must fit a narrow box");
    assert_eq!(wrapped.iter().map(|(line, _)| line.as_str()).collect::<String>(), text);
    for (line, rtl) in wrapped {
        assert!(line.starts_with('ب') && line.ends_with('\u{064e}'), "{line:?}");
        assert!(shaped_width(&t, &line, rtl) <= 42.01, "line exceeds box: {line:?}");
    }
}

#[test]
fn joining_controls_stay_in_the_arabic_shaping_run() {
    if pdfcraft_fonts::document_arabic_font().is_none() {
        return;
    }
    for text in ["ب\u{200c}ب", "ب\u{200d}ب", "PDF ب\u{200c}ب 123", "\u{202e}ب\u{200d}ب\u{202c}"] {
        let t = AddedText { text: text.into(), ..item() };
        assert_eq!(first_undrawable(&t), None, "{text:?}");
        let rtl = unicode_bidi::get_base_direction(text) == unicode_bidi::Direction::Rtl;
        let (pieces, _) = shaped_layout(&t, text, rtl).unwrap();
        assert!(
            pieces.iter().all(|p| match p {
                Piece::Latin(s) => !s.contains(['\u{200c}', '\u{200d}']),
                Piece::Shaped(_) => true,
            }),
            "join controls must reach the shaper"
        );
    }
}

#[test]
fn copied_invisible_controls_draw_without_placeholders_and_keep_their_actual_text() {
    if pdfcraft_fonts::document_arabic_font().is_none() {
        return;
    }
    for control in ['\u{200b}', '\u{2060}', '\u{034f}', '\u{fe0f}', '\u{00ad}', '\u{feff}'] {
        for text in [format!("ب{control}ب"), format!("A{control}B"), control.to_string()] {
            let t = AddedText { text: text.clone(), ..item() };
            assert_eq!(first_undrawable(&t), None, "{text:?}");
            let plain = AddedText { text: text.replace(control, ""), ..t.clone() };
            assert!((text_width(&t) - text_width(&plain)).abs() < 1e-6, "{text:?}");
            let mut doc = Document::new_empty();
            let (mut fonts, mut content) = (Dict::new(), Vec::new());
            draw_shaped(&mut doc, &t, &Dict::new(), &mut fonts, &mut content).unwrap();
            let hex: String = text.encode_utf16().map(|u| format!("{u:04X}")).collect();
            assert!(String::from_utf8(content).unwrap().contains(&format!("/ActualText <FEFF{hex}>")));
        }
    }
    assert_eq!(first_undrawable(&AddedText { text: "نص 😀".into(), ..item() }), Some('😀'));
}

#[test]
fn soft_hyphens_are_invisible_until_a_continuation_break_and_keep_the_original_text() {
    for (text, expected) in [("one\u{200b}two", "one"), ("one\u{00ad}two", "one-")] {
        let width = Family::Helvetica.width("one-", 12.0, false, false);
        let t = AddedText { text: text.into(), size: 12.0, rect: [0.0, 0.0, width, 100.0], ..item() };
        let laid = laid_lines(&t).unwrap();
        assert_eq!(laid.iter().map(|(s, _, _, _)| s.as_str()).collect::<String>(), text);
        let shown: String = laid[0].2.iter().filter_map(|p| if let Piece::Latin(s) = p { Some(s.as_str()) } else { None }).collect();
        assert_eq!(shown, expected);
        assert!(laid.iter().all(|(_, _, _, w)| *w <= width + 1e-6));
    }
}

#[test]
fn directional_override_survives_on_added_text_continuation_lines() {
    let t = AddedText { text: "\u{202e}ABC DEF GHI JKL\u{202c}".into(), size: 12.0, rect: [0.0, 0.0, 30.0, 200.0], ..item() };
    let laid = laid_lines(&t).unwrap();
    let middle = laid.iter().find(|(text, _, _, _)| text.contains("DEF")).unwrap();
    let drawn: String = middle.2.iter().filter_map(|piece| if let Piece::Latin(s) = piece { Some(s.as_str()) } else { None }).collect();
    assert!(drawn.contains("FED"), "wrapped override lost its paragraph context: {drawn:?}");
    assert_eq!(laid.iter().map(|(text, _, _, _)| text.as_str()).collect::<String>(), t.text);
}

#[test]
fn mixed_isolate_continuations_use_the_original_paragraph_levels() {
    if pdfcraft_fonts::document_arabic_font().is_none() {
        return;
    }
    let t =
        AddedText {
            text: "LTR \u{2067}ابت 123 ABC ابت 456 DEF ابت 789 GHI\u{2069} end".into(), size: 12.0, rect: [0.0, 0.0, 55.0, 300.0], ..item()
        };
    let bidi = ParagraphBidiInfo::new(&t.text, Some(Level::ltr()));
    let laid = laid_lines(&t).unwrap();
    assert!(laid.len() > 3);
    let mut start = 0usize;
    for (logical, _, pieces, width) in &laid {
        let end = start + logical.len();
        let display = pdfcraft_fonts::appearance_text::display_line(logical, end < t.text.len());
        let expected: String = bidi.reorder_line(start..start + display.len()).chars().filter(|c| !is_bidi_control(*c)).collect();
        let actual: String = pieces
            .iter()
            .map(|piece| match piece {
                Piece::Latin(s) => s.clone(),
                Piece::Shaped(clusters) => clusters.iter().map(|c| c.text.as_str()).collect(),
            })
            .collect();
        assert_eq!(actual, expected, "isolate context on logical row {logical:?}");
        assert!(*width <= 55.0 + 1e-6);
        start = end;
    }
    assert_eq!(start, t.text.len());
}

#[test]
fn a_separator_after_an_exactly_fitting_word_does_not_add_a_blank_soft_line() {
    let width = Family::Helvetica.width("ABC", 12.0, false, false);
    let t = AddedText { text: "ABC DEF GHI".into(), size: 12.0, rect: [0.0, 0.0, width, 200.0], ..item() };
    let laid = laid_lines(&t).unwrap();
    assert_eq!(laid.iter().map(|(text, _, _, _)| text.as_str()).collect::<String>(), t.text);
    assert_eq!(laid.len(), 3, "a word separator must remain on the preceding logical row");
    assert!(laid.iter().all(|(text, _, _, width)| !text.trim().is_empty() && *width <= t.rect[2] + 1e-6));
    let t = AddedText { align: Align::Right, ..t };
    let mut doc = Document::new_empty();
    let (content, _) = draw(&mut doc, &Content::Text(t), [1.0, 0.0, 0.0, 1.0, 0.0, 0.0], &Dict::new(), false).unwrap();
    let ops = pdfcraft_content::parse(&content).ops;
    assert_eq!(ops.iter().filter(|op| op.is("Tj")).count(), 3);
    assert!(
        ops.iter().filter(|op| op.is("Tm")).all(|op| op.num(4).unwrap() >= -1e-6),
        "hanging separators must not move right-aligned ink outside its box"
    );
    assert!(String::from_utf8(content).unwrap().contains("/ActualText <FEFF0041004200430020>"), "copy must keep the hanging separator");
    for text in ["ABC ", "ABC   "] {
        let t = AddedText { text: text.into(), size: 12.0, rect: [0.0, 0.0, width, 200.0], ..item() };
        let laid = laid_lines(&t).unwrap();
        assert_eq!(laid.len(), 1, "a final invisible suffix must not become a blank soft row");
        assert_eq!(laid[0].0, text);
        assert!(laid[0].3 <= width + 1e-6);
        let wide = AddedText { rect: [0.0, 0.0, 500.0, 200.0], ..t };
        assert_eq!(laid_lines(&wide).unwrap()[0].3, wide.family.width(text, wide.size, wide.bold, wide.italic));
    }
}

#[test]
fn tall_injected_marks_stay_inside_the_box_and_do_not_overlap_the_next_line() {
    let t = item();
    let laid = vec![line(vec![cluster(1, 0.5)], true), line(vec![cluster(1, 0.5)], true)];
    let mut doc = Document::new_empty();
    let (mut fonts, mut out) = (Dict::new(), Vec::new());
    draw_laid_shaped(&mut doc, &t, &laid, &Dict::new(), &mut fonts, &mut out, |_, _| {
        Ok(GlyphOutline { contours: vec![vec![[0.0, -0.6], [0.5, -0.6], [0.5, 1.7], [0.0, 1.7]]], width: 0.5, bbox: [0.0, -0.6, 0.5, 1.7] })
    })
    .unwrap();
    let ys: Vec<f64> = pdfcraft_content::parse(&out).ops.into_iter().filter(|op| op.is("Tm")).map(|op| op.num(5).unwrap()).collect();
    assert_eq!(ys.len(), 2);
    assert!(ys[0] + 1.7 * t.size <= t.rect[3], "the upper mark is outside the text box");
    assert!(ys[1] + 1.7 * t.size < ys[0] - 0.6 * t.size, "adjacent lines' marks overlap");
}

#[test]
fn vocalized_added_text_ink_fits_its_reported_height_after_drawing() {
    if pdfcraft_fonts::document_arabic_font().is_none() {
        return;
    }
    let phrase = "بِسْمِ اللَّهِ الرَّحْمَٰنِ الرَّحِيمِ";
    let t = AddedText { text: format!("{phrase}\n{phrase}"), size: 40.0, rect: [40.0, 300.0, 570.0, 700.0], align: Align::Right, ..item() };
    let rect = text_rect(&t);
    let mut doc = Document::new_empty();
    let (mut fonts, mut out) = (Dict::new(), Vec::new());
    draw_shaped(&mut doc, &t, &Dict::new(), &mut fonts, &mut out).unwrap();
    let (mut name, mut baseline) = (Vec::new(), 0.0);
    let mut checked = 0;
    for op in pdfcraft_content::parse(&out).ops {
        if op.is("Tf") {
            name = op.name(0).unwrap().to_vec();
        }
        if op.is("Tm") {
            baseline = op.num(5).unwrap();
        }
        if op.is("Tj") && name.starts_with(b"PCAr") {
            let code = op.operands[0].as_string().unwrap().bytes[0];
            let font = doc.dict(fonts.get(&name).unwrap()).unwrap();
            let glyph = font.get(b"CharProcs").unwrap().as_dict().unwrap().get(format!("g{code:02X}").as_bytes()).unwrap();
            let object = doc.resolve(glyph);
            let Object::Stream(stream) = object.as_ref() else { panic!() };
            let d1 = pdfcraft_content::parse(&stream.decoded().unwrap()).ops.remove(0);
            let bottom = baseline + d1.num(3).unwrap() * t.size / 1000.0;
            let top = baseline + d1.num(5).unwrap() * t.size / 1000.0;
            assert!(bottom >= rect[1] - 1e-3 && top <= rect[3] + 1e-3, "ink {bottom}..{top} outside {rect:?}");
            checked += 1;
        }
    }
    assert!(checked > 0);
}

/// Thai is drawn by the shaped writer, whose rows grow for stacked marks (a vowel above with a
/// tone mark over it); the box reported for the item is as tall as the rows it draws, so
/// selection, hit testing and the resize handles cover the ink.
#[test]
fn a_thai_items_box_is_as_tall_as_the_rows_it_draws() {
    if pdfcraft_fonts::document_thai_font().is_none() {
        return;
    }
    let t = AddedText {
        rect: [0.0, 0.0, 400.0, 300.0], text: "กิ่ ปู่ ผู้ใช้\nน้ำ ปั้น".into(), size: 40.0, ..AddedText::default()
    };
    let drawn: f64 = laid_lines(&t)
        .unwrap()
        .iter()
        .map(|(_, _, pieces, _)| {
            let extents = line_extents(pieces, |cluster| pdfcraft_fonts::appearance_text::cluster_ink_bounds(cluster).ok());
            (extents[0] + extents[1]) * t.size
        })
        .sum();
    let r = text_rect(&t);
    assert!(drawn > 2.0 * 1.2 * t.size, "the fixture's stacked marks need more than the fixed row height: {drawn}");
    assert!((r[3] - r[1] - drawn).abs() < 1e-6, "box {} for rows {drawn}", r[3] - r[1]);
}
