//! File ▸ Send by email… in the real shell (egui_kittest). The email app is never asked: a
//! stand-in takes the request and answers when the test says, and another stands in for the file
//! manager. Standalone (not in `ui_group`): it writes process-ID temporary files.

use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, Sender, channel};

use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use pdfcraft_platform::mail::{self, Answer, Compose, MailError, Why};
use pdfcraft_ui_egui::{Dialog, PdfCraftApp};

const PDF: &[u8] = include_bytes!("data/form.pdf");

thread_local! {
    /// What the stand-in was asked to attach, and how to answer it.
    static SENT: RefCell<Vec<Compose>> = const { RefCell::new(Vec::new()) };
    static REPLY: RefCell<Option<Sender<Answer>>> = const { RefCell::new(None) };
    static REVEALED: RefCell<Vec<PathBuf>> = const { RefCell::new(Vec::new()) };
}

fn stand_in(compose: Compose, _wake: Box<dyn FnOnce() + Send>) -> Result<Receiver<Answer>, MailError> {
    let (tx, rx) = channel();
    SENT.with(|s| s.borrow_mut().push(compose));
    REPLY.with(|r| *r.borrow_mut() = Some(tx));
    Ok(rx)
}

fn reveal(path: &Path) -> Result<(), String> {
    REVEALED.with(|r| r.borrow_mut().push(path.to_path_buf()));
    Ok(())
}

fn answer(a: Answer) {
    REPLY.with(|r| r.borrow().as_ref().expect("a request was made").send(a).unwrap());
}

fn sent() -> Vec<Compose> {
    SENT.with(|s| s.borrow().clone())
}

/// A document opened from its own file, which this test writes.
fn harness(test: &str) -> (Harness<'static, PdfCraftApp>, PathBuf) {
    let dir = std::env::temp_dir().join(format!("pdfcraft-send-email-test-{}-{test}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join(format!("{test}.pdf"));
    std::fs::write(&path, PDF).unwrap();
    let p = path.to_string_lossy().into_owned();
    let mut h = Harness::builder().with_size(egui::vec2(1400.0, 900.0)).build_eframe(move |_cc| {
        let mut app = PdfCraftApp::new();
        app.set_option("language", "en").unwrap();
        app.open_bytes(&format!("{}.pdf", Path::new(&p).file_stem().unwrap().to_string_lossy()), Some(p.clone()), PDF.to_vec()).unwrap();
        app.send_email.sender = stand_in;
        app.send_email.reveal = reveal;
        app
    });
    h.run_steps(4);
    (h, path)
}

/// The command is in the File menu; an unchanged file goes as it is; a default that can't take a
/// file brings the dialog, whose main button opens a message without it and shows the file.
#[test]
fn an_email_app_that_cant_take_the_file_gets_a_message_and_the_file_is_shown() {
    let (mut h, path) = harness("unchanged");
    h.get_by_label("Menu").click();
    h.run_steps(2);
    h.get_by_label("File ⏵").hover();
    h.run_steps(3);
    // The menu item's label ends with its (empty) shortcut text.
    h.get_by_label_contains("Send by email…").click();
    h.run_steps(2);

    let asked = sent();
    assert_eq!(asked.len(), 1);
    assert_eq!(asked[0].path, path, "the file itself, unchanged");
    assert_eq!((asked[0].subject.as_str(), asked[0].to.len()), ("unchanged", 0));
    assert!(h.state().send_email.waiting());
    assert_eq!(h.state().toast.as_ref().map(|t| t.0.as_str()), Some("Attaching unchanged.pdf to a new email message…"));

    // A second request while the first is under way asks nothing more.
    assert!(h.state_mut().execute("file.send_email"));
    assert_eq!(h.state().toast.as_ref().map(|t| t.0.as_str()), Some("Still waiting for your email app…"));
    assert_eq!(sent().len(), 1);

    // Still no answer: nothing changes.
    h.run_steps(2);
    assert!(h.state().send_email.waiting() && h.state().dialog.is_none());

    answer(Answer::NotAttached { program: Some("Firefox".into()), why: Why::CantAttach, fallback: mail::fallback(&asked[0]) });
    h.run_steps(3);
    assert_eq!(h.state().dialog, Some(Dialog::SendEmail));
    assert!(!h.state().send_email.waiting());
    assert!(h.state().toast.is_none(), "the waiting notice goes when the dialog comes");
    h.get_by_label("Couldn't attach the file");
    h.get_by_label_contains("Firefox can't take a file from PdfCraft.");
    h.get_by_label_contains("PdfCraft will show unchanged.pdf in its folder");
    h.get_by_label("Close");
    h.get_by_label(if cfg!(target_os = "macos") { "Reveal in Finder" } else { "Reveal in File Explorer" });
    h.get_by_label("Open message and show file").click();
    h.run_steps(3);
    assert_eq!(h.state().dialog, None);
    assert_eq!(h.state().last_opened_url.as_deref(), Some("mailto:?subject=unchanged"));
    assert_eq!(REVEALED.with(|r| r.borrow().clone()), vec![path.clone()]);
    let _ = std::fs::remove_dir_all(path.parent().unwrap());
}

/// Unsaved edits go as a copy under the temp folder; the open file and the document stay as they
/// were (nobody is asked to save). An opened message brings no dialog.
#[test]
fn unsaved_edits_go_as_a_copy_and_the_open_file_is_left_alone() {
    let (mut h, path) = harness("edited");
    h.state_mut().apply_edit(pdfcraft_engine::Edit::RotatePages { pages: vec![0], degrees: 90 });
    let id = h.state().views[0].id;
    assert!(h.state().session.get(id).unwrap().dirty);
    assert!(h.state_mut().execute("file.send_email"));
    h.run_steps(2);

    let asked = sent();
    assert_eq!(asked.len(), 1);
    let copy = &asked[0].path;
    assert_ne!(copy, &path);
    assert!(copy.starts_with(std::env::temp_dir()) && copy.ends_with("edited.pdf"), "{}", copy.display());
    // The edits, saved after the original bytes (an incremental save; its date is the save's own).
    let written = std::fs::read(copy).unwrap();
    assert!(written.len() > PDF.len() && written.starts_with(PDF), "the copy is the document with its edits");
    assert!(written.windows(10).any(|w| w == b"/Rotate 90"), "the copy has the rotation");
    assert!(std::fs::read(&path).unwrap() == PDF, "the open file is unchanged");
    let doc = h.state().session.get(id).unwrap();
    assert!(doc.dirty && doc.path.as_deref() == Some(path.to_string_lossy().as_ref()), "the document is still the open file, with its edits unsaved");
    assert!(h.state().dialog.is_none(), "nobody is asked to save");

    answer(Answer::Opened { program: Some("Mail".into()) });
    h.run_steps(3);
    assert!(!h.state().send_email.waiting() && h.state().dialog.is_none() && h.state().send_email.problem.is_none());
    let _ = std::fs::remove_file(copy);
    let _ = std::fs::remove_dir_all(path.parent().unwrap());
}

/// A program that didn't answer, or no email app at all: the dialog says so, and Close closes it.
#[test]
fn every_failure_is_said_and_close_closes() {
    let (mut h, path) = harness("failures");
    for (why, text) in [
        (Why::NoProgram, "No email app is set up on this computer."),
        (Why::NoAnswer, "Your email app didn't open a message in time."),
        (Why::Failed("MAPI error 2".into()), "Your email app didn't open a message. It said: MAPI error 2."),
    ] {
        assert!(h.state_mut().execute("file.send_email"));
        h.run_steps(2);
        let asked = sent().last().cloned().unwrap();
        answer(Answer::NotAttached { program: None, why, fallback: mail::fallback(&asked) });
        h.run_steps(3);
        assert_eq!(h.state().dialog, Some(Dialog::SendEmail));
        h.get_by_label_contains(text);
        h.get_by_label("Close").click();
        h.run_steps(3);
        assert!(h.state().dialog.is_none() && h.state().send_email.problem.is_none());
    }
    assert!(REVEALED.with(|r| r.borrow().is_empty()) && h.state().last_opened_url.is_none(), "Close opens nothing");
    let _ = std::fs::remove_dir_all(path.parent().unwrap());
}
