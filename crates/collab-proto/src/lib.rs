//! Wire protocol for collaborative PDF editing.
//!
//! One envelope for all Craft editors (`ARCHITECTURE.md`); this crate carries the
//! PDF adapter's messages and the rules shared by every adapter:
//!
//! - the server executes a normalised intent and broadcasts the authoritative
//!   result (ADR-0001);
//! - one monotonic server `version`, `opId` de-duplication, `baseVersion` for
//!   structural operations and a PDF `epoch` (ADR-0002, ADR-0003);
//! - merge conflicts are last-writer-wins per annotation property; structural
//!   operations are exclusive (ADR-0004);
//! - presence is ephemeral, capped and never journaled (ADR-0006);
//! - roles are checked by the server, never by the client (ADR-0008).
//!
//! The crate is transport- and storage-agnostic: no socket, no clock, no file
//! system. Parsing and validation are pure, so the same envelope is used by the
//! native server, the web client and the headless tests.
//!
//! The wire format is not `pdfcraft_engine::Edit`: that enum is not serialisable,
//! carries blobs and addresses pages by position (`docs/collab/study.md` §3).

#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{HashSet, VecDeque};

/// The only protocol version this crate understands.
pub const PROTOCOL_VERSION: u8 = 1;

/// Largest JSON frame, in bytes (RFC §4.1).
pub const MAX_MESSAGE_BYTES: usize = 256 * 1024;

/// Largest presence message, in bytes (ADR-0006).
pub const MAX_PRESENCE_BYTES: usize = 2 * 1024;

/// Highest presence rate, in messages per second (ADR-0006).
pub const PRESENCE_HZ: u32 = 10;

/// How long a peer's presence survives without an update, in seconds (ADR-0006).
pub const PRESENCE_TTL_SECS: u64 = 30;

/// Highest page index accepted in an address. PDF allows far more, but a bound
/// keeps hostile input from forcing huge allocations in adapters.
pub const MAX_PAGE_INDEX: u32 = 1_000_000;

/// Longest identifier (actor, session, document, operation) accepted.
pub const MAX_ID_BYTES: usize = 128;

/// Longest annotation name (`/NM`) accepted.
pub const MAX_ANNOTATION_NAME: usize = 128;

/// Largest bookmark path accepted.
pub const MAX_BOOKMARK_DEPTH: usize = 64;

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ProtocolError {
    #[error("unsupported protocol version {0}")]
    Version(u8),
    #[error("{0} is empty")]
    Empty(&'static str),
    #[error("{field} is {len} bytes; the limit is {max}")]
    TooLong { field: &'static str, len: usize, max: usize },
    #[error("page index {0} is above the limit")]
    PageIndex(u32),
    #[error("an operation cannot have a null payload")]
    NullPayload,
    #[error("a structural operation must use the structural conflict class, and only one")]
    StructuralClass,
    #[error("presence is {size} bytes; the limit is {max}")]
    PresenceTooLarge { size: usize, max: usize },
    #[error("message is {size} bytes; the limit is {max}")]
    MessageTooLarge { size: usize, max: usize },
    #[error("cannot encode message: {0}")]
    Encode(String),
}

macro_rules! id_type {
    ($name:ident, $label:literal) => {
        #[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
        #[serde(transparent)]
        pub struct $name(String);

        impl $name {
            /// An identifier must be non-empty and at most [`MAX_ID_BYTES`] bytes.
            pub fn new(value: impl Into<String>) -> Result<Self, ProtocolError> {
                let value = value.into();
                validate_id(&value, $label)?;
                Ok(Self(value))
            }

            pub fn as_str(&self) -> &str {
                &self.0
            }
        }

        impl std::fmt::Display for $name {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str(&self.0)
            }
        }
    };
}

id_type!(ActorId, "actor id");
id_type!(SessionId, "session id");
id_type!(DocKey, "document key");
id_type!(OpId, "operation id");

fn validate_id(value: &str, field: &'static str) -> Result<(), ProtocolError> {
    if value.trim().is_empty() {
        return Err(ProtocolError::Empty(field));
    }
    if value.len() > MAX_ID_BYTES {
        return Err(ProtocolError::TooLong { field, len: value.len(), max: MAX_ID_BYTES });
    }
    Ok(())
}

/// The acting user's role for one session (ADR-0008).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Role {
    Owner,
    Editor,
    Commenter,
    Viewer,
}

impl Role {
    /// Operators that change the document. A `viewer` may only read and send presence.
    pub fn can_write(self) -> bool {
        !matches!(self, Role::Viewer)
    }

    /// Page structure, bookmarks, field structure and other structural operations.
    pub fn can_structural(self) -> bool {
        matches!(self, Role::Owner | Role::Editor)
    }

    /// Comments, annotations and form field values.
    pub fn can_annotate(self) -> bool {
        matches!(self, Role::Owner | Role::Editor | Role::Commenter)
    }
}

/// The form of an operation (ADR-0001).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OpKind {
    Command,
    Primitive,
    Structural,
}

/// The conflict class the adapter assigns to an operation (ADR-0004).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConflictClass {
    /// Merged into the current head; never rewinds another user's edit.
    Merge,
    /// Exclusive: requires `base_version == head`, and bumps the PDF epoch.
    Structural,
}

/// One page addressed by its position and the epoch that position belongs to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct PageRef {
    pub index: u32,
    pub epoch: u64,
}

impl PageRef {
    pub fn new(index: u32, epoch: u64) -> Result<Self, ProtocolError> {
        if index > MAX_PAGE_INDEX {
            return Err(ProtocolError::PageIndex(index));
        }
        Ok(Self { index, epoch })
    }
}

/// What an operation acts on. The address is the session-scoped identity
/// (`ADR-0002`), never a client-side `Selection`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Target {
    Page(PageRef),
    Annotation { page: PageRef, nm: String },
    Field { name: String },
    Attachment { key: String },
    Bookmark { path: Vec<u32>, epoch: u64 },
    Layer { num: u32, generation: u16 },
}

impl Target {
    fn validate(&self) -> Result<(), ProtocolError> {
        match self {
            Target::Page(page) => page.validate(),
            Target::Annotation { page, nm } => {
                page.validate()?;
                if nm.trim().is_empty() {
                    return Err(ProtocolError::Empty("annotation name"));
                }
                if nm.len() > MAX_ANNOTATION_NAME {
                    return Err(ProtocolError::TooLong { field: "annotation name", len: nm.len(), max: MAX_ANNOTATION_NAME });
                }
                Ok(())
            }
            Target::Field { name } => validate_id(name, "field name"),
            Target::Attachment { key } => validate_id(key, "attachment key"),
            Target::Bookmark { path, .. } => {
                if path.len() > MAX_BOOKMARK_DEPTH {
                    return Err(ProtocolError::TooLong { field: "bookmark path", len: path.len(), max: MAX_BOOKMARK_DEPTH });
                }
                Ok(())
            }
            Target::Layer { .. } => Ok(()),
        }
    }
}

impl PageRef {
    fn validate(&self) -> Result<(), ProtocolError> {
        if self.index > MAX_PAGE_INDEX { Err(ProtocolError::PageIndex(self.index)) } else { Ok(()) }
    }
}

/// A client intent. It is not `Edit`: the server translates it through the
/// PDF adapter and applies it with `Session::apply`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Op {
    pub op_id: OpId,
    pub doc: DocKey,
    pub base_version: u64,
    pub epoch: u64,
    pub kind: OpKind,
    pub class: ConflictClass,
    pub target: Target,
    pub payload: Value,
    /// The client's optimistic preview, if it showed one.
    pub preview: Option<String>,
}

impl Op {
    pub fn validate(&self) -> Result<(), ProtocolError> {
        self.op_id.validate()?;
        self.doc.validate()?;
        self.target.validate()?;
        if self.payload.is_null() {
            return Err(ProtocolError::NullPayload);
        }
        if (self.kind == OpKind::Structural) != (self.class == ConflictClass::Structural) {
            return Err(ProtocolError::StructuralClass);
        }
        Ok(())
    }
}

impl ActorId {
    fn validate(&self) -> Result<(), ProtocolError> {
        validate_id(self.as_str(), "actor id")
    }
}

impl OpId {
    fn validate(&self) -> Result<(), ProtocolError> {
        validate_id(self.as_str(), "operation id")
    }
}

impl DocKey {
    fn validate(&self) -> Result<(), ProtocolError> {
        validate_id(self.as_str(), "document key")
    }
}

/// What the client tells the server when it connects. The JWT travels here and
/// nowhere else (ADR-0008).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Hello {
    pub v: u8,
    pub app: String,
    pub doc: DocKey,
    pub jwt: String,
    pub client: ClientInfo,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClientInfo {
    pub name: String,
    pub version: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Lease {
    pub id: String,
    pub ttl_secs: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Capabilities {
    pub kinds: Vec<OpKind>,
    pub classes: Vec<ConflictClass>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Limits {
    pub max_message_bytes: usize,
    pub max_presence_bytes: usize,
    pub presence_hz: u32,
    pub presence_ttl_secs: u64,
    pub max_participants: u32,
    pub max_ops_per_sec: u32,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            max_message_bytes: MAX_MESSAGE_BYTES,
            max_presence_bytes: MAX_PRESENCE_BYTES,
            presence_hz: PRESENCE_HZ,
            presence_ttl_secs: PRESENCE_TTL_SECS,
            max_participants: 8,
            max_ops_per_sec: 50,
        }
    }
}

/// The authoritative answer to `hello`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Welcome {
    pub v: u8,
    pub session_id: SessionId,
    pub actor_id: ActorId,
    pub role: Role,
    pub lease: Lease,
    pub version: u64,
    pub epoch: u64,
    pub capabilities: Capabilities,
    pub limits: Limits,
}

/// What the server accepted, verbatim: the client never invents the result.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Applied {
    pub op_id: OpId,
    pub actor_id: ActorId,
    pub version: u64,
    pub epoch: u64,
    /// The accepted operation, with the server's normalisation (the author from
    /// the token, a generated name, the resolved target). Every replica applies
    /// this description, so a client never needs the raw intent from the wire.
    pub op: Op,
    /// Adapter-specific authoritative result: a confirmed command, a delta, or both.
    pub result: Value,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ErrorCode {
    Auth,
    Lease,
    Stale,
    Conflict,
    Forbidden,
    Invalid,
    Limit,
    Unsupported,
    /// The server could not complete an operation for a reason the client
    /// cannot fix: the request is not retryable and repeating it will not help.
    Internal,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct VersionHint {
    pub version: u64,
    pub epoch: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, thiserror::Error)]
#[error("{message}")]
pub struct Rejected {
    pub op_id: OpId,
    pub code: ErrorCode,
    pub retryable: bool,
    pub hint: Option<VersionHint>,
    pub message: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Resync {
    pub last_version: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SnapshotMode {
    Base,
    BaseAndTail,
}

/// A blob addressed by hash; bytes travel out of band (RFC §4.1).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BlobRef {
    pub sha256: String,
    pub len: u64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Snapshot {
    pub mode: SnapshotMode,
    pub version: u64,
    pub epoch: u64,
    pub base: Option<BlobRef>,
    pub tail: Vec<Value>,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Viewport {
    pub x: f64,
    pub y: f64,
    pub zoom: f64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Selection {
    Annotation { nm: String },
    Text { page: PageRef, rect: [i64; 4] },
}

/// Ephemeral cursor/selection state. Never journaled, never a version bump.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Presence {
    pub actor_id: ActorId,
    pub role: Role,
    pub page: PageRef,
    pub viewport: Viewport,
    pub cursor: Option<[i64; 2]>,
    pub selection: Option<Selection>,
    /// A short colour token (`#RRGGBB`), chosen by the client.
    pub color: String,
}

impl Presence {
    pub fn validate(&self) -> Result<(), ProtocolError> {
        self.actor_id.validate()?;
        self.page.validate()?;
        if !self.viewport.x.is_finite() || !self.viewport.y.is_finite() || !self.viewport.zoom.is_finite() {
            return Err(ProtocolError::Empty("viewport"));
        }
        if self.color.len() > 32 {
            return Err(ProtocolError::TooLong { field: "presence colour", len: self.color.len(), max: 32 });
        }
        let size = serde_json::to_vec(self).map_err(|e| ProtocolError::Encode(e.to_string()))?.len();
        if size > MAX_PRESENCE_BYTES {
            return Err(ProtocolError::PresenceTooLarge { size, max: MAX_PRESENCE_BYTES });
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CommitRequest {
    pub expected_version: u64,
    pub message: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CommitResult {
    pub version: u64,
    pub sha256: String,
    pub bytes: u64,
    /// The file changed outside the session; the cabinet decides (`409`).
    pub conflict: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Ping {
    pub ts: u64,
}

/// Everything a client may send.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ClientMessage {
    Hello(Hello),
    Op(Op),
    Resync(Resync),
    Commit(CommitRequest),
    Presence(Presence),
    Ping(Ping),
}

/// Everything the server may send.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ServerMessage {
    Welcome(Welcome),
    Applied(Applied),
    Rejected(Rejected),
    Presence(Presence),
    Snapshot(Snapshot),
    CommitResult(CommitResult),
    Error(ServerError),
    Pong(Ping),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, thiserror::Error)]
#[error("{message}")]
pub struct ServerError {
    pub code: ErrorCode,
    pub message: String,
    pub retryable: bool,
}

impl ClientMessage {
    pub fn validate(&self) -> Result<(), ProtocolError> {
        match self {
            ClientMessage::Hello(hello) => {
                if hello.v != PROTOCOL_VERSION {
                    return Err(ProtocolError::Version(hello.v));
                }
                validate_id(&hello.app, "app")?;
                hello.doc.validate()?;
                if hello.jwt.trim().is_empty() {
                    return Err(ProtocolError::Empty("jwt"));
                }
                validate_id(&hello.client.name, "client name")?;
                Ok(())
            }
            ClientMessage::Op(op) => op.validate(),
            ClientMessage::Resync(_) | ClientMessage::Commit(_) | ClientMessage::Ping(_) => Ok(()),
            ClientMessage::Presence(presence) => presence.validate(),
        }
    }

    /// The encoded size, checked against [`MAX_MESSAGE_BYTES`].
    pub fn encoded_len(&self) -> Result<usize, ProtocolError> {
        let len = serde_json::to_vec(self).map_err(|e| ProtocolError::Encode(e.to_string()))?.len();
        if len > MAX_MESSAGE_BYTES {
            return Err(ProtocolError::MessageTooLarge { size: len, max: MAX_MESSAGE_BYTES });
        }
        Ok(len)
    }
}

impl ServerMessage {
    pub fn encoded_len(&self) -> Result<usize, ProtocolError> {
        let len = serde_json::to_vec(self).map_err(|e| ProtocolError::Encode(e.to_string()))?.len();
        if len > MAX_MESSAGE_BYTES {
            return Err(ProtocolError::MessageTooLarge { size: len, max: MAX_MESSAGE_BYTES });
        }
        Ok(len)
    }

    pub fn validate_presence(&self) -> Result<(), ProtocolError> {
        match self {
            ServerMessage::Presence(presence) => presence.validate(),
            _ => Ok(()),
        }
    }
}

/// A bounded set of operation ids used for de-duplication (ADR-0003). The
/// server keeps one per document; the window must survive a restart, so the
/// storage layer persists it.
#[derive(Debug, Clone)]
pub struct OpWindow {
    capacity: usize,
    order: VecDeque<String>,
    seen: HashSet<String>,
}

impl OpWindow {
    pub fn new(capacity: usize) -> Self {
        Self { capacity: capacity.max(1), order: VecDeque::new(), seen: HashSet::new() }
    }

    /// Record `op_id`; returns `true` when it had not been seen (the operation
    /// is fresh and must be applied), `false` for a retry.
    pub fn observe(&mut self, op_id: &str) -> bool {
        if self.seen.contains(op_id) {
            return false;
        }
        if self.order.len() == self.capacity
            && let Some(evicted) = self.order.pop_front()
        {
            self.seen.remove(&evicted);
        }
        self.order.push_back(op_id.to_owned());
        self.seen.insert(op_id.to_owned());
        true
    }

    pub fn contains(&self, op_id: &str) -> bool {
        self.seen.contains(op_id)
    }

    pub fn len(&self) -> usize {
        self.seen.len()
    }

    pub fn is_empty(&self) -> bool {
        self.seen.is_empty()
    }
}

/// The transport calls this before forwarding a frame (RFC §4.1).
pub fn validate_message_size(size: usize) -> Result<(), ProtocolError> {
    if size > MAX_MESSAGE_BYTES {
        return Err(ProtocolError::MessageTooLarge { size, max: MAX_MESSAGE_BYTES });
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn op(kind: OpKind, class: ConflictClass) -> Result<Op, ProtocolError> {
        Ok(Op {
            op_id: OpId::new("op-1")?,
            doc: DocKey::new("file-1")?,
            base_version: 7,
            epoch: 3,
            kind,
            class,
            target: Target::Annotation { page: PageRef { index: 2, epoch: 3 }, nm: "ann-1".into() },
            payload: serde_json::json!({"contents": "hello"}),
            preview: None,
        })
    }

    #[test]
    fn hello_round_trips() -> Result<(), Box<dyn std::error::Error>> {
        let msg = ClientMessage::Hello(Hello {
            v: PROTOCOL_VERSION,
            app: "pdfcraft".into(),
            doc: DocKey::new("file-1")?,
            jwt: "token".into(),
            client: ClientInfo { name: "pdfcraft-web".into(), version: "0.1".into() },
        });
        msg.validate()?;
        let encoded = serde_json::to_string(&msg)?;
        let decoded: ClientMessage = serde_json::from_str(&encoded)?;
        assert_eq!(decoded, msg);
        Ok(())
    }

    #[test]
    fn structural_op_round_trips() -> Result<(), Box<dyn std::error::Error>> {
        let mut structural = op(OpKind::Structural, ConflictClass::Structural)?;
        structural.target = Target::Page(PageRef::new(2, 3)?);
        structural.payload = serde_json::json!({"rotate": 90});
        let msg = ClientMessage::Op(structural);
        msg.validate()?;
        let encoded = serde_json::to_string(&msg)?;
        assert!(encoded.contains("\"type\":\"op\""));
        let decoded: ClientMessage = serde_json::from_str(&encoded)?;
        assert_eq!(decoded, msg);
        Ok(())
    }

    #[test]
    fn merge_op_round_trips() -> Result<(), Box<dyn std::error::Error>> {
        let msg = ClientMessage::Op(op(OpKind::Primitive, ConflictClass::Merge)?);
        msg.validate()?;
        let encoded = serde_json::to_string(&msg)?;
        let decoded: ClientMessage = serde_json::from_str(&encoded)?;
        assert_eq!(decoded, msg);
        Ok(())
    }

    #[test]
    fn well_formed_messages_validate() -> Result<(), Box<dyn std::error::Error>> {
        let presence = Presence {
            actor_id: ActorId::new("actor-1")?,
            role: Role::Commenter,
            page: PageRef::new(1, 2)?,
            viewport: Viewport { x: 10.0, y: 20.0, zoom: 1.5 },
            cursor: Some([10, 20]),
            selection: Some(Selection::Annotation { nm: "ann-1".into() }),
            color: "#112233".into(),
        };
        ClientMessage::Presence(presence.clone()).validate()?;
        ServerMessage::Presence(presence).validate_presence()?;
        ClientMessage::Resync(Resync { last_version: 4 }).validate()?;
        ClientMessage::Commit(CommitRequest { expected_version: 4, message: None }).validate()?;
        ServerMessage::Error(ServerError { code: ErrorCode::Stale, message: "retry".into(), retryable: true }).encoded_len()?;
        Ok(())
    }

    #[test]
    fn presence_over_the_limit_is_refused() -> Result<(), Box<dyn std::error::Error>> {
        let presence = Presence {
            actor_id: ActorId::new("actor-1")?,
            role: Role::Viewer,
            page: PageRef::new(1, 0)?,
            viewport: Viewport { x: 0.0, y: 0.0, zoom: 1.0 },
            cursor: None,
            selection: Some(Selection::Annotation { nm: "a".repeat(MAX_PRESENCE_BYTES) }),
            color: "#112233".into(),
        };
        assert!(matches!(presence.validate(), Err(ProtocolError::PresenceTooLarge { .. })));
        Ok(())
    }

    #[test]
    fn unsupported_version_is_refused() -> Result<(), Box<dyn std::error::Error>> {
        let msg = ClientMessage::Hello(Hello {
            v: 2,
            app: "pdfcraft".into(),
            doc: DocKey::new("file-1")?,
            jwt: "token".into(),
            client: ClientInfo { name: "web".into(), version: "0".into() },
        });
        assert_eq!(msg.validate(), Err(ProtocolError::Version(2)));
        Ok(())
    }

    #[test]
    fn structural_must_not_be_a_merge() -> Result<(), Box<dyn std::error::Error>> {
        let bad = op(OpKind::Structural, ConflictClass::Merge)?;
        assert!(matches!(bad.validate(), Err(ProtocolError::StructuralClass)));
        let also_bad = op(OpKind::Command, ConflictClass::Structural)?;
        assert!(matches!(also_bad.validate(), Err(ProtocolError::StructuralClass)));
        Ok(())
    }

    #[test]
    fn empty_and_long_ids_are_refused() {
        assert!(ActorId::new("").is_err());
        assert!(ActorId::new("   ").is_err());
        assert!(SessionId::new("x".repeat(MAX_ID_BYTES + 1)).is_err());
    }

    #[test]
    fn role_permissions_follow_the_task() {
        assert!(Role::Owner.can_structural());
        assert!(!Role::Commenter.can_structural());
        assert!(Role::Commenter.can_annotate());
        assert!(!Role::Viewer.can_annotate());
        assert!(!Role::Viewer.can_write());
    }

    #[test]
    fn op_window_deduplicates_and_evicts() {
        let mut window = OpWindow::new(2);
        assert!(window.observe("a"));
        assert!(window.observe("b"));
        assert!(!window.observe("a"));
        assert!(window.observe("c"));
        assert!(!window.contains("a"));
        assert!(window.contains("b"));
        assert_eq!(window.len(), 2);
    }

    #[test]
    fn message_size_is_bounded() {
        assert!(validate_message_size(10).is_ok());
        assert!(matches!(validate_message_size(MAX_MESSAGE_BYTES + 1), Err(ProtocolError::MessageTooLarge { .. })));
    }

    #[test]
    fn unknown_type_is_not_a_message() {
        assert!(serde_json::from_str::<ClientMessage>("{\"type\":\"evil\"}").is_err());
    }
}
