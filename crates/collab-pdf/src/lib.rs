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
//! | `page.replace` | structural | page |
//! | `page.insert_blank` | structural | page (insert position) |
//! | `page.split` | structural | page (first page of the part) |
//! | `page.combine` | structural | page (insert position) |
//! | `outline.item.add` | structural | outline item (the parent; `null` = top level) |
//! | `outline.item.update` | structural | outline item (its object key) |
//! | `outline.item.delete` | structural | outline item (its object key) |
//! | `outline.item.restore` | structural | outline item (the parent of the copy) |
//! | `outline.item.move` | structural | outline item (its object key) |
//! | `dest.add` | structural | named destination (its name) |
//! | `dest.update` | structural | named destination (its name) |
//! | `dest.delete` | structural | named destination (its name) |
//!
//! A delete keeps a copy of what it removed (RFC §6): the accepted operation
//! answers with its `copy` token, and `annotation.restore` / `page.restore` put
//! that copy back as an ordinary operation, so every replica and the journal see
//! the same thing. The copies live in memory and are rebuilt by `replay`, not
//! written next to the journal: replaying the delete rebuilds the copy.
//!
//! An insert, a duplicate and a page replacement carry their own one-page copy
//! in the operation (`bytes`, base64): the page travels whole, so every replica
//! and the journal insert exactly the same content, and a page over
//! [`MAX_RESTORE_PAGE_BYTES`] is refused before the document changes.
//!
//! A replacement keeps the page it replaced, so its undo can put that page back
//! exactly: the accepted operation answers with the `copy` token, and an undo
//! addresses the same page with that token. When the replaced page did not fit
//! [`MAX_RESTORE_PAGE_BYTES`], no copy is kept and an undo (`"copy": null`) is
//! refused as `conflict` before the document changes. A blank insert names the
//! size of the page it adds (points), so every replica builds the same page.
//!
//! A split takes a range of pages out of the document and carries the part in
//! the accepted operation (`bytes`), so every replica and the journal can join
//! it back; the pages that stay are numbered deterministically and a split that
//! would leave no page behind is refused (`invalid`) before anything changes. A
//! combine inserts the part it carries at a position in one operation, so a
//! multi-page join is one version and one epoch step rather than one per page.
//! Both are bounded by [`MAX_RESTORE_PAGE_BYTES`]: a part over the budget is
//! refused with `limit` before the document changes.
//!
//! Structural operations bump the epoch; merge operations keep it. Annotations
//! are addressed by `/NM` (ADR-0002), never by a client-side index. Commit
//! writes the working file through `Session::save_bytes`, which keeps the
//! document valid and applies a full rewrite only where the edit requires it.
//!
//! The outline and its named destinations are structural too, and addressed by
//! the identity the file itself gives them: an item by its object reference (a
//! path is a position and positions move), a destination by its name. Every
//! operation resolves its address against the document before it writes, and the
//! accepted operation carries what it replaced, so an undo is an operation
//! rather than a guess. See `outline.rs` and RFC §3.1.7.

#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

use pdfcraft_collab_annot::{
    AnnotationInput, FontSizeUpdate, GeometryUpdate, InfoUpdate, MoveUpdate, PartsUpdate, PopupState, PopupUpdate, ReviewStateWire, StyleUpdate,
    WireError, rect_from_value, rect_matches,
};
use pdfcraft_collab_proto::{ActorId, BlobRef, ConflictClass, ErrorCode, MAX_ANNOTATION_NAME, Op, PageRef, Target};
use pdfcraft_collab_server::{Adapter, AdapterError, ApplyOutcome, CommitOutcome, Published};
use pdfcraft_engine::{DocId, Document, Edit, EditError, FieldConflicts, FieldValue, FormFieldKind, Session};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::collections::{HashMap, HashSet};
use std::sync::Arc;

mod outline;
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

/// The largest blank page a `page.insert_blank` may ask for, in points (200
/// inches, the limit of the PDF page size in ISO 32000-2 Annex C). A bigger
/// number is a client mistake, not a page.
const MAX_PAGE_POINTS: f64 = 14400.0;

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

    /// What the cabinet holds, as last agreed (see `Store::cabinet`). `version`
    /// is the cabinet's numbering of its copy: after an operator resolved a
    /// conflict those are the numbers the next publication expects.
    pub fn published(&self) -> Option<Published> {
        self.store.as_ref().and_then(Store::cabinet).map(|copy| Published { version: copy.expected_version(), etag: copy.etag.clone() })
    }

    /// The whole marker, both numberings. `None` when there is no store or no
    /// marker.
    pub fn cabinet(&self) -> Option<CabinetCopy> {
        self.store.as_ref().and_then(Store::cabinet).cloned()
    }

    /// Write the whole marker (see `Store::set_cabinet`). Without a store there
    /// is nowhere to keep it, so this is a no-op.
    pub fn set_cabinet(&mut self, copy: Option<CabinetCopy>) -> Result<(), AdapterError> {
        match self.store.as_mut() {
            Some(store) => store.set_cabinet(copy),
            None => Ok(()),
        }
    }

    /// Record what the cabinet holds (see `Store::set_cabinet`). The version is
    /// the cabinet's numbering; the room's own version of the published base is
    /// kept from the marker. Without a store there is nowhere to keep it, so
    /// this is a no-op.
    pub fn set_published(&mut self, published: Option<Published>) -> Result<(), AdapterError> {
        let base = self.cabinet().map_or(0, |copy| copy.version);
        let copy = published.map(|published| CabinetCopy { version: base, etag: published.etag, expected: Some(published.version) });
        self.set_cabinet(copy)
    }

    /// The file `commit` last wrote, for a publisher that has to send it on.
    pub fn last_commit(&self) -> Option<&Arc<Vec<u8>>> {
        self.last_commit.as_ref()
    }

    /// The version of the committed base `CURRENT` names: the last one written
    /// to disk, whether or not the cabinet took it. `0` without a store.
    pub fn committed_version(&self) -> u64 {
        self.store.as_ref().map_or(0, |store| store.base_state().0)
    }

    /// The bytes of the committed base, read back from the store. A publisher
    /// needs them after a restart (limitation 6), when the process that wrote
    /// them is gone and `last_commit` is empty. `None` without a store or
    /// before the first base.
    pub fn committed_base(&self) -> Result<Option<Vec<u8>>, AdapterError> {
        match self.store.as_ref() {
            Some(store) => store.base(),
            None => Ok(None),
        }
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
    /// line width, line endings, fill and border, the final look of the comment
    /// (RFC §3.1.2).
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

    /// Set the final font size of a text box, typewriter text or callout
    /// (RFC §3.1.3): one addressable operation over `Edit::FontSizeAnnotation`.
    ///
    /// The accepted operation carries the size it replaced (`before`), so an
    /// undo is a normal operation every replica and the journal can replay.
    fn update_font_size(&mut self, page: usize, name: &str, payload: &mut Value) -> Result<Value, AdapterError> {
        let update = FontSizeUpdate::from_payload(payload).map_err(map_wire)?;
        let index = self.annotation_index(page, name)?;
        let props = self
            .document_or_error()?
            .comment_props(page, index)
            .ok_or_else(|| AdapterError::new(ErrorCode::Conflict, "the annotation was deleted", false))?;
        update.check_for(&props).map_err(map_wire)?;
        // An undo names the size it left: if someone resized the text since,
        // putting the old size back would overwrite their edit (ADR-0010 §3).
        if let Some(expect) = payload.get("expect").filter(|value| !value.is_null()) {
            let expect = FontSizeUpdate::from_expect(expect).map_err(map_wire)?;
            if !expect.still_matches(&props) {
                return Err(AdapterError::new(ErrorCode::Conflict, "the text was resized since; the undo would overwrite it", false));
            }
        }
        let before = update.before(&props);
        self.apply_edit(Edit::FontSizeAnnotation { page, index, size: update.size })?;
        if let Value::Object(map) = payload {
            map.remove("expect");
        }
        payload["size"] = json!(update.size);
        if let Some(before) = before {
            payload["before"] = json!(before);
        }
        Ok(json!({ "nm": name }))
    }

    /// Set the final author, subject and note icon of a comment (RFC §3.1.3):
    /// one addressable operation over `Edit::SetAnnotationInfo`.
    ///
    /// `null` in `info` takes a property away (what the undo of a comment that
    /// had none says); an empty string is refused, so a request never wipes a
    /// name by accident. The accepted operation carries the properties it
    /// replaced (`before`), so the undo is a normal operation.
    fn update_info(&mut self, page: usize, name: &str, payload: &mut Value) -> Result<Value, AdapterError> {
        let update = InfoUpdate::from_payload(payload).map_err(map_wire)?;
        let index = self.annotation_index(page, name)?;
        let props = self
            .document_or_error()?
            .comment_props(page, index)
            .ok_or_else(|| AdapterError::new(ErrorCode::Conflict, "the annotation was deleted", false))?;
        update.check_for(&props).map_err(map_wire)?;
        if let Some(expect) = payload.get("expect").filter(|value| !value.is_null()) {
            let expect = InfoUpdate::from_value(expect).map_err(map_wire)?;
            if !expect.still_matches(&props) {
                return Err(AdapterError::new(ErrorCode::Conflict, "the comment was changed since; the undo would overwrite it", false));
            }
        }
        let before = update.before(&props);
        let (author, subject, icon) = update.apply_to();
        self.apply_edit(Edit::SetAnnotationInfo { page, index, author, subject, icon })?;
        if let Value::Object(map) = payload {
            map.remove("expect");
        }
        payload["info"] = update.to_value();
        payload["before"] = before.to_value();
        Ok(json!({ "nm": name }))
    }

    /// Set the fill's and the text's own opacities of a comment (RFC §3.1.6):
    /// one addressable operation over `Edit::PartOpacity`.
    ///
    /// The parts the update names are checked against the comment before
    /// anything changes; the accepted operation carries the numbers it replaced
    /// (`before`), so an undo is a normal operation every replica can replay.
    fn update_parts(&mut self, page: usize, name: &str, payload: &mut Value) -> Result<Value, AdapterError> {
        let update = PartsUpdate::from_payload(payload).map_err(map_wire)?;
        let index = self.annotation_index(page, name)?;
        let props = self
            .document_or_error()?
            .comment_props(page, index)
            .ok_or_else(|| AdapterError::new(ErrorCode::Conflict, "the annotation was deleted", false))?;
        update.check_for(&props).map_err(map_wire)?;
        // An undo names the parts it left: if someone changed them since, putting
        // the old ones back would overwrite their edit (ADR-0010 §3).
        if let Some(expect) = payload.get("expect").filter(|value| !value.is_null()) {
            let expect = PartsUpdate::from_value(expect).map_err(map_wire)?;
            if !expect.still_matches(&props) {
                return Err(AdapterError::new(ErrorCode::Conflict, "the comment's parts changed since; the undo would overwrite it", false));
            }
        }
        let before = update.before(&props);
        let (fill, text) = update.apply_to();
        self.apply_edit(Edit::PartOpacity { page, index, fill, text })?;
        if let Value::Object(map) = payload {
            map.remove("expect");
        }
        payload["parts"] = update.to_value();
        payload["before"] = before.to_value();
        Ok(json!({ "nm": name }))
    }

    /// Move a comment (RFC §3.1.6): one addressable operation over
    /// `Edit::MoveAnnotation`.
    ///
    /// The accepted operation carries the `/Rect` the move started from
    /// (`before`) and the one it left (`after`); an undo is a move by the
    /// opposite offset whose `expect` is `after`.
    fn move_comment(&mut self, page: usize, name: &str, payload: &mut Value) -> Result<Value, AdapterError> {
        let update = MoveUpdate::from_payload(payload).map_err(map_wire)?;
        let index = self.annotation_index(page, name)?;
        let props = self
            .document_or_error()?
            .comment_props(page, index)
            .ok_or_else(|| AdapterError::new(ErrorCode::Conflict, "the annotation was deleted", false))?;
        update.check_for(&props).map_err(map_wire)?;
        let before = update.before(&props).ok_or_else(|| AdapterError::new(ErrorCode::Invalid, "the comment's rectangle can't be read", false))?;
        // An undo names the place it left: if someone moved the comment since,
        // moving it back would overwrite their drag (ADR-0010 §3).
        if let Some(expect) = payload.get("expect").filter(|value| !value.is_null()) {
            let expect = rect_from_value(expect).map_err(map_wire)?;
            if !rect_matches(expect, props.rect) {
                return Err(AdapterError::new(ErrorCode::Conflict, "the comment moved since; the undo would overwrite it", false));
            }
        }
        self.apply_edit(Edit::MoveAnnotation { page, index, dx: update.dx, dy: update.dy })?;
        // What the engine wrote, not what arithmetic predicts: three decimals and
        // a sum are not the same number, and the undo's compare-and-set reads the
        // document back.
        let after = self.document_or_error()?.comment_props(page, index).and_then(|props| props.rect).unwrap_or_else(|| update.after(before));
        if let Value::Object(map) = payload {
            map.remove("expect");
        }
        payload["before"] = json!(before);
        payload["after"] = json!(after);
        Ok(json!({ "nm": name }))
    }

    /// Set the final geometry of a comment (RFC §3.1.6): a resize
    /// (`Edit::ResizeAnnotation`) or a reshape (`Edit::ReshapeAnnotation`), one
    /// addressable operation each.
    ///
    /// The accepted operation carries the geometry the engine drew before and
    /// after the edit (`before`, `after`), so an undo restores the old frame or
    /// points and refuses if someone changed the geometry since.
    fn update_geometry(&mut self, page: usize, name: &str, payload: &mut Value) -> Result<Value, AdapterError> {
        let update = GeometryUpdate::from_payload(payload).map_err(map_wire)?;
        let index = self.annotation_index(page, name)?;
        let props = self
            .document_or_error()?
            .comment_props(page, index)
            .ok_or_else(|| AdapterError::new(ErrorCode::Conflict, "the annotation was deleted", false))?;
        update.check_for(&props).map_err(map_wire)?;
        let before = update.before(&props).ok_or_else(|| AdapterError::new(ErrorCode::Invalid, "the comment's geometry can't be read", false))?;
        // An undo names the geometry it left: if someone resized or reshaped the
        // comment since, restoring the old one would overwrite their edit.
        if let Some(expect) = payload.get("expect").filter(|value| !value.is_null()) {
            let expect = GeometryUpdate::from_value(expect).map_err(map_wire)?;
            if !expect.still_matches(&props) {
                return Err(AdapterError::new(ErrorCode::Conflict, "the comment's geometry changed since; the undo would overwrite it", false));
            }
        }
        match &update {
            GeometryUpdate::Rect(rect) => self.apply_edit(Edit::ResizeAnnotation { page, index, rect: *rect })?,
            GeometryUpdate::Points(points) => self.apply_edit(Edit::ReshapeAnnotation { page, index, points: points.clone() })?,
        }
        let after = self
            .document_or_error()?
            .comment_props(page, index)
            .and_then(|props| update.observed(&props))
            .ok_or_else(|| AdapterError::new(ErrorCode::Invalid, "the comment's geometry can't be read after the edit", false))?;
        if let Value::Object(map) = payload {
            map.remove("expect");
        }
        payload["geometry"] = update.to_value();
        payload["before"] = before.to_value();
        payload["after"] = after.to_value();
        Ok(json!({ "nm": name }))
    }

    /// Open, close or move a comment's pop-up window (RFC §3.1.6): one
    /// addressable operation over `Edit::SetAnnotationPopup`.
    ///
    /// The accepted operation carries the window's state before (`null` when the
    /// comment had none, so the engine created one beside it) and after, read
    /// back from the document: the place a new window gets is the engine's, and
    /// the undo's compare-and-set must name the state the edit really left.
    fn update_popup(&mut self, page: usize, name: &str, payload: &mut Value) -> Result<Value, AdapterError> {
        let update = PopupUpdate::from_payload(payload).map_err(map_wire)?;
        let index = self.annotation_index(page, name)?;
        let props = self
            .document_or_error()?
            .comment_props(page, index)
            .ok_or_else(|| AdapterError::new(ErrorCode::Conflict, "the annotation was deleted", false))?;
        update.check_for(&props).map_err(map_wire)?;
        // An undo names the window it left: if someone moved or opened it since,
        // putting the old place or state back would overwrite their edit.
        if let Some(expect) = payload.get("expect").filter(|value| !value.is_null()) {
            let expect = PopupState::from_value(expect).map_err(map_wire)?;
            if !expect.still_matches(&props) {
                return Err(AdapterError::new(ErrorCode::Conflict, "the window changed since; the undo would overwrite it", false));
            }
        }
        let before = update.before(&props);
        let (rect, open) = update.apply_to();
        self.apply_edit(Edit::SetAnnotationPopup { page, index, rect, open })?;
        let after = self
            .document_or_error()?
            .comment_props(page, index)
            .and_then(|props| props.popup)
            .map(|popup| PopupState {
                rect: [f64::from(popup.rect[0]), f64::from(popup.rect[1]), f64::from(popup.rect[2]), f64::from(popup.rect[3])],
                open: popup.open,
            })
            .ok_or_else(|| AdapterError::new(ErrorCode::Invalid, "the comment's pop-up can't be read after the edit", false))?;
        if let Value::Object(map) = payload {
            map.remove("expect");
        }
        payload["popup"] = update.to_value();
        payload["before"] = before.map_or(Value::Null, |before| before.to_value());
        payload["after"] = after.to_value();
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

    /// Replace the content of `page` with the one-page copy the operation
    /// carries (RFC §3.1.4). The address is the page in the current epoch; a
    /// missing page is `invalid` and a foreign epoch `stale` (retryable). The
    /// payload is checked as a whole — base64, one page, the size budget —
    /// before the document changes, so a bad replacement is never half applied.
    ///
    /// What the page was is copied first, so the undo can put it back exactly
    /// (the `copy` token, like `page.delete`). A page over the budget is still
    /// replaceable; only its undo is refused, because there is no copy to go
    /// back to.
    fn replace_page(&mut self, page: usize, payload: &mut Value) -> Result<Value, AdapterError> {
        let bytes = match payload.get("bytes").filter(|value| value.is_string()) {
            Some(_) => Self::carried_page(payload)?,
            None => match payload.get("copy") {
                Some(Value::String(token)) => match &self.copy(token)?.kind {
                    DeletedKind::Page { bytes, .. } => {
                        if bytes.len() > MAX_RESTORE_PAGE_BYTES {
                            return Err(AdapterError::new(
                                ErrorCode::Limit,
                                format!("the page is {} bytes; one operation can carry at most {MAX_RESTORE_PAGE_BYTES}", bytes.len()),
                                false,
                            ));
                        }
                        bytes.as_slice().to_vec()
                    }
                    DeletedKind::Annotation { .. } => {
                        return Err(AdapterError::new(ErrorCode::Conflict, "the copy is a comment, not a page", false));
                    }
                },
                // The replacement kept no copy (the page was over the budget),
                // so its undo cannot name one: that is a refusal, not a guess.
                Some(Value::Null) => {
                    return Err(AdapterError::new(
                        ErrorCode::Conflict,
                        "the replaced page was too large to keep; the undo cannot put it back",
                        false,
                    ));
                }
                _ => return Err(AdapterError::new(ErrorCode::Invalid, "the operation carries no page", false)),
            },
        };
        Self::one_page_only(&bytes)?;
        // The copy is made before the page goes: afterwards its content is out
        // of the page and only the server still has it.
        let before = self.page_copy(page);
        self.apply_edit(Edit::ReplacePages {
            pages: vec![page],
            name: "replaced-page.pdf".into(),
            bytes: Arc::new(bytes.clone()),
            src_pages: vec![0],
        })?;
        self.epoch = self.epoch.saturating_add(1);
        // The accepted operation describes the page it put there, never the copy
        // token it was applied from: every replica and the journal replay the
        // same bytes (`page.restore` follows the same rule).
        if let Value::Object(map) = payload {
            map.remove("copy");
        }
        payload["bytes"] = json!(base64_encode(&bytes));
        let mut result = json!({ "page": page });
        if let Some((index, copy)) = before {
            let size = copy.len();
            let token = self.push_copy(DeletedKind::Page { index, bytes: copy }, size);
            result["copy"] = json!(token);
        }
        Ok(result)
    }

    /// Insert a blank page of the size the operation names at `at` (RFC §3.1.4).
    /// The size travels in the operation, so every replica builds the same page;
    /// a size that is not a positive finite number of points is `invalid` before
    /// anything changes.
    fn insert_blank(&mut self, at: usize, payload: &mut Value) -> Result<Value, AdapterError> {
        let size = |field: &str| payload.get(field).and_then(Value::as_f64);
        let (Some(width), Some(height)) = (size("width"), size("height")) else {
            return Err(AdapterError::new(ErrorCode::Invalid, "the blank page names no size", false));
        };
        if !width.is_finite() || !height.is_finite() || width <= 0.0 || height <= 0.0 || width > MAX_PAGE_POINTS || height > MAX_PAGE_POINTS {
            return Err(AdapterError::new(ErrorCode::Invalid, "the blank page size is out of range", false));
        }
        self.apply_edit(Edit::InsertBlankPage { at, width, height })?;
        self.epoch = self.epoch.saturating_add(1);
        payload["at"] = json!(at);
        payload["width"] = json!(width);
        payload["height"] = json!(height);
        Ok(json!({ "page": at }))
    }

    /// Take `count` pages starting at `from` out of the document (RFC §3.1.4).
    ///
    /// The boundary is the document's own: a range that starts outside it, runs
    /// past the last page, is empty, or would leave no page behind is `invalid`
    /// before anything changes. A part over [`MAX_RESTORE_PAGE_BYTES`] is
    /// `limit`, because the accepted operation has to fit one frame.
    ///
    /// The accepted operation carries the part (`bytes`), so every replica and
    /// the journal can join it back without a second channel; the pages that
    /// stay keep their order and are renumbered deterministically, and the
    /// result names both sides: `kept` pages in the document, `part` pages in
    /// the part, with the part's sha256.
    fn split_pages(&mut self, from: usize, payload: &mut Value) -> Result<Value, AdapterError> {
        let count = payload
            .get("count")
            .and_then(Value::as_u64)
            .and_then(|count| usize::try_from(count).ok())
            .ok_or_else(|| AdapterError::new(ErrorCode::Invalid, "the split names no page count", false))?;
        let pages = self.document_or_error()?.info.pages.len();
        if count == 0 {
            return Err(AdapterError::new(ErrorCode::Invalid, "a split takes at least one page", false));
        }
        let to = from
            .checked_add(count)
            .filter(|to| *to <= pages)
            .ok_or_else(|| AdapterError::new(ErrorCode::Invalid, "the split runs past the last page", false))?;
        if count == pages {
            return Err(AdapterError::new(ErrorCode::Invalid, "a split must leave a page in the document", false));
        }
        // The part is cut from the authoritative document, never taken from the
        // request: a client that named its own bytes could otherwise put content
        // into the document through an undo.
        let part = self.session.extract(self.id, &(from..to).collect::<Vec<_>>()).map_err(map_edit)?;
        if part.len() > MAX_RESTORE_PAGE_BYTES {
            return Err(AdapterError::new(
                ErrorCode::Limit,
                format!("the part is {} bytes; one operation can carry at most {MAX_RESTORE_PAGE_BYTES}", part.len()),
                false,
            ));
        }
        self.apply_edit(Edit::DeletePages { pages: (from..to).collect() })?;
        self.epoch = self.epoch.saturating_add(1);
        let kept = pages - count;
        // The accepted operation describes the part it took out, so every
        // replica and the journal can put it back (`page.combine`).
        payload["from"] = json!(from);
        payload["to"] = json!(to);
        payload["count"] = json!(count);
        payload["kept"] = json!(kept);
        payload["bytes"] = json!(base64_encode(part.as_slice()));
        Ok(json!({
            "from": from,
            "to": to,
            "count": count,
            "kept": kept,
            "part": { "sha256": sha256_hex(part.as_slice()), "len": part.len() },
        }))
    }

    /// Insert the part the operation carries at `at` in one step (RFC §3.1.4):
    /// the join of a split, and the in-room form of Combine Files. The pages
    /// travel whole, so every replica inserts the same document; the count the
    /// operation names must be the part's own page count, or the request is
    /// `invalid` before anything changes. `at` may be the page count (an
    /// append) and is past the last page otherwise.
    fn combine_pages(&mut self, at: usize, payload: &mut Value) -> Result<Value, AdapterError> {
        let encoded = payload
            .get("bytes")
            .and_then(Value::as_str)
            .ok_or_else(|| AdapterError::new(ErrorCode::Invalid, "the operation carries no part", false))?;
        let bytes = decode_base64(encoded)?;
        if bytes.len() > MAX_RESTORE_PAGE_BYTES {
            return Err(AdapterError::new(
                ErrorCode::Limit,
                format!("the part is {} bytes; one operation can carry at most {MAX_RESTORE_PAGE_BYTES}", bytes.len()),
                false,
            ));
        }
        let count = payload
            .get("count")
            .and_then(Value::as_u64)
            .and_then(|count| usize::try_from(count).ok())
            .filter(|count| *count > 0)
            .ok_or_else(|| AdapterError::new(ErrorCode::Invalid, "the combine names no page count", false))?;
        let carried = part_page_count(&bytes)?;
        if carried != count {
            return Err(AdapterError::new(ErrorCode::Invalid, format!("the part has {carried} pages but the operation names {count}"), false));
        }
        let before = self.document_or_error()?.info.pages.len();
        self.apply_edit(Edit::InsertPagesFrom {
            name: "combined-part.pdf".into(),
            bytes: Arc::new(bytes),
            pages: None,
            at,
            field_conflicts: FieldConflicts::Rename,
        })?;
        self.epoch = self.epoch.saturating_add(1);
        payload["at"] = json!(at);
        payload["count"] = json!(count);
        Ok(json!({ "at": at, "count": count, "pages": before + count }))
    }

    /// A replacement is exactly one page: a copy with two pages (or none) would
    /// put content into the document that nobody asked for. Checked in a scratch
    /// session, before the document changes.
    fn one_page_only(bytes: &[u8]) -> Result<(), AdapterError> {
        let mut scratch = Session::new();
        let id = scratch
            .open("replacement-page.pdf", None, Arc::new(bytes.to_vec()), None)
            .map_err(|e| AdapterError::new(ErrorCode::Invalid, format!("the replacement is not a readable PDF: {e}"), false))?;
        let count = scratch.get(id).map_or(0, |document| document.info.pages.len());
        if count != 1 {
            return Err(AdapterError::new(ErrorCode::Invalid, format!("a page replacement must carry exactly one page, not {count}"), false));
        }
        Ok(())
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
            // A check box's kids may each have an on state (ISO 32000-2
            // §12.7.5.2.3), so the wire's string is an on-state name at least as
            // often as a yes/no word; the engine reads both from a text value.
            FormFieldKind::CheckBox => FieldValue::Text(text.clone()),
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
            ("annotation.font_size", ConflictClass::Merge) => {
                let (page, name) = annotation_target(&effective)?;
                let page = self.page_index(page)?;
                self.update_font_size(page, &name, &mut effective.payload)?
            }
            ("annotation.info", ConflictClass::Merge) => {
                let (page, name) = annotation_target(&effective)?;
                let page = self.page_index(page)?;
                self.update_info(page, &name, &mut effective.payload)?
            }
            ("annotation.parts", ConflictClass::Merge) => {
                let (page, name) = annotation_target(&effective)?;
                let page = self.page_index(page)?;
                self.update_parts(page, &name, &mut effective.payload)?
            }
            ("annotation.move", ConflictClass::Merge) => {
                let (page, name) = annotation_target(&effective)?;
                let page = self.page_index(page)?;
                self.move_comment(page, &name, &mut effective.payload)?
            }
            ("annotation.geometry", ConflictClass::Merge) => {
                let (page, name) = annotation_target(&effective)?;
                let page = self.page_index(page)?;
                self.update_geometry(page, &name, &mut effective.payload)?
            }
            ("annotation.popup", ConflictClass::Merge) => {
                let (page, name) = annotation_target(&effective)?;
                let page = self.page_index(page)?;
                self.update_popup(page, &name, &mut effective.payload)?
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
            ("page.replace", ConflictClass::Structural) => {
                let page = self.page_index(target_page(&effective)?)?;
                self.replace_page(page, &mut effective.payload)?
            }
            ("page.insert_blank", ConflictClass::Structural) => {
                let page = target_page(&effective)?;
                let at = self.insert_position(page)?;
                self.insert_blank(at, &mut effective.payload)?
            }
            ("page.split", ConflictClass::Structural) => {
                let page = self.page_index(target_page(&effective)?)?;
                self.split_pages(page, &mut effective.payload)?
            }
            ("page.combine", ConflictClass::Structural) => {
                let page = target_page(&effective)?;
                let at = self.insert_position(page)?;
                self.combine_pages(at, &mut effective.payload)?
            }
            ("outline.item.add", ConflictClass::Structural) => self.add_outline_item(&effective.target, &mut effective.payload)?,
            ("outline.item.update", ConflictClass::Structural) => self.update_outline_item(&effective.target, &mut effective.payload)?,
            ("outline.item.delete", ConflictClass::Structural) => self.delete_outline_item(&effective.target, &mut effective.payload)?,
            ("outline.item.restore", ConflictClass::Structural) => self.restore_outline_item(&effective.target, &mut effective.payload)?,
            ("outline.item.move", ConflictClass::Structural) => self.move_outline_item(&effective.target, &mut effective.payload)?,
            ("dest.add", ConflictClass::Structural) => self.add_named_destination(&effective.target, &mut effective.payload)?,
            ("dest.update", ConflictClass::Structural) => self.update_named_destination(&effective.target, &mut effective.payload)?,
            ("dest.delete", ConflictClass::Structural) => self.delete_named_destination(&effective.target, &mut effective.payload)?,
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

    fn record(&mut self, version: u64, actor: &ActorId, op: &Op, recorded_ms: u64) -> Result<(), AdapterError> {
        let Some(store) = self.store.as_mut() else { return Ok(()) };
        store.append(version, actor, op, recorded_ms)
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
        WireError::UnsupportedShape(_) | WireError::NotRestylable(_) | WireError::NotMovable(_) | WireError::NotReshapable(_) => {
            ErrorCode::Unsupported
        }
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

/// How many pages a part carries. It is opened and closed before the document
/// changes: a file that is not a readable PDF, or one with no page, is
/// `invalid`, never a partial insert.
fn part_page_count(bytes: &[u8]) -> Result<usize, AdapterError> {
    let mut scratch = Session::new();
    let id = scratch
        .open("combined-part.pdf", None, Arc::new(bytes.to_vec()), None)
        .map_err(|e| AdapterError::new(ErrorCode::Invalid, format!("the part is not a readable PDF: {e}"), false))?;
    let count = scratch.get(id).map_or(0, |document| document.info.pages.len());
    if count == 0 {
        return Err(AdapterError::new(ErrorCode::Invalid, "the part has no pages", false));
    }
    Ok(count)
}

/// The author the adapter records when an operation names none.
pub const DEFAULT_AUTHOR: &str = "PdfCraft";

/// A synthetic document with `n` pages, for adapter and store tests.
#[cfg(test)]
pub(crate) fn fixture(n: usize) -> Vec<u8> {
    fixture_with_padding(n, None)
}

/// The same fixture with one page's content stream padded, so the page's
/// one-page copy can be pushed over [`MAX_RESTORE_PAGE_BYTES`] in a test.
#[cfg(test)]
fn fixture_with_padding(n: usize, padded: Option<(usize, usize)>) -> Vec<u8> {
    let mut objs: Vec<Vec<u8>> = vec![b"<< /Type /Catalog /Pages 2 0 R >>".to_vec()];
    let kids: Vec<String> = (0..n).map(|i| format!("{} 0 R", 4 + 2 * i)).collect();
    objs.push(format!("<< /Type /Pages /Kids [{}] /Count {n} /MediaBox [0 0 200 300] >>", kids.join(" ")).into_bytes());
    objs.push(b"<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>".to_vec());
    for i in 0..n {
        objs.push(format!("<< /Type /Page /Parent 2 0 R /Contents {} 0 R /Resources << /Font << /F1 3 0 R >> >> >>", 5 + 2 * i).into_bytes());
        let mut body = format!("BT /F1 24 Tf 20 150 Td (Page {}) Tj ET", i + 1);
        if let Some((page, size)) = padded
            && page == i
        {
            body.push_str(&" ".repeat(size));
        }
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
    use super::{DEFAULT_AUTHOR, MAX_RESTORE_PAGE_BYTES, PdfAdapter, PopupState, base64_encode, fixture, fixture_with_padding};
    use pdfcraft_collab_proto::{
        ActorId, ClientInfo, CommitResult, ConflictClass, DocKey, ErrorCode, Hello, Op, OpId, OpKind, PageRef, Role, SessionId, Target,
    };
    use pdfcraft_collab_server::{Adapter, Room, RoomConfig};
    use serde_json::{Value, json};

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

    /// A replacement keeps what it replaced, so the undo puts the page back
    /// exactly; the address is the page, and the page count does not move.
    #[test]
    fn a_replaced_page_comes_back_from_the_copy() -> Result<(), Box<dyn std::error::Error>> {
        let editor = ActorId::new("editor")?;
        let mut room = room_with(&editor)?;
        let (_, copy) = room.adapter().page_copy(2).ok_or("the fixture page copies")?;
        let replace = op(
            "rep-1",
            ConflictClass::Structural,
            Target::Page(PageRef { index: 0, epoch: 0 }),
            0,
            0,
            json!({"action": "page.replace", "bytes": base64_encode(copy.as_slice())}),
        )?;
        let applied = room.apply(&editor, &replace, 0)?;
        assert_eq!(applied.epoch, 1, "a replacement is structural");
        assert!(applied.op.payload["bytes"].is_string(), "the accepted operation carries the page");
        assert_eq!(applied.op.payload["copy"], applied.result["copy"], "the accepted operation names the copy it kept");
        let token = applied.result["copy"].as_str().ok_or("the replacement kept no copy")?.to_owned();
        assert_eq!(room.adapter().document().ok_or("gone")?.info.pages.len(), 3, "the page count did not move");
        let version = room.version();
        commit(&mut room, &editor, version)?;
        let bytes = room.adapter().last_commit().cloned().ok_or("no commit bytes")?;
        assert_eq!(page_text(&bytes, 0)?, "Page 3", "the page is the copy the client sent");

        // Undo: the same address, the copy the server kept, and the page as it
        // was before the replacement — byte for byte the content it had.
        let undo = op(
            "undo-1",
            ConflictClass::Structural,
            Target::Page(PageRef { index: 0, epoch: applied.epoch }),
            applied.version,
            applied.epoch,
            json!({"action": "page.replace", "copy": token}),
        )?;
        let undone = room.apply(&editor, &undo, 0)?;
        assert_eq!(undone.epoch, 2);
        assert!(undone.op.payload["bytes"].is_string(), "the undo describes the page it put back");
        assert_ne!(undone.op.payload["copy"], json!(token), "the undo keeps what it replaced itself, not the token it named");
        let version = room.version();
        commit(&mut room, &editor, version)?;
        let bytes = room.adapter().last_commit().cloned().ok_or("no commit bytes")?;
        assert_eq!(page_text(&bytes, 0)?, "Page 1", "the replaced page came back exactly");
        Ok(())
    }

    /// A replacement is one page and within the budget: a payload the operation
    /// cannot carry is refused before the document changes.
    #[test]
    fn a_replacement_that_is_not_one_page_is_refused_before_the_document_changes() -> Result<(), Box<dyn std::error::Error>> {
        let editor = ActorId::new("editor")?;
        let mut room = room_with(&editor)?;
        let two = fixture(2);
        let cases = [("bad-b64", base64_encode(b"not a pdf at all")), ("two-pages", base64_encode(&two)), ("empty", String::new())];
        for (id, bytes) in cases {
            let replace = op(
                id,
                ConflictClass::Structural,
                Target::Page(PageRef { index: 0, epoch: 0 }),
                0,
                0,
                json!({"action": "page.replace", "bytes": bytes}),
            )?;
            let rejected = room.apply(&editor, &replace, 0).err().ok_or("a replacement that is not one page was accepted")?;
            assert_eq!(rejected.code, ErrorCode::Invalid, "{id}");
            assert!(!rejected.retryable, "{id}");
        }
        let missing =
            op("missing", ConflictClass::Structural, Target::Page(PageRef { index: 0, epoch: 0 }), 0, 0, json!({"action": "page.replace"}))?;
        assert_eq!(room.apply(&editor, &missing, 0).err().ok_or("an empty replacement was accepted")?.code, ErrorCode::Invalid);
        assert_eq!(room.adapter().document().ok_or("gone")?.info.pages.len(), 3, "nothing was replaced");
        assert_eq!(room.epoch(), 0, "the epoch did not move");
        assert_eq!(room.version(), 0, "nothing was accepted");
        Ok(())
    }

    /// A replacement carries the page, so a page over the budget is refused
    /// before the document changes: no partial replacement, no epoch bump, no
    /// journal entry.
    #[test]
    fn a_replacement_over_the_limit_is_refused_before_the_document_changes() -> Result<(), Box<dyn std::error::Error>> {
        let editor = ActorId::new("editor")?;
        let mut room = room_with(&editor)?;
        let huge = vec![0u8; MAX_RESTORE_PAGE_BYTES + 1];
        let replace = op(
            "rep-1",
            ConflictClass::Structural,
            Target::Page(PageRef { index: 0, epoch: 0 }),
            0,
            0,
            json!({"action": "page.replace", "bytes": base64_encode(&huge)}),
        )?;
        let rejected = room.apply(&editor, &replace, 0).err().ok_or("an oversized page was accepted")?;
        assert_eq!(rejected.code, ErrorCode::Limit);
        assert!(!rejected.retryable);
        assert_eq!(room.adapter().document().ok_or("gone")?.info.pages.len(), 3, "nothing was replaced");
        assert_eq!(room.epoch(), 0, "the epoch did not move");
        assert_eq!(room.version(), 0, "nothing was accepted");
        let version = room.version();
        commit(&mut room, &editor, version)?;
        let bytes = room.adapter().last_commit().cloned().ok_or("no commit bytes")?;
        assert_eq!(page_text(&bytes, 0)?, "Page 1", "the page is untouched");
        Ok(())
    }

    /// The page the replacement removed was too large to keep, so no copy token
    /// comes back; the undo that names none is a refusal, and the document and
    /// the version are left as they were.
    #[test]
    fn the_undo_of_a_replacement_without_a_copy_is_a_conflict() -> Result<(), Box<dyn std::error::Error>> {
        let editor = ActorId::new("editor")?;
        let bytes = fixture_with_padding(2, Some((0, MAX_RESTORE_PAGE_BYTES + 4096)));
        let adapter = PdfAdapter::open("fixture.pdf", bytes.into())?;
        let mut room = Room::new(SessionId::new("session-1")?, DocKey::new("file-1")?, adapter, RoomConfig::default());
        room.hello(editor.clone(), Role::Editor, &hello()?, 0)?;
        let (_, copy) = room.adapter().page_copy(1).ok_or("the small page copies")?;
        let replace = op(
            "rep-1",
            ConflictClass::Structural,
            Target::Page(PageRef { index: 0, epoch: 0 }),
            0,
            0,
            json!({"action": "page.replace", "bytes": base64_encode(copy.as_slice())}),
        )?;
        let applied = room.apply(&editor, &replace, 0)?;
        assert!(applied.result.get("copy").is_none(), "the big page was not kept, so there is no token");
        assert!(applied.op.payload.get("copy").is_none(), "and the accepted operation names none");
        let undo = op(
            "undo-1",
            ConflictClass::Structural,
            Target::Page(PageRef { index: 0, epoch: applied.epoch }),
            applied.version,
            applied.epoch,
            json!({"action": "page.replace", "copy": null}),
        )?;
        let rejected = room.apply(&editor, &undo, 0).err().ok_or("an undo with no copy was accepted")?;
        assert_eq!(rejected.code, ErrorCode::Conflict);
        assert!(!rejected.retryable);
        assert_eq!(room.version(), 1, "the refusal moved nothing");
        assert_eq!(room.epoch(), 1);
        assert_eq!(room.adapter().document().ok_or("gone")?.info.pages.len(), 2, "the replacement itself stays");
        Ok(())
    }

    /// A replacement addressed to a passed epoch is retryable and changes
    /// nothing: the client resyncs and re-addresses the page.
    #[test]
    fn a_replacement_addressed_to_another_epoch_is_retryable() -> Result<(), Box<dyn std::error::Error>> {
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
        let (_, copy) = room.adapter().page_copy(1).ok_or("the fixture page copies")?;
        let replace = op(
            "rep-1",
            ConflictClass::Structural,
            Target::Page(PageRef { index: 0, epoch: 0 }),
            turned.version,
            turned.epoch,
            json!({"action": "page.replace", "bytes": base64_encode(copy.as_slice())}),
        )?;
        let rejected = room.apply(&editor, &replace, 0).err().ok_or("a foreign epoch was accepted")?;
        assert_eq!(rejected.code, ErrorCode::Stale);
        assert!(rejected.retryable);
        assert_eq!(room.version(), 1, "the stale replacement moved nothing");
        Ok(())
    }

    /// A blank page is built from the size the operation names, at the position
    /// it names; a size that is not a positive number of points is refused.
    #[test]
    fn a_blank_page_is_inserted_with_the_size_it_names() -> Result<(), Box<dyn std::error::Error>> {
        let editor = ActorId::new("editor")?;
        let mut room = room_with(&editor)?;
        let insert = op(
            "blank-1",
            ConflictClass::Structural,
            Target::Page(PageRef { index: 1, epoch: 0 }),
            0,
            0,
            json!({"action": "page.insert_blank", "width": 300.0, "height": 400.0}),
        )?;
        let applied = room.apply(&editor, &insert, 0)?;
        assert_eq!(applied.epoch, 1);
        assert_eq!(applied.op.payload["at"], 1, "the accepted operation names the position");
        let document = room.adapter().document().ok_or("gone")?;
        assert_eq!(document.info.pages.len(), 4);
        let crop = document.info.pages[1].crop;
        assert_eq!((crop[2] - crop[0]).abs(), 300.0, "the width is the one the operation named");
        assert_eq!((crop[3] - crop[1]).abs(), 400.0);
        let version = room.version();
        commit(&mut room, &editor, version)?;
        let bytes = room.adapter().last_commit().cloned().ok_or("no commit bytes")?;
        assert_eq!(page_text(&bytes, 0)?, "Page 1", "the pages before it did not move");

        // The undo of a blank insert is a delete of the page it put there.
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

        for (id, payload) in [
            ("zero", json!({"action": "page.insert_blank", "width": 0.0, "height": 400.0})),
            ("negative", json!({"action": "page.insert_blank", "width": 300.0, "height": -1.0})),
            ("huge", json!({"action": "page.insert_blank", "width": 300.0, "height": 1e9})),
            ("nan", json!({"action": "page.insert_blank", "width": "wide", "height": 400.0})),
        ] {
            let bad = op(id, ConflictClass::Structural, Target::Page(PageRef { index: 0, epoch: 2 }), 2, 2, payload)?;
            let rejected = room.apply(&editor, &bad, 0).err().ok_or("a bad blank page size was accepted")?;
            assert_eq!(rejected.code, ErrorCode::Invalid, "{id}");
        }
        assert_eq!(room.adapter().document().ok_or("gone")?.info.pages.len(), 3, "no bad blank page was added");
        assert_eq!(room.version(), 2, "a refusal accepts nothing");
        Ok(())
    }

    /// The accepted operation carries the page, so a restart replays the
    /// replacement without any client file or in-memory copy.
    #[test]
    fn a_replacement_replays_from_its_operation() -> Result<(), Box<dyn std::error::Error>> {
        let editor = ActorId::new("editor")?;
        let mut room = room_with(&editor)?;
        let (_, copy) = room.adapter().page_copy(2).ok_or("the fixture page copies")?;
        let replace = op(
            "rep-1",
            ConflictClass::Structural,
            Target::Page(PageRef { index: 0, epoch: 0 }),
            0,
            0,
            json!({"action": "page.replace", "bytes": base64_encode(copy.as_slice())}),
        )?;
        let applied = room.apply(&editor, &replace, 0)?.op;
        let adapter = PdfAdapter::open("fixture.pdf", fixture(3).into())?;
        let mut fresh = Room::new(SessionId::new("session-2")?, DocKey::new("file-1")?, adapter, RoomConfig::default());
        fresh.hello(editor.clone(), Role::Editor, &hello()?, 0)?;
        fresh.replay(editor.clone(), applied, 1, 0)?;
        assert_eq!(fresh.epoch(), 1);
        assert_eq!(fresh.adapter().document().ok_or("gone")?.info.pages.len(), 3);
        let version = fresh.version();
        commit(&mut fresh, &editor, version)?;
        let bytes = fresh.adapter().last_commit().cloned().ok_or("no commit bytes")?;
        assert_eq!(page_text(&bytes, 0)?, "Page 3", "the journal's copy replaced the page");
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
        fresh.replay(editor.clone(), applied, 1, 0)?;
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
            fresh.replay(editor.clone(), op, version as u64 + 1, 0)?;
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

    /// A comment's window: where it sits and whether it is open, read back from
    /// the document.
    fn window_of(adapter: &PdfAdapter, nm: &str) -> Result<Option<PopupState>, Box<dyn std::error::Error>> {
        Ok(style_of(adapter, nm)?.popup.map(|popup| PopupState {
            rect: [f64::from(popup.rect[0]), f64::from(popup.rect[1]), f64::from(popup.rect[2]), f64::from(popup.rect[3])],
            open: popup.open,
        }))
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

    /// Line endings travel in the style object of `annotation.update`: one
    /// operation sets a line's two ends, the accepted operation records the pair
    /// it replaced, and a fresh replica replaying it draws the same line.
    #[test]
    fn an_update_sets_line_endings_with_the_rest_of_the_look() -> Result<(), Box<dyn std::error::Error>> {
        let editor = ActorId::new("editor")?;
        let mut room = room_with(&editor)?;
        let page = PageRef { index: 0, epoch: 0 };
        let add = op(
            "add-line",
            ConflictClass::Merge,
            Target::Page(page),
            0,
            0,
            json!({"action": "annotation.add", "nm": "l1",
                   "shape": {"kind": "line", "from": [10.0, 10.0], "to": [90.0, 60.0],
                             "start": "none", "end": "open_arrow"},
                   "style": {"color": [1.0, 0.0, 0.0], "opacity": 1.0, "width": 1.0},
                   "contents": "", "author": DEFAULT_AUTHOR}),
        )?;
        room.apply(&editor, &add, 0)?;

        let update = op(
            "style-endings",
            ConflictClass::Merge,
            Target::Annotation { page, nm: "l1".into() },
            1,
            0,
            json!({"action": "annotation.update", "style": {"endings": ["square", "closed_arrow"]}}),
        )?;
        let applied = room.apply(&editor, &update, 0)?;
        assert_eq!(applied.op.payload["style"]["endings"], json!(["square", "closed_arrow"]));
        assert_eq!(applied.op.payload["before"]["endings"], json!(["none", "open_arrow"]), "the pair it replaced");
        let props = style_of(room.adapter(), "l1")?;
        assert_eq!(props.endings, Some(vec![pdfcraft_engine::LineEnding::Square, pdfcraft_engine::LineEnding::ClosedArrow]));

        // The undo names the pair it left and only applies while it is there.
        let before = applied.op.payload["before"].clone();
        let style = applied.op.payload["style"].clone();
        let theirs = op(
            "style-endings-2",
            ConflictClass::Merge,
            Target::Annotation { page, nm: "l1".into() },
            2,
            0,
            json!({"action": "annotation.update", "style": {"endings": ["butt", "butt"]}}),
        )?;
        room.apply(&editor, &theirs, 0)?;
        let undo = op(
            "undo-endings",
            ConflictClass::Merge,
            Target::Annotation { page, nm: "l1".into() },
            3,
            0,
            json!({"action": "annotation.update", "style": before, "expect": style}),
        )?;
        let refused = room.apply(&editor, &undo, 0).err().ok_or("the undo overwrote another pair of endings")?;
        assert_eq!(refused.code, ErrorCode::Conflict);
        assert_eq!(style_of(room.adapter(), "l1")?.endings, Some(vec![pdfcraft_engine::LineEnding::Butt, pdfcraft_engine::LineEnding::Butt]));

        // Replay: the accepted operation on a fresh replica yields the same line.
        let mut fresh = PdfAdapter::open("fixture.pdf", fixture(3).into())?;
        fresh.apply(&editor, &add)?;
        fresh.apply(&editor, &applied.op)?;
        assert_eq!(style_of(&fresh, "l1")?.endings, Some(vec![pdfcraft_engine::LineEnding::Square, pdfcraft_engine::LineEnding::ClosedArrow]));
        Ok(())
    }

    /// A sticky note has no endings: the whole style update is refused before the
    /// document changes, and the version does not move.
    #[test]
    fn endings_on_a_comment_that_has_none_change_nothing() -> Result<(), Box<dyn std::error::Error>> {
        let editor = ActorId::new("editor")?;
        let mut room = room_with(&editor)?;
        room.apply(&editor, &note("n1", "hello", 0)?, 0)?;
        let page = PageRef { index: 0, epoch: 0 };
        let version = room.version();
        let update = op(
            "style-endings-note",
            ConflictClass::Merge,
            Target::Annotation { page, nm: "n1".into() },
            version,
            0,
            json!({"action": "annotation.update", "style": {"color": [0.0, 0.0, 1.0], "endings": ["square", "slash"]}}),
        )?;
        let refused = room.apply(&editor, &update, 0).err().ok_or("endings on a note were accepted")?;
        assert_eq!(refused.code, ErrorCode::Invalid, "{}", refused.message);
        assert_eq!(room.version(), version, "a refused operation does not move the version");
        let props = style_of(room.adapter(), "n1")?;
        assert_eq!(props.endings, None);
        assert!(!near(props.color, [0.0, 0.0, 1.0]), "the colour did not change either");
        Ok(())
    }

    /// The font size of a text box is one addressable operation: the accepted
    /// operation carries the size it replaced, a fresh replica replays it, and an
    /// undo is refused when someone resized the text since — without a change.
    #[test]
    fn a_font_size_update_resizes_text_and_undoes_as_one_operation() -> Result<(), Box<dyn std::error::Error>> {
        let editor = ActorId::new("editor")?;
        let mut room = room_with(&editor)?;
        let page = PageRef { index: 0, epoch: 0 };
        let add = op(
            "add-box",
            ConflictClass::Merge,
            Target::Page(page),
            0,
            0,
            json!({"action": "annotation.add", "nm": "t1",
                   "shape": {"kind": "text_box", "rect": [10.0, 100.0, 180.0, 160.0], "font_size": 10.0},
                   "style": {"color": [0.0, 0.0, 0.0], "opacity": 1.0, "width": 1.0},
                   "contents": "Some text", "author": DEFAULT_AUTHOR}),
        )?;
        room.apply(&editor, &add, 0)?;
        let props = style_of(room.adapter(), "t1")?;
        assert_eq!(props.font_size, Some(10.0));

        let resize = op(
            "size-1",
            ConflictClass::Merge,
            Target::Annotation { page, nm: "t1".into() },
            1,
            0,
            json!({"action": "annotation.font_size", "size": 18.0}),
        )?;
        let applied = room.apply(&editor, &resize, 0)?;
        assert_eq!(applied.op.payload["size"], json!(18.0));
        assert_eq!(applied.op.payload["before"], json!(10.0), "the size it replaced");
        assert_eq!(style_of(room.adapter(), "t1")?.font_size, Some(18.0));

        // Replay on a fresh replica.
        let mut fresh = PdfAdapter::open("fixture.pdf", fixture(3).into())?;
        fresh.apply(&editor, &add)?;
        fresh.apply(&editor, &applied.op)?;
        assert_eq!(style_of(&fresh, "t1")?.font_size, Some(18.0));

        // Someone resizes it first: the undo's compare-and-set fails and their size stays.
        let theirs = op(
            "size-2",
            ConflictClass::Merge,
            Target::Annotation { page, nm: "t1".into() },
            2,
            0,
            json!({"action": "annotation.font_size", "size": 22.0}),
        )?;
        room.apply(&editor, &theirs, 0)?;
        let version = room.version();
        let undo = op(
            "undo-size",
            ConflictClass::Merge,
            Target::Annotation { page, nm: "t1".into() },
            version,
            0,
            json!({"action": "annotation.font_size", "size": 10.0, "expect": 18.0}),
        )?;
        let refused = room.apply(&editor, &undo, 0).err().ok_or("the undo overwrote another size")?;
        assert_eq!(refused.code, ErrorCode::Conflict);
        assert_eq!(room.version(), version);
        assert_eq!(style_of(room.adapter(), "t1")?.font_size, Some(22.0));

        // With the size in place, the undo puts the old one back.
        let current = room.version();
        let undo = op(
            "undo-size-2",
            ConflictClass::Merge,
            Target::Annotation { page, nm: "t1".into() },
            current,
            0,
            json!({"action": "annotation.font_size", "size": 10.0, "expect": 22.0}),
        )?;
        room.apply(&editor, &undo, 0)?;
        assert_eq!(style_of(room.adapter(), "t1")?.font_size, Some(10.0));
        Ok(())
    }

    /// A note's icon and any comment's author and subject are one addressable
    /// operation; an empty subject is refused before the document changes and the
    /// version stays put.
    #[test]
    fn an_info_update_sets_the_note_icon_and_the_names_or_changes_nothing() -> Result<(), Box<dyn std::error::Error>> {
        let editor = ActorId::new("editor")?;
        let mut room = room_with(&editor)?;
        room.apply(&editor, &note("n1", "hello", 0)?, 0)?;
        let page = PageRef { index: 0, epoch: 0 };

        let update = op(
            "info-1",
            ConflictClass::Merge,
            Target::Annotation { page, nm: "n1".into() },
            1,
            0,
            json!({"action": "annotation.info", "info": {"author": "ada", "subject": "Review", "icon": "help"}}),
        )?;
        let applied = room.apply(&editor, &update, 0)?;
        assert_eq!(applied.op.payload["info"], json!({"author": "ada", "subject": "Review", "icon": "help"}));
        let before = applied.op.payload["before"].clone();
        assert_eq!(before["author"], json!("editor"), "the author it replaced (the token's actor)");
        assert_eq!(before["subject"], json!("Sticky Note"), "the note's own subject");
        assert_eq!(before["icon"], json!("comment"), "the icon the note drew");
        let props = style_of(room.adapter(), "n1")?;
        assert_eq!(props.author, "ada");
        assert_eq!(props.subject, "Review");
        assert_eq!(props.icon, Some(pdfcraft_engine::NoteIcon::Help));

        // An empty subject and an icon on a comment that is not a note are refused whole.
        let version = room.version();
        let empty = op(
            "info-empty",
            ConflictClass::Merge,
            Target::Annotation { page, nm: "n1".into() },
            version,
            0,
            json!({"action": "annotation.info", "info": {"author": "bo", "subject": ""}}),
        )?;
        let refused = room.apply(&editor, &empty, 0).err().ok_or("an empty subject was accepted")?;
        assert_eq!(refused.code, ErrorCode::Invalid, "{}", refused.message);
        assert_eq!(room.version(), version);
        assert_eq!(style_of(room.adapter(), "n1")?.author, "ada", "the author did not change either");

        room.apply(&editor, &rectangle("r1", room.version())?, 0)?;
        let version = room.version();
        let icon = op(
            "info-icon",
            ConflictClass::Merge,
            Target::Annotation { page, nm: "r1".into() },
            version,
            0,
            json!({"action": "annotation.info", "info": {"icon": "star"}}),
        )?;
        let refused = room.apply(&editor, &icon, 0).err().ok_or("an icon on a rectangle was accepted")?;
        assert_eq!(refused.code, ErrorCode::Invalid);

        // The undo names what it replaced (`null` removes the subject again).
        let current = room.version();
        let undo = op(
            "undo-info",
            ConflictClass::Merge,
            Target::Annotation { page, nm: "n1".into() },
            current,
            0,
            json!({"action": "annotation.info", "info": before, "expect": {"author": "ada", "subject": "Review", "icon": "help"}}),
        )?;
        let applied = room.apply(&editor, &undo, 0)?;
        let props = style_of(room.adapter(), "n1")?;
        assert_eq!(props.author, "editor", "the author the update replaced is back");
        assert_eq!(props.subject, "Sticky Note", "the note's own subject is back");
        assert_eq!(props.icon, Some(pdfcraft_engine::NoteIcon::Comment));
        assert_eq!(applied.op.payload["info"]["subject"], json!("Sticky Note"));

        // Replay: the same accepted operations on a fresh replica.
        let mut fresh = PdfAdapter::open("fixture.pdf", fixture(3).into())?;
        fresh.apply(&editor, &note("n1", "hello", 0)?)?;
        fresh.apply(&editor, &update)?;
        fresh.apply(&editor, &applied.op)?;
        let replayed = style_of(&fresh, "n1")?;
        assert_eq!(replayed.author, "editor");
        assert_eq!(replayed.subject, "Sticky Note");
        Ok(())
    }

    /// A comment's fill and text have opacities of their own: `annotation.parts`
    /// sets them as one operation, a fresh replica replays it, and an undo is
    /// refused when someone changed a part since — without a change.
    #[test]
    fn a_parts_update_sets_the_fill_and_text_opacities_and_undoes_as_one_operation() -> Result<(), Box<dyn std::error::Error>> {
        let editor = ActorId::new("editor")?;
        let mut room = room_with(&editor)?;
        let page = PageRef { index: 0, epoch: 0 };
        let add = op(
            "add-box",
            ConflictClass::Merge,
            Target::Page(page),
            0,
            0,
            json!({"action": "annotation.add", "nm": "t1",
                   "shape": {"kind": "text_box", "rect": [10.0, 100.0, 180.0, 160.0], "font_size": 10.0},
                   "style": {"color": [0.0, 0.0, 0.0], "opacity": 1.0, "width": 1.0, "fill": [0.0, 1.0, 0.0]},
                   "contents": "Some text", "author": DEFAULT_AUTHOR}),
        )?;
        room.apply(&editor, &add, 0)?;
        let props = style_of(room.adapter(), "t1")?;
        assert_eq!(props.fill_opacity, Some(1.0));
        assert_eq!(props.text_opacity, Some(1.0));

        let parts = op(
            "parts-1",
            ConflictClass::Merge,
            Target::Annotation { page, nm: "t1".into() },
            1,
            0,
            json!({"action": "annotation.parts", "parts": {"fill": 0.4, "text": 0.7}}),
        )?;
        let applied = room.apply(&editor, &parts, 0)?;
        assert_eq!(applied.op.payload["parts"], json!({"fill": 0.4, "text": 0.7}));
        assert_eq!(applied.op.payload["before"], json!({"fill": 1.0, "text": 1.0}), "the opacities it replaced");
        assert!(applied.op.payload.get("expect").is_none(), "the accepted operation carries no compare-and-set");
        let props = style_of(room.adapter(), "t1")?;
        assert!((props.fill_opacity.unwrap_or_default() - 0.4).abs() < 1e-6);
        assert!((props.text_opacity.unwrap_or_default() - 0.7).abs() < 1e-6);

        // A rectangle has a fill but no text: the whole update is refused, and the version stays.
        room.apply(&editor, &rectangle("r1", room.version())?, 0)?;
        let version = room.version();
        let no_text = op(
            "parts-text-rect",
            ConflictClass::Merge,
            Target::Annotation { page, nm: "r1".into() },
            version,
            0,
            json!({"action": "annotation.parts", "parts": {"text": 0.5}}),
        )?;
        let refused = room.apply(&editor, &no_text, 0).err().ok_or("a text opacity on a rectangle was accepted")?;
        assert_eq!(refused.code, ErrorCode::Invalid, "{}", refused.message);
        assert_eq!(room.version(), version, "a refused operation does not move the version");

        // Someone changes a part: the undo's compare-and-set fails and their value stays.
        let theirs = op(
            "parts-theirs",
            ConflictClass::Merge,
            Target::Annotation { page, nm: "t1".into() },
            room.version(),
            0,
            json!({"action": "annotation.parts", "parts": {"text": 0.9}}),
        )?;
        room.apply(&editor, &theirs, 0)?;
        let version = room.version();
        let undo = op(
            "undo-parts",
            ConflictClass::Merge,
            Target::Annotation { page, nm: "t1".into() },
            version,
            0,
            json!({"action": "annotation.parts", "parts": {"fill": 1.0, "text": 1.0}, "expect": {"fill": 0.4, "text": 0.7}}),
        )?;
        let refused = room.apply(&editor, &undo, 0).err().ok_or("the undo overwrote another opacity")?;
        assert_eq!(refused.code, ErrorCode::Conflict);
        assert_eq!(room.version(), version);
        assert!((style_of(room.adapter(), "t1")?.text_opacity.unwrap_or_default() - 0.9).abs() < 1e-6);

        // With the parts as the operation left them, the undo puts the old ones back.
        let undo = op(
            "undo-parts-2",
            ConflictClass::Merge,
            Target::Annotation { page, nm: "t1".into() },
            room.version(),
            0,
            json!({"action": "annotation.parts", "parts": {"fill": 1.0, "text": 1.0}, "expect": {"fill": 0.4, "text": 0.9}}),
        )?;
        let undone = room.apply(&editor, &undo, 0)?;
        let props = style_of(room.adapter(), "t1")?;
        assert_eq!(props.fill_opacity, Some(1.0));
        assert_eq!(props.text_opacity, Some(1.0));

        // Replay on a fresh replica.
        let mut fresh = PdfAdapter::open("fixture.pdf", fixture(3).into())?;
        fresh.apply(&editor, &add)?;
        fresh.apply(&editor, &applied.op)?;
        fresh.apply(&editor, &undone.op)?;
        let replayed = style_of(&fresh, "t1")?;
        assert_eq!(replayed.fill_opacity, Some(1.0));
        assert_eq!(replayed.text_opacity, Some(1.0));
        Ok(())
    }

    /// A comment's pop-up window is an addressable operation: `annotation.popup`
    /// opens, closes or moves it, a fresh replica replays it, the accepted
    /// operation carries the window's state before and after, and an undo is
    /// refused when someone moved the window since — without a change.
    #[test]
    fn a_popup_window_is_opened_moved_and_undone_with_a_compare_and_set() -> Result<(), Box<dyn std::error::Error>> {
        let editor = ActorId::new("editor")?;
        let mut room = room_with(&editor)?;
        let page = PageRef { index: 0, epoch: 0 };
        room.apply(&editor, &note("n1", "Look here", 0)?, 0)?;
        let window = window_of(room.adapter(), "n1")?.ok_or("a sticky note is created with a window")?;
        assert!(!window.open, "a new note's window starts closed");

        // Opening names the state alone: the place a window has stays.
        let open = op(
            "popup-open",
            ConflictClass::Merge,
            Target::Annotation { page, nm: "n1".into() },
            room.version(),
            0,
            json!({"action": "annotation.popup", "popup": {"open": true}}),
        )?;
        let applied = room.apply(&editor, &open, 0)?;
        assert_eq!(applied.op.payload["popup"], json!({"open": true}));
        assert_eq!(applied.op.payload["before"], json!({"open": false}), "the state it replaced");
        assert!(applied.op.payload.get("expect").is_none(), "the accepted operation carries no compare-and-set");
        let after = PopupState::from_value(&applied.op.payload["after"])?;
        assert!(after.open && after.rect == window.rect, "the engine kept the window's place");
        assert_eq!(window_of(room.adapter(), "n1")?, Some(after));

        // A move names the place alone; the accepted operation carries the state
        // around it, so the undo can put the window back.
        let moved = [200.0, 100.0, 400.0, 220.0];
        let drag = op(
            "popup-move",
            ConflictClass::Merge,
            Target::Annotation { page, nm: "n1".into() },
            room.version(),
            0,
            json!({"action": "annotation.popup", "popup": {"rect": moved}}),
        )?;
        let dragged = room.apply(&editor, &drag, 0)?;
        assert_eq!(dragged.op.payload["before"], json!({"rect": window.rect}), "only the fields the update names");
        assert_eq!(PopupState::from_value(&dragged.op.payload["after"])?.rect, moved);

        // A comment with no window of its own gets one beside it: the accepted
        // operation carries `before: null` and the place the engine chose.
        room.apply(&editor, &rectangle("r1", room.version())?, 0)?;
        assert_eq!(window_of(room.adapter(), "r1")?, None, "a rectangle starts without a window");
        let first = op(
            "popup-first",
            ConflictClass::Merge,
            Target::Annotation { page, nm: "r1".into() },
            room.version(),
            0,
            json!({"action": "annotation.popup", "popup": {"open": true}}),
        )?;
        let created = room.apply(&editor, &first, 0)?;
        assert_eq!(created.op.payload["before"], serde_json::Value::Null);
        let created_state = PopupState::from_value(&created.op.payload["after"])?;
        assert!(created_state.open && created_state.rect[2] - created_state.rect[0] > 1.0);

        // A text box has no window: refused before the document changes.
        room.apply(
            &editor,
            &op(
                "add-box",
                ConflictClass::Merge,
                Target::Page(page),
                room.version(),
                0,
                json!({"action": "annotation.add", "nm": "t1",
                       "shape": {"kind": "text_box", "rect": [10.0, 100.0, 180.0, 160.0], "font_size": 10.0},
                       "contents": "Some text", "author": DEFAULT_AUTHOR}),
            )?,
            0,
        )?;
        let version = room.version();
        let on_a_box = op(
            "popup-box",
            ConflictClass::Merge,
            Target::Annotation { page, nm: "t1".into() },
            version,
            0,
            json!({"action": "annotation.popup", "popup": {"open": true}}),
        )?;
        let refused = room.apply(&editor, &on_a_box, 0).err().ok_or("a text box was given a window")?;
        assert_eq!(refused.code, ErrorCode::Invalid, "{}", refused.message);
        assert_eq!(room.version(), version, "a refused operation does not move the version");
        assert_eq!(window_of(room.adapter(), "t1")?, None, "and changes nothing");

        // Someone drags the window: the undo's compare-and-set fails and their
        // place stays.
        let theirs = [300.0, 120.0, 500.0, 240.0];
        room.apply(
            &editor,
            &op(
                "popup-theirs",
                ConflictClass::Merge,
                Target::Annotation { page, nm: "n1".into() },
                room.version(),
                0,
                json!({"action": "annotation.popup", "popup": {"rect": theirs}}),
            )?,
            0,
        )?;
        let version = room.version();
        let undo = op(
            "undo-drag",
            ConflictClass::Merge,
            Target::Annotation { page, nm: "n1".into() },
            version,
            0,
            json!({"action": "annotation.popup", "popup": {"rect": window.rect}, "expect": {"rect": moved, "open": true}}),
        )?;
        let refused = room.apply(&editor, &undo, 0).err().ok_or("the undo overwrote someone else's drag")?;
        assert_eq!(refused.code, ErrorCode::Conflict);
        assert_eq!(room.version(), version);
        assert_eq!(window_of(room.adapter(), "n1")?.map(|window| window.rect), Some(theirs));

        // With the window where the edit left it, the undo puts the old place
        // back, and a fresh replica replaying the operations lands on the same.
        let undo = op(
            "undo-drag-2",
            ConflictClass::Merge,
            Target::Annotation { page, nm: "n1".into() },
            room.version(),
            0,
            json!({"action": "annotation.popup", "popup": {"rect": window.rect}, "expect": {"rect": theirs, "open": true}}),
        )?;
        let undone = room.apply(&editor, &undo, 0)?;
        assert_eq!(window_of(room.adapter(), "n1")?, Some(PopupState { rect: window.rect, open: true }));

        let mut fresh = PdfAdapter::open("fixture.pdf", fixture(3).into())?;
        fresh.apply(&editor, &note("n1", "Look here", 0)?)?;
        fresh.apply(&editor, &applied.op)?;
        fresh.apply(&editor, &dragged.op)?;
        fresh.apply(&editor, &rectangle("r1", 0)?)?;
        fresh.apply(&editor, &created.op)?;
        fresh.apply(&editor, &undone.op)?;
        assert_eq!(window_of(&fresh, "n1")?, window_of(room.adapter(), "n1")?);
        assert_eq!(window_of(&fresh, "r1")?, Some(created_state));
        Ok(())
    }

    /// A move and a resize are addressable operations on a comment: the accepted
    /// operation carries the rectangle before and after, a fresh replica replays
    /// it, and a stale undo is refused without a change.
    #[test]
    fn a_move_and_a_resize_are_addressable_and_undo_with_a_compare_and_set() -> Result<(), Box<dyn std::error::Error>> {
        let editor = ActorId::new("editor")?;
        let mut room = room_with(&editor)?;
        room.apply(&editor, &rectangle("r1", 0)?, 0)?;
        let page = PageRef { index: 0, epoch: 0 };
        let start = style_of(room.adapter(), "r1")?.rect.ok_or("the rectangle has no rect")?;

        let mv = op(
            "move-1",
            ConflictClass::Merge,
            Target::Annotation { page, nm: "r1".into() },
            1,
            0,
            json!({"action": "annotation.move", "dx": 20.0, "dy": -5.0}),
        )?;
        let applied = room.apply(&editor, &mv, 0)?;
        assert_eq!(applied.op.payload["before"], json!(start));
        let moved = style_of(room.adapter(), "r1")?.rect.ok_or("no rect")?;
        for (i, delta) in [20.0, -5.0, 20.0, -5.0].into_iter().enumerate() {
            assert!((moved[i] - (start[i] + delta)).abs() < 1e-3, "a move shifts /Rect: {moved:?}");
        }
        assert_eq!(applied.op.payload["after"], json!(moved));

        // Someone drags it elsewhere: the undo's compare-and-set fails and their place stays.
        let theirs = op(
            "move-theirs",
            ConflictClass::Merge,
            Target::Annotation { page, nm: "r1".into() },
            2,
            0,
            json!({"action": "annotation.move", "dx": 1.0, "dy": 1.0}),
        )?;
        room.apply(&editor, &theirs, 0)?;
        let version = room.version();
        let undo = op(
            "undo-move",
            ConflictClass::Merge,
            Target::Annotation { page, nm: "r1".into() },
            version,
            0,
            json!({"action": "annotation.move", "dx": -20.0, "dy": 5.0, "expect": moved}),
        )?;
        let refused = room.apply(&editor, &undo, 0).err().ok_or("the undo overwrote another drag")?;
        assert_eq!(refused.code, ErrorCode::Conflict);
        assert_eq!(room.version(), version);
        let after_theirs = style_of(room.adapter(), "r1")?.rect.ok_or("no rect")?;
        assert!((after_theirs[0] - (moved[0] + 1.0)).abs() < 1e-3, "the other drag stayed");

        // With the comment where the move left it, the undo puts it back.
        let undo = op(
            "undo-move-2",
            ConflictClass::Merge,
            Target::Annotation { page, nm: "r1".into() },
            room.version(),
            0,
            json!({"action": "annotation.move", "dx": -21.0, "dy": 4.0, "expect": after_theirs}),
        )?;
        let undone = room.apply(&editor, &undo, 0)?;
        let back = style_of(room.adapter(), "r1")?.rect.ok_or("no rect")?;
        for i in 0..4 {
            assert!((back[i] - start[i]).abs() < 1e-3, "the undo puts the rectangle back: {back:?}");
        }

        let frame = back;
        let target = [frame[0] + 10.0, frame[1] + 10.0, frame[2] + 10.0, frame[3] + 20.0];
        let resize = op(
            "resize-1",
            ConflictClass::Merge,
            Target::Annotation { page, nm: "r1".into() },
            room.version(),
            0,
            json!({"action": "annotation.geometry", "geometry": {"rect": target}}),
        )?;
        let resized = room.apply(&editor, &resize, 0)?;
        assert_eq!(resized.op.payload["geometry"], json!({"rect": target}));
        assert_eq!(resized.op.payload["before"], json!({"rect": frame}));
        assert_eq!(resized.op.payload["after"], json!({"rect": target}));
        let now = style_of(room.adapter(), "r1")?.rect.ok_or("no rect")?;
        for i in 0..4 {
            assert!((now[i] - target[i]).abs() < 1e-3, "a resize sets /Rect: {now:?}");
        }

        // Replay both on a fresh replica, then undo the resize with its own CAS.
        let mut fresh = PdfAdapter::open("fixture.pdf", fixture(3).into())?;
        fresh.apply(&editor, &rectangle("r1", 0)?)?;
        fresh.apply(&editor, &applied.op)?;
        fresh.apply(&editor, &undone.op)?;
        fresh.apply(&editor, &resized.op)?;
        let replayed = style_of(&fresh, "r1")?.rect.ok_or("no rect")?;
        for i in 0..4 {
            assert!((replayed[i] - target[i]).abs() < 1e-3);
        }
        let undo_resize = op(
            "undo-resize",
            ConflictClass::Merge,
            Target::Annotation { page, nm: "r1".into() },
            room.version(),
            0,
            json!({"action": "annotation.geometry", "geometry": {"rect": frame}, "expect": {"rect": target}}),
        )?;
        room.apply(&editor, &undo_resize, 0)?;
        let back = style_of(room.adapter(), "r1")?.rect.ok_or("no rect")?;
        for i in 0..4 {
            assert!((back[i] - frame[i]).abs() < 1e-3, "the undo puts the frame back: {back:?}");
        }
        Ok(())
    }

    /// A line's ends are reshaped as one addressable operation; a comment with no
    /// points is refused before the document changes, and a stale undo changes
    /// nothing.
    #[test]
    fn a_reshape_sets_the_points_of_a_line_and_refuses_a_comment_without_them() -> Result<(), Box<dyn std::error::Error>> {
        let editor = ActorId::new("editor")?;
        let mut room = room_with(&editor)?;
        let page = PageRef { index: 0, epoch: 0 };
        let add = op(
            "add-line",
            ConflictClass::Merge,
            Target::Page(page),
            0,
            0,
            json!({"action": "annotation.add", "nm": "l1",
                   "shape": {"kind": "line", "from": [20.0, 20.0], "to": [120.0, 120.0], "start": "none", "end": "open_arrow"},
                   "style": {"color": [1.0, 0.0, 0.0], "opacity": 1.0, "width": 1.0},
                   "contents": "", "author": DEFAULT_AUTHOR}),
        )?;
        room.apply(&editor, &add, 0)?;
        let start = style_of(room.adapter(), "l1")?.points;
        assert_eq!(start.len(), 2, "a line has two ends");

        let points = vec![[20.0, 20.0], [120.0, 40.0]];
        let reshape = op(
            "reshape-1",
            ConflictClass::Merge,
            Target::Annotation { page, nm: "l1".into() },
            1,
            0,
            json!({"action": "annotation.geometry", "geometry": {"points": points}}),
        )?;
        let applied = room.apply(&editor, &reshape, 0)?;
        assert_eq!(applied.op.payload["before"], json!({"points": start}));
        assert_eq!(applied.op.payload["after"], json!({"points": points}));
        let now = style_of(room.adapter(), "l1")?.points;
        assert_eq!(now.len(), 2);
        assert!((now[1][1] - 40.0).abs() < 1e-3, "the end moved: {now:?}");

        // A rectangle has no points to reshape: refused whole, version unmoved.
        room.apply(&editor, &rectangle("r1", room.version())?, 0)?;
        let version = room.version();
        let no_points = op(
            "reshape-rect",
            ConflictClass::Merge,
            Target::Annotation { page, nm: "r1".into() },
            version,
            0,
            json!({"action": "annotation.geometry", "geometry": {"points": points}}),
        )?;
        let refused = room.apply(&editor, &no_points, 0).err().ok_or("a rectangle was reshaped")?;
        assert_eq!(refused.code, ErrorCode::Unsupported, "{}", refused.message);
        assert_eq!(room.version(), version);

        // Someone moves the end again: the undo cannot overwrite it.
        let theirs = op(
            "reshape-theirs",
            ConflictClass::Merge,
            Target::Annotation { page, nm: "l1".into() },
            room.version(),
            0,
            json!({"action": "annotation.geometry", "geometry": {"points": [[20.0, 20.0], [90.0, 90.0]]}}),
        )?;
        room.apply(&editor, &theirs, 0)?;
        let version = room.version();
        let undo = op(
            "undo-reshape",
            ConflictClass::Merge,
            Target::Annotation { page, nm: "l1".into() },
            version,
            0,
            json!({"action": "annotation.geometry", "geometry": {"points": start}, "expect": {"points": points}}),
        )?;
        let refused = room.apply(&editor, &undo, 0).err().ok_or("the undo overwrote another reshape")?;
        assert_eq!(refused.code, ErrorCode::Conflict);
        assert_eq!(room.version(), version);
        assert!((style_of(room.adapter(), "l1")?.points[1][0] - 90.0).abs() < 1e-3);

        // With the points as the operation left them, the undo puts the old ones back.
        let undo = op(
            "undo-reshape-2",
            ConflictClass::Merge,
            Target::Annotation { page, nm: "l1".into() },
            room.version(),
            0,
            json!({"action": "annotation.geometry", "geometry": {"points": start}, "expect": {"points": [[20.0, 20.0], [90.0, 90.0]]}}),
        )?;
        let undone = room.apply(&editor, &undo, 0)?;
        let back = style_of(room.adapter(), "l1")?.points;
        assert!((back[1][0] - start[1][0]).abs() < 1e-3 && (back[1][1] - start[1][1]).abs() < 1e-3, "{back:?}");

        // Replay: add, reshape, undo on a fresh replica.
        let mut fresh = PdfAdapter::open("fixture.pdf", fixture(3).into())?;
        fresh.apply(&editor, &add)?;
        fresh.apply(&editor, &applied.op)?;
        fresh.apply(&editor, &undone.op)?;
        let replayed = style_of(&fresh, "l1")?.points;
        assert!((replayed[1][0] - start[1][0]).abs() < 1e-3, "{replayed:?}");
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
    fn decode(encoded: &str) -> Result<Vec<u8>, Box<dyn std::error::Error>> {
        use base64::Engine as _;
        Ok(base64::engine::general_purpose::STANDARD.decode(encoded)?)
    }

    /// A split is one structural operation: the range leaves the document, the
    /// accepted operation carries the part, both sides are numbered
    /// deterministically, and a combine of that part puts it back exactly.
    #[test]
    fn a_split_takes_a_range_out_and_carries_the_part() -> Result<(), Box<dyn std::error::Error>> {
        let editor = ActorId::new("editor")?;
        let mut room = room_with(&editor)?;
        let split = op(
            "split-1",
            ConflictClass::Structural,
            Target::Page(PageRef { index: 1, epoch: 0 }),
            0,
            0,
            json!({"action": "page.split", "count": 1}),
        )?;
        let applied = room.apply(&editor, &split, 0)?;
        assert_eq!(applied.epoch, 1, "a split is structural");
        assert_eq!(applied.result["from"], 1);
        assert_eq!(applied.result["to"], 2);
        assert_eq!(applied.result["count"], 1);
        assert_eq!(applied.result["kept"], 2, "both sides are numbered");
        assert!(applied.result["part"]["sha256"].is_string());
        let carried = applied.op.payload["bytes"].as_str().ok_or("the accepted split carries no part")?;
        let part = decode(carried)?;
        assert!(part.starts_with(b"%PDF"), "the part is a PDF");

        let version = room.version();
        commit(&mut room, &editor, version)?;
        let bytes = room.adapter().last_commit().cloned().ok_or("no commit bytes")?;
        assert_eq!(room.adapter().document().ok_or("gone")?.info.pages.len(), 2, "the part left the document");
        assert_eq!(page_text(&bytes, 0)?, "Page 1", "the page before the part did not move");
        assert_eq!(page_text(&bytes, 1)?, "Page 3", "the pages after the part move down by its count");
        let part = std::sync::Arc::new(part);
        assert_eq!(page_text(&part, 0)?, "Page 2", "the part is the page that left, under its own numbering");

        // The undo is the combine of the part the accepted operation carries, at
        // the index the split took it from.
        let join = op(
            "join-1",
            ConflictClass::Structural,
            Target::Page(PageRef { index: 1, epoch: applied.epoch }),
            applied.version,
            applied.epoch,
            json!({"action": "page.combine", "count": 1, "bytes": carried}),
        )?;
        let joined = room.apply(&editor, &join, 0)?;
        assert_eq!(joined.epoch, 2);
        assert_eq!(joined.result["at"], 1);
        assert_eq!(joined.result["count"], 1);
        assert_eq!(joined.result["pages"], 3);
        let version = room.version();
        commit(&mut room, &editor, version)?;
        let bytes = room.adapter().last_commit().cloned().ok_or("no commit bytes")?;
        assert_eq!(page_text(&bytes, 0)?, "Page 1");
        assert_eq!(page_text(&bytes, 1)?, "Page 2", "the part is back where the split took it");
        assert_eq!(page_text(&bytes, 2)?, "Page 3");
        Ok(())
    }

    /// The document's own edges are the split's boundary: an empty range, a
    /// range past the last page, one that names no count, and one that would
    /// take the whole document are refused before anything changes. A part over
    /// the frame budget is `limit`, not a truncated part.
    #[test]
    fn a_split_at_the_documents_edge_is_refused() -> Result<(), Box<dyn std::error::Error>> {
        let editor = ActorId::new("editor")?;
        let mut room = room_with(&editor)?;
        for (id, index, payload) in [
            ("empty", 0, json!({"action": "page.split", "count": 0})),
            ("whole", 0, json!({"action": "page.split", "count": 3})),
            ("past", 1, json!({"action": "page.split", "count": 3})),
            ("missing", 3, json!({"action": "page.split", "count": 1})),
            ("no-count", 0, json!({"action": "page.split"})),
        ] {
            let bad = op(id, ConflictClass::Structural, Target::Page(PageRef { index, epoch: 0 }), 0, 0, payload)?;
            let rejected = room.apply(&editor, &bad, 0).err().ok_or("a boundary split was accepted")?;
            assert_eq!(rejected.code, ErrorCode::Invalid, "{id}");
            assert!(!rejected.retryable, "{id}");
        }
        assert_eq!(room.adapter().document().ok_or("gone")?.info.pages.len(), 3, "no boundary split changed the document");
        assert_eq!(room.version(), 0, "a refusal accepts nothing");
        assert_eq!(room.epoch(), 0);

        // A part that cannot fit one operation is refused with `limit`.
        let adapter = PdfAdapter::open("fixture.pdf", fixture_with_padding(2, Some((1, MAX_RESTORE_PAGE_BYTES))).into())?;
        let mut big = Room::new(SessionId::new("session-2")?, DocKey::new("file-1")?, adapter, RoomConfig::default());
        big.hello(editor.clone(), Role::Editor, &hello()?, 0)?;
        let split =
            op("huge", ConflictClass::Structural, Target::Page(PageRef { index: 1, epoch: 0 }), 0, 0, json!({"action": "page.split", "count": 1}))?;
        let rejected = big.apply(&editor, &split, 0).err().ok_or("an oversized part was accepted")?;
        assert_eq!(rejected.code, ErrorCode::Limit);
        assert!(!rejected.retryable);
        assert_eq!(big.adapter().document().ok_or("gone")?.info.pages.len(), 2, "the oversized split changed nothing");
        assert_eq!(big.version(), 0);
        Ok(())
    }

    /// A split is addressed in the epoch, like every page operation: built on a
    /// passed epoch it is retryable and changes nothing, and the same intent
    /// re-addressed to the head is accepted.
    #[test]
    fn a_split_addressed_to_another_epoch_is_retryable() -> Result<(), Box<dyn std::error::Error>> {
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
        room.apply(&editor, &turn, 0)?;
        let stale = op(
            "split-stale",
            ConflictClass::Structural,
            Target::Page(PageRef { index: 0, epoch: 0 }),
            1,
            1,
            json!({"action": "page.split", "count": 1}),
        )?;
        let rejected = room.apply(&editor, &stale, 0).err().ok_or("a split on a passed epoch was accepted")?;
        assert_eq!(rejected.code, ErrorCode::Stale);
        assert!(rejected.retryable);
        assert_eq!(room.adapter().document().ok_or("gone")?.info.pages.len(), 3, "the stale split changed nothing");
        assert_eq!(room.version(), 1);

        let retry = op(
            "split-retry",
            ConflictClass::Structural,
            Target::Page(PageRef { index: 0, epoch: 1 }),
            1,
            1,
            json!({"action": "page.split", "count": 1}),
        )?;
        let applied = room.apply(&editor, &retry, 0)?;
        assert_eq!(applied.epoch, 2);
        assert_eq!(room.adapter().document().ok_or("gone")?.info.pages.len(), 2, "the retried split applied");
        Ok(())
    }

    /// A combine is one operation for a whole part: the pages are inserted at
    /// the position the target names, the page count the operation declares has
    /// to be the part's, and a split of the same range undoes it.
    #[test]
    fn a_combine_inserts_the_part_it_carries_in_one_step() -> Result<(), Box<dyn std::error::Error>> {
        let editor = ActorId::new("editor")?;
        let mut room = room_with(&editor)?;
        let part = fixture(2);
        let combine = op(
            "combine-1",
            ConflictClass::Structural,
            Target::Page(PageRef { index: 1, epoch: 0 }),
            0,
            0,
            json!({"action": "page.combine", "count": 2, "bytes": base64_encode(&part)}),
        )?;
        let applied = room.apply(&editor, &combine, 0)?;
        assert_eq!(applied.epoch, 1, "a combine is structural");
        assert_eq!(applied.result["at"], 1);
        assert_eq!(applied.result["count"], 2);
        assert_eq!(applied.result["pages"], 5, "the document grew by the whole part");
        let version = room.version();
        commit(&mut room, &editor, version)?;
        let bytes = room.adapter().last_commit().cloned().ok_or("no commit bytes")?;
        assert_eq!(page_text(&bytes, 0)?, "Page 1");
        assert_eq!(page_text(&bytes, 1)?, "Page 1", "the part's first page is where the target named it");
        assert_eq!(page_text(&bytes, 2)?, "Page 2");
        assert_eq!(page_text(&bytes, 3)?, "Page 2", "the document's own pages follow the part");
        assert_eq!(page_text(&bytes, 4)?, "Page 3");

        // Undo: the same range leaves again, as one operation.
        let undo = op(
            "undo-1",
            ConflictClass::Structural,
            Target::Page(PageRef { index: 1, epoch: applied.epoch }),
            applied.version,
            applied.epoch,
            json!({"action": "page.split", "count": 2}),
        )?;
        let undone = room.apply(&editor, &undo, 0)?;
        assert_eq!(undone.result["kept"], 3, "the document is back to its own pages");
        assert_eq!(room.adapter().document().ok_or("gone")?.info.pages.len(), 3);
        Ok(())
    }

    #[test]
    fn a_combine_refuses_a_part_it_cannot_carry() -> Result<(), Box<dyn std::error::Error>> {
        let editor = ActorId::new("editor")?;
        let mut room = room_with(&editor)?;
        let two_pages = fixture(2);
        for (id, at, payload) in [
            ("past", 4, json!({"action": "page.combine", "count": 2, "bytes": base64_encode(&two_pages)})),
            ("mismatch", 0, json!({"action": "page.combine", "count": 1, "bytes": base64_encode(&two_pages)})),
            ("bad-base64", 0, json!({"action": "page.combine", "count": 1, "bytes": "%%%"})),
            ("not-a-pdf", 0, json!({"action": "page.combine", "count": 1, "bytes": base64_encode(b"not a pdf")})),
            ("no-count", 0, json!({"action": "page.combine", "bytes": base64_encode(&two_pages)})),
            ("no-bytes", 0, json!({"action": "page.combine", "count": 1})),
        ] {
            let bad = op(id, ConflictClass::Structural, Target::Page(PageRef { index: at, epoch: 0 }), 0, 0, payload)?;
            let rejected = room.apply(&editor, &bad, 0).err().ok_or("a bad combine was accepted")?;
            assert_eq!(rejected.code, ErrorCode::Invalid, "{id}");
            assert!(!rejected.retryable, "{id}");
        }
        let huge = vec![0u8; MAX_RESTORE_PAGE_BYTES + 1];
        let over = op(
            "huge",
            ConflictClass::Structural,
            Target::Page(PageRef { index: 0, epoch: 0 }),
            0,
            0,
            json!({"action": "page.combine", "count": 1, "bytes": base64_encode(&huge)}),
        )?;
        let rejected = room.apply(&editor, &over, 0).err().ok_or("an oversized part was accepted")?;
        assert_eq!(rejected.code, ErrorCode::Limit);
        assert_eq!(room.adapter().document().ok_or("gone")?.info.pages.len(), 3, "no bad combine changed the document");
        assert_eq!(room.version(), 0, "a refusal accepts nothing");
        assert_eq!(room.epoch(), 0);
        Ok(())
    }

    /// The accepted operation carries everything a replay needs: a split is cut
    /// from the replayed document and a combine inserts the part it carries, so
    /// a restart lands on the same page set without any client file.
    #[test]
    fn a_split_and_a_combine_replay_from_their_operations() -> Result<(), Box<dyn std::error::Error>> {
        let editor = ActorId::new("editor")?;
        let mut room = room_with(&editor)?;
        let split = op(
            "split-1",
            ConflictClass::Structural,
            Target::Page(PageRef { index: 1, epoch: 0 }),
            0,
            0,
            json!({"action": "page.split", "count": 1}),
        )?;
        let split = room.apply(&editor, &split, 0)?.op;
        let part = split.payload["bytes"].clone();

        let adapter = PdfAdapter::open("fixture.pdf", fixture(3).into())?;
        let mut fresh = Room::new(SessionId::new("session-2")?, DocKey::new("file-1")?, adapter, RoomConfig::default());
        fresh.hello(editor.clone(), Role::Editor, &hello()?, 0)?;
        fresh.replay(editor.clone(), split, 1, 0)?;
        assert_eq!(fresh.epoch(), 1);
        assert_eq!(fresh.adapter().document().ok_or("gone")?.info.pages.len(), 2);

        let combine = op(
            "combine-1",
            ConflictClass::Structural,
            Target::Page(PageRef { index: 1, epoch: 1 }),
            1,
            1,
            json!({"action": "page.combine", "count": 1, "bytes": part}),
        )?;
        fresh.replay(editor.clone(), combine, 2, 0)?;
        assert_eq!(fresh.epoch(), 2);
        assert_eq!(fresh.adapter().document().ok_or("gone")?.info.pages.len(), 3);
        let version = fresh.version();
        commit(&mut fresh, &editor, version)?;
        let bytes = fresh.adapter().last_commit().cloned().ok_or("no commit bytes")?;
        assert_eq!(page_text(&bytes, 1)?, "Page 2", "the journal put the part back");
        Ok(())
    }

    // ---- The outline and its named destinations (RFC §3.1.7) --------------

    /// Four pages, four bookmarks (one per page) and a named destination, built
    /// with the engine's own edits so the fixture is a real file.
    fn fixture_with_outline() -> Vec<u8> {
        use pdfcraft_engine::{BookmarkDest, Edit, Session};
        let mut session = Session::new();
        let id = session.open_new("outline.pdf", fixture(4).into()).expect("the fixture opens");
        session
            .apply(id, Edit::SetNamedDestination { name: "intro".into(), dest: BookmarkDest::Page { page: 2, left: Some(20.0), top: Some(280.0) } })
            .expect("the name is set");
        session
            .apply(id, Edit::AddBookmarkWith { parent: vec![], index: 0, title: "Start".into(), dest: BookmarkDest::Named { name: "intro".into() } })
            .expect("the bookmark is added");
        session
            .apply(
                id,
                Edit::AddBookmarkWith {
                    parent: vec![],
                    index: 1,
                    title: "Chapters".into(),
                    dest: BookmarkDest::Page { page: 0, left: None, top: None },
                },
            )
            .expect("the parent is added");
        session
            .apply(
                id,
                Edit::AddBookmarkWith { parent: vec![1], index: 0, title: "One".into(), dest: BookmarkDest::Page { page: 0, left: None, top: None } },
            )
            .expect("the child is added");
        session
            .apply(
                id,
                Edit::AddBookmarkWith { parent: vec![1], index: 1, title: "Two".into(), dest: BookmarkDest::Page { page: 1, left: None, top: None } },
            )
            .expect("the child is added");
        session.save_bytes(id).expect("the fixture saves").as_ref().clone()
    }

    /// The outline as a room's client reads it: `title→page` (page 0 = nowhere).
    fn outline(adapter: &PdfAdapter) -> Vec<String> {
        let page = adapter.session().bookmark_page(adapter.id, 0, usize::MAX).expect("the outline is readable");
        page.rows.into_iter().map(|row| format!("{}→{}", row.title, row.page.map_or(0, |p| p + 1))).collect()
    }

    /// The key of the item titled `title`.
    fn key_of(adapter: &PdfAdapter, title: &str) -> pdfcraft_collab_proto::OutlineKey {
        let page = adapter.session().bookmark_page(adapter.id, 0, usize::MAX).expect("the outline is readable");
        let row = page.rows.into_iter().find(|row| row.title == title).expect("the bookmark is there");
        let (num, generation) = row.object.expect("a document bookmark has an object");
        pdfcraft_collab_proto::OutlineKey::new(num, generation)
    }

    /// The named destination `name` as the document has it.
    fn named(adapter: &PdfAdapter, name: &str) -> Option<pdfcraft_engine::DocumentNamedDestination> {
        adapter.document().and_then(|document| document.named_destinations().into_iter().find(|destination| destination.name == name))
    }

    fn room_with_outline() -> Result<(Room<PdfAdapter>, ActorId), Box<dyn std::error::Error>> {
        let adapter = PdfAdapter::open("outline.pdf", fixture_with_outline().into())?;
        let mut room = Room::new(SessionId::new("session-1")?, DocKey::new("file-1")?, adapter, RoomConfig::default());
        let editor = ActorId::new("editor")?;
        room.hello(editor.clone(), Role::Editor, &hello()?, 0)?;
        Ok((room, editor))
    }

    /// One page of `bytes` as a PDF of its own, for a replacement or a combine.
    fn one_page_copy(bytes: &[u8], index: usize) -> Result<Vec<u8>, Box<dyn std::error::Error>> {
        let mut scratch = pdfcraft_engine::Session::new();
        let id = scratch.open("copy.pdf", None, std::sync::Arc::new(bytes.to_vec()), None)?;
        let count = scratch.get(id).ok_or("open")?.info.pages.len();
        if count > 1 {
            let others: Vec<usize> = (0..count).filter(|i| *i != index).collect();
            scratch.apply(id, pdfcraft_engine::Edit::DeletePages { pages: others })?;
        }
        Ok(scratch.save_bytes(id)?.as_ref().clone())
    }

    /// An item is added under an addressed parent, updated in title and
    /// destination, moved and deleted with the whole subtree as its copy — every
    /// step is one version and one epoch, and the accepted operation describes
    /// what changed.
    #[test]
    fn the_outline_is_added_updated_moved_and_deleted() -> Result<(), Box<dyn std::error::Error>> {
        let (mut room, editor) = room_with_outline()?;
        let chapters = key_of(room.adapter(), "Chapters");
        let two = key_of(room.adapter(), "Two");

        let add = op(
            "add-1",
            ConflictClass::Structural,
            Target::OutlineItem { key: Some(chapters) },
            0,
            0,
            json!({"action": "outline.item.add", "title": "Three", "dest": {"kind": "page", "page": 3}}),
        )?;
        let applied = room.apply(&editor, &add, 0)?;
        assert_eq!((applied.version, applied.epoch), (1, 1), "a structural step");
        let added: pdfcraft_collab_proto::OutlineKey = serde_json::from_value(applied.op.payload["key"].clone())?;
        assert_eq!(applied.op.payload["path"], json!([1, 2]), "the accepted operation names where it went");
        assert_eq!(outline(room.adapter()), ["Start→3", "Chapters→1", "One→1", "Two→2", "Three→4"]);
        assert_eq!(room.adapter().epoch(), 1);

        let update = op(
            "upd-1",
            ConflictClass::Structural,
            Target::OutlineItem { key: Some(two) },
            1,
            1,
            json!({"action": "outline.item.update", "title": "Deux", "dest": {"kind": "named", "name": "intro"}}),
        )?;
        let applied = room.apply(&editor, &update, 0)?;
        assert_eq!(applied.op.payload["before"]["title"], "Two", "the undo image names what it replaced");
        assert_eq!(applied.op.payload["before"]["dest"]["kind"], "page");
        assert_eq!(outline(room.adapter()), ["Start→3", "Chapters→1", "One→1", "Deux→3", "Three→4"]);

        let move_to_top = op(
            "mov-1",
            ConflictClass::Structural,
            Target::OutlineItem { key: Some(two) },
            2,
            2,
            json!({"action": "outline.item.move", "parent": null, "index": 0}),
        )?;
        let applied = room.apply(&editor, &move_to_top, 0)?;
        assert_eq!(applied.op.payload["from"], json!([1, 1]));
        assert_eq!(applied.op.payload["before"]["index"], 1);
        assert_eq!(outline(room.adapter()), ["Deux→3", "Start→3", "Chapters→1", "One→1", "Three→4"]);

        let delete =
            op("del-1", ConflictClass::Structural, Target::OutlineItem { key: Some(chapters) }, 3, 3, json!({"action": "outline.item.delete"}))?;
        let applied = room.apply(&editor, &delete, 0)?;
        let subtree = applied.op.payload["subtree"].clone();
        assert_eq!(applied.op.payload["key"], json!(chapters));
        let nodes = subtree.as_array().ok_or("the copy is a tree")?;
        assert_eq!(nodes.len(), 1);
        assert_eq!(nodes[0]["title"], "Chapters");
        assert_eq!(nodes[0]["children"].as_array().map(Vec::len), Some(2), "the whole subtree travels");
        assert_eq!(outline(room.adapter()), ["Deux→3", "Start→3"]);

        // The undo is an ordinary operation: the subtree goes back where it came
        // from, with its nesting and destinations, and gets new object keys.
        let restore = op(
            "undo-del-1",
            ConflictClass::Structural,
            Target::OutlineItem { key: null_key(&applied.op.payload["parent_key"]) },
            room.version(),
            room.epoch(),
            json!({"action": "outline.item.restore", "subtree": subtree, "index": 2}),
        )?;
        let applied = room.apply(&editor, &restore, 0)?;
        assert!(applied.op.payload["key"].is_object(), "the restored item answers with its new key");
        assert_eq!(outline(room.adapter()), ["Deux→3", "Start→3", "Chapters→1", "One→1", "Three→4"]);
        // And adding to the restored parent works: it is addressed by its new key.
        let restored: pdfcraft_collab_proto::OutlineKey = serde_json::from_value(applied.op.payload["key"].clone())?;
        let add_again = op(
            "add-2",
            ConflictClass::Structural,
            Target::OutlineItem { key: Some(restored) },
            room.version(),
            room.epoch(),
            json!({"action": "outline.item.add", "title": "Four", "dest": {"kind": "page", "page": 0}}),
        )?;
        room.apply(&editor, &add_again, 0)?;
        assert_eq!(outline(room.adapter()), ["Deux→3", "Start→3", "Chapters→1", "One→1", "Three→4", "Four→1"]);
        let _ = added;
        Ok(())
    }

    /// `null` or an object, as the delete's `parent_key` has it.
    fn null_key(value: &serde_json::Value) -> Option<pdfcraft_collab_proto::OutlineKey> {
        if value.is_null() { None } else { serde_json::from_value(value.clone()).ok() }
    }

    /// Hostile or simply wrong addresses are refused before the first write: an
    /// item nobody has, a page or a name this document lacks, a blank or
    /// over-long title, a move into the item's own subtree, and a copy that nests
    /// too deep. The version, the epoch and the document do not move.
    #[test]
    fn a_bad_outline_operation_changes_nothing() -> Result<(), Box<dyn std::error::Error>> {
        let (mut room, editor) = room_with_outline()?;
        let chapters = key_of(room.adapter(), "Chapters");
        let one = key_of(room.adapter(), "One");
        let before = outline(room.adapter());
        let (version, epoch) = (room.version(), room.epoch());
        let missing = pdfcraft_collab_proto::OutlineKey::new(999, 0);
        let long_title = "t".repeat(pdfcraft_collab_proto::MAX_OUTLINE_TITLE + 1);

        let cases: Vec<(Target, Value, ErrorCode)> = vec![
            (
                Target::OutlineItem { key: Some(missing) },
                json!({"action": "outline.item.add", "title": "Lost", "dest": {"kind": "page", "page": 0}}),
                ErrorCode::Invalid,
            ),
            (
                Target::OutlineItem { key: None },
                json!({"action": "outline.item.add", "title": "Lost", "dest": {"kind": "page", "page": 99}}),
                ErrorCode::Invalid,
            ),
            (
                Target::OutlineItem { key: None },
                json!({"action": "outline.item.add", "title": "Lost", "dest": {"kind": "named", "name": "nope"}}),
                ErrorCode::Invalid,
            ),
            (
                Target::OutlineItem { key: None },
                json!({"action": "outline.item.add", "title": "   ", "dest": {"kind": "page", "page": 0}}),
                ErrorCode::Invalid,
            ),
            (
                Target::OutlineItem { key: None },
                json!({"action": "outline.item.add", "title": long_title, "dest": {"kind": "page", "page": 0}}),
                ErrorCode::Limit,
            ),
            (Target::OutlineItem { key: Some(missing) }, json!({"action": "outline.item.update", "title": "Lost"}), ErrorCode::Invalid),
            (Target::OutlineItem { key: Some(chapters) }, json!({"action": "outline.item.update"}), ErrorCode::Invalid),
            (Target::OutlineItem { key: Some(missing) }, json!({"action": "outline.item.delete"}), ErrorCode::Invalid),
            // A move into the item's own subtree is a cycle.
            (Target::OutlineItem { key: Some(chapters) }, json!({"action": "outline.item.move", "parent": one, "index": 0}), ErrorCode::Invalid),
            // A move into itself is the same cycle, one level up.
            (Target::OutlineItem { key: Some(chapters) }, json!({"action": "outline.item.move", "parent": chapters, "index": 0}), ErrorCode::Invalid),
            // A copy that nests deeper than the room carries.
            (Target::OutlineItem { key: None }, json!({"action": "outline.item.restore", "subtree": deep_copy(40), "index": 0}), ErrorCode::Limit),
            // A copy whose title is over the limit.
            (
                Target::OutlineItem { key: None },
                json!({"action": "outline.item.restore", "subtree": [{"title": long_title, "page": 0}], "index": 0}),
                ErrorCode::Limit,
            ),
            // A copy that is small in nodes but too large to broadcast and journal:
            // the accepted operation has to fit the frame on the way out too.
            (
                Target::OutlineItem { key: None },
                json!({"action": "outline.item.restore", "subtree": wide_copy(300, 500), "index": 0}),
                ErrorCode::Limit,
            ),
        ];
        for (index, (target, payload, code)) in cases.into_iter().enumerate() {
            let attempt = op(&format!("bad-{index}"), ConflictClass::Structural, target, version, epoch, payload)?;
            let rejected = room.apply(&editor, &attempt, 0).err().ok_or("a bad operation must be refused")?;
            assert_eq!(rejected.code, code, "{}", rejected.message);
            assert_eq!((room.version(), room.epoch()), (version, epoch), "a refusal moves nothing");
            assert_eq!(outline(room.adapter()), before, "a refusal writes nothing");
        }
        Ok(())
    }

    /// A copy of `count` items with `title` bytes each: many nodes that fit the
    /// count bound but not the copy's byte budget.
    fn wide_copy(count: usize, title: usize) -> Value {
        let title = "t".repeat(title);
        json!((0..count).map(|_| json!({"title": title, "page": 0})).collect::<Vec<_>>())
    }

    /// A subtree of `depth` nested nodes, for the restore's depth bound.
    fn deep_copy(depth: usize) -> Value {
        let mut node = json!({"title": "bottom", "page": 0, "children": []});
        for _ in 1..depth {
            node = json!({"title": "level", "page": 0, "children": [node]});
        }
        json!([node])
    }

    /// A page operation renumbers pages but not what a bookmark goes to: an item
    /// addressed before the operation is still found after it, because the key is
    /// the page-independent object and the server recomputes the path.
    #[test]
    fn page_operations_keep_the_outline_targets_and_the_keys() -> Result<(), Box<dyn std::error::Error>> {
        let (mut room, editor) = room_with_outline()?;
        let two = key_of(room.adapter(), "Two");
        let insert = op(
            "ins-1",
            ConflictClass::Structural,
            Target::Page(PageRef { index: 0, epoch: 0 }),
            0,
            0,
            json!({"action": "page.insert_blank", "width": 100.0, "height": 200.0}),
        )?;
        room.apply(&editor, &insert, 0)?;
        assert_eq!(outline(room.adapter()), ["Start→4", "Chapters→2", "One→2", "Two→3"], "each bookmark points at its page, one number later");
        // The key taken before the insert still addresses the same item.
        let rename = op(
            "upd-1",
            ConflictClass::Structural,
            Target::OutlineItem { key: Some(two) },
            room.version(),
            room.epoch(),
            json!({"action": "outline.item.update", "title": "Deux"}),
        )?;
        let applied = room.apply(&editor, &rename, 0)?;
        assert_eq!(applied.op.payload["path"], json!([1, 1]), "the server recomputed the path");
        assert_eq!(outline(room.adapter()), ["Start→4", "Chapters→2", "One→2", "Deux→3"]);

        // A split takes the first two pages out: bookmarks to them lose their
        // destination, the others keep their page and their key.
        let split = op(
            "split-1",
            ConflictClass::Structural,
            Target::Page(PageRef { index: 0, epoch: room.epoch() }),
            room.version(),
            room.epoch(),
            json!({"action": "page.split", "count": 2}),
        )?;
        room.apply(&editor, &split, 0)?;
        assert_eq!(outline(room.adapter()), ["Start→2", "Chapters→0", "One→0", "Deux→1"]);
        // The named destination that opened the fourth page still opens that page,
        // which is now the second: the destination holds the page object, not a
        // number, so the split moved the number and kept the target.
        assert_eq!(named(room.adapter(), "intro").and_then(|destination| destination.page), Some(1));

        // A combine inserts a part, which brings its own bookmark: the outline
        // grows, and every existing key still addresses its item.
        let part = one_page_copy(&fixture_with_outline(), 2)?;
        let combine = op(
            "combine-1",
            ConflictClass::Structural,
            Target::Page(PageRef { index: 0, epoch: room.epoch() }),
            room.version(),
            room.epoch(),
            json!({"action": "page.combine", "count": 1, "bytes": base64_encode(&part)}),
        )?;
        room.apply(&editor, &combine, 0)?;
        let after_combine = outline(room.adapter());
        assert!(after_combine.iter().any(|row| row.starts_with("combined-part")), "the part brings its bookmark: {after_combine:?}");
        assert!(after_combine.contains(&"Deux→2".to_owned()), "the bookmark kept its page: {after_combine:?}");
        let rename = op(
            "upd-2",
            ConflictClass::Structural,
            Target::OutlineItem { key: Some(two) },
            room.version(),
            room.epoch(),
            json!({"action": "outline.item.update", "title": "Deuxième"}),
        )?;
        room.apply(&editor, &rename, 0)?;
        assert!(outline(room.adapter()).iter().any(|row| row.starts_with("Deuxième")), "the key outlived insert, split and combine");

        // A replacement keeps the page object, so a bookmark to it stays put.
        let replacement = one_page_copy(&fixture(4), 1)?;
        let page = outline(room.adapter()).iter().position(|row| row.starts_with("One")).ok_or("One is listed")?;
        let _ = page;
        let replace = op(
            "replace-1",
            ConflictClass::Structural,
            Target::Page(PageRef { index: 0, epoch: room.epoch() }),
            room.version(),
            room.epoch(),
            json!({"action": "page.replace", "bytes": base64_encode(&replacement)}),
        )?;
        room.apply(&editor, &replace, 0)?;
        assert!(outline(room.adapter()).iter().any(|row| row.starts_with("Deuxième")), "a replacement keeps the outline");
        Ok(())
    }

    /// Named destinations are operations of their own: added, moved, deleted, and
    /// refused when the name is taken, missing or not a page.
    #[test]
    fn named_destinations_are_operations() -> Result<(), Box<dyn std::error::Error>> {
        let (mut room, editor) = room_with_outline()?;
        assert_eq!(named(room.adapter(), "intro").and_then(|destination| destination.page), Some(2));

        let add = op(
            "dest-1",
            ConflictClass::Structural,
            Target::NamedDest { name: "appendix".into() },
            0,
            0,
            json!({"action": "dest.add", "dest": {"kind": "page", "page": 3, "left": 5.0, "top": 100.0}}),
        )?;
        let applied = room.apply(&editor, &add, 0)?;
        assert_eq!(applied.op.payload["name"], "appendix");
        assert_eq!(named(room.adapter(), "appendix").and_then(|destination| destination.page), Some(3));

        // An add of a name that is taken is a conflict, not an overwrite.
        let duplicate = op(
            "dest-2",
            ConflictClass::Structural,
            Target::NamedDest { name: "appendix".into() },
            room.version(),
            room.epoch(),
            json!({"action": "dest.add", "dest": {"kind": "page", "page": 0}}),
        )?;
        assert!(matches!(room.apply(&editor, &duplicate, 0), Err(rejected) if rejected.code == ErrorCode::Conflict));

        let update = op(
            "dest-3",
            ConflictClass::Structural,
            Target::NamedDest { name: "appendix".into() },
            room.version(),
            room.epoch(),
            json!({"action": "dest.update", "dest": {"kind": "page", "page": 0}}),
        )?;
        let applied = room.apply(&editor, &update, 0)?;
        assert_eq!(applied.op.payload["before"]["page"], 3, "the undo image names where it was");
        assert_eq!(named(room.adapter(), "appendix").and_then(|destination| destination.page), Some(0));

        let delete = op(
            "dest-4",
            ConflictClass::Structural,
            Target::NamedDest { name: "appendix".into() },
            room.version(),
            room.epoch(),
            json!({"action": "dest.delete"}),
        )?;
        let applied = room.apply(&editor, &delete, 0)?;
        assert_eq!(applied.op.payload["before"]["page"], 0, "the copy names where it went");
        assert!(named(room.adapter(), "appendix").is_none());

        // What cannot be kept is refused before anything changes.
        let (version, epoch) = (room.version(), room.epoch());
        let cases: Vec<(Value, ErrorCode)> = vec![
            (json!({"action": "dest.update", "dest": {"kind": "page", "page": 0}}), ErrorCode::Conflict),
            (json!({"action": "dest.add", "dest": {"kind": "page", "page": 99}}), ErrorCode::Invalid),
            (json!({"action": "dest.add", "dest": {"kind": "named", "name": "intro"}}), ErrorCode::Invalid),
            (json!({"action": "dest.add", "dest": {"kind": "nowhere"}}), ErrorCode::Invalid),
            (json!({"action": "dest.delete"}), ErrorCode::Conflict),
        ];
        for (index, (payload, code)) in cases.into_iter().enumerate() {
            let name = if index == 1 { "fresh".to_owned() } else { "appendix".to_owned() };
            let attempt = op(&format!("bad-dest-{index}"), ConflictClass::Structural, Target::NamedDest { name }, version, epoch, payload)?;
            let rejected = room.apply(&editor, &attempt, 0).err().ok_or("a bad destination operation must be refused")?;
            assert_eq!(rejected.code, code, "{}", rejected.message);
            assert_eq!((room.version(), room.epoch()), (version, epoch));
        }
        // A blank name never reaches the adapter.
        let blank =
            op("bad-name", ConflictClass::Structural, Target::NamedDest { name: "  ".into() }, version, epoch, json!({"action": "dest.delete"}))?;
        assert!(matches!(room.apply(&editor, &blank, 0), Err(rejected) if rejected.code == ErrorCode::Invalid));
        Ok(())
    }

    /// A viewer (and a commenter) may not touch the outline: the room refuses it
    /// before the adapter runs, so neither the version nor the document moves.
    #[test]
    fn a_viewer_cannot_change_the_outline() -> Result<(), Box<dyn std::error::Error>> {
        let (mut room, _editor) = room_with_outline()?;
        let viewer = ActorId::new("viewer")?;
        room.hello(viewer.clone(), Role::Viewer, &hello()?, 0)?;
        let commenter = ActorId::new("commenter")?;
        room.hello(commenter.clone(), Role::Commenter, &hello()?, 0)?;
        let before = outline(room.adapter());
        let (version, epoch) = (room.version(), room.epoch());
        let attempt = op(
            "viewer-add",
            ConflictClass::Structural,
            Target::OutlineItem { key: None },
            0,
            0,
            json!({"action": "outline.item.add", "title": "Mine", "dest": {"kind": "page", "page": 0}}),
        )?;
        for actor in [&viewer, &commenter] {
            let rejected = room.apply(actor, &attempt, 0).err().ok_or("a viewer cannot write")?;
            assert_eq!(rejected.code, ErrorCode::Forbidden);
        }
        let destination = op(
            "viewer-dest",
            ConflictClass::Structural,
            Target::NamedDest { name: "mine".into() },
            0,
            0,
            json!({"action": "dest.add", "dest": {"kind": "page", "page": 0}}),
        )?;
        assert!(matches!(room.apply(&viewer, &destination, 0), Err(rejected) if rejected.code == ErrorCode::Forbidden));
        assert_eq!((room.version(), room.epoch()), (version, epoch));
        assert_eq!(outline(room.adapter()), before);
        assert!(named(room.adapter(), "mine").is_none());
        Ok(())
    }
}
