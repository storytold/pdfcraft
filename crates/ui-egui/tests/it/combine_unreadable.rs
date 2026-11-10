//! Combine files: when several files can't be read, the one notice names all of them.

use crate::support;
use egui_kittest::Harness;
use pdfcraft_ui_egui::{OsEvent, PdfCraftApp};

#[test]
fn every_unreadable_path_is_named() {
    let dir = std::env::temp_dir().join(format!("pdfcraft-combine-unreadable-{}", std::process::id()));
    let paths: Vec<String> = ["first-missing.pdf", "second-missing.pdf"].iter().map(|n| dir.join(n).to_string_lossy().into_owned()).collect();
    let queue = std::rc::Rc::new(std::cell::RefCell::new(vec![OsEvent::Combine(paths)]));
    let q = queue.clone();
    let mut h = Harness::builder().with_size(egui::vec2(1400.0, 900.0)).build_eframe(move |_cc| {
        let mut app = PdfCraftApp::new();
        app.set_option("language", "en").unwrap();
        app.os_events = Some(Box::new(move || q.borrow_mut().drain(..).collect()));
        app
    });
    // The files are read on a worker thread; the notice comes with the answer.
    support::wait_until(&mut h, "the notice", |app| app.toast.is_some());
    let (notice, _) = h.state().toast.clone().expect("a notice is shown");
    assert!(notice.contains("first-missing.pdf"), "the first failure is named: {notice}");
    assert!(notice.contains("second-missing.pdf"), "the last failure is named: {notice}");
    assert!(h.state().combine_draft.is_empty(), "nothing readable, nothing listed");
}
