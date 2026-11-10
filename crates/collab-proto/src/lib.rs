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

/// Most comment names one presence selection carries: the primary plus the
/// others selected with it. Well inside [`MAX_PRESENCE_BYTES`].
pub const MAX_SELECTION_NAMES: usize = 32;

/// Most per-line rectangles one presence selection carries. A larger selection
/// falls back to its bounding box, so the frame stays under the limit.
pub const MAX_SELECTION_QUADS: usize = 48;

/// Largest bookmark path accepted.
pub const MAX_BOOKMARK_DEPTH: usize = 64;

/// Deepest an outline item may sit, counting the top level as 1. This is the
/// depth the engine's own listing reaches, so an item deeper than this cannot be
/// addressed by a path at all; an operation that would put one there is refused.
pub const MAX_OUTLINE_DEPTH: usize = 32;

/// Longest outline item title, in bytes; the frame limit is what a single
/// operation may carry, this is what a title may be.
pub const MAX_OUTLINE_TITLE: usize = 512;

/// Most outline items a room accepts in one document (the document's own
/// outline, plus what the room adds).
pub const MAX_OUTLINE_ITEMS: usize = 10_000;

/// Longest named destination name, in bytes.
pub const MAX_DEST_NAME: usize = 128;

/// Most named destinations a room accepts in one document.
pub const MAX_NAMED_DESTINATIONS: usize = 10_000;

/// Most nodes one `outline.item.move` may carry as its undo image. A subtree
/// over this is refused before anything changes; the frame bound catches the
/// rest.
pub const MAX_OUTLINE_COPY_NODES: usize = 2_000;

/// Largest subtree copy, in bytes, that one operation may carry. The `applied`
/// frame is [`MAX_MESSAGE_BYTES`], and the copy has to fit in it beside the
/// operation around it, so the bound is half of it. A delete whose subtree is
/// bigger is refused with `limit` before the document changes.
pub const MAX_OUTLINE_COPY_BYTES: usize = MAX_MESSAGE_BYTES / 2;

/// Largest one-page copy an operation may carry (RFC §6.1). Base64 of it still
/// fits one frame, and a page over this is refused before any mutation: an
/// insert, a duplicate or a restore either travels whole or does not happen.
pub const MAX_PAGE_COPY_BYTES: usize = 160 * 1024;

/// Longest page label, in UTF-8 bytes, and the longest prefix of one: the
/// ISO 32000-2 bound every reader is asked to accept. A range whose label would
/// be longer is refused before the document changes.
pub const MAX_PAGE_LABEL_BYTES: usize = 1024;

/// Most label ranges a room accepts in one document. The PDF number tree can
/// hold more, but a range list this long no longer fits what one operation can
/// reasonably rewrite; a set that would cross it is refused (`limit`).
pub const MAX_PAGE_LABEL_RANGES: usize = 1_024;

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ProtocolError {
    #[error("unsupported protocol version {0}")]
    Version(u8),
    #[error("{0} is empty")]
    Empty(&'static str),
    #[error("{field} is {len} bytes; the limit is {max}")]
    TooLong { field: &'static str, len: usize, max: usize },
    #[error("{field}: {count} is above the limit of {max}")]
    TooMany { field: &'static str, count: usize, max: usize },
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

/// The numbering style of a page-label range (ISO 32000-2 §12.4.2, Table 159),
/// as the wire names it. `None` writes the prefix alone, with no number.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PageLabelStyle {
    /// 1, 2, 3
    Decimal,
    /// I, II, III
    UpperRoman,
    /// i, ii, iii
    LowerRoman,
    /// A, B, C
    UpperAlpha,
    /// a, b, c
    LowerAlpha,
    /// The prefix alone.
    None,
}

impl PageLabelStyle {
    /// The style as the wire writes it.
    pub const fn name(self) -> &'static str {
        match self {
            Self::Decimal => "decimal",
            Self::UpperRoman => "upper_roman",
            Self::LowerRoman => "lower_roman",
            Self::UpperAlpha => "upper_alpha",
            Self::LowerAlpha => "lower_alpha",
            Self::None => "none",
        }
    }

    /// The style a wire name means, or `None` for a name this protocol does not
    /// know (never a guess).
    pub fn parse(name: &str) -> Option<Self> {
        Some(match name {
            "decimal" => Self::Decimal,
            "upper_roman" => Self::UpperRoman,
            "lower_roman" => Self::LowerRoman,
            "upper_alpha" => Self::UpperAlpha,
            "lower_alpha" => Self::LowerAlpha,
            "none" => Self::None,
            _ => return None,
        })
    }
}

/// One page addressed by its position and the epoch that position belongs to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct PageRef {
    pub index: u32,
    pub epoch: u64,
}

/// An outline item's address: the object reference it has in the document.
///
/// A bookmark's path (`[0, 2]`) is a position, and positions move when anything
/// is added or removed above them; its object does not. Page objects keep their
/// identity through an insert, a split, a combine and a replacement, which is
/// why a destination array names a page object rather than a page number, and
/// an outline item is addressed the same way (`docs/collab/RFC.md` §3.1.7).
/// The pair is the PDF writer's own indirect reference (`num` `gen` `R`); an
/// item that no longer exists, or one the room cannot see, is `invalid`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct OutlineKey {
    pub num: u32,
    pub generation: u16,
}

impl OutlineKey {
    pub const fn new(num: u32, generation: u16) -> Self {
        Self { num, generation }
    }

    /// The key as the wire writes it: `12 0 R` without the spaces, so a log line
    /// or an error message can name it.
    pub fn as_string(self) -> String {
        format!("{} {} R", self.num, self.generation)
    }
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
    Annotation {
        page: PageRef,
        nm: String,
    },
    Field {
        name: String,
    },
    Attachment {
        key: String,
    },
    Bookmark {
        path: Vec<u32>,
        epoch: u64,
    },
    Layer {
        num: u32,
        generation: u16,
    },
    /// An outline item, by its object reference. `None` is the outline itself:
    /// the parent of the top level, which an add or a move into it names.
    OutlineItem {
        key: Option<OutlineKey>,
    },
    /// A named destination, by its name (`/Names /Dests`).
    NamedDest {
        name: String,
    },
    /// A page-label range, by the page it starts on (0-based). `page.label.set`
    /// labels a span that begins here; `page.label.clear` removes the range that
    /// starts here.
    PageLabel {
        start: u32,
    },
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
            Target::OutlineItem { key } => match key {
                // Object 0 is the free list's head, never an outline item.
                Some(key) if key.num == 0 => Err(ProtocolError::Empty("outline key")),
                _ => Ok(()),
            },
            Target::NamedDest { name } => {
                if name.trim().is_empty() {
                    return Err(ProtocolError::Empty("named destination"));
                }
                if name.len() > MAX_DEST_NAME {
                    return Err(ProtocolError::TooLong { field: "named destination", len: name.len(), max: MAX_DEST_NAME });
                }
                Ok(())
            }
            Target::PageLabel { start } => {
                if *start > MAX_PAGE_INDEX {
                    return Err(ProtocolError::PageIndex(*start));
                }
                Ok(())
            }
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
    /// The document is busy with a call the server must finish first (a commit
    /// being published to the cabinet). The request is retryable: nothing was
    /// changed by it.
    Busy,
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

/// What a participant has selected, as presence carries it: a comment name or
/// geometry, never the document's text (ADR-0006). Both shapes are additive over
/// the first wire form: a peer that predates the extra fields ignores them, and
/// the required ones (`nm`, `rect`) still describe the selection on their own.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Selection {
    /// A selected comment and any others selected with it, by `/NM`. `nm` is the
    /// one the sender acts on (it may be the only one); `also` lists the rest, so
    /// an older peer still sees the primary selection.
    Annotation {
        nm: String,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        also: Vec<String>,
    },
    /// A text range: its bounding box, the per-line pieces that make it up, and
    /// the caret (the active end, where typing would start).
    Text {
        page: PageRef,
        rect: [i64; 4],
        /// One rectangle per selected line piece, in reading order; empty when the
        /// sender only knows the bounding box.
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        quads: Vec<[i64; 4]>,
        /// The caret in user space, at the active end of the range.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        caret: Option<[i64; 2]>,
    },
}

impl Selection {
    /// The `/NM` names of a comment selection, primary first.
    pub fn names(&self) -> Vec<&str> {
        match self {
            Self::Annotation { nm, also } => std::iter::once(nm.as_str()).chain(also.iter().map(String::as_str)).collect(),
            Self::Text { .. } => Vec::new(),
        }
    }

    /// A selection must stay small and must not carry text: see [`MAX_SELECTION_NAMES`],
    /// [`MAX_SELECTION_QUADS`] and [`MAX_PRESENCE_BYTES`].
    pub fn validate(&self) -> Result<(), ProtocolError> {
        match self {
            Self::Annotation { nm, also } => {
                let names = std::iter::once(nm).chain(also.iter());
                let mut count = 0;
                for name in names {
                    validate_id(name, "annotation name")?;
                    count += 1;
                }
                if count > MAX_SELECTION_NAMES {
                    return Err(ProtocolError::TooMany { field: "selection names", count, max: MAX_SELECTION_NAMES });
                }
            }
            Self::Text { page, quads, .. } => {
                page.validate()?;
                if quads.len() > MAX_SELECTION_QUADS {
                    return Err(ProtocolError::TooMany { field: "selection quads", count: quads.len(), max: MAX_SELECTION_QUADS });
                }
            }
        }
        Ok(())
    }
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
        if let Some(selection) = &self.selection {
            selection.validate()?;
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
            selection: Some(Selection::Annotation { nm: "ann-1".into(), also: Vec::new() }),
            color: "#112233".into(),
        };
        ClientMessage::Presence(presence.clone()).validate()?;
        ServerMessage::Presence(presence).validate_presence()?;
        ClientMessage::Resync(Resync { last_version: 4 }).validate()?;
        ClientMessage::Commit(CommitRequest { expected_version: 4, message: None }).validate()?;
        ServerMessage::Error(ServerError { code: ErrorCode::Stale, message: "retry".into(), retryable: true }).encoded_len()?;
        Ok(())
    }

    /// A selection that only the first wire form describes still parses: `also`,
    /// `quads` and `caret` are additive, so a peer that predates them keeps
    /// sending (and reading) the plain form.
    #[test]
    fn the_plain_selection_form_still_parses() -> Result<(), Box<dyn std::error::Error>> {
        let text: Selection = serde_json::from_str(r#"{"kind":"text","page":{"index":3,"epoch":1},"rect":[1,2,3,4]}"#)?;
        assert_eq!(text, Selection::Text { page: PageRef { index: 3, epoch: 1 }, rect: [1, 2, 3, 4], quads: Vec::new(), caret: None });
        assert_eq!(serde_json::to_string(&text)?, r#"{"kind":"text","page":{"index":3,"epoch":1},"rect":[1,2,3,4]}"#);
        let named: Selection = serde_json::from_str(r#"{"kind":"annotation","nm":"ada-hl-1"}"#)?;
        assert_eq!(named, Selection::Annotation { nm: "ada-hl-1".into(), also: Vec::new() });
        assert_eq!(named.names(), ["ada-hl-1"]);
        Ok(())
    }

    /// Several comments travel as one list, and a text range as its per-line
    /// pieces plus the caret.
    #[test]
    fn a_selection_lists_names_quads_and_a_caret() -> Result<(), Box<dyn std::error::Error>> {
        let names = Selection::Annotation { nm: "ada-hl-1".into(), also: vec!["ada-hl-2".into(), "bo-note".into()] };
        assert_eq!(names.names(), ["ada-hl-1", "ada-hl-2", "bo-note"]);
        names.validate()?;
        let text = Selection::Text {
            page: PageRef { index: 1, epoch: 2 },
            rect: [10, 90, 50, 100],
            quads: vec![[10, 95, 50, 100], [10, 90, 30, 95]],
            caret: Some([30, 90]),
        };
        text.validate()?;
        let encoded = serde_json::to_string(&text)?;
        assert!(encoded.contains(r#""quads":[[10,95,50,100],[10,90,30,95]]"#));
        assert!(encoded.contains(r#""caret":[30,90]"#));
        assert_eq!(serde_json::from_str::<Selection>(&encoded)?, text);
        Ok(())
    }

    /// The bounds keep a hostile or simply large selection from filling a frame:
    /// too many names or per-line pieces are refused before anything is drawn.
    #[test]
    fn selection_limits_are_enforced() -> Result<(), Box<dyn std::error::Error>> {
        let many = Selection::Annotation { nm: "nm".into(), also: (0..MAX_SELECTION_NAMES).map(|i| format!("nm-{i}")).collect() };
        assert!(matches!(many.validate(), Err(ProtocolError::TooMany { field: "selection names", .. })));
        let empty = Selection::Annotation { nm: String::new(), also: Vec::new() };
        assert_eq!(empty.validate(), Err(ProtocolError::Empty("annotation name")));
        let quads = Selection::Text {
            page: PageRef { index: 0, epoch: 0 },
            rect: [0, 0, 1, 1],
            quads: vec![[0, 0, 1, 1]; MAX_SELECTION_QUADS + 1],
            caret: None,
        };
        assert!(matches!(quads.validate(), Err(ProtocolError::TooMany { field: "selection quads", .. })));
        let off_page = Selection::Text { page: PageRef { index: MAX_PAGE_INDEX + 1, epoch: 0 }, rect: [0, 0, 1, 1], quads: Vec::new(), caret: None };
        assert_eq!(off_page.validate(), Err(ProtocolError::PageIndex(MAX_PAGE_INDEX + 1)));
        Ok(())
    }

    #[test]
    fn presence_over_the_limit_is_refused() -> Result<(), Box<dyn std::error::Error>> {
        // Within the per-field limits, the encoded frame is still too big when the
        // coordinates are large: the size check catches what the counts let through.
        let quads = vec![[i64::MAX, i64::MAX, i64::MAX, i64::MAX]; MAX_SELECTION_QUADS];
        let presence = Presence {
            actor_id: ActorId::new("actor-1")?,
            role: Role::Viewer,
            page: PageRef::new(1, 0)?,
            viewport: Viewport { x: 0.0, y: 0.0, zoom: 1.0 },
            cursor: None,
            selection: Some(Selection::Text { page: PageRef { index: 1, epoch: 0 }, rect: [0, 0, 1, 1], quads, caret: None }),
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
