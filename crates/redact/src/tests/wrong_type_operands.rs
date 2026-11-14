//! An operator whose operands aren't of the types it takes is skipped by the renderer, and changes
//! nothing: after `70 Tc /x Tc` the character spacing is still 70. Read as a reset to a default,
//! such an operator moved where redaction placed glyphs, and the colour it reads cleared a pattern
//! the renderer still paints with.

use super::*;

/// How many glyphs applying a mark over `rect` removes from a page drawing `content`.
fn glyphs(content: &[u8], rect: [f64; 4]) -> usize {
    let mut doc = one_page(content, "", vec![]);
    mark(&mut doc, 0, &[rect], "");
    apply(&mut doc, None).unwrap().glyphs
}

/// The text state is as the operator skipped left it. Each glyph after it stands where the
/// renderer draws it, under the mark (A, B 5 points wide at 10 points): with the spacing, scaling,
/// leading or rise reset, it stood elsewhere and stayed.
#[test]
fn a_text_state_operator_of_the_wrong_type_changes_nothing() {
    for (content, rect) in [
        (&b"BT /F1 10 Tf 70 Tc /x Tc 20 20 Td (AB) Tj ET"[..], [90.0, 15.0, 110.0, 35.0]),
        (b"BT /F1 10 Tf 70 Tc Tc 20 20 Td (AB) Tj ET", [90.0, 15.0, 110.0, 35.0]),
        (b"BT /F1 10 Tf 70 Tw /x Tw 20 20 Td (A B) Tj ET", [97.0, 15.0, 110.0, 35.0]),
        (b"BT /F1 10 Tf 1500 Tz /x Tz 20 20 Td (AB) Tj ET", [100.0, 15.0, 110.0, 35.0]),
        (b"BT /F1 10 Tf 20 200 Td 100 TL /x TL T* (A) Tj ET", [15.0, 95.0, 30.0, 115.0]),
        (b"BT /F1 10 Tf 20 200 Td -100 Ts /x Ts (A) Tj ET", [15.0, 95.0, 30.0, 115.0]),
    ] {
        assert_eq!(glyphs(content, rect), 1, "{}", String::from_utf8_lossy(content));
    }
}

/// `Tf` takes a font and a size; with a string for the font, the renderer skips it whole, and the
/// size stays 10. Read as a size of 40 alone, the glyphs grew to reach a mark above the line.
#[test]
fn a_font_operator_of_the_wrong_type_changes_nothing() {
    assert_eq!(glyphs(b"BT /F1 10 Tf (x) 40 Tf 20 20 Td (AB) Tj ET", [0.0, 30.0, 300.0, 50.0]), 0);
    assert_eq!(glyphs(b"BT /F1 10 Tf /F1 40 Tf 20 20 Td (AB) Tj ET", [0.0, 30.0, 300.0, 50.0]), 2, "a size that reads is taken");
}

/// Text made invisible (`3 Tr`) stays invisible after `/x Tr`, so Remove Hidden Information takes
/// it. Read as mode 0, it was taken for drawn, and the hidden text stayed. `Q` gives back the mode
/// before it, so the text after is drawn and stays.
#[test]
fn invisible_text_stays_invisible_after_a_render_mode_of_the_wrong_type() {
    use crate::sanitize::{Hidden, remove_hidden};
    let mut doc = one_page(b"q BT /F1 10 Tf 3 Tr /x Tr 20 260 Td (HIDDENSECRET) Tj ET Q BT /F1 10 Tf 20 20 Td (PUBLIC) Tj ET", "", vec![]);
    let removed = remove_hidden(&mut doc, &[Hidden::HiddenText]);
    assert!(removed.as_ref().is_ok_and(|r| r.iter().any(|(h, n)| *h == Hidden::HiddenText && *n > 0)), "{removed:?}");
    let c = content(&doc, 0);
    assert!(!c.contains("HIDDENSECRET") && c.contains("PUBLIC"), "{c}");
}

/// A colour operator whose operands don't read is skipped, so the pattern selected before it is
/// still the colour: the fill after it paints with the pattern, which stays once its other paint
/// under the mark went. Read as a plain colour, it cleared the pattern, which was pruned. A colour
/// that reads clears it.
#[test]
fn a_colour_operator_of_the_wrong_type_keeps_the_pattern() {
    for (colour, pruned) in [
        ("/x g", 0),
        ("g", 0),
        ("/x 0 0 rg", 0),
        ("0 0 /x k", 0),
        ("0 /x 0 1 k", 0),
        ("/CSR 5 cs", 0),
        ("cs", 0),
        ("0 g", 1),
        ("0 0 0 rg", 1),
        ("0 0 0 1 k", 1),
        ("/DeviceGray cs", 1),
    ] {
        let mut doc = one_page(
            format!("/Pattern cs /P0 scn 10 260 20 20 re f {colour} 200 20 50 50 re f").as_bytes(),
            "/Pattern << /P0 7 0 R >>",
            vec![stream("/PatternType 1 /PaintType 1 /TilingType 1 /BBox [0 0 10 10] /XStep 10 /YStep 10 /Resources << >>", b"0 0 5 5 re f")],
        );
        mark(&mut doc, 0, &[[0.0, 250.0, 40.0, 290.0]], "");
        let r = apply(&mut doc, None).unwrap();
        assert_eq!((r.paths_removed, r.patterns_removed), (1, pruned), "{colour}");
    }
    // The stroking forms, for a stroke.
    for (colour, pruned) in [("/x G", 0), ("/x 0 0 RG", 0), ("0 0 /x K", 0), ("0 /x 0 1 K", 0), ("/CSR 5 CS", 0), ("0 G", 1)] {
        let mut doc = one_page(
            format!("/Pattern CS /P0 SCN 10 260 m 30 280 l S {colour} 200 20 m 250 70 l S").as_bytes(),
            "/Pattern << /P0 7 0 R >>",
            vec![stream("/PatternType 1 /PaintType 1 /TilingType 1 /BBox [0 0 10 10] /XStep 10 /YStep 10 /Resources << >>", b"0 0 5 5 re f")],
        );
        mark(&mut doc, 0, &[[0.0, 250.0, 40.0, 290.0]], "");
        let r = apply(&mut doc, None).unwrap();
        assert_eq!((r.paths_removed, r.patterns_removed), (1, pruned), "{colour}");
    }
}

/// `'` and `"` whose operands aren't of their types are skipped by the renderer: no line moves,
/// no spacing changes, no text. The text after them stands where the renderer draws it, under the
/// mark (A, B at x 20 to 30, y 20): read as moving the line, it stood 50 below and stayed.
#[test]
fn a_quote_operator_of_the_wrong_type_moves_nothing() {
    for content in [
        &b"BT /F1 10 Tf 20 20 Td 50 TL 5 ' (AB) Tj ET"[..],
        b"BT /F1 10 Tf 20 20 Td 50 TL /x ' (AB) Tj ET",
        b"BT /F1 10 Tf 20 20 Td 50 TL /x 70 (zz) \" (AB) Tj ET",
        b"BT /F1 10 Tf 20 20 Td 50 TL 0 /x (zz) \" (AB) Tj ET",
    ] {
        assert_eq!(glyphs(content, [15.0, 15.0, 40.0, 35.0]), 2, "{}", String::from_utf8_lossy(content));
    }
}

/// A `"` the renderer skips, with a mark where the walk would have placed its text, is left as it
/// is: rewritten into `Tw`, `Tc`, `T*` and `TJ`, it would draw text the renderer never drew.
#[test]
fn a_quote_operator_of_the_wrong_type_under_a_mark_is_left_as_it_is() {
    let mut doc = one_page(b"BT /F1 10 Tf 20 200 Td 14 TL /x 70 (AB) \" ET", "", vec![]);
    mark(&mut doc, 0, &[[15.0, 180.0, 27.0, 196.0]], "");
    assert_eq!(apply(&mut doc, None).unwrap().glyphs, 0);
    let c = content(&doc, 0);
    assert!(!c.contains("TJ") && !c.contains("T*"), "{c}");
}

/// The Type 3 cleanup skips a `Tf` without a size as the renderer does: the glyph after it is
/// drawn in the Type 3 font before it, so the font stays on the page with its glyph. Read as a
/// switch to the other font, the cleanup unlinked the glyph and dropped the font.
#[test]
fn a_type3_font_stays_after_a_font_operator_of_the_wrong_type() {
    for tf in ["/F1 /x Tf", "/F1 (x) Tf"] {
        let mut doc = pdf(vec![
            b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
            b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_vec(),
            b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 300 300] /Contents 4 0 R /Resources << /Font << /F1 5 0 R /T3 7 0 R >> >> >>".to_vec(),
            stream("", format!("BT /T3 10 Tf 20 20 Td {tf} (a) Tj ET BT /F1 10 Tf 20 200 Td (SECRET) Tj ET").as_bytes()),
            FONT.replace("95 0 R", "6 0 R").into_bytes(),
            widths(),
            b"<< /Type /Font /Subtype /Type3 /FontBBox [0 0 10 10] /FontMatrix [0.1 0 0 0.1 0 0] /CharProcs << /a 8 0 R >> \
               /Encoding << /Differences [97 /a] >> /FirstChar 97 /LastChar 97 /Widths [10] >>"
                .to_vec(),
            stream("", b"10 0 d0 0 0 10 10 re f"),
        ]);
        mark(&mut doc, 0, &[[0.0, 190.0, 300.0, 220.0]], "");
        apply(&mut doc, None).unwrap();
        let page = pdfcraft_model::pages(&doc).swap_remove(0);
        let res = page.dict.get(b"Resources").map(|r| doc.resolve(r)).and_then(|r| r.as_dict().cloned()).unwrap_or_default();
        let fonts = res.get(b"Font").map(|f| doc.resolve(f)).and_then(|f| f.as_dict().cloned()).unwrap_or_default();
        assert!(fonts.contains(b"T3"), "{tf}: the page still lists the Type 3 font: {fonts:?}");
        let font = doc.get(ObjRef::new(7, 0)).as_dict().cloned().unwrap_or_default();
        let procs = font.get(b"CharProcs").map(|p| doc.resolve(p)).and_then(|p| p.as_dict().cloned()).unwrap_or_default();
        assert!(procs.contains(b"a"), "{tf}: the glyph drawn stays: {font:?}");
    }
}

/// A render mode the renderer doesn't know (`9 Tr`, `-1 Tr`) it fills with, as mode 0: text in it
/// paints with the fill pattern, which stays once its other paint under the mark went.
#[test]
fn an_unknown_render_mode_fills() {
    for mode in ["9 Tr", "-1 Tr"] {
        let mut doc = one_page(
            format!("/Pattern cs /P0 scn 10 260 20 20 re f BT /F1 10 Tf {mode} 200 20 Td (AB) Tj ET").as_bytes(),
            "/Pattern << /P0 7 0 R >>",
            vec![stream("/PatternType 1 /PaintType 1 /TilingType 1 /BBox [0 0 10 10] /XStep 10 /YStep 10 /Resources << >>", b"0 0 5 5 re f")],
        );
        mark(&mut doc, 0, &[[0.0, 250.0, 40.0, 290.0]], "");
        let r = apply(&mut doc, None).unwrap();
        assert_eq!((r.paths_removed, r.patterns_removed), (1, 0), "{mode}");
    }
}

/// A `Tj` whose operand isn't a string is skipped by the renderer: read as a `TJ` adjustment, its
/// number moved the pen 500 points, and the text after it stood off the page, under no mark.
#[test]
fn a_show_operator_of_the_wrong_type_moves_nothing() {
    assert_eq!(glyphs(b"BT /F1 10 Tf 20 20 Td -50000 Tj (AB) Tj ET", [15.0, 15.0, 40.0, 35.0]), 2);
}

/// `/F2 /x Tf` is skipped whole: the text after it is laid out in `/F1` (A and B 5 points wide), so
/// a mark beside it takes nothing. Read as a switch to `/F2` (glyphs 20 points wide), B stood under
/// the mark and went. `/F2 10 Tf` switches.
#[test]
fn a_font_name_without_a_size_switches_no_font() {
    let page = |tf: &str| {
        pdf(vec![
            b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
            b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_vec(),
            b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 300 300] /Contents 4 0 R /Resources << /Font << /F1 5 0 R /F2 7 0 R >> >> >>".to_vec(),
            stream("", format!("BT /F1 10 Tf {tf} 20 20 Td (AB) Tj ET").as_bytes()),
            FONT.replace("95 0 R", "6 0 R").into_bytes(),
            widths(),
            FONT.replace("95 0 R", "8 0 R").into_bytes(),
            format!("[{}]", vec!["2000"; 95].join(" ")).into_bytes(),
        ])
    };
    let removed = |tf: &str| {
        let mut doc = page(tf);
        mark(&mut doc, 0, &[[32.0, 15.0, 58.0, 35.0]], "");
        apply(&mut doc, None).unwrap().glyphs
    };
    assert_eq!(removed("/F2 /x Tf"), 0);
    assert!(removed("/F2 10 Tf") > 0, "a font with a size is switched to");
}

/// The Type 3 cleanup counts no glyph for a `"` the renderer skips: the only glyph drawn is under
/// the mark, so the font has none left and leaves the page. Counted as drawing, its program stayed.
/// A `"` whose spacings are numbers draws, a lone `-` being one (0), and the font stays.
#[test]
fn a_type3_glyph_only_a_skipped_quote_operator_shows_is_not_kept() {
    for (quote, kept) in [("/x 70 (a) \"", false), ("0 - (a) \"", true), ("0 0 (a) \"", true)] {
        assert_eq!(type3_kept(quote), kept, "{quote}");
    }
}

/// Whether the Type 3 font `/T3` stays on a page that shows its glyph `a` under a mark and with
/// `show` elsewhere.
fn type3_kept(show: &str) -> bool {
    let mut doc = pdf(vec![
        b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
        b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_vec(),
        b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 300 300] /Contents 4 0 R /Resources << /Font << /F1 5 0 R /T3 7 0 R >> >> >>".to_vec(),
        stream("", format!("BT /T3 10 Tf 20 260 Td (a) Tj ET BT /T3 10 Tf 20 20 Td {show} ET").as_bytes()),
        FONT.replace("95 0 R", "6 0 R").into_bytes(),
        widths(),
        b"<< /Type /Font /Subtype /Type3 /FontBBox [0 0 10 10] /FontMatrix [0.1 0 0 0.1 0 0] /CharProcs << /a 8 0 R >> \
           /Encoding << /Differences [97 /a] >> /FirstChar 97 /LastChar 97 /Widths [10] >>"
            .to_vec(),
        stream("", b"10 0 d0 0 0 10 10 re f"),
    ]);
    mark(&mut doc, 0, &[[0.0, 200.0, 300.0, 300.0]], "");
    apply(&mut doc, None).unwrap();
    let page = pdfcraft_model::pages(&doc).swap_remove(0);
    let res = page.dict.get(b"Resources").map(|r| doc.resolve(r)).and_then(|r| r.as_dict().cloned()).unwrap_or_default();
    let fonts = res.get(b"Font").map(|f| doc.resolve(f)).and_then(|f| f.as_dict().cloned()).unwrap_or_default();
    fonts.contains(b"T3")
}

/// A lone `-` is the number 0 to the renderer, as to the content parser: `- Tc` sets the spacing
/// to 0 and `- Tr` the mode to fill, not left as they were, and a `"` with it among its spacings
/// draws. With the sign dropped, the B was taken for 70 points on, the visible text for hidden and
/// removed, and the quoted text for not drawn.
#[test]
fn a_lone_minus_is_zero() {
    use crate::sanitize::{Hidden, remove_hidden};
    assert_eq!(glyphs(b"BT /F1 10 Tf 70 Tc - Tc 20 20 Td (AB) Tj ET", [26.0, 15.0, 29.0, 35.0]), 1);
    assert_eq!(glyphs(b"BT /F1 10 Tf 20 100 Td 50 TL 0 - (AB) \" ET", [15.0, 45.0, 40.0, 65.0]), 2);
    let mut doc = one_page(b"BT /F1 10 Tf 3 Tr - Tr 20 260 Td (VISIBLE) Tj ET", "", vec![]);
    let _ = remove_hidden(&mut doc, &[Hidden::HiddenText]);
    assert!(content(&doc, 0).contains("VISIBLE"), "{}", content(&doc, 0));
}
