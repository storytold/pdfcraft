//! An unchanged chart keeps the embedded workbook its `c:externalData` points at (#335).

use std::io::{Cursor, Read, Write};
use std::sync::Arc;

use deckcraft_geom::Xfrm;
use deckcraft_model::chart::{Chart, ChartType};
use deckcraft_model::{LayoutType, Shape, ShapeId, ShapeKind, defaults};

const WORKBOOK: &[u8] = b"chart-workbook-bytes-v1";
const EMBED: &str = "ppt/embeddings/Microsoft_Excel_Worksheet1.xlsx";

fn chart_pptx() -> Vec<u8> {
    let mut p = defaults::new_presentation(None);
    p.slides.clear();
    let layout = defaults::layout_of_kind(&p, LayoutType::Blank).unwrap_or_default();
    let mut slide = defaults::new_slide(&mut p, layout).expect("slide");
    let mut shape =
        Shape { id: ShapeId(p.alloc_id().expect("id")), name: "Pie".into(), xfrm: Some(Xfrm::new(40.0, 40.0, 400.0, 300.0)), ..Default::default() };
    shape.kind = ShapeKind::Chart(Box::new(Chart::sample(ChartType::Pie)));
    slide.shapes.push(shape);
    p.slides.push(Arc::new(slide));
    deckcraft_pptx::export(&p).expect("export")
}

/// Add an embedded workbook and point the chart part at it, the way PowerPoint writes a chart.
fn with_workbook(bytes: &[u8]) -> Vec<u8> {
    let mut zip = zip::ZipArchive::new(Cursor::new(bytes)).expect("zip");
    let mut out = zip::ZipWriter::new(Cursor::new(Vec::new()));
    let opts = zip::write::SimpleFileOptions::default();
    let mut chart_part = None;
    for i in 0..zip.len() {
        let mut f = zip.by_index(i).expect("entry");
        let name = f.name().to_string();
        let mut data = vec![];
        f.read_to_end(&mut data).expect("read");
        if name.starts_with("ppt/charts/chart") && name.ends_with(".xml") && !name.contains("_rels") {
            let xml = String::from_utf8(data).expect("chart xml");
            assert!(xml.contains("</c:chartSpace>"), "{name}");
            data = xml
                .replacen("</c:chartSpace>", r#"<c:externalData r:id="rId1"><c:autoUpdate val="0"/></c:externalData></c:chartSpace>"#, 1)
                .into_bytes();
            chart_part = Some(name.clone());
        } else if name == "[Content_Types].xml" {
            let xml = String::from_utf8(data).expect("content types");
            data = xml
                .replacen(
                    "</Types>",
                    r#"<Override PartName="/ppt/embeddings/Microsoft_Excel_Worksheet1.xlsx" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.sheet"/></Types>"#,
                    1,
                )
                .into_bytes();
        }
        out.start_file(&name, opts).expect("start");
        out.write_all(&data).expect("write");
    }
    let chart_part = chart_part.expect("chart part");
    let rels = format!("ppt/charts/_rels/{}.rels", chart_part.rsplit('/').next().unwrap_or("chart1.xml"));
    let rels_xml = br#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/package" Target="../embeddings/Microsoft_Excel_Worksheet1.xlsx"/><Relationship Id="rIdBad" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/slide" Target="../slides/slide1.xml"/></Relationships>"#;
    out.start_file(rels, opts).expect("rels");
    out.write_all(rels_xml).expect("rels bytes");
    out.start_file(EMBED, opts).expect("embed");
    out.write_all(WORKBOOK).expect("workbook");
    out.finish().expect("finish").into_inner()
}

fn parts(bytes: &[u8]) -> Vec<(String, Vec<u8>)> {
    let mut zip = zip::ZipArchive::new(Cursor::new(bytes)).expect("zip");
    let mut out = vec![];
    for i in 0..zip.len() {
        let mut f = zip.by_index(i).expect("entry");
        let name = f.name().to_string();
        let mut data = vec![];
        f.read_to_end(&mut data).expect("read");
        out.push((name, data));
    }
    out
}

fn chart_of(bytes: &[u8]) -> deckcraft_model::Chart {
    let p = deckcraft_pptx::import(bytes).expect("import");
    p.slides
        .iter()
        .flat_map(|s| s.shapes.iter())
        .find_map(|s| match &s.kind {
            ShapeKind::Chart(c) if s.name == "Pie" => Some((**c).clone()),
            _ => None,
        })
        .expect("pie chart")
}

#[test]
fn an_unchanged_chart_keeps_its_embedded_workbook() {
    let src = with_workbook(&chart_pptx());
    let chart = chart_of(&src);
    assert_eq!(chart.kept.parts.iter().filter(|p| p.name == EMBED).count(), 1);
    assert_eq!(chart.kept.parts.iter().find(|p| p.name == EMBED).map(|p| p.bytes.as_slice()), Some(WORKBOOK));

    let saved = deckcraft_pptx::export(&deckcraft_pptx::import(&src).expect("import")).expect("export");
    let saved_parts = parts(&saved);
    // Kept parts are written under fresh names, so they can't land on a part the writer makes.
    let workbook = saved_parts.iter().find(|(n, _)| n == "ppt/embeddings/kept1.xlsx").map(|(_, d)| d.as_slice());
    assert_eq!(workbook, Some(WORKBOOK), "the embedding part is dropped on save");
    let rels = saved_parts
        .iter()
        .find(|(n, _)| n.starts_with("ppt/charts/_rels/chart") && n.ends_with(".xml.rels"))
        .map(|(_, d)| String::from_utf8_lossy(d).into_owned())
        .expect("chart relationships");
    assert!(rels.contains(r#"Id="rId1""#) && rels.contains("../embeddings/kept1.xlsx"), "{rels}");
    assert!(!rels.contains("slide1.xml"), "a chart relationship must not replace the slide: {rels}");
    let slide = saved_parts.iter().find(|(n, _)| n == "ppt/slides/slide1.xml").map(|(_, d)| String::from_utf8_lossy(d).into_owned()).expect("slide");
    assert!(slide.contains("sld"), "{slide}");
    assert!(!slide.as_bytes().windows(WORKBOOK.len()).any(|w| w == WORKBOOK));
    let xml = saved_parts
        .iter()
        .find(|(n, _)| n.starts_with("ppt/charts/chart") && n.ends_with(".xml"))
        .map(|(_, d)| String::from_utf8_lossy(d).into_owned())
        .expect("chart xml");
    assert!(xml.contains("externalData"), "{xml}");
}

#[test]
fn an_edited_chart_does_not_write_a_stale_workbook() {
    let src = with_workbook(&chart_pptx());
    let mut p = deckcraft_pptx::import(&src).expect("import");
    let slide = Arc::make_mut(p.slides.get_mut(0).expect("slide"));
    let chart = slide.shapes.iter_mut().find_map(|s| match &mut s.kind {
        ShapeKind::Chart(c) if s.name == "Pie" => Some(c),
        _ => None,
    });
    let chart = chart.expect("pie");
    let value = chart.series.get_mut(0).and_then(|s| s.values.get_mut(0)).expect("value");
    *value = Some(99.0);
    let saved = parts(&deckcraft_pptx::export(&p).expect("export"));
    assert!(saved.iter().all(|(n, _)| !n.starts_with("ppt/embeddings/")), "a stale workbook was written after the chart changed");
    let xml = saved
        .iter()
        .find(|(n, _)| n.starts_with("ppt/charts/chart") && n.ends_with(".xml"))
        .map(|(_, d)| String::from_utf8_lossy(d).into_owned())
        .expect("chart xml");
    assert!(!xml.contains("externalData"), "{xml}");
}

fn png(rgb: [u8; 3]) -> Vec<u8> {
    let img = image::RgbImage::from_pixel(4, 4, image::Rgb(rgb));
    let mut out = Cursor::new(vec![]);
    image::DynamicImage::ImageRgb8(img).write_to(&mut out, image::ImageFormat::Png).expect("png");
    out.into_inner()
}

/// Rewrite `src`: `edit` may change any entry (name, bytes → new bytes), then `extra` entries are added.
fn rewrite(src: &[u8], edit: impl Fn(&str, Vec<u8>) -> Vec<u8>, extra: &[(&str, &[u8])]) -> Vec<u8> {
    let mut zip = zip::ZipArchive::new(Cursor::new(src)).expect("zip");
    let mut out = zip::ZipWriter::new(Cursor::new(Vec::new()));
    let opts = zip::write::SimpleFileOptions::default();
    for i in 0..zip.len() {
        let mut f = zip.by_index(i).expect("entry");
        let name = f.name().to_string();
        let mut data = vec![];
        f.read_to_end(&mut data).expect("read");
        let data = edit(&name, data);
        out.start_file(&name, opts).expect("start");
        out.write_all(&data).expect("write");
    }
    for (name, data) in extra {
        out.start_file(*name, opts).expect("start");
        out.write_all(data).expect("write");
    }
    out.finish().expect("finish").into_inner()
}

fn unique_names(parts: &[(String, Vec<u8>)]) {
    let mut seen = std::collections::HashSet::new();
    for (n, _) in parts {
        assert!(seen.insert(n.to_ascii_lowercase()), "duplicate zip entry {n}");
    }
}

fn text_of(parts: &[(String, Vec<u8>)], name: &str) -> String {
    parts.iter().find(|(n, _)| n == name).map(|(_, d)| String::from_utf8_lossy(d).into_owned()).unwrap_or_else(|| panic!("no {name}"))
}

const RT: &str = "http://schemas.openxmlformats.org/officeDocument/2006/relationships";

/// A chart whose relationships reach parts named like the ones the writer generates
/// (`ppt/media/image1.png`, `ppt/drawings/…`) must not collide with them on save: no duplicate
/// zip entries, and the slide's own picture keeps its bytes.
#[test]
fn kept_chart_parts_never_collide_with_written_parts() {
    let picture = png([10, 200, 30]);
    let mut p = defaults::new_presentation(None);
    p.slides.clear();
    let img = p.add_media("image1.png", "image/png", picture.clone()).expect("media");
    let layout = defaults::layout_of_kind(&p, LayoutType::Blank).unwrap_or_default();
    let mut slide = defaults::new_slide(&mut p, layout).expect("slide");
    // The chart comes first, so its kept parts are written before the picture.
    let mut chart =
        Shape { id: ShapeId(p.alloc_id().expect("id")), name: "Pie".into(), xfrm: Some(Xfrm::new(40.0, 40.0, 400.0, 300.0)), ..Default::default() };
    chart.kind = ShapeKind::Chart(Box::new(Chart::sample(ChartType::Pie)));
    slide.shapes.push(chart);
    let mut pic =
        Shape { id: ShapeId(p.alloc_id().expect("id")), name: "Pic".into(), xfrm: Some(Xfrm::new(500.0, 40.0, 80.0, 80.0)), ..Default::default() };
    pic.kind = ShapeKind::Picture { fill: deckcraft_model::style::PictureFill { media: img, ..Default::default() } };
    slide.shapes.push(pic);
    p.slides.push(Arc::new(slide));
    let base = deckcraft_pptx::export(&p).expect("export");
    assert!(parts(&base).iter().any(|(n, d)| n == "ppt/media/image1.png" && *d == picture));

    // The chart's own relationships reach the slide picture's part and a drawing whose
    // relationships reach it too (and, hostile, the slide).
    let chart_rels = format!(
        r#"<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="{RT}/package" Target="../embeddings/Microsoft_Excel_Worksheet1.xlsx"/><Relationship Id="rId2" Type="{RT}/chartUserShapes" Target="../drawings/drawing1.xml"/><Relationship Id="rId3" Type="{RT}/image" Target="../media/image1.png"/></Relationships>"#
    );
    let drawing_rels = format!(
        r#"<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="{RT}/image" Target="../media/image1.png"/><Relationship Id="rId2" Type="{RT}/slide" Target="../slides/slide1.xml"/></Relationships>"#
    );
    let drawing = br#"<c:userShapes xmlns:c="http://schemas.openxmlformats.org/drawingml/2006/chart"/>"#;
    let src = rewrite(
        &base,
        |name, data| {
            if name.starts_with("ppt/charts/chart") && name.ends_with(".xml") {
                String::from_utf8(data)
                    .expect("chart")
                    .replacen("</c:chartSpace>", r#"<c:externalData r:id="rId1"/><c:userShapes r:id="rId2"/></c:chartSpace>"#, 1)
                    .into_bytes()
            } else {
                data
            }
        },
        &[
            ("ppt/charts/_rels/chart1.xml.rels", chart_rels.as_bytes()),
            ("ppt/drawings/drawing1.xml", drawing),
            ("ppt/drawings/_rels/drawing1.xml.rels", drawing_rels.as_bytes()),
            (EMBED, WORKBOOK),
        ],
    );

    let saved = parts(&deckcraft_pptx::export(&deckcraft_pptx::import(&src).expect("import")).expect("export"));
    unique_names(&saved);
    let picture_parts: Vec<&String> = saved.iter().filter(|(_, d)| *d == picture).map(|(n, _)| n).collect();
    assert!(picture_parts.iter().any(|n| *n == "ppt/media/image1.png"), "{picture_parts:?}");
    let slide_rels = text_of(&saved, "ppt/slides/_rels/slide1.xml.rels");
    assert!(slide_rels.contains("../media/image1.png"), "{slide_rels}");

    // The chart's relationships point at the renamed parts, which hold the kept bytes.
    let rels = text_of(&saved, "ppt/charts/_rels/chart1.xml.rels");
    assert!(!rels.contains("image1.png") && !rels.contains("drawing1.xml") && !rels.contains("Worksheet1"), "{rels}");
    let target = |rels: &str, id: &str| -> String {
        let at = rels.find(&format!(r#"Id="{id}""#)).unwrap_or_else(|| panic!("{id} in {rels}"));
        let rest = &rels[at..];
        let t = rest.find(r#"Target=""#).expect("target") + 8;
        rest[t..].split('"').next().unwrap_or("").to_string()
    };
    let resolve = |from_dir: &str, t: &str| format!("ppt/{}", t.trim_start_matches("../").trim_start_matches(from_dir));
    let img_part = resolve("", &target(&rels, "rId3"));
    assert!(img_part.starts_with("ppt/media/kept"), "{img_part}");
    assert!(saved.iter().any(|(n, d)| *n == img_part && *d == picture), "{img_part}");
    let drawing_part = resolve("", &target(&rels, "rId2"));
    assert!(saved.iter().any(|(n, d)| *n == drawing_part && d.as_slice() == drawing.as_slice()), "{drawing_part}");
    let wb = resolve("", &target(&rels, "rId1"));
    assert!(saved.iter().any(|(n, d)| *n == wb && d.as_slice() == WORKBOOK), "{wb}");

    // The drawing's own relationships follow the rename; the one to the slide is dropped.
    let file = drawing_part.rsplit('/').next().unwrap_or("");
    let drels = text_of(&saved, &format!("ppt/drawings/_rels/{file}.rels"));
    assert_eq!(resolve("", &target(&drels, "rId1")), img_part, "{drels}");
    assert!(!drels.contains("slide"), "{drels}");

    // And the result reads back with the same kept parts.
    let again = deckcraft_pptx::export(&deckcraft_pptx::import(&saved_zip(&saved)).expect("reimport")).expect("export again");
    unique_names(&parts(&again));
}

fn saved_zip(parts: &[(String, Vec<u8>)]) -> Vec<u8> {
    let mut out = zip::ZipWriter::new(Cursor::new(Vec::new()));
    let opts = zip::write::SimpleFileOptions::default();
    for (n, d) in parts {
        out.start_file(n.as_str(), opts).expect("start");
        out.write_all(d).expect("write");
    }
    out.finish().expect("finish").into_inner()
}

/// An unchanged Office 2016 chart keeps its embedded workbook too; its plain-chart fallback
/// carries none.
#[test]
fn an_unchanged_chartex_keeps_its_embedded_workbook() {
    let mut p = defaults::new_presentation(None);
    p.slides.clear();
    let layout = defaults::layout_of_kind(&p, LayoutType::Blank).unwrap_or_default();
    let mut slide = defaults::new_slide(&mut p, layout).expect("slide");
    let mut shape =
        Shape { id: ShapeId(p.alloc_id().expect("id")), name: "Pie".into(), xfrm: Some(Xfrm::new(40.0, 40.0, 400.0, 300.0)), ..Default::default() };
    shape.kind = ShapeKind::Chart(Box::new(Chart::sample(ChartType::Waterfall)));
    slide.shapes.push(shape);
    p.slides.push(Arc::new(slide));
    let base = deckcraft_pptx::export(&p).expect("export");
    assert!(parts(&base).iter().any(|(n, _)| n == "ppt/charts/chartEx1.xml"));
    let rels = format!(
        r#"<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="{RT}/package" Target="../embeddings/Microsoft_Excel_Worksheet1.xlsx"/></Relationships>"#
    );
    let src = rewrite(
        &base,
        |name, data| {
            if name == "ppt/charts/chartEx1.xml" {
                String::from_utf8(data)
                    .expect("chartex")
                    .replacen("<cx:chartData>", r#"<cx:chartData><cx:externalData r:id="rId1" cx:autoUpdate="0"/>"#, 1)
                    .into_bytes()
            } else {
                data
            }
        },
        &[("ppt/charts/_rels/chartEx1.xml.rels", rels.as_bytes()), (EMBED, WORKBOOK)],
    );
    let chart = chart_of(&src);
    assert_eq!(chart.kind, ChartType::Waterfall);
    assert!(chart.kept.parts.iter().any(|p| p.bytes == WORKBOOK));

    let saved = parts(&deckcraft_pptx::export(&deckcraft_pptx::import(&src).expect("import")).expect("export"));
    unique_names(&saved);
    let xml = text_of(&saved, "ppt/charts/chartEx1.xml");
    assert!(xml.contains("externalData"), "{xml}");
    let rels = text_of(&saved, "ppt/charts/_rels/chartEx1.xml.rels");
    assert!(rels.contains("../embeddings/kept1.xlsx"), "{rels}");
    assert!(saved.iter().any(|(n, d)| n == "ppt/embeddings/kept1.xlsx" && d.as_slice() == WORKBOOK));
    assert!(saved.iter().all(|(n, _)| n != "ppt/charts/_rels/chart1.xml.rels"), "the fallback chart has no workbook");
    assert!(!text_of(&saved, "ppt/charts/chart1.xml").contains("externalData"));
}

/// A chart past the presentation's point budget is not modelled, but a save writes the original
/// chart part and its workbook back instead of dropping both.
#[test]
fn a_chart_past_the_point_budget_is_saved_whole() {
    let src = with_workbook(&chart_pptx());
    let p = deckcraft_pptx::import_with_chart_budget(&src, 0).expect("import");
    let chart = p
        .slides
        .iter()
        .flat_map(|s| s.shapes.iter())
        .find_map(|s| match &s.kind {
            ShapeKind::Chart(c) => Some(c.clone()),
            _ => None,
        })
        .expect("the chart stays a chart");
    assert!(chart.verbatim && chart.series.is_empty());
    let saved = parts(&deckcraft_pptx::export(&p).expect("export"));
    let workbook = saved.iter().find(|(n, _)| n == "ppt/embeddings/kept1.xlsx").map(|(_, d)| d.as_slice());
    assert_eq!(workbook, Some(WORKBOOK));
    let xml = saved
        .iter()
        .find(|(n, _)| n.starts_with("ppt/charts/chart") && n.ends_with(".xml"))
        .map(|(_, d)| String::from_utf8_lossy(d).into_owned())
        .expect("chart xml");
    assert!(xml.contains("externalData") && xml.contains("c:pie"), "{xml}");
}
