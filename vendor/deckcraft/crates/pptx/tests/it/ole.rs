//! Embedded (OLE) objects and their preview pictures, as other versions of PowerPoint write them.

use std::io::{Cursor, Read, Write};

use deckcraft_model::{ShapeKind, defaults};

fn png() -> Vec<u8> {
    let mut out = Cursor::new(vec![]);
    image::DynamicImage::ImageRgb8(image::RgbImage::from_pixel(4, 4, image::Rgb([10, 200, 30])))
        .write_to(&mut out, image::ImageFormat::Png)
        .expect("png");
    out.into_inner()
}

/// An exported deck with `parts` added (or replaced) and `edit` applied to every part's text.
fn with_parts(parts: &[(&str, Vec<u8>)], edit: impl Fn(&str, String) -> String) -> Vec<u8> {
    let base = deckcraft_pptx::export(&defaults::new_presentation(None)).expect("export");
    let mut z = zip::ZipArchive::new(Cursor::new(base)).expect("zip");
    let mut out = zip::ZipWriter::new(Cursor::new(Vec::new()));
    let o = zip::write::SimpleFileOptions::default();
    for i in 0..z.len() {
        let mut f = z.by_index(i).expect("entry");
        let name = f.name().to_string();
        if parts.iter().any(|(n, _)| *n == name) {
            continue;
        }
        let mut d = vec![];
        f.read_to_end(&mut d).expect("read");
        if name.ends_with(".xml") || name.ends_with(".rels") {
            d = edit(&name, String::from_utf8(d).expect("utf8")).into_bytes();
        }
        out.start_file(name, o).expect("start");
        out.write_all(&d).expect("write");
    }
    for (name, data) in parts {
        out.start_file(*name, o).expect("start");
        out.write_all(data).expect("write");
    }
    out.finish().expect("finish").into_inner()
}

/// PowerPoint 2007 style: `p:oleObj spid` with no `p:pic`; the preview is the `v:imagedata` of the
/// VML shape with that id in the slide's legacy drawing part.
fn ole_deck(spid_in_vml: &str) -> Vec<u8> {
    let vml = format!(
        r#"<xml xmlns:v="urn:schemas-microsoft-com:vml" xmlns:o="urn:schemas-microsoft-com:office:office">
 <v:shape id="_x0000_s9999" style='position:absolute;left:0;top:0;width:1in;height:1in'><v:imagedata o:relid="rId7" o:title=""/></v:shape>
 <v:shape id="{spid_in_vml}" style='position:absolute;left:0;top:0;width:2in;height:1in'><v:imagedata o:relid="rId1" o:title=""/></v:shape>
</xml>"#
    );
    let vml_rels = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/image" Target="../media/preview.png"/><Relationship Id="rId7" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/image" Target="../media/other.png"/></Relationships>"#;
    let frame = r#"<p:graphicFrame><p:nvGraphicFramePr><p:cNvPr id="900" name="Object 3"/><p:cNvGraphicFramePr/><p:nvPr/></p:nvGraphicFramePr><p:xfrm><a:off x="0" y="0"/><a:ext cx="1828800" cy="914400"/></p:xfrm><a:graphic><a:graphicData uri="http://schemas.openxmlformats.org/presentationml/2006/ole"><p:oleObj spid="_x0000_s681987" name="Slide" r:id="rIdOle" imgW="4664160" imgH="3497400" progId="PowerPoint.Slide.8"><p:embed/></p:oleObj></a:graphicData></a:graphic></p:graphicFrame>"#;
    with_parts(
        &[
            ("ppt/drawings/vmlDrawing1.vml", vml.into_bytes()),
            ("ppt/drawings/_rels/vmlDrawing1.vml.rels", vml_rels.as_bytes().to_vec()),
            ("ppt/media/preview.png", png()),
            ("ppt/media/other.png", vec![1, 2, 3]),
            ("ppt/embeddings/oleObject1.bin", vec![0; 16]),
        ],
        |name, s| {
            if name == "ppt/slides/slide1.xml" {
                s.replacen("</p:spTree>", &format!("{frame}</p:spTree>"), 1)
            } else if name == "ppt/slides/_rels/slide1.xml.rels" {
                s.replacen(
                    "</Relationships>",
                    r#"<Relationship Id="rIdVml" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/vmlDrawing" Target="../drawings/vmlDrawing1.vml"/><Relationship Id="rIdOle" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/oleObject" Target="../embeddings/oleObject1.bin"/></Relationships>"#,
                    1,
                )
            } else {
                s
            }
        },
    )
}

fn object_preview(bytes: &[u8]) -> Option<Vec<u8>> {
    let p = deckcraft_pptx::import(bytes).expect("import");
    let s = p.slides[0].shapes.iter().find(|s| s.name == "Object 3").expect("object");
    match &s.kind {
        ShapeKind::Opaque { preview, label, .. } => {
            assert_eq!(label, "PowerPoint.Slide.8");
            preview.and_then(|m| p.media(m)).map(|m| m.data.to_vec())
        }
        k => panic!("{k:?}"),
    }
}

#[test]
fn an_old_ole_object_shows_its_vml_preview() {
    assert_eq!(object_preview(&ole_deck("_x0000_s681987")), Some(png()), "the VML shape with the object's spid");
}

#[test]
fn an_ole_object_without_a_matching_vml_shape_has_no_preview() {
    assert_eq!(object_preview(&ole_deck("_x0000_s1")), None);
}

/// PowerPoint 2010 and later: `p:oleObj` with a fallback `p:pic` inside `mc:AlternateContent`, whose
/// `spPr` can give the object a fill and an outline (old equation objects on coloured boxes).
#[test]
fn an_ole_objects_fallback_picture_gives_it_its_fill_and_outline() {
    let pic = |spr: &str| {
        format!(
            r#"<p:graphicFrame><p:nvGraphicFramePr><p:cNvPr id="900" name="Object 3"/><p:cNvGraphicFramePr/><p:nvPr/></p:nvGraphicFramePr><p:xfrm><a:off x="0" y="0"/><a:ext cx="1828800" cy="914400"/></p:xfrm><a:graphic><a:graphicData uri="http://schemas.openxmlformats.org/presentationml/2006/ole"><mc:AlternateContent xmlns:mc="http://schemas.openxmlformats.org/markup-compatibility/2006"><mc:Choice xmlns:v="urn:schemas-microsoft-com:vml" Requires="v"><p:oleObj name="Equation" r:id="rIdOle" imgW="1663700" imgH="228600" progId="Equation.3"><p:embed/></p:oleObj></mc:Choice><mc:Fallback><p:oleObj name="Equation" r:id="rIdOle" imgW="1663700" imgH="228600" progId="Equation.3"><p:embed/><p:pic><p:nvPicPr><p:cNvPr id="0" name="Object 3"/><p:cNvPicPr/><p:nvPr/></p:nvPicPr><p:blipFill><a:blip r:embed="rIdImg"/><a:stretch><a:fillRect/></a:stretch></p:blipFill><p:spPr><a:xfrm><a:off x="0" y="0"/><a:ext cx="1828800" cy="914400"/></a:xfrm><a:prstGeom prst="rect"><a:avLst/></a:prstGeom>{spr}</p:spPr></p:pic></p:oleObj></mc:Fallback></mc:AlternateContent></a:graphicData></a:graphic></p:graphicFrame>"#
        )
    };
    let deck = |frame: String| {
        with_parts(&[("ppt/media/eq.png", png()), ("ppt/embeddings/oleObject1.bin", vec![0; 16])], move |name, s| {
            if name == "ppt/slides/slide1.xml" {
                s.replacen("</p:spTree>", &format!("{frame}</p:spTree>"), 1)
            } else if name == "ppt/slides/_rels/slide1.xml.rels" {
                s.replacen(
                    "</Relationships>",
                    r#"<Relationship Id="rIdImg" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/image" Target="../media/eq.png"/><Relationship Id="rIdOle" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/oleObject" Target="../embeddings/oleObject1.bin"/></Relationships>"#,
                    1,
                )
            } else {
                s
            }
        })
    };
    let object = |bytes: Vec<u8>| {
        let p = deckcraft_pptx::import(&bytes).expect("import");
        let s = p.slides[0].shapes.iter().find(|s| s.name == "Object 3").expect("object").clone();
        assert!(matches!(&s.kind, ShapeKind::Opaque { preview: Some(_), label, .. } if label == "Equation.3"), "{:?}", s.kind);
        s
    };
    let filled = object(deck(pic(
        r#"<a:solidFill><a:srgbClr val="FF9900"/></a:solidFill><a:ln w="12700"><a:solidFill><a:srgbClr val="0000FF"/></a:solidFill></a:ln>"#,
    )));
    let orange = deckcraft_model::style::Fill::solid(deckcraft_model::ColorRef::rgb(deckcraft_color::Rgba::rgb(0xFF, 0x99, 0)));
    assert_eq!(filled.fill, Some(orange.clone()));
    assert_eq!(filled.line.as_ref().and_then(|l| l.width), Some(1.0));
    // Saved as its preview picture (its kept XML can't be written back), it keeps the fill.
    let mut p = deckcraft_pptx::import(&deck(pic(r#"<a:solidFill><a:srgbClr val="FF9900"/></a:solidFill>"#))).expect("import");
    for s in &mut std::sync::Arc::make_mut(&mut p.slides[0]).shapes {
        if let ShapeKind::Opaque { xml, .. } = &mut s.kind {
            xml.clear();
        }
    }
    let back = deckcraft_pptx::import(&deckcraft_pptx::export(&p).expect("export")).expect("reimport");
    let saved = back.slides[0].shapes.iter().find(|s| s.name == "Object 3").expect("object");
    assert!(matches!(saved.kind, ShapeKind::Picture { .. }), "{:?}", saved.kind);
    assert_eq!(saved.fill, Some(orange));
    // Without a fill of its own the object stays unfilled.
    assert_eq!(object(deck(pic(""))).fill, None);
}
