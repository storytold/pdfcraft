//! A character only the redacted text showed leaves its font's `/ToUnicode` map and `/Widths`.

use super::*;

/// F1's ToUnicode map.
const MAP: &str = "1 begincodespacerange <00> <FF> endcodespacerange 8 beginbfchar <20> <0020> <45> <0045> <4B> <004B> <50> <0050> <51> <0051> <5A> <005A> <61> <0061> <62> <0062> endbfchar";

/// Page 1 shows "KEEP ab" and, at y 150, "QZ" in F1, a subset with a ToUnicode map; the mark over
/// "QZ" removes them. Page 2 draws `page2` with the resources `res2`, objects 10 on being `extra`.
/// The catalog also holds `form`.
fn two_pages(form: &str, page2: &[u8], res2: &str, extra: Vec<Vec<u8>>) -> Document {
    let page = |c: u32, res: &str| format!("<< /Type /Page /Parent 2 0 R /MediaBox [0 0 300 300] /Contents {c} 0 R /Resources << {res} >> >>");
    let mut objs = vec![
        format!("<< /Type /Catalog /Pages 2 0 R {form} >>").into_bytes(),
        b"<< /Type /Pages /Kids [3 0 R 8 0 R] /Count 2 >>".to_vec(),
        page(4, "/Font << /F1 5 0 R >>").into_bytes(),
        stream("", b"BT /F1 10 Tf 20 250 Td (KEEP ab) Tj 0 -100 Td (QZ) Tj ET"),
        b"<< /Type /Font /Subtype /Type1 /BaseFont /ABCDEF+Helvetica /Encoding /WinAnsiEncoding /FirstChar 32 /LastChar 126 /Widths 6 0 R /ToUnicode 7 0 R >>".to_vec(),
        widths(),
        stream("", MAP.as_bytes()),
        page(9, res2).into_bytes(),
        stream("", page2),
    ];
    objs.extend(extra);
    let mut doc = pdf(objs);
    mark(&mut doc, 0, &[[10.0, 140.0, 100.0, 165.0]], "");
    doc
}

/// Page 2 shows "Z" in F1.
fn shows_z(form: &str) -> Document {
    two_pages(form, b"BT /F1 10 Tf 20 250 Td (Z) Tj ET", "/Font << /F1 5 0 R >>", Vec::new())
}

/// F1's widths and its ToUnicode map.
fn lists(doc: &Document) -> (Vec<f64>, String) {
    let font = doc.get(ObjRef::new(5, 0)).as_dict().cloned().unwrap();
    let widths: Vec<f64> = doc.resolve(font.get(b"Widths").unwrap()).as_array().unwrap().iter().filter_map(Object::as_f64).collect();
    let Object::Stream(map) = &*doc.resolve(font.get(b"ToUnicode").unwrap()) else { panic!("no ToUnicode") };
    (widths, String::from_utf8_lossy(&doc.with_direct_parms(map).decoded().unwrap()).into_owned())
}

/// The widths of Q, Z and K.
fn qzk(widths: &[f64]) -> [f64; 3] {
    (*b"QZK").map(|c| widths[usize::from(c) - 32])
}

/// Q is shown nowhere after the apply: it leaves F1's map and its width is 0. Z stays, page 2
/// still shows it, and so does K, which page 1 still shows. Also when the map's `/DecodeParms` is
/// given by reference.
#[test]
fn characters_only_the_redacted_text_used_leave_the_fonts() {
    for indirect in [false, true] {
        let mut doc = shows_z("");
        if indirect {
            let map = predicted(&mut doc, pdfcraft_cos::Dict::new(), MAP.as_bytes());
            doc.set(ObjRef::new(7, 0), map);
        }
        assert_eq!(apply(&mut doc, None).unwrap().glyphs, 2);
        let (widths, map) = lists(&doc);
        assert_eq!(qzk(&widths), [0.0, 500.0, 500.0], "widths of Q, Z and K, parameters by reference: {indirect}");
        assert!(!map.contains("<51>") && map.contains("<5A> <005A>") && map.contains("<4B> <004B>"), "{map}");
    }
}

/// A font the form fields write with may show any character later, and so may any font when the
/// form's `/DR` can't be read: it is left as it was.
#[test]
fn a_font_the_form_fields_use_is_left_alone() {
    for form in ["/AcroForm << /DR << /Font << /Helv 5 0 R >> >> >>", "/AcroForm << /DR [5 0 R] >>"] {
        let mut doc = shows_z(form);
        let before = lists(&doc);
        assert_eq!(apply(&mut doc, None).unwrap().glyphs, 2);
        assert_eq!(lists(&doc), before, "{form}");
    }
}

/// A map that builds on another through its dictionary's `/UseCMap` is left as it is, the link
/// to its base included: only the widths go.
#[test]
fn a_map_built_on_another_is_left_as_it_is() {
    let mut doc = shows_z("");
    let dict: pdfcraft_cos::Dict = [(b"UseCMap".to_vec(), Object::name("Base"))].into_iter().collect();
    doc.set(ObjRef::new(7, 0), pdfcraft_cos::Stream::flate(dict, MAP.as_bytes()));
    assert_eq!(apply(&mut doc, None).unwrap().glyphs, 2);
    assert_eq!(qzk(&lists(&doc).0)[0], 0.0, "Q's width");
    let font = doc.get(ObjRef::new(5, 0)).as_dict().cloned().unwrap();
    assert_eq!(font.get(b"ToUnicode").and_then(Object::as_ref), Some(ObjRef::new(7, 0)), "the map");
}

/// Page 2 still shows Q, through a form whose `/Subtype` is given by reference: Q stays.
#[test]
fn text_in_a_form_named_by_reference_keeps_its_characters() {
    let form = stream("/Subtype 11 0 R /BBox [0 0 300 300] /Resources << /Font << /F1 5 0 R >> >>", b"BT /F1 10 Tf 20 250 Td (Q) Tj ET");
    let mut doc = two_pages("", b"/Fm Do", "/XObject << /Fm 10 0 R >>", vec![form, b"/Form".to_vec()]);
    assert_eq!(apply(&mut doc, None).unwrap().glyphs, 2);
    let (widths, map) = lists(&doc);
    assert_eq!(qzk(&widths)[0], 500.0, "Q's width");
    assert!(map.contains("<51> <0051>"), "{map}");
}

/// Page 2 shows a Type 3 font whose `/Subtype` is given by reference. Its glyph shows Q in F1,
/// which the scan doesn't read, so every font is left as it was.
#[test]
fn a_type3_font_named_by_reference_leaves_the_fonts_alone() {
    let type3 = b"<< /Type /Font /Subtype 11 0 R /FontBBox [0 0 1000 1000] /FontMatrix [0.001 0 0 0.001 0 0] /FirstChar 65 /LastChar 65 /Widths [500] /Encoding << /Differences [65 /A] >> /CharProcs << /A 12 0 R >> /Resources << /Font << /F1 5 0 R >> >> >>";
    let glyph = stream("", b"500 0 d0 BT /F1 10 Tf (Q) Tj ET");
    let page2 = b"BT /F3 10 Tf 20 250 Td (A) Tj ET";
    let mut doc = two_pages("", page2, "/Font << /F1 5 0 R /F3 10 0 R >>", vec![type3.to_vec(), b"/Type3".to_vec(), glyph]);
    let before = lists(&doc);
    assert_eq!(apply(&mut doc, None).unwrap().glyphs, 2);
    assert_eq!(lists(&doc), before);
}

/// A hidden page template (`/Names /Templates`) is out of the page tree, but a form can show it
/// later. It shows Q, so Q keeps its width and its map entry; so it does when the name dictionary
/// can't be read.
#[test]
fn a_hidden_page_template_keeps_its_characters() {
    let template = b"<< /Type /Template /MediaBox [0 0 300 300] /Contents 11 0 R /Resources << /Font << /F1 5 0 R >> >> >>";
    let body = stream("", b"BT /F1 10 Tf 20 250 Td (Q) Tj ET");
    for names in ["/Names << /Templates << /Names [(extra) 10 0 R] >> >>", "/Names 99 0 R"] {
        let mut doc = two_pages(names, b"", "", vec![template.to_vec(), body.clone()]);
        assert_eq!(apply(&mut doc, None).unwrap().glyphs, 2);
        let (widths, map) = lists(&doc);
        assert_eq!(qzk(&widths)[0], 500.0, "Q's width, {names}");
        assert!(map.contains("<51> <0051>"), "{map}");
    }
}
