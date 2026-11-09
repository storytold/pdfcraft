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
//! | `annotation.update` | merge | annotation (`/NM`) |
//! | `annotation.reply` | merge | annotation (`/NM`) |
//! | `annotation.delete` | merge | annotation (`/NM`) |
//! | `annotation.restore` | merge | annotation (`/NM`) |
//! | `field.set_value` | merge | field (full name) |
//! | `page.rotate` | structural | page |
//! | `page.move` | structural | page |
//! | `page.delete` | structural | page |
//! | `page.restore` | structural | page |
//! | `page.insert` | structural | page |
//! | `page.duplicate` | structural | page |
//!
//! A delete keeps a copy of what it removed (RFC §6): the accepted operation
//! answers with its `copy` token, and `annotation.restore` / `page.restore` put
//! that copy back as an ordinary operation, so every replica and the journal see
//! the same thing. The copies live in memory and are rebuilt by `replay`, not
//! written next to the journal: replaying the delete rebuilds the copy.
//!
//! An insert and a duplicate carry their own one-page copy in the operation
//! (`bytes`, base64): the page travels whole, so every replica and the journal
//! insert exactly the same content, and a page over
//! [`MAX_RESTORE_PAGE_BYTES`] is refused before the document changes.
//!
//! Structural operations bump the epoch; merge operations keep it. Annotations
//! are addressed by `/NM` (ADR-0002), never by a client-side index. Commit
//! writes the working file through `Session::save_bytes`, which keeps the
//! document valid and applies a full rewrite only where the edit requires it.

#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

use pdfcraft_collab_annot::{AnnotationInput, ReviewStateWire, StyleUpdate, WireError};
use pdfcraft_collab_proto::{ActorId, BlobRef, ConflictClass, ErrorCode, MAX_ANNOTATION_NAME, Op, PageRef, Target};
use pdfcraft_collab_server::{Adapter, AdapterError, ApplyOutcome, CommitOutcome, Published};
use pdfcraft_engine::{DocId, Document, Edit, EditError, FieldConflicts, FieldValue, FormFieldKind, Session};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::collections::{HashMap, HashSet};
use std::sync::Arc;

mod store;

pub use store::{CabinetCopy, JournalRecord, Store};

/// One authoritative PDF, opened through the engine façade.
pub struct PdfAdapter {
    session: Session,
    id: DocId,
    epoch: u64,
    /// The durable base and journal. `None` keeps the document in memory only
    /// (tests, headless adapters).
    store: Option<Store>,
    /// The file as last written by `commit`, kept so a publisher can send it
    /// without saving the document a second time.
    last_commit: Option<Arc<Vec<u8>>>,
    /// The wire body of every comment this room added, by `/NM`. The copy an
    /// `annotation.delete` keeps for undo comes from here; a comment that was
    /// in the file before the session has no wire body, so deleting it stays
    /// irreversible.
    known: HashMap<String, Value>,
    /// What this room deleted, oldest first, bounded. Rebuilt by `replay` (the
    /// journal holds the deletes, and the document is at the same state when
    /// they are re-applied), so it is not written next to the journal.
    copies: Vec<DeletedCopy>,
    next_copy: u64,
}

/// What a delete kept so its inverse can put it back (RFC §6, ADR-0010). The
/// token is what the operation carries; it is deterministic on replay, because
/// the copies are made in journal order.
struct DeletedCopy {
    token: String,
    kind: DeletedKind,
    /// Roughly what the copy holds, for the total-size bound.
    bytes: usize,
}

enum DeletedKind {
    /// A comment this room added: the payload it arrived with, the page it was
    /// on, and the `/NM` of the comment it replied to (`/IRT`), so a reply comes
    /// back a reply.
    Annotation { page: u32, nm: String, parent: Option<String>, payload: Value },
    /// The deleted page as a one-page PDF: what `page.restore` inserts.
    Page { index: u32, bytes: Arc<Vec<u8>> },
}

/// The largest one-page copy an operation can carry (RFC §6.1, ADR-0010). The
/// `applied` frame is 256 KiB and base64 is 4/3 of the copy, so the page itself
/// is bounded below the frame; a page over this is refused before the document
/// changes (a delete keeps no copy and its undo is refused), which is the bound
/// ADR-0010 asks for. The number lives in `collab-proto` so the client refuses
/// the same page for the same reason.
pub const MAX_RESTORE_PAGE_BYTES: usize = pdfcraft_collab_proto::MAX_PAGE_COPY_BYTES;

/// How many deletions keep a copy, and how many bytes they may hold together.
const MAX_COPIES: usize = 64;
const MAX_COPY_BYTES: usize = 64 * 1024 * 1024;

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
        Ok(Self { session, id, epoch: 0, store: None, last_commit: None, known: HashMap::new(), copies: Vec::new(), next_copy: 0 })
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

    /// Make the room durable: every accepted operation is appended to the
    /// store's journal and every commit writes a versioned base. The current
    /// epoch comes from the store, so a resumed document keeps accepting merge
    /// operations that were addressed to its pages.
    pub fn attach_store(&mut self, store: Store) {
        self.epoch = store.base_state().1;
        self.store = Some(store);
    }

    /// What the cabinet holds, as last agreed (see `Store::cabinet`).
    pub fn published(&self) -> Option<Published> {
        self.store.as_ref().and_then(Store::cabinet).map(|copy| Published { version: copy.version, etag: copy.etag.clone() })
    }

    /// Record what the cabinet holds (see `Store::set_cabinet`). Without a
    /// store there is nowhere to keep it, so this is a no-op.
    pub fn set_published(&mut self, published: Option<Published>) -> Result<(), AdapterError> {
        let copy = published.map(|published| CabinetCopy { version: published.version, etag: published.etag });
        match self.store.as_mut() {
            Some(store) => store.set_cabinet(copy),
            None => Ok(()),
        }
    }

    /// The file `commit` last wrote, for a publisher that has to send it on.
    pub fn last_commit(&self) -> Option<&Arc<Vec<u8>>> {
        self.last_commit.as_ref()
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

    /// The `/NM` a collaboration client proposed for a new annotation. Absent
    /// means the engine names it; a present name must be free (ADR-0002).
    fn requested_name(payload: &Value) -> Result<Option<String>, AdapterError> {
        let Some(value) = payload.get("nm").filter(|value| !value.is_null()) else { return Ok(None) };
        let name = value.as_str().ok_or_else(|| AdapterError::new(ErrorCode::Invalid, "nm must be a string", false))?;
        if name.trim().is_empty() {
            return Err(AdapterError::new(ErrorCode::Invalid, "nm must not be empty", false));
        }
        if name.len() > MAX_ANNOTATION_NAME {
            return Err(AdapterError::new(ErrorCode::Invalid, "nm is too long", false));
        }
        Ok(Some(name.to_owned()))
    }

    fn ensure_free_name(&self, name: &str) -> Result<(), AdapterError> {
        let taken = self.document_or_error()?.info.annotations.iter().any(|a| a.name.as_deref() == Some(name));
        if taken {
            return Err(AdapterError::new(ErrorCode::Conflict, "an annotation with this name already exists", false));
        }
        Ok(())
    }

    fn add_annotation(&mut self, page: usize, payload: &Value) -> Result<Value, AdapterError> {
        let input = AnnotationInput::from_payload(payload).map_err(map_wire)?;
        let name = Self::requested_name(payload)?;
        if let Some(name) = &name {
            self.ensure_free_name(name)?;
        }
        let before: HashSet<String> = self.document_or_error()?.info.annotations.iter().filter_map(|a| a.name.clone()).collect();
        let new = input.to_engine(page);
        match name {
            Some(name) => self.apply_edit(Edit::AddAnnotationNamed(new, name))?,
            None => self.apply_edit(Edit::AddAnnotation(new))?,
        }
        let added = self
            .document_or_error()?
            .info
            .annotations
            .iter()
            .find(|a| a.page == page && a.name.as_ref().is_some_and(|n| !before.contains(n)))
            .and_then(|a| a.name.clone())
            .ok_or_else(|| AdapterError::new(ErrorCode::Invalid, "the annotation was added without a name", false))?;
        // Remember the wire body: a delete of this comment keeps it for undo.
        let mut body = payload.clone();
        body["nm"] = json!(added);
        self.known.insert(added.clone(), body);
        Ok(json!({ "nm": added }))
    }

    fn set_contents(&mut self, page: usize, name: &str, payload: &Value) -> Result<Value, AdapterError> {
        let text = payload.get("text").and_then(Value::as_str).ok_or_else(|| AdapterError::new(ErrorCode::Invalid, "text is missing", false))?;
        // An undo names the text it left: if someone wrote in between, putting
        // the old text back would overwrite their edit (chair, ADR-0010 §3).
        if let Some(expect) = payload.get("expect").and_then(Value::as_str) {
            let current = self
                .document_or_error()?
                .info
                .annotations
                .iter()
                .find(|a| a.page == page && a.name.as_deref() == Some(name))
                .map(|a| a.contents.clone().unwrap_or_default());
            if current.as_deref() != Some(expect) {
                return Err(AdapterError::new(ErrorCode::Conflict, "the comment changed since; the undo would overwrite it", false));
            }
        }
        let index = self.annotation_index(page, name)?;
        self.apply_edit(Edit::SetAnnotationContents { page, index, text: text.to_owned() })?;
        Ok(json!({ "nm": name }))
    }

    fn reply(&mut self, page: usize, name: &str, payload: &Value) -> Result<Value, AdapterError> {
        // A reply may also set the comment's review status (`/State`,
        // `/StateModel /Review`); the engine writes the status text itself, and
        // the accepted operation carries it so every replica and the journal
        // produce the same reply.
        let state = match payload.get("state").filter(|value| !value.is_null()) {
            Some(value) => {
                let state = value.as_str().ok_or_else(|| AdapterError::new(ErrorCode::Invalid, "state must be a name", false))?;
                Some(ReviewStateWire::parse(state).map_err(map_wire)?.into())
            }
            None => None,
        };
        let text = payload.get("text").and_then(Value::as_str);
        if state.is_none() && text.is_none() {
            return Err(AdapterError::new(ErrorCode::Invalid, "text is missing", false));
        }
        let author = payload.get("author").and_then(Value::as_str).unwrap_or(DEFAULT_AUTHOR).to_owned();
        // The parent is resolved before anything is added: a reply to a comment
        // that is gone is refused, not appended to a thread that no longer exists.
        let index = self.annotation_index(page, name)?;
        let reply_name = Self::requested_name(payload)?;
        if state.is_some() && reply_name.is_none() {
            return Err(AdapterError::new(ErrorCode::Invalid, "a state reply needs an nm", false));
        }
        if let Some(reply_name) = &reply_name {
            self.ensure_free_name(reply_name)?;
        }
        let text = text.unwrap_or_default().to_owned();
        let before: HashSet<String> = self.document_or_error()?.info.annotations.iter().filter_map(|a| a.name.clone()).collect();
        match (state, reply_name) {
            (Some(state), Some(reply_name)) => {
                self.apply_edit(Edit::SetAnnotationStatusNamed { page, index, state, author, name: reply_name })?;
            }
            (None, Some(reply_name)) => self.apply_edit(Edit::ReplyToAnnotationNamed { page, index, text, author, name: reply_name })?,
            (None, None) => self.apply_edit(Edit::ReplyToAnnotation { page, index, text, author })?,
            (Some(_), None) => return Err(AdapterError::new(ErrorCode::Invalid, "a state reply needs an nm", false)),
        }
        let added = self
            .document_or_error()?
            .info
            .annotations
            .iter()
            .find(|a| a.page == page && a.name.as_ref().is_some_and(|n| !before.contains(n)))
            .and_then(|a| a.name.clone())
            .ok_or_else(|| AdapterError::new(ErrorCode::Invalid, "the reply was added without a name", false))?;
        let mut body = payload.clone();
        body["nm"] = json!(added);
        // The status text is the engine's ("Accepted set by ada"); recording it
        // keeps the accepted operation self-describing for every replica.
        if state.is_some()
            && let Some(contents) =
                self.document_or_error()?.info.annotations.iter().find(|a| a.name.as_deref() == Some(added.as_str())).and_then(|a| a.contents.clone())
        {
            body["text"] = json!(contents);
        }
        self.known.insert(added.clone(), body);
        Ok(json!({ "nm": added }))
    }

    /// Restyle the comment `/NM` as one all-or-nothing edit: colour, opacity,
    /// line width, fill and border, the final look of the comment (RFC §3.1.2).
    ///
    /// The address is checked before anything changes: a comment that is gone,
    /// locked, or has no such property is refused, never half-applied. The
    /// accepted operation carries `before` — the look it replaced — so an undo
    /// is a normal operation every replica and the journal can replay.
    fn update_annotation(&mut self, page: usize, name: &str, payload: &mut Value) -> Result<Value, AdapterError> {
        let update = StyleUpdate::from_payload(payload).map_err(map_wire)?;
        let index = self.annotation_index(page, name)?;
        let props = self
            .document_or_error()?
            .comment_props(page, index)
            .ok_or_else(|| AdapterError::new(ErrorCode::Conflict, "the annotation was deleted", false))?;
        update.check_for(&props).map_err(map_wire)?;
        // An undo names the look it left: if someone restyled the comment since,
        // putting the old look back would overwrite their edit (ADR-0010 §3).
        if let Some(expect) = payload.get("expect").filter(|value| !value.is_null()) {
            let expect = StyleUpdate::from_value(expect).map_err(map_wire)?;
            if !expect.still_matches(&props) {
                return Err(AdapterError::new(ErrorCode::Conflict, "the comment was restyled since; the undo would overwrite it", false));
            }
        }
        let before = update.before(&props);
        self.apply_edit(Edit::RestyleAnnotation { page, index, style: update.to_restyle() })?;
        if let Value::Object(map) = payload {
            map.remove("expect");
        }
        payload["style"] = update.to_value();
        payload["before"] = before.to_value();
        Ok(json!({ "nm": name }))
    }

    fn delete(&mut self, page: usize, name: &str) -> Result<Value, AdapterError> {
        let index = self.annotation_index(page, name)?;
        let parent = self
            .document_or_error()?
            .info
            .annotations
            .iter()
            .find(|a| a.page == page && a.name.as_deref() == Some(name))
            .and_then(|a| a.in_reply_to.clone());
        let known = self.known.get(name).cloned();
        self.apply_edit(Edit::DeleteAnnotation { page, index })?;
        self.known.remove(name);
        let mut result = json!({ "nm": name });
        if let Some(payload) = known {
            let size = serde_json::to_vec(&payload).map_or(0, |bytes| bytes.len());
            let token = self.push_copy(DeletedKind::Annotation { page: page as u32, nm: name.to_owned(), parent, payload }, size);
            result["copy"] = json!(token);
        }
        Ok(result)
    }

    /// Put a deleted comment back, from the copy its delete kept. Returns the
    /// wire body so the accepted operation carries it: every replica adds the
    /// same shape, not just a name.
    fn restore_annotation(&mut self, page: usize, name: &str, payload: &Value) -> Result<(Value, Option<String>), AdapterError> {
        // An accepted restore carries the body (and the parent's `/NM`), so
        // replaying the journal needs no copy; a client's request carries only
        // the token.
        let (body, parent) = match payload.get("body") {
            Some(body) => {
                let parent = payload.get("parent").and_then(Value::as_str).map(str::to_owned);
                (body.clone(), parent)
            }
            None => {
                let token = payload
                    .get("copy")
                    .and_then(Value::as_str)
                    .ok_or_else(|| AdapterError::new(ErrorCode::Invalid, "the restore names no copy", false))?;
                match &self.copy(token)?.kind {
                    DeletedKind::Annotation { page: copied_page, nm, parent, payload } => {
                        if nm != name || usize::try_from(*copied_page).ok() != Some(page) {
                            return Err(AdapterError::new(ErrorCode::Invalid, "the copy belongs to another comment", false));
                        }
                        (payload.clone(), parent.clone())
                    }
                    DeletedKind::Page { .. } => {
                        return Err(AdapterError::new(ErrorCode::Conflict, "the copy is a page, not a comment", false));
                    }
                }
            }
        };
        // Someone re-created the comment under the same name: the undo would
        // overwrite their copy, so it is a conflict rather than a merge.
        self.ensure_free_name(name)?;
        match &parent {
            Some(parent) if !parent.is_empty() => {
                let parent_index = self
                    .annotation_index(page, parent)
                    .map_err(|_| AdapterError::new(ErrorCode::Conflict, "the comment this reply answered is gone", false))?;
                let text = body.get("text").and_then(Value::as_str).unwrap_or_default().to_owned();
                let author = body.get("author").and_then(Value::as_str).unwrap_or(DEFAULT_AUTHOR).to_owned();
                // A status reply comes back with its `/State`, not as a plain reply.
                match body.get("state").and_then(Value::as_str) {
                    Some(state) => {
                        let state = ReviewStateWire::parse(state).map_err(map_wire)?;
                        self.apply_edit(Edit::SetAnnotationStatusNamed {
                            page,
                            index: parent_index,
                            state: state.into(),
                            author,
                            name: name.to_owned(),
                        })?;
                    }
                    None => self.apply_edit(Edit::ReplyToAnnotationNamed { page, index: parent_index, text, author, name: name.to_owned() })?,
                }
            }
            _ => {
                let input = AnnotationInput::from_payload(&body).map_err(map_wire)?;
                self.apply_edit(Edit::AddAnnotationNamed(input.to_engine(page), name.to_owned()))?;
            }
        }
        self.known.insert(name.to_owned(), body.clone());
        Ok((body, parent))
    }

    /// A one-page PDF of `index` as the document stands, or `None` when it does
    /// not fit the frame budget (the delete then has no copy to undo with).
    fn page_copy(&self, index: usize) -> Option<(u32, Arc<Vec<u8>>)> {
        let bytes = self.session.save_bytes(self.id).ok()?;
        let mut scratch = Session::new();
        let id = scratch.open("restore-page.pdf", None, bytes, None).ok()?;
        let count = scratch.get(id)?.info.pages.len();
        if index >= count {
            return None;
        }
        if count > 1 {
            let others: Vec<usize> = (0..count).filter(|i| *i != index).collect();
            scratch.apply(id, Edit::DeletePages { pages: others }).ok()?;
        }
        let page = scratch.save_bytes(id).ok()?;
        if page.len() > MAX_RESTORE_PAGE_BYTES {
            return None;
        }
        Some((u32::try_from(index).ok()?, page))
    }

    fn delete_page(&mut self, page: usize) -> Result<Value, AdapterError> {
        // The copy is made before the page goes: afterwards its content is out
        // of the document (the engine frees the objects).
        let copy = self.page_copy(page);
        self.apply_edit(Edit::DeletePages { pages: vec![page] })?;
        self.epoch = self.epoch.saturating_add(1);
        let mut result = json!({ "page": page });
        if let Some((index, bytes)) = copy {
            let size = bytes.len();
            let token = self.push_copy(DeletedKind::Page { index, bytes }, size);
            result["copy"] = json!(token);
        }
        Ok(result)
    }

    /// Insert a deleted page back from its copy, at `at` (clamped to the page
    /// count). The page travels in the operation itself so every replica and
    /// the journal can insert it without a second channel.
    fn restore_page(&mut self, payload: &mut Value) -> Result<Value, AdapterError> {
        let at = payload
            .get("at")
            .and_then(Value::as_u64)
            .and_then(|at| usize::try_from(at).ok())
            .ok_or_else(|| AdapterError::new(ErrorCode::Invalid, "the restore names no position", false))?;
        // An accepted restore carries the page, so replaying the journal needs
        // no copy; a client's request carries only the token.
        let bytes = match payload.get("bytes").and_then(Value::as_str) {
            Some(encoded) => decode_base64(encoded)?,
            None => {
                let token = payload
                    .get("copy")
                    .and_then(Value::as_str)
                    .ok_or_else(|| AdapterError::new(ErrorCode::Invalid, "the restore names no copy", false))?;
                match &self.copy(token)?.kind {
                    DeletedKind::Page { index, bytes } => {
                        payload["copy_page"] = json!(index);
                        bytes.as_slice().to_vec()
                    }
                    DeletedKind::Annotation { .. } => {
                        return Err(AdapterError::new(ErrorCode::Conflict, "the copy is a comment, not a page", false));
                    }
                }
            }
        };
        let at = at.min(self.document_or_error()?.info.pages.len());
        payload["bytes"] = json!(base64_encode(bytes.as_slice()));
        payload["at"] = json!(at);
        // The operation is broadcast and journaled: it has to fit one frame.
        if serde_json::to_vec(payload).map_or(usize::MAX, |bytes| bytes.len()) > MAX_RESTORE_PAGE_BYTES {
            return Err(AdapterError::new(ErrorCode::Limit, "the deleted page is too large to restore in one operation", false));
        }
        self.apply_edit(Edit::InsertPagesFrom {
            name: "restore-page.pdf".into(),
            bytes: Arc::new(bytes),
            pages: Some(vec![0]),
            at,
            field_conflicts: FieldConflicts::Rename,
        })?;
        self.epoch = self.epoch.saturating_add(1);
        Ok(json!({ "page": at }))
    }

    /// Where an inserted page goes: a page address in the current epoch, allowed
    /// to equal the page count (an append). A foreign epoch is retryable, not a
    /// refusal: the pages changed, so the client resyncs and re-addresses.
    fn insert_position(&self, page: PageRef) -> Result<usize, AdapterError> {
        if page.epoch != self.epoch {
            return Err(AdapterError::new(ErrorCode::Stale, "the page belongs to another epoch", true));
        }
        let index = usize::try_from(page.index).map_err(|_| AdapterError::new(ErrorCode::Invalid, "the page index is too large", false))?;
        if index > self.document_or_error()?.info.pages.len() {
            return Err(AdapterError::new(ErrorCode::Invalid, "the position is past the last page", false));
        }
        Ok(index)
    }

    /// The one-page copy an insert or a duplicate carries, checked against the
    /// operation budget before anything changes: a page over the limit is a
    /// refusal with its size, never a partial insert.
    fn carried_page(payload: &Value) -> Result<Vec<u8>, AdapterError> {
        let encoded = payload
            .get("bytes")
            .and_then(Value::as_str)
            .ok_or_else(|| AdapterError::new(ErrorCode::Invalid, "the operation carries no page", false))?;
        let bytes = decode_base64(encoded)?;
        if bytes.len() > MAX_RESTORE_PAGE_BYTES {
            return Err(AdapterError::new(
                ErrorCode::Limit,
                format!("the page is {} bytes; one operation can carry at most {MAX_RESTORE_PAGE_BYTES}", bytes.len()),
                false,
            ));
        }
        Ok(bytes)
    }

    /// Insert the page the operation carries at `at`. The copy stays in the
    /// accepted operation, so every replica and the journal insert the same
    /// bytes without a second channel (the rule `page.restore` follows).
    fn insert_page(&mut self, payload: &mut Value, at: usize) -> Result<Value, AdapterError> {
        let bytes = Self::carried_page(payload)?;
        payload["bytes"] = json!(base64_encode(&bytes));
        payload["at"] = json!(at);
        self.apply_edit(Edit::InsertPagesFrom {
            name: "inserted-page.pdf".into(),
            bytes: Arc::new(bytes),
            pages: Some(vec![0]),
            at,
            field_conflicts: FieldConflicts::Rename,
        })?;
        self.epoch = self.epoch.saturating_add(1);
        Ok(json!({ "page": at }))
    }

    /// Duplicate `page` from the copy the operation carries. The address is the
    /// source page in the current epoch: an edit someone else made to it (or a
    /// later structural operation, which the room answers as stale) does not
    /// lose it, because it is a page reference, not an offset. The copy goes at
    /// the position the operation names, or right after the source when it names
    /// none.
    fn duplicate_page(&mut self, page: usize, payload: &mut Value) -> Result<Value, AdapterError> {
        let count = self.document_or_error()?.info.pages.len();
        let at = match payload.get("at").and_then(Value::as_u64).and_then(|at| usize::try_from(at).ok()) {
            Some(at) if at <= count => at,
            Some(_) => return Err(AdapterError::new(ErrorCode::Invalid, "the position is past the last page", false)),
            None => page.saturating_add(1),
        };
        self.insert_page(payload, at)
    }

    /// Keep a copy and answer its token. The oldest copies go first, so a long
    /// session cannot grow without bound (ADR-0010: no copy, no undo).
    fn push_copy(&mut self, kind: DeletedKind, bytes: usize) -> String {
        self.next_copy = self.next_copy.saturating_add(1);
        let token = format!("c{}", self.next_copy);
        self.copies.push(DeletedCopy { token: token.clone(), kind, bytes });
        let mut total: usize = self.copies.iter().map(|copy| copy.bytes).sum();
        while self.copies.len() > MAX_COPIES || (total > MAX_COPY_BYTES && self.copies.len() > 1) {
            if let Some(old) = self.copies.first() {
                total = total.saturating_sub(old.bytes);
            }
            self.copies.remove(0);
        }
        token
    }

    fn copy(&self, token: &str) -> Result<&DeletedCopy, AdapterError> {
        self.copies
            .iter()
            .find(|copy| copy.token == token)
            .ok_or_else(|| AdapterError::new(ErrorCode::Conflict, "the deleted copy has expired; undo is no longer possible", false))
    }

    fn set_field(&mut self, name: &str, payload: &Value) -> Result<Value, AdapterError> {
        let text = payload.get("value").and_then(Value::as_str).unwrap_or_default().to_owned();
        let field = self
            .document_or_error()?
            .form
            .iter()
            .find(|f| f.name == name)
            .ok_or_else(|| AdapterError::new(ErrorCode::Invalid, "the field does not exist", false))?;
        let kind = field.kind;
        // As for a comment: an undo names the value it left, and a value someone
        // else typed in between is a conflict, not something to overwrite.
        if let Some(expect) = payload.get("expect").and_then(Value::as_str)
            && !field_matches(&field.value.first().cloned().unwrap_or_default(), expect)
        {
            return Err(AdapterError::new(ErrorCode::Conflict, "the field changed since; the undo would overwrite it", false));
        }
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
    fn apply(&mut self, actor: &ActorId, op: &Op) -> Result<ApplyOutcome, AdapterError> {
        // The broadcast operation is the authoritative intent: the author comes
        // from the verified token, not from the client (ADR-0008 §7), and a name
        // the engine generated is filled in so every replica uses it.
        let mut effective = op.clone();
        let action = effective
            .payload
            .get("action")
            .and_then(Value::as_str)
            .ok_or_else(|| AdapterError::new(ErrorCode::Invalid, "the payload has no action", false))?
            .to_owned();
        let names_something = matches!(action.as_str(), "annotation.add" | "annotation.reply");
        if names_something {
            effective.payload["author"] = json!(actor.as_str());
        }
        let result = match (action.as_str(), effective.class) {
            ("annotation.add", ConflictClass::Merge) => {
                let page = self.page_index(target_page(&effective)?)?;
                self.add_annotation(page, &effective.payload)?
            }
            ("annotation.set_contents", ConflictClass::Merge) => {
                let (page, name) = annotation_target(&effective)?;
                self.set_contents(self.page_index(page)?, &name, &effective.payload)?
            }
            ("annotation.update", ConflictClass::Merge) => {
                let (page, name) = annotation_target(&effective)?;
                let page = self.page_index(page)?;
                self.update_annotation(page, &name, &mut effective.payload)?
            }
            ("annotation.reply", ConflictClass::Merge) => {
                let (page, name) = annotation_target(&effective)?;
                self.reply(self.page_index(page)?, &name, &effective.payload)?
            }
            ("annotation.delete", ConflictClass::Merge) => {
                let (page, name) = annotation_target(&effective)?;
                self.delete(self.page_index(page)?, &name)?
            }
            ("annotation.restore", ConflictClass::Merge) => {
                let (page, name) = annotation_target(&effective)?;
                let page = self.page_index(page)?;
                let (body, parent) = self.restore_annotation(page, &name, &effective.payload)?;
                // The accepted operation carries the body: every replica adds
                // the same shape, and the journal can replay the restore.
                effective.payload["body"] = body;
                if let Some(parent) = parent {
                    effective.payload["parent"] = json!(parent);
                }
                json!({ "nm": name })
            }
            ("field.set_value", ConflictClass::Merge) => {
                let Target::Field { name } = &effective.target else {
                    return Err(AdapterError::new(ErrorCode::Invalid, "a field operation needs a field target", false));
                };
                self.set_field(name, &effective.payload)?
            }
            ("page.rotate", ConflictClass::Structural) => {
                let page = self.page_index(target_page(&effective)?)?;
                let degrees = effective
                    .payload
                    .get("degrees")
                    .and_then(Value::as_i64)
                    .ok_or_else(|| AdapterError::new(ErrorCode::Invalid, "degrees are missing", false))?;
                if let Some(expect) = effective.payload.get("expect").and_then(Value::as_i64) {
                    let current = self.document_or_error()?.info.pages.get(page).map_or(0, |p| i64::from(p.rotation));
                    if current != expect {
                        return Err(AdapterError::new(ErrorCode::Conflict, "the page was turned since; the undo would overwrite it", false));
                    }
                }
                self.apply_edit(Edit::RotatePages { pages: vec![page], degrees })?;
                self.epoch = self.epoch.saturating_add(1);
                json!({ "page": page })
            }
            ("page.move", ConflictClass::Structural) => {
                let page = self.page_index(target_page(&effective)?)?;
                let to = effective
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
                let page = self.page_index(target_page(&effective)?)?;
                self.delete_page(page)?
            }
            ("page.restore", ConflictClass::Structural) => self.restore_page(&mut effective.payload)?,
            ("page.insert", ConflictClass::Structural) => {
                let page = target_page(&effective)?;
                let at = self.insert_position(page)?;
                self.insert_page(&mut effective.payload, at)?
            }
            ("page.duplicate", ConflictClass::Structural) => {
                let page = self.page_index(target_page(&effective)?)?;
                self.duplicate_page(page, &mut effective.payload)?
            }
            (other, _) => {
                return Err(AdapterError::new(ErrorCode::Unsupported, format!("unsupported pdfcraft action {other:?}"), false));
            }
        };
        if names_something && let Some(nm) = result.get("nm").cloned() {
            effective.payload["nm"] = nm;
        }
        // A delete answers with the token of its copy; the accepted operation
        // carries it too, so an undo can name it and the journal records what
        // was kept (RFC §6).
        if let Some(copy) = result.get("copy").cloned() {
            effective.payload["copy"] = copy;
        }
        Ok(ApplyOutcome { result, op: effective, epoch: self.epoch })
    }

    fn record(&mut self, version: u64, actor: &ActorId, op: &Op) -> Result<(), AdapterError> {
        let Some(store) = self.store.as_mut() else { return Ok(()) };
        store.append(version, actor, op)
    }

    fn commit(&mut self, version: u64, epoch: u64) -> Result<CommitOutcome, AdapterError> {
        let bytes = self.session.save_bytes(self.id).map_err(map_edit)?;
        if let Some(store) = self.store.as_mut() {
            // The window between the acknowledged journal record and this base
            // write is the one a crash test parks in (see `store.rs`); it is a
            // no-op unless a debug build is explicitly armed.
            store::park_at_fault("commit-before-base");
            store.write_base(version, epoch, bytes.as_slice())?;
            // Entries the new base contains are skipped at replay by version, so
            // a failed truncation costs disk, not correctness.
            let _ = store.truncate(version);
        }
        let outcome = CommitOutcome { sha256: sha256_hex(bytes.as_slice()), bytes: bytes.len() as u64, conflict: false };
        self.last_commit = Some(bytes);
        Ok(outcome)
    }

    fn published(&self) -> Option<Published> {
        PdfAdapter::published(self)
    }

    fn set_published(&mut self, published: Option<Published>) -> Result<(), AdapterError> {
        PdfAdapter::set_published(self, published)
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

/// A payload the shared annotation schema refuses: an unsupported shape is
/// `Unsupported`, malformed data is `Invalid`. Neither is retryable.
fn map_wire(error: WireError) -> AdapterError {
    let code = match &error {
        WireError::UnsupportedShape(_) | WireError::NotRestylable(_) => ErrorCode::Unsupported,
        WireError::Malformed(_) | WireError::Invalid { .. } | WireError::TooMany { .. } => ErrorCode::Invalid,
    };
    AdapterError::new(code, error.to_string(), false)
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

/// A field value the way the wire carries it, for the undo's expected value.
/// A document stores a check box as its wire text too, but `off` has more than
/// one spelling, so the comparison is by meaning.
fn field_matches(stored: &str, expect: &str) -> bool {
    if stored == expect {
        return true;
    }
    let off = |value: &str| matches!(value.to_ascii_lowercase().as_str(), "" | "0" | "false" | "off" | "no");
    let on = |value: &str| matches!(value.to_ascii_lowercase().as_str(), "1" | "true" | "on" | "yes");
    (off(stored) && off(expect)) || (on(stored) && on(expect))
}

fn base64_encode(bytes: &[u8]) -> String {
    use base64::Engine as _;
    base64::engine::general_purpose::STANDARD.encode(bytes)
}

fn decode_base64(encoded: &str) -> Result<Vec<u8>, AdapterError> {
    use base64::Engine as _;
    base64::engine::general_purpose::STANDARD
        .decode(encoded)
        .map_err(|e| AdapterError::new(ErrorCode::Invalid, format!("the page copy is not base64: {e}"), false))
}

/// The author the adapter records when an operation names none.
pub const DEFAULT_AUTHOR: &str = "PdfCraft";

/// A synthetic document with `n` pages, for adapter and store tests.
#[cfg(test)]
pub(crate) fn fixture(n: usize) -> Vec<u8> {
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

#[cfg(test)]
mod tests {
    use super::{DEFAULT_AUTHOR, MAX_RESTORE_PAGE_BYTES, PdfAdapter, base64_encode, fixture};
    use pdfcraft_collab_proto::{
        ActorId, ClientInfo, CommitResult, ConflictClass, DocKey, ErrorCode, Hello, Op, OpId, OpKind, PageRef, Role, SessionId, Target,
    };
    use pdfcraft_collab_server::{Adapter, Room, RoomConfig};
    use serde_json::json;

    /// Commit the way the transport does: the base under the room lock, the
    /// publication outside it (this adapter has none), the outcome after.
    fn commit(room: &mut Room<impl Adapter>, actor: &ActorId, expected: u64) -> Result<CommitResult, pdfcraft_collab_proto::ServerError> {
        let commit = room.commit(actor, expected)?;
        let outcome = commit.publish();
        room.finish(commit, outcome)
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
            json!({"action": "annotation.add", "at": [40.0, 250.0], "contents": "hello", "author": DEFAULT_AUTHOR, "nm": "client-1-note"}),
        )?;
        let applied = room.apply(&editor, &add, 0)?;
        let name = applied.result["nm"].as_str().ok_or("no /NM returned")?.to_owned();
        assert_eq!(name, "client-1-note", "the client's /NM is kept, so every replica can address it");

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
        let committed = commit(&mut room, &editor, version)?;
        assert!(!committed.sha256.is_empty());
        assert!(committed.bytes > 0);
        Ok(())
    }

    #[test]
    fn typed_annotations_apply_and_unshared_ones_are_refused() -> Result<(), Box<dyn std::error::Error>> {
        let mut adapter = PdfAdapter::open("fixture.pdf", fixture(2).into())?;
        let editor = ActorId::new("editor")?;

        let highlight = op(
            "hl-1",
            ConflictClass::Merge,
            Target::Page(PageRef { index: 0, epoch: 0 }),
            0,
            0,
            json!({
                "action": "annotation.add",
                "nm": "ada-hl-1",
                "shape": {"kind": "text_markup", "markup": "highlight", "quads": [[20.0, 200.0, 120.0, 200.0, 20.0, 190.0, 120.0, 190.0]]},
                "style": {"color": [0.1, 0.7, 0.3], "opacity": 0.4, "width": 1.0},
                "contents": "look here",
                "author": "ignored"
            }),
        )?;
        let applied = Adapter::apply(&mut adapter, &editor, &highlight)?;
        assert_eq!(applied.op.payload["shape"]["kind"], "text_markup");
        assert_eq!(applied.op.payload["author"], "editor", "the token names the author");
        assert_eq!(applied.result["nm"], "ada-hl-1");
        {
            let document = adapter.document().ok_or("the document is open")?;
            let annotation =
                document.info.annotations.iter().find(|a| a.name.as_deref() == Some("ada-hl-1")).ok_or("the highlight is in the document")?;
            assert_eq!(annotation.subtype, "Highlight");
            assert_eq!(annotation.contents.as_deref(), Some("look here"));
            assert_eq!(annotation.quads.len(), 1);
            let color = annotation.color.ok_or("the highlight carries its colour")?;
            assert!((f64::from(color[0]) - 0.1).abs() < 0.01);
        }

        let stamp = op(
            "st-1",
            ConflictClass::Merge,
            Target::Page(PageRef { index: 1, epoch: 0 }),
            1,
            0,
            json!({
                "action": "annotation.add",
                "nm": "ada-stamp-1",
                "shape": {"kind": "stamp", "rect": [20.0, 20.0, 120.0, 60.0], "stamp": "draft"},
                "contents": "draft"
            }),
        )?;
        Adapter::apply(&mut adapter, &editor, &stamp)?;
        let document = adapter.document().ok_or("the document is open")?;
        let stored = document.info.annotations.iter().find(|a| a.name.as_deref() == Some("ada-stamp-1")).ok_or("the stamp is in the document")?;
        assert_eq!(stored.subtype, "Stamp");

        let signature = op(
            "sig-1",
            ConflictClass::Merge,
            Target::Page(PageRef { index: 0, epoch: 0 }),
            2,
            0,
            json!({"action": "annotation.add", "nm": "sig", "shape": {"kind": "signature", "strokes": [[[0.0, 0.0], [1.0, 1.0]]]}}),
        )?;
        let refused = Adapter::apply(&mut adapter, &editor, &signature);
        assert!(matches!(refused, Err(e) if e.code == pdfcraft_collab_proto::ErrorCode::Unsupported), "a drawn signature is not shared");
        Ok(())
    }

    #[test]
    fn a_duplicate_client_name_is_refused() -> Result<(), Box<dyn std::error::Error>> {
        let adapter = PdfAdapter::open("fixture.pdf", fixture(1).into())?;
        let mut room = Room::new(SessionId::new("session-1")?, DocKey::new("file-1")?, adapter, RoomConfig::default());
        let editor = ActorId::new("editor")?;
        room.hello(editor.clone(), Role::Editor, &hello()?, 0)?;
        let add = |id: &str, base: u64| {
            op(
                id,
                ConflictClass::Merge,
                Target::Page(PageRef { index: 0, epoch: 0 }),
                base,
                0,
                json!({"action": "annotation.add", "at": [40.0, 250.0], "contents": "hello", "nm": "same-name"}),
            )
        };
        room.apply(&editor, &add("a1", 0)?, 0)?;
        assert!(
            matches!(room.apply(&editor, &add("a2", 1)?, 0), Err(e) if e.code == pdfcraft_collab_proto::ErrorCode::Conflict),
            "two annotations may not share an /NM"
        );
        Ok(())
    }

    #[test]
    fn a_large_document_opens_edits_and_commits() -> Result<(), Box<dyn std::error::Error>> {
        // 500 pages: the acceptance criterion, and a guard against a per-page copy
        // on every operation.
        let started = std::time::Instant::now();
        let adapter = PdfAdapter::open("large.pdf", fixture(500).into())?;
        let mut room = Room::new(SessionId::new("session-1")?, DocKey::new("file-1")?, adapter, RoomConfig::default());
        let editor = ActorId::new("editor")?;
        room.hello(editor.clone(), Role::Editor, &hello()?, 0)?;
        let add = op(
            "a1",
            ConflictClass::Merge,
            Target::Page(PageRef { index: 499, epoch: 0 }),
            0,
            0,
            json!({"action": "annotation.add", "at": [40.0, 250.0], "contents": "last page", "nm": "late-note"}),
        )?;
        room.apply(&editor, &add, 0)?;
        let rotate =
            op("r1", ConflictClass::Structural, Target::Page(PageRef { index: 0, epoch: 0 }), 1, 0, json!({"action": "page.rotate", "degrees": 90}))?;
        room.apply(&editor, &rotate, 0)?;
        let committed = commit(&mut room, &editor, 2)?;
        assert!(!committed.sha256.is_empty());
        assert!(committed.bytes > 0);
        println!("500 pages: {:?}", started.elapsed());
        Ok(())
    }

    #[test]
    fn the_author_comes_from_the_token_not_the_payload() -> Result<(), Box<dyn std::error::Error>> {
        let mut adapter = PdfAdapter::open("fixture.pdf", fixture(1).into())?;
        let actor = ActorId::new("user-7")?;
        let add = op(
            "a1",
            ConflictClass::Merge,
            Target::Page(PageRef { index: 0, epoch: 0 }),
            0,
            0,
            json!({"action": "annotation.add", "at": [40.0, 250.0], "contents": "hello", "author": "Mallory", "nm": "n1"}),
        )?;
        let outcome = adapter.apply(&actor, &add)?;
        assert_eq!(outcome.op.payload["author"], "user-7");
        let document = adapter.document().ok_or("the document is gone")?;
        assert_eq!(document.info.annotations[0].author.as_deref(), Some("user-7"));
        Ok(())
    }

    #[test]
    fn adapter_commit_is_a_valid_pdf() -> Result<(), Box<dyn std::error::Error>> {
        let mut adapter = PdfAdapter::open("fixture.pdf", fixture(1).into())?;
        let outcome = adapter.commit(0, 0)?;
        assert!(!outcome.sha256.is_empty());
        assert_eq!(outcome.sha256.len(), 64);
        assert!(adapter.base().is_some());
        let room_commit: CommitResult = CommitResult { version: 0, sha256: outcome.sha256, bytes: outcome.bytes, conflict: false };
        assert_eq!(room_commit.bytes, outcome.bytes);
        Ok(())
    }

    fn note(nm: &str, contents: &str, base: u64) -> Result<Op, Box<dyn std::error::Error>> {
        op(
            &format!("add-{nm}"),
            ConflictClass::Merge,
            Target::Page(PageRef { index: 0, epoch: 0 }),
            base,
            0,
            json!({"action": "annotation.add", "at": [40.0, 250.0], "contents": contents, "author": DEFAULT_AUTHOR, "nm": nm}),
        )
    }

    fn room_with(editor: &ActorId) -> Result<Room<PdfAdapter>, Box<dyn std::error::Error>> {
        let adapter = PdfAdapter::open("fixture.pdf", fixture(3).into())?;
        let mut room = Room::new(SessionId::new("session-1")?, DocKey::new("file-1")?, adapter, RoomConfig::default());
        room.hello(editor.clone(), Role::Editor, &hello()?, 0)?;
        Ok(room)
    }

    /// The proof of RFC §6: a deleted comment comes back from the server's copy,
    /// with its shape, its text and its `/NM`, as an ordinary operation.
    #[test]
    fn a_deleted_comment_comes_back_from_the_copy() -> Result<(), Box<dyn std::error::Error>> {
        let editor = ActorId::new("editor")?;
        let mut room = room_with(&editor)?;
        room.apply(&editor, &note("n1", "hello", 0)?, 0)?;
        let page = PageRef { index: 0, epoch: 0 };
        let delete = op("del-1", ConflictClass::Merge, Target::Annotation { page, nm: "n1".into() }, 1, 0, json!({"action": "annotation.delete"}))?;
        let applied = room.apply(&editor, &delete, 0)?;
        let copy = applied.result["copy"].as_str().ok_or("the delete kept no copy")?.to_owned();
        assert!(!room.adapter().document().ok_or("gone")?.info.annotations.iter().any(|a| a.name.as_deref() == Some("n1")));

        let expected_epoch = applied.epoch;
        let restore = op(
            "undo-1",
            ConflictClass::Merge,
            Target::Annotation { page: PageRef { index: 0, epoch: expected_epoch }, nm: "n1".into() },
            2,
            expected_epoch,
            json!({"action": "annotation.restore", "copy": copy}),
        )?;
        let restored = room.apply(&editor, &restore, 0)?;
        // The accepted operation carries the body: every replica adds the shape,
        // and the journal can re-apply the restore without the copy.
        assert!(restored.op.payload.get("body").is_some(), "the restore carries the comment's wire body");
        let document = room.adapter().document().ok_or("gone")?;
        let back = document.info.annotations.iter().find(|a| a.name.as_deref() == Some("n1")).ok_or("the comment did not come back")?;
        assert_eq!(back.contents.as_deref(), Some("hello"));
        assert_eq!(back.page, 0);
        Ok(())
    }

    #[test]
    fn a_deleted_reply_comes_back_as_a_reply() -> Result<(), Box<dyn std::error::Error>> {
        let editor = ActorId::new("editor")?;
        let mut room = room_with(&editor)?;
        room.apply(&editor, &note("n1", "hello", 0)?, 0)?;
        let page = PageRef { index: 0, epoch: 0 };
        let reply = op(
            "reply-1",
            ConflictClass::Merge,
            Target::Annotation { page, nm: "n1".into() },
            1,
            0,
            json!({"action": "annotation.reply", "text": "agreed", "author": DEFAULT_AUTHOR, "nm": "r1"}),
        )?;
        room.apply(&editor, &reply, 0)?;
        let delete = op("del-1", ConflictClass::Merge, Target::Annotation { page, nm: "r1".into() }, 2, 0, json!({"action": "annotation.delete"}))?;
        let applied = room.apply(&editor, &delete, 0)?;
        let copy = applied.result["copy"].as_str().ok_or("the delete kept no copy")?.to_owned();
        let restore = op(
            "undo-1",
            ConflictClass::Merge,
            Target::Annotation { page, nm: "r1".into() },
            3,
            0,
            json!({"action": "annotation.restore", "copy": copy}),
        )?;
        room.apply(&editor, &restore, 0)?;
        let document = room.adapter().document().ok_or("gone")?;
        let back = document.info.annotations.iter().find(|a| a.name.as_deref() == Some("r1")).ok_or("the reply did not come back")?;
        assert_eq!(back.contents.as_deref(), Some("agreed"));
        assert_eq!(back.in_reply_to.as_deref(), Some("n1"), "it is still a reply to the same comment");
        Ok(())
    }

    #[test]
    fn an_expired_or_foreign_copy_is_a_conflict() -> Result<(), Box<dyn std::error::Error>> {
        let editor = ActorId::new("editor")?;
        let mut room = room_with(&editor)?;
        room.apply(&editor, &note("n1", "hello", 0)?, 0)?;
        let page = PageRef { index: 0, epoch: 0 };
        let restore = op(
            "undo-1",
            ConflictClass::Merge,
            Target::Annotation { page, nm: "n1".into() },
            1,
            0,
            json!({"action": "annotation.restore", "copy": "c99"}),
        )?;
        let rejected = room.apply(&editor, &restore, 0).err().ok_or("an unknown copy was accepted")?;
        assert_eq!(rejected.code, ErrorCode::Conflict);
        assert!(!rejected.retryable);
        Ok(())
    }

    /// Undo is compare-and-set: an inverse names the value its original left, and
    /// a value someone else wrote since is a conflict, never an overwrite.
    #[test]
    fn an_undo_does_not_overwrite_someone_elses_edit() -> Result<(), Box<dyn std::error::Error>> {
        let editor = ActorId::new("editor")?;
        let mut room = room_with(&editor)?;
        room.apply(&editor, &note("n1", "hello", 0)?, 0)?;
        let page = PageRef { index: 0, epoch: 0 };
        let mine = op(
            "set-1",
            ConflictClass::Merge,
            Target::Annotation { page, nm: "n1".into() },
            1,
            0,
            json!({"action": "annotation.set_contents", "text": "mine"}),
        )?;
        room.apply(&editor, &mine, 0)?;
        let theirs = op(
            "set-2",
            ConflictClass::Merge,
            Target::Annotation { page, nm: "n1".into() },
            2,
            0,
            json!({"action": "annotation.set_contents", "text": "theirs"}),
        )?;
        room.apply(&editor, &theirs, 0)?;
        // The inverse of my edit: put the old text back, but only if my text is
        // still there. It is not.
        let undo = op(
            "undo-1",
            ConflictClass::Merge,
            Target::Annotation { page, nm: "n1".into() },
            3,
            0,
            json!({"action": "annotation.set_contents", "text": "hello", "expect": "mine"}),
        )?;
        let rejected = room.apply(&editor, &undo, 0).err().ok_or("the undo overwrote another edit")?;
        assert_eq!(rejected.code, ErrorCode::Conflict);
        assert!(!rejected.retryable);
        let document = room.adapter().document().ok_or("gone")?;
        assert_eq!(document.info.annotations[0].contents.as_deref(), Some("theirs"));
        Ok(())
    }

    #[test]
    fn a_turned_page_refuses_an_undo_that_would_overwrite_a_later_turn() -> Result<(), Box<dyn std::error::Error>> {
        let editor = ActorId::new("editor")?;
        let mut room = room_with(&editor)?;
        let page = PageRef { index: 0, epoch: 0 };
        let turn = op("rot-1", ConflictClass::Structural, Target::Page(page), 0, 0, json!({"action": "page.rotate", "degrees": 90}))?;
        let applied = room.apply(&editor, &turn, 0)?;
        let again = op(
            "rot-2",
            ConflictClass::Structural,
            Target::Page(PageRef { index: 0, epoch: applied.epoch }),
            applied.version,
            applied.epoch,
            json!({"action": "page.rotate", "degrees": 180}),
        )?;
        let applied = room.apply(&editor, &again, 0)?;
        let undo = op(
            "undo-1",
            ConflictClass::Structural,
            Target::Page(PageRef { index: 0, epoch: applied.epoch }),
            applied.version,
            applied.epoch,
            json!({"action": "page.rotate", "degrees": 0, "expect": 90}),
        )?;
        let rejected = room.apply(&editor, &undo, 0).err().ok_or("the undo overwrote a later turn")?;
        assert_eq!(rejected.code, ErrorCode::Conflict);
        Ok(())
    }

    /// A page is out of every replica once deleted, so only the server's copy can
    /// put it back: the operation carries the page itself.
    #[test]
    fn a_deleted_page_comes_back_with_its_content() -> Result<(), Box<dyn std::error::Error>> {
        let editor = ActorId::new("editor")?;
        let mut room = room_with(&editor)?;
        let delete = op("del-1", ConflictClass::Structural, Target::Page(PageRef { index: 1, epoch: 0 }), 0, 0, json!({"action": "page.delete"}))?;
        let applied = room.apply(&editor, &delete, 0)?;
        let copy = applied.result["copy"].as_str().ok_or("the delete kept no copy")?.to_owned();
        assert_eq!(room.adapter().document().ok_or("gone")?.info.pages.len(), 2);

        let restore = op(
            "undo-1",
            ConflictClass::Structural,
            Target::Page(PageRef { index: 1, epoch: applied.epoch }),
            applied.version,
            applied.epoch,
            json!({"action": "page.restore", "copy": copy, "at": 1}),
        )?;
        let restored = room.apply(&editor, &restore, 0)?;
        assert!(restored.op.payload["bytes"].is_string(), "the accepted operation carries the page");
        let document = room.adapter().document().ok_or("gone")?;
        assert_eq!(document.info.pages.len(), 3, "the page is back where it was");
        let version = room.version();
        let committed = commit(&mut room, &editor, version)?;
        let bytes = room.adapter().last_commit().cloned().ok_or("no commit bytes")?;
        assert!(bytes.windows(6).any(|w| w == b"Page 2"), "the page's content came back");
        assert!(committed.bytes > 0);
        Ok(())
    }

    /// The text of one page of `bytes`, as a reader sees it: the proof that the
    /// copy that travelled is the page the client cut.
    fn page_text(bytes: &std::sync::Arc<Vec<u8>>, page: usize) -> Result<String, Box<dyn std::error::Error>> {
        let mut session = pdfcraft_engine::Session::new();
        let id = session.open("check.pdf", None, bytes.clone(), None)?;
        let document = session.get(id).ok_or("the check document is open")?;
        let mut exporter = pdfcraft_engine::export::Exporter::new(document);
        Ok(exporter.text(page)?.trim().to_owned())
    }

    /// An insert carries the page itself: every replica inserts the same bytes at
    /// the address the operation names, and the epoch moves.
    #[test]
    fn an_inserted_page_lands_at_its_address() -> Result<(), Box<dyn std::error::Error>> {
        let editor = ActorId::new("editor")?;
        let mut room = room_with(&editor)?;
        let (_, copy) = room.adapter().page_copy(2).ok_or("the fixture page copies")?;
        let insert = op(
            "ins-1",
            ConflictClass::Structural,
            Target::Page(PageRef { index: 0, epoch: 0 }),
            0,
            0,
            json!({"action": "page.insert", "at": 0, "bytes": base64_encode(copy.as_slice())}),
        )?;
        let applied = room.apply(&editor, &insert, 0)?;
        assert_eq!(applied.epoch, 1, "an insert is structural");
        assert!(applied.op.payload["bytes"].is_string(), "the accepted operation carries the page");
        assert_eq!(applied.op.payload["at"], 0);
        assert_eq!(room.adapter().document().ok_or("gone")?.info.pages.len(), 4);
        let version = room.version();
        commit(&mut room, &editor, version)?;
        let bytes = room.adapter().last_commit().cloned().ok_or("no commit bytes")?;
        assert_eq!(page_text(&bytes, 0)?, "Page 3", "the copied page is at the address");
        assert_eq!(page_text(&bytes, 1)?, "Page 1", "the original first page moved down");
        Ok(())
    }

    #[test]
    fn a_duplicate_lands_at_the_position_it_names() -> Result<(), Box<dyn std::error::Error>> {
        let editor = ActorId::new("editor")?;
        let mut room = room_with(&editor)?;
        let (_, copy) = room.adapter().page_copy(1).ok_or("the fixture page copies")?;
        let duplicate = op(
            "dup-1",
            ConflictClass::Structural,
            Target::Page(PageRef { index: 1, epoch: 0 }),
            0,
            0,
            json!({"action": "page.duplicate", "at": 2, "bytes": base64_encode(copy.as_slice())}),
        )?;
        let applied = room.apply(&editor, &duplicate, 0)?;
        assert_eq!(applied.epoch, 1);
        assert_eq!(room.adapter().document().ok_or("gone")?.info.pages.len(), 4);
        let version = room.version();
        commit(&mut room, &editor, version)?;
        let bytes = room.adapter().last_commit().cloned().ok_or("no commit bytes")?;
        assert_eq!(page_text(&bytes, 2)?, "Page 2", "the copy follows its source");
        Ok(())
    }

    /// A page over the operation budget is refused before the document changes:
    /// no partial insert, no epoch bump, no journal entry.
    #[test]
    fn a_page_over_the_limit_is_refused_before_the_document_changes() -> Result<(), Box<dyn std::error::Error>> {
        let editor = ActorId::new("editor")?;
        let mut room = room_with(&editor)?;
        let huge = vec![0u8; MAX_RESTORE_PAGE_BYTES + 1];
        let insert = op(
            "ins-1",
            ConflictClass::Structural,
            Target::Page(PageRef { index: 0, epoch: 0 }),
            0,
            0,
            json!({"action": "page.insert", "at": 0, "bytes": base64_encode(&huge)}),
        )?;
        let rejected = room.apply(&editor, &insert, 0).err().ok_or("an oversized page was accepted")?;
        assert_eq!(rejected.code, ErrorCode::Limit);
        assert!(!rejected.retryable);
        assert_eq!(room.adapter().document().ok_or("gone")?.info.pages.len(), 3, "nothing was inserted");
        assert_eq!(room.epoch(), 0, "the epoch did not move");
        assert_eq!(room.version(), 0, "nothing was accepted");
        Ok(())
    }

    #[test]
    fn an_insert_addressed_to_another_epoch_is_retryable() -> Result<(), Box<dyn std::error::Error>> {
        let editor = ActorId::new("editor")?;
        let mut room = room_with(&editor)?;
        let turn = op(
            "rot-1",
            ConflictClass::Structural,
            Target::Page(PageRef { index: 0, epoch: 0 }),
            0,
            0,
            json!({"action": "page.rotate", "degrees": 90}),
        )?;
        let turned = room.apply(&editor, &turn, 0)?;
        let (_, copy) = room.adapter().page_copy(0).ok_or("the fixture page copies")?;
        let insert = op(
            "ins-1",
            ConflictClass::Structural,
            Target::Page(PageRef { index: 0, epoch: 0 }),
            turned.version,
            turned.epoch,
            json!({"action": "page.insert", "at": 0, "bytes": base64_encode(copy.as_slice())}),
        )?;
        let rejected = room.apply(&editor, &insert, 0).err().ok_or("a foreign epoch was accepted")?;
        assert_eq!(rejected.code, ErrorCode::Stale);
        assert!(rejected.retryable, "the client resyncs and re-addresses the page");
        assert_eq!(room.adapter().document().ok_or("gone")?.info.pages.len(), 3);
        Ok(())
    }

    /// The undo of an insert is a delete of the page it put there, so the
    /// document is back to the page count it had.
    #[test]
    fn the_undo_of_an_insert_returns_the_original_page_count() -> Result<(), Box<dyn std::error::Error>> {
        let editor = ActorId::new("editor")?;
        let mut room = room_with(&editor)?;
        let (_, copy) = room.adapter().page_copy(2).ok_or("the fixture page copies")?;
        let insert = op(
            "ins-1",
            ConflictClass::Structural,
            Target::Page(PageRef { index: 1, epoch: 0 }),
            0,
            0,
            json!({"action": "page.insert", "at": 1, "bytes": base64_encode(copy.as_slice())}),
        )?;
        let applied = room.apply(&editor, &insert, 0)?;
        assert_eq!(room.adapter().document().ok_or("gone")?.info.pages.len(), 4);
        let undo = op(
            "undo-1",
            ConflictClass::Structural,
            Target::Page(PageRef { index: 1, epoch: applied.epoch }),
            applied.version,
            applied.epoch,
            json!({"action": "page.delete"}),
        )?;
        room.apply(&editor, &undo, 0)?;
        assert_eq!(room.adapter().document().ok_or("gone")?.info.pages.len(), 3, "the page count is back");
        Ok(())
    }

    /// The accepted operation carries the copy, so a restart replays the insert
    /// without any client file.
    #[test]
    fn an_insert_replays_from_its_operation() -> Result<(), Box<dyn std::error::Error>> {
        let editor = ActorId::new("editor")?;
        let mut room = room_with(&editor)?;
        let (_, copy) = room.adapter().page_copy(2).ok_or("the fixture page copies")?;
        let insert = op(
            "ins-1",
            ConflictClass::Structural,
            Target::Page(PageRef { index: 0, epoch: 0 }),
            0,
            0,
            json!({"action": "page.insert", "at": 0, "bytes": base64_encode(copy.as_slice())}),
        )?;
        let applied = room.apply(&editor, &insert, 0)?.op;
        let adapter = PdfAdapter::open("fixture.pdf", fixture(3).into())?;
        let mut fresh = Room::new(SessionId::new("session-2")?, DocKey::new("file-1")?, adapter, RoomConfig::default());
        fresh.hello(editor.clone(), Role::Editor, &hello()?, 0)?;
        fresh.replay(editor.clone(), applied, 1)?;
        let document = fresh.adapter().document().ok_or("gone")?;
        assert_eq!(document.info.pages.len(), 4, "the journal's copy inserted the page");
        assert_eq!(fresh.epoch(), 1);
        Ok(())
    }

    #[test]
    fn a_restore_replays_without_its_copy() -> Result<(), Box<dyn std::error::Error>> {
        let editor = ActorId::new("editor")?;
        let mut room = room_with(&editor)?;
        let page = PageRef { index: 0, epoch: 0 };
        let mut applied_ops = vec![room.apply(&editor, &note("n1", "hello", 0)?, 0)?.op];
        let delete = op("del-1", ConflictClass::Merge, Target::Annotation { page, nm: "n1".into() }, 1, 0, json!({"action": "annotation.delete"}))?;
        let applied = room.apply(&editor, &delete, 0)?;
        let copy = applied.result["copy"].as_str().ok_or("no copy")?.to_owned();
        applied_ops.push(applied.op);
        let restore = op(
            "undo-1",
            ConflictClass::Merge,
            Target::Annotation { page, nm: "n1".into() },
            2,
            0,
            json!({"action": "annotation.restore", "copy": copy}),
        )?;
        applied_ops.push(room.apply(&editor, &restore, 0)?.op);

        // A restart replays the journal on a fresh adapter: the accepted restore
        // carries the comment's body, so it needs no in-memory copy.
        let adapter = PdfAdapter::open("fixture.pdf", fixture(3).into())?;
        let mut fresh = Room::new(SessionId::new("session-2")?, DocKey::new("file-1")?, adapter, RoomConfig::default());
        fresh.hello(editor.clone(), Role::Editor, &hello()?, 0)?;
        for (version, op) in applied_ops.into_iter().enumerate() {
            fresh.replay(editor.clone(), op, version as u64 + 1)?;
        }
        let document = fresh.adapter().document().ok_or("gone")?;
        assert_eq!(document.info.annotations.len(), 1);
        assert_eq!(document.info.annotations[0].contents.as_deref(), Some("hello"));
        Ok(())
    }
    /// A rectangle with a width and a fill: the shape whose look has every
    /// property `annotation.update` may set.
    fn rectangle(nm: &str, base: u64) -> Result<Op, Box<dyn std::error::Error>> {
        op(
            &format!("add-{nm}"),
            ConflictClass::Merge,
            Target::Page(PageRef { index: 0, epoch: 0 }),
            base,
            0,
            json!({"action": "annotation.add", "nm": nm,
                   "shape": {"kind": "rectangle", "rect": [10.0, 10.0, 80.0, 40.0]},
                   "style": {"color": [1.0, 0.0, 0.0], "opacity": 1.0, "width": 1.0},
                   "contents": "", "author": DEFAULT_AUTHOR}),
        )
    }

    fn style_of(adapter: &PdfAdapter, nm: &str) -> Result<pdfcraft_engine::CommentProps, Box<dyn std::error::Error>> {
        let document = adapter.document().ok_or("the document is gone")?;
        let comment = document.info.annotations.iter().find(|a| a.name.as_deref() == Some(nm)).ok_or("the comment is gone")?;
        document.comment_props(comment.page, comment.index).ok_or_else(|| "the comment has no properties".into())
    }

    fn near(got: Option<[f64; 3]>, want: [f64; 3]) -> bool {
        got.is_some_and(|got| (0..3).all(|i| (got[i] - want[i]).abs() < 1e-3))
    }

    /// The final look of a comment is one operation: colour, opacity, width and
    /// fill land together, the accepted operation carries the look it replaced,
    /// and a fresh replica replaying it draws the same comment.
    #[test]
    fn an_update_restyles_a_comment_and_carries_the_look_it_replaced() -> Result<(), Box<dyn std::error::Error>> {
        let editor = ActorId::new("editor")?;
        let mut room = room_with(&editor)?;
        room.apply(&editor, &rectangle("r1", 0)?, 0)?;
        let page = PageRef { index: 0, epoch: 0 };
        let update = op(
            "style-1",
            ConflictClass::Merge,
            Target::Annotation { page, nm: "r1".into() },
            1,
            0,
            json!({"action": "annotation.update",
                   "style": {"color": [0.0, 0.0, 1.0], "opacity": 0.5, "width": 2.0, "fill": [0.0, 1.0, 0.0]}}),
        )?;
        let applied = room.apply(&editor, &update, 0)?;
        assert_eq!(applied.result["nm"], "r1");
        let style = style_of(room.adapter(), "r1")?;
        assert!(near(style.color, [0.0, 0.0, 1.0]), "the colour is the new one");
        assert!((style.opacity - 0.5).abs() < 1e-6);
        assert!((style.width.unwrap_or_default() - 2.0).abs() < 1e-6);
        assert!(near(style.fill, [0.0, 1.0, 0.0]), "the fill is the new one");

        // The accepted operation says what it replaced, and carries no `expect`:
        // that is the request's compare-and-set, not part of the result.
        let before = applied.op.payload.get("before").ok_or("the update records no before")?;
        assert_eq!(before["color"], json!([1.0, 0.0, 0.0]));
        assert_eq!(before["width"], json!(1.0));
        assert_eq!(before["fill"], json!(null), "there was no fill");
        assert!(applied.op.payload.get("expect").is_none());

        // Replay: the same accepted operation on a fresh replica yields the same look.
        let mut fresh = PdfAdapter::open("fixture.pdf", fixture(3).into())?;
        fresh.apply(&editor, &rectangle("r1", 0)?)?;
        fresh.apply(&editor, &applied.op)?;
        let replayed = style_of(&fresh, "r1")?;
        assert!(near(replayed.color, [0.0, 0.0, 1.0]));
        assert!((replayed.opacity - 0.5).abs() < 1e-6);
        assert!(near(replayed.fill, [0.0, 1.0, 0.0]));
        Ok(())
    }

    /// An update is refused before anything changes: a comment that is gone, and
    /// a property the comment does not have, leave the document exactly as it was.
    #[test]
    fn an_update_that_cannot_be_applied_changes_nothing() -> Result<(), Box<dyn std::error::Error>> {
        let editor = ActorId::new("editor")?;
        let mut room = room_with(&editor)?;
        room.apply(&editor, &note("n1", "hello", 0)?, 0)?;
        let page = PageRef { index: 0, epoch: 0 };

        let ghost = op(
            "style-ghost",
            ConflictClass::Merge,
            Target::Annotation { page, nm: "ghost".into() },
            1,
            0,
            json!({"action": "annotation.update", "style": {"color": [0.0, 0.0, 1.0]}}),
        )?;
        let rejected = room.apply(&editor, &ghost, 0).err().ok_or("an update to a missing comment was accepted")?;
        assert_eq!(rejected.code, ErrorCode::Conflict, "{}", rejected.message);

        // A sticky note has no fill and no width: the whole step is refused.
        let fill = op(
            "style-fill",
            ConflictClass::Merge,
            Target::Annotation { page, nm: "n1".into() },
            1,
            0,
            json!({"action": "annotation.update", "style": {"color": [0.0, 0.0, 1.0], "fill": [0.0, 1.0, 0.0]}}),
        )?;
        let rejected = room.apply(&editor, &fill, 0).err().ok_or("a fill on a note was accepted")?;
        assert_eq!(rejected.code, ErrorCode::Invalid, "{}", rejected.message);

        let document = room.adapter().document().ok_or("gone")?;
        assert_eq!(document.info.annotations.len(), 1);
        let style = style_of(room.adapter(), "n1")?;
        assert!(!near(style.color, [0.0, 0.0, 1.0]), "the colour did not change");
        assert_eq!(style.fill, None);
        Ok(())
    }

    /// The undo of a restyle puts the old look back, and only while the look it
    /// replaced is still there: someone else's later restyle is a conflict.
    #[test]
    fn an_undo_of_a_restyle_puts_the_old_look_back_unless_someone_restyled_since() -> Result<(), Box<dyn std::error::Error>> {
        let editor = ActorId::new("editor")?;
        let mut room = room_with(&editor)?;
        room.apply(&editor, &rectangle("r1", 0)?, 0)?;
        let page = PageRef { index: 0, epoch: 0 };

        let update = op(
            "style-1",
            ConflictClass::Merge,
            Target::Annotation { page, nm: "r1".into() },
            1,
            0,
            json!({"action": "annotation.update", "style": {"color": [0.0, 0.0, 1.0], "opacity": 0.5}}),
        )?;
        let applied = room.apply(&editor, &update, 0)?;
        let before = applied.op.payload.get("before").cloned().ok_or("no before")?;
        let style = applied.op.payload.get("style").cloned().ok_or("no style")?;

        // Someone restyles the same comment before the undo: the undo's
        // compare-and-set fails and their look is kept.
        let theirs = op(
            "style-2",
            ConflictClass::Merge,
            Target::Annotation { page, nm: "r1".into() },
            2,
            0,
            json!({"action": "annotation.update", "style": {"color": [0.0, 1.0, 0.0]}}),
        )?;
        room.apply(&editor, &theirs, 0)?;
        let undo = op(
            "undo-1",
            ConflictClass::Merge,
            Target::Annotation { page, nm: "r1".into() },
            3,
            0,
            json!({"action": "annotation.update", "style": before.clone(), "expect": style.clone()}),
        )?;
        let rejected = room.apply(&editor, &undo, 0).err().ok_or("the undo overwrote another restyle")?;
        assert_eq!(rejected.code, ErrorCode::Conflict);
        assert!(!rejected.retryable);
        assert!(near(style_of(room.adapter(), "r1")?.color, [0.0, 1.0, 0.0]));

        // With the look still in place, the undo restores what the update replaced.
        let plain = op(
            "style-3",
            ConflictClass::Merge,
            Target::Annotation { page, nm: "r1".into() },
            4,
            0,
            json!({"action": "annotation.update", "style": {"color": [0.25, 0.25, 0.25]}}),
        )?;
        let applied = room.apply(&editor, &plain, 0)?;
        let before = applied.op.payload["before"].clone();
        let style = applied.op.payload["style"].clone();
        let undo = op(
            "undo-2",
            ConflictClass::Merge,
            Target::Annotation { page, nm: "r1".into() },
            5,
            0,
            json!({"action": "annotation.update", "style": before, "expect": style}),
        )?;
        room.apply(&editor, &undo, 0)?;
        assert!(near(style_of(room.adapter(), "r1")?.color, [0.0, 1.0, 0.0]), "the look the update replaced is back");
        Ok(())
    }

    /// A reply can set the review status (`/State`, `/StateModel /Review`); the
    /// status survives a delete and restore, and a reply to a deleted comment is
    /// refused before anything is added.
    #[test]
    fn a_state_reply_sets_the_status_and_a_reply_to_a_deleted_comment_is_refused() -> Result<(), Box<dyn std::error::Error>> {
        let editor = ActorId::new("editor")?;
        let mut room = room_with(&editor)?;
        room.apply(&editor, &note("n1", "hello", 0)?, 0)?;
        let page = PageRef { index: 0, epoch: 0 };
        let reply = op(
            "reply-1",
            ConflictClass::Merge,
            Target::Annotation { page, nm: "n1".into() },
            1,
            0,
            json!({"action": "annotation.reply", "nm": "r1", "text": "sounds right", "author": DEFAULT_AUTHOR, "state": "accepted"}),
        )?;
        let applied = room.apply(&editor, &reply, 0)?;
        assert_eq!(applied.result["nm"], "r1");
        assert_eq!(applied.op.payload["state"], "accepted");
        let document = room.adapter().document().ok_or("gone")?;
        let state = document.info.annotations.iter().find(|a| a.name.as_deref() == Some("r1")).ok_or("the reply is gone")?;
        assert_eq!(state.state.as_deref(), Some("Accepted"), "the reply sets /State");
        assert_eq!(state.in_reply_to.as_deref(), Some("n1"));
        assert_eq!(state.contents.as_deref(), Some("Accepted set by editor"), "the status text is the engine's");

        // The status survives the delete: the copy carries the body, the restore
        // rebuilds the same state reply.
        let delete = op("del-1", ConflictClass::Merge, Target::Annotation { page, nm: "r1".into() }, 2, 0, json!({"action": "annotation.delete"}))?;
        let applied = room.apply(&editor, &delete, 0)?;
        let copy = applied.result["copy"].as_str().ok_or("the delete kept no copy")?.to_owned();
        let restore = op(
            "undo-1",
            ConflictClass::Merge,
            Target::Annotation { page, nm: "r1".into() },
            3,
            0,
            json!({"action": "annotation.restore", "copy": copy}),
        )?;
        room.apply(&editor, &restore, 0)?;
        let document = room.adapter().document().ok_or("gone")?;
        let back = document.info.annotations.iter().find(|a| a.name.as_deref() == Some("r1")).ok_or("the reply did not come back")?;
        assert_eq!(back.state.as_deref(), Some("Accepted"));

        // Deleting the parent takes its replies with it; a reply addressed to it
        // afterwards is refused, and no annotation is added.
        let delete =
            op("del-parent", ConflictClass::Merge, Target::Annotation { page, nm: "n1".into() }, 4, 0, json!({"action": "annotation.delete"}))?;
        room.apply(&editor, &delete, 0)?;
        let late = op(
            "reply-2",
            ConflictClass::Merge,
            Target::Annotation { page, nm: "n1".into() },
            5,
            0,
            json!({"action": "annotation.reply", "nm": "r2", "text": "too late", "author": DEFAULT_AUTHOR}),
        )?;
        let rejected = room.apply(&editor, &late, 0).err().ok_or("a reply to a deleted comment was accepted")?;
        assert_eq!(rejected.code, ErrorCode::Conflict, "{}", rejected.message);
        assert!(room.adapter().document().ok_or("gone")?.info.annotations.is_empty(), "nothing was added");
        Ok(())
    }
}
