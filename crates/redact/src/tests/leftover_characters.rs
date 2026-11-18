//! A character only the redacted text showed leaves its font's `/ToUnicode` map and `/Widths`.

use super::*;

/// Page 1 shows "KEEP ab" and, at y 150, "QZ"; page 2 shows "Z". All in F1, a subset with a
/// ToUnicode map, whose text the mark over "QZ" removes. With `acroform`, form fields write with F1.
fn two_pages(acroform: bool) -> Document {
    let form = if acroform { "/AcroForm << /DR << /Font << /Helv 5 0 R >> >> >>" } else { "" };
    let page = |c: u32| format!("<< /Type /Page /Parent 2 0 R /MediaBox [0 0 300 300] /Contents {c} 0 R /Resources << /Font << /F1 5 0 R >> >> >>");
    let map = "1 begincodespacerange <00> <FF> endcodespacerange 8 beginbfchar <20> <0020> <45> <0045> <4B> <004B> <50> <0050> <51> <0051> <5A> <005A> <61> <0061> <62> <0062> endbfchar";
    let mut doc = pdf(vec![
        format!("<< /Type /Catalog /Pages 2 0 R {form} >>").into_bytes(),
        b"<< /Type /Pages /Kids [3 0 R 8 0 R] /Count 2 >>".to_vec(),
        page(4).into_bytes(),
        stream("", b"BT /F1 10 Tf 20 250 Td (KEEP ab) Tj 0 -100 Td (QZ) Tj ET"),
        b"<< /Type /Font /Subtype /Type1 /BaseFont /ABCDEF+Helvetica /Encoding /WinAnsiEncoding /FirstChar 32 /LastChar 126 /Widths 6 0 R /ToUnicode 7 0 R >>".to_vec(),
        widths(),
        stream("", map.as_bytes()),
        page(9).into_bytes(),
        stream("", b"BT /F1 10 Tf 20 250 Td (Z) Tj ET"),
    ]);
    mark(&mut doc, 0, &[[10.0, 140.0, 100.0, 165.0]], "");
    doc
}

/// F1's widths of Q, Z and K, and its ToUnicode map.
fn lists(doc: &Document) -> ([f64; 3], String) {
    let font = doc.get(ObjRef::new(5, 0)).as_dict().cloned().unwrap();
    let widths: Vec<f64> = doc.resolve(font.get(b"Widths").unwrap()).as_array().unwrap().iter().filter_map(Object::as_f64).collect();
    let at = |c: u8| widths[usize::from(c) - 32];
    let Object::Stream(map) = &*doc.resolve(font.get(b"ToUnicode").unwrap()) else { panic!("no ToUnicode") };
    ([at(b'Q'), at(b'Z'), at(b'K')], String::from_utf8_lossy(&map.decoded().unwrap()).into_owned())
}

/// Q is shown nowhere after the apply: it leaves F1's map and its width is 0. Z stays, page 2
/// still shows it, and so does K, which page 1 still shows.
#[test]
fn characters_only_the_redacted_text_used_leave_the_fonts() {
    let mut doc = two_pages(false);
    assert_eq!(apply(&mut doc, None).unwrap().glyphs, 2);
    let (widths, map) = lists(&doc);
    assert_eq!(widths, [0.0, 500.0, 500.0], "widths of Q, Z and K");
    assert!(!map.contains("<51>") && map.contains("<5A> <005A>") && map.contains("<4B> <004B>"), "{map}");
}

/// A font the form fields write with may show any character later: it is left as it was.
#[test]
fn a_font_the_form_fields_use_is_left_alone() {
    let mut doc = two_pages(true);
    assert_eq!(apply(&mut doc, None).unwrap().glyphs, 2);
    let (widths, map) = lists(&doc);
    assert_eq!(widths, [500.0, 500.0, 500.0], "widths of Q, Z and K");
    assert!(map.contains("<51> <0051>"), "{map}");
}
