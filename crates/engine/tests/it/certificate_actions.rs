//! An action run on a certificate-protected document (`/Adobe.PubSec`) is given the document's
//! file, which stays encrypted: it is refused like any other file nobody's certificate opens,
//! and never yields a decrypted copy to be written out.
use crate::support;

use pdfcraft_engine::Edit;
use pdfcraft_engine::actions::{Action, Step, run_on};
use support::certificate::{contains, one_page, open_with_identity, protected};

#[test]
fn run_on_refuses_a_certificate_documents_file_bytes() {
    let (mut s, id) = open_with_identity(protected(one_page(&[])));
    // Unsaved edits too: the working file is written encrypted.
    s.apply(id, Edit::SetInfo { key: "Subject".into(), value: "Unsaved 9KX2".into() }).unwrap();
    let file = s.get(id).unwrap().file_bytes().unwrap();
    assert!(contains(&file, b"Adobe.PubSec"), "the file handed out is the encrypted one");
    assert!(!contains(&file, b"Unsaved 9KX2"), "the edit is not in clear");
    let action = Action { name: "Title".into(), description: String::new(), steps: vec![Step::SetTitle("Plain".into())], builtin: false };
    match run_on(&action, "secret.pdf", file, |_, _| {}) {
        Ok(r) => panic!("the action ran and produced a {}-byte file", r.bytes.len()),
        Err(e) => assert!(e.contains("certificate"), "refused for want of a certificate: {e}"),
    }
}
