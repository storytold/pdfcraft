//! A certificate-protected document (`/Adobe.PubSec`) takes the same scoped refresh after an
//! edit as any other: its decrypted copy never reads as encrypted, which must not be taken for
//! a change of encryption that needs the full re-inspection.
use crate::support;

use std::sync::Arc;

use pdfcraft_engine::Edit;
use support::certificate::{one_page, open_with_identity, protected};

#[test]
fn metadata_edit_on_certificate_document_keeps_the_view_data() {
    let (mut s, id) = open_with_identity(protected(one_page(&[])));
    let (form, display) = {
        let doc = s.get(id).unwrap();
        (doc.form.clone(), doc.display.clone())
    };
    s.apply(id, Edit::SetInfo { key: "Subject".into(), value: "Scoped".into() }).unwrap();
    let doc = s.get(id).unwrap();
    assert_eq!(doc.info.subject.as_deref(), Some("Scoped"));
    assert!(Arc::ptr_eq(&doc.form, &form), "the form list is kept: a scoped refresh, not a full one");
    assert!(Arc::ptr_eq(&doc.display, &display), "the renderer's copy is kept");
}
