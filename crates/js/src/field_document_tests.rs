use crate::session::{Job, ScriptSession};
use crate::{DocInfo, Event, FieldState, FieldType};
use std::sync::Arc;

// Original synthetic form. Field.doc is the read-only owning document, not a
// snapshot. Source: JavaScript for Acrobat API Reference, Field.doc.
#[test]
fn a_retained_field_reaches_its_live_document_and_other_fields() {
    let job = |script: &str, pages: usize| Job {
        script: script.into(),
        event: Event::field("Mouse Up", "qty", ""),
        doc: DocInfo { num_pages: pages, ..Default::default() },
        fields: Arc::new(vec![FieldState::new("qty", FieldType::Text, vec!["4".into()]), FieldState::new("total", FieldType::Text, vec![])]),
        doc_scripts: vec!["var held = getField('qty');".into()],
        calculations: vec![],
        timer: None,
    };
    let mut session = ScriptSession::default();
    let first = session.run(job("held.doc.numPages;", 1), &[]);
    assert_eq!(first.error, None);
    assert_eq!(first.result.as_deref(), Some("1"));
    let second = session.run(
        job(
            "held.doc = {}; var owner = event.target.doc; \
             owner.getField('total').value = held.value * owner.numPages; \
             JSON.stringify(held); held.doc === this && owner === held.doc;",
            3,
        ),
        &[],
    );
    assert_eq!(second.error, None);
    assert_eq!(second.result.as_deref(), Some("true"));
    assert_eq!(second.changed.iter().find(|field| field.name == "total").map(|field| &field.value), Some(&vec!["12".into()]));
}
