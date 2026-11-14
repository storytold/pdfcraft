//! A certificate-protected document (`/Adobe.PubSec`) is read through a decrypted copy that stays
//! in memory: what the session hands out to be written, such as a crash-recovery snapshot, is the
//! encrypted working file.
use crate::support;

use std::sync::Arc;

use pdfcraft_engine::{Edit, Session};
use support::certificate::{contains, identity, one_page, open_with_identity, protected};

const EDITED: &str = "Unsaved subject 4F7Q";

#[test]
fn autosave_of_a_certificate_protected_document_stays_ciphertext() {
    let (mut s, id) = open_with_identity(protected(one_page(&[])));
    s.apply(id, Edit::SetInfo { key: "Subject".into(), value: EDITED.into() }).unwrap();
    let snapshots = s.autosave_snapshots();
    let snapshot = snapshots.iter().find(|x| x.doc == id).expect("the edited document has a snapshot");
    assert!(snapshot.encrypted);
    assert!(contains(&snapshot.bytes, b"Adobe.PubSec"), "the snapshot is the encrypted file, not the decrypted copy");
    assert!(!contains(&snapshot.bytes, EDITED.as_bytes()), "the edit is not written in clear");
    // It is the document as edited: the identity opens it, edit included.
    let mut r = Session::new();
    r.add_identity(identity());
    let back = r.open("recovered.pdf", None, snapshot.bytes.clone(), None).unwrap();
    assert_eq!(r.get(back).unwrap().info.subject.as_deref(), Some(EDITED));
}

/// Unchanged for every other document: the snapshot is the working file the session reads.
#[test]
fn autosave_of_an_ordinary_document_is_its_working_file() {
    let mut s = Session::new().with_clock(|| 1_700_000_000);
    let id = s.open("plain.pdf", None, Arc::new(one_page(&[])), None).unwrap();
    s.apply(id, Edit::SetInfo { key: "Subject".into(), value: EDITED.into() }).unwrap();
    let snapshots = s.autosave_snapshots();
    let snapshot = snapshots.iter().find(|x| x.doc == id).expect("the edited document has a snapshot");
    assert!(!snapshot.encrypted);
    assert!(Arc::ptr_eq(&snapshot.bytes, &s.get(id).unwrap().bytes));
}
