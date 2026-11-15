//! Snapshot with several tabs: each tab keeps its own area and its menu (Print area…, Save as
//! image…), so what the menu does is about that tab's area; leaving Snapshot puts away the area in
//! every tab, not only the one on screen.

use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use pdfcraft_ui_egui::{PdfCraftApp, QuickTool};

/// Two tabs, a.pdf (0) and b.pdf (1), with Snapshot chosen and the system clipboard left alone.
fn harness() -> Harness<'static, PdfCraftApp> {
    let mut h = Harness::builder().with_size(egui::vec2(1400.0, 900.0)).build_eframe(|_cc| {
        let mut app = PdfCraftApp::new();
        app.set_option("language", "en").unwrap();
        app.open_bytes("a.pdf", None, crate::support::fixtures::pdf_pages(2)).unwrap();
        app.open_bytes("b.pdf", None, crate::support::fixtures::pdf_pages(2)).unwrap();
        app.system_clipboard = false;
        app
    });
    crate::support::settle(&mut h);
    assert!(h.state_mut().execute("edit.snapshot"));
    h.run_steps(2);
    h
}

/// Take a snapshot of `area` on page 1 in tab `index`, as the Snapshot tool does at the end of a drag.
fn snapshot_in(h: &mut Harness<'static, PdfCraftApp>, index: usize, area: [f32; 4]) -> (u32, u32, Vec<u8>) {
    h.state_mut().active = Some(index);
    h.run_steps(2);
    h.state_mut().snapshot(index, 0, area).expect("a snapshot");
    h.state_mut().views[index].area_pick = Some((0, area));
    h.run_steps(2);
    let (w, hgt, px) = h.state().last_snapshot.clone().expect("kept");
    (w, hgt, px.to_vec())
}

/// Snapshot in tab A, then in tab B, back to A and Save as image…: the PNG is A's area, not the
/// snapshot B took last.
#[test]
fn save_as_image_writes_this_tabs_area() {
    let mut h = harness();
    // Areas of different shapes, so their pictures can't be mistaken for each other.
    let (wa, ha, pa) = snapshot_in(&mut h, 0, [10.0, 130.0, 150.0, 170.0]);
    let (wb, hb, _) = snapshot_in(&mut h, 1, [20.0, 20.0, 60.0, 140.0]);
    assert_ne!((wa, ha), (wb, hb));
    // Back to A: its area is still outlined with its menu.
    h.state_mut().active = Some(0);
    h.run_steps(3);
    assert_eq!(h.state().views[0].area_pick, Some((0, [10.0, 130.0, 150.0, 170.0])));
    let dir = std::env::temp_dir().join(format!("pdfcraft-snapshot-tabs-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let file = dir.join("a area.png");
    h.state_mut().save_override = Some(file.to_string_lossy().into_owned());
    h.get_by_label("Save as image…").click();
    h.run_steps(3);
    let png = std::fs::read(&file).expect("written");
    let _ = std::fs::remove_dir_all(&dir);
    // The PNG header's width and height (IHDR), then the picture itself.
    let size = |png: &[u8]| (u32::from_be_bytes(png[16..20].try_into().unwrap()), u32::from_be_bytes(png[20..24].try_into().unwrap()));
    assert_eq!(size(&png), (wa, ha), "tab A's area, not tab B's ({wb} × {hb})");
    assert_eq!(png, pdfcraft_engine::export::encode_png(wa, ha, &pa).unwrap(), "tab A's pixels");
}

/// Print area… opens Print on the area; the area stays outlined (with its menu) in the tab, as
/// the parity note says, until another tool is chosen.
#[test]
fn the_area_stays_after_print_area() {
    let mut h = harness();
    let area = [10.0, 130.0, 150.0, 170.0];
    snapshot_in(&mut h, 0, area);
    h.get_by_label("Print area…").click();
    h.run_steps(4);
    assert_eq!(h.state().dialog, Some(pdfcraft_ui_egui::Dialog::Print));
    assert_eq!(h.state().views[0].area_pick, Some((0, area)));
}

/// An area is picked in each of two tabs while Snapshot is on; choosing another tool puts both
/// away, including the tab that is not on screen.
#[test]
fn leaving_snapshot_clears_every_tab() {
    let mut h = harness();
    assert_eq!(h.state().quick_tool, QuickTool::Snapshot);
    for index in 0..2 {
        h.state_mut().active = Some(index);
        h.run_steps(2);
        h.state_mut().views[index].area_pick = Some((0, [10.0, 130.0, 150.0, 170.0]));
        h.run_steps(2);
        // Still Snapshot: the area stays outlined.
        assert!(h.state().views[index].area_pick.is_some(), "tab {index} keeps its area under Snapshot");
    }
    // Tab B is on screen; choose another tool.
    h.state_mut().quick_tool = QuickTool::Select;
    h.run_steps(3);
    assert!(h.state().views[1].area_pick.is_none(), "the tab on screen");
    assert!(h.state().views[0].area_pick.is_none(), "the other tab");
}

/// A change to the page list leaves no area behind: its page index would name another page.
#[test]
fn a_document_change_puts_the_area_away() {
    let mut h = harness();
    h.state_mut().views[0].area_pick = Some((0, [10.0, 130.0, 150.0, 170.0]));
    let info = h.state().session.get(h.state().views[0].id).unwrap().info.clone();
    h.state_mut().views[0].document_changed(&info);
    assert!(h.state().views[0].area_pick.is_none());
}
