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

use pdfcraft_collab_annot::{
    AnnotationInput, BorderShapeWire, EndingWire, FontSizeUpdate, IconWire, InfoUpdate, ReviewStateWire, StyleUpdate, WireError,
};
use pdfcraft_collab_proto::{Applied, ConflictClass, DocKey, Op, OpId, OpKind, PageRef, Target};
use pdfcraft_engine::{DocId, Document, Edit, FieldConflicts, FieldValue, FormFieldKind, NoteIcon, Session, Shape};
use serde_json::{Value, json};
use std::sync::Arc;
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
    #[error("the page is {0} bytes; one operation can carry at most {max}", max = pdfcraft_collab_proto::MAX_PAGE_COPY_BYTES)]
    TooLarge(usize),
}

impl From<WireError> for ProjectionError {
    fn from(error: WireError) -> Self {
        match error {
            WireError::UnsupportedShape(_) | WireError::NotRestylable(_) | WireError::TooMany { .. } => Self::Unsupported(error.to_string()),
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

/// The comment's text, refusing to apply unless it still is what the undo left.
pub fn set_contents_payload_expecting(text: &str, expect: &str) -> Value {
    json!({"action": "annotation.set_contents", "text": text, "expect": expect})
}

pub fn reply_payload(nm: &str, text: &str, author: &str) -> Value {
    json!({"action": "annotation.reply", "nm": nm, "text": text, "author": author})
}

/// A reply that also sets the comment's review status (`/State`,
/// `/StateModel /Review`). The engine writes the status text itself, so the
/// accepted operation carries it.
pub fn state_reply_payload(nm: &str, state: pdfcraft_engine::ReviewState, author: &str) -> Value {
    let state = ReviewStateWire::from(state);
    json!({"action": "annotation.reply", "nm": nm, "author": author, "state": state.name()})
}

/// The body of an `annotation.update`: the final look of the comment, named by
/// the operation's target.
pub fn style_update_payload(update: &StyleUpdate) -> Value {
    update.to_payload()
}

/// The update that undoes one: the look `style` puts back, applied only if the
/// comment still has the look `expect` names (an undo is a compare-and-set).
pub fn style_restore_payload(style: &StyleUpdate, expect: &StyleUpdate) -> Value {
    json!({"action": "annotation.update", "style": style.to_value(), "expect": expect.to_value()})
}

/// The body of an `annotation.font_size`: the final size of a text box,
/// typewriter text or callout, named by the operation's target.
pub fn font_size_payload(size: f64) -> Value {
    FontSizeUpdate { size }.to_payload()
}

/// The update that undoes a font size: the size it replaced, applied only if the
/// comment still has the one the operation left.
pub fn font_size_restore_payload(size: f64, expect: f64) -> Value {
    json!({"action": "annotation.font_size", "size": size, "expect": expect})
}

/// The body of an `annotation.info`: the author, subject and note icon of an
/// existing comment, named by the operation's target.
pub fn info_payload(update: &InfoUpdate) -> Value {
    update.to_payload()
}

/// The update that undoes one: the properties `info` puts back, applied only if
/// the comment still has the ones `expect` names.
pub fn info_restore_payload(info: &InfoUpdate, expect: &InfoUpdate) -> Value {
    json!({"action": "annotation.info", "info": info.to_value(), "expect": expect.to_value()})
}

/// One style update for a group of interactive style edits on the same comment.
///
/// The interface sends a colour, a width and a border as separate edits in one
/// batch (one undo step); on the wire they are one `annotation.update`, so every
/// replica applies one version and the undo puts the whole look back. `None`
/// when the edits are not all style edits on one comment, or when they carry
/// something the wire does not: the caller refuses them as a whole.
pub fn style_update_for_edits(edits: &[Edit]) -> Option<(usize, usize, StyleUpdate)> {
    let mut target: Option<(usize, usize)> = None;
    let mut update = StyleUpdate::default();
    for edit in edits {
        let (page, index) = match edit {
            Edit::StyleAnnotation { page, index, .. }
            | Edit::FillAnnotation { page, index, .. }
            | Edit::BorderColorAnnotation { page, index, .. }
            | Edit::BorderShapeAnnotation { page, index, .. } => (*page, *index),
            _ => return None,
        };
        if target.get_or_insert((page, index)) != &(page, index) {
            return None;
        }
        match edit {
            Edit::StyleAnnotation { color, opacity, width, endings, .. } => {
                if let Some(color) = color {
                    update.color = Some(*color);
                }
                if let Some(opacity) = opacity {
                    update.opacity = Some(*opacity);
                }
                if let Some(width) = width {
                    update.width = Some(*width);
                }
                // A line's or a callout's endings travel in the same style object,
                // so one choice of the dialog is still one operation.
                if let Some(endings) = endings {
                    update.endings = Some(endings.iter().copied().map(EndingWire::from).collect());
                }
            }
            Edit::FillAnnotation { fill, .. } => update.fill = Some(*fill),
            Edit::BorderColorAnnotation { color, .. } => update.border_color = Some(*color),
            Edit::BorderShapeAnnotation { shape, .. } => update.border_shape = Some(BorderShapeWire::from(*shape)),
            _ => return None,
        }
    }
    let (page, index) = target?;
    (!update.is_empty()).then_some((page, index, update))
}

pub fn delete_payload() -> Value {
    json!({"action": "annotation.delete"})
}

pub fn field_value_payload(value: &str) -> Value {
    json!({"action": "field.set_value", "value": value})
}

/// The field's value, refusing to apply unless it still is what the undo left.
pub fn field_value_payload_expecting(value: &str, expect: &str) -> Value {
    json!({"action": "field.set_value", "value": value, "expect": expect})
}

pub fn rotate_payload(degrees: i64) -> Value {
    json!({"action": "page.rotate", "degrees": degrees})
}

/// The page's rotation, refusing to apply unless the page still has `expect`.
pub fn rotate_payload_expecting(degrees: i64, expect: i64) -> Value {
    json!({"action": "page.rotate", "degrees": degrees, "expect": expect})
}

pub fn move_payload(to: usize) -> Value {
    json!({"action": "page.move", "to": to})
}

pub fn delete_page_payload() -> Value {
    json!({"action": "page.delete"})
}

/// One page inserted at `at` (an index in the page order as it is now, which
/// may be the page count: an append). The copy is a one-page PDF: the page
/// travels whole, so every replica inserts the same content (RFC §6.1).
pub fn insert_page_payload(copy: &[u8], at: usize) -> Value {
    json!({"action": "page.insert", "at": at, "bytes": encode_base64(copy)})
}

/// A copy of `page` inserted at `at` (right after it when the caller says so).
/// The address is the source page; the position is where the copy goes.
pub fn duplicate_page_payload(copy: &[u8], at: usize) -> Value {
    json!({"action": "page.duplicate", "at": at, "bytes": encode_base64(copy)})
}

/// The content of the addressed page replaced by a one-page PDF (RFC §3.1.4).
/// The address is the operation's target page; the copy travels whole.
pub fn replace_page_payload(copy: &[u8]) -> Value {
    json!({"action": "page.replace", "bytes": encode_base64(copy)})
}

/// The undo of a replacement: the page the server kept, named by its copy
/// token. A replacement whose page was too large to keep answers no token, and
/// the undo that names none (`copy: null`) is refused as `conflict` before the
/// document changes.
pub fn replace_page_from_copy_payload(token: Option<&str>) -> Value {
    match token {
        Some(token) => json!({"action": "page.replace", "copy": token}),
        None => json!({"action": "page.replace", "copy": null}),
    }
}

/// A blank page of `width × height` points at `at` (the page count appends).
pub fn blank_page_payload(width: f64, height: f64) -> Value {
    json!({"action": "page.insert_blank", "width": width, "height": height})
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
        ("annotation.update", ConflictClass::Merge) => {
            // The final look of an existing comment; the address is checked
            // against this replica's document before the edit is built.
            let (page, name) = annotation_target(op)?;
            let page = page.index as usize;
            let index = annotation_index(document, page, &name)?;
            let update = StyleUpdate::from_payload(&op.payload)?;
            let props = document.comment_props(page, index).ok_or_else(|| ProjectionError::AnnotationGone(name.clone(), page))?;
            update.check_for(&props)?;
            Ok(Edit::RestyleAnnotation { page, index, style: update.to_restyle() })
        }
        ("annotation.font_size", ConflictClass::Merge) => {
            // The final font size of an existing text box, typewriter text or
            // callout, addressed by its `/NM`.
            let (page, name) = annotation_target(op)?;
            let page = page.index as usize;
            let index = annotation_index(document, page, &name)?;
            let update = FontSizeUpdate::from_payload(&op.payload)?;
            let props = document.comment_props(page, index).ok_or_else(|| ProjectionError::AnnotationGone(name.clone(), page))?;
            update.check_for(&props)?;
            Ok(Edit::FontSizeAnnotation { page, index, size: update.size })
        }
        ("annotation.info", ConflictClass::Merge) => {
            // The final author, subject and note icon of an existing comment.
            let (page, name) = annotation_target(op)?;
            let page = page.index as usize;
            let index = annotation_index(document, page, &name)?;
            let update = InfoUpdate::from_payload(&op.payload)?;
            let props = document.comment_props(page, index).ok_or_else(|| ProjectionError::AnnotationGone(name.clone(), page))?;
            update.check_for(&props)?;
            let (author, subject, icon) = update.apply_to();
            Ok(Edit::SetAnnotationInfo { page, index, author, subject, icon })
        }
        ("annotation.reply", ConflictClass::Merge) => {
            let (page, name) = annotation_target(op)?;
            let index = annotation_index(document, page.index as usize, &name)?;
            let author = op.payload.get("author").and_then(Value::as_str).unwrap_or_default().to_owned();
            // A reply may also set the review status; the engine writes the
            // status text, and every replica uses the server's `/NM`.
            match op.payload.get("state").and_then(Value::as_str) {
                Some(state) => Ok(Edit::SetAnnotationStatusNamed {
                    page: page.index as usize,
                    index,
                    state: ReviewStateWire::parse(state)?.into(),
                    author,
                    name: text(op, "nm")?.to_owned(),
                }),
                None => Ok(Edit::ReplyToAnnotationNamed {
                    page: page.index as usize,
                    index,
                    text: text(op, "text")?.to_owned(),
                    author,
                    name: text(op, "nm")?.to_owned(),
                }),
            }
        }
        ("annotation.delete", ConflictClass::Merge) => {
            let (page, name) = annotation_target(op)?;
            let index = annotation_index(document, page.index as usize, &name)?;
            Ok(Edit::DeleteAnnotation { page: page.index as usize, index })
        }
        ("annotation.restore", ConflictClass::Merge) => {
            // The accepted operation carries the comment's wire body; the copy
            // token only means something to the server (RFC §6). A reply comes
            // back a reply (`parent`), and a state reply keeps its `/State`.
            let (page, name) = annotation_target(op)?;
            let page = page.index as usize;
            let body = op.payload.get("body").ok_or(ProjectionError::Missing("body"))?;
            let author = body.get("author").and_then(Value::as_str).unwrap_or_default().to_owned();
            if let Some(parent) = op.payload.get("parent").and_then(Value::as_str).filter(|parent| !parent.is_empty()) {
                let index = annotation_index(document, page, parent)?;
                return match body.get("state").and_then(Value::as_str) {
                    Some(state) => Ok(Edit::SetAnnotationStatusNamed { page, index, state: ReviewStateWire::parse(state)?.into(), author, name }),
                    None => Ok(Edit::ReplyToAnnotationNamed {
                        page,
                        index,
                        text: body.get("text").and_then(Value::as_str).unwrap_or_default().to_owned(),
                        author,
                        name,
                    }),
                };
            }
            let input = AnnotationInput::from_payload(body)?;
            Ok(Edit::AddAnnotationNamed(input.to_engine(page), name))
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
        ("page.restore", ConflictClass::Structural) => {
            // The page's content travels in the operation: a deleted page is out
            // of every replica, so only the server's copy can put it back.
            let at = op.payload.get("at").and_then(Value::as_u64).and_then(|at| usize::try_from(at).ok()).ok_or(ProjectionError::Missing("at"))?;
            let encoded = op.payload.get("bytes").and_then(Value::as_str).ok_or(ProjectionError::Missing("bytes"))?;
            let bytes = decode_base64(encoded)?;
            Ok(Edit::InsertPagesFrom {
                name: "restore-page.pdf".into(),
                bytes: Arc::new(bytes),
                pages: Some(vec![0]),
                at,
                field_conflicts: FieldConflicts::Rename,
            })
        }
        ("page.insert", ConflictClass::Structural) => {
            // The inserted page travels in the operation, so every replica
            // inserts the same bytes. The address is the target; the accepted
            // operation also carries the normalised position.
            let at = page_position(op).or_else(|| page_target(op).ok().map(|page| page.index as usize)).ok_or(ProjectionError::Missing("at"))?;
            insert_edit(op, at, "inserted-page.pdf")
        }
        ("page.duplicate", ConflictClass::Structural) => {
            // The address is the source page; the copy goes at `at` (right after
            // the source when the operation names no position).
            let page = page_target(op)?;
            let at = page_position(op).unwrap_or_else(|| (page.index as usize).saturating_add(1));
            insert_edit(op, at, "duplicate-page.pdf")
        }
        ("page.replace", ConflictClass::Structural) => {
            // The replacement travels in the operation; the address is the page
            // it replaces. A page beyond the budget is refused here too, so this
            // replica never applies what the server would not accept.
            let page = page_target(op)?;
            let encoded = op.payload.get("bytes").and_then(Value::as_str).ok_or(ProjectionError::Missing("bytes"))?;
            let bytes = decode_base64(encoded)?;
            if bytes.len() > pdfcraft_collab_proto::MAX_PAGE_COPY_BYTES {
                return Err(ProjectionError::TooLarge(bytes.len()));
            }
            Ok(Edit::ReplacePages { pages: vec![page.index as usize], name: "replaced-page.pdf".into(), bytes: Arc::new(bytes), src_pages: vec![0] })
        }
        ("page.insert_blank", ConflictClass::Structural) => {
            // A blank page is built from its size, which travels with the
            // operation, so every replica adds the same page.
            let at = page_position(op).unwrap_or_else(|| page_target(op).map_or(0, |page| page.index as usize));
            let width = op.payload.get("width").and_then(Value::as_f64).ok_or(ProjectionError::Missing("width"))?;
            let height = op.payload.get("height").and_then(Value::as_f64).ok_or(ProjectionError::Missing("height"))?;
            Ok(Edit::InsertBlankPage { at, width, height })
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

fn decode_base64(encoded: &str) -> Result<Vec<u8>, ProjectionError> {
    use base64::Engine as _;
    base64::engine::general_purpose::STANDARD.decode(encoded).map_err(|e| ProjectionError::Invalid(format!("the page copy is not base64: {e}")))
}

fn encode_base64(bytes: &[u8]) -> String {
    use base64::Engine as _;
    base64::engine::general_purpose::STANDARD.encode(bytes)
}

/// Where an insert or duplicate puts its page, as the accepted operation
/// carries it.
fn page_position(op: &Op) -> Option<usize> {
    op.payload.get("at").and_then(Value::as_u64).and_then(|at| usize::try_from(at).ok())
}

/// The edit that inserts the one-page copy an operation carries. The copy is
/// checked against the same budget the server uses: a page it would refuse is
/// never applied locally either.
fn insert_edit(op: &Op, at: usize, name: &str) -> Result<Edit, ProjectionError> {
    let encoded = op.payload.get("bytes").and_then(Value::as_str).ok_or(ProjectionError::Missing("bytes"))?;
    let bytes = decode_base64(encoded)?;
    if bytes.len() > pdfcraft_collab_proto::MAX_PAGE_COPY_BYTES {
        return Err(ProjectionError::TooLarge(bytes.len()));
    }
    Ok(Edit::InsertPagesFrom { name: name.into(), bytes: Arc::new(bytes), pages: Some(vec![0]), at, field_conflicts: FieldConflicts::Rename })
}

/// One page of `bytes` as a PDF of its own: what an insert or a duplicate
/// carries. The scratch session is the engine's own copier, so the copy is the
/// page as every replica draws it.
fn one_page_copy(bytes: &[u8], index: usize) -> Result<Vec<u8>, ProjectionError> {
    let mut scratch = Session::new();
    let id = scratch.open("page-copy.pdf", None, Arc::new(bytes.to_vec()), None).map_err(|e| ProjectionError::Edit(e.to_string()))?;
    let count = scratch.get(id).ok_or(ProjectionError::NoDocument)?.info.pages.len();
    if index >= count {
        return Err(ProjectionError::Missing("page"));
    }
    if count > 1 {
        let others: Vec<usize> = (0..count).filter(|i| *i != index).collect();
        scratch.apply(id, Edit::DeletePages { pages: others }).map_err(|e| ProjectionError::Edit(e.to_string()))?;
    }
    let copy = scratch.save_bytes(id).map_err(|e| ProjectionError::Edit(e.to_string()))?;
    if copy.len() > pdfcraft_collab_proto::MAX_PAGE_COPY_BYTES {
        return Err(ProjectionError::TooLarge(copy.len()));
    }
    Ok(copy.as_ref().clone())
}

/// How many pages a source PDF has: the pages `Edit::InsertPagesFrom` names
/// when it names none.
fn source_page_count(bytes: &[u8]) -> Result<usize, ProjectionError> {
    let mut scratch = Session::new();
    let id = scratch.open("insert-source.pdf", None, Arc::new(bytes.to_vec()), None).map_err(|e| ProjectionError::Edit(e.to_string()))?;
    Ok(scratch.get(id).ok_or(ProjectionError::NoDocument)?.info.pages.len())
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
        Edit::SetAnnotationStatus { page, index, state, author } => {
            // Acrobat's "Set status" is a reply to the comment that carries
            // `/State`; the engine writes the status text on every replica.
            let root = name_at(document, *page, *index)?;
            let nm = new_annotation_name(client_id);
            Ok(vec![merge_op(
                doc,
                next_id(client_id, counter)?,
                base_version,
                epoch,
                Target::Annotation { page: PageRef { index: *page as u32, epoch }, nm: root },
                state_reply_payload(&nm, *state, author),
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
        Edit::StyleAnnotation { .. } | Edit::FillAnnotation { .. } | Edit::BorderColorAnnotation { .. } | Edit::BorderShapeAnnotation { .. } => {
            // One choice of the comment bar is one `annotation.update`.
            let (page, index, update) =
                style_update_for_edits(std::slice::from_ref(edit)).ok_or_else(|| ProjectionError::Unsupported(edit.label().to_string()))?;
            Ok(vec![style_op(document, doc, client_id, base_version, epoch, counter, page, index, update)?])
        }
        Edit::FontSizeAnnotation { page, index, size } => {
            let nm = name_at(document, *page, *index)?;
            let props = document.comment_props(*page, *index).ok_or_else(|| ProjectionError::AnnotationGone(nm.clone(), *page))?;
            FontSizeUpdate { size: *size }.check_for(&props)?;
            Ok(vec![merge_op(
                doc,
                next_id(client_id, counter)?,
                base_version,
                epoch,
                Target::Annotation { page: PageRef { index: *page as u32, epoch }, nm },
                font_size_payload(*size),
            )])
        }
        Edit::SetAnnotationInfo { page, index, author, subject, icon } => {
            let nm = name_at(document, *page, *index)?;
            // `None` leaves a property as it is; the interface's empty field is a
            // value of its own, refused by the wire (a name is never wiped by
            // accident; an undo that removes one says `null`).
            let update = InfoUpdate { author: author.clone().map(Some), subject: subject.clone().map(Some), icon: icon.map(Into::into) };
            if update.is_empty() {
                return Err(ProjectionError::Unsupported(edit.label().to_string()));
            }
            let props = document.comment_props(*page, *index).ok_or_else(|| ProjectionError::AnnotationGone(nm.clone(), *page))?;
            update.check_for(&props)?;
            Ok(vec![merge_op(
                doc,
                next_id(client_id, counter)?,
                base_version,
                epoch,
                Target::Annotation { page: PageRef { index: *page as u32, epoch }, nm },
                info_payload(&update),
            )])
        }
        Edit::Batch { label, edits } => {
            // One undo step of the interface (Comment Properties ▸ OK) may
            // restyle a comment, resize its text and change its author, subject
            // and icon: each kind is its own addressable operation, and the
            // group still undoes as one step.
            Ok(annotation_property_ops(document, edits, doc, client_id, base_version, epoch, counter)?
                .ok_or_else(|| ProjectionError::Unsupported(label.clone()))?)
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
        Edit::InsertPagesFrom { bytes, pages, at, .. } => {
            // One page per operation: the page travels whole, and the position
            // of each is the insert position plus what went before it.
            let wanted = match pages {
                Some(list) => list.clone(),
                None => (0..source_page_count(bytes)?).collect(),
            };
            wanted
                .into_iter()
                .enumerate()
                .map(|(i, page)| {
                    let copy = one_page_copy(&bytes[..], page)?;
                    let at = at.saturating_add(i);
                    Ok(structural_op(
                        doc.clone(),
                        next_id(client_id, counter)?,
                        base_version,
                        epoch,
                        Target::Page(PageRef { index: at as u32, epoch }),
                        insert_page_payload(&copy, at),
                    ))
                })
                .collect::<Result<Vec<_>, ProjectionError>>()
        }
        Edit::DuplicatePages { pages, at } => {
            // Each copy is cut from the document as it is now; the address is
            // the source page, so a later edit to the page set is what the
            // room's version check catches, not a stale offset.
            let mut sorted = pages.clone();
            sorted.sort_unstable();
            sorted.dedup();
            let first_at = at.unwrap_or_else(|| sorted.iter().copied().max().map_or(0, |page| page.saturating_add(1)));
            let source = document.bytes.clone();
            sorted
                .into_iter()
                .enumerate()
                .map(|(i, page)| {
                    let copy = one_page_copy(&source[..], page)?;
                    let at = first_at.saturating_add(i);
                    Ok(structural_op(
                        doc.clone(),
                        next_id(client_id, counter)?,
                        base_version,
                        epoch,
                        Target::Page(PageRef { index: page as u32, epoch }),
                        duplicate_page_payload(&copy, at),
                    ))
                })
                .collect::<Result<Vec<_>, ProjectionError>>()
        }
        Edit::ReplacePages { pages, bytes, src_pages, .. } => {
            // One page per operation: each replacement travels whole and is
            // addressed to the page it replaces, so the page count and the
            // addresses of the other pages do not move.
            if pages.len() != src_pages.len() || pages.is_empty() {
                return Err(ProjectionError::Invalid(format!("{} sources cannot replace {} pages", src_pages.len(), pages.len())));
            }
            pages
                .iter()
                .zip(src_pages)
                .map(|(page, src)| {
                    let copy = one_page_copy(&bytes[..], *src)?;
                    Ok(structural_op(
                        doc.clone(),
                        next_id(client_id, counter)?,
                        base_version,
                        epoch,
                        Target::Page(PageRef { index: *page as u32, epoch }),
                        replace_page_payload(&copy),
                    ))
                })
                .collect::<Result<Vec<_>, ProjectionError>>()
        }
        Edit::InsertBlankPage { at, width, height } => Ok(vec![structural_op(
            doc,
            next_id(client_id, counter)?,
            base_version,
            epoch,
            Target::Page(PageRef { index: *at as u32, epoch }),
            blank_page_payload(*width, *height),
        )]),
        other => Err(ProjectionError::Unsupported(other.label().to_string())),
    }
}

/// One `annotation.update` for a style change on the comment at `(page, index)`,
/// addressed by its `/NM`. The comment's own properties are checked first: a
/// property it does not have is refused on this side too, so the step fails as
/// a whole instead of half-applying.
#[allow(clippy::too_many_arguments)]
fn style_op(
    document: &Document,
    doc: DocKey,
    client_id: &str,
    base_version: u64,
    epoch: u64,
    counter: &mut u64,
    page: usize,
    index: usize,
    update: StyleUpdate,
) -> Result<Op, ProjectionError> {
    let nm = name_at(document, page, index)?;
    let props = document.comment_props(page, index).ok_or_else(|| ProjectionError::AnnotationGone(nm.clone(), page))?;
    update.check_for(&props)?;
    let target = Target::Annotation { page: PageRef { index: page as u32, epoch }, nm };
    Ok(merge_op(doc, next_id(client_id, counter)?, base_version, epoch, target, update.to_payload()))
}

/// The operations one `Edit::Batch` from Comment Properties becomes: the style
/// (`annotation.update`), the font size (`annotation.font_size`) and the
/// author/subject/icon (`annotation.info`) of one comment, in that order. Each
/// is its own addressable operation; the caller's history group undoes them as
/// one step. `None` when the batch is not all property edits on one comment.
fn annotation_property_ops(
    document: &Document,
    edits: &[Edit],
    doc: DocKey,
    client_id: &str,
    base_version: u64,
    epoch: u64,
    counter: &mut u64,
) -> Result<Option<Vec<Op>>, ProjectionError> {
    let mut target: Option<(usize, usize)> = None;
    let mut style: Vec<Edit> = Vec::new();
    let mut size = None;
    let mut info = InfoUpdate::default();
    for edit in edits {
        let (page, index) = match edit {
            Edit::StyleAnnotation { page, index, .. }
            | Edit::FillAnnotation { page, index, .. }
            | Edit::BorderColorAnnotation { page, index, .. }
            | Edit::BorderShapeAnnotation { page, index, .. }
            | Edit::FontSizeAnnotation { page, index, .. }
            | Edit::SetAnnotationInfo { page, index, .. } => (*page, *index),
            _ => return Ok(None),
        };
        if target.get_or_insert((page, index)) != &(page, index) {
            return Ok(None);
        }
        match edit {
            Edit::FontSizeAnnotation { size: named, .. } => size = Some(*named),
            Edit::SetAnnotationInfo { author, subject, icon, .. } => {
                if let Some(author) = author {
                    info.author = Some(Some(author.clone()));
                }
                if let Some(subject) = subject {
                    info.subject = Some(Some(subject.clone()));
                }
                if let Some(icon) = icon {
                    info.icon = Some(IconWire::from(*icon));
                }
            }
            other => style.push(other.clone()),
        }
    }
    let Some((page, index)) = target else { return Ok(None) };
    let nm = name_at(document, page, index)?;
    let props = document.comment_props(page, index).ok_or_else(|| ProjectionError::AnnotationGone(nm.clone(), page))?;
    let address = Target::Annotation { page: PageRef { index: page as u32, epoch }, nm };
    let mut ops = Vec::new();
    if !style.is_empty() {
        let (_, _, update) = style_update_for_edits(&style).ok_or_else(|| ProjectionError::Unsupported("a style edit".into()))?;
        update.check_for(&props)?;
        ops.push(merge_op(doc.clone(), next_id(client_id, counter)?, base_version, epoch, address.clone(), update.to_payload()));
    }
    if let Some(size) = size {
        FontSizeUpdate { size }.check_for(&props)?;
        ops.push(merge_op(doc.clone(), next_id(client_id, counter)?, base_version, epoch, address.clone(), font_size_payload(size)));
    }
    if !info.is_empty() {
        info.check_for(&props)?;
        ops.push(merge_op(doc.clone(), next_id(client_id, counter)?, base_version, epoch, address, info_payload(&info)));
    }
    Ok((!ops.is_empty()).then_some(ops))
}

/// The operation that undoes `op`, built from the document as it was *before*
/// applying it. Undo is a normal operation (ADR-0010), so a conflict with
/// someone else's later edit is reported, not forced: each inverse names the
/// value its original left (`expect`), and the server refuses if that value has
/// changed since. The two deletions have no local inverse — the content is gone
/// from the replica — so the server keeps a copy and the interface builds their
/// inverse from the accepted operation's `copy` token (`inverse_after`).
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
            let expect = op.payload.get("text").and_then(Value::as_str).unwrap_or_default().to_owned();
            merge_op(doc, op_id, base, epoch, Target::Annotation { page, nm }, set_contents_payload_expecting(&previous, &expect))
        }
        "annotation.update" => {
            // The undo puts the look the update replaced back, and only if the
            // comment still has the look this update left (`expect`): a later
            // restyle by someone else is a conflict, not an overwrite.
            let (page, nm) = annotation_target(op)?;
            let index = annotation_index(document, page.index as usize, &nm)?;
            let props =
                document.comment_props(page.index as usize, index).ok_or_else(|| ProjectionError::AnnotationGone(nm.clone(), page.index as usize))?;
            let update = StyleUpdate::from_payload(&op.payload)?;
            let before = update.before(&props);
            if before.is_empty() {
                return Ok(None);
            }
            merge_op(doc, op_id, base, epoch, Target::Annotation { page, nm }, style_restore_payload(&before, &update))
        }
        "annotation.font_size" => {
            // The undo puts the size the operation replaced back, and only if the
            // comment still has the one it left (`expect`).
            let (page, nm) = annotation_target(op)?;
            let index = annotation_index(document, page.index as usize, &nm)?;
            let props =
                document.comment_props(page.index as usize, index).ok_or_else(|| ProjectionError::AnnotationGone(nm.clone(), page.index as usize))?;
            let size = FontSizeUpdate::from_payload(&op.payload)?.size;
            let Some(before) = (FontSizeUpdate { size }).before(&props) else { return Ok(None) };
            merge_op(doc, op_id, base, epoch, Target::Annotation { page, nm }, font_size_restore_payload(before, size))
        }
        "annotation.info" => {
            // The undo puts the author, subject and icon the operation replaced
            // back, and only if the comment still has the ones it left.
            let (page, nm) = annotation_target(op)?;
            let index = annotation_index(document, page.index as usize, &nm)?;
            let props =
                document.comment_props(page.index as usize, index).ok_or_else(|| ProjectionError::AnnotationGone(nm.clone(), page.index as usize))?;
            let update = InfoUpdate::from_payload(&op.payload)?;
            let before = update.before(&props);
            if before.is_empty() {
                return Ok(None);
            }
            merge_op(doc, op_id, base, epoch, Target::Annotation { page, nm }, info_restore_payload(&before, &update))
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
            let expect = op.payload.get("value").and_then(Value::as_str).unwrap_or_default().to_owned();
            merge_op(doc, op_id, base, epoch, Target::Field { name: name.clone() }, field_value_payload_expecting(&previous, &expect))
        }
        "page.rotate" => {
            // `degrees` is a turn *by* that much (`RotatePages` adds it), so the
            // inverse is the opposite turn, not the previous angle.
            let page = page_target(op)?;
            let previous = document.info.pages.get(page.index as usize).map(|p| i64::from(p.rotation)).ok_or(ProjectionError::Missing("page"))?;
            let degrees = op.payload.get("degrees").and_then(Value::as_i64).unwrap_or_default();
            let expect = (previous + degrees).rem_euclid(360);
            structural_op(doc, op_id, base, epoch, Target::Page(page), rotate_payload_expecting((360 - degrees).rem_euclid(360), expect))
        }
        "page.move" => {
            let page = page_target(op)?;
            let to = op.payload.get("to").and_then(Value::as_u64).and_then(|to| usize::try_from(to).ok()).ok_or(ProjectionError::Missing("to"))?;
            structural_op(doc, op_id, base, epoch, Target::Page(PageRef { index: to as u32, epoch }), move_payload(page.index as usize))
        }
        // A deletion and a replacement keep what they removed on the server, so
        // their undo is built from the accepted operation's copy token
        // (`inverse_after`), exactly alike: this replica no longer has what was
        // deleted or replaced.
        "page.delete" | "page.replace" => return Ok(None),
        "page.insert" | "page.duplicate" | "page.insert_blank" => {
            // The undo of an insert is a delete of the page it put there; the
            // page is addressed where the operation placed it.
            let at = page_position(op).or_else(|| page_target(op).ok().map(|page| page.index as usize)).ok_or(ProjectionError::Missing("at"))?;
            structural_op(doc, op_id, base, epoch, Target::Page(PageRef { index: at as u32, epoch }), delete_page_payload())
        }
        other => return Err(ProjectionError::Unsupported(other.to_owned())),
    };
    Ok(Some(inverse))
}

/// The inverse of a deletion, once the server has answered how to undo it: the
/// accepted operation carries the `copy` token of what it removed. `None` when
/// the server kept no copy (the comment was not the room's, or the page was too
/// large to travel back in one operation).
pub fn inverse_after(applied: &Applied) -> Option<Op> {
    let op = &applied.op;
    let action = op.payload.get("action").and_then(Value::as_str)?;
    let doc = op.doc.clone();
    let op_id = OpId::new(format!("undo-{}", op.op_id.as_str())).ok()?;
    // A restyle is undone from the look the server replaced (`before`), not from
    // this replica's guess: `inverse_for` built one before the operation was
    // sent, and this is the authority that patches it.
    if action == "annotation.update" {
        let style = StyleUpdate::from_value(op.payload.get("style")?).ok()?;
        let before = StyleUpdate::from_value(op.payload.get("before")?).ok()?;
        if before.is_empty() {
            return None;
        }
        let Target::Annotation { page, nm } = &op.target else { return None };
        let target = Target::Annotation { page: *page, nm: nm.clone() };
        return Some(merge_op(doc, op_id, applied.version, applied.epoch, target, style_restore_payload(&before, &style)));
    }
    // A font size and a comment's author/subject/icon are undone the same way:
    // from the value the server says it replaced (`before`).
    if action == "annotation.font_size" {
        let size = FontSizeUpdate::from_payload(&op.payload).ok()?.size;
        let before = op.payload.get("before").and_then(Value::as_f64)?;
        let Target::Annotation { page, nm } = &op.target else { return None };
        let target = Target::Annotation { page: *page, nm: nm.clone() };
        return Some(merge_op(doc, op_id, applied.version, applied.epoch, target, font_size_restore_payload(before, size)));
    }
    if action == "annotation.info" {
        let info = InfoUpdate::from_value(op.payload.get("info")?).ok()?;
        let before = InfoUpdate::from_value(op.payload.get("before")?).ok()?;
        if before.is_empty() {
            return None;
        }
        let Target::Annotation { page, nm } = &op.target else { return None };
        let target = Target::Annotation { page: *page, nm: nm.clone() };
        return Some(merge_op(doc, op_id, applied.version, applied.epoch, target, info_restore_payload(&before, &info)));
    }
    // A replaced page comes back from the copy the server kept — and when it
    // kept none (the page was over the budget), the undo is still sent, naming
    // no copy, so the refusal is `conflict` from the room rather than a guess
    // made here. A refusal changes neither the document nor the version.
    if action == "page.replace" {
        let Target::Page(page) = &op.target else { return None };
        let token = op.payload.get("copy").and_then(Value::as_str);
        let target = Target::Page(PageRef { index: page.index, epoch: applied.epoch });
        return Some(structural_op(doc, op_id, applied.version, applied.epoch, target, replace_page_from_copy_payload(token)));
    }
    let copy = op.payload.get("copy").and_then(Value::as_str)?.to_owned();
    match action {
        "annotation.delete" => {
            let Target::Annotation { page, nm } = &op.target else { return None };
            Some(merge_op(
                doc,
                op_id,
                applied.version,
                applied.epoch,
                Target::Annotation { page: *page, nm: nm.clone() },
                json!({"action": "annotation.restore", "copy": copy}),
            ))
        }
        "page.delete" => {
            // The target is the page that was deleted; it goes back at that
            // index, addressed in the epoch the delete left.
            let Target::Page(page) = &op.target else { return None };
            let at = page.index;
            let page = PageRef { index: at, epoch: applied.epoch };
            let payload = json!({"action": "page.restore", "copy": copy, "at": at});
            Some(structural_op(doc, op_id, applied.version, applied.epoch, Target::Page(page), payload))
        }
        _ => None,
    }
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
        assert_eq!(inverse.payload["expect"], "edited", "the undo names the text it leaves behind");

        let rotate = pdfcraft_engine::Edit::RotatePages { pages: vec![0], degrees: 90 };
        let ops = super::ops_for_edit(document, &rotate, doc()?, "client", 1, 0, &mut counter)?;
        let inverse = super::inverse_for(document, &ops[0], OpId::new("inv-2")?)?;
        let inverse = inverse.ok_or("a rotation can be undone")?;
        assert_eq!(inverse.payload["degrees"], 270, "the opposite quarter turn: `degrees` is a turn by, not an angle");
        assert_eq!(inverse.payload["expect"], 90);
        Ok(())
    }

    /// A deletion has no local inverse: the server keeps the copy and the
    /// accepted operation names it. These are the two halves of RFC §6.
    #[test]
    fn a_deletion_is_undone_from_the_server_copy() -> Result<(), Box<dyn std::error::Error>> {
        let (session, id) = open()?;
        let document = session.get(id).ok_or("gone")?;
        let page = PageRef { index: 0, epoch: 0 };
        let add = merge_op(doc()?, OpId::new("a1")?, 0, 0, Target::Page(page), add_note_payload("root", [40.0, 250.0], "hello", "Ada"));
        assert!(super::inverse_for(document, &add, OpId::new("i1")?)?.is_some());
        let delete = merge_op(doc()?, OpId::new("d1")?, 1, 0, Target::Annotation { page, nm: "root".into() }, super::delete_payload());
        assert!(super::inverse_for(document, &delete, OpId::new("i2")?)?.is_none(), "the replica no longer has what was deleted");
        Ok(())
    }

    #[test]
    fn the_accepted_delete_answers_with_the_operation_that_undoes_it() -> Result<(), Box<dyn std::error::Error>> {
        let page = PageRef { index: 1, epoch: 3 };
        let mut delete = merge_op(doc()?, OpId::new("d1")?, 4, 3, Target::Annotation { page, nm: "root".into() }, super::delete_payload());
        delete.payload["copy"] = serde_json::json!("c7");
        let applied = pdfcraft_collab_proto::Applied {
            op_id: delete.op_id.clone(),
            actor_id: pdfcraft_collab_proto::ActorId::new("ada")?,
            version: 5,
            epoch: 3,
            op: delete,
            result: serde_json::json!({"nm": "root", "copy": "c7"}),
        };
        let undo = super::inverse_after(&applied).ok_or("a deletion with a copy can be undone")?;
        assert_eq!(undo.payload["action"], "annotation.restore");
        assert_eq!(undo.payload["copy"], "c7");
        assert_eq!(undo.target, Target::Annotation { page, nm: "root".into() });
        assert_eq!((undo.base_version, undo.epoch), (5, 3), "the undo is built on the head the delete left");

        let mut page_delete = super::structural_op(
            doc()?,
            OpId::new("d2")?,
            6,
            3,
            Target::Page(PageRef { index: 2, epoch: 3 }),
            serde_json::json!({"action": "page.delete"}),
        );
        page_delete.payload["copy"] = serde_json::json!("c8");
        let applied = pdfcraft_collab_proto::Applied {
            op_id: page_delete.op_id.clone(),
            actor_id: pdfcraft_collab_proto::ActorId::new("ada")?,
            version: 7,
            epoch: 4,
            op: page_delete,
            result: serde_json::json!({"page": 2, "copy": "c8"}),
        };
        let undo = super::inverse_after(&applied).ok_or("a page with a copy can be undone")?;
        assert_eq!(undo.payload["action"], "page.restore");
        assert_eq!(undo.payload["at"], 2);
        assert_eq!(undo.class, pdfcraft_collab_proto::ConflictClass::Structural);
        assert_eq!(undo.target, Target::Page(PageRef { index: 2, epoch: 4 }), "the restored page is addressed in the new epoch");

        let applied = pdfcraft_collab_proto::Applied {
            op_id: OpId::new("d3")?,
            actor_id: pdfcraft_collab_proto::ActorId::new("ada")?,
            version: 8,
            epoch: 4,
            op: merge_op(doc()?, OpId::new("d3")?, 7, 4, Target::Annotation { page, nm: "x".into() }, super::delete_payload()),
            result: serde_json::json!({"nm": "x"}),
        };
        assert!(super::inverse_after(&applied).is_none(), "no copy, no undo: a comment the room did not add");
        Ok(())
    }

    #[test]
    fn a_restore_adds_the_comment_its_operation_carries() -> Result<(), Box<dyn std::error::Error>> {
        let (mut session, id) = open()?;
        let page = PageRef { index: 0, epoch: 0 };
        let mut restore = merge_op(
            doc()?,
            OpId::new("u1")?,
            1,
            0,
            Target::Annotation { page, nm: "root".into() },
            serde_json::json!({"action": "annotation.restore", "copy": "c1"}),
        );
        restore.payload["body"] = add_note_payload("root", [40.0, 250.0], "hello", "Ada");
        let edit = apply(&mut session, id, &restore)?;
        assert!(matches!(edit, pdfcraft_engine::Edit::AddAnnotationNamed(_, _)));
        let document = session.get(id).ok_or("gone")?;
        let back = document.info.annotations.iter().find(|a| a.name.as_deref() == Some("root")).ok_or("the comment did not come back")?;
        assert_eq!(back.contents.as_deref(), Some("hello"));
        Ok(())
    }

    #[test]
    fn a_page_restore_inserts_the_page_its_operation_carries() -> Result<(), Box<dyn std::error::Error>> {
        let (mut session, id) = open()?;
        // A one-page PDF, base64-encoded as the server sends it: two pages of the
        // fixture, copied with the rest deleted.
        let mut scratch = Session::new();
        let src = scratch.open("copy.pdf", None, fixture(2).into(), None)?;
        let one = {
            let count = scratch.get(src).ok_or("gone")?.info.pages.len();
            if count > 1 {
                scratch.apply(src, pdfcraft_engine::Edit::DeletePages { pages: (1..count).collect() })?;
            }
            scratch.save_bytes(src)?
        };
        use base64::Engine as _;
        let encoded = base64::engine::general_purpose::STANDARD.encode(one.as_slice());
        let op = super::structural_op(
            doc()?,
            OpId::new("u1")?,
            1,
            1,
            Target::Page(PageRef { index: 1, epoch: 1 }),
            serde_json::json!({"action": "page.restore", "copy": "c1", "at": 1, "bytes": encoded}),
        );
        let edit = apply(&mut session, id, &op)?;
        assert!(matches!(edit, pdfcraft_engine::Edit::InsertPagesFrom { .. }));
        assert_eq!(session.get(id).ok_or("gone")?.info.pages.len(), 3, "the page landed at the position the delete left");
        Ok(())
    }

    #[test]
    fn an_insert_and_a_duplicate_become_addressable_operations() -> Result<(), Box<dyn std::error::Error>> {
        let (mut session, id) = open()?;
        let mut counter = 0;

        // Insert one page of another file at the end.
        let mut source = Session::new();
        let src = source.open("insert.pdf", None, fixture(1).into(), None)?;
        let bytes = source.save_bytes(src)?;
        let insert = pdfcraft_engine::Edit::InsertPagesFrom {
            name: "insert.pdf".into(),
            bytes,
            pages: None,
            at: 2,
            field_conflicts: pdfcraft_engine::FieldConflicts::Rename,
        };
        let document = session.get(id).ok_or("gone")?;
        let ops = super::ops_for_edit(document, &insert, doc()?, "client", 0, 0, &mut counter)?;
        assert_eq!(ops.len(), 1);
        assert_eq!(ops[0].payload["action"], "page.insert");
        assert_eq!(ops[0].payload["at"], 2);
        assert_eq!(ops[0].target, Target::Page(PageRef { index: 2, epoch: 0 }));
        assert!(ops[0].payload["bytes"].is_string(), "the page travels with the operation");
        let inverse = super::inverse_for(document, &ops[0], OpId::new("inv-1")?)?.ok_or("an insert can be undone")?;
        assert_eq!(inverse.payload["action"], "page.delete");
        assert_eq!(inverse.target, Target::Page(PageRef { index: 2, epoch: 0 }), "the undo deletes the page it put there");
        apply(&mut session, id, &ops[0])?;
        assert_eq!(session.get(id).ok_or("gone")?.info.pages.len(), 3, "the replica inserted the page");

        // Duplicate page 1: the copy goes right after it.
        let duplicate = pdfcraft_engine::Edit::DuplicatePages { pages: vec![1], at: None };
        let document = session.get(id).ok_or("gone")?;
        let ops = super::ops_for_edit(document, &duplicate, doc()?, "client", 1, 1, &mut counter)?;
        assert_eq!(ops.len(), 1);
        assert_eq!(ops[0].payload["action"], "page.duplicate");
        assert_eq!(ops[0].payload["at"], 2);
        assert_eq!(ops[0].target, Target::Page(PageRef { index: 1, epoch: 1 }), "the address is the source page");
        let inverse = super::inverse_for(document, &ops[0], OpId::new("inv-2")?)?.ok_or("a duplicate can be undone")?;
        assert_eq!(inverse.payload["action"], "page.delete");
        apply(&mut session, id, &ops[0])?;
        assert_eq!(session.get(id).ok_or("gone")?.info.pages.len(), 4);
        Ok(())
    }

    /// A page the server would refuse is refused here too, before an operation
    /// is built: the replica never shows a page the room cannot carry.
    #[test]
    fn a_page_over_the_limit_is_refused_on_this_side_too() -> Result<(), Box<dyn std::error::Error>> {
        let (session, id) = open()?;
        let document = session.get(id).ok_or("gone")?;
        let huge = vec![0u8; pdfcraft_collab_proto::MAX_PAGE_COPY_BYTES + 1];
        let op =
            super::structural_op(doc()?, OpId::new("i1")?, 0, 0, Target::Page(PageRef { index: 0, epoch: 0 }), super::insert_page_payload(&huge, 0));
        assert!(matches!(super::edit_for(document, &op), Err(ProjectionError::TooLarge(_))));
        Ok(())
    }

    /// The text of one page of the open document, as a reader sees it.
    fn page_text(session: &Session, id: pdfcraft_engine::DocId, page: usize) -> Result<String, Box<dyn std::error::Error>> {
        let document = session.get(id).ok_or("gone")?;
        let mut exporter = pdfcraft_engine::export::Exporter::new(document);
        Ok(exporter.text(page)?.trim().to_owned())
    }

    /// Replace Pages is one operation per page: the replacement travels whole,
    /// the address is the page it replaces, and the undo comes from the copy the
    /// server keeps (this replica no longer has the page).
    #[test]
    fn a_page_replacement_becomes_one_addressed_operation_and_its_undo() -> Result<(), Box<dyn std::error::Error>> {
        let (mut session, id) = open()?;
        let mut counter = 0;
        let mut source = Session::new();
        let src = source.open("replacement.pdf", None, fixture(2).into(), None)?;
        let bytes = source.save_bytes(src)?;
        let edit = pdfcraft_engine::Edit::ReplacePages { pages: vec![1], name: "replacement.pdf".into(), bytes, src_pages: vec![0] };
        let document = session.get(id).ok_or("gone")?;
        let ops = super::ops_for_edit(document, &edit, doc()?, "client", 0, 0, &mut counter)?;
        assert_eq!(ops.len(), 1, "one page, one operation");
        assert_eq!(ops[0].payload["action"], "page.replace");
        assert_eq!(ops[0].target, Target::Page(PageRef { index: 1, epoch: 0 }));
        assert!(ops[0].payload["bytes"].is_string(), "the page travels with the operation");
        assert!(
            super::inverse_for(document, &ops[0], OpId::new("inv-1")?)?.is_none(),
            "the replica has no page to put back: the server keeps the copy"
        );
        apply(&mut session, id, &ops[0])?;
        assert_eq!(session.get(id).ok_or("gone")?.info.pages.len(), 2, "a replacement does not change the page count");
        assert_eq!(page_text(&session, id, 1)?, "Page 1", "the page is the replacement's content");
        Ok(())
    }

    /// A multi-page Replace Pages is one operation per page, so no page's
    /// address moves; a count mismatch is refused as a whole.
    #[test]
    fn a_multi_page_replacement_is_one_operation_per_page() -> Result<(), Box<dyn std::error::Error>> {
        let (session, id) = open()?;
        let mut counter = 0;
        let mut source = Session::new();
        let src = source.open("replacement.pdf", None, fixture(2).into(), None)?;
        let bytes = source.save_bytes(src)?;
        let edit =
            pdfcraft_engine::Edit::ReplacePages { pages: vec![0, 1], name: "replacement.pdf".into(), bytes: bytes.clone(), src_pages: vec![1, 0] };
        let document = session.get(id).ok_or("gone")?;
        let ops = super::ops_for_edit(document, &edit, doc()?, "client", 0, 0, &mut counter)?;
        assert_eq!(ops.len(), 2);
        assert_eq!(ops[0].target, Target::Page(PageRef { index: 0, epoch: 0 }));
        assert_eq!(ops[1].target, Target::Page(PageRef { index: 1, epoch: 0 }));
        let mismatch = pdfcraft_engine::Edit::ReplacePages { pages: vec![0], name: "replacement.pdf".into(), bytes, src_pages: vec![0, 1] };
        assert!(super::ops_for_edit(document, &mismatch, doc()?, "client", 0, 0, &mut counter).is_err(), "a count mismatch is refused whole");
        Ok(())
    }

    /// The accepted replacement answers with the operation that undoes it: the
    /// same page, addressed in the epoch the replacement left, with the copy the
    /// server kept. No copy means the undo names none and the room refuses it.
    #[test]
    fn the_accepted_replacement_answers_with_the_operation_that_undoes_it() -> Result<(), Box<dyn std::error::Error>> {
        let mut replace = super::structural_op(
            doc()?,
            OpId::new("r1")?,
            4,
            3,
            Target::Page(PageRef { index: 1, epoch: 3 }),
            serde_json::json!({"action": "page.replace", "bytes": "AA=="}),
        );
        replace.payload["copy"] = serde_json::json!("c9");
        let applied = pdfcraft_collab_proto::Applied {
            op_id: replace.op_id.clone(),
            actor_id: pdfcraft_collab_proto::ActorId::new("ada")?,
            version: 5,
            epoch: 4,
            op: replace,
            result: serde_json::json!({"page": 1, "copy": "c9"}),
        };
        let undo = super::inverse_after(&applied).ok_or("a replacement with a copy can be undone")?;
        assert_eq!(undo.payload["action"], "page.replace");
        assert_eq!(undo.payload["copy"], "c9");
        assert_eq!(undo.target, Target::Page(PageRef { index: 1, epoch: 4 }), "the undo addresses the page in the new epoch");
        assert_eq!(undo.class, pdfcraft_collab_proto::ConflictClass::Structural);
        assert_eq!((undo.base_version, undo.epoch), (5, 4), "the undo is built on the head the replacement left");
        assert!(undo.payload.get("bytes").is_none(), "the page comes from the server's copy, not from this replica");

        let no_copy = super::structural_op(
            doc()?,
            OpId::new("r2")?,
            5,
            4,
            Target::Page(PageRef { index: 1, epoch: 4 }),
            serde_json::json!({"action": "page.replace", "bytes": "AA=="}),
        );
        let applied = pdfcraft_collab_proto::Applied {
            op_id: no_copy.op_id.clone(),
            actor_id: pdfcraft_collab_proto::ActorId::new("ada")?,
            version: 6,
            epoch: 5,
            op: no_copy,
            result: serde_json::json!({"page": 1}),
        };
        let undo = super::inverse_after(&applied).ok_or("the undo is still attempted")?;
        assert_eq!(undo.payload["action"], "page.replace");
        assert!(undo.payload["copy"].is_null(), "the room refuses it as conflict instead of this side guessing");
        Ok(())
    }

    /// A blank page is an addressable operation: the position is the target,
    /// the size travels with it, and its undo deletes the page it added.
    #[test]
    fn a_blank_page_becomes_an_addressed_operation_with_its_size() -> Result<(), Box<dyn std::error::Error>> {
        let (mut session, id) = open()?;
        let mut counter = 0;
        let edit = pdfcraft_engine::Edit::InsertBlankPage { at: 2, width: 300.0, height: 400.0 };
        let document = session.get(id).ok_or("gone")?;
        let ops = super::ops_for_edit(document, &edit, doc()?, "client", 0, 0, &mut counter)?;
        assert_eq!(ops.len(), 1);
        assert_eq!(ops[0].payload["action"], "page.insert_blank");
        assert_eq!(ops[0].payload["width"], 300.0);
        assert_eq!(ops[0].payload["height"], 400.0);
        assert_eq!(ops[0].target, Target::Page(PageRef { index: 2, epoch: 0 }), "the address is the insert position");
        let inverse = super::inverse_for(document, &ops[0], OpId::new("inv-1")?)?.ok_or("a blank insert can be undone")?;
        assert_eq!(inverse.payload["action"], "page.delete");
        assert_eq!(inverse.target, Target::Page(PageRef { index: 2, epoch: 0 }));
        apply(&mut session, id, &ops[0])?;
        assert_eq!(session.get(id).ok_or("gone")?.info.pages.len(), 3, "the replica added the page");
        Ok(())
    }

    /// A replacement whose page is over the budget is refused here before an
    /// operation is built, and an accepted one over the budget is never applied.
    #[test]
    fn a_replacement_over_the_limit_is_refused_on_this_side_too() -> Result<(), Box<dyn std::error::Error>> {
        let (session, id) = open()?;
        let document = session.get(id).ok_or("gone")?;
        let huge = vec![0u8; pdfcraft_collab_proto::MAX_PAGE_COPY_BYTES + 1];
        let encoded = {
            use base64::Engine as _;
            base64::engine::general_purpose::STANDARD.encode(&huge)
        };
        let op = super::structural_op(
            doc()?,
            OpId::new("r1")?,
            0,
            0,
            Target::Page(PageRef { index: 0, epoch: 0 }),
            serde_json::json!({"action": "page.replace", "bytes": encoded}),
        );
        assert!(matches!(super::edit_for(document, &op), Err(ProjectionError::TooLarge(_))));
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

    /// One choice of the comment bar becomes one `annotation.update`: every
    /// replica applies one version, and the inverse puts the old look back and
    /// names the look it leaves.
    #[test]
    fn a_style_edit_becomes_one_update_and_its_inverse() -> Result<(), Box<dyn std::error::Error>> {
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
        let index = document.info.annotations.iter().position(|a| a.name.as_deref() == Some("root")).ok_or("the comment is missing")?;
        let previous = document.comment_props(0, index).ok_or("no properties")?.color;

        let mut counter = 0;
        let edit = pdfcraft_engine::Edit::StyleAnnotation { page: 0, index, color: Some([0.0, 0.0, 1.0]), opacity: None, width: None, endings: None };
        let ops = super::ops_for_edit(document, &edit, doc()?, "ada", 3, 0, &mut counter)?;
        assert_eq!(ops.len(), 1, "a restyle is one operation");
        let target = Target::Annotation { page: PageRef { index: 0, epoch: 0 }, nm: "root".into() };
        assert_eq!(ops[0].target, target);
        assert_eq!(ops[0].payload["action"], "annotation.update");
        assert_eq!(ops[0].payload["style"]["color"], serde_json::json!([0.0, 0.0, 1.0]));
        assert_eq!(ops[0].payload["style"].get("opacity"), None, "an unnamed property is not in the style");

        let document = session.get(id).ok_or("gone")?;
        let inverse = super::inverse_for(document, &ops[0], OpId::new("i1")?)?.ok_or("a restyle can be undone")?;
        assert_eq!(inverse.payload["action"], "annotation.update");
        assert_eq!(inverse.payload["style"]["color"], serde_json::json!(previous), "the inverse puts the old colour back");
        assert_eq!(inverse.payload["expect"]["color"], serde_json::json!([0.0, 0.0, 1.0]), "and refuses if the colour changed since");
        Ok(())
    }

    /// The batch Comment Properties sends (a width and a fill together) is one
    /// update, not one operation per edit, and applying the accepted operation
    /// draws the same comment on a replica.
    #[test]
    fn a_batch_of_style_edits_is_one_update_and_round_trips() -> Result<(), Box<dyn std::error::Error>> {
        let (mut session, id) = open()?;
        let (mut other, other_id) = open()?;
        let shape = pdfcraft_engine::Shape::Rectangle { rect: [10.0, 10.0, 80.0, 40.0] };
        let style = pdfcraft_engine::Style::default_for(&shape);
        let new = pdfcraft_engine::NewAnnotation { page: 0, shape, style, contents: String::new(), author: "Ada".into() };
        // Both replicas add the rectangle from the same operation, so both know
        // it under the same `/NM`.
        let add =
            merge_op(doc()?, OpId::new("a0")?, 0, 0, Target::Page(PageRef { index: 0, epoch: 0 }), super::add_annotation_payload(&new, "rect-1")?);
        apply(&mut session, id, &add)?;
        apply(&mut other, other_id, &add)?;

        let document = session.get(id).ok_or("gone")?;
        let index = document.info.annotations.iter().position(|a| a.name.as_deref() == Some("rect-1")).ok_or("the rectangle is missing")?;
        let batch = pdfcraft_engine::Edit::Batch {
            label: "Change comment properties".into(),
            edits: vec![
                pdfcraft_engine::Edit::StyleAnnotation { page: 0, index, color: None, opacity: None, width: Some(3.0), endings: None },
                pdfcraft_engine::Edit::FillAnnotation { page: 0, index, fill: Some([0.0, 1.0, 0.0]) },
            ],
        };
        let mut counter = 0;
        let ops = super::ops_for_edit(document, &batch, doc()?, "ada", 1, 0, &mut counter)?;
        assert_eq!(ops.len(), 1, "one undo step is one operation");
        assert_eq!(ops[0].payload["style"]["width"], serde_json::json!(3.0));
        assert_eq!(ops[0].payload["style"]["fill"], serde_json::json!([0.0, 1.0, 0.0]));

        // The accepted operation is the same body; the other replica applies it.
        let accepted = merge_op(doc()?, OpId::new("style-1")?, 1, 0, ops[0].target.clone(), ops[0].payload.clone());
        let edit = super::edit_for(other.get(other_id).ok_or("gone")?, &accepted)?;
        assert!(matches!(edit, pdfcraft_engine::Edit::RestyleAnnotation { page: 0, .. }));
        other.apply_remote(other_id, edit)?;
        let props = other.get(other_id).ok_or("gone")?.comment_props(0, 0).ok_or("no properties")?;
        assert!((props.width.unwrap_or_default() - 3.0).abs() < 1e-6);
        assert_eq!(props.fill, Some([0.0, 1.0, 0.0]));
        Ok(())
    }

    /// A style edit the comment cannot carry is refused here too, before an
    /// operation is built: a sticky note has no line endings to change.
    #[test]
    fn a_style_edit_the_comment_cannot_carry_is_refused_on_this_side_too() -> Result<(), Box<dyn std::error::Error>> {
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
        let index = document.info.annotations.iter().position(|a| a.name.as_deref() == Some("root")).ok_or("the comment is missing")?;
        let edit = pdfcraft_engine::Edit::StyleAnnotation {
            page: 0,
            index,
            color: Some([0.0, 0.0, 1.0]),
            opacity: None,
            width: None,
            endings: Some(vec![pdfcraft_engine::LineEnding::OpenArrow, pdfcraft_engine::LineEnding::None]),
        };
        let mut counter = 0;
        assert!(matches!(super::ops_for_edit(document, &edit, doc()?, "ada", 0, 0, &mut counter), Err(ProjectionError::Invalid(_))));
        Ok(())
    }

    /// A line's endings travel inside the style object of `annotation.update`;
    /// the inverse names the pair the update replaced, and the accepted operation
    /// draws the same line on the other replica.
    #[test]
    fn line_endings_travel_with_the_style_and_their_undo_names_the_pair() -> Result<(), Box<dyn std::error::Error>> {
        use pdfcraft_engine as engine;
        let (mut session, id) = open()?;
        let (mut other, other_id) = open()?;
        let shape =
            engine::Shape::Line { from: [20.0, 20.0], to: [150.0, 100.0], start: engine::LineEnding::None, end: engine::LineEnding::OpenArrow };
        let new = engine::NewAnnotation {
            page: 0,
            shape: shape.clone(),
            style: engine::Style::default_for(&shape),
            contents: String::new(),
            author: "Ada".into(),
        };
        let add =
            merge_op(doc()?, OpId::new("a0")?, 0, 0, Target::Page(PageRef { index: 0, epoch: 0 }), super::add_annotation_payload(&new, "line-1")?);
        apply(&mut session, id, &add)?;
        apply(&mut other, other_id, &add)?;

        let document = session.get(id).ok_or("gone")?;
        let index = document.info.annotations.iter().position(|a| a.name.as_deref() == Some("line-1")).ok_or("the line is missing")?;
        let edit = engine::Edit::StyleAnnotation {
            page: 0,
            index,
            color: None,
            opacity: None,
            width: None,
            endings: Some(vec![engine::LineEnding::Square, engine::LineEnding::ClosedArrow]),
        };
        let mut counter = 0;
        let ops = super::ops_for_edit(document, &edit, doc()?, "ada", 1, 0, &mut counter)?;
        assert_eq!(ops.len(), 1, "one choice of the dialog is one operation");
        assert_eq!(ops[0].payload["action"], "annotation.update");
        assert_eq!(ops[0].payload["style"]["endings"], serde_json::json!(["square", "closed_arrow"]));

        let inverse = super::inverse_for(document, &ops[0], OpId::new("i1")?)?.ok_or("endings can be undone")?;
        assert_eq!(inverse.payload["style"]["endings"], serde_json::json!(["none", "open_arrow"]), "the pair it replaced");
        assert_eq!(inverse.payload["expect"]["endings"], serde_json::json!(["square", "closed_arrow"]));

        // The accepted operation is the same body; the other replica applies it.
        let accepted = merge_op(doc()?, OpId::new("style-1")?, 1, 0, ops[0].target.clone(), ops[0].payload.clone());
        let edit = super::edit_for(other.get(other_id).ok_or("gone")?, &accepted)?;
        assert!(matches!(edit, engine::Edit::RestyleAnnotation { .. }));
        other.apply_remote(other_id, edit)?;
        let props = other.get(other_id).ok_or("gone")?.comment_props(0, 0).ok_or("no properties")?;
        assert_eq!(props.endings, Some(vec![engine::LineEnding::Square, engine::LineEnding::ClosedArrow]));
        Ok(())
    }

    /// A font size and a comment's author/subject/icon are their own addressable
    /// operations; the inverse of each names what it left, and the accepted
    /// operation is replayed as the engine edit it came from.
    #[test]
    fn a_font_size_and_an_info_change_become_operations_with_cas_inverses() -> Result<(), Box<dyn std::error::Error>> {
        use pdfcraft_engine as engine;
        let (mut session, id) = open()?;
        let (mut other, other_id) = open()?;
        let shape = engine::Shape::TextBox { rect: [10.0, 100.0, 180.0, 160.0], font_size: 10.0 };
        let new = engine::NewAnnotation {
            page: 0,
            shape: shape.clone(),
            style: engine::Style::default_for(&shape),
            contents: "Box".into(),
            author: "Ada".into(),
        };
        let add =
            merge_op(doc()?, OpId::new("a0")?, 0, 0, Target::Page(PageRef { index: 0, epoch: 0 }), super::add_annotation_payload(&new, "box-1")?);
        apply(&mut session, id, &add)?;
        apply(&mut other, other_id, &add)?;
        let document = session.get(id).ok_or("gone")?;
        let index = document.info.annotations.iter().position(|a| a.name.as_deref() == Some("box-1")).ok_or("the box is missing")?;
        let mut counter = 0;

        let size = engine::Edit::FontSizeAnnotation { page: 0, index, size: 16.0 };
        let ops = super::ops_for_edit(document, &size, doc()?, "ada", 1, 0, &mut counter)?;
        assert_eq!(ops.len(), 1);
        assert_eq!(ops[0].payload["action"], "annotation.font_size");
        assert_eq!(ops[0].payload["size"], serde_json::json!(16.0));
        let inverse = super::inverse_for(document, &ops[0], OpId::new("i-size")?)?.ok_or("a size can be undone")?;
        assert_eq!(inverse.payload["size"], serde_json::json!(10.0));
        assert_eq!(inverse.payload["expect"], serde_json::json!(16.0));
        let accepted = merge_op(doc()?, OpId::new("size-1")?, 1, 0, ops[0].target.clone(), ops[0].payload.clone());
        let edit = super::edit_for(other.get(other_id).ok_or("gone")?, &accepted)?;
        assert!(matches!(edit, engine::Edit::FontSizeAnnotation { size, .. } if (size - 16.0).abs() < 1e-9));
        other.apply_remote(other_id, edit)?;
        assert_eq!(other.get(other_id).ok_or("gone")?.comment_props(0, 0).ok_or("no properties")?.font_size, Some(16.0));

        // The author, subject and icon of the same comment are one `annotation.info`.
        let document = session.get(id).ok_or("gone")?;
        let info = engine::Edit::SetAnnotationInfo { page: 0, index, author: Some("Bo".into()), subject: Some("Review".into()), icon: None };
        let ops = super::ops_for_edit(document, &info, doc()?, "ada", 2, 0, &mut counter)?;
        assert_eq!(ops.len(), 1);
        assert_eq!(ops[0].payload["action"], "annotation.info");
        assert_eq!(ops[0].payload["info"], serde_json::json!({"author": "Bo", "subject": "Review"}));
        let inverse = super::inverse_for(document, &ops[0], OpId::new("i-info")?)?.ok_or("info can be undone")?;
        assert_eq!(inverse.payload["info"], serde_json::json!({"author": "Ada", "subject": "Text Box"}), "the text box's own subject comes back");
        assert_eq!(inverse.payload["expect"], serde_json::json!({"author": "Bo", "subject": "Review"}));
        let accepted = merge_op(doc()?, OpId::new("info-1")?, 2, 0, ops[0].target.clone(), ops[0].payload.clone());
        let edit = super::edit_for(other.get(other_id).ok_or("gone")?, &accepted)?;
        assert!(matches!(edit, engine::Edit::SetAnnotationInfo { author: Some(ref a), .. } if a == "Bo"));
        other.apply_remote(other_id, edit)?;
        let props = other.get(other_id).ok_or("gone")?.comment_props(0, 0).ok_or("no properties")?;
        assert_eq!(props.author, "Bo");
        assert_eq!(props.subject, "Review");
        Ok(())
    }

    /// One undo step of Comment Properties stays one step: a batch that restyles
    /// a comment, resizes its text and renames its author becomes three
    /// addressable operations, in that order, and refuses as a whole if any part
    /// is not an annotation property edit.
    #[test]
    fn a_mixed_properties_batch_becomes_one_operation_per_kind() -> Result<(), Box<dyn std::error::Error>> {
        use pdfcraft_engine as engine;
        let (mut session, id) = open()?;
        let shape = engine::Shape::TextBox { rect: [10.0, 100.0, 180.0, 160.0], font_size: 10.0 };
        let new = engine::NewAnnotation {
            page: 0,
            shape: shape.clone(),
            style: engine::Style::default_for(&shape),
            contents: "Box".into(),
            author: "Ada".into(),
        };
        apply(
            &mut session,
            id,
            &merge_op(doc()?, OpId::new("a0")?, 0, 0, Target::Page(PageRef { index: 0, epoch: 0 }), super::add_annotation_payload(&new, "box-1")?),
        )?;
        let document = session.get(id).ok_or("gone")?;
        let index = document.info.annotations.iter().position(|a| a.name.as_deref() == Some("box-1")).ok_or("the box is missing")?;
        let batch = engine::Edit::Batch {
            label: "Change comment properties".into(),
            edits: vec![
                engine::Edit::StyleAnnotation { page: 0, index, color: Some([0.0, 0.0, 1.0]), opacity: None, width: None, endings: None },
                engine::Edit::FontSizeAnnotation { page: 0, index, size: 14.0 },
                engine::Edit::SetAnnotationInfo { page: 0, index, author: Some("Bo".into()), subject: None, icon: None },
            ],
        };
        let mut counter = 0;
        let ops = super::ops_for_edit(document, &batch, doc()?, "ada", 1, 0, &mut counter)?;
        let actions: Vec<String> = ops.iter().map(|op| op.payload["action"].as_str().unwrap_or_default().to_owned()).collect();
        assert_eq!(actions, ["annotation.update", "annotation.font_size", "annotation.info"]);
        assert!(ops.iter().all(|op| op.target == ops[0].target), "every operation addresses the same comment");

        // A batch with an edit the wire does not carry is refused whole.
        let mixed = engine::Edit::Batch {
            label: "Change comment properties".into(),
            edits: vec![
                engine::Edit::StyleAnnotation { page: 0, index, color: Some([0.0, 0.0, 1.0]), opacity: None, width: None, endings: None },
                engine::Edit::MoveAnnotation { page: 0, index, dx: 5.0, dy: 5.0 },
            ],
        };
        let document = session.get(id).ok_or("gone")?;
        assert!(matches!(super::ops_for_edit(document, &mixed, doc()?, "ada", 1, 0, &mut counter), Err(ProjectionError::Unsupported(_))));
        Ok(())
    }

    /// A status reply is a named reply with a review state, and its restore
    /// keeps the `/State` instead of coming back as a plain reply.
    #[test]
    fn a_state_reply_keeps_its_status_through_the_wire_and_a_restore() -> Result<(), Box<dyn std::error::Error>> {
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
        let index = document.info.annotations.iter().position(|a| a.name.as_deref() == Some("root")).ok_or("the comment is missing")?;
        let mut counter = 0;
        let edit = pdfcraft_engine::Edit::SetAnnotationStatus { page: 0, index, state: pdfcraft_engine::ReviewState::Accepted, author: "Ada".into() };
        let ops = super::ops_for_edit(document, &edit, doc()?, "ada", 0, 0, &mut counter)?;
        assert_eq!(ops.len(), 1);
        assert_eq!(ops[0].payload["action"], "annotation.reply");
        assert_eq!(ops[0].payload["state"], "accepted");
        assert_eq!(ops[0].target, Target::Annotation { page: PageRef { index: 0, epoch: 0 }, nm: "root".into() });

        // The accepted operation carries the server's `/NM` and the status text.
        let mut payload = ops[0].payload.clone();
        payload["nm"] = serde_json::json!("status-1");
        payload["text"] = serde_json::json!("Accepted set by Ada");
        let accepted = merge_op(doc()?, OpId::new("s1")?, 0, 0, ops[0].target.clone(), payload);
        let document = session.get(id).ok_or("gone")?;
        assert!(matches!(
            super::edit_for(document, &accepted)?,
            pdfcraft_engine::Edit::SetAnnotationStatusNamed { state: pdfcraft_engine::ReviewState::Accepted, .. }
        ));

        // A restore of a status reply comes back a status reply.
        let restore = merge_op(
            doc()?,
            OpId::new("u1")?,
            1,
            0,
            Target::Annotation { page: PageRef { index: 0, epoch: 0 }, nm: "status-1".into() },
            serde_json::json!({"action": "annotation.restore",
                               "parent": "root",
                               "body": {"action": "annotation.reply", "nm": "status-1", "text": "Accepted set by Ada", "author": "Ada", "state": "accepted"}}),
        );
        let document = session.get(id).ok_or("gone")?;
        assert!(matches!(
            super::edit_for(document, &restore)?,
            pdfcraft_engine::Edit::SetAnnotationStatusNamed { state: pdfcraft_engine::ReviewState::Accepted, .. }
        ));
        Ok(())
    }

    /// The accepted operation is the authority for a restyle's undo: it carries
    /// the look the server replaced, which is what the inverse puts back.
    #[test]
    fn the_accepted_update_answers_with_the_operation_that_undoes_it() -> Result<(), Box<dyn std::error::Error>> {
        let target = Target::Annotation { page: PageRef { index: 0, epoch: 0 }, nm: "root".into() };
        let mut op = merge_op(
            doc()?,
            OpId::new("style-1")?,
            4,
            0,
            target,
            serde_json::json!({"action": "annotation.update", "style": {"color": [0.0, 0.0, 1.0]}}),
        );
        op.payload["before"] = serde_json::json!({"color": [1.0, 0.0, 0.0]});
        let applied = pdfcraft_collab_proto::Applied {
            op_id: op.op_id.clone(),
            actor_id: pdfcraft_collab_proto::ActorId::new("ada")?,
            version: 5,
            epoch: 0,
            op,
            result: serde_json::json!({"nm": "root"}),
        };
        let undo = super::inverse_after(&applied).ok_or("a restyle can be undone from the accepted operation")?;
        assert_eq!(undo.payload["action"], "annotation.update");
        assert_eq!(undo.payload["style"]["color"], serde_json::json!([1.0, 0.0, 0.0]));
        assert_eq!(undo.payload["expect"]["color"], serde_json::json!([0.0, 0.0, 1.0]));
        assert_eq!(undo.base_version, 5);
        assert_eq!(undo.target, applied.op.target);
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
