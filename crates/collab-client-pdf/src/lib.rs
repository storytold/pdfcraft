//! Client-side projection of collaboration operations onto a PDF document.
//!
//! The room broadcasts the accepted operation (`Applied.op`); every replica
//! turns it into an engine [`Edit`] and applies it with
//! [`pdfcraft_engine::Session::apply_remote`], so no replica invents its own
//! address for a comment or a page. The same function serves a local optimistic
//! edit: the operation a client queues is already the operation it will apply.
//!
//! Annotation addresses are `/NM` (ADR-0002). The client both proposes a name
//! for what it creates ([`new_annotation_name`]) and resolves names coming from
//! other clients against its own document.

#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

use pdfcraft_collab_annot::{AnnotationInput, WireError};
use pdfcraft_collab_proto::{ConflictClass, DocKey, Op, OpId, OpKind, PageRef, Target};
use pdfcraft_engine::{DocId, Document, Edit, FieldValue, FormFieldKind, NoteIcon, Session, Shape};
use serde_json::{Value, json};
use std::sync::atomic::{AtomicU64, Ordering};

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ProjectionError {
    #[error("this operation is not supported by the PDF client")]
    Unsupported(String),
    #[error("the operation is missing {0}")]
    Missing(&'static str),
    #[error("the comment named {0} is not on page {1}")]
    AnnotationGone(String, usize),
    #[error("several comments on page {0} share the name {1}")]
    AmbiguousName(usize, String),
    #[error("the document is gone")]
    NoDocument,
    #[error("the engine refused the edit: {0}")]
    Edit(String),
    #[error("the operation could not be named: {0}")]
    Invalid(String),
}

impl From<WireError> for ProjectionError {
    fn from(error: WireError) -> Self {
        match error {
            WireError::UnsupportedShape(_) | WireError::TooMany { .. } => Self::Unsupported(error.to_string()),
            WireError::Malformed(_) | WireError::Invalid { .. } => Self::Invalid(error.to_string()),
        }
    }
}

/// A client-chosen `/NM` in the ADR-0002 shape: the client id, a millisecond
/// stamp and a counter. Unique per client and session without a network round
/// trip; the server rejects a duplicate among all replicas.
pub fn new_annotation_name(client_id: &str) -> String {
    static NEXT: AtomicU64 = AtomicU64::new(1);
    let clean: String = client_id.chars().map(|c| if c.is_ascii_alphanumeric() || c == '-' || c == '_' { c } else { '-' }).collect();
    let ms = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis()).unwrap_or(0);
    let count = NEXT.fetch_add(1, Ordering::Relaxed);
    format!("{clean}-{ms:x}-{count:x}")
}

/// A merge operation (comments, fields) addressed to the current epoch.
pub fn merge_op(doc: DocKey, op_id: OpId, base_version: u64, epoch: u64, target: Target, payload: Value) -> Op {
    Op { op_id, doc, base_version, epoch, kind: OpKind::Primitive, class: ConflictClass::Merge, target, payload, preview: None }
}

/// A structural operation (pages). The server serialises these and advances the
/// epoch, so it must be built on the head version.
pub fn structural_op(doc: DocKey, op_id: OpId, base_version: u64, epoch: u64, target: Target, payload: Value) -> Op {
    Op { op_id, doc, base_version, epoch, kind: OpKind::Structural, class: ConflictClass::Structural, target, payload, preview: None }
}

/// The typed body of an `annotation.add`: the shape, its style and the text,
/// addressed to `nm`. The server validates the same JSON (ADR-0002).
pub fn add_annotation_payload(new: &pdfcraft_engine::NewAnnotation, nm: &str) -> Result<Value, ProjectionError> {
    AnnotationInput::try_from(new)?.to_payload(nm).map_err(ProjectionError::from)
}

/// A sticky note, kept as a named helper: the most common annotation.
pub fn add_note_payload(nm: &str, at: [f64; 2], contents: &str, author: &str) -> Value {
    let mut input = AnnotationInput::new(Shape::Note { at, icon: NoteIcon::Comment });
    input.contents = contents.to_owned();
    input.author = author.to_owned();
    input.to_payload(nm).unwrap_or_else(|_| {
        // The note shape is always encodable; keep a payload the server can
        // still read if the wire form ever rejects it.
        json!({"action": "annotation.add", "nm": nm, "at": at, "contents": contents, "author": author})
    })
}

pub fn set_contents_payload(text: &str) -> Value {
    json!({"action": "annotation.set_contents", "text": text})
}

pub fn reply_payload(nm: &str, text: &str, author: &str) -> Value {
    json!({"action": "annotation.reply", "nm": nm, "text": text, "author": author})
}

pub fn delete_payload() -> Value {
    json!({"action": "annotation.delete"})
}

pub fn field_value_payload(value: &str) -> Value {
    json!({"action": "field.set_value", "value": value})
}

pub fn rotate_payload(degrees: i64) -> Value {
    json!({"action": "page.rotate", "degrees": degrees})
}

pub fn move_payload(to: usize) -> Value {
    json!({"action": "page.move", "to": to})
}

pub fn delete_page_payload() -> Value {
    json!({"action": "page.delete"})
}

/// The engine edit for an accepted operation. Names are resolved against
/// `document`; a comment that vanished is an error, never a silent skip.
pub fn edit_for(document: &Document, op: &Op) -> Result<Edit, ProjectionError> {
    let action = op.payload.get("action").and_then(Value::as_str).ok_or(ProjectionError::Missing("action"))?;
    match (action, op.class) {
        ("annotation.add", ConflictClass::Merge) => {
            let page = page_target(op)?;
            let nm = text(op, "nm")?.to_owned();
            let input = AnnotationInput::from_payload(&op.payload)?;
            Ok(Edit::AddAnnotationNamed(input.to_engine(page.index as usize), nm))
        }
        ("annotation.set_contents", ConflictClass::Merge) => {
            let (page, name) = annotation_target(op)?;
            let index = annotation_index(document, page.index as usize, &name)?;
            Ok(Edit::SetAnnotationContents { page: page.index as usize, index, text: text(op, "text")?.to_owned() })
        }
        ("annotation.reply", ConflictClass::Merge) => {
            let (page, name) = annotation_target(op)?;
            let index = annotation_index(document, page.index as usize, &name)?;
            Ok(Edit::ReplyToAnnotationNamed {
                page: page.index as usize,
                index,
                text: text(op, "text")?.to_owned(),
                author: op.payload.get("author").and_then(Value::as_str).unwrap_or_default().to_owned(),
                name: text(op, "nm")?.to_owned(),
            })
        }
        ("annotation.delete", ConflictClass::Merge) => {
            let (page, name) = annotation_target(op)?;
            let index = annotation_index(document, page.index as usize, &name)?;
            Ok(Edit::DeleteAnnotation { page: page.index as usize, index })
        }
        ("field.set_value", ConflictClass::Merge) => {
            let Target::Field { name } = &op.target else {
                return Err(ProjectionError::Missing("field target"));
            };
            let kind = document.form.iter().find(|f| f.name == *name).map(|f| f.kind).ok_or(ProjectionError::Missing("form field"))?;
            let value = op.payload.get("value").and_then(Value::as_str).unwrap_or_default();
            Ok(Edit::SetFieldValue { name: name.clone(), value: field_value(kind, value) })
        }
        ("page.rotate", ConflictClass::Structural) => {
            let page = page_target(op)?;
            let degrees = op.payload.get("degrees").and_then(Value::as_i64).ok_or(ProjectionError::Missing("degrees"))?;
            Ok(Edit::RotatePages { pages: vec![page.index as usize], degrees })
        }
        ("page.move", ConflictClass::Structural) => {
            let page = page_target(op)?;
            let to = op.payload.get("to").and_then(Value::as_u64).and_then(|to| usize::try_from(to).ok()).ok_or(ProjectionError::Missing("to"))?;
            Ok(Edit::MovePages { pages: vec![page.index as usize], to })
        }
        ("page.delete", ConflictClass::Structural) => {
            let page = page_target(op)?;
            Ok(Edit::DeletePages { pages: vec![page.index as usize] })
        }
        (other, _) => Err(ProjectionError::Unsupported(other.to_owned())),
    }
}

/// Apply an accepted operation to the local document without touching the
/// local snapshot undo history. Returns the edit, for callers that also want to
/// drive the interface (selection, panel reveal).
pub fn apply(session: &mut Session, doc: DocId, op: &Op) -> Result<Edit, ProjectionError> {
    let edit = {
        let document = session.get(doc).ok_or(ProjectionError::NoDocument)?;
        edit_for(document, op)?
    };
    session.apply_remote(doc, edit.clone()).map_err(|e| ProjectionError::Edit(e.to_string()))?;
    Ok(edit)
}

fn page_target(op: &Op) -> Result<PageRef, ProjectionError> {
    match &op.target {
        Target::Page(page) => Ok(*page),
        _ => Err(ProjectionError::Missing("page target")),
    }
}

fn annotation_target(op: &Op) -> Result<(PageRef, String), ProjectionError> {
    match &op.target {
        Target::Annotation { page, nm } => Ok((*page, nm.clone())),
        _ => Err(ProjectionError::Missing("annotation target")),
    }
}

fn text<'a>(op: &'a Op, field: &'static str) -> Result<&'a str, ProjectionError> {
    op.payload.get(field).and_then(Value::as_str).ok_or(ProjectionError::Missing(field))
}

fn annotation_index(document: &Document, page: usize, name: &str) -> Result<usize, ProjectionError> {
    let mut named = document.info.annotations.iter().filter(|a| a.page == page && a.name.as_deref() == Some(name));
    let first = named.next().ok_or_else(|| ProjectionError::AnnotationGone(name.to_owned(), page))?;
    if named.next().is_some() {
        return Err(ProjectionError::AmbiguousName(page, name.to_owned()));
    }
    Ok(first.index)
}

fn field_value(kind: FormFieldKind, text: &str) -> FieldValue {
    match kind {
        FormFieldKind::CheckBox => FieldValue::Check(matches!(text, "1" | "true" | "on" | "yes")),
        FormFieldKind::Radio => FieldValue::Radio((!text.is_empty()).then(|| text.to_owned())),
        FormFieldKind::Combo | FormFieldKind::List => FieldValue::Choice(vec![text.to_owned()]),
        _ => FieldValue::Text(text.to_owned()),
    }
}

// ── Edit → operation (the interface's side of the wire) ─────────────────────────────────────

/// Turn a user edit into the operations the server accepts. The caller sends
/// them; the document changes only when `applied` comes back, so no optimistic
/// state has to be reconciled.
pub fn ops_for_edit(
    document: &Document,
    edit: &Edit,
    doc: DocKey,
    client_id: &str,
    base_version: u64,
    epoch: u64,
    counter: &mut u64,
) -> Result<Vec<Op>, ProjectionError> {
    match edit {
        Edit::AddAnnotation(new) => {
            let nm = new_annotation_name(client_id);
            let target = Target::Page(PageRef { index: new.page as u32, epoch });
            let payload = add_annotation_payload(new, &nm)?;
            Ok(vec![merge_op(doc, next_id(client_id, counter)?, base_version, epoch, target, payload)])
        }
        Edit::SetAnnotationContents { page, index, text } => {
            let nm = name_at(document, *page, *index)?;
            Ok(vec![merge_op(
                doc,
                next_id(client_id, counter)?,
                base_version,
                epoch,
                Target::Annotation { page: PageRef { index: *page as u32, epoch }, nm },
                set_contents_payload(text),
            )])
        }
        Edit::ReplyToAnnotation { page, index, text, author } => {
            let root = name_at(document, *page, *index)?;
            let nm = new_annotation_name(client_id);
            Ok(vec![merge_op(
                doc,
                next_id(client_id, counter)?,
                base_version,
                epoch,
                Target::Annotation { page: PageRef { index: *page as u32, epoch }, nm: root },
                reply_payload(&nm, text, author),
            )])
        }
        Edit::DeleteAnnotation { page, index } => {
            let nm = name_at(document, *page, *index)?;
            Ok(vec![merge_op(
                doc,
                next_id(client_id, counter)?,
                base_version,
                epoch,
                Target::Annotation { page: PageRef { index: *page as u32, epoch }, nm },
                delete_payload(),
            )])
        }
        Edit::SetFieldValue { name, value } => {
            let target = Target::Field { name: name.clone() };
            Ok(vec![merge_op(doc, next_id(client_id, counter)?, base_version, epoch, target, field_value_payload(&field_text(value)))])
        }
        Edit::RotatePages { pages, degrees } => pages
            .iter()
            .map(|page| {
                Ok(structural_op(
                    doc.clone(),
                    next_id(client_id, counter)?,
                    base_version,
                    epoch,
                    Target::Page(PageRef { index: *page as u32, epoch }),
                    rotate_payload(*degrees),
                ))
            })
            .collect::<Result<Vec<_>, ProjectionError>>(),
        Edit::MovePages { pages, to } if pages.len() == 1 => Ok(vec![structural_op(
            doc,
            next_id(client_id, counter)?,
            base_version,
            epoch,
            Target::Page(PageRef { index: pages[0] as u32, epoch }),
            move_payload(*to),
        )]),
        Edit::DeletePages { pages } => {
            let mut sorted = pages.clone();
            sorted.sort_unstable();
            sorted.dedup();
            // Delete from the end so the earlier indices stay valid.
            sorted
                .into_iter()
                .rev()
                .map(|page| {
                    Ok(structural_op(
                        doc.clone(),
                        next_id(client_id, counter)?,
                        base_version,
                        epoch,
                        Target::Page(PageRef { index: page as u32, epoch }),
                        delete_page_payload(),
                    ))
                })
                .collect::<Result<Vec<_>, ProjectionError>>()
        }
        other => Err(ProjectionError::Unsupported(other.label().to_string())),
    }
}

/// The operation that undoes `op`, built from the document as it was *before*
/// applying it. `None` where the change cannot be reverted: a deletion, a page
/// removal or a form reset. Undo is a normal operation (ADR-0010), so a
/// conflict with someone else's later edit is reported, not forced.
pub fn inverse_for(document: &Document, op: &Op, op_id: OpId) -> Result<Option<Op>, ProjectionError> {
    let action = op.payload.get("action").and_then(Value::as_str).ok_or(ProjectionError::Missing("action"))?;
    let base = op.base_version;
    let epoch = op.epoch;
    let doc = op.doc.clone();
    let inverse = match action {
        "annotation.add" => {
            let nm = text(op, "nm")?.to_owned();
            let page = page_target(op)?;
            merge_op(doc, op_id, base, epoch, Target::Annotation { page, nm }, delete_payload())
        }
        "annotation.reply" => {
            let nm = text(op, "nm")?.to_owned();
            let page = page_target(op)?;
            merge_op(doc, op_id, base, epoch, Target::Annotation { page, nm }, delete_payload())
        }
        "annotation.set_contents" => {
            let (page, nm) = annotation_target(op)?;
            let previous = document
                .info
                .annotations
                .iter()
                .find(|a| a.page == page.index as usize && a.name.as_deref() == Some(nm.as_str()))
                .map(|a| a.contents.clone().unwrap_or_default())
                .ok_or_else(|| ProjectionError::AnnotationGone(nm.clone(), page.index as usize))?;
            merge_op(doc, op_id, base, epoch, Target::Annotation { page, nm }, set_contents_payload(&previous))
        }
        "annotation.delete" => return Ok(None),
        "field.set_value" => {
            let Target::Field { name } = &op.target else { return Err(ProjectionError::Missing("field target")) };
            let field = document.form.iter().find(|f| f.name == *name).ok_or(ProjectionError::Missing("form field"))?;
            let previous = field.value.first().cloned().unwrap_or_default();
            let previous = if matches!(field.kind, FormFieldKind::CheckBox) {
                if previous.is_empty() { "false".to_owned() } else { "true".to_owned() }
            } else {
                previous
            };
            merge_op(doc, op_id, base, epoch, Target::Field { name: name.clone() }, field_value_payload(&previous))
        }
        "page.rotate" => {
            let page = page_target(op)?;
            let previous = document.info.pages.get(page.index as usize).map(|p| i64::from(p.rotation)).ok_or(ProjectionError::Missing("page"))?;
            structural_op(doc, op_id, base, epoch, Target::Page(page), rotate_payload(previous))
        }
        "page.move" => {
            let page = page_target(op)?;
            let to = op.payload.get("to").and_then(Value::as_u64).and_then(|to| usize::try_from(to).ok()).ok_or(ProjectionError::Missing("to"))?;
            structural_op(doc, op_id, base, epoch, Target::Page(PageRef { index: to as u32, epoch }), move_payload(page.index as usize))
        }
        "page.delete" => return Ok(None),
        other => return Err(ProjectionError::Unsupported(other.to_owned())),
    };
    Ok(Some(inverse))
}

fn next_id(client_id: &str, counter: &mut u64) -> Result<OpId, ProjectionError> {
    *counter = counter.saturating_add(1);
    OpId::new(format!("{client_id}-{counter}")).map_err(|e| ProjectionError::Invalid(e.to_string()))
}

fn name_at(document: &Document, page: usize, index: usize) -> Result<String, ProjectionError> {
    document
        .info
        .annotations
        .iter()
        .find(|a| a.page == page && a.index == index)
        .and_then(|a| a.name.clone())
        .ok_or(ProjectionError::Missing("annotation /NM"))
}

fn field_text(value: &FieldValue) -> String {
    match value {
        FieldValue::Text(text) => text.clone(),
        FieldValue::Check(true) => "true".into(),
        FieldValue::Check(false) => "false".into(),
        FieldValue::Radio(Some(state)) => state.clone(),
        FieldValue::Radio(None) => String::new(),
        FieldValue::Choice(values) => values.first().cloned().unwrap_or_default(),
    }
}

#[cfg(test)]
mod tests {
    use super::{
        ProjectionError, add_note_payload, apply, edit_for, merge_op, new_annotation_name, reply_payload, rotate_payload, set_contents_payload,
    };
    use pdfcraft_collab_proto::{DocKey, OpId, PageRef, Target};
    use pdfcraft_engine::Session;

    fn fixture(n: usize) -> Vec<u8> {
        let mut objs: Vec<Vec<u8>> = vec![b"<< /Type /Catalog /Pages 2 0 R >>".to_vec()];
        let kids: Vec<String> = (0..n).map(|i| format!("{} 0 R", 4 + 2 * i)).collect();
        objs.push(format!("<< /Type /Pages /Kids [{}] /Count {n} /MediaBox [0 0 200 300] >>", kids.join(" ")).into_bytes());
        objs.push(b"<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>".to_vec());
        for i in 0..n {
            objs.push(format!("<< /Type /Page /Parent 2 0 R /Contents {} 0 R /Resources << /Font << /F1 3 0 R >> >> >>", 5 + 2 * i).into_bytes());
            let body = format!("BT /F1 24 Tf 20 150 Td (Page {}) Tj ET", i + 1);
            objs.push(format!("<< /Length {} >>\nstream\n{body}\nendstream", body.len()).into_bytes());
        }
        let mut out = b"%PDF-1.7\n".to_vec();
        let mut offsets = Vec::new();
        for (i, o) in objs.iter().enumerate() {
            offsets.push(out.len());
            out.extend_from_slice(format!("{} 0 obj\n", i + 1).as_bytes());
            out.extend_from_slice(o);
            out.extend_from_slice(b"\nendobj\n");
        }
        let xref = out.len();
        out.extend_from_slice(format!("xref\n0 {}\n0000000000 65535 f \n", objs.len() + 1).as_bytes());
        for o in offsets {
            out.extend_from_slice(format!("{o:010} 00000 n \n").as_bytes());
        }
        out.extend_from_slice(format!("trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n", objs.len() + 1).as_bytes());
        out
    }

    fn open() -> Result<(Session, pdfcraft_engine::DocId), Box<dyn std::error::Error>> {
        let mut session = Session::new();
        let id = session.open_new("fixture.pdf", fixture(2).into())?;
        Ok((session, id))
    }

    fn doc() -> Result<DocKey, Box<dyn std::error::Error>> {
        Ok(DocKey::new("file-1")?)
    }

    #[test]
    fn a_remote_comment_is_applied_with_the_server_name() -> Result<(), Box<dyn std::error::Error>> {
        let (mut session, id) = open()?;
        let op = merge_op(
            doc()?,
            OpId::new("a1")?,
            0,
            0,
            Target::Page(PageRef { index: 1, epoch: 0 }),
            add_note_payload("other-client-1", [40.0, 250.0], "hello", "Ada"),
        );
        let edit = apply(&mut session, id, &op)?;
        assert!(matches!(edit, pdfcraft_engine::Edit::AddAnnotationNamed(_, _)));
        let document = session.get(id).ok_or("gone")?;
        let annotation = document.info.annotations.iter().find(|a| a.name.as_deref() == Some("other-client-1")).ok_or("the comment is missing")?;
        assert_eq!(annotation.page, 1);
        assert_eq!(annotation.contents.as_deref(), Some("hello"));
        assert!(session.undo(id).is_err(), "a collaborative edit has no local snapshot undo");
        Ok(())
    }

    #[test]
    fn editing_replying_and_deleting_resolve_the_name() -> Result<(), Box<dyn std::error::Error>> {
        let (mut session, id) = open()?;
        let page = Target::Page(PageRef { index: 0, epoch: 0 });
        apply(&mut session, id, &merge_op(doc()?, OpId::new("a1")?, 0, 0, page, add_note_payload("root", [40.0, 250.0], "hello", "Ada")))?;
        apply(
            &mut session,
            id,
            &merge_op(
                doc()?,
                OpId::new("s1")?,
                1,
                0,
                Target::Annotation { page: PageRef { index: 0, epoch: 0 }, nm: "root".into() },
                set_contents_payload("edited"),
            ),
        )?;
        apply(
            &mut session,
            id,
            &merge_op(
                doc()?,
                OpId::new("r1")?,
                2,
                0,
                Target::Annotation { page: PageRef { index: 0, epoch: 0 }, nm: "root".into() },
                reply_payload("reply-1", "agreed", "Bo"),
            ),
        )?;
        let document = session.get(id).ok_or("gone")?;
        let root = document.info.annotations.iter().find(|a| a.name.as_deref() == Some("root")).ok_or("no root")?;
        assert_eq!(root.contents.as_deref(), Some("edited"));
        let reply = document.info.annotations.iter().find(|a| a.name.as_deref() == Some("reply-1")).ok_or("no reply")?;
        assert_eq!(reply.contents.as_deref(), Some("agreed"));

        apply(
            &mut session,
            id,
            &merge_op(
                doc()?,
                OpId::new("d1")?,
                3,
                0,
                Target::Annotation { page: PageRef { index: 0, epoch: 0 }, nm: "root".into() },
                super::delete_payload(),
            ),
        )?;
        let document = session.get(id).ok_or("gone")?;
        assert!(document.info.annotations.iter().all(|a| a.name.as_deref() != Some("root")), "the thread is gone");
        assert!(document.info.annotations.iter().all(|a| a.name.as_deref() != Some("reply-1")), "replies go with it");
        Ok(())
    }

    #[test]
    fn a_structural_operation_rotates_the_page() -> Result<(), Box<dyn std::error::Error>> {
        let (mut session, id) = open()?;
        let op = super::structural_op(doc()?, OpId::new("rot")?, 0, 0, Target::Page(PageRef { index: 1, epoch: 0 }), rotate_payload(90));
        apply(&mut session, id, &op)?;
        assert_eq!(session.get(id).ok_or("gone")?.info.pages[1].rotation, 90);
        Ok(())
    }

    #[test]
    fn a_missing_comment_is_an_error_not_a_skip() -> Result<(), Box<dyn std::error::Error>> {
        let (session, id) = open()?;
        let op = merge_op(
            doc()?,
            OpId::new("s1")?,
            0,
            0,
            Target::Annotation { page: PageRef { index: 0, epoch: 0 }, nm: "gone".into() },
            set_contents_payload("x"),
        );
        let document = session.get(id).ok_or("gone")?;
        assert!(matches!(edit_for(document, &op), Err(ProjectionError::AnnotationGone(_, _))));
        Ok(())
    }

    #[test]
    fn an_unknown_action_is_refused() -> Result<(), Box<dyn std::error::Error>> {
        let (session, id) = open()?;
        let op = merge_op(doc()?, OpId::new("x")?, 0, 0, Target::Page(PageRef { index: 0, epoch: 0 }), serde_json::json!({"action": "rm -rf"}));
        let document = session.get(id).ok_or("gone")?;
        assert!(matches!(edit_for(document, &op), Err(ProjectionError::Unsupported(_))));
        Ok(())
    }

    #[test]
    fn an_edit_becomes_an_operation_with_its_inverse() -> Result<(), Box<dyn std::error::Error>> {
        let (mut session, id) = open()?;
        apply(
            &mut session,
            id,
            &merge_op(
                doc()?,
                OpId::new("a1")?,
                0,
                0,
                Target::Page(PageRef { index: 0, epoch: 0 }),
                add_note_payload("root", [40.0, 250.0], "hello", "Ada"),
            ),
        )?;
        let document = session.get(id).ok_or("gone")?;
        let mut counter = 0;
        let ops = super::ops_for_edit(
            document,
            &pdfcraft_engine::Edit::SetAnnotationContents { page: 0, index: 0, text: "edited".into() },
            doc()?,
            "client",
            1,
            0,
            &mut counter,
        )?;
        assert_eq!(ops.len(), 1);
        assert_eq!(ops[0].payload["text"], "edited");
        assert_eq!(ops[0].target, Target::Annotation { page: PageRef { index: 0, epoch: 0 }, nm: "root".into() });
        let inverse = super::inverse_for(document, &ops[0], OpId::new("inv-1")?)?;
        let inverse = inverse.ok_or("a text edit can be undone")?;
        assert_eq!(inverse.payload["text"], "hello");

        let rotate = pdfcraft_engine::Edit::RotatePages { pages: vec![0], degrees: 90 };
        let ops = super::ops_for_edit(document, &rotate, doc()?, "client", 1, 0, &mut counter)?;
        let inverse = super::inverse_for(document, &ops[0], OpId::new("inv-2")?)?;
        assert_eq!(inverse.ok_or("a rotation can be undone")?.payload["degrees"], 0);
        Ok(())
    }

    #[test]
    fn client_names_are_prefixed_and_unique() {
        let first = new_annotation_name("client 7");
        let second = new_annotation_name("client 7");
        assert!(first.starts_with("client-7-"));
        assert_ne!(first, second);
        assert!(first.len() <= pdfcraft_collab_proto::MAX_ANNOTATION_NAME);
    }

    /// engine → operation → engine: every shared shape survives the wire with
    /// its subtype, geometry, colour, opacity and text. The second document is
    /// built from the same bytes, so this is the path two replicas take.
    #[test]
    fn every_shared_shape_round_trips_between_documents() -> Result<(), Box<dyn std::error::Error>> {
        use pdfcraft_engine as engine;

        let quads = vec![[72.0f64, 700.0, 220.0, 700.0, 72.0, 686.0, 220.0, 686.0], [72.0, 680.0, 220.0, 680.0, 72.0, 668.0, 220.0, 668.0]];
        let shapes = vec![
            engine::Shape::Note { at: [40.0, 250.0], icon: engine::NoteIcon::Comment },
            engine::Shape::TextMarkup { kind: engine::Markup::Highlight, quads: quads.clone() },
            engine::Shape::TextMarkup { kind: engine::Markup::Squiggly, quads: vec![quads[0]] },
            engine::Shape::Rectangle { rect: [30.0, 30.0, 120.0, 90.0] },
            engine::Shape::Oval { rect: [30.0, 30.0, 120.0, 90.0] },
            engine::Shape::Line { from: [20.0, 20.0], to: [150.0, 100.0], start: engine::LineEnding::None, end: engine::LineEnding::OpenArrow },
            engine::Shape::Ink { strokes: vec![vec![[20.0, 20.0], [40.0, 60.0], [80.0, 30.0]]] },
            engine::Shape::FreehandHighlight { strokes: vec![vec![[20.0, 20.0], [90.0, 30.0]]] },
            engine::Shape::TextBox { rect: [20.0, 30.0, 180.0, 90.0], font_size: 12.0 },
            engine::Shape::Typewriter { rect: [20.0, 120.0, 180.0, 150.0], font_size: 14.0 },
            engine::Shape::Stamp { rect: [40.0, 40.0, 160.0, 100.0], stamp: engine::StampKind::Draft, by: None },
            engine::Shape::Mark { rect: [60.0, 60.0, 90.0, 90.0], mark: engine::FillMark::Check },
        ];

        for shape in shapes {
            let (first, first_id) = open()?;
            let mut counter = 0;
            let annotation = engine::NewAnnotation {
                page: 0,
                style: engine::Style::default_for(&shape),
                shape: shape.clone(),
                contents: "shared text".into(),
                author: "ada".into(),
            };
            let document = first.get(first_id).ok_or("the source document is open")?;
            let ops = super::ops_for_edit(document, &engine::Edit::AddAnnotation(annotation.clone()), doc()?, "ada", 0, 0, &mut counter)?;
            assert_eq!(ops.len(), 1);
            assert!(ops[0].payload["shape"]["kind"].is_string(), "{shape:?} travels as a tagged object");
            assert!(ops[0].payload.get("at").is_none(), "the position lives in the shape, not at the top level");

            let (mut second, second_id) = open()?;
            let remote = {
                let document = second.get(second_id).ok_or("the second document is open")?;
                super::edit_for(document, &ops[0])?
            };
            match &remote {
                engine::Edit::AddAnnotationNamed(parsed, nm) => {
                    assert_eq!(nm, ops[0].payload["nm"].as_str().ok_or("the operation names the annotation")?);
                    assert_eq!(parsed.shape, shape, "the shape survives the wire");
                    assert_eq!(parsed.style, annotation.style, "the style survives the wire");
                    assert_eq!(parsed.contents, "shared text");
                    assert_eq!(parsed.author, "ada");
                }
                other => return Err(format!("expected a named annotation, got {}", other.label()).into()),
            }
            super::apply(&mut second, second_id, &ops[0])?;

            let document = second.get(second_id).ok_or("the target document is open")?;
            let name = ops[0].payload["nm"].as_str().ok_or("the name is in the payload")?;
            let stored = document.info.annotations.iter().find(|a| a.name.as_deref() == Some(name)).ok_or("the annotation is in the document")?;
            assert_eq!(stored.subtype, shape.subtype(), "subtype of {shape:?}");
            assert_eq!(stored.contents.as_deref(), Some("shared text"), "text of {shape:?}");
            assert_eq!(stored.page, 0);
            if let engine::Shape::TextMarkup { quads, .. } = &shape {
                let stored: Vec<[f32; 8]> = stored.quads.clone();
                assert_eq!(stored.len(), quads.len());
                for (got, want) in stored.iter().zip(quads) {
                    for (g, w) in got.iter().zip(want) {
                        assert!((f64::from(*g) - w).abs() < 0.01, "quad {got:?} != {want:?}");
                    }
                }
            }
            if let Some(color) = stored.color {
                for (got, want) in color.iter().zip(annotation.style.color) {
                    assert!((f64::from(*got) - want).abs() < 0.01, "colour {color:?} != {:?}", annotation.style.color);
                }
            }
        }
        Ok(())
    }

    #[test]
    fn an_unshared_shape_is_refused_with_a_reason() -> Result<(), Box<dyn std::error::Error>> {
        let (session, id) = open()?;
        let document = session.get(id).ok_or("gone")?;
        let new = engine_new(&pdfcraft_engine::Shape::Redact {
            quads: vec![[0.0; 8]],
            overlay: String::new(),
            look: pdfcraft_engine::OverlayLook::default(),
        });
        let mut counter = 0;
        let error = super::ops_for_edit(document, &pdfcraft_engine::Edit::AddAnnotation(new), doc()?, "ada", 0, 0, &mut counter);
        assert!(matches!(error, Err(ProjectionError::Unsupported(message)) if message.contains("redaction")));
        Ok(())
    }

    fn engine_new(shape: &pdfcraft_engine::Shape) -> pdfcraft_engine::NewAnnotation {
        pdfcraft_engine::NewAnnotation {
            page: 0,
            style: pdfcraft_engine::Style::default_for(shape),
            shape: shape.clone(),
            contents: String::new(),
            author: "ada".into(),
        }
    }
}
