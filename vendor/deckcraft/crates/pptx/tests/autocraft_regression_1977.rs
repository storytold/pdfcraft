//! A series-level `c:dLbls/c:delete val="1"` hides that series' labels although the
//! chart-wide `showVal` is on (#1977).

use std::io::{Cursor, Read, Write};
use std::sync::Arc;

use deckcraft_geom::Xfrm;
use deckcraft_model::chart::{Chart, ChartType};
use deckcraft_model::{LayoutType, Presentation, Shape, ShapeId, ShapeKind, defaults};

fn bar_chart_pptx() -> Vec<u8> {
    let mut p = defaults::new_presentation(None);
    p.slides.clear();
    let layout = defaults::layout_of_kind(&p, LayoutType::Blank).unwrap_or_default();
    let mut slide = defaults::new_slide(&mut p, layout).expect("slide");
    let mut shape =
        Shape { id: ShapeId(p.alloc_id().expect("id")), name: "Bars".into(), xfrm: Some(Xfrm::new(40.0, 40.0, 400.0, 300.0)), ..Default::default() };
    let mut chart = Chart::sample(ChartType::Column);
    chart.data_labels = false;
    for s in &mut chart.series {
        s.data_labels = None;
    }
    shape.kind = ShapeKind::Chart(Box::new(chart));
    slide.shapes.push(shape);
    p.slides.push(Arc::new(slide));
    deckcraft_pptx::export(&p).expect("export")
}

fn reimport(bytes: &[u8], f: impl Fn(&str) -> String) -> Presentation {
    let mut zip = zip::ZipArchive::new(Cursor::new(bytes)).expect("zip");
    let mut out = zip::ZipWriter::new(Cursor::new(Vec::new()));
    let opts = zip::write::SimpleFileOptions::default();
    for i in 0..zip.len() {
        let mut entry = zip.by_index(i).expect("entry");
        let name = entry.name().to_string();
        let mut data = vec![];
        entry.read_to_end(&mut data).expect("read");
        if name.starts_with("ppt/charts/chart") && name.ends_with(".xml") && !name.contains("_rels") {
            let xml = String::from_utf8(data).expect("chart xml");
            data = f(&xml).into_bytes();
        }
        out.start_file(&name, opts).expect("start");
        out.write_all(&data).expect("write");
    }
    let patched = out.finish().expect("finish").into_inner();
    deckcraft_pptx::import(&patched).expect("import")
}

fn chart_of(p: &Presentation) -> &Chart {
    let s = p.slides.first().expect("slide");
    for sh in &s.shapes {
        if let ShapeKind::Chart(c) = &sh.kind {
            return c;
        }
    }
    panic!("no chart in the deck");
}

#[test]
fn series_dlbls_delete_hides_that_series_labels() {
    let p = reimport(&bar_chart_pptx(), |xml| {
        xml.replacen("<c:cat>", r#"<c:dLbls><c:delete val="1"/></c:dLbls><c:cat>"#, 1).replacen(
            "<c:gapWidth",
            r#"<c:dLbls><c:showVal val="1"/></c:dLbls><c:gapWidth"#,
            1,
        )
    });
    let c = chart_of(&p);
    assert!(c.data_labels, "the group-level showVal turns labels on");
    assert_eq!(c.series[0].data_labels, Some(false), "the deleted series has no labels");
    assert!(!c.series_data_labels(&c.series[0]), "so its labels are not drawn");
    assert!(c.series_data_labels(&c.series[1]), "the next series shows the chart-wide labels");
}
