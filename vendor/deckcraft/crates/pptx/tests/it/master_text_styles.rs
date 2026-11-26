//! Imported empty master styles must not become the defaults used to author new decks.
use std::io::{Cursor, Read, Write};
use std::sync::Arc;

use deckcraft_geom::Xfrm;
use deckcraft_model::resolve::{self, Ctx};
use deckcraft_model::text::{Align, Bullet, ParaProps, TextBody};
use deckcraft_model::{PhType, Placeholder, Presentation, Shape, ShapeId, defaults};

fn subtitle(text: &str) -> Shape {
    let mut body = TextBody::from_text(text);
    body.paragraphs[0].props.align = Some(Align::Center);
    Shape {
        id: ShapeId(102),
        name: "Centered subtitle".into(),
        ph: Some(Placeholder { kind: PhType::SubTitle, idx: 1, ..Default::default() }),
        xfrm: Some(Xfrm::new(120.0, 120.0, 720.0, 90.0)),
        text: Some(body),
        ..Default::default()
    }
}

/// Author an input with empty title/other styles and the specified body style, without asking
/// the serializer under test to supply its own master text styles in the input.
fn imported(body_style: &str) -> Presentation {
    let mut p = defaults::new_presentation(None);
    let m = Arc::make_mut(&mut p.masters[0]);
    m.shapes.clear();
    for l in &mut m.layouts {
        l.shapes = vec![subtitle("")];
    }
    Arc::make_mut(&mut p.slides[0]).shapes = vec![subtitle("Centered subtitle")];
    let bytes = deckcraft_pptx::export(&p).expect("base package");
    let mut z = zip::ZipArchive::new(Cursor::new(bytes)).expect("zip");
    let mut out = zip::ZipWriter::new(Cursor::new(Vec::new()));
    for i in 0..z.len() {
        let mut part = z.by_index(i).expect("part");
        let name = part.name().to_string();
        let mut data = Vec::new();
        part.read_to_end(&mut data).expect("read");
        if name.starts_with("ppt/slideMasters/slideMaster") && name.ends_with(".xml") {
            let mut xml = String::from_utf8(data).expect("generated XML");
            let start = xml.find("<p:txStyles>").expect("styles start");
            let end = xml.find("</p:txStyles>").expect("styles end") + "</p:txStyles>".len();
            xml.replace_range(start..end, &format!("<p:txStyles><p:titleStyle/><p:bodyStyle>{body_style}</p:bodyStyle><p:otherStyle/></p:txStyles>"));
            data = xml.into_bytes();
        }
        out.start_file(name, zip::write::SimpleFileOptions::default()).expect("start");
        out.write_all(&data).expect("write");
    }
    deckcraft_pptx::import(&out.finish().expect("finish").into_inner()).expect("import fixture")
}

fn resolved_subtitle(p: &Presentation) -> ParaProps {
    let slide = &p.slides[0];
    let shape = slide.shapes.iter().find(|s| s.name == "Centered subtitle").expect("subtitle");
    assert_eq!(shape.ph_type(), Some(PhType::SubTitle));
    assert_eq!(shape.xfrm, Some(Xfrm::new(120.0, 120.0, 720.0, 90.0)));
    let text = shape.text.as_ref().expect("text");
    assert_eq!(text.text(), "Centered subtitle");
    resolve::para(&Ctx::for_slide(p, slide).expect("context"), shape, &text.paragraphs[0])
}

fn saved(p: &Presentation) -> Presentation {
    deckcraft_pptx::import(&deckcraft_pptx::export(p).expect("export")).expect("reopen")
}

#[test]
fn an_imported_empty_master_keeps_its_unbulleted_subtitle_after_two_saves() {
    let mut p = imported("");
    let before = resolved_subtitle(&p);
    assert_eq!(before.bullet, None);
    assert_eq!(before.align, Some(Align::Center));
    for _ in 0..2 {
        p = saved(&p);
        assert_eq!(resolved_subtitle(&p).bullet, None, "saving must not add a subtitle bullet");
        let master = &p.masters[0];
        assert!(master.title_style.is_empty(), "do not invent an imported title style");
        assert!(master.body_style.is_empty(), "do not invent bullets in an imported body style");
        assert!(master.other_style.is_empty(), "do not invent an imported other-text style");
        assert_eq!(resolved_subtitle(&p), before);
    }
}

#[test]
fn an_explicit_unbulleted_body_style_keeps_its_properties() {
    let mut p = imported(r#"<a:lvl1pPr><a:buNone/><a:defRPr sz="1900"><a:latin typeface="Carlito"/></a:defRPr></a:lvl1pPr>"#);
    let expected = p.masters[0].body_style.clone();
    assert_eq!(resolved_subtitle(&p).bullet, Some(Bullet::None));
    for _ in 0..2 {
        p = saved(&p);
        assert_eq!(p.masters[0].body_style, expected);
        assert_eq!(resolved_subtitle(&p).bullet, Some(Bullet::None));
    }
}

#[test]
fn newly_authored_presentations_keep_their_populated_master_defaults() {
    let mut p = defaults::new_presentation(None);
    let m = &p.masters[0];
    let expected = (m.title_style.clone(), m.body_style.clone(), m.other_style.clone());
    assert!(!expected.0.is_empty() && !expected.1.is_empty() && !expected.2.is_empty());
    assert_eq!(expected.1.levels.len(), 9);
    assert!(matches!(expected.1.level(0).expect("body level").para.bullet, Some(Bullet::Char { .. })));
    for _ in 0..2 {
        p = saved(&p);
        let m = &p.masters[0];
        assert_eq!((&m.title_style, &m.body_style, &m.other_style), (&expected.0, &expected.1, &expected.2));
    }
}
