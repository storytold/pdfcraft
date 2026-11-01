//! Snapshot's area menu (Print area…, Save as image…) with several tabs: each tab keeps its own
//! area, so what it does must be about that tab's area.

use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use pdfcraft_ui_egui::PdfCraftApp;

/// `n` pages of 200×300, each saying "Page i+1".
fn fixture(n: usize) -> Vec<u8> {
    let mut objs: Vec<String> = vec!["<< /Type /Catalog /Pages 2 0 R >>".into()];
    let kids: Vec<String> = (0..n).map(|i| format!("{} 0 R", 4 + 2 * i)).collect();
    objs.push(format!("<< /Type /Pages /Kids [{}] /Count {n} /MediaBox [0 0 200 300] >>", kids.join(" ")));
    objs.push("<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>".into());
    for i in 0..n {
        objs.push(format!("<< /Type /Page /Parent 2 0 R /Contents {} 0 R /Resources << /Font << /F1 3 0 R >> >> >>", 5 + 2 * i));
        let body = format!("BT /F1 18 Tf 20 150 Td (Page {}) Tj ET", i + 1);
        objs.push(format!("<< /Length {} >>\nstream\n{body}\nendstream", body.len()));
    }
    let mut out = b"%PDF-1.7\n".to_vec();
    let mut offs = Vec::new();
    for (i, o) in objs.iter().enumerate() {
        offs.push(out.len());
        out.extend_from_slice(format!("{} 0 obj\n{o}\nendobj\n", i + 1).as_bytes());
    }
    let x = out.len();
    out.extend_from_slice(format!("xref\n0 {}\n0000000000 65535 f \n", objs.len() + 1).as_bytes());
    for o in offs {
        out.extend_from_slice(format!("{o:010} 00000 n \n").as_bytes());
    }
    out.extend_from_slice(format!("trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{x}\n%%EOF\n", objs.len() + 1).as_bytes());
    out
}

/// Two tabs, a.pdf (0) and b.pdf (1), with Snapshot chosen and the system clipboard left alone.
fn harness() -> Harness<'static, PdfCraftApp> {
    let mut h = Harness::builder().with_size(egui::vec2(1400.0, 900.0)).build_eframe(|_cc| {
        let mut app = PdfCraftApp::new();
        app.set_option("language", "en").unwrap();
        app.open_bytes("a.pdf", None, fixture(2)).unwrap();
        app.open_bytes("b.pdf", None, fixture(2)).unwrap();
        app.system_clipboard = false;
        app
    });
    // Background rendering can take seconds on a loaded CI runner.
    for _ in 0..3000 {
        h.run_steps(2);
        if !h.state().render_pending() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
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
