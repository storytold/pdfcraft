//! File ▸ Send by email… with unsaved edits: the copy for the email app goes into a new folder of
//! its own that only its owner can read, created fresh, so a folder or a link someone else put in
//! the shared temp folder is never written through. Unix only (modes and symlinks); the email app
//! is never asked: a stand-in takes the request. Its temporary files are named after the process
//! and the test.
#![cfg(unix)]

use std::cell::RefCell;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, Sender, channel};

use egui_kittest::Harness;
use pdfcraft_platform::mail::{Answer, Compose, MailError};
use pdfcraft_ui_egui::PdfCraftApp;

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

fn sent() -> Vec<Compose> {
    SENT.with(|s| s.borrow().clone())
}

/// A document opened from its own file (named `<test>-<process>.pdf`), with an unsaved edit.
fn edited(test: &str) -> (Harness<'static, PdfCraftApp>, PathBuf) {
    let dir = std::env::temp_dir().join(format!("pdfcraft-send-email-private-test-{}-{test}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join(format!("{test}-{}.pdf", std::process::id()));
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
    h.state_mut().apply_edit(pdfcraft_engine::Edit::RotatePages { pages: vec![0], degrees: 90 });
    let id = h.state().views[0].id;
    assert!(h.state().session.get(id).unwrap().dirty);
    (h, path)
}

/// Sends the document and returns the file the email app was asked to attach; the stand-in then
/// answers that the message opened.
fn send(h: &mut Harness<'static, PdfCraftApp>) -> PathBuf {
    let before = sent().len();
    assert!(h.state_mut().execute("file.send_email"));
    h.run_steps(2);
    let asked = sent();
    assert_eq!(asked.len(), before + 1, "one request per send");
    REPLY.with(|r| r.borrow().as_ref().expect("a request was made").send(Answer::Opened { program: None }).unwrap());
    h.run_steps(3);
    assert!(!h.state().send_email.waiting());
    asked[before].path.clone()
}

fn mode(path: &Path) -> u32 {
    std::fs::symlink_metadata(path).unwrap().permissions().mode() & 0o777
}

/// Each send's copy is in a folder of its own, made for it under the temp folder and readable
/// only by its owner; the copy is a plain file only its owner can read, and a second send of the
/// same document leaves the first copy as it was.
#[test]
fn unsaved_copy_goes_to_a_fresh_owner_only_folder() {
    let (mut h, path) = edited("fresh");
    let shared = std::env::temp_dir().join("pdfcraft-send-email");
    let first = send(&mut h);
    let dir = first.parent().unwrap().to_path_buf();
    assert_ne!(dir, shared, "not the folder every send and every user shares");
    assert_eq!(dir.parent(), Some(std::env::temp_dir().as_path()), "{}", first.display());
    assert!(std::fs::symlink_metadata(&dir).unwrap().is_dir());
    assert_eq!(mode(&dir), 0o700, "only the owner can list or enter the folder");
    let meta = std::fs::symlink_metadata(&first).unwrap();
    assert!(meta.is_file(), "a plain file, not a link");
    assert_eq!(mode(&first), 0o600, "only the owner can read the copy");
    assert_eq!(first.file_name(), path.file_name(), "the copy keeps the document's name");
    let written = std::fs::read(&first).unwrap();
    assert!(written.len() > PDF.len() && written.starts_with(PDF), "the copy is the document with its edits");

    let second = send(&mut h);
    assert_ne!(second.parent(), Some(dir.as_path()), "a second send gets a new folder");
    assert_eq!(mode(second.parent().unwrap()), 0o700);
    assert_eq!(second.file_name(), first.file_name());
    assert_eq!(std::fs::read(&first).unwrap(), written, "the first copy is left for its email message");

    let _ = std::fs::remove_dir_all(&dir);
    let _ = std::fs::remove_dir_all(second.parent().unwrap());
    let _ = std::fs::remove_dir_all(path.parent().unwrap());
}

/// Another user's link, planted where the copy used to go (the shared folder under the temp
/// folder, named after the document) and pointing at one of the victim's files: the send writes
/// its own new file elsewhere and the victim's file is untouched.
#[test]
fn a_planted_symlink_is_not_followed() {
    let (mut h, path) = edited("planted");
    let victim = path.with_file_name("victim.txt");
    std::fs::write(&victim, b"the victim's own file").unwrap();
    let shared = std::env::temp_dir().join("pdfcraft-send-email");
    std::fs::create_dir_all(&shared).unwrap();
    let planted = shared.join(path.file_name().unwrap());
    let _ = std::fs::remove_file(&planted);
    std::os::unix::fs::symlink(&victim, &planted).unwrap();

    let copy = send(&mut h);
    assert_eq!(std::fs::read(&victim).unwrap(), b"the victim's own file", "the link was written through");
    assert_ne!(copy, planted, "the planted link is not the attachment");
    assert!(!copy.starts_with(&shared), "{}", copy.display());
    let meta = std::fs::symlink_metadata(&copy).unwrap();
    assert!(meta.is_file() && !meta.file_type().is_symlink(), "the attachment is a file of its own");
    assert!(std::fs::read(&copy).unwrap().starts_with(PDF));
    assert!(std::fs::symlink_metadata(&planted).unwrap().file_type().is_symlink(), "the planted link is left alone");

    let _ = std::fs::remove_file(&planted);
    let _ = std::fs::remove_dir(&shared);
    let _ = std::fs::remove_dir_all(copy.parent().unwrap());
    let _ = std::fs::remove_dir_all(path.parent().unwrap());
}
