//! File ▸ Send by email… while the email app is slow: if the person opens another dialog before
//! the answer comes, a "couldn't attach" answer waits until that dialog is closed, instead of
//! replacing it. The email app is never asked: a stand-in takes the request and answers when the
//! test says. Its temporary files are named after the process and the test.

use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, Sender, channel};

use egui_kittest::Harness;
use pdfcraft_platform::mail::{self, Answer, Compose, MailError, Why};
use pdfcraft_ui_egui::{Dialog, PdfCraftApp};

const PDF: &[u8] = include_bytes!("../data/form.pdf");

thread_local! {
    /// What the stand-in was asked to attach, and how to answer it.
    static SENT: RefCell<Vec<Compose>> = const { RefCell::new(Vec::new()) };
    static REPLY: RefCell<Option<Sender<Answer>>> = const { RefCell::new(None) };
}

fn stand_in(compose: Compose, _wake: Box<dyn FnOnce() + Send>) -> Result<Receiver<Answer>, MailError> {
    let (tx, rx) = channel();
    SENT.with(|s| s.borrow_mut().push(compose));
    REPLY.with(|r| *r.borrow_mut() = Some(tx));
    Ok(rx)
}

fn reveal(_path: &Path) -> Result<(), String> {
    Ok(())
}

fn answer(a: Answer) {
    REPLY.with(|r| r.borrow().as_ref().expect("a request was made").send(a).unwrap());
}

/// A document opened from its own file, which this test writes.
fn harness(test: &str) -> (Harness<'static, PdfCraftApp>, PathBuf) {
    let dir = std::env::temp_dir().join(format!("pdfcraft-send-email-dialog-test-{}-{test}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join(format!("{test}.pdf"));
    std::fs::write(&path, PDF).unwrap();
    let p = path.to_string_lossy().into_owned();
    let mut h = Harness::builder().with_size(egui::vec2(1400.0, 900.0)).build_eframe(move |_cc| {
        let mut app = PdfCraftApp::new();
        app.set_option("language", "en").unwrap();
        let name = Path::new(&p).file_name().unwrap().to_string_lossy().into_owned();
        app.open_bytes(&name, Some(p.clone()), PDF.to_vec()).unwrap();
        app.send_email.sender = stand_in;
        app.send_email.reveal = reveal;
        app
    });
    h.run_steps(4);
    (h, path)
}

/// The answer that the file wasn't attached comes while Preferences is open: Preferences stays,
/// and the answer is kept; once it is closed the Send by email dialog comes.
fn answer_while_another_dialog_is_open(test: &str, why: Why) {
    let (mut h, path) = harness(test);
    assert!(h.state_mut().execute("file.send_email"));
    h.run_steps(2);
    assert!(h.state().send_email.waiting());

    h.state_mut().dialog = Some(Dialog::Preferences);
    h.run_steps(2);
    let asked = SENT.with(|s| s.borrow().clone());
    assert_eq!(asked.len(), 1);
    answer(Answer::NotAttached { program: None, why, fallback: mail::fallback(&asked[0]) });
    h.run_steps(3);
    assert_eq!(h.state().dialog, Some(Dialog::Preferences), "the open dialog is not replaced");
    assert!(h.state().send_email.problem.is_none());
    assert!(h.state().send_email.waiting(), "the answer waits for the dialog to close");

    h.state_mut().dialog = None;
    h.run_steps(3);
    assert_eq!(h.state().dialog, Some(Dialog::SendEmail));
    assert!(h.state().send_email.problem.is_some());
    assert!(!h.state().send_email.waiting());
    let _ = std::fs::remove_dir_all(path.parent().unwrap());
}

#[test]
fn a_not_attached_answer_waits_for_the_open_dialog() {
    answer_while_another_dialog_is_open("not-attached", Why::CantAttach);
}

#[test]
fn a_no_answer_answer_waits_for_the_open_dialog() {
    answer_while_another_dialog_is_open("no-answer", Why::NoAnswer);
}
