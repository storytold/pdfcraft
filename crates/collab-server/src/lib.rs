//! Authoritative room state for one collaborative PDF.
//!
//! This crate is the transport-free core of `pdfcraft-collab-server` (RFC §9,
//! ADR-0003/0004/0006/0008/0009): one writer, one monotonic `version`, a bounded
//! operation journal, `opId` de-duplication, one editing lease, server-side role
//! checks and a resync tail. The adapter applies the operation to the document;
//! `crates/collab-pdf` (the engine adapter) and the WebSocket transport sit on
//! top of it.
//!
//! Time is supplied by the caller (`now_ms`), so every rule is testable without
//! a clock or a socket.

#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

use pdfcraft_collab_proto::{
    ActorId, Applied, BlobRef, Capabilities, CommitResult, ConflictClass, DocKey, ErrorCode, Hello, Lease, Limits, Op, OpId, Presence, Rejected,
    Resync, Role, ServerError, SessionId, Snapshot, SnapshotMode, VersionHint, Welcome,
};
use serde_json::Value;
use std::collections::{HashMap, VecDeque};

/// What an adapter returns after applying an operation. `epoch` is the epoch
/// after the operation; a structural operation must advance it.
#[derive(Debug, Clone, PartialEq)]
pub struct ApplyOutcome {
    pub result: Value,
    pub epoch: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommitOutcome {
    pub sha256: String,
    pub bytes: u64,
    pub conflict: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{message}")]
pub struct AdapterError {
    pub code: ErrorCode,
    pub message: String,
    pub retryable: bool,
}

impl AdapterError {
    pub fn new(code: ErrorCode, message: impl Into<String>, retryable: bool) -> Self {
        Self { code, message: message.into(), retryable }
    }
}

/// Applies one operation to the authoritative document and produces the file on
/// commit. The PDF implementation lives in the engine adapter; tests use a fake.
pub trait Adapter {
    fn apply(&mut self, op: &Op) -> Result<ApplyOutcome, AdapterError>;
    fn commit(&mut self) -> Result<CommitOutcome, AdapterError>;
    /// The current base bytes, when the journal no longer covers a resync.
    fn base(&self) -> Option<BlobRef>;
}

#[derive(Debug, Clone)]
pub struct RoomConfig {
    pub limits: Limits,
    /// How many applied operation ids are remembered for de-duplication.
    pub op_window: usize,
    /// How many journal entries the tail keeps before compaction must use a base.
    pub journal_len: usize,
    pub lease_ttl_secs: u64,
}

impl Default for RoomConfig {
    fn default() -> Self {
        Self { limits: Limits::default(), op_window: 10_000, journal_len: 10_000, lease_ttl_secs: 120 }
    }
}

#[derive(Debug, Clone)]
struct Participant {
    role: Role,
    last_presence_ms: Option<u64>,
    color: Option<String>,
}

#[derive(Debug, Clone)]
struct LeaseState {
    holder: ActorId,
    expires_ms: u64,
}

#[derive(Debug, Clone)]
struct JournalEntry {
    version: u64,
    op: Value,
}

/// One document's authoritative session. Not `Sync`: the transport owns the
/// room and serialises calls; the adapter may hold a non-`Send` renderer.
pub struct Room<A: Adapter> {
    session_id: SessionId,
    doc: DocKey,
    adapter: A,
    config: RoomConfig,
    version: u64,
    epoch: u64,
    participants: HashMap<ActorId, Participant>,
    lease: Option<LeaseState>,
    journal: VecDeque<JournalEntry>,
    applied_order: VecDeque<String>,
    applied: HashMap<String, Applied>,
}

impl<A: Adapter> Room<A> {
    pub fn new(session_id: SessionId, doc: DocKey, adapter: A, config: RoomConfig) -> Self {
        Self {
            session_id,
            doc,
            adapter,
            config,
            version: 0,
            epoch: 0,
            participants: HashMap::new(),
            lease: None,
            journal: VecDeque::new(),
            applied_order: VecDeque::new(),
            applied: HashMap::new(),
        }
    }

    pub fn version(&self) -> u64 {
        self.version
    }

    pub fn epoch(&self) -> u64 {
        self.epoch
    }

    pub fn participants(&self) -> usize {
        self.participants.len()
    }

    pub fn journal_len(&self) -> usize {
        self.journal.len()
    }

    /// Join. The role comes from the verified token, never from the body.
    pub fn hello(&mut self, actor: ActorId, role: Role, hello: &Hello, now_ms: u64) -> Result<Welcome, ServerError> {
        if hello.app != "pdfcraft" {
            return Err(server_error(ErrorCode::Unsupported, "this server only accepts pdfcraft sessions", false));
        }
        if hello.doc != self.doc {
            return Err(server_error(ErrorCode::Invalid, "the session belongs to another document", false));
        }
        let mut lease = Lease { id: format!("{}:{}", self.session_id, actor.as_str()), ttl_secs: 0 };
        if role.can_structural() && self.take_lease(&actor, now_ms) {
            lease.ttl_secs = self.config.lease_ttl_secs;
        }
        self.participants.entry(actor.clone()).and_modify(|p| p.role = role).or_insert(Participant { role, last_presence_ms: None, color: None });
        Ok(Welcome {
            v: pdfcraft_collab_proto::PROTOCOL_VERSION,
            session_id: self.session_id.clone(),
            actor_id: actor,
            role,
            lease,
            version: self.version,
            epoch: self.epoch,
            capabilities: Capabilities {
                kinds: vec![
                    pdfcraft_collab_proto::OpKind::Command,
                    pdfcraft_collab_proto::OpKind::Primitive,
                    pdfcraft_collab_proto::OpKind::Structural,
                ],
                classes: vec![ConflictClass::Merge, ConflictClass::Structural],
            },
            limits: self.config.limits.clone(),
        })
    }

    /// Take or renew the writing lease for `actor`. Succeeds when the lease is
    /// free, expired or already held by `actor`; a live lease held by someone
    /// else is left untouched. Only structural writes need the lease, so a
    /// second editor may join and comment, but cannot change pages until the
    /// holder leaves or its TTL runs out (ADR-0008).
    fn take_lease(&mut self, actor: &ActorId, now_ms: u64) -> bool {
        let available = self.lease.as_ref().is_none_or(|l| l.holder == *actor || l.expires_ms <= now_ms);
        if !available {
            return false;
        }
        let expires_ms = now_ms.saturating_add(self.config.lease_ttl_secs.saturating_mul(1_000));
        self.lease = Some(LeaseState { holder: actor.clone(), expires_ms });
        true
    }

    /// A heartbeat from the lease holder extends it. Everyone else is ignored:
    /// a ping is not a write and must not move the lease.
    pub fn heartbeat(&mut self, actor: &ActorId, now_ms: u64) -> bool {
        let Some(lease) = self.lease.as_mut() else { return false };
        if lease.holder != *actor {
            return false;
        }
        lease.expires_ms = now_ms.saturating_add(self.config.lease_ttl_secs.saturating_mul(1_000));
        true
    }

    /// Apply one operation. The checks run in this order: membership, role,
    /// de-duplication, document, lease, then `baseVersion`/`epoch`. A retried
    /// `opId` returns the stored result without touching the document.
    pub fn apply(&mut self, actor: &ActorId, op: &Op, now_ms: u64) -> Result<Applied, Rejected> {
        let Some(participant) = self.participants.get(actor) else {
            return Err(self.reject(&op.op_id, ErrorCode::Forbidden, false, "not in this session"));
        };
        if participant.role == Role::Viewer {
            return Err(self.reject(&op.op_id, ErrorCode::Forbidden, false, "viewers cannot change the document"));
        }
        if let Some(applied) = self.applied.get(op.op_id.as_str()) {
            return Ok(applied.clone());
        }
        if op.doc != self.doc {
            return Err(self.reject(&op.op_id, ErrorCode::Invalid, false, "the operation belongs to another document"));
        }
        if op.class == ConflictClass::Structural && !participant.role.can_structural() {
            return Err(self.reject(&op.op_id, ErrorCode::Forbidden, false, "this role cannot change pages"));
        }
        if op.class == ConflictClass::Merge && !participant.role.can_annotate() {
            return Err(self.reject(&op.op_id, ErrorCode::Forbidden, false, "this role cannot comment"));
        }
        if op.class == ConflictClass::Structural && !self.take_lease(actor, now_ms) {
            return Err(self.reject(&op.op_id, ErrorCode::Lease, true, "another editor holds the document lease"));
        }
        if op.class == ConflictClass::Structural && op.base_version != self.version {
            return Err(self.reject(&op.op_id, ErrorCode::Stale, true, "the document changed; resync and retry"));
        }
        if op.class == ConflictClass::Merge && op.epoch != self.epoch {
            return Err(self.reject(&op.op_id, ErrorCode::Stale, true, "the pages changed; resync and retry"));
        }
        let outcome = match self.adapter.apply(op) {
            Ok(outcome) => outcome,
            Err(e) => return Err(self.reject(&op.op_id, e.code, e.retryable, e.message)),
        };
        if op.class == ConflictClass::Structural && outcome.epoch <= self.epoch {
            return Err(self.reject(&op.op_id, ErrorCode::Invalid, false, "a structural operation must advance the epoch"));
        }
        if outcome.epoch < self.epoch {
            return Err(self.reject(&op.op_id, ErrorCode::Invalid, false, "the epoch cannot move backwards"));
        }
        self.version = self.version.saturating_add(1);
        self.epoch = outcome.epoch;
        let applied = Applied { op_id: op.op_id.clone(), actor_id: actor.clone(), version: self.version, epoch: self.epoch, result: outcome.result };
        if let Ok(op_value) = serde_json::to_value(op) {
            self.journal.push_back(JournalEntry { version: self.version, op: op_value });
            while self.journal.len() > self.config.journal_len {
                self.journal.pop_front();
            }
        }
        self.remember(applied.clone());
        Ok(applied)
    }

    fn remember(&mut self, applied: Applied) {
        let id = applied.op_id.as_str().to_owned();
        self.applied_order.push_back(id.clone());
        self.applied.insert(id, applied);
        while self.applied_order.len() > self.config.op_window {
            if let Some(old) = self.applied_order.pop_front() {
                self.applied.remove(&old);
            }
        }
    }

    /// What a reconnecting client needs after `last_version`. If the journal no
    /// longer starts at `last_version + 1`, the answer is a base snapshot.
    pub fn resync(&self, actor: &ActorId, req: &Resync) -> Result<Snapshot, ServerError> {
        if !self.participants.contains_key(actor) {
            return Err(server_error(ErrorCode::Forbidden, "not in this session", false));
        }
        let first = self.journal.front().map(|e| e.version).unwrap_or(self.version.saturating_add(1));
        let base = self.adapter.base();
        if req.last_version.saturating_add(1) < first && base.is_some() {
            return Ok(Snapshot { mode: SnapshotMode::Base, version: self.version, epoch: self.epoch, base, tail: Vec::new() });
        }
        let tail = self.journal.iter().filter(|e| e.version > req.last_version).map(|e| e.op.clone()).collect();
        Ok(Snapshot { mode: SnapshotMode::BaseAndTail, version: self.version, epoch: self.epoch, base: None, tail })
    }

    /// Record a presence update. Returns `false` when the rate limit dropped it.
    /// Presence never changes `version` and is not journaled.
    pub fn presence(&mut self, actor: &ActorId, presence: &Presence, now_ms: u64) -> Result<bool, ServerError> {
        let Some(participant) = self.participants.get_mut(actor) else {
            return Err(server_error(ErrorCode::Forbidden, "not in this session", false));
        };
        if presence.actor_id != *actor {
            return Err(server_error(ErrorCode::Invalid, "presence names another actor", false));
        }
        let min_gap = 1_000 / u64::from(self.config.limits.presence_hz.max(1));
        if let Some(last) = participant.last_presence_ms
            && now_ms.saturating_sub(last) < min_gap
        {
            return Ok(false);
        }
        if let Err(e) = presence.validate() {
            return Err(server_error(ErrorCode::Limit, e.to_string(), false));
        }
        participant.last_presence_ms = Some(now_ms);
        participant.color = Some(presence.color.clone());
        Ok(true)
    }

    /// Commit the authoritative state to the cabinet. The version is not bumped:
    /// the file is a new cabinet version, not a document edit.
    pub fn commit(&mut self, actor: &ActorId, expected_version: u64) -> Result<CommitResult, ServerError> {
        let Some(participant) = self.participants.get(actor) else {
            return Err(server_error(ErrorCode::Forbidden, "not in this session", false));
        };
        if !participant.role.can_structural() {
            return Err(server_error(ErrorCode::Forbidden, "this role cannot commit the file", false));
        }
        if expected_version != self.version {
            return Err(server_error(ErrorCode::Stale, "the document changed; resync and commit again", true));
        }
        let outcome = self.adapter.commit().map_err(|e| server_error(e.code, e.message, e.retryable))?;
        Ok(CommitResult { version: self.version, sha256: outcome.sha256, bytes: outcome.bytes, conflict: outcome.conflict })
    }

    /// A participant left: release the lease if they held it.
    pub fn disconnect(&mut self, actor: &ActorId) {
        self.participants.remove(actor);
        if self.lease.as_ref().is_some_and(|l| l.holder == *actor) {
            self.lease = None;
        }
    }

    /// The actor holding the editing lease at `now_ms`, if any.
    pub fn lease_holder(&self, now_ms: u64) -> Option<&ActorId> {
        self.lease.as_ref().filter(|l| l.expires_ms > now_ms).map(|l| &l.holder)
    }

    fn reject(&self, op_id: &OpId, code: ErrorCode, retryable: bool, message: impl Into<String>) -> Rejected {
        Rejected {
            op_id: op_id.clone(),
            code,
            retryable,
            hint: Some(VersionHint { version: self.version, epoch: self.epoch }),
            message: message.into(),
        }
    }
}

fn server_error(code: ErrorCode, message: impl Into<String>, retryable: bool) -> ServerError {
    ServerError { code, message: message.into(), retryable }
}

#[cfg(test)]
mod tests {
    use super::{Adapter, AdapterError, ApplyOutcome, CommitOutcome, Room, RoomConfig};
    use pdfcraft_collab_proto::{
        ActorId, BlobRef, ClientInfo, ConflictClass, DocKey, ErrorCode, Hello, Op, OpId, OpKind, PageRef, Presence, Role, Selection, SessionId,
        Target, Viewport,
    };
    use serde_json::json;

    struct Fake {
        epoch: u64,
        applied: usize,
    }

    impl Adapter for Fake {
        fn apply(&mut self, op: &Op) -> Result<ApplyOutcome, AdapterError> {
            if op.payload.get("fail").is_some() {
                return Err(AdapterError::new(ErrorCode::Invalid, "the adapter refused it", false));
            }
            self.applied += 1;
            if op.class == ConflictClass::Structural {
                self.epoch += 1;
            }
            Ok(ApplyOutcome { result: json!({"n": self.applied}), epoch: self.epoch })
        }

        fn commit(&mut self) -> Result<CommitOutcome, AdapterError> {
            Ok(CommitOutcome { sha256: "abc".into(), bytes: 10, conflict: false })
        }

        fn base(&self) -> Option<BlobRef> {
            Some(BlobRef { sha256: "base".into(), len: 1 })
        }
    }

    fn room() -> Result<Room<Fake>, Box<dyn std::error::Error>> {
        Ok(Room::new(
            SessionId::new("session-1")?,
            DocKey::new("file-1")?,
            Fake { epoch: 0, applied: 0 },
            RoomConfig { journal_len: 2, ..RoomConfig::default() },
        ))
    }

    fn actor(name: &str) -> Result<ActorId, Box<dyn std::error::Error>> {
        Ok(ActorId::new(name)?)
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

    fn op(id: &str, kind: OpKind, class: ConflictClass, base: u64, epoch: u64) -> Result<Op, Box<dyn std::error::Error>> {
        Ok(Op {
            op_id: OpId::new(id)?,
            doc: DocKey::new("file-1")?,
            base_version: base,
            epoch,
            kind,
            class,
            target: Target::Annotation { page: PageRef { index: 0, epoch }, nm: "ann-1".into() },
            payload: json!({"text": "hello"}),
            preview: None,
        })
    }

    #[test]
    fn the_lease_is_granted_to_one_editor_and_expires() -> Result<(), Box<dyn std::error::Error>> {
        let mut room = room()?;
        let a = actor("a")?;
        let b = actor("b")?;
        let hello = hello()?;
        let first = room.hello(a.clone(), Role::Editor, &hello, 1_000)?;
        assert_eq!(first.lease.ttl_secs, RoomConfig::default().lease_ttl_secs);
        // A second editor joins read-only for pages: the lease is what a writer
        // needs, not a condition for entering the room.
        let second = room.hello(b.clone(), Role::Editor, &hello, 1_000)?;
        assert_eq!(second.lease.ttl_secs, 0);
        assert_eq!(room.participants(), 2);
        Ok(())
    }

    #[test]
    fn structural_writes_need_the_lease_and_a_heartbeat_keeps_it() -> Result<(), Box<dyn std::error::Error>> {
        let mut room = room()?;
        let a = actor("a")?;
        let b = actor("b")?;
        let hello = hello()?;
        let ttl = RoomConfig::default().lease_ttl_secs * 1_000;
        room.hello(a.clone(), Role::Editor, &hello, 1_000)?;
        room.hello(b.clone(), Role::Editor, &hello, 1_000)?;

        let first = op("s1", OpKind::Structural, ConflictClass::Structural, 0, 0)?;
        assert!(matches!(room.apply(&b, &first, 1_000), Err(e) if e.code == ErrorCode::Lease && e.retryable));

        // The holder renews without writing; the lease survives past its old expiry.
        assert!(room.heartbeat(&a, 1_000 + ttl / 2));
        assert!(matches!(room.apply(&b, &first, 1_000 + ttl / 2 + 1), Err(e) if e.code == ErrorCode::Lease));

        // Without another heartbeat the lease expires and the next writer takes it.
        let after = 1_000 + ttl + ttl / 2 + 2;
        assert!(room.apply(&b, &first, after).is_ok());
        assert_eq!(room.epoch(), 1);
        assert!(matches!(room.lease_holder(after), Some(holder) if holder == &b));
        Ok(())
    }

    #[test]
    fn roles_and_classes_are_checked_by_the_server() -> Result<(), Box<dyn std::error::Error>> {
        let mut room = room()?;
        let hello = hello()?;
        let viewer = actor("viewer")?;
        let commenter = actor("commenter")?;
        let editor = actor("editor")?;
        room.hello(viewer.clone(), Role::Viewer, &hello, 0)?;
        let merge = op("v1", OpKind::Primitive, ConflictClass::Merge, 0, 0)?;
        assert!(matches!(room.apply(&viewer, &merge, 0), Err(e) if e.code == ErrorCode::Forbidden));

        room.hello(commenter.clone(), Role::Commenter, &hello, 0)?;
        assert!(room.apply(&commenter, &merge, 0).is_ok());
        let structural = op("c1", OpKind::Structural, ConflictClass::Structural, 1, 0)?;
        assert!(matches!(room.apply(&commenter, &structural, 0), Err(e) if e.code == ErrorCode::Forbidden));

        room.hello(editor.clone(), Role::Editor, &hello, 0)?;
        assert!(room.apply(&editor, &structural, 0).is_ok());
        Ok(())
    }

    #[test]
    fn structural_requires_head_and_merge_requires_epoch() -> Result<(), Box<dyn std::error::Error>> {
        let mut room = room()?;
        let hello = hello()?;
        let editor = actor("editor")?;
        room.hello(editor.clone(), Role::Editor, &hello, 0)?;
        let first = op("s1", OpKind::Structural, ConflictClass::Structural, 0, 0)?;
        room.apply(&editor, &first, 0)?;
        assert_eq!(room.version(), 1);
        assert_eq!(room.epoch(), 1);
        let stale = op("s2", OpKind::Structural, ConflictClass::Structural, 0, 1)?;
        assert!(matches!(room.apply(&editor, &stale, 0), Err(e) if e.code == ErrorCode::Stale && e.retryable));
        let old_epoch = op("m1", OpKind::Primitive, ConflictClass::Merge, 0, 0)?;
        assert!(matches!(room.apply(&editor, &old_epoch, 0), Err(e) if e.code == ErrorCode::Stale && e.retryable));
        Ok(())
    }

    #[test]
    fn an_op_id_is_applied_once() -> Result<(), Box<dyn std::error::Error>> {
        let mut room = room()?;
        let hello = hello()?;
        let editor = actor("editor")?;
        room.hello(editor.clone(), Role::Editor, &hello, 0)?;
        let first_op = op("same", OpKind::Primitive, ConflictClass::Merge, 0, 0)?;
        let first = room.apply(&editor, &first_op, 0)?;
        let retry = room.apply(&editor, &first_op, 0)?;
        assert_eq!(first, retry);
        assert_eq!(room.version(), 1);
        Ok(())
    }

    #[test]
    fn resync_returns_a_tail_or_a_base() -> Result<(), Box<dyn std::error::Error>> {
        let mut room = room()?;
        let hello = hello()?;
        let editor = actor("editor")?;
        room.hello(editor.clone(), Role::Editor, &hello, 0)?;
        for i in 0..3 {
            room.apply(&editor, &op(&format!("o{i}"), OpKind::Primitive, ConflictClass::Merge, 0, 0)?, 0)?;
        }
        let tail = room.resync(&editor, &pdfcraft_collab_proto::Resync { last_version: 1 })?;
        assert_eq!(tail.mode, pdfcraft_collab_proto::SnapshotMode::BaseAndTail);
        assert_eq!(tail.tail.len(), 2);
        let base = room.resync(&editor, &pdfcraft_collab_proto::Resync { last_version: 0 })?;
        assert_eq!(base.mode, pdfcraft_collab_proto::SnapshotMode::Base);
        assert!(base.base.is_some());
        Ok(())
    }

    #[test]
    fn presence_is_rate_limited_and_never_bumps_the_version() -> Result<(), Box<dyn std::error::Error>> {
        let mut room = room()?;
        let hello = hello()?;
        let viewer = actor("viewer")?;
        room.hello(viewer.clone(), Role::Viewer, &hello, 0)?;
        let presence = Presence {
            actor_id: viewer.clone(),
            role: Role::Viewer,
            page: PageRef { index: 0, epoch: 0 },
            viewport: Viewport { x: 0.0, y: 0.0, zoom: 1.0 },
            cursor: None,
            selection: Some(Selection::Annotation { nm: "ann-1".into() }),
            color: "#112233".into(),
        };
        assert!(room.presence(&viewer, &presence, 1_000)?);
        assert!(!room.presence(&viewer, &presence, 1_050)?);
        assert!(room.presence(&viewer, &presence, 1_200)?);
        assert_eq!(room.version(), 0);
        Ok(())
    }

    #[test]
    fn commit_checks_role_and_version() -> Result<(), Box<dyn std::error::Error>> {
        let mut room = room()?;
        let hello = hello()?;
        let viewer = actor("viewer")?;
        let editor = actor("editor")?;
        room.hello(viewer.clone(), Role::Viewer, &hello, 0)?;
        assert_eq!(room.commit(&viewer, 0).map(|_| ()), Err(super::server_error(ErrorCode::Forbidden, "this role cannot commit the file", false)));
        room.hello(editor.clone(), Role::Editor, &hello, 0)?;
        assert!(room.commit(&editor, 0).is_ok());
        assert!(matches!(room.commit(&editor, 7), Err(e) if e.code == ErrorCode::Stale && e.retryable));
        Ok(())
    }

    #[test]
    fn disconnect_releases_the_lease() -> Result<(), Box<dyn std::error::Error>> {
        let mut room = room()?;
        let hello = hello()?;
        let a = actor("a")?;
        let b = actor("b")?;
        room.hello(a.clone(), Role::Editor, &hello, 0)?;
        room.hello(b.clone(), Role::Editor, &hello, 0)?;
        let structural = op("s1", OpKind::Structural, ConflictClass::Structural, 0, 0)?;
        assert!(matches!(room.apply(&b, &structural, 0), Err(e) if e.code == ErrorCode::Lease));
        room.disconnect(&a);
        assert!(room.apply(&b, &structural, 0).is_ok());
        Ok(())
    }
}
