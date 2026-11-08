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
    /// The operation as applied: the server's author, generated names and any
    /// target normalisation are in it, and it is what the room broadcasts.
    pub op: Op,
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
    /// Apply `op` on behalf of `actor`. The author recorded in the document is
    /// the verified actor, never a client-supplied string (ADR-0008 §7).
    fn apply(&mut self, actor: &ActorId, op: &Op) -> Result<ApplyOutcome, AdapterError>;
    /// Persist an accepted operation before it is acknowledged. The default is
    /// a no-op: an adapter whose document lives only in memory (tests, a
    /// headless session) has nothing to replay. An adapter that returns an
    /// error here has already changed the document, so the room is poisoned:
    /// see [`Room::apply`].
    fn record(&mut self, version: u64, actor: &ActorId, op: &Op) -> Result<(), AdapterError> {
        let _ = (version, actor, op);
        Ok(())
    }
    /// Write the document as it stands at `version`/`epoch` and make it the
    /// new recovery base. Called after the room has acknowledged the head.
    fn commit(&mut self, version: u64, epoch: u64) -> Result<CommitOutcome, AdapterError>;
    /// The current base bytes, when the journal no longer covers a resync.
    fn base(&self) -> Option<BlobRef>;
    /// What the cabinet holds for this file, as last agreed (RFC §8). `None`
    /// means "unknown": external-change checks are skipped rather than guessed,
    /// which is what a room seeded before the marker existed needs.
    fn published(&self) -> Option<Published> {
        None
    }
    /// Record what the cabinet holds after it accepted a commit.
    fn set_published(&mut self, published: Option<Published>) -> Result<(), AdapterError> {
        let _ = published;
        Ok(())
    }
}

/// The version and etag of the cabinet's copy of the file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Published {
    pub version: u64,
    pub etag: String,
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
    /// Per actor: the second the window started and operations in it.
    op_windows: HashMap<ActorId, (u64, u32)>,
    lease: Option<LeaseState>,
    journal: VecDeque<JournalEntry>,
    /// The version last written to the cabinet's file.
    committed_version: u64,
    applied_order: VecDeque<String>,
    applied: HashMap<String, Applied>,
    /// Set when the adapter could not persist an accepted operation. The
    /// document in memory is ahead of the durable state, so no further write is
    /// accepted; resync and commit still work.
    poisoned: Option<String>,
    /// Set when a commit was refused because the cabinet's copy changed
    /// outside the session. Writes continue (the journal keeps them), commits
    /// stay refused until an operator re-seeds the room, and `/health` says so.
    conflict: Option<String>,
}

impl<A: Adapter> Room<A> {
    pub fn new(session_id: SessionId, doc: DocKey, adapter: A, config: RoomConfig) -> Self {
        Self::resume(session_id, doc, adapter, 0, 0, config)
    }

    /// A room opened from a committed base: the document already contains
    /// `version` operations and its structural epoch is `epoch`. The journal
    /// entries after `version` are replayed with [`Self::replay`].
    pub fn resume(session_id: SessionId, doc: DocKey, adapter: A, version: u64, epoch: u64, config: RoomConfig) -> Self {
        Self {
            session_id,
            doc,
            adapter,
            config,
            version,
            epoch,
            participants: HashMap::new(),
            op_windows: HashMap::new(),
            lease: None,
            journal: VecDeque::new(),
            committed_version: version,
            applied_order: VecDeque::new(),
            applied: HashMap::new(),
            poisoned: None,
            conflict: None,
        }
    }

    /// The authoritative adapter, for server-side inspection (tests, commit tools).
    pub fn adapter(&self) -> &A {
        &self.adapter
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

    /// Why the room refuses writes, if the durable journal failed.
    pub fn poisoned(&self) -> Option<&str> {
        self.poisoned.as_deref()
    }

    /// Why the room refuses commits, if the cabinet's copy changed under it.
    pub fn conflict(&self) -> Option<&str> {
        self.conflict.as_deref()
    }

    /// Join. The role comes from the verified token, never from the body.
    pub fn hello(&mut self, actor: ActorId, role: Role, hello: &Hello, now_ms: u64) -> Result<Welcome, ServerError> {
        if hello.app != "pdfcraft" {
            return Err(server_error(ErrorCode::Unsupported, "this server only accepts pdfcraft sessions", false));
        }
        if hello.doc != self.doc {
            return Err(server_error(ErrorCode::Invalid, "the session belongs to another document", false));
        }
        // Participants are people, not connections: the same actor reconnecting
        // or opening a second window does not take a second seat.
        if !self.participants.contains_key(&actor)
            && self.participants.len() >= usize::try_from(self.config.limits.max_participants).unwrap_or(usize::MAX)
        {
            return Err(server_error(ErrorCode::Limit, "the session has no free participant slot", false));
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
    /// A fixed one-second window per actor. A retried `opId` returns before
    /// this check, so de-duplication does not spend quota.
    fn allow_op(&mut self, actor: &ActorId, now_ms: u64) -> bool {
        let limit = self.config.limits.max_ops_per_sec.max(1);
        let second = now_ms / 1_000;
        let window = self.op_windows.entry(actor.clone()).or_insert((second, 0));
        if window.0 != second {
            *window = (second, 0);
        }
        if window.1 >= limit {
            return false;
        }
        window.1 = window.1.saturating_add(1);
        true
    }

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
        let Some(role) = self.participants.get(actor).map(|p| p.role) else {
            return Err(self.reject(&op.op_id, ErrorCode::Forbidden, false, "not in this session"));
        };
        if role == Role::Viewer {
            return Err(self.reject(&op.op_id, ErrorCode::Forbidden, false, "viewers cannot change the document"));
        }
        if let Some(reason) = &self.poisoned {
            return Err(self.reject(&op.op_id, ErrorCode::Internal, false, format!("the room is read-only: {reason}")));
        }
        if let Some(applied) = self.applied.get(op.op_id.as_str()) {
            return Ok(applied.clone());
        }
        if op.doc != self.doc {
            return Err(self.reject(&op.op_id, ErrorCode::Invalid, false, "the operation belongs to another document"));
        }
        if !self.allow_op(actor, now_ms) {
            return Err(self.reject(&op.op_id, ErrorCode::Limit, true, "too many operations; slow down and retry"));
        }
        if op.class == ConflictClass::Structural && !role.can_structural() {
            return Err(self.reject(&op.op_id, ErrorCode::Forbidden, false, "this role cannot change pages"));
        }
        if op.class == ConflictClass::Merge && !role.can_annotate() {
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
        let outcome = match self.adapter.apply(actor, op) {
            Ok(outcome) => outcome,
            Err(e) => return Err(self.reject(&op.op_id, e.code, e.retryable, e.message)),
        };
        // A structural operation must advance the epoch. An adapter that breaks
        // that contract has already changed the document, so the room cannot
        // keep a coherent head: it is poisoned, not merely rejecting.
        if (op.class == ConflictClass::Structural && outcome.epoch <= self.epoch) || outcome.epoch < self.epoch {
            let message = format!("the adapter applied version {} but returned epoch {}", self.version.saturating_add(1), outcome.epoch);
            self.poisoned = Some(message.clone());
            return Err(self.reject(&op.op_id, ErrorCode::Internal, false, message));
        }
        self.version = self.version.saturating_add(1);
        self.epoch = outcome.epoch;
        let applied = Applied {
            op_id: op.op_id.clone(),
            actor_id: actor.clone(),
            version: self.version,
            epoch: self.epoch,
            op: outcome.op,
            result: outcome.result,
        };
        if let Ok(op_value) = serde_json::to_value(&applied.op) {
            self.journal.push_back(JournalEntry { version: self.version, op: op_value });
            while self.journal.len() > self.config.journal_len {
                self.journal.pop_front();
            }
        }
        self.remember(applied.clone());
        // The operation is acknowledged only once it is durable: a crash may
        // lose the document's last edits, but never one the client was told
        // about. The adapter has already changed the document, so a failure
        // here cannot be retried on top of it.
        if let Err(e) = self.adapter.record(self.version, actor, &applied.op) {
            self.poisoned = Some(e.message.clone());
            let hint = Some(VersionHint { version: self.version, epoch: self.epoch });
            return Err(Rejected { op_id: applied.op_id, code: ErrorCode::Internal, retryable: false, hint, message: e.message });
        }
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

    /// Rebuild the head from a durable journal after a restart.
    ///
    /// The entries were accepted before the crash, so membership, role, lease
    /// and de-duplication checks are skipped; the operation is applied as it
    /// was accepted, and `version` must be the stored one (a gap means the
    /// journal and the base disagree about the document). The adapter's
    /// document is at the base, so its epoch is advanced by the replayed
    /// structural operations, not by the caller.
    pub fn replay(&mut self, actor: ActorId, op: Op, version: u64) -> Result<(), ServerError> {
        if version != self.version.saturating_add(1) {
            return Err(server_error(ErrorCode::Invalid, format!("the journal jumps from version {} to {version}", self.version), false));
        }
        if op.doc != self.doc {
            return Err(server_error(ErrorCode::Invalid, "a journal entry belongs to another document", false));
        }
        let class = op.class;
        let outcome = self
            .adapter
            .apply(&actor, &op)
            .map_err(|e| server_error(ErrorCode::Invalid, format!("replaying version {version}: {}", e.message), false))?;
        if (class == ConflictClass::Structural && outcome.epoch <= self.epoch) || outcome.epoch < self.epoch {
            return Err(server_error(ErrorCode::Invalid, format!("replaying version {version} breaks the epoch"), false));
        }
        self.version = version;
        self.epoch = outcome.epoch;
        let applied = Applied { op_id: op.op_id.clone(), actor_id: actor, version, epoch: self.epoch, op: outcome.op, result: outcome.result };
        if let Ok(op_value) = serde_json::to_value(&applied.op) {
            self.journal.push_back(JournalEntry { version, op: op_value });
            while self.journal.len() > self.config.journal_len {
                self.journal.pop_front();
            }
        }
        self.remember(applied);
        Ok(())
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
        self.write_commit()
    }

    /// The session has changes the cabinet file does not have yet.
    pub fn is_dirty(&self) -> bool {
        self.version > self.committed_version
    }

    /// Declare which version the cabinet already holds. A room resumed from a
    /// base that was written but never published is dirty again: the last
    /// client leaving has to publish it.
    pub fn mark_published(&mut self, version: u64) {
        // A marker ahead of the base cannot be true; clamping keeps a damaged
        // marker from claiming the cabinet has work the room never wrote.
        self.committed_version = version.min(self.version);
    }

    /// The server commits on its own (last participant leaving, RFC §8). No
    /// actor or expected version: the room's head is what is written.
    pub fn commit_now(&mut self) -> Result<CommitResult, ServerError> {
        self.write_commit()
    }

    /// One code path for both commits. An adapter that refuses because the
    /// cabinet's copy changed leaves the room dirty and flags it, so `/health`
    /// can report a session nobody can save.
    fn write_commit(&mut self) -> Result<CommitResult, ServerError> {
        let outcome = match self.adapter.commit(self.version, self.epoch) {
            Ok(outcome) => outcome,
            Err(e) => {
                if e.code == ErrorCode::Conflict {
                    self.conflict = Some(e.message.clone());
                }
                return Err(server_error(e.code, e.message, e.retryable));
            }
        };
        self.conflict = None;
        self.committed_version = self.version;
        Ok(CommitResult { version: self.version, sha256: outcome.sha256, bytes: outcome.bytes, conflict: outcome.conflict })
    }

    /// A participant left: release the lease if they held it.
    pub fn disconnect(&mut self, actor: &ActorId) {
        self.participants.remove(actor);
        self.op_windows.remove(actor);
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
        ActorId, BlobRef, ClientInfo, ConflictClass, DocKey, ErrorCode, Hello, Limits, Op, OpId, OpKind, PageRef, Presence, Role, Selection,
        SessionId, Target, Viewport,
    };
    use serde_json::json;

    struct Fake {
        epoch: u64,
        applied: usize,
        /// Accepted operations as the room persisted them, when recording works.
        recorded: Vec<(u64, String, String)>,
        record_error: Option<&'static str>,
    }

    impl Fake {
        fn new() -> Self {
            Self { epoch: 0, applied: 0, recorded: Vec::new(), record_error: None }
        }
    }

    impl Adapter for Fake {
        fn apply(&mut self, _actor: &ActorId, op: &Op) -> Result<ApplyOutcome, AdapterError> {
            if op.payload.get("fail").is_some() {
                return Err(AdapterError::new(ErrorCode::Invalid, "the adapter refused it", false));
            }
            self.applied += 1;
            if op.class == ConflictClass::Structural {
                self.epoch += 1;
            }
            Ok(ApplyOutcome { result: json!({"n": self.applied}), op: op.clone(), epoch: self.epoch })
        }

        fn record(&mut self, version: u64, actor: &ActorId, op: &Op) -> Result<(), AdapterError> {
            if let Some(message) = self.record_error {
                return Err(AdapterError::new(ErrorCode::Internal, message, false));
            }
            self.recorded.push((version, actor.as_str().to_owned(), op.op_id.as_str().to_owned()));
            Ok(())
        }

        fn commit(&mut self, _version: u64, _epoch: u64) -> Result<CommitOutcome, AdapterError> {
            Ok(CommitOutcome { sha256: "abc".into(), bytes: 10, conflict: false })
        }

        fn base(&self) -> Option<BlobRef> {
            Some(BlobRef { sha256: "base".into(), len: 1 })
        }
    }

    fn room() -> Result<Room<Fake>, Box<dyn std::error::Error>> {
        Ok(Room::new(SessionId::new("session-1")?, DocKey::new("file-1")?, Fake::new(), RoomConfig { journal_len: 2, ..RoomConfig::default() }))
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
    fn participant_and_operation_limits_are_enforced() -> Result<(), Box<dyn std::error::Error>> {
        let mut room = Room::new(
            SessionId::new("session-1")?,
            DocKey::new("file-1")?,
            Fake::new(),
            RoomConfig { limits: Limits { max_participants: 2, max_ops_per_sec: 2, ..Limits::default() }, ..RoomConfig::default() },
        );
        let hello = hello()?;
        let a = actor("a")?;
        let b = actor("b")?;
        let c = actor("c")?;
        room.hello(a.clone(), Role::Editor, &hello, 0)?;
        room.hello(b.clone(), Role::Commenter, &hello, 0)?;
        assert_eq!(
            room.hello(c, Role::Viewer, &hello, 0).map(|_| ()),
            Err(super::server_error(ErrorCode::Limit, "the session has no free participant slot", false))
        );
        // Reconnecting the same person does not need a second seat.
        assert!(room.hello(a.clone(), Role::Editor, &hello, 0).is_ok());

        let first = op("o1", OpKind::Primitive, ConflictClass::Merge, 0, 0)?;
        let second = op("o2", OpKind::Primitive, ConflictClass::Merge, 0, 0)?;
        let third = op("o3", OpKind::Primitive, ConflictClass::Merge, 0, 0)?;
        room.apply(&a, &first, 0)?;
        room.apply(&a, &second, 0)?;
        assert!(matches!(room.apply(&a, &third, 500), Err(e) if e.code == ErrorCode::Limit && e.retryable));
        // A retry of an accepted opId answers from the de-duplication window
        // and does not spend the new second's quota.
        assert!(room.apply(&a, &first, 500).is_ok());
        assert!(room.apply(&a, &third, 1_000).is_ok());
        Ok(())
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

    #[test]
    fn an_accepted_operation_is_recorded_before_it_is_acknowledged() -> Result<(), Box<dyn std::error::Error>> {
        let mut room = room()?;
        let hello = hello()?;
        let editor = actor("editor")?;
        room.hello(editor.clone(), Role::Editor, &hello, 0)?;
        room.apply(&editor, &op("r1", OpKind::Primitive, ConflictClass::Merge, 0, 0)?, 0)?;
        room.apply(&editor, &op("r2", OpKind::Structural, ConflictClass::Structural, 1, 0)?, 0)?;
        assert_eq!(
            room.adapter().recorded,
            vec![(1, "editor".to_owned(), "r1".to_owned()), (2, "editor".to_owned(), "r2".to_owned())],
            "the room hands the accepted operation to the adapter with its version and author"
        );
        Ok(())
    }

    #[test]
    fn a_failed_journal_write_poisons_the_room_but_commit_still_works() -> Result<(), Box<dyn std::error::Error>> {
        let mut adapter = Fake::new();
        adapter.record_error = Some("the disk is full");
        let mut room = Room::new(SessionId::new("session-1")?, DocKey::new("file-1")?, adapter, RoomConfig::default());
        let hello = hello()?;
        let editor = actor("editor")?;
        room.hello(editor.clone(), Role::Editor, &hello, 0)?;
        let first = op("p1", OpKind::Primitive, ConflictClass::Merge, 0, 0)?;
        assert!(matches!(room.apply(&editor, &first, 0), Err(e) if e.code == ErrorCode::Internal && !e.retryable));
        assert_eq!(room.poisoned(), Some("the disk is full"));
        // The document changed before the journal failed; a second write must not
        // build on it.
        let second = op("p2", OpKind::Primitive, ConflictClass::Merge, 1, 0)?;
        assert!(matches!(room.apply(&editor, &second, 0), Err(e) if e.code == ErrorCode::Internal));
        assert_eq!(room.version(), 1);
        // Committing writes the base at the current head, which is how an
        // operator rescues what the journal lost.
        assert!(room.commit(&editor, 1).is_ok());
        Ok(())
    }

    #[test]
    fn a_commit_refused_by_the_adapter_flags_the_room_and_stays_dirty() -> Result<(), Box<dyn std::error::Error>> {
        struct Conflicting(Fake);
        impl Adapter for Conflicting {
            fn apply(&mut self, actor: &ActorId, op: &Op) -> Result<ApplyOutcome, AdapterError> {
                self.0.apply(actor, op)
            }
            fn commit(&mut self, _version: u64, _epoch: u64) -> Result<CommitOutcome, AdapterError> {
                Err(AdapterError::new(ErrorCode::Conflict, "the file changed outside the session", false))
            }
            fn base(&self) -> Option<BlobRef> {
                self.0.base()
            }
        }

        let mut room = Room::new(SessionId::new("session-1")?, DocKey::new("file-1")?, Conflicting(Fake::new()), RoomConfig::default());
        let hello = hello()?;
        let editor = actor("editor")?;
        room.hello(editor.clone(), Role::Editor, &hello, 0)?;
        room.apply(&editor, &op("p1", OpKind::Primitive, ConflictClass::Merge, 0, 0)?, 0)?;
        assert_eq!(room.conflict(), None, "no conflict before a commit is refused");
        assert!(matches!(room.commit(&editor, 1), Err(e) if e.code == ErrorCode::Conflict && !e.retryable));
        assert_eq!(room.conflict(), Some("the file changed outside the session"));
        assert!(room.is_dirty(), "a refused commit did not publish the head");
        assert_eq!(room.version(), 1, "the edits are still accepted and journalled");
        // The server-initiated commit is refused the same way and says so.
        assert!(matches!(room.commit_now(), Err(e) if e.code == ErrorCode::Conflict));
        Ok(())
    }

    #[test]
    fn an_adapter_that_breaks_the_epoch_contract_poisons_the_room() -> Result<(), Box<dyn std::error::Error>> {
        struct Frozen(Fake);
        impl Adapter for Frozen {
            fn apply(&mut self, actor: &ActorId, op: &Op) -> Result<ApplyOutcome, AdapterError> {
                let mut outcome = self.0.apply(actor, op)?;
                // The document changed, but a structural operation did not move
                // the epoch: the room cannot describe the resulting head.
                outcome.epoch = 0;
                Ok(outcome)
            }
            fn commit(&mut self, version: u64, epoch: u64) -> Result<CommitOutcome, AdapterError> {
                self.0.commit(version, epoch)
            }
            fn base(&self) -> Option<BlobRef> {
                self.0.base()
            }
        }

        let mut room = Room::new(SessionId::new("session-1")?, DocKey::new("file-1")?, Frozen(Fake::new()), RoomConfig::default());
        let hello = hello()?;
        let editor = actor("editor")?;
        room.hello(editor.clone(), Role::Editor, &hello, 0)?;
        let structural = op("s1", OpKind::Structural, ConflictClass::Structural, 0, 0)?;
        assert!(matches!(room.apply(&editor, &structural, 0), Err(e) if e.code == ErrorCode::Internal));
        assert!(room.poisoned().is_some(), "the head is not coherent, so no later operation may be accepted");
        assert_eq!(room.version(), 0, "a broken contract must not advance the version");
        Ok(())
    }

    #[test]
    fn replay_rebuilds_the_head_epoch_and_the_retry_window() -> Result<(), Box<dyn std::error::Error>> {
        let mut room = Room::resume(SessionId::new("session-1")?, DocKey::new("file-1")?, Fake::new(), 0, 0, RoomConfig::default());
        let editor = actor("editor")?;
        room.replay(editor.clone(), op("a1", OpKind::Primitive, ConflictClass::Merge, 0, 0)?, 1)?;
        room.replay(editor.clone(), op("s1", OpKind::Structural, ConflictClass::Structural, 1, 0)?, 2)?;
        assert_eq!(room.version(), 2);
        assert_eq!(room.epoch(), 1);
        assert_eq!(room.journal_len(), 2);
        assert!(room.is_dirty(), "the replayed entries are not in the base yet");

        // After a join, a retry of a replayed opId is answered from the window
        // instead of being applied a second time, even though its epoch is old.
        let hello = hello()?;
        room.hello(editor.clone(), Role::Editor, &hello, 0)?;
        let retried = op("a1", OpKind::Primitive, ConflictClass::Merge, 0, 0)?;
        let applied = room.apply(&editor, &retried, 0)?;
        assert_eq!(applied.version, 1);
        assert_eq!(room.version(), 2);
        Ok(())
    }

    #[test]
    fn a_journal_gap_or_a_foreign_document_is_refused() -> Result<(), Box<dyn std::error::Error>> {
        let mut room = Room::resume(SessionId::new("session-1")?, DocKey::new("file-1")?, Fake::new(), 4, 2, RoomConfig::default());
        let editor = actor("editor")?;
        assert!(room.replay(editor.clone(), op("a2", OpKind::Primitive, ConflictClass::Merge, 4, 2)?, 3).is_err(), "version 3 is behind the base");
        let mut foreign = op("a5", OpKind::Primitive, ConflictClass::Merge, 4, 2)?;
        foreign.doc = DocKey::new("file-2")?;
        assert!(room.replay(editor, foreign, 5).is_err());
        assert_eq!(room.version(), 4);
        Ok(())
    }
}
