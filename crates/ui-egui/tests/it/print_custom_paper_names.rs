//! A custom paper cannot take the name of a standard one: papers are looked up by name, so the
//! standard size would be found first and a remembered custom paper would come back as it.

use egui::accesskit::Role;
use egui_kittest::Harness;
use egui_kittest::kittest::{NodeT, Queryable};
use pdfcraft_ui_egui::PdfCraftApp;

fn harness() -> Harness<'static, PdfCraftApp> {
    let mut h = Harness::builder().with_size(egui::vec2(1400.0, 900.0)).build_eframe(|_cc| {
        let mut app = PdfCraftApp::new();
        app.set_option("language", "en").unwrap();
        app.open_bytes("form.pdf", None, include_bytes!("../data/form.pdf").to_vec()).unwrap();
        app
    });
    h.run_steps(4);
    h
}

#[test]
fn a_custom_paper_cannot_take_a_standard_name() {
    let mut h = harness();
    // No printers here: the list is empty as soon as it is asked for.
    h.state_mut().printer_source = Some(std::sync::Arc::new(Vec::new));
    assert!(h.state_mut().execute("print.dialog"));
    for _ in 0..500 {
        h.run_steps(1);
        if !h.state().print_draft.printers_loading {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(2));
    }
    assert!(!h.state().print_draft.printers_loading, "the printer list loaded");
    h.run_steps(8);
    h.get_by_label("Custom…").click();
    h.run_steps(2);
    let before = h.state().print_draft.paper_count();
    let per_point = h.state().print_draft.units.per_point();
    let (w, l) = (format!("{}", 288.0 * per_point), format!("{}", 432.0 * per_point));
    // The name of a standard paper, in either case, is refused: Add stays disabled.
    for name in ["A4", " a4 "] {
        h.state_mut().print_draft.custom_new = Some((name.into(), w.clone(), l.clone()));
        h.run_steps(3);
        assert!(h.get_by_role_and_label(Role::Button, "Add").accesskit_node().is_disabled(), "{name:?} is a standard paper's name");
        assert_eq!(h.state().print_draft.paper_count(), before);
    }
    // Any other name is accepted by the same row.
    h.state_mut().print_draft.custom_new = Some(("A4 label".into(), w.clone(), l.clone()));
    h.run_steps(3);
    assert!(!h.get_by_role_and_label(Role::Button, "Add").accesskit_node().is_disabled());

    // Settings read back are checked the same way: the custom "A4" is dropped, the rest kept.
    let paper = |name: &str| serde_json::json!({ "name": name, "width": 288.0, "height": 432.0 });
    let settings = serde_json::json!({
        "print_custom_papers": [paper("A4"), paper("Label"), paper("us letter")],
        "print_paper_by_printer": { "": "A4" },
    });
    let mut fresh = PdfCraftApp::new();
    fresh.restore(&settings.to_string());
    let names: Vec<&str> = fresh.print_draft.custom_papers.iter().map(|p| p.name.as_str()).collect();
    assert_eq!(names, ["Label"]);
}
