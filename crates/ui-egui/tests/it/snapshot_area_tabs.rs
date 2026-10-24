//! The Snapshot tool belongs to the app, not to a tab: leaving it puts away the copied area
//! (and its menu) in every tab, not only the one on screen.

use egui_kittest::Harness;
use pdfcraft_ui_egui::{PdfCraftApp, QuickTool};

/// One page of 200×300 saying "Page 1".
fn fixture() -> Vec<u8> {
    let body = "BT /F1 18 Tf 20 150 Td (Page 1) Tj ET";
    let objs = [
        "<< /Type /Catalog /Pages 2 0 R >>".to_string(),
        "<< /Type /Pages /Kids [4 0 R] /Count 1 /MediaBox [0 0 200 300] >>".to_string(),
        "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>".to_string(),
        "<< /Type /Page /Parent 2 0 R /Contents 5 0 R /Resources << /Font << /F1 3 0 R >> >> >>".to_string(),
        format!("<< /Length {} >>\nstream\n{body}\nendstream", body.len()),
    ];
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
        app.open_bytes("a.pdf", None, fixture()).unwrap();
        app.open_bytes("b.pdf", None, fixture()).unwrap();
        app.system_clipboard = false;
        app
    });
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
