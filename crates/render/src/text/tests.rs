use super::*;

fn em_box(text: &str, x0: f32, y0: f32, x1: f32, y1: f32) -> TextGlyph {
    TextGlyph { text: text.into(), rect: [x0, y0, x1, y1], quad: [[x0, y0], [x1, y0], [x1, y1], [x0, y1]], direction: [1.0, 0.0] }
}

#[test]
fn subscript_stays_in_its_word() {
    // "CO2-E": an 8.1 pt "CO" and "-E", with a 4.7 pt "2" drawn 2.7 pt lower (#3174).
    let glyphs = vec![
        em_box("C", 0.0, 92.5, 5.0, 100.6),
        em_box("O", 5.0, 92.5, 11.0, 100.6),
        em_box("2", 11.0, 97.9, 13.5, 102.6),
        em_box("-", 13.5, 92.5, 16.0, 100.6),
        em_box("E", 16.0, 92.5, 21.0, 100.6),
    ];
    assert_eq!(layout(glyphs).plain_text(), "CO2-E");
}

#[test]
fn identity_v_columns_are_searchable_in_reading_order() {
    use lopdf::{Document, Object, Stream, dictionary};
    for encoding in ["Identity-V", "UniJIS-UCS2-V"] {
        let mut doc = Document::with_version("1.7");
        let cmap = b"/CIDInit /ProcSet findresource begin 12 dict begin begincmap /CIDSystemInfo << /Registry (Adobe) /Ordering (UCS) /Supplement 0 >> def /CMapName /TestUnicode def /CMapType 2 def 1 begincodespacerange <0000> <FFFF> endcodespacerange 5 beginbfchar <0001> <65E5> <0002> <672C> <0003> <8A9E> <0004> <6587> <0005> <7AE0> endbfchar endcmap CMapName currentdict /CMap defineresource pop end end";
        let cmap = if encoding == "Identity-V" {
            cmap.to_vec()
        } else {
            let mut mapping = String::from_utf8(cmap.to_vec()).unwrap();
            for (code, unicode) in [("0001", "65E5"), ("0002", "672C"), ("0003", "8A9E"), ("0004", "6587"), ("0005", "7AE0")] {
                mapping = mapping.replace(&format!("<{code}> <{unicode}>"), &format!("<{unicode}> <{unicode}>"));
            }
            mapping.into_bytes()
        };
        let unicode = doc.add_object(Stream::new(dictionary! {}, cmap));
        let cid = doc.add_object(dictionary! {
        "Type" => "Font", "Subtype" => "CIDFontType2", "BaseFont" => "TestVertical",
        "CIDSystemInfo" => dictionary! { "Registry" => Object::string_literal("Adobe"), "Ordering" => Object::string_literal("Identity"), "Supplement" => 0 },
        "DW" => 1000, "DW2" => vec![Object::Integer(880), Object::Integer(-1000)], "CIDToGIDMap" => "Identity"
    });
        let font = doc.add_object(dictionary! { "Type" => "Font", "Subtype" => "Type0", "BaseFont" => "TestVertical", "Encoding" => encoding, "DescendantFonts" => vec![Object::Reference(cid)], "ToUnicode" => unicode });
        let horizontal = doc.add_object(dictionary! { "Type" => "Font", "Subtype" => "Type0", "BaseFont" => "TestVertical", "Encoding" => "Identity-H", "DescendantFonts" => vec![Object::Reference(cid)], "ToUnicode" => unicode });
        let (first, second) = if encoding == "Identity-V" { ("000100020003", "00040005") } else { ("65E5672C8A9E", "65877AE0") };
        let operators =
            format!("BT /H1 20 Tf 1 0 0 1 40 280 Tm <{first}> Tj /F1 20 Tf 1 0 0 1 200 250 Tm <{first}> Tj 1 0 0 1 170 250 Tm <{second}> Tj ET");
        let content = doc.add_object(Stream::new(dictionary! {}, operators.into_bytes()));
        let pages = doc.new_object_id();
        let page = doc.add_object(dictionary! { "Type" => "Page", "Parent" => pages, "MediaBox" => vec![Object::Integer(0), Object::Integer(0), Object::Integer(300), Object::Integer(300)], "Resources" => dictionary! {"Font" => dictionary! {"F1" => font, "H1" => horizontal}}, "Contents" => content });
        doc.objects.insert(pages, dictionary! {"Type" => "Pages", "Kids" => vec![Object::Reference(page)], "Count" => 1}.into());
        let catalog = doc.add_object(dictionary! {"Type" => "Catalog", "Pages" => pages});
        doc.trailer.set("Root", catalog);
        let mut bytes = Vec::new();
        doc.save_to(&mut bytes).unwrap();
        let pdf = Pdf::new(std::sync::Arc::new(bytes)).unwrap();
        let text = extract_page(&pdf, 0, &InterpreterSettings::default()).unwrap();
        assert_eq!(text.plain_text(), "日本語\n日本語\n文章");
        assert_eq!(text.find("日本語"), vec![0..3, 3..6]);
        assert_eq!(text.find("文章"), vec![6..8]);
        let rects = text.line_rects(3..6);
        assert_eq!(rects.len(), 1);
        assert!(rects[0][3] - rects[0][1] > rects[0][2] - rects[0][0], "a vertical search highlight: {rects:?}");
        assert!(text.glyphs[3].rect[0] > text.glyphs[6].rect[0], "columns read right to left in page view coordinates");
    }
}

/// WSCAD writes `/ToUnicode /Identity-H`, a name rather than a CMap stream, on Type 0 fonts
/// whose two-byte codes are UTF-16 code units. The codes read as that Unicode text (as in
/// Acrobat), so the page's text can be selected and found; before, it had none. Controls (here a
/// DEL and a C1 control at the line ends) are not text.
#[test]
fn identity_to_unicode_names_give_text() {
    use lopdf::{Document, Object, Stream, dictionary};
    for name in ["Identity-H", "Identity-V"] {
        let mut doc = Document::with_version("1.4");
        let cid = doc.add_object(dictionary! {
            "Type" => "Font", "Subtype" => "CIDFontType2", "BaseFont" => "RobotoMono-Regular",
            "CIDSystemInfo" => dictionary! { "Registry" => Object::string_literal("Adobe"), "Ordering" => Object::string_literal("Identity"), "Supplement" => 0 },
            "DW" => 600, "CIDToGIDMap" => "Identity"
        });
        let font = doc.add_object(dictionary! { "Type" => "Font", "Subtype" => "Type0", "BaseFont" => "RobotoMono-Regular", "Encoding" => "Identity-H", "DescendantFonts" => vec![Object::Reference(cid)], "ToUnicode" => name });
        let operators = "BT /F1 12 Tf 1 0 0 1 20 250 Tm <004400650063006B0062006C006100740074007F> Tj 1 0 0 1 20 200 Tm <005A00FC0072006900630068002020AC0085> Tj ET";
        let content = doc.add_object(Stream::new(dictionary! {}, operators.as_bytes().to_vec()));
        let pages = doc.new_object_id();
        let page = doc.add_object(dictionary! { "Type" => "Page", "Parent" => pages, "MediaBox" => vec![Object::Integer(0), Object::Integer(0), Object::Integer(300), Object::Integer(300)], "Resources" => dictionary! {"Font" => dictionary! {"F1" => font}}, "Contents" => content });
        doc.objects.insert(pages, dictionary! {"Type" => "Pages", "Kids" => vec![Object::Reference(page)], "Count" => 1}.into());
        let catalog = doc.add_object(dictionary! {"Type" => "Catalog", "Pages" => pages});
        doc.trailer.set("Root", catalog);
        let mut bytes = Vec::new();
        doc.save_to(&mut bytes).unwrap();
        let pdf = Pdf::new(std::sync::Arc::new(bytes)).unwrap();
        let text = extract_page(&pdf, 0, &InterpreterSettings::default()).unwrap();
        assert_eq!(text.plain_text(), "Deckblatt\nZürich €", "{name}");
        assert_eq!(text.find("zürich"), vec![9..15], "{name}: glyphs 9 to 14");
    }
}

/// One page per text matrix, each showing `codes` in Helvetica whose `/ToUnicode` maps the
/// codes in `unicode` (a `bfchar` body) to several characters.
fn ligature_pages(unicode: &str, codes: &str, matrices: &[&str]) -> Pdf {
    use lopdf::{Document, Object, Stream, dictionary};
    let mut doc = Document::with_version("1.7");
    let cmap = format!(
        "/CIDInit /ProcSet findresource begin 12 dict begin begincmap /CIDSystemInfo << /Registry (Adobe) /Ordering (UCS) /Supplement 0 >> def /CMapName /TestLigatures def /CMapType 2 def 1 begincodespacerange <00> <FF> endcodespacerange {unicode} endcmap CMapName currentdict /CMap defineresource pop end end"
    );
    let to_unicode = doc.add_object(Stream::new(dictionary! {}, cmap.into_bytes()));
    let font = doc.add_object(dictionary! { "Type" => "Font", "Subtype" => "Type1", "BaseFont" => "Helvetica", "ToUnicode" => to_unicode });
    let pages = doc.new_object_id();
    let mut kids = Vec::new();
    for m in matrices {
        let content = doc.add_object(Stream::new(dictionary! {}, format!("BT /F1 20 Tf {m} Tm ({codes}) Tj ET").into_bytes()));
        let page = doc.add_object(dictionary! { "Type" => "Page", "Parent" => pages, "MediaBox" => vec![Object::Integer(0), Object::Integer(0), Object::Integer(300), Object::Integer(300)], "Resources" => dictionary! {"Font" => dictionary! {"F1" => font}}, "Contents" => content });
        kids.push(Object::Reference(page));
    }
    let count = kids.len() as i64;
    doc.objects.insert(pages, dictionary! {"Type" => "Pages", "Kids" => kids, "Count" => count}.into());
    let catalog = doc.add_object(dictionary! {"Type" => "Catalog", "Pages" => pages});
    doc.trailer.set("Root", catalog);
    let mut bytes = Vec::new();
    doc.save_to(&mut bytes).unwrap();
    Pdf::new(std::sync::Arc::new(bytes)).unwrap()
}

#[test]
fn a_ligature_is_one_glyph_in_every_writing_direction() {
    // "o", one glyph for "ffi", "c", "e": "office", turned 0°, 90°, 180° and 270°. The same
    // codes without the ligature mapping ("oWce") are the control for the glyph box.
    let turns = ["1 0 0 1 100 150", "0 1 -1 0 150 100", "-1 0 0 -1 200 150", "0 -1 1 0 150 200"];
    let ligature = ligature_pages("1 beginbfchar <57> <006600660069> endbfchar", "oWce", &turns);
    let control = ligature_pages("", "oWce", &turns);
    for (page, turn) in turns.iter().enumerate() {
        let text = extract_page(&ligature, page, &InterpreterSettings::default()).unwrap();
        let plain = extract_page(&control, page, &InterpreterSettings::default()).unwrap();
        assert_eq!(plain.plain_text(), "oWce", "Tm {turn}");
        assert_eq!(text.plain_text(), "office", "Tm {turn}");
        assert_eq!(text.glyphs.iter().map(|g| g.text.as_str()).collect::<Vec<_>>(), ["o", "ffi", "c", "e"], "Tm {turn}");
        assert_eq!(text.glyphs[1].rect, plain.glyphs[1].rect, "the painted glyph's box, Tm {turn}");
        assert_eq!(text.find("ffi"), vec![1..2], "Tm {turn}");
        assert_eq!(text.find("of"), vec![0..2], "a match ending inside the ligature covers it, Tm {turn}");
        assert_eq!(text.line_rects(0..4).len(), 1, "one line, Tm {turn}");
    }
}

/// One page per content stream, in Helvetica (`/F1`), with `/P1` in the resources'
/// `/Properties` naming the `/ActualText` "fi".
fn marked_pages(contents: &[&str]) -> Pdf {
    use lopdf::{Document, Object, Stream, dictionary};
    let mut doc = Document::with_version("1.7");
    let font = doc.add_object(dictionary! { "Type" => "Font", "Subtype" => "Type1", "BaseFont" => "Helvetica" });
    let pages = doc.new_object_id();
    let mut kids = Vec::new();
    for c in contents {
        let content = doc.add_object(Stream::new(dictionary! {}, c.as_bytes().to_vec()));
        let resources = dictionary! {
            "Font" => dictionary! {"F1" => font},
            "Properties" => dictionary! {"P1" => dictionary! {"ActualText" => Object::string_literal("fi")}},
        };
        let page = doc.add_object(dictionary! { "Type" => "Page", "Parent" => pages, "MediaBox" => vec![Object::Integer(0), Object::Integer(0), Object::Integer(300), Object::Integer(300)], "Resources" => resources, "Contents" => content });
        kids.push(Object::Reference(page));
    }
    let count = kids.len() as i64;
    doc.objects.insert(pages, dictionary! {"Type" => "Pages", "Kids" => kids, "Count" => count}.into());
    let catalog = doc.add_object(dictionary! {"Type" => "Catalog", "Pages" => pages});
    doc.trailer.set("Root", catalog);
    let mut bytes = Vec::new();
    doc.save_to(&mut bytes).unwrap();
    Pdf::new(std::sync::Arc::new(bytes)).unwrap()
}

fn marked_text(pdf: &Pdf, page: usize) -> PageText {
    extract_page(pdf, page, &InterpreterSettings::default()).unwrap()
}

#[test]
fn redraws_of_a_long_replacement_share_a_text_budget() {
    let long = "0041".repeat(32 * 1024);
    let redraws = 200;
    let content = format!(
        "BT /F1 20 Tf 1 0 0 1 100 150 Tm /Span <</ActualText <FEFF{long}>>> BDC (x) Tj EMC {} ET",
        "1 0 0 1 100 150 Tm (x) Tj ".repeat(redraws)
    );
    let pdf = marked_pages(&[&content]);
    REDRAW_TEXT_BYTES.with(|c| c.set(0));
    let text = marked_text(&pdf, 0);
    let given = REDRAW_TEXT_BYTES.with(|c| c.get());
    assert!(given <= MAX_REDRAW_TEXT, "{given} bytes of replacement text given to {redraws} redraws");
    assert!(text.plain_text().starts_with("AAAA"));
    // A short replacement still covers its redraw.
    let pdf = marked_pages(&["BT /F1 20 Tf 1 0 0 1 100 150 Tm /Span <</ActualText (fi)>> BDC (x) Tj EMC 1 0 0 1 100 150 Tm (x) Tj ET"]);
    REDRAW_TEXT_BYTES.with(|c| c.set(0));
    assert_eq!(marked_text(&pdf, 0).plain_text(), "fi");
    assert_eq!(REDRAW_TEXT_BYTES.with(|c| c.get()), 2);
}

#[test]
fn text_after_an_inline_image_survives_a_non_ascii_byte_before_a_later_ei() {
    // Everyday content both ways: a string with a byte above 0x7F, and a later word holding
    // a capital "EI". The end-of-data look-ahead used to read the string's bytes as image
    // data, so the image swallowed the rest of the page.
    let pdf = marked_pages(&[
        "BI /W 1 /H 1 /BPC 8 /CS /DeviceGray ID \0 EI BT /F1 20 Tf 1 0 0 1 20 200 Tm (Gr\u{fc}\u{df}e) Tj 1 0 0 1 20 150 Tm (EINGANG) Tj ET",
    ]);

    assert!(marked_text(&pdf, 0).plain_text().contains("EINGANG"), "the text after the inline image was not extracted");
}

#[test]
fn actual_text_replaces_the_glyphs_it_marks_in_every_writing_direction() {
    // Chrome marks each shaped Thai cluster this way: "น้ำ" (U+0E19 U+0E49 U+0E33) here stands
    // for the two glyphs "xy", between "a" and "b". The same codes unmarked are the control.
    let turns = ["1 0 0 1 100 150", "0 1 -1 0 150 100", "-1 0 0 -1 200 150", "0 -1 1 0 150 200"];
    let marked: Vec<String> =
        turns.iter().map(|m| format!("BT /F1 20 Tf {m} Tm (a) Tj /Span <</ActualText <FEFF0E190E490E33>>> BDC (xy) Tj EMC (b) Tj ET")).collect();
    let plain: Vec<String> = turns.iter().map(|m| format!("BT /F1 20 Tf {m} Tm (axyb) Tj ET")).collect();
    let marked = marked_pages(&marked.iter().map(String::as_str).collect::<Vec<_>>());
    let plain = marked_pages(&plain.iter().map(String::as_str).collect::<Vec<_>>());
    for (page, turn) in turns.iter().enumerate() {
        let text = marked_text(&marked, page);
        let control = marked_text(&plain, page);
        assert_eq!(control.plain_text(), "axyb", "Tm {turn}");
        assert_eq!(text.plain_text(), "aน้ำb", "Tm {turn}");
        assert_eq!(text.glyphs.iter().map(|g| g.text.as_str()).collect::<Vec<_>>(), ["a", "น้ำ", "b"], "Tm {turn}");
        assert_eq!(text.find("น้ำ"), vec![1..2], "Tm {turn}");
        assert_eq!(text.line_rects(0..3).len(), 1, "one line, Tm {turn}");
        // The replacement's box is the union of the boxes of the glyphs it replaces.
        let (x, y) = (&control.glyphs[1].rect, &control.glyphs[2].rect);
        let union = [x[0].min(y[0]), x[1].min(y[1]), x[2].max(y[2]), x[3].max(y[3])];
        for (got, want) in text.glyphs[1].rect.iter().zip(union) {
            assert!((got - want).abs() < 0.01, "box {:?} ≠ {union:?}, Tm {turn}", text.glyphs[1].rect);
        }
        assert_eq!(text.glyphs[1].direction, control.glyphs[1].direction, "Tm {turn}");
    }
}

#[test]
fn redraw_dedupe_work_is_bounded_on_hostile_pages() {
    // 100,000 replacements and 100,000 unmarked glyphs that never match, all at one spot:
    // unbounded, every replacement would compare every glyph (10^10 comparisons).
    const N: usize = 100_000;
    let glyph = |text: &str| {
        let quad = [[100.0, 110.0], [110.0, 110.0], [110.0, 100.0], [100.0, 100.0]];
        (0, [1.0, 0.0], TextGlyph { text: text.into(), rect: [100.0, 100.0, 110.0, 110.0], quad, direction: [1.0, 0.0] })
    };
    let mut dev = TextDevice::new();
    // A genuine redraw first: it is still deduplicated within the budget.
    dev.glyphs.push(glyph("fi"));
    dev.glyphs.push(glyph("x"));
    dev.replaced.push(Replaced { at: Some(0), text: "fi".into(), originals: vec![("x".into(), [100.0, 100.0, 110.0, 110.0])] });
    dev.glyphs.extend((0..N).map(|_| glyph("y")));
    for _ in 0..N {
        dev.glyphs.push(glyph("z"));
        let at = dev.glyphs.len() - 1;
        dev.replaced.push(Replaced { at: Some(at), text: "z".into(), originals: vec![("q".into(), [100.0, 100.0, 110.0, 110.0])] });
    }
    let started = std::time::Instant::now();
    dev.finish();
    assert!(started.elapsed() < std::time::Duration::from_secs(30), "took {:?}", started.elapsed());
    assert_eq!(dev.glyphs.len(), 2 * N + 2, "unmatched glyphs are kept");
    assert_eq!(dev.glyphs[1].2.text, "fi", "the redraw found within the budget takes the replacement");
}

#[test]
fn actual_text_follows_the_pdf_rules_for_marked_content() {
    let pdf = marked_pages(&[
        // Named in the resources' /Properties.
        "BT /F1 20 Tf 1 0 0 1 100 150 Tm (o) Tj /Span /P1 BDC (W) Tj EMC (ce) Tj ET",
        // Nested: the outermost replacement covers everything inside it.
        "BT /F1 20 Tf 1 0 0 1 100 150 Tm /Span <</ActualText (AB)>> BDC /Span <</ActualText (Z)>> BDC (x) Tj EMC (y) Tj EMC ET",
        // Empty: the marked glyphs stand for no text (a soft hyphen at a line break); the gap
        // the hyphen leaves still reads as one.
        "BT /F1 20 Tf 1 0 0 1 100 150 Tm (co) Tj /Span <</ActualText ()>> BDC (-) Tj EMC (op) Tj ET",
        // Across two lines: each glyph keeps its own text, so selection and lines still work.
        "BT /F1 20 Tf 1 0 0 1 100 150 Tm /Span <</ActualText (Q)>> BDC (x) Tj 0 -40 Td (y) Tj EMC ET",
        // Unbalanced: a stray EMC first, and a replacement the page never closes.
        "EMC BT /F1 20 Tf 1 0 0 1 100 150 Tm (a) Tj /Span <</ActualText (fi)>> BDC (xy) Tj ET",
        // Marked content without /ActualText changes nothing.
        "BT /F1 20 Tf 1 0 0 1 100 150 Tm /P <</MCID 0>> BDC (ab) Tj EMC /Artifact BMC (c) Tj EMC ET",
    ]);
    let plain: Vec<String> = (0..6).map(|p| marked_text(&pdf, p).plain_text()).collect();
    assert_eq!(plain, ["ofice", "AB", "co op", "x\ny", "afi", "abc"]);
}

#[test]
fn an_unmarked_redraw_of_actual_text_reads_as_the_same_text() {
    // Canva draws text twice (an outline or shadow) and marks only one drawing. The copy,
    // before or after the marked one and half a point off, takes the same replacement and is
    // dropped as a redraw; the same codes elsewhere on the page keep their own text.
    let marked = "BT /F1 20 Tf 1 0 0 1 100 150 Tm (a) Tj /Span <</ActualText <FEFF0E190E490E33>>> BDC (xy) Tj EMC (b) Tj ET";
    let copy = "BT /F1 20 Tf 1 0 0 1 100.5 150.5 Tm (axyb) Tj ET";
    let elsewhere = "BT /F1 20 Tf 1 0 0 1 100 50 Tm (axyb) Tj ET";
    let pdf = marked_pages(&[&format!("{marked} {copy} {elsewhere}"), &format!("{copy} {marked} {elsewhere}")]);
    for page in 0..2 {
        assert_eq!(marked_text(&pdf, page).plain_text(), "aน้ำb\naxyb", "page {page}");
    }
}

/// Issue #3109: text drawn in clipping mode (`7 Tr`) sets the clip for a subsequent fill,
/// shading or image without painting the glyphs directly, and must still reach `TextDevice`
/// with the same boxes and search ranges as modes 0–6.
#[test]
fn clipping_mode_text_is_extracted_and_searchable_like_other_rendering_modes() {
    let fill_through = " 0 0 1 rg 0 0 300 300 re f";
    let mut pages: Vec<String> =
        (0..=7).map(|mode| format!("q BT /F1 24 Tf {mode} Tr 72 150 Td () Tj [(Hello) -200 (World)] TJ ET{fill_through} Q")).collect();
    // Page 8: /ActualText wrapping 7 Tr clip-only text.
    pages.push(format!("q /Span <</ActualText (Hello World)>> BDC BT /F1 24 Tf 7 Tr 72 150 Td (Glyphs) Tj ET EMC{fill_through} Q"));
    // Page 9: 0 Tr underpainting followed by 7 Tr at the same position (outline/gradient overlay) deduplicates.
    pages.push(format!("BT /F1 24 Tf 0 Tr 72 150 Td (Hello World) Tj ET q BT /F1 24 Tf 7 Tr 72 150 Td (Hello World) Tj ET{fill_through} Q"));
    // Page 10: 90° turned 7 Tr text.
    pages.push(format!("q BT /F1 24 Tf 7 Tr 0 1 -1 0 150 100 Tm (Hello World) Tj ET{fill_through} Q"));
    // Page 11: 7 Tr without any fill-through (visually blank, still extractable).
    pages.push("BT /F1 24 Tf 7 Tr 72 150 Td (Hello World) Tj ET".into());

    let refs: Vec<&str> = pages.iter().map(String::as_str).collect();
    let pdf = marked_pages(&refs);
    let control = marked_text(&pdf, 0);
    assert_eq!(control.plain_text(), "Hello World");
    let control_rects = control.line_rects(0..5);
    assert_eq!(control_rects.len(), 1);
    for mode in 0..=7 {
        let text = marked_text(&pdf, mode);
        assert_eq!(text.plain_text(), "Hello World", "mode {mode} Tr");
        assert_eq!(text.find("hello"), vec![0..5], "mode {mode} Tr");
        assert_eq!(text.find_opts("Hello", true, true), vec![0..5], "mode {mode} Tr");
        assert_eq!(
            text.find_with(|chars| chars.starts_with(&['H', 'e', 'l', 'l', 'o']).then_some(0..5).into_iter().collect()),
            vec![0..5],
            "mode {mode} Tr"
        );
        assert_eq!(text.line_rects(0..5), control_rects, "mode {mode} Tr");
        assert_eq!(text.glyphs, control.glyphs, "mode {mode} Tr");
    }
    let actual = marked_text(&pdf, 8);
    assert_eq!(actual.plain_text(), "Hello World");
    assert_eq!(actual.find("Hello"), vec![0..1]);

    let deduped = marked_text(&pdf, 9);
    assert_eq!(deduped.plain_text(), "Hello World");

    let turned = marked_text(&pdf, 10);
    assert_eq!(turned.plain_text(), "Hello World");
    assert_eq!(turned.find("World"), vec![6..11]);

    let unfilled = marked_text(&pdf, 11);
    assert_eq!(unfilled.plain_text(), "Hello World");

    // Visual check: page 7 (`7 Tr` + blue fill-through) paints blue only inside the clipped
    // glyph outlines, whereas page 11 (`7 Tr` alone) paints no ink at all.
    let mut renderer = crate::PageRenderer::new(std::sync::Arc::new(pdf.data().as_ref().to_vec()), crate::RenderConfig::default());
    let filled_px = renderer.render(crate::RenderRequest { page: 7, kind: crate::RequestKind::Pixels, scale: 1.0, ..Default::default() });
    assert!(filled_px.error.is_none());
    let blue_pixels = filled_px.rgba.as_chunks::<4>().0.iter().filter(|p| p[0] < 32 && p[1] < 32 && p[2] > 200).count();
    let white_pixels = filled_px.rgba.as_chunks::<4>().0.iter().filter(|p| p[..3] == [255, 255, 255]).count();
    assert!(blue_pixels > 50 && white_pixels > 80_000, "blue={blue_pixels}, white={white_pixels}");

    let unfilled_px = renderer.render(crate::RenderRequest { page: 11, kind: crate::RequestKind::Pixels, scale: 1.0, ..Default::default() });
    assert!(unfilled_px.error.is_none());
    assert!(unfilled_px.rgba.as_chunks::<4>().0.iter().all(|p| p[..3] == [255, 255, 255]));
}

#[test]
fn clipping_mode_filter_keeps_titles_and_excludes_ocr() {
    let pdf = marked_pages(&[
        "q BT /F1 24 Tf 7 Tr 30 200 Td (Clipped heading) Tj ET 0 0 1 rg 0 0 300 300 re f Q BT /F1 12 Tf 3 Tr 30 100 Td (Hidden OCR) Tj ET",
        "q /Span <</ActualText (Accessible heading)>> BDC BT /F1 24 Tf 7 Tr 30 200 Td (Clipped heading) Tj ET EMC 0 0 1 rg 0 0 300 300 re f Q",
    ]);
    assert!(marked_text(&pdf, 0).plain_text().contains("Hidden OCR"));
    for (page, expected) in [(0, "Clipped heading"), (1, "Accessible heading")] {
        let text = extract_page_filtered(&pdf, page, &InterpreterSettings::default(), TextFilter { images: true, ..Default::default() }).unwrap();
        assert_eq!(text.plain_text(), expected);
        assert!(!text.find("heading").is_empty());
    }
}

/// ISO 32000-2 §9.3.6: Type 3 text under `7 Tr` is not rendered (vendored hayro-interpret
/// patch 64), but it is still clipping text, not an invisible text layer: the filter that
/// drops OCR layers keeps it, as it keeps clipping text in other fonts.
#[test]
fn clipping_mode_type3_text_is_kept_by_the_ocr_filter() {
    let font = "<< /Type /Font /Subtype /Type3 /FontBBox [0 0 1000 1000] /FontMatrix [0.001 0 0 0.001 0 0] \
                    /CharProcs << /A 6 0 R >> /Encoding << /Type /Encoding /Differences [65 /A] >> \
                    /FirstChar 65 /LastChar 65 /Widths [1000] >>";
    let glyph = "1000 0 100 0 900 800 d1 100 0 800 800 re f";
    let content = "BT /T3 24 Tf 7 Tr 30 200 Td (AAA) Tj ET BT /T3 12 Tf 3 Tr 30 100 Td (A) Tj ET";
    let objects = [
        "<< /Type /Catalog /Pages 2 0 R >>".to_string(),
        "<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_string(),
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 300 300] /Contents 4 0 R /Resources << /Font << /T3 5 0 R >> >> >>".to_string(),
        format!("<< /Length {} >>\nstream\n{content}\nendstream", content.len()),
        font.to_string(),
        format!("<< /Length {} >>\nstream\n{glyph}\nendstream", glyph.len()),
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
    bytes.extend_from_slice(format!("trailer << /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n", objects.len() + 1).as_bytes());
    let pdf = Pdf::new(std::sync::Arc::new(bytes)).unwrap();
    let all = extract_page_filtered(&pdf, 0, &InterpreterSettings::default(), TextFilter::default()).unwrap();
    assert_eq!(all.plain_text().matches('A').count(), 4, "{:?}", all.plain_text());
    let filtered = extract_page_filtered(&pdf, 0, &InterpreterSettings::default(), TextFilter { images: true, ..Default::default() }).unwrap();
    assert_eq!(filtered.plain_text().trim(), "AAA", "the 7 Tr text stays, the 3 Tr layer goes");
}

/// One page per `(encoding, content)`, in Helvetica (`/F1`).
fn encoded_pages(pages: &[(lopdf::Object, &[u8])]) -> Pdf {
    use lopdf::{Document, Object, Stream, dictionary};
    let mut doc = Document::with_version("1.7");
    let pages_id = doc.new_object_id();
    let mut kids = Vec::new();
    for (encoding, content) in pages {
        let font = doc.add_object(dictionary! { "Type" => "Font", "Subtype" => "Type1", "BaseFont" => "Helvetica", "Encoding" => encoding.clone() });
        let content = doc.add_object(Stream::new(dictionary! {}, content.to_vec()));
        let page = doc.add_object(dictionary! { "Type" => "Page", "Parent" => pages_id, "MediaBox" => vec![Object::Integer(0), Object::Integer(0), Object::Integer(300), Object::Integer(300)], "Resources" => dictionary! {"Font" => dictionary! {"F1" => font}}, "Contents" => content });
        kids.push(Object::Reference(page));
    }
    let count = kids.len() as i64;
    doc.objects.insert(pages_id, dictionary! {"Type" => "Pages", "Kids" => kids, "Count" => count}.into());
    let catalog = doc.add_object(dictionary! {"Type" => "Catalog", "Pages" => pages_id});
    doc.trailer.set("Root", catalog);
    let mut bytes = Vec::new();
    doc.save_to(&mut bytes).unwrap();
    Pdf::new(std::sync::Arc::new(bytes)).unwrap()
}

/// The Annex D.2 alias codes extract as the ink they draw, not as formatting characters.
///
/// WinAnsiEncoding gives 0xAD and 0xA0 the glyph names `hyphen` and `space`, the names it
/// also gives 0x2D and 0x20, and MacRomanEncoding gives 0xCA `space`. Reporting the Adobe
/// Glyph List values U+00AD and U+00A0 for them hid visible text: a soft hyphen is invisible,
/// so "well-known" written that way matched no search and no redaction pattern.
/// `XiaoBiaoSong.pdf` in the pdf.js corpus drew 104 of them.
///
/// A `/Differences` entry naming `sfthyphen` or `nbspace` is a deliberate statement about one
/// code and keeps its glyph list value. pdftotext draws the same line: on this fixture it
/// reads the base-encoded codes as "a-b a b" and the named ones as U+00AD.
#[test]
fn the_annex_d2_alias_codes_read_as_an_ordinary_hyphen_and_space() {
    use lopdf::{Object, dictionary};
    let name = |n: &str| Object::Name(n.as_bytes().to_vec());
    let differences = Object::Dictionary(dictionary! {
        "Type" => "Encoding",
        "BaseEncoding" => "WinAnsiEncoding",
        "Differences" => vec![Object::Integer(88), name("sfthyphen"), Object::Integer(89), name("nbspace")],
    });
    let pdf = encoded_pages(&[
        (name("WinAnsiEncoding"), &b"BT /F1 20 Tf 1 0 0 1 20 150 Tm (a\xadb\xa0c) Tj ET"[..]),
        (name("MacRomanEncoding"), &b"BT /F1 20 Tf 1 0 0 1 20 150 Tm (a\xcab) Tj ET"[..]),
        (differences, &b"BT /F1 20 Tf 1 0 0 1 20 150 Tm (aXbYc) Tj ET"[..]),
    ]);
    let texts = |page| extract_page(&pdf, page, &InterpreterSettings::default()).unwrap().glyphs.iter().map(|g| g.text.clone()).collect::<Vec<_>>();
    assert_eq!(texts(0), ["a", "-", "b", " ", "c"], "WinAnsi 0xAD and 0xA0");
    assert_eq!(texts(1), ["a", " ", "b"], "MacRoman 0xCA");
    assert_eq!(texts(2), ["a", "\u{ad}", "b", "\u{a0}", "c"], "a named glyph keeps its glyph list value");
}

/// #3169, #3170: glyph names outside the Adobe Glyph List still read as text. Ligature
/// (`f_i`) and suffixed (`hyphen.case`) names go by the glyph list specification's full
/// mapping. A name that only numbers the glyph (`c251` small caps, `C39`, `C174`, `C39.sc`)
/// reads as nothing, not as a guess from the code (review on #3239: with `/C1` at 233,
/// StandardEncoding's 233 would read `Ø` for a glyph drawn as `é`), and so do a ZapfDingbats
/// name (`a12`) and a name with no meaning left (`foo`).
#[test]
fn glyph_names_outside_the_glyph_list_still_read_as_text() {
    use lopdf::{Object, dictionary};
    let name = |n: &str| Object::Name(n.as_bytes().to_vec());
    let differences = Object::Dictionary(dictionary! {
        "Type" => "Encoding",
        "Differences" => vec![
            Object::Integer(30), name("f_i"), name("f_l"), Object::Integer(2), name("hyphen.case"),
            Object::Integer(39), name("C39"), Object::Integer(97), name("c251"), Object::Integer(174), name("C174"),
            Object::Integer(88), name("a12"), name("foo"), name("uni00660069"),
            Object::Integer(61), name("C39.sc"), Object::Integer(233), name("C1"),
        ],
    });
    // One glyph per line: the test font has none of these glyphs, so they have no width, and
    // on one line a later `fi` would sit on the first and be dropped as its fake-bold copy.
    let mut content = b"BT /F1 20 Tf 20 290 Td".to_vec();
    for code in *b"\x1e\x1f\x02'a\xaeXYZ=\xe9" {
        content.extend_from_slice(b" (");
        content.push(code);
        content.extend_from_slice(b") Tj 0 -25 Td");
    }
    content.extend_from_slice(b" ET");
    let pdf = encoded_pages(&[(differences, &content[..])]);
    let text = extract_page(&pdf, 0, &InterpreterSettings::default()).unwrap().plain_text();
    let lines: Vec<&str> = text.lines().map(str::trim).filter(|l| !l.is_empty()).collect();
    assert_eq!(lines, ["fi", "fl", "-", "fi"], "{text:?}");
}

/// Review on #3239: the glyph list specification's `uni` and `u` forms take uppercase hex
/// digits only, and map a name with lowercase ones to nothing ("uni20ac" in its examples).
/// Names upstream's own lookup can't read (several groups, a suffix) show it: lowercase reads
/// as nothing, uppercase as its characters. A bare `uni20ac` or `u1040c` is still read by
/// upstream's lenient `unicode_from_name`, outside this mapping.
#[test]
fn glyph_names_with_lowercase_hex_read_as_nothing() {
    use lopdf::{Object, dictionary};
    let name = |n: &str| Object::Name(n.as_bytes().to_vec());
    let differences = Object::Dictionary(dictionary! {
        "Type" => "Encoding",
        "Differences" => vec![Object::Integer(65), name("uni20ac0308"), name("uni20AC0308"), name("u1040c.alt"), name("u1040C.alt"), name("f_uni20ac"), name("f_uni20AC")],
    });
    let pdf = encoded_pages(&[(
        differences,
        &b"BT /F1 20 Tf 20 180 Td (A) Tj 0 -25 Td (B) Tj 0 -25 Td (C) Tj 0 -25 Td (D) Tj 0 -25 Td (E) Tj 0 -25 Td (F) Tj ET"[..],
    )]);
    // One glyph per line, so each name's text stays its own (and a name read as nothing
    // leaves no line).
    let text = extract_page(&pdf, 0, &InterpreterSettings::default()).unwrap().plain_text();
    let lines: Vec<&str> = text.lines().map(str::trim).filter(|l| !l.is_empty()).collect();
    assert_eq!(lines, ["\u{20ac}\u{308}", "\u{1040c}", "f", "f\u{20ac}"], "{text:?}");
}

#[test]
fn narrow_ligatures_keep_every_letter() {
    // Split evenly, this "ffi" put its two "f"s so close that the second was dropped as a
    // fake-bold copy of the first.
    let pdf = ligature_pages("1 beginbfchar <62> <006600660069> endbfchar", "obce", &["1 0 0 1 100 150"]);
    let text = extract_page(&pdf, 0, &InterpreterSettings::default()).unwrap();
    assert_eq!(text.plain_text(), "office");
}

#[test]
fn right_to_left_ligatures_keep_their_logical_order() {
    // "سلام" drawn left to right as meem, alef, lam, seen; then with lam-alef as one glyph.
    let letters = ligature_pages("4 beginbfchar <61> <0645> <62> <0644> <63> <0633> <64> <0627> endbfchar", "adbc", &["1 0 0 1 100 150"]);
    let ligature = ligature_pages("3 beginbfchar <61> <0645> <62> <06440627> <63> <0633> endbfchar", "abc", &["1 0 0 1 100 150"]);
    for pdf in [letters, ligature] {
        assert_eq!(extract_page(&pdf, 0, &InterpreterSettings::default()).unwrap().plain_text(), "سلام");
    }
    // "بَاب" with beh and its fatha as one glyph, as Add text writes shaped Arabic: the mark
    // stays after its letter.
    let marked = ligature_pages("3 beginbfchar <61> <0628064E> <62> <0627> <63> <0628> endbfchar", "cba", &["1 0 0 1 100 150"]);
    assert_eq!(extract_page(&marked, 0, &InterpreterSettings::default()).unwrap().plain_text(), "بَاب");
}

#[test]
fn vertical_ligatures_stay_in_their_column() {
    use lopdf::{Document, Object, Stream, dictionary};
    let mut doc = Document::with_version("1.7");
    // CID 2 is one glyph for "株式".
    let cmap = b"/CIDInit /ProcSet findresource begin 12 dict begin begincmap /CIDSystemInfo << /Registry (Adobe) /Ordering (UCS) /Supplement 0 >> def /CMapName /TestUnicode def /CMapType 2 def 1 begincodespacerange <0000> <FFFF> endcodespacerange 3 beginbfchar <0001> <65E5> <0002> <682A5F0F> <0003> <8A9E> endbfchar endcmap CMapName currentdict /CMap defineresource pop end end";
    let unicode = doc.add_object(Stream::new(dictionary! {}, cmap.to_vec()));
    let cid = doc.add_object(dictionary! {
        "Type" => "Font", "Subtype" => "CIDFontType2", "BaseFont" => "TestVertical",
        "CIDSystemInfo" => dictionary! { "Registry" => Object::string_literal("Adobe"), "Ordering" => Object::string_literal("Identity"), "Supplement" => 0 },
        "DW" => 1000, "DW2" => vec![Object::Integer(880), Object::Integer(-1000)], "CIDToGIDMap" => "Identity"
    });
    let font = doc.add_object(dictionary! { "Type" => "Font", "Subtype" => "Type0", "BaseFont" => "TestVertical", "Encoding" => "Identity-V", "DescendantFonts" => vec![Object::Reference(cid)], "ToUnicode" => unicode });
    let content = doc.add_object(Stream::new(dictionary! {}, b"BT /F1 20 Tf 1 0 0 1 200 250 Tm <000100020003> Tj ET".to_vec()));
    let pages = doc.new_object_id();
    let page = doc.add_object(dictionary! { "Type" => "Page", "Parent" => pages, "MediaBox" => vec![Object::Integer(0), Object::Integer(0), Object::Integer(300), Object::Integer(300)], "Resources" => dictionary! {"Font" => dictionary! {"F1" => font}}, "Contents" => content });
    doc.objects.insert(pages, dictionary! {"Type" => "Pages", "Kids" => vec![Object::Reference(page)], "Count" => 1}.into());
    let catalog = doc.add_object(dictionary! {"Type" => "Catalog", "Pages" => pages});
    doc.trailer.set("Root", catalog);
    let mut bytes = Vec::new();
    doc.save_to(&mut bytes).unwrap();
    let pdf = Pdf::new(std::sync::Arc::new(bytes)).unwrap();
    let text = extract_page(&pdf, 0, &InterpreterSettings::default()).unwrap();
    assert_eq!(text.plain_text(), "日株式語");
    assert_eq!(text.glyphs.len(), 3);
    assert_eq!(text.line_rects(0..3).len(), 1, "one column");
}

#[test]
fn upright_mapping_round_trips() {
    let r = [10.0, 20.0, 30.0, 25.0];
    for q in 0..4 {
        assert_eq!(from_upright(to_upright(r, q, 300.0, 200.0), q, 300.0, 200.0), r, "quarter {q}");
    }
    // A 90° page: the view's top-right corner is the upright top-left.
    assert_eq!(to_upright([290.0, 0.0, 300.0, 10.0], 1, 300.0, 200.0), [0.0, 0.0, 10.0, 10.0]);
}

#[test]
fn turkish_dotted_and_dotless_i_match_in_any_case() {
    // "İSTANBUL IRMAK KIŞ" on one line: each word typed in lower case must find it, and the
    // reverse. `to_lowercase` alone made `İ` two characters and left `ı` unmatched.
    let mut v = Vec::new();
    word(&mut v, "İSTANBUL", 10.0, 10.0, 7.0);
    word(&mut v, "IRMAK", 80.0, 10.0, 7.0);
    word(&mut v, "KIŞ", 130.0, 10.0, 7.0);
    let upper = layout(v);
    assert_eq!(upper.plain_text(), "İSTANBUL IRMAK KIŞ");
    assert_eq!(upper.find("istanbul"), vec![0..8]);
    assert_eq!(upper.find("İstanbul"), vec![0..8]);
    assert_eq!(upper.find("ırmak"), vec![8..13]);
    assert_eq!(upper.find("kış"), vec![13..16]);
    assert_eq!(upper.find("istanbul ırmak kış"), vec![0..16], "a phrase across the words");
    assert_eq!(upper.find_opts("ırmak", false, true), vec![8..13], "whole words");
    assert!(upper.find_opts("istanbul", true, false).is_empty(), "case-sensitive search stays exact");
    assert_eq!(upper.find_opts("İSTANBUL", true, false), vec![0..8]);

    let mut v = Vec::new();
    word(&mut v, "istanbul", 10.0, 10.0, 7.0);
    word(&mut v, "ırmak", 80.0, 10.0, 7.0);
    word(&mut v, "kış", 130.0, 10.0, 7.0);
    let lower = layout(v);
    assert_eq!(lower.find("İSTANBUL"), vec![0..8]);
    assert_eq!(lower.find("IRMAK"), vec![8..13]);
    assert_eq!(lower.find("KIŞ"), vec![13..16]);
}

#[test]
fn greek_sigma_forms_match_without_changing_exact_search_or_glyph_ranges() {
    let mut v = Vec::new();
    word(&mut v, "κόσμος", 10.0, 10.0, 7.0);
    word(&mut v, "ΚΌΣΜΟΣ", 80.0, 10.0, 7.0);
    word(&mut v, "σςΣ", 150.0, 10.0, 7.0);
    let text = layout(v);
    assert_eq!(text.plain_text(), "κόσμος ΚΌΣΜΟΣ σςΣ");
    for query in ["κόσμος", "ΚΌΣΜΟΣ"] {
        assert_eq!(text.find(query), vec![0..6, 6..12]);
        assert_eq!(text.find_opts(query, false, true), vec![0..6, 6..12]);
    }
    for query in ["σ", "ς", "Σ"] {
        assert_eq!(text.find(query), vec![2..3, 5..6, 8..9, 11..12, 12..13, 13..14, 14..15]);
        assert!(text.find_opts(query, false, true).is_empty());
    }
    assert_eq!(text.find_opts("κόσμος", true, false), vec![0..6]);
    assert_eq!(text.find_opts("ΚΌΣΜΟΣ", true, false), vec![6..12]);
    assert_eq!(text.find_opts("ς", true, false), vec![5..6, 13..14]);
    assert_eq!(text.find_opts_limited("ς", false, false, 2), vec![2..3, 5..6]);
    assert_eq!(text.line_rects(text.find("κόσμος")[1].clone()), text.line_rects(6..12));
}

#[test]
fn case_folding_keeps_one_character_per_character() {
    for s in ["İSTANBUL", "ırmak", "Hello WORLD", "ÇAĞLAR", "Straße", "ΣΟΦΙΑ", "Σσς", "日本語"] {
        assert_eq!(fold_case(s).chars().count(), s.chars().count(), "{s}");
    }
    assert_eq!(fold_case("Hello WORLD"), "hello world");
    assert_eq!(fold_case("ÇAĞLAR Şişli"), "çağlar şişli");
    assert_eq!(fold_case("İIıi"), "iiii");
    assert_eq!(fold_case("Σσς"), "σσσ");
}

fn g(t: &str, x0: f32, y0: f32, x1: f32) -> TextGlyph {
    let rect = [x0, y0, x1, y0 + 10.0];
    TextGlyph { text: t.into(), rect, quad: [[x0, y0], [x1, y0], [x1, rect[3]], [x0, rect[3]]], direction: [1.0, 0.0] }
}

/// A glyph whose em box is `w` × `h` at `origin`, advancing along `dir` with its up edge along
/// `up` (a unit step of each), as a sheared or turned text matrix draws it.
fn tilted(t: &str, origin: [f32; 2], dir: [f32; 2], up: [f32; 2], w: f32, h: f32) -> TextGlyph {
    let p = |a: f32, b: f32| [origin[0] + dir[0] * a + up[0] * b, origin[1] + dir[1] * a + up[1] * b];
    let quad = [p(0.0, 0.0), p(w, 0.0), p(w, h), p(0.0, h)];
    let xs = quad.map(|q| q[0]);
    let ys = quad.map(|q| q[1]);
    let rect = [
        xs.iter().copied().fold(f32::MAX, f32::min),
        ys.iter().copied().fold(f32::MAX, f32::min),
        xs.iter().copied().fold(f32::MIN, f32::max),
        ys.iter().copied().fold(f32::MIN, f32::max),
    ];
    TextGlyph { text: t.into(), rect, quad, direction: dir }
}

/// #3693: pointing at the middle of a glyph in sheared or turned text finds that glyph, not
/// the one before it whose bounding box overlaps it.
#[test]
fn nearest_finds_the_glyph_under_the_pointer_in_sheared_and_turned_text() {
    let s = std::f32::consts::FRAC_1_SQRT_2;
    // (writing direction, up edge) in view space (y down): upright, sheared 0.35, turned 45°.
    for (dir, up) in [([1.0, 0.0], [0.0, -1.0]), ([1.0, 0.0], [0.35, -1.0]), ([s, -s], [-s, -s])] {
        let mut advance = 0.0;
        let glyphs: Vec<TextGlyph> = "film in the list of tiny jilt flit"
            .chars()
            .map(|c| {
                let w = if "filjt ".contains(c) { 3.0 } else { 6.0 };
                let g = tilted(&c.to_string(), [50.0 + dir[0] * advance, 300.0 + dir[1] * advance], dir, up, w, 14.0);
                advance += w;
                g
            })
            .collect();
        let t = layout(glyphs.clone());
        assert_eq!(t.glyphs.len(), glyphs.len(), "layout kept every glyph");
        for (i, g) in glyphs.iter().enumerate() {
            let cx = g.quad.iter().map(|q| q[0]).sum::<f32>() / 4.0;
            let cy = g.quad.iter().map(|q| q[1]).sum::<f32>() / 4.0;
            assert_eq!(t.nearest(cx, cy), Some(i), "dir {dir:?} up {up:?}: glyph {i} ({})", g.text);
        }
        // Past either end of the line, the end glyph is still the nearest.
        let (first, last) = (&glyphs[0], &glyphs[glyphs.len() - 1]);
        assert_eq!(t.nearest(first.quad[0][0] - dir[0] * 8.0, first.quad[0][1] - dir[1] * 8.0), Some(0));
        assert_eq!(t.nearest(last.quad[1][0] + dir[0] * 8.0, last.quad[1][1] + dir[1] * 8.0), Some(glyphs.len() - 1));
    }
}

#[test]
fn a_degenerate_or_nan_quad_holds_no_point() {
    assert!(!quad_contains(&[[1.0, 1.0]; 4], 1.0, 1.0));
    assert!(!quad_contains(&[[0.0, 0.0], [1.0, 0.0], [f32::NAN, 1.0], [0.0, 1.0]], 0.5, 0.5));
    assert!(quad_contains(&[[0.0, 0.0], [4.0, 0.0], [4.0, 4.0], [0.0, 4.0]], 4.0, 2.0));
    // Wound the other way.
    assert!(quad_contains(&[[0.0, 0.0], [0.0, 4.0], [4.0, 4.0], [4.0, 0.0]], 2.0, 2.0));
}

#[test]
fn lines_words_and_search() {
    // "Hello world" on line 1, "Next" on line 2.
    let mut v = Vec::new();
    for (i, c) in "Hello".chars().enumerate() {
        v.push(g(&c.to_string(), 10.0 + i as f32 * 6.0, 10.0, 16.0 + i as f32 * 6.0));
    }
    for (i, c) in "world".chars().enumerate() {
        v.push(g(&c.to_string(), 50.0 + i as f32 * 6.0, 10.0, 56.0 + i as f32 * 6.0));
    }
    for (i, c) in "Next".chars().enumerate() {
        v.push(g(&c.to_string(), 10.0 + i as f32 * 6.0, 30.0, 16.0 + i as f32 * 6.0));
    }
    let t = layout(v);
    assert_eq!(t.plain_text(), "Hello world\nNext");
    assert_eq!(t.find("WORLD next"), vec![5..14]);
    assert_eq!(t.find("o"), vec![4..5, 6..7]);
    assert_eq!(t.line_rects(3..12).len(), 2);
    assert_eq!(t.nearest(52.0, 14.0), Some(5));
}

/// #1315: a custom match that ends on a reconstructed word or line separator does not take
/// the following glyph.
#[test]
fn a_match_ending_on_a_separator_stops_before_the_next_glyph() {
    let glyphs: Vec<TextGlyph> = "quickbrown"
        .chars()
        .enumerate()
        .map(|(i, c)| {
            let x = i as f32 * 6.0;
            g(&c.to_string(), x, 0.0, x + 6.0)
        })
        .collect();
    for line_break in [false, true] {
        let mut space_before = vec![false; 10];
        space_before[5] = !line_break;
        let line_of = (0..10).map(|i| u32::from(line_break && i >= 5)).collect();
        let page = PageText { glyphs: glyphs.clone(), line_of, space_before };
        let plain = if line_break { "quick\nbrown" } else { "quick brown" };
        assert_eq!(page.plain_text(), plain);
        assert_eq!(page.find("quick "), vec![0..5]);
        let span = |r: std::ops::Range<usize>| Vec::from([r]);
        assert_eq!(page.find_with(|_| span(0..5)), vec![0..5]);
        assert_eq!(page.find_with(|_| span(5..11)), vec![5..10]);
        assert_eq!(page.find_with(|_| span(6..11)), vec![5..10]);
        let hits = page.find_with(|_| span(0..6));
        assert_eq!(page.text_of(hits[0].clone()), "quick");
        assert_eq!(hits, vec![0..5]);
    }
}

#[test]
fn phrase_search_ignores_repeated_spaces() {
    // "quick  brown" drawn with two space glyphs in a row. Text extraction keeps both spaces,
    // but the phrase search has always collapsed runs of whitespace in the query, so the page
    // text has to be collapsed the same way or the phrase is never found (#814).
    let mut v = Vec::new();
    word(&mut v, "quick", 10.0, 10.0, 6.0);
    v.push(g(" ", 46.0, 10.0, 52.0));
    v.push(g(" ", 52.0, 10.0, 58.0));
    word(&mut v, "brown", 58.0, 10.0, 6.0);
    let n = v.len();
    let t = PageText { glyphs: v, line_of: vec![0; n], space_before: vec![false; n] };
    assert_eq!(t.plain_text(), "quick  brown");
    assert_eq!(t.find("quick brown"), vec![0..12]);
    assert_eq!(t.find("quick  brown"), vec![0..12], "the query's own run of spaces is collapsed too");
    assert_eq!(t.find("quick\tbrown"), vec![0..12]);
    assert_eq!(t.find("quick"), vec![0..5]);
    assert_eq!(t.find("brown"), vec![7..12]);
    assert_eq!(t.find_opts_limited("o", false, false, 0), Vec::new());
    assert!(t.find("quick absent").is_empty());
}

#[test]
fn limited_search_stops_after_the_requested_matches() {
    let mut glyphs = Vec::new();
    word(&mut glyphs, "oooo", 10.0, 10.0, 6.0);
    let text = layout(glyphs);
    assert_eq!(text.find_opts_limited("o", false, false, 2), vec![0..1, 1..2]);
    assert_eq!(text.find_opts_limited("o", false, false, 10), vec![0..1, 1..2, 2..3, 3..4]);
}

fn word(v: &mut Vec<TextGlyph>, s: &str, x: f32, y: f32, advance: f32) {
    for (i, c) in s.chars().enumerate() {
        let x0 = x + i as f32 * advance;
        v.push(g(&c.to_string(), x0, y, x0 + 6.0));
    }
}

#[test]
fn fake_bold_duplicates_are_removed() {
    let mut v = Vec::new();
    for (i, c) in "Bold".chars().enumerate() {
        let x = 10.0 + i as f32 * 7.0;
        v.push(g(&c.to_string(), x, 10.0, x + 6.0));
        v.push(g(&c.to_string(), x + 0.3, 10.0, x + 6.3));
    }
    assert_eq!(layout(v).plain_text(), "Bold");
    // A zero-width combining mark redrawn on itself is dropped too.
    let v = vec![g("e", 10.0, 10.0, 16.0), g("\u{301}", 13.0, 10.0, 13.0), g("e", 10.0, 10.0, 16.0), g("\u{301}", 13.0, 10.0, 13.0)];
    assert_eq!(layout(v).plain_text(), "e\u{301}");
}

/// #1467: condensed text (80 Tz and below, OCR layers fitted to the scan) sets narrow letters
/// closer than a fifth of their height apart; a doubled letter there is two letters, not a redraw.
#[test]
fn condensed_double_letters_are_kept() {
    for scale in [0.8, 0.65, 0.5] {
        let mut v = Vec::new();
        let mut x = 10.0;
        // Helvetica advances at 10 pt, in thousandths of an em.
        for (c, adv) in [('A', 667.0), ('l', 222.0), ('l', 222.0), ('e', 556.0)] {
            let w = adv / 100.0 * scale;
            v.push(g(&c.to_string(), x, 10.0, x + w));
            x += w;
        }
        assert_eq!(layout(v.clone()).plain_text(), "Alle", "at {scale} horizontal scaling");
        // Each letter redrawn a fifth of a point to the right is still one letter.
        let bold: Vec<TextGlyph> = v.iter().flat_map(|c| [c.clone(), g(&c.text, c.rect[0] + 0.2, 10.0, c.rect[2] + 0.2)]).collect();
        assert_eq!(layout(bold).plain_text(), "Alle", "fake bold at {scale} horizontal scaling");
    }
}

/// #2457: OCR text layers squeeze each word into its box down to 20 Tz and below, where a
/// narrow letter is less than a twentieth of its height wide. Doubled letters, and letters
/// repeated two or three places on, are still letters there, not redraws.
#[test]
fn ocr_condensed_double_letters_are_kept() {
    // Helvetica advances, in thousandths of an em.
    let adv = |c: char| match c {
        'M' => 833.0,
        'K' | 'B' => 667.0,
        'W' => 944.0,
        'I' | 't' | 'f' => 278.0,
        'i' | 'l' => 222.0,
        'm' => 833.0,
        'r' => 333.0,
        's' => 500.0,
        _ => 556.0,
    };
    for scale in [0.2, 0.1, 0.05] {
        for word in ["Hallo", "Mitte", "Wille", "Kommission", "Illustration", "still", "Brunnen", "Kaffee"] {
            let mut v = Vec::new();
            let mut x = 10.0;
            for c in word.chars() {
                let w = adv(c) / 100.0 * scale;
                v.push(g(&c.to_string(), x, 10.0, x + w));
                x += w;
            }
            assert_eq!(layout(v.clone()).plain_text(), word, "at {scale} horizontal scaling");
            // Each letter drawn twice at the same place is still one letter.
            let bold: Vec<TextGlyph> = v.iter().flat_map(|c| [c.clone(), c.clone()]).collect();
            assert_eq!(layout(bold).plain_text(), word, "fake bold at {scale} horizontal scaling");
            // And redrawn a quarter of its own width to the right (the match reaches half of it).
            let shifted: Vec<TextGlyph> = v
                .iter()
                .flat_map(|c| {
                    let d = (c.rect[2] - c.rect[0]) * 0.25;
                    [c.clone(), g(&c.text, c.rect[0] + d, 10.0, c.rect[2] + d)]
                })
                .collect();
            assert_eq!(layout(shifted).plain_text(), word, "shifted fake bold at {scale} horizontal scaling");
        }
    }
}

#[test]
fn two_columns_read_left_then_right_even_if_drawn_interleaved() {
    let mut v = Vec::new();
    // Drawn row by row across both columns, as some producers do.
    word(&mut v, "Left1", 10.0, 10.0, 6.0);
    word(&mut v, "Right1", 200.0, 10.0, 6.0);
    word(&mut v, "Left2", 10.0, 24.0, 6.0);
    word(&mut v, "Right2", 200.0, 24.0, 6.0);
    let t = layout(v);
    assert_eq!(t.plain_text(), "Left1\nLeft2\nRight1\nRight2");
}

/// Lays out one-word lines, each `(text, x, y)`, 10 pt high: 14 pt apart is a line in a paragraph, 24 pt a
/// paragraph gap.
fn lines(at: &[(&str, f32, f32)]) -> String {
    let mut v = Vec::new();
    for &(s, x, y) in at {
        word(&mut v, s, x, y, 6.0);
    }
    layout(v).plain_text()
}

#[test]
fn a_column_reads_on_past_a_paragraph_gap() {
    // #3658: a heading and paragraph A at the top of the left column, a paragraph gap, then paragraph B, which
    // goes on at the top of the right column, and paragraph C below it.
    let two = [
        ("Heading", 10.0, 10.0),
        ("A1", 10.0, 30.0),
        ("A2", 10.0, 44.0),
        ("A3", 10.0, 58.0),
        ("B1", 10.0, 82.0),
        ("B2", 10.0, 96.0),
        ("B3", 200.0, 30.0),
        ("B4", 200.0, 44.0),
        ("C1", 200.0, 68.0),
        ("C2", 200.0, 82.0),
    ];
    assert_eq!(lines(&two), "Heading\nA1\nA2\nA3\nB1\nB2\nB3\nB4\nC1\nC2");
    // Three columns, each with a gap at a different height.
    let three = [
        ("A1", 10.0, 10.0),
        ("A2", 10.0, 24.0),
        ("B1", 10.0, 48.0),
        ("C1", 150.0, 10.0),
        ("D1", 150.0, 38.0),
        ("D2", 150.0, 52.0),
        ("E1", 290.0, 10.0),
        ("E2", 290.0, 24.0),
        ("E3", 290.0, 38.0),
        ("F1", 290.0, 62.0),
    ];
    assert_eq!(lines(&three), "A1\nA2\nB1\nC1\nD1\nD2\nE1\nE2\nE3\nF1");
}

#[test]
fn a_column_reads_on_past_a_line_that_only_touches_it() {
    // The #3658 page with a right-column paragraph whose first line touches B1's by 1 pt or 0.1 pt, or whose
    // second line carries a 6 pt footnote marker reaching up beside B2: none of them is a row with B1.
    let page = |extra: &[(&str, f32, f32, f32)]| {
        let mut v = Vec::new();
        let left = [("H", 10.0, 10.0), ("A1", 10.0, 30.0), ("A2", 10.0, 44.0), ("A3", 10.0, 58.0), ("B1", 10.0, 82.0), ("B2", 10.0, 96.0)];
        for &(s, x, y) in left.iter().chain(&[("R1", 200.0, 30.0), ("R2", 200.0, 44.0)]) {
            word(&mut v, s, x, y, 6.0);
        }
        for &(s, x, y, h) in extra {
            for (i, c) in s.chars().enumerate() {
                let x0 = x + i as f32 * h * 0.6;
                v.push(em_box(&c.to_string(), x0, y, x0 + h * 0.6, y + h));
            }
        }
        layout(v).plain_text().replace('\n', " ")
    };
    assert_eq!(page(&[("S1", 200.0, 91.0, 10.0), ("S2", 200.0, 105.0, 10.0)]), "H A1 A2 A3 B1 B2 R1 R2 S1 S2", "1 pt touch");
    assert_eq!(page(&[("S1", 200.0, 72.1, 10.0), ("S2", 200.0, 86.1, 10.0)]), "H A1 A2 A3 B1 B2 R1 R2 S1 S2", "0.1 pt touch");
    let marker = page(&[("S1", 200.0, 68.0, 10.0), ("x", 200.0, 96.0, 10.0), ("9", 206.5, 90.0, 6.0), ("S2y", 214.0, 96.0, 10.0)]);
    assert!(marker.starts_with("H A1 A2 A3 B1 B2 R1 R2 S1 "), "footnote marker: {marker}");
}

#[test]
fn a_full_width_line_ends_the_columns_above_it() {
    // Two columns, a full-width heading, two more columns and a full-width footnote. The left column stops at the
    // heading and the footnote: the right column above each one reads first.
    let page = [
        ("L1", 10.0, 10.0),
        ("L2", 10.0, 24.0),
        ("L3", 10.0, 48.0),
        ("R1", 200.0, 10.0),
        ("R2", 200.0, 38.0),
        ("Fullwidthheading", 10.0, 80.0),
        ("L4", 10.0, 110.0),
        ("L5", 10.0, 124.0),
        ("L6", 10.0, 148.0),
        ("R3", 200.0, 110.0),
        ("R4", 200.0, 138.0),
        ("Fullwidthfootnote", 10.0, 200.0),
    ];
    // The two full-width lines run past the right column (x = 286 and 304) at 18 pt a letter.
    let mut v = Vec::new();
    for &(s, x, y) in &page {
        word(&mut v, s, x, y, if s.len() > 2 { 18.0 } else { 6.0 });
    }
    assert_eq!(layout(v).plain_text(), "L1\nL2\nL3\nR1\nR2\nFullwidthheading\nL4\nL5\nL6\nR3\nR4\nFullwidthfootnote");
}

#[test]
fn table_rows_apart_still_read_across() {
    // Rows a paragraph gap apart whose cells start level read row by row, as before #3658.
    let rows = [("Name", 10.0, 10.0), ("Qty", 100.0, 10.0), ("Apple", 10.0, 34.0), ("12", 100.0, 34.0), ("Pear", 10.0, 58.0), ("7", 100.0, 58.0)];
    assert_eq!(lines(&rows), "Name\nQty\nApple\n12\nPear\n7");
}

#[test]
fn form_rows_in_mixed_sizes_still_read_across() {
    // 8 pt labels beside 12 pt values on one baseline, rows 30 pt apart: the tops differ by 4 pt, but the first
    // lines overlap, so each row reads label then value.
    let mut v = Vec::new();
    let sized = |v: &mut Vec<TextGlyph>, s: &str, x: f32, base: f32, h: f32| {
        for (i, c) in s.chars().enumerate() {
            let x0 = x + i as f32 * h * 0.6;
            v.push(em_box(&c.to_string(), x0, base - h, x0 + h * 0.6, base));
        }
    };
    for (i, (label, value)) in [("Name", "Alice"), ("Street", "Main"), ("City", "Oslo")].into_iter().enumerate() {
        let base = 20.0 + 30.0 * i as f32;
        sized(&mut v, label, 10.0, base, 8.0);
        sized(&mut v, value, 100.0, base, 12.0);
    }
    assert_eq!(layout(v).plain_text(), "Name\nAlice\nStreet\nMain\nCity\nOslo");
    // Values drawn 6 pt below their labels, rows a paragraph gap apart.
    let offset =
        [("Name", 10.0, 10.0), ("Val1", 100.0, 16.0), ("Addr", 10.0, 40.0), ("Val2", 100.0, 46.0), ("City", 10.0, 70.0), ("Val3", 100.0, 76.0)];
    assert_eq!(lines(&offset), "Name\nVal1\nAddr\nVal2\nCity\nVal3");
}

#[test]
fn a_column_does_not_jump_to_a_distant_footer_footnote_or_caption() {
    // A page number far below the left column reads after the right column.
    let footer = [
        ("L1", 10.0, 10.0),
        ("L2", 10.0, 24.0),
        ("L3", 10.0, 48.0),
        ("R1", 200.0, 10.0),
        ("R2", 200.0, 24.0),
        ("R3", 200.0, 48.0),
        ("Pg7", 10.0, 400.0),
    ];
    assert_eq!(lines(&footer), "L1\nL2\nR1\nR2\nL3\nR3\nPg7");
    // So does a footnote under the left column only, with the page number under the right one.
    let footnote = [
        ("L1", 10.0, 10.0),
        ("L2", 10.0, 24.0),
        ("L3", 10.0, 48.0),
        ("R1", 200.0, 10.0),
        ("R2", 200.0, 24.0),
        ("R3", 200.0, 48.0),
        ("Note", 10.0, 380.0),
        ("Pg7", 200.0, 400.0),
    ];
    assert_eq!(lines(&footnote), "L1\nL2\nR1\nR2\nL3\nR3\nNote\nPg7");
    // A caption under a full-width figure (no text from 70 to 250), as narrow as the left column, reads after both
    // columns above the figure and before the columns below it.
    let caption = [
        ("L1", 10.0, 10.0),
        ("L2", 10.0, 24.0),
        ("L3", 10.0, 38.0),
        ("R1", 200.0, 10.0),
        ("R2", 200.0, 24.0),
        ("R3", 200.0, 38.0),
        ("Cap", 10.0, 260.0),
        ("M1", 10.0, 290.0),
        ("M2", 10.0, 304.0),
        ("N1", 200.0, 290.0),
        ("N2", 200.0, 304.0),
    ];
    assert_eq!(lines(&caption), "L1\nL2\nL3\nR1\nR2\nR3\nCap\nM1\nM2\nN1\nN2");
}

#[test]
fn many_column_paragraphs_order_in_bounded_time() {
    // 2000 paragraphs in two columns, the right one offset by half a paragraph: every block is a column
    // continuation, and choosing each one scans the remaining blocks.
    let mut v = Vec::new();
    for i in 0..1000u16 {
        let y = 10.0 + 24.0 * f32::from(i);
        word(&mut v, "L", 10.0, y, 6.0);
        word(&mut v, "R", 200.0, y + 12.0, 6.0);
    }
    let start = std::time::Instant::now();
    let text = layout(v).plain_text();
    assert!(start.elapsed() < std::time::Duration::from_secs(5), "{:?}", start.elapsed());
    // Compared without printing 4000 characters on failure.
    let expected = format!("{}{}", "L\n".repeat(1000), "R\n".repeat(999) + "R");
    assert!(text == expected, "the first R reads after {:?} lines, not 1000", text.find('R').map(|i| i / 2));
}

#[test]
fn tracked_text_keeps_letters_together() {
    let mut v = Vec::new();
    word(&mut v, "CHAPTER", 10.0, 10.0, 9.0); // 3pt tracking between 6pt-wide letters
    word(&mut v, "FOUR", 10.0 + 7.0 * 9.0 + 5.0, 10.0, 9.0);
    assert_eq!(layout(v).plain_text(), "CHAPTER FOUR");
}

#[test]
fn toc_leaders_do_not_hide_word_gaps() {
    use lopdf::{Document, Object, Stream, dictionary};
    for dots in [0, 5, 10, 20, 40, 60] {
        for (words, expected) in [
            ("(Was)-333(ist)-333(OTRS?)", "Was ist OTRS?"),
            ("(Das)-333(OTRS)-333(Web-Interface)", "Das OTRS Web-Interface"),
            ("(Neuen)-333(Zugang)-333(beantragen)", "Neuen Zugang beantragen"),
        ] {
            // The issue's LaTeX TJ spacing, without embedding the corpus PDF or a font asset.
            let mut doc = Document::with_version("1.7");
            let font = doc.add_object(dictionary! { "Type" => "Font", "Subtype" => "Type1", "BaseFont" => "Times-Roman" });
            let operators = format!("BT /F1 10 Tf 10 100 Td [{words}-441{}-1663(7)] TJ ET", "-500(.)".repeat(dots));
            let content = doc.add_object(Stream::new(dictionary! {}, operators.into_bytes()));
            let pages = doc.new_object_id();
            let page = doc.add_object(dictionary! {
                "Type" => "Page", "Parent" => pages,
                "MediaBox" => vec![0.into(), 0.into(), 800.into(), Object::Integer(200)],
                "Resources" => dictionary! { "Font" => dictionary! { "F1" => font } }, "Contents" => content
            });
            doc.objects.insert(pages, dictionary! { "Type" => "Pages", "Kids" => vec![Object::Reference(page)], "Count" => 1 }.into());
            let catalog = doc.add_object(dictionary! { "Type" => "Catalog", "Pages" => pages });
            doc.trailer.set("Root", catalog);
            let mut bytes = Vec::new();
            doc.save_to(&mut bytes).unwrap();
            let pdf = Pdf::new(std::sync::Arc::new(bytes)).unwrap();
            let text = extract_page(&pdf, 0, &InterpreterSettings::default()).unwrap();
            assert_eq!(text.plain_text(), format!("{expected} {}7", ". ".repeat(dots)), "{dots} leader dots");
            let hits = text.find_opts(expected, true, true);
            assert_eq!(hits.len(), 1, "{expected:?}, {dots} leader dots");
            assert_eq!(text.text_of(hits[0].clone()), expected);
            assert_eq!(text.line_rects(hits[0].clone()).len(), 1);
            assert_eq!(text.find(&expected.to_lowercase()), hits);
            assert!(text.find(&expected.replace(' ', "")).is_empty());
        }
    }
}

#[test]
fn short_tracked_ellipses_remain_searchable() {
    for dots in [3, 4] {
        let mut v = Vec::new();
        for i in 0..dots {
            let x = 10.0 + i as f32 * 4.0;
            v.push(g(".", x, 10.0, x + 2.0));
        }
        let ellipsis = ".".repeat(dots);
        let text = layout(v.clone());
        assert_eq!(text.plain_text(), ellipsis);
        assert_eq!(text.find(&ellipsis), vec![0..dots]);
        v.insert(0, g("A", 0.0, 10.0, 6.0));
        let x = 12.0 + dots as f32 * 4.0;
        v.push(g("B", x, 10.0, x + 6.0));
        let text = layout(v);
        let expected = format!("A {ellipsis} B");
        assert_eq!(text.plain_text(), expected);
        assert_eq!(text.find(&expected), vec![0..dots + 2]);
    }
}

#[test]
fn leaders_preserve_tracking_in_letters_numbers_and_punctuation() {
    for advance in [9.0, 12.0] {
        for leader in [".", "-", "…"] {
            let mut v = Vec::new();
            word(&mut v, "CHAPTER-1", 10.0, 10.0, advance);
            let x = 10.0 + 9.0 * advance + 5.0;
            word(&mut v, "1.4", x, 10.0, advance);
            let mut x = x + 3.0 * advance + 5.0;
            for _ in 0..60 {
                v.push(g(leader, x, 10.0, x + 2.5));
                x += 11.0;
            }
            let text = layout(v);
            assert_eq!(text.plain_text(), format!("CHAPTER-1 1.4 {}", vec![leader; 60].join(" ")));
            assert_eq!(text.find("chapter-1 1.4"), vec![0..12]);
        }
    }
    // Decimal punctuation and repeated letters/digits still use their original tracking estimate.
    for value in ["1.4", "1111", "llll"] {
        let mut v = Vec::new();
        word(&mut v, value, 10.0, 10.0, 12.0);
        assert_eq!(layout(v).plain_text(), value);
    }
}

#[test]
fn rtl_runs_come_out_in_logical_order_and_cjk_has_no_spaces() {
    let mut v = Vec::new();
    // Hebrew "שלום" drawn visually right-to-left: ם ו ל ש from left to right.
    word(&mut v, "םולש", 10.0, 10.0, 6.0);
    assert_eq!(layout(v).plain_text(), "שלום");
    let mut v = Vec::new();
    word(&mut v, "每个字", 10.0, 10.0, 9.0);
    assert_eq!(layout(v).plain_text(), "每个字");
}

/// #1062: the word gap stays between the runs when the right-hand run is drawn first, and
/// between the words of a right-to-left line.
#[test]
fn word_gaps_follow_position_not_drawing_order() {
    let mut v = Vec::new();
    word(&mut v, "19.4", 40.0, 10.0, 6.0);
    word(&mut v, "20.0", 10.0, 10.0, 6.0);
    assert_eq!(layout(v).plain_text(), "20.0 19.4");
    let mut v = Vec::new();
    // "שלום עולם" drawn visually: עולם reversed on the left, שלום reversed on the right.
    word(&mut v, "םלוע", 10.0, 10.0, 6.0);
    word(&mut v, "םולש", 40.0, 10.0, 6.0);
    assert_eq!(layout(v).plain_text(), "שלום עולם");
}

/// #2667: a number or punctuation on a right-to-left line must not split it into separately reversed words.
#[test]
fn rtl_line_with_number_and_punctuation_reads_in_logical_order() {
    let line = |words: &[&str]| {
        let mut v = Vec::new();
        let mut x = 10.0;
        for w in words {
            word(&mut v, w, x, 10.0, 6.0);
            x += w.chars().count() as f32 * 6.0 + 6.0;
        }
        layout(v).plain_text()
    };
    // Each line as drawn, left to right.
    assert_eq!(line(&["םילקש", "120", "ריחמ"]), "מחיר 120 שקלים");
    assert_eq!(line(&[".הקידב", "ךמסמ", "והז", ":תירבע"]), "עברית: זהו מסמך בדיקה.");
    assert_eq!(line(&["باتك", "12.5", "ددع"]), "عدد 12.5 كتاب");
    assert_eq!(line(&["םלוע", "םולש"]), "שלום עולם");
    assert_eq!(line(&["Total", "םולש", "price"]), "Total שלום price");
}

/// Two lines, "ab cd" and "e fg": words stop at blank glyphs, word gaps and line ends.
#[test]
fn words_and_lines_around_a_glyph() {
    let t = PageText {
        glyphs: Vec::from(["a", "b", " ", "c", "d", "e", "f", "g"].map(|s| TextGlyph {
            text: s.into(),
            rect: [0.0; 4],
            quad: [[0.0; 2]; 4],
            direction: [1.0, 0.0],
        })),
        line_of: vec![0, 0, 0, 0, 0, 1, 1, 1],
        space_before: vec![false, false, false, false, false, false, true, false],
    };
    assert_eq!(t.word_at(1), Some((0, 1)));
    assert_eq!(t.word_at(3), Some((3, 4)));
    assert_eq!(t.word_at(4), Some((3, 4)));
    assert_eq!(t.word_at(5), Some((5, 5)));
    assert_eq!(t.word_at(7), Some((6, 7)));
    assert_eq!(t.line_at(2), Some((0, 4)));
    assert_eq!(t.line_at(6), Some((5, 7)));
    assert_eq!(t.word_at(8), None);
    assert_eq!(t.line_at(8), None);
    assert_eq!(PageText::default().line_at(0), None);
}

/// A 400 × 400 page drawing `ops` with Helvetica as /F1 and Courier as /F2.
fn text_page(ops: &str) -> Pdf {
    use lopdf::{Document, Object, Stream, dictionary};
    let mut doc = Document::with_version("1.7");
    let helvetica = doc.add_object(dictionary! { "Type" => "Font", "Subtype" => "Type1", "BaseFont" => "Helvetica" });
    let courier = doc.add_object(dictionary! { "Type" => "Font", "Subtype" => "Type1", "BaseFont" => "Courier" });
    let content = doc.add_object(Stream::new(dictionary! {}, ops.as_bytes().to_vec()));
    let pages = doc.new_object_id();
    let page = doc.add_object(dictionary! { "Type" => "Page", "Parent" => pages, "MediaBox" => vec![Object::Integer(0), Object::Integer(0), Object::Integer(400), Object::Integer(400)], "Resources" => dictionary! {"Font" => dictionary! {"F1" => helvetica, "F2" => courier}}, "Contents" => content });
    doc.objects.insert(pages, dictionary! {"Type" => "Pages", "Kids" => vec![Object::Reference(page)], "Count" => 1}.into());
    let catalog = doc.add_object(dictionary! {"Type" => "Catalog", "Pages" => pages});
    doc.trailer.set("Root", catalog);
    let mut bytes = Vec::new();
    doc.save_to(&mut bytes).unwrap();
    Pdf::new(std::sync::Arc::new(bytes)).unwrap()
}

/// `body` shown in /F`font` at `size` with its baseline turned `degrees` counter-clockwise.
fn turned(degrees: f64, font: u8, size: f64, x: f64, y: f64, body: &str) -> String {
    let (s, c) = degrees.to_radians().sin_cos();
    format!("BT /F{font} {size} Tf {c:.6} {s:.6} {:.6} {c:.6} {x:.4} {y:.4} Tm {body} ET\n", -s)
}

fn page_text(ops: &str) -> String {
    extract_page(&text_page(ops), 0, &InterpreterSettings::default()).unwrap().plain_text()
}

#[test]
fn short_doubled_word_at_same_origin_does_not_interleave() {
    let ops = "BT /F1 12 Tf 50 Tz 1 0 0 1 100 200 Tm (world) Tj ET\n".repeat(2);
    assert_single_word(page_glyphs(&ops));
}

#[test]
fn short_near_shadow_does_not_interleave() {
    let ops = "BT /F1 12 Tf 50 Tz 1 0 0 1 100 200 Tm (world) Tj ET\nBT /F1 12 Tf 50 Tz 1 0 0 1 101 200 Tm (world) Tj ET\n";
    assert_single_word(page_glyphs(ops));
}

#[test]
fn turned_short_doubled_word_does_not_interleave() {
    let one = turned(37.0, 1, 12.0, 100.0, 200.0, "50 Tz (world) Tj");
    assert_single_word(page_glyphs(&(one.clone() + &one)));
}

#[test]
fn large_backward_tj_adjustment_stays_in_one_run() {
    let text = page_glyphs("BT /F1 12 Tf 50 Tz 1 0 0 1 100 200 Tm [(wor) 3000 (ld)] TJ ET");
    assert_eq!(text.plain_text(), "ld wor");
    assert_eq!(text.glyphs.iter().map(|g| g.text.as_str()).collect::<Vec<_>>(), ["l", "d", "w", "o", "r"]);
    assert!(text.line_of.iter().all(|line| *line == 0));
    assert_eq!(text.space_before, [false, false, true, false, false]);
}

#[test]
fn reset_prefix_requires_one_glyph_height_of_progress() {
    let mut glyphs = Vec::new();
    word(&mut glyphs, "abc", 10.0, 10.0, 6.0);
    word(&mut glyphs, "abc", 10.0, 10.0, 6.0);
    let glyphs: Vec<&TextGlyph> = glyphs.iter().collect();
    assert!(!restarts_run(&glyphs, 0, 3, 9.99));
    assert!(!restarts_run(&glyphs, 0, 3, 10.0));
    assert!(restarts_run(&glyphs, 0, 3, 10.01));
}

#[test]
fn reset_prefix_uses_the_existing_redraw_position_tolerance() {
    let mut near = Vec::new();
    word(&mut near, "abc", 10.0, 10.0, 6.0);
    word(&mut near, "abc", 11.99, 10.0, 6.0);
    let near: Vec<&TextGlyph> = near.iter().collect();
    assert!(restarts_run(&near, 0, 3, 10.01));

    let mut far = Vec::new();
    word(&mut far, "abc", 10.0, 10.0, 6.0);
    word(&mut far, "abc", 12.01, 10.0, 6.0);
    let far: Vec<&TextGlyph> = far.iter().collect();
    assert!(!restarts_run(&far, 0, 3, 10.01));
}

#[test]
fn reset_prefix_rejects_non_finite_positions() {
    let mut glyphs = Vec::new();
    word(&mut glyphs, "abc", 10.0, 10.0, 6.0);
    word(&mut glyphs, "abc", 10.0, 10.0, 6.0);
    glyphs[3].rect[0] = f32::NAN;
    let glyphs: Vec<&TextGlyph> = glyphs.iter().collect();
    assert!(!restarts_run(&glyphs, 0, 3, 10.01));
}

#[test]
fn turned_lines_group_along_their_baseline() {
    // Two lines 13 pt apart, words separated by TJ offsets (no space glyphs), the second
    // line drawn first. Turned away from the quarter turns, the lines' boxes overlap.
    for degrees in [12.0, 30.0, 37.0, 45.0, 60.0, 135.0, 200.0, 300.0, -45.0] {
        let (s, c) = f64::to_radians(degrees).sin_cos();
        let second = turned(degrees, 1, 12.0, 200.0 + s * 13.0, 200.0 - c * 13.0, "[(Second) -400 (line) -400 (here)] TJ");
        let first = turned(degrees, 1, 12.0, 200.0, 200.0, "[(First) -400 (line) -400 (text)] TJ");
        assert_eq!(page_text(&(second + &first)), "First line text\nSecond line here", "{degrees}°");
    }
}

#[test]
fn crossing_turned_lines_stay_apart() {
    // Lines at 30° and −30° crossing each other, their glyphs drawn alternately.
    let (up, down) = ("Upward words here", "Downward words now");
    let mut ops = String::new();
    let (s, c) = f64::to_radians(30.0).sin_cos();
    for (k, (a, b)) in up.chars().zip(down.chars()).enumerate() {
        let along = 7.2 * k as f64;
        ops += &turned(30.0, 2, 12.0, 100.0 + c * along, 150.0 + s * along, &format!("({a}) Tj"));
        ops += &turned(-30.0, 2, 12.0, 100.0 + c * along, 250.0 - s * along, &format!("({b}) Tj"));
    }
    ops += &turned(-30.0, 2, 12.0, 100.0 + c * 7.2 * 17.0, 250.0 - s * 7.2 * 17.0, "(w) Tj");
    assert_eq!(page_text(&ops), "Downward words now\nUpward words here");
}

#[test]
fn text_along_a_curve_is_one_line() {
    // Courier glyphs (7.2 pt advance) set on circles of radius 90 around (200, 200): over
    // the top reading clockwise, under the bottom reading counter-clockwise, and a full ring.
    let arc = |text: &str, from: f64, clockwise: bool, radius: f64| {
        let step = (7.2 / radius).to_degrees() * if clockwise { -1.0 } else { 1.0 };
        let mut ops = String::new();
        for (k, ch) in text.chars().enumerate() {
            let a = from + step * k as f64;
            let (s, c) = a.to_radians().sin_cos();
            ops += &turned(if clockwise { a - 90.0 } else { a + 90.0 }, 2, 12.0, 200.0 + radius * c, 200.0 + radius * s, &format!("({ch}) Tj"));
        }
        ops
    };
    assert_eq!(page_text(&arc("CURVED STAMP TEXT", 150.0, true, 90.0)), "CURVED STAMP TEXT");
    assert_eq!(page_text(&arc("BOTTOM ARC WORDS", 220.0, false, 90.0)), "BOTTOM ARC WORDS");
    assert_eq!(page_text(&arc("ROUND SEAL OF THE COMPANY LIMITED", 180.0, true, 50.0)), "ROUND SEAL OF THE COMPANY LIMITED");
}

#[test]
fn sheared_text_keeps_its_word_gaps() {
    // The baseline climbs at about 31° while the glyphs stay upright.
    let ops = "BT /F1 12 Tf 1 0.6 0 1 100 200 Tm [(Sheared) -400 (first)] TJ 1 0.6 0 1 100 187 Tm [(Sheared) -400 (second)] TJ ET";
    assert_eq!(page_text(ops), "Sheared first\nSheared second");
}

#[test]
fn turned_text_keeps_its_place_in_the_reading_order() {
    // A turned label between a heading and its caption reads between them, as it did when
    // it was laid out with the horizontal text; a vertical body still follows its heading.
    let ops = "BT /F1 14 Tf 1 0 0 1 50 360 Tm (Heading above) Tj ET\n".to_string()
        + &turned(30.0, 1, 12.0, 80.0, 250.0, "(Turned label) Tj")
        + "BT /F1 12 Tf 1 0 0 1 50 150 Tm (Caption below) Tj ET\n";
    assert_eq!(page_text(&ops), "Heading above\nTurned label\nCaption below");
}

#[test]
fn turned_glyphs_keep_their_shape_and_direction() {
    // 30° counter-clockwise on the page is 30° up from the x axis in view space (y down).
    let pdf = text_page(&(turned(30.0, 1, 20.0, 100.0, 100.0, "(Ab) Tj") + "BT /F1 20 Tf 1 0 0.5 1 100 300 Tm (Ab) Tj ET"));
    let text = extract_page(&pdf, 0, &InterpreterSettings::default()).unwrap();
    assert_eq!(text.plain_text(), "Ab\nAb");
    for g in &text.glyphs {
        assert!(g.quad.iter().flatten().chain(&g.direction).chain(&g.rect).all(|c| c.is_finite()), "{g:?}");
        let bounds = g.quad.iter().fold([f32::INFINITY, f32::INFINITY, f32::NEG_INFINITY, f32::NEG_INFINITY], |r, p| {
            [r[0].min(p[0]), r[1].min(p[1]), r[2].max(p[0]), r[3].max(p[1])]
        });
        assert_eq!(g.rect, bounds, "the box bounds the quad");
        let [x, y] = [g.quad[1][0] - g.quad[0][0], g.quad[1][1] - g.quad[0][1]];
        assert!(
            (x * g.direction[1] - y * g.direction[0]).abs() < 1e-3 && x * g.direction[0] + y * g.direction[1] > 0.0,
            "quad[0] → quad[1] runs along the direction: {g:?}"
        );
    }
    let (skewed, turned) = (&text.glyphs[0], &text.glyphs[2]);
    assert!((turned.direction[0] - 30f32.to_radians().cos()).abs() < 1e-6 && (turned.direction[1] + 0.5).abs() < 1e-6, "{turned:?}");
    assert_eq!(skewed.direction, [1.0, 0.0]);
    let slant = skewed.quad[3][0] - skewed.quad[0][0];
    assert!((slant - 10.0).abs() < 1e-3, "the skewed glyph's upright edge leans by half its 20 pt height: {skewed:?}");
}

/// The operators drawing each `(text, text matrix)` run in Helvetica 12 pt, one text object per run.
fn run_ops(runs: &[(&str, &str)]) -> String {
    runs.iter().map(|(text, tm)| format!("BT /F1 12 Tf {tm} Tm ({text}) Tj ET\n")).collect()
}

fn page_glyphs(ops: &str) -> PageText {
    extract_page(&text_page(ops), 0, &InterpreterSettings::default()).unwrap()
}

fn assert_single_word(text: PageText) {
    assert_eq!(text.plain_text(), "world");
    assert_eq!(text.glyphs.iter().map(|g| g.text.as_str()).collect::<String>(), "world");
    assert_eq!(text.line_of, [0; 5]);
    assert_eq!(text.space_before, [false; 5]);
}

#[test]
fn runs_printed_over_each_other_stay_whole_at_any_angle() {
    // Two runs that start at one point (exact overprint), that start 40 pt along and 3 pt down
    // (shifted), and that are 14 pt apart (clear): at 0° and at 37°, each run comes out whole.
    let tm = |angle: f32, along: f32, down: f32| {
        let (s, c) = angle.to_radians().sin_cos();
        let (x, y) = (100.0 + along * c - down * s, 200.0 + along * s + down * c);
        format!("{c:.6} {s:.6} {:.6} {c:.6} {x:.4} {y:.4}", -s)
    };
    let (alpha, delta) = ("Alpha beta gamma", "Delta epsilon zeta");
    for angle in [0.0, 37.0] {
        for (along, down) in [(0.0, 0.0), (40.0, -3.0), (0.0, -14.0)] {
            let text = page_glyphs(&run_ops(&[(alpha, &tm(angle, 0.0, 0.0)), (delta, &tm(angle, along, down))]));
            for run in [alpha, delta] {
                assert!(
                    !text.find(run).is_empty(),
                    "{run:?} whole at {angle}° with the second run {along} pt along and {down} pt across: {:?}",
                    text.plain_text()
                );
            }
        }
    }
}

#[test]
fn a_run_drawn_over_a_line_leaves_the_line_whole() {
    // "XX" is drawn over the middle of "beta" after the line, so it is a run of its own that
    // overlaps the line. The line must still come out as one whole line.
    let text = page_glyphs(&run_ops(&[("Alpha beta gamma", "1 0 0 1 100 200"), ("XX", "1 0 0 1 136 200")]));
    let flat = text.plain_text();
    assert!(flat.contains("Alpha beta gamma"), "{flat:?}");
    assert!(flat.contains("XX") && !flat.contains('\n'), "{flat:?}");
}

#[test]
fn a_combining_mark_drawn_last_stays_with_its_letter() {
    // "ab", then a far-off "Z", then a zero-width combining mark at the start of "b". The mark
    // is a run of its own, but it has no extent, so it goes back next to "b".
    let mut v = Vec::new();
    word(&mut v, "ab", 10.0, 10.0, 6.0);
    v.push(g("Z", 200.0, 10.0, 206.0));
    v.push(g("\u{301}", 16.0, 10.0, 16.0));
    let t = layout(v);
    assert_eq!(t.glyphs.iter().map(|g| g.text.as_str()).collect::<Vec<_>>(), ["a", "b", "\u{301}", "Z"]);
}

#[test]
fn runs_differing_only_in_glyph_boundaries_are_not_redraws() {
    // "ab" + "c" and "a" + "bc" have the same letters over the same places, but the glyph
    // boundaries differ, so the second is not a redraw of the first and both stay.
    let mut v = vec![g("ab", 10.0, 10.0, 16.0), g("c", 16.0, 10.0, 22.0)];
    v.push(g("Z", 200.0, 10.0, 206.0));
    v.push(g("a", 10.0, 10.0, 16.0));
    v.push(g("bc", 16.0, 10.0, 22.0));
    assert_eq!(layout(v).glyphs.len(), 5);
}

#[test]
fn many_combining_marks_stay_with_their_letters() {
    // 400 letters on one line, then a zero-width mark at the start of each one, drawn after the whole line:
    // the marks form one run of their own, and each goes back next to its letter, in order.
    let letters: Vec<String> = (0..400).map(|i| char::from(b'a' + (i % 26) as u8).to_string()).collect();
    let mut v = Vec::new();
    for (i, c) in letters.iter().enumerate() {
        let x = 10.0 + i as f32 * 6.0;
        v.push(g(c, x, 10.0, x + 6.0));
    }
    for i in 0..letters.len() {
        let x = 10.0 + i as f32 * 6.0;
        v.push(g("\u{301}", x, 10.0, x));
    }
    let t = layout(v);
    let expected: Vec<&str> = letters.iter().flat_map(|c| [c.as_str(), "\u{301}"]).collect();
    assert_eq!(t.glyphs.iter().map(|g| g.text.as_str()).collect::<Vec<_>>(), expected);
    assert!(t.line_of.iter().all(|line| *line == 0));
}

/// `count` runs of three glyphs, each starting within 3 pt of x = 10 on one baseline, so they all overprint one
/// another. Every glyph has a text of its own, so no glyph is taken for a fake-bold copy. With `separated`, each run
/// follows a far-off glyph, which makes it a run of its own.
fn overprinted_runs(count: usize, separated: bool) -> Vec<TextGlyph> {
    let mut v = Vec::new();
    for k in 0..count {
        let text: String = (0..3).map(|i| char::from_u32(0x4E00 + (3 * k + i) as u32).unwrap()).collect();
        word(&mut v, &text, 10.0 + (k * 7 % 31) as f32 * 0.1, 10.0, 6.0);
        if separated {
            let far = char::from_u32(0x2500 + k as u32).unwrap().to_string();
            v.push(g(&far, 500.0, 10.0, 506.0));
        }
    }
    v
}

#[test]
fn a_dense_overprint_joins_its_runs_and_keeps_each_whole() {
    // 150 runs of three glyphs: about 100 000 overlapping pairs, far more than the sweep's budget. The segment is
    // joined as one component, so the sweep is skipped, and each run still comes out whole.
    let v = overprinted_runs(150, false);
    let refs: Vec<&TextGlyph> = v.iter().collect();
    let height = |i: usize| (refs[i].rect[3] - refs[i].rect[1]).max(0.1);
    let mut sc = Scratch { text: (0..150).map(|k| Run { start: 3 * k, end: 3 * k + 3 }).collect(), ..Default::default() };
    assert!(collide(&refs, &height, 10.0, &mut sc), "more pairs than the budget");
    assert_eq!(sc.units.len(), 1, "one component");

    let v = overprinted_runs(150, true);
    let runs: Vec<String> = (0..150).map(|k| v[4 * k..4 * k + 3].iter().map(|g| g.text.as_str()).collect()).collect();
    let t = layout(v);
    assert_eq!(t.glyphs.len(), 600);
    for run in &runs {
        assert!(!t.find(run).is_empty(), "{run:?} whole in {:?}", t.plain_text());
    }
}

#[test]
fn overlapping_rows_each_start_a_word() {
    // 60 rows of three glyphs, each row starting 2 pt right of the one before and overprinting it, so every row
    // is a forced word break. The rows are separated by far-off glyphs, which makes each one a run of its own.
    // Every glyph has a text of its own, so no glyph is taken for a fake-bold copy.
    let mut v = Vec::new();
    let mut rows = Vec::new();
    for k in 0..60 {
        let text: String = (0..3).map(|i| char::from_u32(0x0400 + (3 * k + i) as u32).unwrap()).collect();
        word(&mut v, &text, 10.0 + 2.0 * k as f32, 10.0, 6.0);
        v.push(g(&char::from_u32(0x0500 + k as u32).unwrap().to_string(), 500.0, 10.0, 506.0));
        rows.push(text);
    }
    let t = layout(v);
    let expected: String = rows.join(" ");
    assert!(t.plain_text().starts_with(&expected), "{:?}", t.plain_text());
}

#[test]
fn a_long_line_keeps_every_word_gap() {
    // 1500 five-letter words on one line, a word gap after each: every gap is found, and only those.
    let mut v = Vec::new();
    let mut words = Vec::new();
    for w in 0..1500 {
        let text: String = (0..5).map(|i| char::from(b'a' + ((w * 7 + i * 3) % 26) as u8)).collect();
        word(&mut v, &text, 10.0 + w as f32 * 36.0, 10.0, 6.0);
        words.push(text);
    }
    assert_eq!(layout(v).plain_text(), words.join(" "));
}

#[test]
fn a_redraw_is_found_among_look_alike_runs_up_to_the_candidate_limit() {
    // 40 runs of "abc" share a text and a grid cell, but each differs from every other by more than a fifth of a
    // line in its second or third glyph, so none redraws another. A run of "abc" at the place of the newest run is
    // found, as the newest candidates are checked first. One at the place of the oldest run is 40 candidates back,
    // past the limit, so it is kept.
    let abc = |v: &mut Vec<TextGlyph>, k: usize| {
        let (b, c) = (16.0 + 2.5 * (k % 4) as f32, 22.0 + 2.5 * (k / 4) as f32);
        v.push(g("a", 10.0, 10.0, 16.0));
        v.push(g("b", b, 10.0, b + 6.0));
        v.push(g("c", c, 10.0, c + 6.0));
    };
    let far = |v: &mut Vec<TextGlyph>| {
        for _ in 0..4 {
            let x = 500.0 + 10.0 * v.len() as f32;
            v.push(g("|", x, 10.0, x + 6.0));
        }
    };
    let mut v = Vec::new();
    for k in 0..40 {
        abc(&mut v, k);
        far(&mut v);
    }
    abc(&mut v, 39);
    far(&mut v);
    assert_eq!(layout(v.clone()).glyphs.iter().filter(|g| g.text == "a").count(), 40);
    abc(&mut v, 0);
    assert_eq!(layout(v).glyphs.iter().filter(|g| g.text == "a").count(), 41);
}

#[test]
fn a_run_redrawn_at_the_same_place_is_printed_once() {
    // "Hello world" drawn again 0.1 pt away (doubled text) prints once. The same words far
    // below are a separate line, as they are not a redraw.
    let ops = run_ops(&[("Hello world", "1 0 0 1 100 200"), ("Hello world", "1 0 0 1 100.1 200.1"), ("Hello world", "1 0 0 1 100 100")]);
    assert_eq!(page_text(&ops), "Hello world\nHello world");
}

#[test]
fn a_redraw_is_found_past_an_earlier_run_with_the_same_key() {
    // Two "Hello" runs share a text and a grid cell but do not redraw each other: the second is
    // tracked wider. A third run redraws the second and is dropped. A far-off "Z" keeps each run
    // a segment of its own.
    let mut v = Vec::new();
    word(&mut v, "Hello", 10.0, 10.0, 6.0);
    v.push(g("Z", 200.0, 10.0, 206.0));
    word(&mut v, "Hello", 10.5, 10.0, 9.0);
    v.push(g("Z", 300.0, 10.0, 306.0));
    word(&mut v, "Hello", 10.6, 10.0, 9.0);
    let t = layout(v);
    assert_eq!(t.glyphs.iter().filter(|g| g.text == "H").count(), 2, "{:?}", t.plain_text());
    assert_eq!(t.glyphs.len(), 12);
}

#[test]
fn runs_that_only_touch_or_overlap_slightly_still_join() {
    // "beta" is drawn first, then "Alpha" to its left: 2 pt apart (touching) or 2 pt overlapping
    // (negative spacing). Both come out as one line in reading order. Where the word space goes
    // for reversed fragments is not asserted here.
    let mut v = Vec::new();
    word(&mut v, "beta", 42.0, 10.0, 6.0);
    word(&mut v, "Alpha", 10.0, 10.0, 6.0);
    let t = layout(v);
    assert_eq!(t.plain_text().replace(' ', ""), "Alphabeta");
    assert!(t.line_of.iter().all(|line| *line == 0));
    let mut v = Vec::new();
    word(&mut v, "beta", 38.0, 10.0, 6.0);
    word(&mut v, "Alpha", 10.0, 10.0, 6.0);
    let t = layout(v);
    assert_eq!(t.plain_text(), "Alphabeta");
}

/// A three-row table, "Name Qty / Apple 12 / Pear 7": the columns sit far enough apart that
/// reading order takes one column, then the other.
fn table() -> PageText {
    let mut v = Vec::new();
    for (row, (name, qty)) in [("Name", "Qty"), ("Apple", "12"), ("Pear", "7")].into_iter().enumerate() {
        let y = 10.0 + row as f32 * 14.0;
        word(&mut v, name, 10.0, y, 6.0);
        word(&mut v, qty, 100.0, y, 6.0);
    }
    layout(v)
}

/// Issue #740: a column selection takes only what is inside the rectangle.
#[test]
fn a_column_selection_takes_one_table_column() {
    let t = table();
    assert_eq!(t.plain_text(), "Name\nApple\nPear\nQty\n12\n7", "reading order is column by column");
    let qty = t.glyphs_in([95.0, 0.0, 130.0, 60.0]);
    assert_eq!(t.column_text(&qty), "Qty\n12\n7");
    assert_eq!(t.glyph_rects(&qty).len(), 3, "one highlight per row");
    // Corners in either order.
    assert_eq!(t.glyphs_in([130.0, 60.0, 95.0, 0.0]), qty);
}

/// Across columns the text comes out row by row, cells separated by tabs.
#[test]
fn a_column_selection_across_columns_reads_row_by_row() {
    let t = table();
    let all = t.glyphs_in([0.0, 0.0, 200.0, 60.0]);
    assert_eq!(t.column_text(&all), "Name\tQty\nApple\t12\nPear\t7");
    // The middle row only, and only the start of "Apple": centres decide.
    let part = t.glyphs_in([0.0, 25.0, 31.0, 33.0]);
    assert_eq!(t.column_text(&part), "Appl");
    assert_eq!(t.glyph_rects(&part), vec![[10.0, 24.0, 34.0, 34.0]]);
}

/// A rectangle that misses every glyph, or is not a number, selects nothing.
#[test]
fn an_empty_or_degenerate_column_selection_is_empty() {
    let t = table();
    for r in [[300.0, 300.0, 400.0, 400.0], [f32::NAN, 0.0, 200.0, 60.0], [50.0, 50.0, 50.0, 50.0]] {
        let none = t.glyphs_in(r);
        assert!(none.is_empty(), "{r:?}");
        assert_eq!(t.column_text(&none), "");
        assert!(t.glyph_rects(&none).is_empty());
    }
    assert_eq!(PageText::default().column_text(&[0, 7, usize::MAX]), "", "indices past the text are ignored");
}

/// Right-to-left rows put their cells right to left.
#[test]
fn right_to_left_rows_read_right_to_left() {
    let mut v = Vec::new();
    // "שלום" (right) and "עולם" (left), each drawn visually.
    word(&mut v, "םולש", 100.0, 10.0, 6.0);
    word(&mut v, "םלוע", 10.0, 10.0, 6.0);
    let t = layout(v);
    let all = t.glyphs_in([0.0, 0.0, 200.0, 30.0]);
    assert_eq!(t.column_text(&all), "שלום\tעולם");
}

/// Vertical lines (CJK vertical writing) come out one per row, in reading order.
#[test]
fn vertical_lines_keep_reading_order() {
    // Written downwards: the em box's edge towards the next line is its left one.
    let cell = |t: &str, x: f32, y: f32| TextGlyph {
        text: t.into(),
        rect: [x, y, x + 10.0, y + 10.0],
        quad: [[x, y], [x, y + 10.0], [x + 10.0, y + 10.0], [x + 10.0, y]],
        direction: [0.0, 1.0],
    };
    let t = PageText {
        // Right column 日本 first, then the left column 語文, each top to bottom.
        glyphs: vec![cell("日", 40.0, 10.0), cell("本", 40.0, 20.0), cell("語", 20.0, 10.0), cell("文", 20.0, 20.0)],
        line_of: vec![0, 0, 1, 1],
        space_before: vec![false; 4],
    };
    let all = t.glyphs_in([0.0, 0.0, 60.0, 40.0]);
    assert_eq!(t.column_text(&all), "日本\n語文");
}

/// An embedded TrueType font with no `/Encoding`, flagged `flags`, its `/ToUnicode` (if
/// any) mapping only printable ASCII, as OCR engines write the text layer of a scan.
fn ocr_layer_text(flags: i64, ascii_to_unicode: bool) -> String {
    use lopdf::{Document, Object, Stream, dictionary};
    let mut doc = Document::with_version("1.7");
    let program = doc.add_object(Stream::new(dictionary! {}, include_bytes!("../../../../assets/fonts/Inter-Regular.ttf").to_vec()));
    let descriptor =
        doc.add_object(dictionary! { "Type" => "FontDescriptor", "FontName" => "Inter-Regular", "Flags" => flags, "FontFile2" => program });
    let mut font = dictionary! { "Type" => "Font", "Subtype" => "TrueType", "BaseFont" => "Inter-Regular", "FontDescriptor" => descriptor };
    if ascii_to_unicode {
        let cmap = b"/CIDInit /ProcSet findresource begin 12 dict begin begincmap /CMapName /Ascii def /CMapType 2 def 1 begincodespacerange <00> <FF> endcodespacerange 1 beginbfrange <20> <7E> <0020> endbfrange endcmap CMapName currentdict /CMap defineresource pop end end";
        font.set("ToUnicode", doc.add_object(Stream::new(dictionary! {}, cmap.to_vec())));
    }
    let font = doc.add_object(font);
    // WinAnsi bytes: "Rückfragen für Überhitzungsschutz: können, weiß, Café".
    let mut ops = b"BT /F1 14 Tf 1 0 0 1 20 200 Tm (".to_vec();
    ops.extend_from_slice(b"R\xfcckfragen f\xfcr \xdcberhitzungsschutz: k\xf6nnen, wei\xdf, Caf\xe9");
    ops.extend_from_slice(b") Tj ET");
    let content = doc.add_object(Stream::new(dictionary! {}, ops));
    let pages = doc.new_object_id();
    let page = doc.add_object(dictionary! { "Type" => "Page", "Parent" => pages, "MediaBox" => vec![Object::Integer(0), Object::Integer(0), Object::Integer(400), Object::Integer(400)], "Resources" => dictionary! {"Font" => dictionary! {"F1" => font}}, "Contents" => content });
    doc.objects.insert(pages, dictionary! {"Type" => "Pages", "Kids" => vec![Object::Reference(page)], "Count" => 1}.into());
    let catalog = doc.add_object(dictionary! {"Type" => "Catalog", "Pages" => pages});
    doc.trailer.set("Root", catalog);
    let mut bytes = Vec::new();
    doc.save_to(&mut bytes).unwrap();
    let pdf = Pdf::new(std::sync::Arc::new(bytes)).unwrap();
    extract_page(&pdf, 0, &InterpreterSettings::default()).unwrap().plain_text()
}

#[test]
fn truetype_without_an_encoding_reads_its_codes_as_win_ansi() {
    // Café: 0xE9 is /Oslash in StandardEncoding, so reading the codes as Standard first and
    // WinAnsi only for the gaps would still give "CafØ".
    let expected = "Rückfragen für Überhitzungsschutz: können, weiß, Café";
    assert_eq!(ocr_layer_text(4, false), expected, "symbolic, no /ToUnicode");
    assert_eq!(ocr_layer_text(32, true), expected, "nonsymbolic, /ToUnicode for ASCII only");
}

/// Issue #2929: a footnote reference (superscript) on a line stays in that line instead of
/// splitting out as a raised segment that steals the block's tail and pushes the host line
/// after the paragraph.
#[test]
fn paragraph_selection_includes_lines_with_footnote_references() {
    for inline in [true, false] {
        let ops = if inline {
            "BT /F1 12 Tf 1 0 0 1 40 300 Tm (First line of the paragraph) Tj \
             1 0 0 1 40 285 Tm (Second line with a note) Tj \
             /F1 8 Tf 5.5 Ts (1) Tj 0 Ts \
             /F1 12 Tf ( and more text) Tj \
             1 0 0 1 40 270 Tm (Third line of the paragraph) Tj \
             /F1 7 Tf 1 0 0 1 40 104 Tm (1) Tj \
             /F1 10 Tf 1 0 0 1 47 100 Tm (Footnote at the bottom) Tj ET"
        } else {
            // Superscripts drawn in a separate pass after the body text.
            "BT /F1 12 Tf 1 0 0 1 40 300 Tm (First line of the paragraph) Tj \
             1 0 0 1 40 285 Tm (Second line with a note) Tj \
             1 0 0 1 173 285 Tm (and more text) Tj \
             1 0 0 1 40 270 Tm (Third line of the paragraph) Tj \
             /F1 10 Tf 1 0 0 1 47 100 Tm (Footnote at the bottom) Tj \
             /F1 8 Tf 1 0 0 1 165 290.5 Tm (1) Tj \
             /F1 7 Tf 1 0 0 1 40 104 Tm (1) Tj ET"
        };
        let text = page_glyphs(ops);
        let start = text.nearest(40.0, 95.0).unwrap();
        let end = text.nearest(185.0, 128.0).unwrap();
        assert_eq!(
            text.text_of(start..end + 1),
            "First line of the paragraph\nSecond line with a note1 and more text\nThird line of the paragraph",
            "inline={inline}"
        );
        assert_eq!(text.line_rects(start..end + 1).len(), 3, "three selected lines, inline={inline}");
        assert_eq!(
            text.plain_text(),
            "First line of the paragraph\nSecond line with a note1 and more text\nThird line of the paragraph\n1 Footnote at the bottom",
            "inline={inline}"
        );
    }

    // Superscript on the first line, multi-character bracketed reference `[12]` on the second
    // line, and subscript on the third line (both inline and out-of-order).
    for inline in [true, false] {
        let ops = if inline {
            "BT /F1 12 Tf 1 0 0 1 40 300 Tm (Opening claim) Tj \
             /F1 8 Tf 5.5 Ts (1) Tj 0 Ts \
             /F1 12 Tf ( in the first line) Tj \
             1 0 0 1 40 285 Tm (Middle citation) Tj \
             /F1 8 Tf 5.5 Ts ([12]) Tj 0 Ts \
             /F1 12 Tf ( continues here) Tj \
             1 0 0 1 40 270 Tm (Formula H) Tj \
             /F1 8 Tf -3 Ts (2) Tj 0 Ts \
             /F1 12 Tf (O in the third line) Tj ET"
        } else {
            "BT /F1 12 Tf 1 0 0 1 40 300 Tm (Opening claim) Tj \
             1 0 0 1 125 300 Tm (in the first line) Tj \
             1 0 0 1 40 285 Tm (Middle citation) Tj \
             1 0 0 1 135 285 Tm (continues here) Tj \
             1 0 0 1 40 270 Tm (Formula H) Tj \
             1 0 0 1 101 270 Tm (O in the third line) Tj \
             /F1 8 Tf 1 0 0 1 117 305.5 Tm (1) Tj \
             1 0 0 1 117 290.5 Tm ([12]) Tj \
             1 0 0 1 96.5 267 Tm (2) Tj ET"
        };
        let text = page_glyphs(ops);
        assert_eq!(
            text.plain_text(),
            "Opening claim1 in the first line\nMiddle citation[12] continues here\nFormula H2O in the third line",
            "inline={inline}"
        );
        assert_eq!(text.line_rects(0..text.glyphs.len()).len(), 3, "inline={inline}");
    }

    // Justified wide gap (18pt = 1.5 * h) after an inline superscript stays on the same line,
    // and an out-of-order superscript kerned slightly over punctuation does not break the host
    // word via overprint collision.
    let justified = page_glyphs(
        "BT /F1 12 Tf 1 0 0 1 40 300 Tm (Justified clause.) Tj \
         /F1 8 Tf 5.5 Ts (1) Tj 0 Ts \
         /F1 12 Tf 1 0 0 1 151 300 Tm (Next sentence on the same line) Tj \
         1 0 0 1 40 285 Tm (Kerned clause.) Tj \
         1 0 0 1 130 285 Tm (continues cleanly) Tj \
         /F1 8 Tf 1 0 0 1 118 290.5 Tm (2) Tj ET",
    );
    assert_eq!(justified.plain_text(), "Justified clause.1 Next sentence on the same line\nKerned clause.2 continues cleanly");
    assert_eq!(justified.line_rects(0..justified.glyphs.len()).len(), 2);

    // A line that begins with a raised superscript marker and continues with taller body text
    // adopts the body text's vertical band (Seg::extend_box first branch) so a later
    // out-of-order superscript on that line still measures against the body baseline.
    let leading_script = page_glyphs(
        "BT /F1 8 Tf 1 0 0 1 40 300 Tm 6 Ts (1) Tj 0 Ts \
         /F1 12 Tf ( See the appendix) Tj \
         1 0 0 1 146 300 Tm (for details) Tj \
         /F1 8 Tf 1 0 0 1 139 303.6 Tm (2) Tj ET",
    );
    assert_eq!(leading_script.plain_text(), "1 See the appendix2 for details");
    let marker = leading_script.glyphs.first().unwrap().rect;
    let body = leading_script.glyphs.iter().find(|g| g.text == "S").unwrap().rect;
    let last = leading_script.glyphs.last().unwrap().rect;
    let center = |r: [f32; 4]| ((r[0] + r[2]) / 2.0, (r[1] + r[3]) / 2.0);
    let (start_x, start_y) = center(marker);
    let (end_x, end_y) = center(last);
    let start = leading_script.nearest(start_x, start_y).unwrap();
    let end = leading_script.nearest(end_x, end_y).unwrap();
    assert_eq!(leading_script.text_of(start..end + 1), "1 See the appendix2 for details");
    let rects = leading_script.line_rects(start..end + 1);
    assert_eq!(rects.len(), 1);
    assert!(marker[1] < body[1] && marker[3] < body[3], "marker must be raised above the body");
    let close = |actual: f32, expected: f32| assert!((actual - expected).abs() < 0.001, "{actual} != {expected}");
    // Highlight rectangles cover the actual glyphs, including the raised marker. They do
    // not expose the segment's baseline band used for line grouping.
    for (actual, expected) in rects[0].into_iter().zip([marker[0], marker[1], last[2], body[3]]) {
        close(actual, expected);
    }
    close(rects[0][3] - rects[0][1], 14.8);
    close(body[3] - 0.2 * 12.0, 100.0); // 300 pt PDF baseline on a 400 pt page.

    // Pin that internal band too: replacing the first extend_box condition with `false`
    // must fail here even when plain text and highlight rectangles remain unchanged.
    let mut segment = Seg::new(Run { start: 0, end: 1 }, marker, marker[3] - marker[1]);
    segment.extend_box(&body, body[3] - body[1]);
    for (actual, expected) in segment.bbox.into_iter().zip([marker[0], body[1], body[2], body[3]]) {
        close(actual, expected);
    }
    close(segment.h, 12.0);
    close(segment.bbox[3] - segment.bbox[1], 12.0);
    close(segment.bbox[3] - 0.2 * segment.h, 100.0);
    close(segment.cy(), (body[1] + body[3]) / 2.0);
}

#[test]
fn text_outside_the_crop_box_is_clipped() {
    use lopdf::{Document, Object, Stream, dictionary};
    let ops = [
        "BT /F1 12 Tf 1 0 0 1 100 200 Tm (Inside crop) Tj ET",
        "BT /F1 12 Tf 1 0 0 1 100 150 Tm /Span <</ActualText (ActualIn)>> BDC (in) Tj EMC ET",
        // Keep every glyph of OutSpan outside the crop; a boundary-crossing glyph remains visible.
        "BT /F1 12 Tf 1 0 0 1 -100 125 Tm /Span <</ActualText (CroppedSpan KeptSpan)>> BDC (OutSpan) Tj 1 0 0 1 100 125 Tm (KeptSpan) Tj EMC ET",
        "BT /F1 12 Tf 1 0 0 1 5 200 Tm (LeftOut) Tj ET",
        "BT /F1 12 Tf 1 0 0 1 360 200 Tm (RightOut) Tj ET",
        "BT /F1 12 Tf 1 0 0 1 100 20 Tm (BelowOut) Tj ET",
        "BT /F1 12 Tf 1 0 0 1 100 370 Tm (AboveOut) Tj ET",
        "BT /F1 12 Tf 1 0 0 1 100 20 Tm /Span <</ActualText (ActualOut)>> BDC (out) Tj EMC ET",
        "BT /F1 12 Tf 1 0 0 1 46 100 Tm (A) Tj ET",
    ]
    .join("\n");
    for rotate in [0, 90, 180, 270] {
        for unit in [1, 2] {
            let mut doc = Document::with_version("1.7");
            let font = doc.add_object(dictionary! { "Type" => "Font", "Subtype" => "Type1", "BaseFont" => "Helvetica" });
            let content = doc.add_object(Stream::new(dictionary! {}, ops.as_bytes().to_vec()));
            let pages = doc.new_object_id();
            let page = doc.add_object(dictionary! {
                "Type" => "Page",
                "Parent" => pages,
                "MediaBox" => vec![Object::Integer(0), Object::Integer(0), Object::Integer(400), Object::Integer(400)],
                "CropBox" => vec![Object::Integer(50), Object::Integer(60), Object::Integer(350), Object::Integer(340)],
                "Rotate" => rotate,
                "UserUnit" => unit,
                "Resources" => dictionary! { "Font" => dictionary! { "F1" => font } },
                "Contents" => content,
            });
            doc.objects.insert(pages, dictionary! { "Type" => "Pages", "Kids" => vec![Object::Reference(page)], "Count" => 1 }.into());
            let catalog = doc.add_object(dictionary! { "Type" => "Catalog", "Pages" => pages });
            doc.trailer.set("Root", catalog);
            let mut bytes = Vec::new();
            doc.save_to(&mut bytes).unwrap();
            let pdf = Pdf::new(std::sync::Arc::new(bytes)).unwrap();
            let text = extract_page(&pdf, 0, &InterpreterSettings::default()).unwrap();
            assert_eq!(text.plain_text(), "Inside crop\nActualIn\nKeptSpan\nA", "rotate={rotate} unit={unit}");
            assert!(!text.find("Inside").is_empty(), "rotate={rotate} unit={unit}");
            assert!(!text.find("ActualIn").is_empty(), "rotate={rotate} unit={unit}");
            assert!(!text.find("KeptSpan").is_empty(), "rotate={rotate} unit={unit}");
            for hidden in ["LeftOut", "RightOut", "BelowOut", "AboveOut", "ActualOut", "CroppedSpan", "OutSpan"] {
                assert!(text.find(hidden).is_empty(), "{hidden} must be clipped at rotate={rotate} unit={unit}");
            }
        }
    }

    // A CropBox that is zero-area or disjoint from the MediaBox leaves an empty visible region,
    // even when a glyph straddles the clamped intersection point.
    for crop in [[200, 200, 200, 200], [100, 200, 300, 200], [500, 500, 600, 600]] {
        let mut doc = Document::with_version("1.7");
        let font = doc.add_object(dictionary! { "Type" => "Font", "Subtype" => "Type1", "BaseFont" => "Helvetica" });
        let content = doc.add_object(Stream::new(dictionary! {}, ops.as_bytes().to_vec()));
        let pages = doc.new_object_id();
        let page = doc.add_object(dictionary! {
            "Type" => "Page",
            "Parent" => pages,
            "MediaBox" => vec![Object::Integer(0), Object::Integer(0), Object::Integer(400), Object::Integer(400)],
            "CropBox" => crop.map(Object::Integer).to_vec(),
            "Resources" => dictionary! { "Font" => dictionary! { "F1" => font } },
            "Contents" => content,
        });
        doc.objects.insert(pages, dictionary! { "Type" => "Pages", "Kids" => vec![Object::Reference(page)], "Count" => 1 }.into());
        let catalog = doc.add_object(dictionary! { "Type" => "Catalog", "Pages" => pages });
        doc.trailer.set("Root", catalog);
        let mut bytes = Vec::new();
        doc.save_to(&mut bytes).unwrap();
        let pdf = Pdf::new(std::sync::Arc::new(bytes)).unwrap();
        let text = extract_page(&pdf, 0, &InterpreterSettings::default()).unwrap();
        assert!(text.is_empty(), "expected no glyphs on {crop:?}, got {:?}", text.plain_text());
    }
}
