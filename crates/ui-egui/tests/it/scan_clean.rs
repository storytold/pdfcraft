//! Scan & OCR ▸ Enhance scanned file in the real shell (egui_kittest): the dialog, the run and
//! its report, and that the page keeps its comment while its scan is cleaned in place.

use std::sync::Arc;

use crate::support::AppHarness as _;
use craft::host::ManualJobs;
use craft::pages::ui::PageRangeOptions;
use craft::pages::{PageRange, PageScope};
use craft::ui::dialog::dialog_id;
use craft::ui::patterns::form_dialogs::HostedParamDialog;
use egui_kittest::Harness;
use pdfcraft_engine::{Edit, NewAnnotation, NoteIcon, Session, Shape, Style};
use pdfcraft_ui_egui::PdfCraftApp;

/// A two-page PDF: page 1 a grey scan with a darker gutter and text bars (and a sticky note),
/// page 2 the same scan again.
fn scan() -> Vec<u8> {
    let img = image::GrayImage::from_fn(300, 400, |x, y| {
        let light = 0.55 + 0.45 * (x as f32 / 60.0).min(1.0);
        let ink = if (60..360).contains(&y) && (y - 60) % 40 < 6 && (60..240).contains(&x) { 0.12 } else { 0.78 };
        image::Luma([(ink * light * 255.0) as u8])
    });
    let mut png = Vec::new();
    img.write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png).unwrap();
    let mut s = Session::new();
    let bytes = s.create_from_images(&[("a.png".into(), png.clone()), ("b.png".into(), png)]).unwrap();
    let id = s.open("scan.pdf", None, bytes, None).unwrap();
    let shape = Shape::Note { at: [20.0, 20.0], icon: NoteIcon::Comment };
    let note = NewAnnotation { page: 0, style: Style::default_for(&shape), shape, contents: "Keep me".into(), author: "Tester".into() };
    s.apply(id, Edit::AddAnnotation(note)).unwrap();
    s.save_bytes(id).unwrap().to_vec()
}

/// The scan open in an app whose jobs wait until `run_jobs` runs them.
fn harness() -> (Harness<'static, PdfCraftApp>, Arc<ManualJobs>) {
    let (app, jobs) = crate::control::app_with_manual_jobs();
    let mut h = Harness::builder().with_size(egui::vec2(1400.0, 900.0)).build_eframe(move |cc| {
        crate::support::install_runtime(&cc.egui_ctx);
        {
            let mut app = app;
            app.set_option(&egui::Context::default(), "language", "en").unwrap();
            app.open_bytes(&egui::Context::default(), "scan.pdf", None, scan()).unwrap();
            app
        }
    });
    h.run_steps(4);
    (h, jobs)
}

/// Runs the host's queued jobs, then lets the app poll them.
fn run_jobs(h: &mut Harness<'static, PdfCraftApp>, jobs: &ManualJobs) {
    jobs.run_all();
    h.run_steps(3);
}

#[test]
fn enhance_scanned_file_cleans_the_chosen_pages_and_says_what_it_did() {
    let (mut h, jobs) = harness();
    let id = h.state().active_ids().unwrap().1;
    assert!(h.state().session.get(id).unwrap().comment_props(0, 0).is_some());
    assert!(h.execute("ocr.enhance"));
    h.run_steps(2);
    let ctx = h.ctx.clone();
    let current = PageRange { scope: PageScope::Current, ..PageRange::default() };
    let value = serde_json::to_value(PageRangeOptions::field_value(&current)).unwrap();
    h.state_mut().dialogs.set_json_field(&ctx, dialog_id("enhance-scan"), "pages", value).unwrap();
    h.run_steps(1);
    assert!(h.state_mut().dialogs.confirm::<HostedParamDialog>(&ctx));
    h.run_steps(2);
    run_jobs(&mut h, &jobs);
    assert!(crate::support::shown_notice(&h).is_some(), "a notice says what was done");
    let app = h.state();
    assert!(app.scan_clean_run.is_none() && !app.dialogs.is_open::<HostedParamDialog>(), "finished and closed");
    let doc = app.session.get(id).unwrap();
    assert!(doc.can_undo().is_some(), "one undo step restores the page");
    assert_eq!(doc.info.pages.len(), 2);
    assert_eq!(doc.comment_props(0, 0).map(|p| p.author), Some("Tester".to_string()), "the sticky note stays");
}

#[test]
fn nothing_to_enhance_is_said_not_done() {
    let (mut h, jobs) = harness();
    let id = h.state().active_ids().unwrap().1;
    h.state_mut().scan_clean_draft.settings = pdfcraft_engine::scan_clean::ScanCleanSettings { level: 0, edge_shadows: false, despeckle: false };
    // With every filter off the dialog's OK is disabled; starting anyway changes nothing.
    let ctx_h = h.ctx.clone();
    h.state_mut().start_scan_clean(&ctx_h);
    run_jobs(&mut h, &jobs);
    assert!(crate::support::shown_notice(&h).is_some(), "a notice says nothing was cleaned");
    let app = h.state();
    assert_eq!(app.session.get(id).unwrap().can_undo(), None);
}
