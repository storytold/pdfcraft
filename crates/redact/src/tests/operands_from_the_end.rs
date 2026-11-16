//! An operator takes its operands from the end, as the renderer reads them, and a surplus before
//! them is ignored: `(x) /OC /MC0 BDC` marks a layer's block, `(x) /GS1 gs` selects `/GS1`. Read
//! from the front, the surplus stood in for the operands, and what they named went unseen.

use super::*;

/// Hidden-layer text in a block whose tag and property list follow a string: Remove Hidden
/// Information found nothing to remove, and the text stayed.
#[test]
fn a_hidden_layers_block_after_a_surplus_operand_is_removed() {
    use crate::sanitize::{Hidden, remove_hidden};
    let mut doc = one_page(
        b"(x) /OC /MC0 BDC BT /F1 10 Tf 20 260 Td (LAYERSECRET) Tj ET EMC BT /F1 10 Tf 20 20 Td (PUBLIC) Tj ET",
        "/Properties << /MC0 7 0 R >>",
        vec![b"<< /Type /OCG /Name (Hidden) >>".to_vec()],
    );
    doc.update_dict(ObjRef::new(1, 0), |d| {
        d.set(
            b"OCProperties".to_vec(),
            Object::Dict(Dict::from_iter([
                (b"OCGs".to_vec(), Object::Array(vec![Object::Ref(ObjRef::new(7, 0))])),
                (b"D".to_vec(), Object::Dict(Dict::from_iter([(b"OFF".to_vec(), Object::Array(vec![Object::Ref(ObjRef::new(7, 0))]))]))),
            ])),
        );
    })
    .unwrap();
    let removed = remove_hidden(&mut doc, &[Hidden::HiddenLayers]);
    assert!(removed.as_ref().is_ok_and(|r| r.iter().any(|(h, n)| *h == Hidden::HiddenLayers && *n > 0)), "{removed:?}");
    let c = content(&doc, 0);
    assert!(!c.contains("LAYERSECRET") && c.contains("PUBLIC"), "{c}");
}

/// Paint under a mark named by `/ActualText` in a block after a surplus operand: the name, which
/// would still read the word, kept it.
#[test]
fn actual_text_after_a_surplus_operand_goes_with_the_paint_it_names() {
    let mut doc = one_page(b"(x) /Span <</ActualText (SECRET)>> BDC 100 100 m 160 100 l 160 120 l 100 120 l h f EMC", "", vec![]);
    mark(&mut doc, 0, &[[90.0, 95.0, 130.0, 125.0]], "");
    let r = apply(&mut doc, None).unwrap();
    assert_eq!(r.paths_removed, 1);
    let c = content(&doc, 0);
    assert!(!c.contains("SECRET"), "{c}");
}

/// A tagged paragraph whose `MCID` follows a surplus operand: its structure element kept the
/// removed text in `/ActualText` and `/Alt`.
#[test]
fn a_tag_after_a_surplus_operand_loses_what_redaction_removed() {
    let content = b"(x) /P <</MCID 0>> BDC BT /F1 10 Tf 10 200 Td (SECRET) Tj ET EMC";
    let mut doc = pdf(vec![
        b"<< /Type /Catalog /Pages 2 0 R /StructTreeRoot 7 0 R /MarkInfo << /Marked true >> >>".to_vec(),
        b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_vec(),
        b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 300 300] /Contents 4 0 R /StructParents 0 /Resources << /Font << /F1 5 0 R >> >> >>".to_vec(),
        stream("", content),
        FONT.replace("95 0 R", "6 0 R").into_bytes(),
        widths(),
        b"<< /Type /StructTreeRoot /K 8 0 R >>".to_vec(),
        b"<< /S /Document /P 7 0 R /K [9 0 R] >>".to_vec(),
        b"<< /S /P /P 8 0 R /Pg 3 0 R /ActualText (SECRET) /Alt (the secret) /K 0 >>".to_vec(),
    ]);
    mark(&mut doc, 0, &[[5.0, 195.0, 60.0, 212.0]], "");
    assert_eq!(apply(&mut doc, None).unwrap().tags, 1);
    let secret = doc.get(ObjRef::new(9, 0)).as_dict().cloned().unwrap();
    assert!(secret.get(b"ActualText").is_none() && secret.get(b"Alt").is_none(), "{secret:?}");
}

/// A property list still drawn with, named after a surplus operand, stays in the page's resources
/// when the redaction cleans them; read from the front, it was taken for unused and retired.
#[test]
fn a_property_list_named_after_a_surplus_operand_is_kept() {
    let mut doc = one_page(
        b"BT /F1 10 Tf 20 260 Td (SECRET) Tj ET (x) /Span /Pr1 BDC BT /F1 10 Tf 20 20 Td (KEPT) Tj ET EMC",
        "/Properties << /Pr1 << /Lang (en-GB) >> >>",
        vec![],
    );
    mark(&mut doc, 0, &[[0.0, 200.0, 300.0, 300.0]], "");
    assert_eq!(apply(&mut doc, None).unwrap().glyphs, 6);
    let page = pdfcraft_model::pages(&doc).swap_remove(0);
    let res = page.dict.get(b"Resources").map(|r| doc.resolve(r)).and_then(|r| r.as_dict().cloned()).unwrap_or_default();
    let props = res.get(b"Properties").map(|p| doc.resolve(p)).and_then(|p| p.as_dict().cloned()).unwrap_or_default();
    assert!(props.contains(b"Pr1"), "{res:?}");
}

/// A font set only by an ExtGState selected after a surplus operand: read from the front, no font
/// was set, every glyph stood at the start of the line with no size, and a mark over the end of
/// the word removed nothing.
#[test]
fn a_font_from_an_extgstate_selected_after_a_surplus_operand_places_the_text() {
    let mut doc = one_page(b"BT (x) /GS1 gs 20 260 Td (SECRET) Tj ET", "/ExtGState << /GS1 << /Font [5 0 R 10] >> >>", vec![]);
    // Over the last two glyphs, E and T at x = 40 to 50.
    mark(&mut doc, 0, &[[42.0, 255.0, 55.0, 275.0]], "");
    assert_eq!(apply(&mut doc, None).unwrap().glyphs, 2);
}

/// `/Pattern cs` after a surplus operand, once the pattern's paint under the mark went: it selects
/// the Pattern space and keeps the pattern, so the fill after it still counts as painting with it
/// and the pattern stays. Read from the front, the surplus cleared the pattern, which was pruned.
/// A grey space clears it.
#[test]
fn a_pattern_space_selected_after_a_surplus_operand_keeps_the_pattern() {
    for (space, pruned) in [("(x) /Pattern", 0), ("/Pattern", 0), ("(x) /DeviceGray", 1)] {
        let mut doc = one_page(
            format!("/Pattern cs /P0 scn 10 260 20 20 re f {space} cs 200 20 50 50 re f").as_bytes(),
            "/Pattern << /P0 7 0 R >>",
            vec![stream("/PatternType 1 /PaintType 1 /TilingType 1 /BBox [0 0 10 10] /XStep 10 /YStep 10 /Resources << >>", b"0 0 5 5 re f")],
        );
        mark(&mut doc, 0, &[[0.0, 250.0, 40.0, 290.0]], "");
        let r = apply(&mut doc, None).unwrap();
        assert_eq!((r.paths_removed, r.patterns_removed), (1, pruned), "{space}");
    }
}

/// A Type 3 font selected after a surplus operand, outside the mark, whose glyph selects an
/// ExtGState, a colour space, a property list and a font after one too: the cleanups after the
/// walk took the font for undrawn and those for unused. All stay, with the glyph.
#[test]
fn a_type3_font_and_its_glyphs_resources_named_after_a_surplus_operand_are_kept() {
    let mut doc = pdf(vec![
        b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
        b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_vec(),
        b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 300 300] /Contents 4 0 R /Resources << /Font << /F1 5 0 R /T3 7 0 R >> >> >>".to_vec(),
        stream("", b"BT /F1 10 Tf 20 260 Td (SECRET) Tj ET BT (x) /T3 12 Tf 20 20 Td (a) Tj ET"),
        FONT.replace("95 0 R", "6 0 R").into_bytes(),
        widths(),
        b"<< /Type /Font /Subtype /Type3 /FontBBox [0 0 10 10] /FontMatrix [0.1 0 0 0.1 0 0] /CharProcs << /a 8 0 R >> \
           /Encoding << /Differences [97 /a] >> /FirstChar 97 /LastChar 97 /Widths [10] /Resources << /ExtGState << /GS0 << /CA 0.5 >> >> \
           /ColorSpace << /CS0 /DeviceGray >> /Properties << /P0 << >> >> /Font << /F0 5 0 R >> >> >>"
            .to_vec(),
        stream("", b"10 0 d0 (x) /GS0 gs (x) /CS0 cs (x) /Span /P0 BDC 0 0 10 10 re f EMC BT (x) /F0 1 Tf ET"),
    ]);
    mark(&mut doc, 0, &[[0.0, 200.0, 300.0, 300.0]], "");
    assert_eq!(apply(&mut doc, None).unwrap().glyphs, 6);
    let page = pdfcraft_model::pages(&doc).swap_remove(0);
    let res = page.dict.get(b"Resources").map(|r| doc.resolve(r)).and_then(|r| r.as_dict().cloned()).unwrap_or_default();
    let fonts = res.get(b"Font").map(|f| doc.resolve(f)).and_then(|f| f.as_dict().cloned()).unwrap_or_default();
    let font = fonts.get(b"T3").map(|f| doc.resolve(f)).and_then(|f| f.as_dict().cloned());
    let font = font.unwrap_or_else(|| panic!("the drawn Type 3 font stays: {fonts:?}"));
    let procs = font.get(b"CharProcs").map(|p| doc.resolve(p)).and_then(|p| p.as_dict().cloned()).unwrap_or_default();
    assert!(procs.contains(b"a"), "its glyph stays: {font:?}");
    let font_res = font.get(b"Resources").map(|r| doc.resolve(r)).and_then(|r| r.as_dict().cloned()).unwrap_or_default();
    let states = font_res.get(b"ExtGState").map(|s| doc.resolve(s)).and_then(|s| s.as_dict().cloned()).unwrap_or_default();
    assert!(states.contains(b"GS0"), "the ExtGState its glyph selects stays: {font_res:?}");
    for (kind, name) in [(&b"ColorSpace"[..], &b"CS0"[..]), (b"Properties", b"P0"), (b"Font", b"F0")] {
        let held = font_res.get(kind).map(|d| doc.resolve(d)).and_then(|d| d.as_dict().cloned()).unwrap_or_default();
        assert!(held.contains(name), "the {} its glyph uses stays: {font_res:?}", String::from_utf8_lossy(name));
    }
}

/// A tagged paragraph whose `TJ` after a surplus operand still draws once the mark has removed the
/// word beside it: read from the front, that `TJ` drew nothing, the paragraph was taken for empty,
/// and its structure element lost the content the page still shows.
#[test]
fn a_tagged_tj_after_a_surplus_operand_still_draws() {
    let content = b"/P <</MCID 0>> BDC BT /F1 10 Tf 10 200 Td (SECRET) Tj 10 -100 Td (x) [(KEPT)] TJ ET EMC";
    let mut doc = pdf(vec![
        b"<< /Type /Catalog /Pages 2 0 R /StructTreeRoot 7 0 R /MarkInfo << /Marked true >> >>".to_vec(),
        b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_vec(),
        b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 300 300] /Contents 4 0 R /StructParents 0 /Resources << /Font << /F1 5 0 R >> >> >>".to_vec(),
        stream("", content),
        FONT.replace("95 0 R", "6 0 R").into_bytes(),
        widths(),
        b"<< /Type /StructTreeRoot /K 8 0 R >>".to_vec(),
        b"<< /S /Document /P 7 0 R /K [9 0 R] >>".to_vec(),
        b"<< /S /P /P 8 0 R /Pg 3 0 R /K 0 >>".to_vec(),
    ]);
    mark(&mut doc, 0, &[[5.0, 195.0, 60.0, 212.0]], "");
    assert_eq!(apply(&mut doc, None).unwrap().glyphs, 6);
    let paragraph = doc.get(ObjRef::new(9, 0)).as_dict().cloned().unwrap();
    assert_eq!(paragraph.get(b"K").and_then(Object::as_int), Some(0), "it still tags what the page shows: {paragraph:?}");
}

/// A path's points follow a surplus number: read from the front, each point was shifted by one,
/// so a square under the mark was placed beside it and kept, and one beside the mark was placed
/// under it and removed.
#[test]
fn a_paths_points_after_a_surplus_number_are_where_the_renderer_draws_them() {
    let under = b"0 0 1 rg 9 100 100 m 9 140 100 l 9 140 140 l 9 100 140 l h f 9 100 100 m 9 140 140 l S".as_slice();
    let mut doc = one_page(under, "", vec![]);
    mark(&mut doc, 0, &[[90.0, 90.0, 150.0, 150.0]], "");
    assert_eq!(apply(&mut doc, None).unwrap().paths_removed, 2, "both paths under the mark go");
    let beside =
        b"0 0 1 rg 0 200 200 m 250 200 l 250 250 l 200 250 l h f 0 200 200 m 210 210 220 220 230 230 c 240 240 250 250 v 200 250 200 260 y h f"
            .as_slice();
    let mut doc = one_page(beside, "", vec![]);
    mark(&mut doc, 0, &[[0.0, 0.0, 20.0, 20.0]], "");
    assert_eq!(apply(&mut doc, None).unwrap().paths_removed, 0, "the paths beside the mark stay");
    // Curves whose points, after a surplus, reach under the mark.
    for curve in [&b"200 200 m 9 10 10 12 12 15 15 c f"[..], b"200 200 m 9 10 10 15 15 v f", b"200 200 m 9 10 10 15 15 y f"] {
        let mut doc = one_page(curve, "", vec![]);
        mark(&mut doc, 0, &[[0.0, 0.0, 20.0, 20.0]], "");
        assert_eq!(apply(&mut doc, None).unwrap().paths_removed, 1, "{}", String::from_utf8_lossy(curve));
    }
    // A line, and curves, from a point beside the mark whose points, after a surplus, stay beside
    // it: read from the front, the surplus was a point under it.
    for curve in [
        &b"100 100 500 m 510 510 l S"[..],
        b"500 500 m 100 100 510 l S",
        b"100 500 m 100 100 500 110 500 120 500 c f",
        b"100 500 m 100 110 500 120 500 v f",
        b"100 500 m 100 110 500 120 500 y f",
    ] {
        let mut doc = one_page(curve, "", vec![]);
        mark(&mut doc, 0, &[[90.0, 90.0, 150.0, 150.0]], "");
        assert_eq!(apply(&mut doc, None).unwrap().paths_removed, 0, "{}", String::from_utf8_lossy(curve));
    }
}

/// An `MCID` given by reference: in a named property list, and in a structure element's `/MCR`.
/// Read directly, it was missed, and the element kept the removed text.
#[test]
fn an_mcid_given_by_reference_loses_what_redaction_removed() {
    for (content, properties, kid) in [
        (b"/P /MC0 BDC BT /F1 10 Tf 10 200 Td (SECRET) Tj ET EMC".as_slice(), "/Properties << /MC0 << /MCID 10 0 R >> >>", "0"),
        (b"/P <</MCID 0>> BDC BT /F1 10 Tf 10 200 Td (SECRET) Tj ET EMC", "", "<< /Type /MCR /Pg 3 0 R /MCID 10 0 R >>"),
    ] {
        let mut doc = pdf(vec![
            b"<< /Type /Catalog /Pages 2 0 R /StructTreeRoot 7 0 R /MarkInfo << /Marked true >> >>".to_vec(),
            b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_vec(),
            format!("<< /Type /Page /Parent 2 0 R /MediaBox [0 0 300 300] /Contents 4 0 R /StructParents 0 /Resources << /Font << /F1 5 0 R >> {properties} >> >>").into_bytes(),
            stream("", content),
            FONT.replace("95 0 R", "6 0 R").into_bytes(),
            widths(),
            b"<< /Type /StructTreeRoot /K 8 0 R >>".to_vec(),
            b"<< /S /Document /P 7 0 R /K [9 0 R] >>".to_vec(),
            format!("<< /S /P /P 8 0 R /Pg 3 0 R /ActualText (SECRET) /Alt (the SECRET) /K {kid} >>").into_bytes(),
            b"0".to_vec(),
        ]);
        mark(&mut doc, 0, &[[5.0, 195.0, 60.0, 212.0]], "");
        assert_eq!(apply(&mut doc, None).unwrap().tags, 1, "{properties}{kid}");
        let secret = doc.get(ObjRef::new(9, 0)).as_dict().cloned().unwrap();
        assert!(secret.get(b"ActualText").is_none() && secret.get(b"Alt").is_none(), "{secret:?}");
    }
    // An `/MCR` naming the same MCID in a form's stream (`/Stm`) is other content: it stays.
    let mut doc = pdf(vec![
        b"<< /Type /Catalog /Pages 2 0 R /StructTreeRoot 7 0 R /MarkInfo << /Marked true >> >>".to_vec(),
        b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_vec(),
        b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 300 300] /Contents 4 0 R /StructParents 0 /Resources << /Font << /F1 5 0 R >> >> >>".to_vec(),
        stream("", b"/P <</MCID 0>> BDC BT /F1 10 Tf 10 200 Td (SECRET) Tj ET EMC"),
        FONT.replace("95 0 R", "6 0 R").into_bytes(),
        widths(),
        b"<< /Type /StructTreeRoot /K 8 0 R >>".to_vec(),
        b"<< /S /Document /P 7 0 R /K [9 0 R] >>".to_vec(),
        b"<< /S /P /P 8 0 R /Pg 3 0 R /ActualText (FORMTEXT) /K << /Type /MCR /Pg 3 0 R /Stm 11 0 R /MCID 10 0 R >> >>".to_vec(),
        b"0".to_vec(),
        stream("/Type /XObject /Subtype /Form /BBox [0 0 300 300]", b"/P <</MCID 0>> BDC EMC"),
    ]);
    mark(&mut doc, 0, &[[5.0, 195.0, 60.0, 212.0]], "");
    apply(&mut doc, None).unwrap();
    let other = doc.get(ObjRef::new(9, 0)).as_dict().cloned().unwrap();
    assert!(other.get(b"ActualText").is_some(), "{other:?}");
}

/// The resources of page `page`, resolved.
fn resources_of(doc: &Document, page: usize) -> Dict {
    let page = pdfcraft_model::pages(doc).swap_remove(page);
    page.dict.get(b"Resources").map(|r| doc.resolve(r)).and_then(|r| r.as_dict().cloned()).unwrap_or_default()
}

/// `dict`'s dictionary `key`, resolved.
fn dict_in(doc: &Document, dict: &Dict, key: &[u8]) -> Dict {
    dict.get(key).map(|d| doc.resolve(d)).and_then(|d| d.as_dict().cloned()).unwrap_or_default()
}

/// A property list a second page sharing the resources names after a surplus operand, once the
/// first page's block that named it was redacted: read from the front, the second page's name went
/// unseen and the list was retired from under it.
#[test]
fn a_property_list_another_page_names_after_a_surplus_operand_is_kept() {
    use pdfcraft_cos::Stream;
    let mut doc = one_page(
        b"/Span /P0 BDC BT /F1 10 Tf 10 200 Td (SECRET) Tj ET EMC",
        "/Properties << /P0 7 0 R >>",
        vec![b"<< /ActualText (SECRET) >>".to_vec()],
    );
    let shared = doc.add(Object::Dict(resources_of(&doc, 0)));
    doc.update_dict(ObjRef::new(3, 0), |d| d.set(b"Resources".to_vec(), Object::Ref(shared))).unwrap();
    let mut second = doc.get(ObjRef::new(3, 0)).as_dict().cloned().unwrap();
    let contents = doc.add(Object::Stream(Stream::from_raw(Dict::new(), b"(x) /Span /P0 BDC BT /F1 10 Tf 10 100 Td (Other) Tj ET EMC".to_vec())));
    second.set(b"Contents".to_vec(), Object::Ref(contents));
    let second = doc.add(Object::Dict(second));
    doc.update_dict(ObjRef::new(2, 0), |d| {
        d.set(b"Kids".to_vec(), Object::Array(vec![Object::Ref(ObjRef::new(3, 0)), Object::Ref(second)]));
        d.set(b"Count".to_vec(), Object::Int(2));
    })
    .unwrap();
    mark(&mut doc, 0, &[[5.0, 195.0, 60.0, 212.0]], "");
    apply(&mut doc, Some(&[0])).unwrap();
    let properties = dict_in(&doc, &resources_of(&doc, 1), b"Properties");
    assert!(properties.contains(b"P0"), "the second page's property list stays: {properties:?}");
}

/// A Type 3 font set only by an ExtGState's `/Font`, selected after a surplus operand: read from the
/// front, the cleanups took it for undrawn, dropped it from the page and unlinked its glyph.
#[test]
fn a_type3_font_an_extgstate_selects_after_a_surplus_operand_is_kept() {
    let mut doc = pdf(vec![
        b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
        b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_vec(),
        b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 300 300] /Contents 4 0 R /Resources << /Font << /F1 5 0 R /T3 7 0 R >> /ExtGState << /GS1 << /Font [7 0 R 12] >> >> >> >>".to_vec(),
        stream("", b"BT /F1 10 Tf 20 260 Td (SECRET) Tj ET BT (x) /GS1 gs 20 20 Td (a) Tj ET"),
        FONT.replace("95 0 R", "6 0 R").into_bytes(),
        widths(),
        b"<< /Type /Font /Subtype /Type3 /FontBBox [0 0 10 10] /FontMatrix [0.1 0 0 0.1 0 0] /CharProcs << /a 8 0 R /b 9 0 R >> \
           /Encoding << /Differences [97 /a /b] >> /FirstChar 97 /LastChar 98 /Widths [10 10] >>"
            .to_vec(),
        stream("", b"10 0 d0 0 0 10 10 re f"),
        stream("", b"10 0 d0 0 0 5 5 re f"),
    ]);
    mark(&mut doc, 0, &[[0.0, 200.0, 300.0, 300.0]], "");
    assert_eq!(apply(&mut doc, None).unwrap().glyphs, 6);
    let fonts = dict_in(&doc, &resources_of(&doc, 0), b"Font");
    assert!(fonts.contains(b"T3"), "the page still lists the font: {fonts:?}");
    let font = doc.get(ObjRef::new(7, 0)).as_dict().cloned().unwrap_or_default();
    let procs = dict_in(&doc, &font, b"CharProcs");
    assert!(procs.contains(b"a"), "the glyph the page draws stays: {procs:?}");
}

/// A Type 3 font without resources of its own, set by an ExtGState selected after a surplus operand:
/// its glyph paints through the page's resources, so it may paint with the page's pattern, which
/// stays once the page's own fill with it was redacted (the text itself fills with grey). Read from the front, the font was taken
/// for one with resources, and the pattern was pruned.
#[test]
fn a_type3_font_without_resources_an_extgstate_selects_after_a_surplus_operand_keeps_the_pattern() {
    let mut doc = pdf(vec![
        b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
        b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_vec(),
        b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 300 300] /Contents 4 0 R /Resources << /Font << /F1 5 0 R >> /Pattern << /P0 9 0 R >> /ExtGState << /GS1 << /Font [7 0 R 12] >> >> >> >>".to_vec(),
        stream("", b"/Pattern cs /P0 scn 10 260 20 20 re f 0 g BT (x) /GS1 gs 150 20 Td (a) Tj ET"),
        FONT.replace("95 0 R", "6 0 R").into_bytes(),
        widths(),
        b"<< /Type /Font /Subtype /Type3 /FontBBox [0 0 10 10] /FontMatrix [0.1 0 0 0.1 0 0] /CharProcs << /a 8 0 R >> \
           /Encoding << /Differences [97 /a] >> /FirstChar 97 /LastChar 97 /Widths [10] >>"
            .to_vec(),
        stream("", b"10 0 d0 /Pattern cs /P0 scn 0 0 10 10 re f"),
        stream("/PatternType 1 /PaintType 1 /TilingType 1 /BBox [0 0 10 10] /XStep 10 /YStep 10 /Resources << >>", b"0 0 5 5 re f"),
    ]);
    mark(&mut doc, 0, &[[0.0, 250.0, 40.0, 290.0]], "");
    let r = apply(&mut doc, None).unwrap();
    assert_eq!((r.paths_removed, r.patterns_removed), (1, 0));
}
