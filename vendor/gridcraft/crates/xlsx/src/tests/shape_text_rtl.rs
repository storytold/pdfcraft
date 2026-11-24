//! Text boxes and shapes keep their paragraphs' right-to-left direction (`a:pPr rtl`).

use std::io::Read;
use std::sync::Arc;

use gridcraft_model::{Anchor, Color, Shape, ShapeFormat, ShapeKind, ShapeTextAlign, Sheet, Workbook};

use super::minimal;
use crate::{read_xlsx, write_xlsx};

const ARABIC_TEXT_BOX: &str = r#"<xdr:wsDr xmlns:xdr="http://schemas.openxmlformats.org/drawingml/2006/spreadsheetDrawing" xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main">
<xdr:twoCellAnchor><xdr:from><xdr:col>1</xdr:col><xdr:colOff>0</xdr:colOff><xdr:row>1</xdr:row><xdr:rowOff>0</xdr:rowOff></xdr:from><xdr:to><xdr:col>4</xdr:col><xdr:colOff>0</xdr:colOff><xdr:row>4</xdr:row><xdr:rowOff>0</xdr:rowOff></xdr:to>
<xdr:sp macro="" textlink=""><xdr:nvSpPr><xdr:cNvPr id="2" name="TextBox 1"/><xdr:cNvSpPr txBox="1"/></xdr:nvSpPr>
<xdr:spPr><a:xfrm><a:off x="0" y="0"/><a:ext cx="1828800" cy="571500"/></a:xfrm><a:prstGeom prst="rect"><a:avLst/></a:prstGeom></xdr:spPr>
<xdr:txBody><a:bodyPr vertOverflow="clip" wrap="square" rtlCol="0" anchor="t"/><a:lstStyle/>
<a:p><a:pPr algn="r" rtl="1"/><a:r><a:rPr lang="ar-SA" sz="1100"/><a:t>مرحبا بالعالم</a:t></a:r></a:p>
</xdr:txBody></xdr:sp><xdr:clientData/></xdr:twoCellAnchor></xdr:wsDr>"#;

fn read_drawing(drawing: &str) -> Workbook {
    let rels = r#"<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/drawing" Target="../drawings/drawing1.xml"/></Relationships>"#;
    let bytes = minimal(
        r#"<sheetData/><drawing r:id="rId1"/>"#,
        &[("xl/worksheets/_rels/sheet1.xml.rels", rels), ("xl/drawings/drawing1.xml", drawing)],
        "",
        "",
    );
    read_xlsx(&bytes).unwrap().0
}

fn written_drawing(wb: &Workbook) -> String {
    let bytes = write_xlsx(wb).unwrap();
    let mut archive = zip::ZipArchive::new(std::io::Cursor::new(bytes)).unwrap();
    let mut drawing = String::new();
    archive.by_name("xl/drawings/drawing1.xml").unwrap().read_to_string(&mut drawing).unwrap();
    drawing
}

fn text_box(text: &str, format: ShapeFormat) -> Workbook {
    let mut sheet = Sheet::new("Sheet1");
    sheet.shapes.push(Shape {
        id: 1,
        kind: ShapeKind::TextBox,
        anchor: Anchor { width: 120.0, height: 40.0, ..Default::default() },
        fill: Color::Auto,
        line: Color::Auto,
        text: text.into(),
        format,
        rotation: 0.0,
        math: Default::default(),
    });
    Workbook { sheets: vec![Arc::new(sheet)], ..Default::default() }
}

#[test]
fn shape_text_keeps_right_to_left_paragraphs() {
    let wb = read_drawing(ARABIC_TEXT_BOX);
    let shape = &wb.sheet(0).unwrap().shapes[0];
    assert_eq!(shape.text, "مرحبا بالعالم");
    assert!(shape.format.rtl, "rtl=\"1\" on the first paragraph is read");
    assert_eq!(shape.format.align, Some(ShapeTextAlign::Right));

    let xml = written_drawing(&wb);
    assert!(xml.contains(r#"<a:pPr algn="r" rtl="1"/>"#), "saved paragraph keeps rtl: {xml}");

    let (reopened, _) = read_xlsx(&write_xlsx(&wb).unwrap()).unwrap();
    let after = &reopened.sheet(0).unwrap().shapes[0];
    assert!(after.format.rtl);
    assert_eq!(after.format.align, Some(ShapeTextAlign::Right));
    assert_eq!(after.text, "مرحبا بالعالم");

    let xml = written_drawing(&text_box("سطر\nآخر", ShapeFormat { rtl: true, ..Default::default() }));
    assert_eq!(xml.matches(r#"<a:pPr rtl="1"/>"#).count(), 2, "every paragraph is right-to-left without an alignment: {xml}");
}

#[test]
fn shape_text_without_rtl_writes_unchanged_ppr() {
    let wb = read_drawing(&ARABIC_TEXT_BOX.replace(r#" rtl="1""#, ""));
    assert!(!wb.sheet(0).unwrap().shapes[0].format.rtl);
    let wb = read_drawing(&ARABIC_TEXT_BOX.replace(r#" rtl="1""#, r#" rtl="0""#));
    assert!(!wb.sheet(0).unwrap().shapes[0].format.rtl);

    let xml = written_drawing(&text_box("left\nto right", ShapeFormat { align: Some(ShapeTextAlign::Right), ..Default::default() }));
    assert_eq!(xml.matches(r#"<a:pPr algn="r"/>"#).count(), 2, "{xml}");
    assert!(!xml.contains("rtl=\"1\""), "{xml}");

    let xml = written_drawing(&text_box("plain", ShapeFormat::default()));
    assert!(!xml.contains("<a:pPr"), "no paragraph properties without alignment or direction: {xml}");
}

#[test]
fn a_rotation_that_is_not_a_finite_number_reads_as_zero() {
    for rot in ["NaN", "inf", "-inf"] {
        let wb = read_drawing(&ARABIC_TEXT_BOX.replace("<a:xfrm>", &format!("<a:xfrm rot=\"{rot}\">")));
        assert_eq!(wb.sheet(0).unwrap().shapes[0].rotation, 0.0, "rot={rot}");
    }
}

#[test]
fn word_art_keeps_right_to_left_paragraphs() {
    let mut wb = text_box("مرحبا\nبالعالم", ShapeFormat { rtl: true, align: Some(ShapeTextAlign::Right), ..Default::default() });
    wb.sheet_mut(0).unwrap().shapes[0].kind = ShapeKind::WordArt;
    let xml = written_drawing(&wb);
    assert!(xml.contains(r#"<a:prstTxWarp prst="textNoShape">"#));
    assert_eq!(xml.matches(r#"<a:pPr algn="r" rtl="1"/>"#).count(), 2);
    let (reopened, _) = read_xlsx(&write_xlsx(&wb).unwrap()).unwrap();
    let shape = &reopened.sheet(0).unwrap().shapes[0];
    assert_eq!(shape.kind, ShapeKind::WordArt);
    assert!(shape.format.rtl);
    assert_eq!(shape.format.align, Some(ShapeTextAlign::Right));
    assert_eq!(shape.text, "مرحبا\nبالعالم");
}

#[test]
fn word_art_with_empty_text_keeps_its_kind() {
    let mut wb = text_box("", ShapeFormat::default());
    wb.sheet_mut(0).unwrap().shapes[0].kind = ShapeKind::WordArt;
    let (reopened, _) = read_xlsx(&write_xlsx(&wb).unwrap()).unwrap();
    assert_eq!(reopened.sheet(0).unwrap().shapes[0].kind, ShapeKind::WordArt);
}

#[test]
fn a_text_warp_on_a_filled_ellipse_does_not_make_it_word_art() {
    let drawing = ARABIC_TEXT_BOX
        .replace(r#"<xdr:cNvSpPr txBox="1"/>"#, "<xdr:cNvSpPr/>")
        .replace(r#"prst="rect""#, r#"prst="ellipse""#)
        .replace(r#"anchor="t"/>"#, r#"anchor="t"><a:prstTxWarp prst="textTriangle"><a:avLst/></a:prstTxWarp></a:bodyPr>"#);
    let wb = read_drawing(&drawing);
    assert_eq!(wb.sheet(0).unwrap().shapes[0].kind, ShapeKind::Ellipse);
}

/// `textNoShape` is the standard "no text warp" value, so on its own it does not make an ordinary
/// shape WordArt: a filled ellipse that carries it reads and saves as the same filled ellipse.
#[test]
fn an_explicit_no_warp_on_a_filled_ellipse_stays_an_ellipse() {
    let drawing = ARABIC_TEXT_BOX
        .replace(r#"<xdr:cNvSpPr txBox="1"/>"#, "<xdr:cNvSpPr/>")
        .replace(
            r#"<a:prstGeom prst="rect"><a:avLst/></a:prstGeom>"#,
            r#"<a:prstGeom prst="ellipse"><a:avLst/></a:prstGeom><a:solidFill><a:srgbClr val="FF0000"/></a:solidFill>"#,
        )
        .replace(r#"anchor="t"/>"#, r#"anchor="t"><a:prstTxWarp prst="textNoShape"><a:avLst/></a:prstTxWarp></a:bodyPr>"#);
    let wb = read_drawing(&drawing);
    let shape = &wb.sheet(0).unwrap().shapes[0];
    assert_eq!(shape.kind, ShapeKind::Ellipse);
    assert_eq!(shape.fill, Color::Rgb(0xFF0000));
    let xml = written_drawing(&wb);
    assert!(xml.contains(r#"<a:prstGeom prst="ellipse">"#), "{xml}");
    assert!(xml.contains(r#"<a:solidFill><a:srgbClr val="FF0000"/></a:solidFill>"#), "{xml}");
    assert!(!xml.contains(r#"txBox="1""#), "{xml}");
    let (reopened, _) = read_xlsx(&write_xlsx(&wb).unwrap()).unwrap();
    assert_eq!(reopened.sheet(0).unwrap().shapes[0].kind, ShapeKind::Ellipse);
    // The same warp on a filled rectangle that is not a text box keeps it a rectangle too.
    let rect = drawing.replace(r#"prst="ellipse""#, r#"prst="rect""#);
    assert_eq!(read_drawing(&rect).sheet(0).unwrap().shapes[0].kind, ShapeKind::Rectangle);
    // WordArt as written (a text box) or as a fill-less, line-less rectangle still reads as WordArt.
    let text_box = ARABIC_TEXT_BOX.replace(r#"anchor="t"/>"#, r#"anchor="t"><a:prstTxWarp prst="textNoShape"><a:avLst/></a:prstTxWarp></a:bodyPr>"#);
    assert_eq!(read_drawing(&text_box).sheet(0).unwrap().shapes[0].kind, ShapeKind::WordArt);
    let bare = text_box
        .replace(r#"<xdr:cNvSpPr txBox="1"/>"#, "<xdr:cNvSpPr/>")
        .replace(r#"<a:prstGeom prst="rect"><a:avLst/></a:prstGeom>"#, r#"<a:prstGeom prst="rect"><a:avLst/></a:prstGeom><a:noFill/>"#);
    assert_eq!(read_drawing(&bare).sheet(0).unwrap().shapes[0].kind, ShapeKind::WordArt);
}

/// #2170: a shape font typeface holding `"`, `<` or `&` is escaped as an attribute, so it can't
/// close the `typeface` attribute early and inject others; it survives a save and reopen.
#[test]
fn shape_font_typeface_is_attribute_escaped() {
    let font = r#"a"b<c&d' injected="x"#;
    let wb = text_box("hi", ShapeFormat { font: Some(font.into()), ..Default::default() });
    let xml = written_drawing(&wb);
    assert!(xml.contains(r#"<a:latin typeface="a&quot;b&lt;c&amp;d"#), "typeface is attribute-escaped: {xml}");
    assert!(!xml.contains(r#" injected="x""#), "no attribute injected: {xml}");

    let (reopened, _) = read_xlsx(&write_xlsx(&wb).unwrap()).unwrap();
    let after = &reopened.sheet(0).unwrap().shapes[0];
    assert_eq!(after.format.font.as_deref(), Some(font));
    assert_eq!(after.text, "hi");
}
