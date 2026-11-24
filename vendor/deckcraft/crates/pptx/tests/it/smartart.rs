//! SmartArt drawings: the text of each pre-drawn shape is laid out on its `dsp:txXfrm`.
//!
//! The deck is built here: an export of a new presentation with a SmartArt frame, data part and
//! drawing part written by the test.

use std::io::{Cursor, Read, Write};

use deckcraft_engine::render::place_slide;
use deckcraft_geom::{Point, Rect};
use deckcraft_model::{Presentation, ShapeKind, defaults};

const EMU: f64 = 12_700.0;

/// Frame of the diagram on the slide, in points.
const FRAME: (f64, f64, f64, f64) = (72.0, 72.0, 432.0, 288.0);

fn emu(pt: f64) -> i64 {
    (pt * EMU).round() as i64
}

fn xfrm(tag: &str, rot: f64, x: f64, y: f64, w: f64, h: f64) -> String {
    let rot = if rot == 0.0 { String::new() } else { format!(" rot=\"{}\"", (rot * 60_000.0).round() as i64) };
    format!(r#"<{tag}{rot}><a:off x="{}" y="{}"/><a:ext cx="{}" cy="{}"/></{tag}>"#, emu(x), emu(y), emu(w), emu(h))
}

/// One `dsp:sp`: shape box, text rectangle (both in diagram points) and text.
fn drawing_shape(prst: &str, shape: (f64, f64, f64, f64, f64), text_rect: (f64, f64, f64, f64, f64), text: &str) -> String {
    let (r, x, y, w, h) = shape;
    let (tr, tx, ty, tw, th) = text_rect;
    format!(
        r#"<dsp:sp modelId="{{00000000-0000-0000-0000-000000000000}}"><dsp:nvSpPr><dsp:cNvPr id="0" name=""/><dsp:cNvSpPr/></dsp:nvSpPr><dsp:spPr>{}<a:prstGeom prst="{prst}"><a:avLst/></a:prstGeom><a:solidFill><a:srgbClr val="DDEEFF"/></a:solidFill></dsp:spPr><dsp:style><a:lnRef idx="1"><a:schemeClr val="accent1"/></a:lnRef><a:fillRef idx="1"><a:schemeClr val="accent1"/></a:fillRef><a:effectRef idx="0"><a:schemeClr val="accent1"/></a:effectRef><a:fontRef idx="minor"><a:schemeClr val="dk1"/></a:fontRef></dsp:style><dsp:txBody><a:bodyPr wrap="square" lIns="0" tIns="0" rIns="0" bIns="0" anchor="ctr"><a:noAutofit/></a:bodyPr><a:lstStyle/><a:p><a:r><a:rPr lang="en-US" sz="1000"/><a:t>{text}</a:t></a:r></a:p></dsp:txBody>{}</dsp:sp>"#,
        xfrm("a:xfrm", r, x, y, w, h),
        xfrm("dsp:txXfrm", tr, tx, ty, tw, th),
    )
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

fn zip_of(files: &[(String, Vec<u8>)]) -> Vec<u8> {
    let mut z = zip::ZipWriter::new(Cursor::new(Vec::new()));
    let o = zip::write::SimpleFileOptions::default();
    for (n, d) in files {
        z.start_file(n.as_str(), o).expect("start");
        z.write_all(d).expect("write");
    }
    z.finish().expect("finish").into_inner()
}

fn replace(files: &mut [(String, Vec<u8>)], name: &str, from: &str, to: &str) {
    let (_, d) = files.iter_mut().find(|(n, _)| n == name).unwrap_or_else(|| panic!("{name} missing"));
    let s = String::from_utf8(d.clone()).expect("utf8");
    assert!(s.contains(from), "{name} has no {from}");
    *d = s.replacen(from, to, 1).into_bytes();
}

/// A deck whose first slide holds a SmartArt frame drawn by `shapes`.
fn smartart_deck(shapes: &[String]) -> Vec<u8> {
    let mut files = entries(&deckcraft_pptx::export(&defaults::new_presentation(None)).expect("export"));
    let (fx, fy, fw, fh) = FRAME;
    let frame = format!(
        r#"<p:graphicFrame><p:nvGraphicFramePr><p:cNvPr id="90" name="Diagram 1"/><p:cNvGraphicFramePr/><p:nvPr/></p:nvGraphicFramePr><p:xfrm><a:off x="{}" y="{}"/><a:ext cx="{}" cy="{}"/></p:xfrm><a:graphic><a:graphicData uri="http://schemas.openxmlformats.org/drawingml/2006/diagram"><dgm:relIds xmlns:dgm="http://schemas.openxmlformats.org/drawingml/2006/diagram" r:dm="rIdDm" r:lo="rIdLo" r:qs="rIdQs" r:cs="rIdCs"/></a:graphicData></a:graphic></p:graphicFrame></p:spTree>"#,
        emu(fx),
        emu(fy),
        emu(fw),
        emu(fh)
    );
    replace(&mut files, "ppt/slides/slide1.xml", "</p:spTree>", &frame);
    replace(
        &mut files,
        "ppt/slides/_rels/slide1.xml.rels",
        "</Relationships>",
        r#"<Relationship Id="rIdDm" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/diagramData" Target="../diagrams/data1.xml"/><Relationship Id="rIdDr" Type="http://schemas.microsoft.com/office/2007/relationships/diagramDrawing" Target="../diagrams/drawing1.xml"/></Relationships>"#,
    );
    let data = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<dgm:dataModel xmlns:dgm="http://schemas.openxmlformats.org/drawingml/2006/diagram" xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main"><dgm:ptLst/><dgm:extLst><a:ext uri="http://schemas.microsoft.com/office/drawing/2008/diagram"><dsp:dataModelExt xmlns:dsp="http://schemas.microsoft.com/office/drawing/2008/diagram" relId="rIdDr" minVer="http://schemas.openxmlformats.org/drawingml/2006/diagram"/></a:ext></dgm:extLst></dgm:dataModel>"#;
    let drawing = format!(
        r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<dsp:drawing xmlns:dgm="http://schemas.openxmlformats.org/drawingml/2006/diagram" xmlns:dsp="http://schemas.microsoft.com/office/drawing/2008/diagram" xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main"><dsp:spTree><dsp:nvGrpSpPr><dsp:cNvPr id="0" name=""/><dsp:cNvGrpSpPr/></dsp:nvGrpSpPr><dsp:grpSpPr/>{}</dsp:spTree></dsp:drawing>"#,
        shapes.concat()
    );
    files.push(("ppt/diagrams/data1.xml".into(), data.as_bytes().to_vec()));
    files.push(("ppt/diagrams/drawing1.xml".into(), drawing.into_bytes()));
    zip_of(&files)
}

/// Each text of slide 1 with the slide-space box it is laid out in (insets applied), the number of
/// lines, and the direction of its baseline.
fn text_boxes(p: &Presentation) -> Vec<(String, Rect, usize, Point)> {
    place_slide(p, 0)
        .texts
        .iter()
        .map(|t| {
            let m = t.transform;
            let r = t.layout.inner;
            let corners = [Point::new(r.x0, r.y0), Point::new(r.x1, r.y0), Point::new(r.x1, r.y1), Point::new(r.x0, r.y1)].map(|c| m * c);
            let mut bounds = Rect::from_points(corners[0], corners[2]);
            bounds = bounds.union_pt(corners[1]).union_pt(corners[3]);
            let dir = (m * Point::new(1.0, 0.0)) - (m * Point::new(0.0, 0.0));
            let dir = Point::new(dir.x / dir.hypot(), dir.y / dir.hypot());
            (t.body.text(), bounds, t.layout.lines.len(), dir)
        })
        .collect()
}

fn near(a: Rect, b: Rect) -> bool {
    [a.x0 - b.x0, a.y0 - b.y0, a.x1 - b.x1, a.y1 - b.y1].iter().all(|d| d.abs() < 0.05)
}

/// Diagram rectangle (x, y, w, h) → slide rectangle.
fn on_slide(x: f64, y: f64, w: f64, h: f64) -> Rect {
    Rect::new(FRAME.0 + x, FRAME.1 + y, FRAME.0 + x + w, FRAME.1 + y + h)
}

fn check(p: &Presentation, expected: &[(&str, Rect)]) {
    let boxes = text_boxes(p);
    for (text, want) in expected {
        let (_, got, lines, dir) = boxes.iter().find(|(t, ..)| t == text).unwrap_or_else(|| panic!("no text {text:?} in {boxes:?}"));
        assert!(near(*got, *want), "{text:?}: laid out in {got:?}, want {want:?}");
        assert!((dir.x - 1.0).abs() < 1e-6 && dir.y.abs() < 1e-6, "{text:?}: baseline turned to {dir:?}");
        assert_eq!(*lines, 1, "{text:?}: wrapped into {lines} lines");
    }
}

#[test]
fn smartart_text_follows_tx_xfrm() {
    let deck = smartart_deck(&[
        // A narrow shape whose text rectangle is three times wider.
        drawing_shape("rect", (0.0, 0.0, 0.0, 72.0, 144.0), (0.0, 0.0, 36.0, 216.0, 72.0), "wider than its shape"),
        // A tall shape turned by 90° (wide on the slide), its text rectangle turned back: horizontal text.
        drawing_shape("roundRect", (90.0, 300.0, -60.0, 48.0, 216.0), (-90.0, 220.0, 28.0, 208.0, 40.0), "turned shape, upright text"),
        // A text rectangle inside the shape: the shape keeps its text, with wider insets.
        drawing_shape("roundRect", (0.0, 0.0, 192.0, 216.0, 72.0), (0.0, 24.0, 204.0, 168.0, 48.0), "inside its shape"),
    ]);
    let expected = [
        ("wider than its shape", on_slide(0.0, 36.0, 216.0, 72.0)),
        ("turned shape, upright text", on_slide(220.0, 28.0, 208.0, 40.0)),
        ("inside its shape", on_slide(24.0, 204.0, 168.0, 48.0)),
    ];

    let p = deckcraft_pptx::import(&deck).expect("import");
    assert!(p.validate().is_empty(), "{:?}", p.validate());
    check(&p, &expected);
    // The diagram is one group; text that left its shape is in a frameless shape of the group.
    let group = p.slides[0].shapes.iter().find(|s| s.name == "Diagram 1").expect("diagram");
    let ShapeKind::Group { children, .. } = &group.kind else { panic!("diagram is not a group") };
    assert_eq!(children.len(), 5);
    let inside = children.iter().find(|s| s.text.as_ref().is_some_and(|t| t.text() == "inside its shape")).expect("inside");
    assert!(matches!(inside.geom, deckcraft_model::Geom::Preset { ref name, .. } if name == "roundRect"));
    for moved in children.iter().filter(|s| s.text.as_ref().is_some_and(|t| t.text() != "inside its shape")) {
        assert_eq!(moved.fill, Some(deckcraft_model::style::Fill::None));
        assert_eq!(moved.line.as_ref().and_then(|l| l.fill.clone()), Some(deckcraft_model::style::Fill::None));
    }

    // The exported deck lays the text out in the same places.
    let q = deckcraft_pptx::import(&deckcraft_pptx::export(&p).expect("export")).expect("re-import");
    check(&q, &expected);
}
