//! The Geospatial location tool reads a map page's latitude and longitude under the pointer,
//! keeps a clicked location in the Measure panel, and says when a page has no map.

use crate::support::AppHarness as _;
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use pdfcraft_ui_egui::PdfCraftApp;

/// Page 1 is a map whose viewport, the whole 200 × 200 page, spans 34–35°N, 118–117°W in WGS 84;
/// page 2 is blank.
const MAP: &[u8] = b"%PDF-1.7
1 0 obj << /Type /Catalog /Pages 2 0 R >> endobj
2 0 obj << /Type /Pages /Kids [3 0 R 4 0 R] /Count 2 /MediaBox [0 0 200 200] >> endobj
3 0 obj << /Type /Page /Parent 2 0 R /VP [<< /Type /Viewport /Name (Quadrangle) /BBox [0 0 200 200]
  /Measure << /Type /Measure /Subtype /GEO /GPTS [34 -118 35 -118 35 -117 34 -117] /GCS << /Type /GEOGCS /EPSG 4326 >> >> >>] >> endobj
4 0 obj << /Type /Page /Parent 2 0 R >> endobj
trailer << /Root 1 0 R >>
%%EOF";

fn harness() -> Harness<'static, PdfCraftApp> {
    let mut h = Harness::builder().with_size(egui::vec2(1400.0, 900.0)).build_eframe(|cc| {
        crate::support::install_runtime(&cc.egui_ctx);
        let mut app = crate::support::inline_app();
        app.set_option(&cc.egui_ctx, "language", "en").unwrap();
        app.open_bytes(&cc.egui_ctx, "map.pdf", None, MAP.to_vec()).unwrap();
        app
    });
    h.run_steps(4);
    h.set_option("zoom", "100").unwrap();
    h.run_steps(3);
    h
}

/// Run command `id` as the menus do.
fn run(h: &mut Harness<'static, PdfCraftApp>, id: &str) {
    let ctx = h.ctx.clone();
    h.state_mut().run_command(&ctx, id);
}

#[test]
fn pointing_at_a_map_reads_and_keeps_its_latitude_and_longitude() {
    let mut h = harness();
    let doc = h.state().views[0].id;
    let generation = h.state().session.get(doc).unwrap().edit_generation();
    run(&mut h, "measure.geo");
    h.run_steps(3);
    assert!(h.query_by_label_contains("Coordinate system: WGS 84").is_some());
    let page = h.state().views[0].page_screen_rect(0).unwrap();
    // Three quarters across and a quarter down the page: 34.75°N, 117.25°W.
    let at = page.min + egui::vec2(page.width() * 0.75, page.height() * 0.25);
    h.hover_at(at);
    h.run_steps(3);
    let location = h.state().views[0].session.measure.location.clone().expect("a location under the pointer");
    assert!((location.lat - 34.75).abs() < 0.01 && (location.lon + 117.25).abs() < 0.01, "{location:?}");
    assert!(h.query_by_label_contains(&location.dms).is_some(), "the panel shows {}", location.dms);
    assert!(h.state().views[0].session.measure.pinned.is_none());

    // A click keeps it.
    h.input_mut().events.push(egui::Event::PointerButton {
        pos: at,
        button: egui::PointerButton::Primary,
        pressed: true,
        modifiers: egui::Modifiers::NONE,
    });
    h.input_mut().events.push(egui::Event::PointerButton {
        pos: at,
        button: egui::PointerButton::Primary,
        pressed: false,
        modifiers: egui::Modifiers::NONE,
    });
    h.run_steps(3);
    let pinned = h.state().views[0].session.measure.pinned.clone().expect("a kept location");
    assert_eq!(pinned.decimal, location.decimal);
    assert!(h.query_by_label_contains("Kept location").is_some());

    // Off the page, the pointer has no location, and the kept one stays.
    h.hover_at(page.max + egui::vec2(40.0, 40.0));
    h.run_steps(3);
    assert!(h.query_by_label_contains(&location.dms).is_some(), "only the kept location is shown");
    assert_eq!(h.state().views[0].session.measure.pinned, Some(pinned));

    // The tool reads the page; it doesn't add anything to it.
    assert_eq!(h.state().session.get(doc).unwrap().edit_generation(), generation);
}

#[test]
fn a_page_without_a_map_says_so() {
    let mut h = harness();
    h.state_mut().views[0].go_to_page(1);
    run(&mut h, "measure.geo");
    h.run_steps(3);
    assert!(h.query_by_label("This page has no geospatial information.").is_some());
}

#[test]
fn a_kept_location_stays_to_copy_on_a_page_without_a_map() {
    let mut h = harness();
    run(&mut h, "measure.geo");
    h.run_steps(3);
    let page = h.state().views[0].page_screen_rect(0).unwrap();
    let at = page.center();
    h.hover_at(at);
    h.run_steps(2);
    h.input_mut().events.push(egui::Event::PointerButton {
        pos: at,
        button: egui::PointerButton::Primary,
        pressed: true,
        modifiers: egui::Modifiers::NONE,
    });
    h.input_mut().events.push(egui::Event::PointerButton {
        pos: at,
        button: egui::PointerButton::Primary,
        pressed: false,
        modifiers: egui::Modifiers::NONE,
    });
    h.run_steps(3);
    assert!(h.state().views[0].session.measure.pinned.is_some());
    // Another tool, on the blank page: the kept location is still there to copy.
    run(&mut h, "measure.distance");
    h.state_mut().views[0].go_to_page(1);
    h.run_steps(3);
    assert!(h.query_by_label_contains("Kept location").is_some());
    assert!(h.query_by_label("Copy").is_some());
}
