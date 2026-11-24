//! Series-level data labels: `c:dLbls/c:showVal` on a series overrides the chart-wide setting,
//! and a bare `<c:showVal/>` (no `val` attribute) means true, as the schema's default (#868).

use std::io::{Cursor, Read, Write};
use std::sync::Arc;

use deckcraft_geom::Xfrm;
use deckcraft_model::chart::{Chart, ChartType};
use deckcraft_model::{LayoutType, Presentation, Shape, ShapeId, ShapeKind, defaults};

/// A one-slide deck whose bar chart shows no labels anywhere.
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

/// Rewrite the chart part's XML through `f` and re-import the deck.
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

/// `<c:showVal/>` on the first series: that series shows labels although the chart-wide
/// setting is off; the other series stay quiet.
#[test]
fn bare_series_showval_shows_labels() {
    let p = reimport(&bar_chart_pptx(), |xml| xml.replacen("<c:cat>", r#"<c:dLbls><c:showVal/></c:dLbls><c:cat>"#, 1));
    let c = chart_of(&p);
    assert!(!c.data_labels, "the chart-wide setting stays off");
    assert_eq!(c.series[0].data_labels, Some(true), "a bare showVal means true");
    assert!(c.series_data_labels(&c.series[0]), "the first series' labels show");
    for s in c.series.iter().skip(1) {
        assert_eq!(s.data_labels, None, "other series keep the chart's setting");
        assert!(!c.series_data_labels(s), "and stay quiet");
    }
}

/// `val="0"` on the series hides its labels although the chart-wide setting is on.
#[test]
fn series_showval_false_hides_labels() {
    let p = reimport(&bar_chart_pptx(), |xml| {
        xml.replacen("<c:cat>", r#"<c:dLbls><c:showVal val="0"/></c:dLbls><c:cat>"#, 1).replacen(
            "<c:gapWidth",
            r#"<c:dLbls><c:showVal/></c:dLbls><c:gapWidth"#,
            1,
        )
    });
    let c = chart_of(&p);
    assert!(c.data_labels, "the bare chart-wide showVal turns labels on");
    assert_eq!(c.series[0].data_labels, Some(false), "the explicit series false is read");
    assert!(!c.series_data_labels(&c.series[0]), "and hides that series' labels");
    assert!(c.series_data_labels(&c.series[1]), "the next series takes the chart-wide on");
}

/// The series' label setting survives a save and load.
#[test]
fn series_label_settings_round_trip() {
    let mut p = defaults::new_presentation(None);
    p.slides.clear();
    let layout = defaults::layout_of_kind(&p, LayoutType::Blank).unwrap_or_default();
    let mut slide = defaults::new_slide(&mut p, layout).expect("slide");
    let mut chart = Chart::sample(ChartType::Column);
    chart.data_labels = true;
    chart.series[0].data_labels = Some(false);
    chart.series[1].data_labels = Some(true);
    let mut shape =
        Shape { id: ShapeId(p.alloc_id().expect("id")), name: "Bars".into(), xfrm: Some(Xfrm::new(40.0, 40.0, 400.0, 300.0)), ..Default::default() };
    shape.kind = ShapeKind::Chart(Box::new(chart));
    slide.shapes.push(shape);
    p.slides.push(Arc::new(slide));

    let bytes = deckcraft_pptx::export(&p).expect("export");
    let back = deckcraft_pptx::import(&bytes).expect("import");
    let c = chart_of(&back);
    assert!(c.data_labels);
    assert_eq!(c.series[0].data_labels, Some(false), "the explicit off survives");
    assert_eq!(c.series[1].data_labels, Some(true), "the explicit on survives");
    assert!(!c.series_data_labels(&c.series[0]));
    assert!(c.series_data_labels(&c.series[1]));
}

/// A one-slide deck with a combo chart: a bar group and a line group.
fn combo_chart_pptx() -> Vec<u8> {
    let mut p = defaults::new_presentation(None);
    p.slides.clear();
    let layout = defaults::layout_of_kind(&p, LayoutType::Blank).unwrap_or_default();
    let mut slide = defaults::new_slide(&mut p, layout).expect("slide");
    let mut chart = Chart::sample(ChartType::Combo);
    chart.series.truncate(2);
    chart.series[1].kind = Some(ChartType::Line);
    let mut shape =
        Shape { id: ShapeId(p.alloc_id().expect("id")), name: "Bars".into(), xfrm: Some(Xfrm::new(40.0, 40.0, 400.0, 300.0)), ..Default::default() };
    shape.kind = ShapeKind::Chart(Box::new(chart));
    slide.shapes.push(shape);
    p.slides.push(Arc::new(slide));
    deckcraft_pptx::export(&p).expect("export")
}

/// A combo chart's groups keep their own label setting: the group that asks for no labels
/// keeps them off, even when the other group asks for them (#868).
#[test]
fn a_combo_chart_keeps_each_groups_own_label_setting() {
    let p = reimport(&combo_chart_pptx(), |xml| {
        // The bar group asks for no labels, the line group for labels.
        xml.replacen("<c:gapWidth", r#"<c:dLbls><c:showVal val="0"/></c:dLbls><c:gapWidth"#, 1).replacen(
            "<c:marker val=\"1\"/>",
            r#"<c:dLbls><c:showVal/></c:dLbls><c:marker val="1"/>"#,
            1,
        )
    });
    let c = chart_of(&p);
    assert!(c.data_labels, "the line group's ask turns the chart-wide setting on");
    assert_eq!(c.series[0].data_labels, Some(false), "the bar group asks for no labels");
    assert!(!c.series_data_labels(&c.series[0]), "so the bars stay quiet");
    assert!(c.series_data_labels(&c.series[1]), "the line takes the chart-wide on");
}

/// Turning Data Labels on through `chart.options` shows the labels of series that the file
/// switched off one by one.
#[test]
fn chart_options_data_labels_on_overrides_series_off() {
    use deckcraft_engine::Session;
    let p = reimport(&bar_chart_pptx(), |xml| xml.replace("<c:cat>", r#"<c:dLbls><c:showVal val="0"/></c:dLbls><c:cat>"#));
    assert!(chart_of(&p).series.iter().all(|s| !chart_of(&p).series_data_labels(s)), "every series starts quiet");
    let path = std::env::temp_dir().join("deckcraft_series_labels_off.pptx");
    std::fs::write(&path, deckcraft_pptx::export(&p).expect("export")).expect("write");
    let mut s = Session::with_new();
    s.execute("file.open", &serde_json::json!({"path": path.to_string_lossy()})).expect("open");
    let doc = s.doc().unwrap().doc.clone();
    let id = doc.slides[0].shapes.iter().find(|sh| matches!(sh.kind, ShapeKind::Chart(_))).expect("chart").id.0;
    assert!(chart_of(&doc).series.iter().all(|x| !chart_of(&doc).series_data_labels(x)));
    s.execute("chart.options", &serde_json::json!({"id": id, "dataLabels": true})).expect("options");
    let doc = s.doc().unwrap().doc.clone();
    let c = chart_of(&doc);
    assert!(c.series.iter().all(|x| c.series_data_labels(x)), "every series shows its labels");
    let _ = std::fs::remove_file(path);
}
