//! PDF adapter for the collaboration room.
//!
//! Translates the normalised operations of [`pdfcraft_collab_proto::Op`] into
//! engine edits and applies them through `Session::apply` (ADR-0001). The
//! adapter owns the authoritative document; the room owns versions, ordering,
//! the lease and roles.
//!
//! Supported v1 operations (the payload carries `action`):
//!
//! | action | class | target |
//! |---|---|---|
//! | `annotation.add` | merge | page |
//! | `annotation.set_contents` | merge | annotation (`/NM`) |
//! | `annotation.reply` | merge | annotation (`/NM`) |
//! | `annotation.delete` | merge | annotation (`/NM`) |
//! | `field.set_value` | merge | field (full name) |
//! | `page.rotate` | structural | page |
//! | `page.move` | structural | page |
//! | `page.delete` | structural | page |
//!
//! Structural operations bump the epoch; merge operations keep it. Annotations
//! are addressed by `/NM` (ADR-0002), never by a client-side index. Commit
//! writes the working file through `Session::save_bytes`, which keeps the
//! document valid and applies a full rewrite only where the edit requires it.

#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

use pdfcraft_collab_proto::{BlobRef, ConflictClass, ErrorCode, Op, PageRef, Target};
use pdfcraft_collab_server::{Adapter, AdapterError, ApplyOutcome, CommitOutcome};
use pdfcraft_engine::{DocId, Document, Edit, EditError, FieldValue, FormFieldKind, NewAnnotation, NoteIcon, Session, Shape, Style};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::collections::HashSet;
use std::sync::Arc;

/// One authoritative PDF, opened through the engine façade.
pub struct PdfAdapter {
    session: Session,
    id: DocId,
    epoch: u64,
}

impl PdfAdapter {
    /// Open a PDF for a collaboration session. Encrypted and signed documents are
    /// refused: the session would need their passwords on the server and a full
    /// rewrite would invalidate the signatures (`RFC.md` §12).
    pub fn open(name: impl Into<String>, bytes: Arc<Vec<u8>>) -> Result<Self, AdapterError> {
        let mut session = Session::new();
        let id = session.open(name, None, bytes, None).map_err(|e| AdapterError::new(ErrorCode::Invalid, e.to_string(), false))?;
        let document = session.get(id).ok_or_else(|| AdapterError::new(ErrorCode::Invalid, "the document did not open", false))?;
        if document.info.encrypted {
            return Err(AdapterError::new(ErrorCode::Unsupported, "an encrypted PDF cannot start a collaboration session", false));
        }
        if document.is_signed() {
            return Err(AdapterError::new(ErrorCode::Unsupported, "a signed PDF cannot start a collaboration session", false));
        }
        if !document.editable() {
            return Err(AdapterError::new(
                ErrorCode::Forbidden,
                document.read_only_reason.clone().unwrap_or_else(|| "the document cannot be edited".into()),
                false,
            ));
        }
        Ok(Self { session, id, epoch: 0 })
    }

    pub fn session(&self) -> &Session {
        &self.session
    }

    pub fn document(&self) -> Option<&Document> {
        self.session.get(self.id)
    }

    pub fn epoch(&self) -> u64 {
        self.epoch
    }

    fn apply_edit(&mut self, edit: Edit) -> Result<(), AdapterError> {
        self.session.apply(self.id, edit).map_err(map_edit)
    }

    fn document_or_error(&self) -> Result<&Document, AdapterError> {
        self.session.get(self.id).ok_or_else(|| AdapterError::new(ErrorCode::Invalid, "the document is gone", false))
    }

    fn page_index(&self, page: PageRef) -> Result<usize, AdapterError> {
        if page.epoch != self.epoch {
            return Err(AdapterError::new(ErrorCode::Stale, "the page belongs to another epoch", true));
        }
        let index = usize::try_from(page.index).map_err(|_| AdapterError::new(ErrorCode::Invalid, "the page index is too large", false))?;
        if index >= self.document_or_error()?.info.pages.len() {
            return Err(AdapterError::new(ErrorCode::Invalid, "the page does not exist", false));
        }
        Ok(index)
    }

    /// The `/Annots` position of the one annotation on `page` whose `/NM` is `name`.
    fn annotation_index(&self, page: usize, name: &str) -> Result<usize, AdapterError> {
        let document = self.document_or_error()?;
        let mut named = document.info.annotations.iter().filter(|a| a.page == page && a.name.as_deref() == Some(name));
        let first = named.next().ok_or_else(|| AdapterError::new(ErrorCode::Conflict, "the annotation was deleted", false))?;
        if named.next().is_some() {
            return Err(AdapterError::new(ErrorCode::Conflict, "several annotations share this name", false));
        }
        Ok(first.index)
    }

    fn add_annotation(&mut self, page: usize, payload: &Value) -> Result<Value, AdapterError> {
        let at = point(payload.get("at"))?;
        let contents = payload.get("contents").and_then(Value::as_str).unwrap_or_default().to_owned();
        let author = payload.get("author").and_then(Value::as_str).unwrap_or("PdfCraft").to_owned();
        let before: HashSet<String> = self.document_or_error()?.info.annotations.iter().filter_map(|a| a.name.clone()).collect();
        let shape = Shape::Note { at, icon: NoteIcon::Comment };
        self.apply_edit(Edit::AddAnnotation(NewAnnotation { page, style: Style::default_for(&shape), shape, contents, author }))?;
        let added = self
            .document_or_error()?
            .info
            .annotations
            .iter()
            .find(|a| a.page == page && a.name.as_ref().is_some_and(|n| !before.contains(n)))
            .and_then(|a| a.name.clone())
            .ok_or_else(|| AdapterError::new(ErrorCode::Invalid, "the annotation was added without a name", false))?;
        Ok(json!({ "nm": added }))
    }

    fn set_contents(&mut self, page: usize, name: &str, payload: &Value) -> Result<Value, AdapterError> {
        let text = payload.get("text").and_then(Value::as_str).ok_or_else(|| AdapterError::new(ErrorCode::Invalid, "text is missing", false))?;
        let index = self.annotation_index(page, name)?;
        self.apply_edit(Edit::SetAnnotationContents { page, index, text: text.to_owned() })?;
        Ok(json!({ "nm": name }))
    }

    fn reply(&mut self, page: usize, name: &str, payload: &Value) -> Result<Value, AdapterError> {
        let text = payload.get("text").and_then(Value::as_str).ok_or_else(|| AdapterError::new(ErrorCode::Invalid, "text is missing", false))?;
        let author = payload.get("author").and_then(Value::as_str).unwrap_or("PdfCraft").to_owned();
        let index = self.annotation_index(page, name)?;
        let before: HashSet<String> = self.document_or_error()?.info.annotations.iter().filter_map(|a| a.name.clone()).collect();
        self.apply_edit(Edit::ReplyToAnnotation { page, index, text: text.to_owned(), author })?;
        let added = self
            .document_or_error()?
            .info
            .annotations
            .iter()
            .find(|a| a.page == page && a.name.as_ref().is_some_and(|n| !before.contains(n)))
            .and_then(|a| a.name.clone())
            .ok_or_else(|| AdapterError::new(ErrorCode::Invalid, "the reply was added without a name", false))?;
        Ok(json!({ "nm": added }))
    }

    fn delete(&mut self, page: usize, name: &str) -> Result<Value, AdapterError> {
        let index = self.annotation_index(page, name)?;
        self.apply_edit(Edit::DeleteAnnotation { page, index })?;
        Ok(json!({ "nm": name }))
    }

    fn set_field(&mut self, name: &str, payload: &Value) -> Result<Value, AdapterError> {
        let text = payload.get("value").and_then(Value::as_str).unwrap_or_default().to_owned();
        let kind = self
            .document_or_error()?
            .form
            .iter()
            .find(|f| f.name == name)
            .map(|f| f.kind)
            .ok_or_else(|| AdapterError::new(ErrorCode::Invalid, "the field does not exist", false))?;
        let value = match kind {
            FormFieldKind::CheckBox => FieldValue::Check(matches!(text.as_str(), "1" | "true" | "on" | "yes")),
            FormFieldKind::Radio => FieldValue::Radio((!text.is_empty()).then_some(text)),
            FormFieldKind::Combo | FormFieldKind::List => FieldValue::Choice(vec![text]),
            _ => FieldValue::Text(text),
        };
        self.apply_edit(Edit::SetFieldValue { name: name.to_owned(), value })?;
        Ok(json!({ "field": name }))
    }
}

impl Adapter for PdfAdapter {
    fn apply(&mut self, op: &Op) -> Result<ApplyOutcome, AdapterError> {
        let action = op
            .payload
            .get("action")
            .and_then(Value::as_str)
            .ok_or_else(|| AdapterError::new(ErrorCode::Invalid, "the payload has no action", false))?;
        let result = match (action, op.class) {
            ("annotation.add", ConflictClass::Merge) => {
                let page = self.page_index(target_page(op)?)?;
                self.add_annotation(page, &op.payload)?
            }
            ("annotation.set_contents", ConflictClass::Merge) => {
                let (page, name) = annotation_target(op)?;
                self.set_contents(self.page_index(page)?, &name, &op.payload)?
            }
            ("annotation.reply", ConflictClass::Merge) => {
                let (page, name) = annotation_target(op)?;
                self.reply(self.page_index(page)?, &name, &op.payload)?
            }
            ("annotation.delete", ConflictClass::Merge) => {
                let (page, name) = annotation_target(op)?;
                self.delete(self.page_index(page)?, &name)?
            }
            ("field.set_value", ConflictClass::Merge) => {
                let Target::Field { name } = &op.target else {
                    return Err(AdapterError::new(ErrorCode::Invalid, "a field operation needs a field target", false));
                };
                self.set_field(name, &op.payload)?
            }
            ("page.rotate", ConflictClass::Structural) => {
                let page = self.page_index(target_page(op)?)?;
                let degrees = op
                    .payload
                    .get("degrees")
                    .and_then(Value::as_i64)
                    .ok_or_else(|| AdapterError::new(ErrorCode::Invalid, "degrees are missing", false))?;
                self.apply_edit(Edit::RotatePages { pages: vec![page], degrees })?;
                self.epoch = self.epoch.saturating_add(1);
                json!({ "page": page })
            }
            ("page.move", ConflictClass::Structural) => {
                let page = self.page_index(target_page(op)?)?;
                let to = op
                    .payload
                    .get("to")
                    .and_then(Value::as_u64)
                    .and_then(|to| usize::try_from(to).ok())
                    .ok_or_else(|| AdapterError::new(ErrorCode::Invalid, "the new position is missing", false))?;
                self.apply_edit(Edit::MovePages { pages: vec![page], to })?;
                self.epoch = self.epoch.saturating_add(1);
                json!({ "page": page, "to": to })
            }
            ("page.delete", ConflictClass::Structural) => {
                let page = self.page_index(target_page(op)?)?;
                self.apply_edit(Edit::DeletePages { pages: vec![page] })?;
                self.epoch = self.epoch.saturating_add(1);
                json!({ "page": page })
            }
            (other, _) => {
                return Err(AdapterError::new(ErrorCode::Unsupported, format!("unsupported pdfcraft action {other:?}"), false));
            }
        };
        Ok(ApplyOutcome { result, epoch: self.epoch })
    }

    fn commit(&mut self) -> Result<CommitOutcome, AdapterError> {
        let bytes = self.session.save_bytes(self.id).map_err(map_edit)?;
        Ok(CommitOutcome { sha256: sha256_hex(&bytes), bytes: bytes.len() as u64, conflict: false })
    }

    fn base(&self) -> Option<BlobRef> {
        let document = self.session.get(self.id)?;
        Some(BlobRef { sha256: sha256_hex(&document.bytes), len: document.bytes.len() as u64 })
    }
}

fn target_page(op: &Op) -> Result<PageRef, AdapterError> {
    match &op.target {
        Target::Page(page) => Ok(*page),
        _ => Err(AdapterError::new(ErrorCode::Invalid, "a page operation needs a page target", false)),
    }
}

fn annotation_target(op: &Op) -> Result<(PageRef, String), AdapterError> {
    match &op.target {
        Target::Annotation { page, nm } => Ok((*page, nm.clone())),
        _ => Err(AdapterError::new(ErrorCode::Invalid, "an annotation operation needs an annotation target", false)),
    }
}

fn point(value: Option<&Value>) -> Result<[f64; 2], AdapterError> {
    let array = value
        .and_then(Value::as_array)
        .filter(|a| a.len() == 2)
        .ok_or_else(|| AdapterError::new(ErrorCode::Invalid, "the point must be [x, y]", false))?;
    let x = array[0].as_f64().ok_or_else(|| AdapterError::new(ErrorCode::Invalid, "the point is not numeric", false))?;
    let y = array[1].as_f64().ok_or_else(|| AdapterError::new(ErrorCode::Invalid, "the point is not numeric", false))?;
    if !x.is_finite() || !y.is_finite() {
        return Err(AdapterError::new(ErrorCode::Invalid, "the point must be finite", false));
    }
    Ok([x, y])
}

fn map_edit(error: EditError) -> AdapterError {
    let code = match &error {
        EditError::ReadOnly(_) | EditError::NotPermitted(_) | EditError::SignedRewrite(_) => ErrorCode::Forbidden,
        _ => ErrorCode::Invalid,
    };
    AdapterError::new(code, error.to_string(), false)
}

fn sha256_hex(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    digest.iter().map(|byte| format!("{byte:02x}")).collect()
}

/// The author the adapter records when an operation names none.
pub const DEFAULT_AUTHOR: &str = "PdfCraft";

#[cfg(test)]
mod tests {
    use super::{DEFAULT_AUTHOR, PdfAdapter};
    use pdfcraft_collab_proto::{
        ActorId, ClientInfo, CommitResult, ConflictClass, DocKey, Hello, Op, OpId, OpKind, PageRef, Role, SessionId, Target,
    };
    use pdfcraft_collab_server::{Adapter, Room, RoomConfig};
    use serde_json::json;

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

    fn op(
        id: &str,
        class: ConflictClass,
        target: Target,
        base: u64,
        epoch: u64,
        payload: serde_json::Value,
    ) -> Result<Op, Box<dyn std::error::Error>> {
        Ok(Op {
            op_id: OpId::new(id)?,
            doc: DocKey::new("file-1")?,
            base_version: base,
            epoch,
            kind: if class == ConflictClass::Structural { OpKind::Structural } else { OpKind::Primitive },
            class,
            target,
            payload,
            preview: None,
        })
    }

    fn hello() -> Result<Hello, Box<dyn std::error::Error>> {
        Ok(Hello {
            v: pdfcraft_collab_proto::PROTOCOL_VERSION,
            app: "pdfcraft".into(),
            doc: DocKey::new("file-1")?,
            jwt: "token".into(),
            client: ClientInfo { name: "test".into(), version: "0".into() },
        })
    }

    #[test]
    fn comments_merge_and_pages_serialize() -> Result<(), Box<dyn std::error::Error>> {
        let adapter = PdfAdapter::open("fixture.pdf", fixture(2).into())?;
        let mut room = Room::new(SessionId::new("session-1")?, DocKey::new("file-1")?, adapter, RoomConfig::default());
        let editor = ActorId::new("editor")?;
        room.hello(editor.clone(), Role::Editor, &hello()?, 0)?;

        let add = op(
            "add-1",
            ConflictClass::Merge,
            Target::Page(PageRef { index: 0, epoch: 0 }),
            0,
            0,
            json!({"action": "annotation.add", "at": [40.0, 250.0], "contents": "hello", "author": DEFAULT_AUTHOR}),
        )?;
        let applied = room.apply(&editor, &add, 0)?;
        let name = applied.result["nm"].as_str().ok_or("no /NM returned")?.to_owned();

        let set = op(
            "set-1",
            ConflictClass::Merge,
            Target::Annotation { page: PageRef { index: 0, epoch: 0 }, nm: name },
            1,
            0,
            json!({"action": "annotation.set_contents", "text": "edited"}),
        )?;
        room.apply(&editor, &set, 0)?;

        let rotate = op(
            "rot-1",
            ConflictClass::Structural,
            Target::Page(PageRef { index: 0, epoch: 0 }),
            2,
            0,
            json!({"action": "page.rotate", "degrees": 90}),
        )?;
        let applied = room.apply(&editor, &rotate, 0)?;
        assert_eq!(applied.epoch, 1);

        let stale = op(
            "add-2",
            ConflictClass::Merge,
            Target::Page(PageRef { index: 0, epoch: 0 }),
            3,
            0,
            json!({"action": "annotation.add", "at": [40.0, 250.0]}),
        )?;
        assert!(room.apply(&editor, &stale, 0).is_err(), "an old epoch must be refused");

        let version = room.version();
        let committed = room.commit(&editor, version)?;
        assert!(!committed.sha256.is_empty());
        assert!(committed.bytes > 0);
        Ok(())
    }

    #[test]
    fn adapter_commit_is_a_valid_pdf() -> Result<(), Box<dyn std::error::Error>> {
        let mut adapter = PdfAdapter::open("fixture.pdf", fixture(1).into())?;
        let outcome = adapter.commit()?;
        assert!(!outcome.sha256.is_empty());
        assert_eq!(outcome.sha256.len(), 64);
        assert!(adapter.base().is_some());
        let room_commit: CommitResult = CommitResult { version: 0, sha256: outcome.sha256, bytes: outcome.bytes, conflict: false };
        assert_eq!(room_commit.bytes, outcome.bytes);
        Ok(())
    }
}
