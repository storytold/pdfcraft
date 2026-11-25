//! Edit ▸ Attach a file… attaches to the document that was active when the picker opened. If the
//! user switches tabs while choosing, the pick is dropped (and they are told) rather than the file
//! landing in whichever document is active by then.

use std::sync::Arc;

use craft::host::{AppInfo, Host, ManualJobs, Picked, ScriptedFiles, Wake};
use craft::platform::clipboard::{Clipboard, EventClipboard};
use craft::platform::open::FileIntake;
use craft::platform::watch::FileWatch;
use egui_kittest::Harness;
use pdfcraft_ui_egui::{AppServices, PdfCraftApp};

use crate::support;
use crate::support::AppHarness as _;

/// A one-page document with a proper xref table.
fn fixture() -> Vec<u8> {
    let objs = [
        "<< /Type /Catalog /Pages 2 0 R >>".to_string(),
        "<< /Type /Pages /Kids [3 0 R] /Count 1 /MediaBox [0 0 200 300] >>".to_string(),
        "<< /Type /Page /Parent 2 0 R >>".to_string(),
    ];
    let mut out = b"%PDF-1.7\n".to_vec();
    let mut offsets = Vec::new();
    for (i, o) in objs.iter().enumerate() {
        offsets.push(out.len());
        out.extend_from_slice(format!("{} 0 obj\n{o}\nendobj\n", i + 1).as_bytes());
    }
    let xref = out.len();
    out.extend_from_slice(format!("xref\n0 {}\n0000000000 65535 f \n", objs.len() + 1).as_bytes());
    for o in offsets {
        out.extend_from_slice(format!("{o:010} 00000 n \n").as_bytes());
    }
    out.extend_from_slice(format!("trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n", objs.len() + 1).as_bytes());
    out
}

/// The app on a host whose picks stay open until the test answers them, with the files that
/// host is asked for.
fn harness() -> (Harness<'static, PdfCraftApp>, Arc<ScriptedFiles>) {
    let files = Arc::new(ScriptedFiles::held());
    let jobs = Arc::new(ManualJobs::new());
    let host = Arc::new(Host::builder(AppInfo { id: "pdfcraft", name: "PdfCraft", version: "0" }).jobs(jobs.clone()).files(files.clone()).build());

    let (intake, _sender) = FileIntake::new(Wake::none());
    let services = AppServices::new(
        Wake::none(),
        Clipboard::new(EventClipboard::new(|| 0, |_| Ok(0))),
        None,
        None,
        intake,
        FileWatch::new(Wake::none()).inline(),
    );

    let mut h = Harness::builder().with_size(egui::vec2(1200.0, 800.0)).build_eframe(move |cc| {
        crate::support::install_runtime(&cc.egui_ctx);
        {
            let mut app = PdfCraftApp::with_host(host, services);
            app.set_option(&egui::Context::default(), "language", "en").unwrap();
            app.open_bytes(&egui::Context::default(), "a.pdf", None, fixture()).unwrap();
            app.open_bytes(&egui::Context::default(), "b.pdf", None, fixture()).unwrap();
            jobs.run_all();
            app
        }
    });
    h.run_steps(2);
    (h, files)
}

fn dirty(app: &PdfCraftApp, view: usize) -> bool {
    app.session.get(app.views[view].id).is_some_and(|d| d.dirty)
}

#[test]
fn an_attachment_pick_is_dropped_when_the_user_switches_tabs() {
    let dir = std::env::temp_dir().join(format!("pdfcraft-attach-target-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let file = dir.join("note.txt");
    std::fs::write(&file, b"hello").unwrap();

    let (mut h, files) = harness();
    h.state_mut().active = Some(0);
    assert!(h.execute("edit.attach"));
    h.run_steps(2);
    // The user switches tabs before the pick arrives.
    h.state_mut().active = Some(1);
    assert!(files.answer(Picked::Paths(vec![file.clone()])).is_some(), "the attach picker was open");
    h.run_steps(2);
    assert!(!dirty(h.state(), 0), "the document the pick was for is unchanged");
    assert!(!dirty(h.state(), 1), "the newly active document did not get the attachment");
    assert!(support::shown_notice(&h).is_some(), "the user is told why nothing happened");
    let _ = std::fs::remove_dir_all(&dir);
}
