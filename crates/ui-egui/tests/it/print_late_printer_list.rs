//! A printer list that arrives late must not undo what was picked while it was being read, when
//! the printer it names is the one the dialog already has.

use egui_kittest::Harness;
use pdfcraft_engine::print::spool::Printer;
use pdfcraft_ui_egui::PdfCraftApp;
use std::sync::{Arc, Mutex};

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

fn wait_for_list(h: &mut Harness<'static, PdfCraftApp>) {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    while h.state().print_draft.printers_loading && std::time::Instant::now() < deadline {
        h.run_steps(1);
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    assert!(!h.state().print_draft.printers_loading, "the printer list loaded");
}

#[test]
fn a_late_list_keeps_the_paper_picked_meanwhile() {
    let mut h = harness();
    // The source waits for the test to let it answer, once the first list is in.
    let gate = Arc::new(Mutex::new(()));
    let wait = gate.clone();
    h.state_mut().printer_source = Some(Arc::new(move || {
        let _open = wait.lock();
        vec![Printer { name: "Office".into(), default: true, two_sided: None, description: String::new() }]
    }));
    assert!(h.state_mut().execute("print.dialog"));
    wait_for_list(&mut h);
    h.run_steps(4);
    assert_eq!(h.state().print_draft.printer.as_deref(), Some("Office"));

    // "Office" is remembered with Tabloid (the third size).
    h.state_mut().print_draft.remembered_paper.insert("Office".into(), "Tabloid".into());
    h.state_mut().dialog = None;
    let hold = gate.lock().unwrap();
    assert!(h.state_mut().execute("print.dialog"));
    h.run_steps(2);
    assert!(h.state().print_draft.printers_loading, "the fresh list is still being read");
    assert_eq!(h.state().print_draft.printer.as_deref(), Some("Office"), "last time's list shows at once");
    assert_eq!((h.state().print_draft.paper_name().as_str(), h.state().print_draft.remember_paper), ("Tabloid", true));

    // Meanwhile the person picks another paper and unticks Remember.
    {
        let d = &mut h.state_mut().print_draft;
        d.paper = 0;
        d.remember_paper = false;
    }
    drop(hold);
    wait_for_list(&mut h);
    h.run_steps(2);
    let d = &h.state().print_draft;
    assert_eq!(d.printer.as_deref(), Some("Office"), "the same printer");
    assert_eq!((d.paper, d.remember_paper), (0, false), "the paper and the Remember box are as they were left");
}
