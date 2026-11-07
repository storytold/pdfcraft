//! File ▸ Attach to email… in the real shell (egui_kittest). The email app is never asked: a
//! stand-in takes the request and answers when the test says, and another stands in for the file
//! manager. Its temporary files are named after the process and the test.

use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, Sender, channel};

use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use pdfcraft_engine::{Edit, NewAnnotation, Shape, Style, rect_quad};
use pdfcraft_platform::mail::{self, Answer, Compose, MailError, Why};
use pdfcraft_ui_egui::{Dialog, PdfCraftApp};

const PDF: &[u8] = include_bytes!("../data/form.pdf");

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
    crate::support::open_menu(&mut h, "File");
    // Pressed as a screen reader would: a pointer click on the File menu's lower items doesn't
    // reach them headless.
    crate::support::menu_row(&h, "Attach to email…").click_accesskit();
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
    // The copy's folder is its own, made for this send.
    let _ = std::fs::remove_dir_all(copy.parent().unwrap());
    let _ = std::fs::remove_dir_all(path.parent().unwrap());
}

/// Unapplied redaction marks hide nothing in the file, so sending asks first: Cancel sends nothing,
/// Send anyway sends, and the next send asks again.
#[test]
fn unapplied_redaction_marks_ask_before_sending() {
    let (mut h, path) = harness("marks");
    let shape = Shape::Redact { quads: vec![rect_quad([80.0, 140.0, 100.0, 180.0])], overlay: String::new(), look: Default::default() };
    let style = Style::default_for(&shape);
    let mark = NewAnnotation { page: 0, shape, style, contents: String::new(), author: "Test".into() };
    assert!(h.state_mut().apply_edit(Edit::AddAnnotation(mark)));

    assert!(h.state_mut().execute("file.send_email"));
    h.run_steps(2);
    assert_eq!(h.state().dialog, Some(Dialog::SendEmailMarks));
    h.get_by_label("Cancel").click();
    h.run_steps(3);
    assert!(h.state().dialog.is_none() && sent().is_empty(), "Cancel sends nothing");

    assert!(h.state_mut().execute("file.send_email"));
    h.run_steps(2);
    h.get_by_label("Send anyway").click();
    h.run_steps(3);
    assert!(h.state().dialog.is_none());
    assert_eq!(sent().len(), 1);
    answer(Answer::Opened { program: Some("Mail".into()) });
    h.run_steps(3);

    assert!(h.state_mut().execute("file.send_email"));
    h.run_steps(2);
    assert_eq!(h.state().dialog, Some(Dialog::SendEmailMarks), "each send asks");
    let _ = std::fs::remove_dir_all(sent()[0].path.parent().unwrap());
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

/// A document with a typed-in field, on screen whole, opened from its own file.
fn form_harness(test: &str, pdf: &[u8]) -> (Harness<'static, PdfCraftApp>, PathBuf) {
    let dir = std::env::temp_dir().join(format!("pdfcraft-send-email-test-{}-{test}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join(format!("{test}.pdf"));
    std::fs::write(&path, pdf).unwrap();
    let (p, bytes) = (path.to_string_lossy().into_owned(), pdf.to_vec());
    let mut h = Harness::builder().with_size(egui::vec2(1400.0, 900.0)).build_eframe(move |_cc| {
        let mut app = PdfCraftApp::new();
        app.set_option("language", "en").unwrap();
        app.open_bytes(&format!("{}.pdf", Path::new(&p).file_stem().unwrap().to_string_lossy()), Some(p.clone()), bytes.clone()).unwrap();
        app.set_option("left", "closed").unwrap();
        app.set_option("zoom", "150").unwrap();
        app.send_email.sender = stand_in;
        app.send_email.reveal = reveal;
        app
    });
    crate::support::settle(&mut h);
    (h, path)
}

fn click_field(h: &mut Harness<'static, PdfCraftApp>, name: &str) {
    let p = {
        let s = h.state();
        let doc = s.session.get(s.views[0].id).unwrap();
        let f = doc.form.iter().find(|f| f.name == name).unwrap();
        pdfcraft_ui_egui::forms_ui::field_screen_rect(&s.views[0], &doc.info, f, 0).expect("on screen").center()
    };
    h.hover_at(p);
    h.run_steps(1);
    h.drag_at(p);
    h.run_steps(1);
    h.drop_at(p);
    h.run_steps(3);
}

/// What is typed into a field and not yet entered goes with the document, as it does for Save.
#[test]
fn text_still_being_typed_into_a_field_is_in_the_attachment() {
    let (mut h, path) = form_harness("typing", PDF);
    click_field(&mut h, "name");
    assert!(h.state().views[0].forms.focus.is_some(), "the field's editor is open");
    h.event(egui::Event::Text("Ada Lovelace".into()));
    h.run_steps(2);

    assert!(h.state_mut().execute("file.send_email"));
    h.run_steps(2);
    let asked = sent();
    assert_eq!(asked.len(), 1);
    assert_ne!(asked[0].path, path, "a copy: the typing made the document differ from its file");
    assert!(std::fs::read(&asked[0].path).unwrap().windows(12).any(|w| w == b"Ada Lovelace"), "the typed text is in the copy");
    assert_eq!(std::fs::read(&path).unwrap(), PDF, "the open file is unchanged");
    answer(Answer::Opened { program: None });
    h.run_steps(3);
    let _ = std::fs::remove_dir_all(asked[0].path.parent().unwrap());
    let _ = std::fs::remove_dir_all(path.parent().unwrap());
}

/// A value the field's script refuses is not sent: the field stays open and nothing is asked.
#[test]
fn a_refused_field_value_sends_nothing() {
    let (mut h, path) = form_harness("refused", &validated_form());
    click_field(&mut h, "qty");
    h.event(egui::Event::Text("500".into()));
    h.run_steps(2);

    assert!(h.state_mut().execute("file.send_email"));
    h.run_steps(3);
    assert!(sent().is_empty(), "the stand-in was never asked");
    assert!(!h.state().send_email.waiting());
    let focus = h.state().views[0].forms.focus.clone().expect("the refused field stays open");
    assert_eq!(focus.name, "qty");
    let _ = std::fs::remove_dir_all(path.parent().unwrap());
}

/// A field with a script that refuses a quantity over 100.
fn validated_form() -> Vec<u8> {
    let objs = [
        "<< /Type /Catalog /Pages 2 0 R /AcroForm << /Fields [4 0 R] /DA (/Helv 0 Tf 0 g) /DR << /Font << /Helv 5 0 R >> >> >> >>",
        "<< /Type /Pages /Kids [3 0 R] /Count 1 /MediaBox [0 0 300 400] >>",
        "<< /Type /Page /Parent 2 0 R /Annots [4 0 R] >>",
        "<< /Type /Annot /Subtype /Widget /FT /Tx /T (qty) /V (4) /Rect [20 350 280 372] /P 3 0 R /F 4 /AA << /V << /S /JavaScript /JS (if (event.value > 100) { app.alert('At most 100'); event.rc = false; }) >> >> >>",
        "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica /Encoding /WinAnsiEncoding >>",
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

/// A failure that arrives while the "save changes?" question is open waits behind it (the
/// question is not a dialog, but nothing may cover it), and the "still waiting" notice stays up
/// meanwhile.
#[test]
fn an_answer_waits_behind_the_save_question_and_the_waiting_notice_stays() {
    let (mut h, path) = harness("save-question");
    h.state_mut().apply_edit(Edit::RotatePages { pages: vec![0], degrees: 90 });
    assert!(h.state_mut().execute("file.send_email"));
    h.run_steps(2);
    assert!(h.state().send_email.waiting());
    let asked = sent();

    // Closing the edited document asks whether to save it.
    assert!(h.state_mut().execute("file.close"));
    h.run_steps(3);
    assert!(h.state().close_request.is_some(), "the save question is open");

    // A second request while waiting says so, and that notice is kept up, not left to fade.
    assert!(h.state_mut().execute("file.send_email"));
    h.run_steps(2);
    assert_eq!(h.state().toast.as_ref().map(|t| t.0.as_str()), Some("Still waiting for your email app…"));
    if let Some(toast) = h.state_mut().toast.as_mut() {
        toast.1 = 100.0;
    }
    h.run_steps(2);
    assert!(h.state().toast.as_ref().is_some_and(|t| t.1 < 50.0), "the waiting notice is kept up, not left to fade");

    answer(Answer::NotAttached { program: None, why: Why::NoAnswer, fallback: mail::fallback(&asked[0]) });
    h.run_steps(3);
    assert!(h.state().close_request.is_some(), "the question is still there");
    assert!(h.state().dialog.is_none() && h.state().send_email.problem.is_none(), "the dialog waits");
    assert!(h.state().send_email.waiting());

    // Once the question is answered the dialog comes.
    h.state_mut().close_request = None;
    h.run_steps(3);
    assert_eq!(h.state().dialog, Some(Dialog::SendEmail));
    assert!(h.state().send_email.problem.is_some());
    let _ = std::fs::remove_dir_all(asked[0].path.parent().unwrap());
    let _ = std::fs::remove_dir_all(path.parent().unwrap());
}

/// A copy made for the email app that was never handed to it (the request itself refused) is
/// removed; one the app was asked to attach stays.
#[test]
fn a_copy_that_was_never_handed_over_is_removed() {
    fn refuse(compose: Compose, _wake: Box<dyn FnOnce() + Send>) -> Result<Receiver<Answer>, MailError> {
        SENT.with(|s| s.borrow_mut().push(compose));
        Err(MailError::CouldNotStart("refused".into()))
    }
    let (mut h, path) = harness("refused-copy");
    h.state_mut().apply_edit(Edit::RotatePages { pages: vec![0], degrees: 90 });
    h.state_mut().send_email.sender = refuse;
    assert!(h.state_mut().execute("file.send_email"));
    h.run_steps(2);
    let asked = sent();
    assert_eq!(asked.len(), 1);
    assert!(!asked[0].path.exists(), "the copy is gone");
    assert!(!asked[0].path.parent().unwrap().exists(), "and its folder");
    assert!(path.exists(), "the open file is never touched");
    let _ = std::fs::remove_dir_all(path.parent().unwrap());
}
