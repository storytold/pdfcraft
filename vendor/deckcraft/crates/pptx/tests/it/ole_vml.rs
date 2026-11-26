//! Legacy OLE objects whose preview picture lives in the slide's VML drawing, and ActiveX controls
//! shown by their pictures.

use std::io::{Cursor, Read, Write};

use deckcraft_model::ShapeKind;
use deckcraft_model::defaults;

const FRAME: &str = r#"<p:graphicFrame><p:nvGraphicFramePr><p:cNvPr id="90" name="Object 3"/><p:cNvGraphicFramePr/><p:nvPr/></p:nvGraphicFramePr><p:xfrm><a:off x="0" y="0"/><a:ext cx="914400" cy="914400"/></p:xfrm><a:graphic><a:graphicData uri="http://schemas.openxmlformats.org/presentationml/2006/ole"><p:oleObj spid="_x0000_s2050" name="Slide" r:id="rIdOle" progId="Thing.1"><p:embed/></p:oleObj></a:graphicData></a:graphic></p:graphicFrame>"#;

const VML: &str = r#"<xml xmlns:v="urn:schemas-microsoft-com:vml" xmlns:o="urn:schemas-microsoft-com:office:office"><v:shape id="_x0000_s2049"><v:imagedata o:relid="rIdA"/></v:shape><v:shape id="_x0000_s2050" style='position:absolute;left:0;top:0;width:1in;height:1in'><v:imagedata o:relid="rIdB" o:title=""/></v:shape></xml>"#;

const VML_RELS: &str = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rIdA" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/image" Target="../media/other.png"/><Relationship Id="rIdB" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/image" Target="../media/preview.png"/></Relationships>"#;

const SLIDE_RELS: &str = r#"<Relationship Id="rIdVml" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/vmlDrawing" Target="../drawings/vmlDrawing1.vml"/><Relationship Id="rIdPic" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/image" Target="../media/preview.png"/>"#;

/// An ActiveX control as PowerPoint writes it: a VML-only choice, and a fallback with its picture.
const CONTROLS: &str = r#"<p:controls><mc:AlternateContent xmlns:mc="http://schemas.openxmlformats.org/markup-compatibility/2006"><mc:Choice xmlns:v="urn:schemas-microsoft-com:vml" Requires="v"><p:control spid="1137" name="CheckBox1" r:id="rIdX" imgW="1933560" imgH="514440"/></mc:Choice><mc:Fallback><p:control name="CheckBox1" r:id="rIdX" imgW="1933560" imgH="514440"><p:pic><p:nvPicPr><p:cNvPr id="4" name="CheckBox1"/><p:cNvPicPr/><p:nvPr/></p:nvPicPr><p:blipFill><a:blip r:embed="rIdPic"/><a:stretch><a:fillRect/></a:stretch></p:blipFill><p:spPr><a:xfrm><a:off x="12700" y="25400"/><a:ext cx="127000" cy="63500"/></a:xfrm><a:prstGeom prst="rect"><a:avLst/></a:prstGeom></p:spPr></p:pic></p:control></mc:Fallback></mc:AlternateContent><p:control name="NoPicture" r:id="rIdY"/></p:controls>"#;

fn png(v: u8) -> Vec<u8> {
    let mut out = Cursor::new(vec![]);
    image::DynamicImage::ImageRgb8(image::RgbImage::from_pixel(2, 2, image::Rgb([v, v, v])))
        .write_to(&mut out, image::ImageFormat::Png)
        .expect("png");
    out.into_inner()
}

/// A new deck whose first slide holds [`FRAME`], with the VML drawing, its rels and two pictures.
fn deck() -> Vec<u8> {
    deck_with(FRAME)
}

/// [`deck`] with `frames` in place of [`FRAME`].
fn deck_with(frames: &str) -> Vec<u8> {
    let bytes = deckcraft_pptx::export(&defaults::new_presentation(None)).expect("export");
    let mut z = zip::ZipArchive::new(Cursor::new(bytes)).expect("zip");
    let mut out = zip::ZipWriter::new(Cursor::new(Vec::new()));
    let o = zip::write::SimpleFileOptions::default();
    for i in 0..z.len() {
        let mut f = z.by_index(i).expect("entry");
        let mut d = vec![];
        f.read_to_end(&mut d).expect("read");
        let name = f.name().to_string();
        if name == "ppt/slides/slide1.xml" {
            let s = String::from_utf8(d).expect("utf8");
            let at = s.rfind("</p:spTree>").expect("spTree");
            let s = format!("{}{frames}{}", &s[..at], &s[at..]);
            let at = s.rfind("</p:cSld>").expect("cSld");
            d = format!("{}{CONTROLS}{}", &s[..at], &s[at..]).into_bytes();
        } else if name == "ppt/slides/_rels/slide1.xml.rels" {
            let s = String::from_utf8(d).expect("utf8");
            d = s.replace("</Relationships>", &format!("{SLIDE_RELS}</Relationships>")).into_bytes();
        }
        out.start_file(name, o).expect("start");
        out.write_all(&d).expect("write");
    }
    for (name, data) in [
        ("ppt/drawings/vmlDrawing1.vml", VML.as_bytes().to_vec()),
        ("ppt/drawings/_rels/vmlDrawing1.vml.rels", VML_RELS.as_bytes().to_vec()),
        ("ppt/media/other.png", png(10)),
        ("ppt/media/preview.png", png(200)),
    ] {
        out.start_file(name, o).expect("start");
        out.write_all(&data).expect("write");
    }
    out.finish().expect("finish").into_inner()
}

#[test]
fn an_ole_object_without_a_picture_shows_its_vml_preview() {
    let p = deckcraft_pptx::import(&deck()).expect("import");
    let s = p.slides[0].shapes.iter().find(|s| s.name == "Object 3").expect("frame");
    let ShapeKind::Opaque { preview: Some(m), label, .. } = &s.kind else { panic!("{:?}", s.kind) };
    assert_eq!(label, "Thing.1");
    let media = p.media.iter().find(|x| x.id == *m).expect("media");
    // The shape whose id is the object's spid, not the first one in the drawing.
    assert_eq!(media.name, "preview.png");
}

#[test]
fn activex_controls_show_their_pictures() {
    let p = deckcraft_pptx::import(&deck()).expect("import");
    let s = p.slides[0].shapes.iter().find(|s| s.name == "CheckBox1").expect("control picture");
    let ShapeKind::Picture { fill } = &s.kind else { panic!("{:?}", s.kind) };
    assert_eq!(p.media.iter().find(|m| m.id == fill.media).map(|m| m.name.as_str()), Some("preview.png"));
    let x = s.xfrm.expect("xfrm");
    assert_eq!((x.x, x.y, x.w, x.h), (1.0, 2.0, 10.0, 5.0));
    assert!(!p.slides[0].shapes.iter().any(|s| s.name == "NoPicture"), "a control without a picture draws nothing");
}

#[test]
fn many_objects_sharing_one_vml_drawing_each_get_their_preview() {
    // 200 frames look up the same drawing (it is parsed once); every other one names a shape id
    // the drawing doesn't have.
    let frames: String = (0..200)
        .map(|i| {
            let spid = match i % 4 {
                0 => "_x0000_s2050",
                1 => "_x0000_s9999",
                2 => "_x0000_s2049",
                _ => "missing",
            };
            FRAME.replace("id=\"90\" name=\"Object 3\"", &format!("id=\"{}\" name=\"Obj {i}\"", 1000 + i)).replace("_x0000_s2050", spid)
        })
        .collect();
    let p = deckcraft_pptx::import(&deck_with(&frames)).expect("import");
    for i in 0..200 {
        let s = p.slides[0].shapes.iter().find(|s| s.name == format!("Obj {i}")).expect("frame");
        let ShapeKind::Opaque { preview, .. } = &s.kind else { panic!("{:?}", s.kind) };
        let name = preview.and_then(|m| p.media.iter().find(|x| x.id == m)).map(|m| m.name.as_str());
        let want = match i % 4 {
            0 => Some("preview.png"),
            2 => Some("other.png"),
            _ => None,
        };
        assert_eq!(name, want, "frame {i}");
    }
}
