//! Lock and hidden state (Selection pane): what PowerPoint writes for Lock, and what it reads as locked.

use std::io::{Cursor, Read, Write};

use deckcraft_geom::Xfrm;
use deckcraft_model::style::PictureFill;
use deckcraft_model::table::Table;
use deckcraft_model::{Presentation, Shape, ShapeId, ShapeKind, Slide, defaults};

fn png() -> Vec<u8> {
    let img = image::RgbImage::from_pixel(4, 4, image::Rgb([10, 200, 30]));
    let mut out = Cursor::new(vec![]);
    image::DynamicImage::ImageRgb8(img).write_to(&mut out, image::ImageFormat::Png).expect("png");
    out.into_inner()
}

fn shape(p: &mut Presentation, name: &str, x: f64) -> Shape {
    Shape { id: ShapeId(p.alloc_id().expect("ids")), name: name.into(), xfrm: Some(Xfrm::new(x, 40.0, 80.0, 60.0)), ..Default::default() }
}

/// Slide 1 of a blank deck holds: a locked shape, an unlocked one, a hidden locked text box, a
/// locked group (one child unlocked, one hidden), and a locked picture, connector and table.
fn deck() -> Presentation {
    let mut p = defaults::new_presentation(None);
    let img = p.add_media("image1.png", "image/png", png()).expect("media");
    let mut v = vec![];
    let mut a = shape(&mut p, "Locked", 0.0);
    a.locked = true;
    v.push(a);
    v.push(shape(&mut p, "Free", 100.0));
    let mut t = shape(&mut p, "Hidden text", 200.0);
    t.text_box = true;
    t.hidden = true;
    t.locked = true;
    v.push(t);
    let mut c1 = shape(&mut p, "Child locked", 300.0);
    c1.locked = true;
    let c2 = shape(&mut p, "Child free", 400.0);
    let mut c3 = shape(&mut p, "Child hidden", 500.0);
    c3.hidden = true;
    let gx = Xfrm::new(300.0, 40.0, 280.0, 60.0);
    let mut g = Shape {
        id: ShapeId(p.alloc_id().expect("ids")),
        name: "Group".into(),
        xfrm: Some(gx),
        kind: ShapeKind::Group { children: vec![c1, c2, c3], child: gx },
        locked: true,
        ..Default::default()
    };
    g.hidden = false;
    v.push(g);
    let mut pic = shape(&mut p, "Pic", 0.0);
    pic.kind = ShapeKind::Picture { fill: PictureFill { media: img, ..Default::default() } };
    pic.locked = true;
    v.push(pic);
    let mut cx = shape(&mut p, "Conn", 100.0);
    cx.kind = ShapeKind::Connector { start: None, end: None };
    cx.geom = deckcraft_model::Geom::preset("straightConnector1");
    cx.locked = true;
    v.push(cx);
    let mut tb = shape(&mut p, "Tbl", 200.0);
    tb.kind = ShapeKind::Table(Table::new(2, 2, 80.0, 30.0));
    tb.locked = true;
    v.push(tb);
    if let Some(s) = p.slides.first_mut() {
        std::sync::Arc::make_mut(s).shapes = v;
    }
    p
}

fn find<'a>(s: &'a Slide, name: &str) -> &'a Shape {
    let mut found = None;
    deckcraft_model::walk(&s.shapes, &mut |x, _| {
        if found.is_none() && x.name == name {
            found = Some(x);
        }
    });
    found.unwrap_or_else(|| panic!("no shape {name}"))
}

fn entries(bytes: &[u8]) -> Vec<(String, Vec<u8>)> {
    let mut z = zip::ZipArchive::new(Cursor::new(bytes)).expect("zip");
    (0..z.len())
        .map(|i| {
            let mut f = z.by_index(i).expect("entry");
            let mut d = vec![];
            f.read_to_end(&mut d).expect("read");
            (f.name().to_string(), d)
        })
        .collect()
}

fn rezip(es: &[(String, Vec<u8>)]) -> Vec<u8> {
    let mut z = zip::ZipWriter::new(Cursor::new(Vec::new()));
    let o = zip::write::SimpleFileOptions::default();
    for (n, d) in es {
        z.start_file(n.as_str(), o).expect("start");
        z.write_all(d).expect("write");
    }
    z.finish().expect("finish").into_inner()
}

fn slide1_xml(bytes: &[u8]) -> String {
    let es = entries(bytes);
    let (_, d) = es.iter().find(|(n, _)| n == "ppt/slides/slide1.xml").expect("slide1");
    String::from_utf8(d.clone()).expect("utf8")
}

/// The element following `name="…"` up to the end of the object's non-visual properties.
fn nv_of<'a>(xml: &'a str, name: &str) -> &'a str {
    let i = xml.find(&format!("name=\"{name}\"")).unwrap_or_else(|| panic!("no {name}"));
    let rest = &xml[i..];
    let j = rest.find("<p:nvPr").unwrap_or(rest.len());
    &rest[..j]
}

#[test]
fn lock_and_hidden_round_trip() {
    let p = deck();
    let q = deckcraft_pptx::import(&deckcraft_pptx::export(&p).expect("export")).expect("import");
    let s = &q.slides[0];
    for (name, locked, hidden) in [
        ("Locked", true, false),
        ("Free", false, false),
        ("Hidden text", true, true),
        ("Group", true, false),
        ("Child locked", true, false),
        ("Child free", false, false),
        ("Child hidden", false, true),
        ("Pic", true, false),
        ("Conn", true, false),
        ("Tbl", true, false),
    ] {
        let sh = find(s, name);
        assert_eq!((sh.locked, sh.hidden), (locked, hidden), "{name}");
    }
    // The stacking order of the slide and of the group survives too.
    let names: Vec<&str> = s.shapes.iter().map(|x| x.name.as_str()).collect();
    assert_eq!(names, ["Locked", "Free", "Hidden text", "Group", "Pic", "Conn", "Tbl"]);
    let kids: Vec<&str> = find(s, "Group").children().iter().map(|x| x.name.as_str()).collect();
    assert_eq!(kids, ["Child locked", "Child free", "Child hidden"]);
}

#[test]
fn lock_writes_powerpoints_full_attribute_set() {
    let xml = slide1_xml(&deckcraft_pptx::export(&deck()).expect("export"));
    let sp = r#"noGrp="1" noRot="1" noMove="1" noResize="1" noEditPoints="1" noAdjustHandles="1" noChangeArrowheads="1" noChangeShapeType="1""#;
    assert!(nv_of(&xml, "Locked").contains(&format!("<a:spLocks {sp}/>")), "{}", nv_of(&xml, "Locked"));
    assert!(nv_of(&xml, "Hidden text").contains(&format!("<a:spLocks {sp}/>")));
    assert!(nv_of(&xml, "Hidden text").contains(r#"hidden="1""#));
    assert!(!nv_of(&xml, "Free").contains("Locks"), "an unlocked shape writes no locks: {}", nv_of(&xml, "Free"));
    assert!(nv_of(&xml, "Group").contains(r#"<a:grpSpLocks noGrp="1" noUngrp="1" noRot="1" noMove="1" noResize="1"/>"#));
    assert!(nv_of(&xml, "Conn").contains(&format!("<a:cxnSpLocks {sp}/>")), "{}", nv_of(&xml, "Conn"));
    assert!(nv_of(&xml, "Pic").contains(&format!(r#"<a:picLocks noChangeAspect="1" {sp} noCrop="1"/>"#)), "{}", nv_of(&xml, "Pic"));
    assert!(nv_of(&xml, "Tbl").contains(r#"<a:graphicFrameLocks noGrp="1" noDrilldown="1" noMove="1" noResize="1"/>"#));
}

/// PowerPoint reads Lock only from the whole set: a lone `noMove`/`noResize` (another tool's
/// partial lock), or the set without `noGrp`, isn't locked.
#[test]
fn partial_locks_are_not_lock() {
    let mut p = defaults::new_presentation(None);
    let a = shape(&mut p, "A", 0.0);
    let b = shape(&mut p, "B", 100.0);
    let c = shape(&mut p, "C", 200.0);
    let mut d = shape(&mut p, "D", 300.0);
    d.locked = true;
    if let Some(s) = p.slides.first_mut() {
        std::sync::Arc::make_mut(s).shapes = vec![a, b, c, d];
    }
    let mut es = entries(&deckcraft_pptx::export(&p).expect("export"));
    let slide = es.iter_mut().find(|(n, _)| n == "ppt/slides/slide1.xml").expect("slide1");
    let xml = String::from_utf8(slide.1.clone()).expect("utf8");
    let swap = |xml: String, name: &str, locks: &str| {
        let i = xml.find(&format!("name=\"{name}\"")).expect("name");
        let j = i + xml[i..].find("<p:cNvSpPr").expect("cNvSpPr");
        let k = j + xml[j..].find("<p:nvPr").expect("nvPr");
        format!("{}<p:cNvSpPr>{locks}</p:cNvSpPr>{}", &xml[..j], &xml[k..])
    };
    let xml = swap(xml, "A", r#"<a:spLocks noMove="1" noResize="1"/>"#);
    let xml = swap(
        xml,
        "B",
        r#"<a:spLocks noRot="1" noMove="1" noResize="1" noEditPoints="1" noAdjustHandles="1" noChangeArrowheads="1" noChangeShapeType="1"/>"#,
    );
    let xml =
        swap(xml, "C", r#"<a:spLocks noGrp="1" noRot="1" noMove="1" noResize="1" noEditPoints="1" noAdjustHandles="1" noChangeShapeType="1"/>"#);
    slide.1 = xml.into_bytes();
    let q = deckcraft_pptx::import(&rezip(&es)).expect("import");
    let s = &q.slides[0];
    assert!(!find(s, "A").locked);
    assert!(!find(s, "B").locked);
    assert!(!find(s, "C").locked);
    assert!(find(s, "D").locked);
}

/// Unlocked objects written by PowerPoint keep an empty locks element; that's not locked.
#[test]
fn empty_locks_element_is_unlocked() {
    let mut p = defaults::new_presentation(None);
    let a = shape(&mut p, "A", 0.0);
    if let Some(s) = p.slides.first_mut() {
        std::sync::Arc::make_mut(s).shapes = vec![a];
    }
    let mut es = entries(&deckcraft_pptx::export(&p).expect("export"));
    let slide = es.iter_mut().find(|(n, _)| n == "ppt/slides/slide1.xml").expect("slide1");
    let xml = String::from_utf8(slide.1.clone()).expect("utf8").replacen("<p:cNvSpPr/>", "<p:cNvSpPr><a:spLocks/></p:cNvSpPr>", 1);
    slide.1 = xml.into_bytes();
    let q = deckcraft_pptx::import(&rezip(&es)).expect("import");
    assert!(!find(&q.slides[0], "A").locked);
}
