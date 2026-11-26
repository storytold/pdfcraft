//! Fills and styles read from decks written by other tools.

use std::io::{Cursor, Read, Write};

use deckcraft_geom::Xfrm;
use deckcraft_model::style::Fill;
use deckcraft_model::{Shape, ShapeId, defaults};

const STYLE_GUID: &str = "{ABCDEF01-2345-4678-9ABC-DEF012345678}";

/// A table style definition as PowerPoint writes one: red table background, see-through white
/// bands, 2 pt blue outer borders and no inside lines.
fn style_xml() -> String {
    format!(
        r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<a:tblStyleLst xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main" def="{STYLE_GUID}"><a:tblStyle styleId="{STYLE_GUID}" styleName="Probe Style"><a:tblBg><a:fillRef idx="1"><a:srgbClr val="FF0000"/></a:fillRef></a:tblBg><a:wholeTbl><a:tcTxStyle b="on"><a:srgbClr val="00FF00"/></a:tcTxStyle><a:tcStyle><a:tcBdr><a:left><a:ln w="25400"><a:solidFill><a:srgbClr val="0000FF"/></a:solidFill></a:ln></a:left><a:insideH><a:ln><a:noFill/></a:ln></a:insideH></a:tcBdr><a:fill><a:noFill/></a:fill></a:tcStyle></a:wholeTbl><a:band1H><a:tcStyle><a:tcBdr/><a:fill><a:solidFill><a:srgbClr val="FFFFFF"><a:alpha val="20000"/></a:srgbClr></a:solidFill></a:fill></a:tcStyle></a:band1H></a:tblStyle></a:tblStyleLst>"#
    )
}

/// An exported deck with a 3x2 table using the probe style.
fn table_deck() -> Vec<u8> {
    let mut p = defaults::new_presentation(None);
    let id = ShapeId(p.alloc_id().expect("ids"));
    let first = p.slides[0].id;
    let mut t = deckcraft_model::Table::new(3, 2, 300.0, 30.0);
    t.style = "medium2-accent1".into();
    p.slide_mut(first).expect("slide").shapes.push(Shape {
        id,
        name: "Styled".into(),
        xfrm: Some(Xfrm::new(10.0, 10.0, 300.0, 90.0)),
        kind: deckcraft_model::ShapeKind::Table(t),
        ..Default::default()
    });
    let bytes = deckcraft_pptx::export(&p).expect("export");
    // Point the table at the probe style and replace the style part.
    let mut z = zip::ZipArchive::new(Cursor::new(bytes)).expect("zip");
    let mut out = zip::ZipWriter::new(Cursor::new(Vec::new()));
    let o = zip::write::SimpleFileOptions::default();
    for i in 0..z.len() {
        let mut f = z.by_index(i).expect("entry");
        let mut d = vec![];
        f.read_to_end(&mut d).expect("read");
        let name = f.name().to_string();
        if name == "ppt/tableStyles.xml" {
            d = style_xml().into_bytes();
        } else if name.starts_with("ppt/slides/slide") && name.ends_with(".xml") {
            let s = String::from_utf8(d).expect("utf8");
            let start = s.find("<a:tableStyleId>").map(|i| i + "<a:tableStyleId>".len());
            let end = s.find("</a:tableStyleId>");
            d = match (start, end) {
                (Some(a), Some(b)) => format!("{}{}{}", &s[..a], STYLE_GUID, &s[b..]).into_bytes(),
                _ => s.into_bytes(),
            };
        }
        out.start_file(name, o).expect("start");
        out.write_all(&d).expect("write");
    }
    out.finish().expect("finish").into_inner()
}

#[test]
fn table_styles_the_file_defines_are_read_used_and_kept() {
    use deckcraft_model::table::{TableBorder, TablePart};
    let p = deckcraft_pptx::import(&table_deck()).expect("import");
    let def = p.table_styles.iter().find(|d| d.id == STYLE_GUID).expect("the probe style is read");
    assert_eq!(def.name, "Probe Style");
    let bg = def.background.as_ref().expect("table background");
    assert!(matches!(bg.fill_ref, Some((1, _))));
    let whole = def.part(TablePart::WholeTbl).expect("whole table");
    assert_eq!(whole.bold, Some(true));
    assert!(whole.text_color.is_some());
    assert!(matches!(&whole.borders[0], Some(TableBorder::Line { line }) if line.width == Some(2.0)));
    assert!(matches!(&whole.borders[4], Some(TableBorder::Line { line }) if matches!(line.fill, Some(Fill::None))));
    assert!(matches!(def.part(TablePart::Band1H).and_then(|b| b.fill.clone()), Some(Fill::Solid { .. })));
    // The table keeps the style's GUID, so it's drawn from the definition.
    let s = p.slides[0].shapes.iter().find(|s| s.name == "Styled").expect("table");
    let deckcraft_model::ShapeKind::Table(t) = &s.kind else { panic!("table") };
    assert_eq!(t.style, STYLE_GUID);
    // Saving writes the definition back, unchanged.
    let again = deckcraft_pptx::export(&p).expect("export");
    let q = deckcraft_pptx::import(&again).expect("reimport");
    let def2 = q.table_styles.iter().find(|d| d.id == STYLE_GUID).expect("kept on save");
    assert_eq!(def2.parts, def.parts);
}

#[test]
fn table_style_no_table_uses_is_kept_on_save() {
    let mut p = deckcraft_pptx::import(&table_deck()).expect("import");
    assert!(p.table_styles.iter().any(|d| d.id == STYLE_GUID));
    // Drop the only table that uses the style; the deck still defines it.
    for s in &mut p.slides {
        std::sync::Arc::make_mut(s).shapes.retain(|sh| !matches!(sh.kind, deckcraft_model::ShapeKind::Table(_)));
    }
    let q = deckcraft_pptx::import(&deckcraft_pptx::export(&p).expect("export")).expect("reimport");
    let d = q.table_styles.iter().find(|d| d.id == STYLE_GUID).expect("style kept");
    assert_eq!(d.name, "Probe Style");
}
