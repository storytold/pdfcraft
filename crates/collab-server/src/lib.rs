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
    fn record(&mut self, version: u64, actor: &ActorId, op: &Op, recorded_ms: u64) -> Result<(), AdapterError> {
        let _ = (version, actor, op, recorded_ms);
        Ok(())
    }
    /// Write the document as it stands at `version`/`epoch` and make it the
    /// new recovery base. Called after the room has acknowledged the head.
    /// Work that talks to another service (the cabinet POST) is not done here:
    /// the adapter hands it out through [`Adapter::take_publication`], so the
    /// room is not held while it runs.
    fn commit(&mut self, version: u64, epoch: u64) -> Result<CommitOutcome, AdapterError>;
    /// The publication the last `commit` left for outside the room lock. The
    /// caller runs it after releasing the lock and reports the result with
    /// [`Room::finish`]. An adapter with nothing to publish returns `None`.
    fn take_publication(&mut self) -> Option<Box<dyn Publication>> {
        None
    }
    /// Record what the publication did, under the room lock again: advance the
    /// cabinet marker or leave it alone. A failure here is reported to the
    /// committer; the room stays dirty.
    fn finish_publish(&mut self, version: u64, outcome: PublishOutcome) -> Result<(), AdapterError> {
        let _ = (version, outcome);
        Ok(())
    }
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
    /// The publication of the committed base the cabinet has not taken: the one
    /// a commit wrote and whose POST never landed, or the one an operator's
    /// `resolve` made publishable again (limitation 6). It carries the version,
    /// the bytes and therefore the derived idempotency key the interrupted
    /// attempt would have sent, so a cabinet that already took the version
    /// answers it as a duplicate instead of creating a second one.
    ///
    /// The default is `None`: an adapter that only publishes inside `commit`
    /// (an in-memory session, a test) has nothing to republish. The room asks
    /// for this in [`Room::republish`]; the transport owns the attempt from
    /// there and reports it through [`Room::finish_retry`].
    fn pending_publication(&mut self) -> Result<Option<Box<dyn Publication>>, AdapterError> {
        Ok(None)
    }
}

/// The version and etag of the cabinet's copy of the file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Published {
    pub version: u64,
    pub etag: String,
}

/// What a publication did: the cabinet's copy after the call, or why it
/// failed. `Ok(None)` means the adapter had nothing to publish (dev mode, an
/// in-memory session): the commit is already the publication.
pub type PublishOutcome = Result<Option<Published>, AdapterError>;

/// Work a commit leaves for after the room lock is released: today the POST to
/// the cabinet. `Adapter::commit` has already written the base, so running this
/// never changes the document; it only talks to another service. It must not
/// run under the hub lock — a cabinet that does not answer would freeze every
/// other document in the process (RFC §16, limitation 9).
pub trait Publication: Send {
    /// Run the call once. It takes `&self` because a failed attempt may be run
    /// again by the retry queue: every attempt carries the same payload, the
    /// same version and the same idempotency key, so a repeated delivery is a
    /// duplicate to the cabinet, never a second version.
    fn run(&self) -> PublishOutcome;
}

/// A commit that has written its base and still has a publication to run.
/// [`Room::commit`] returns it; [`Room::finish`] records the outcome.
pub struct Commit {
    result: CommitResult,
    publication: Option<Box<dyn Publication>>,
}

impl Commit {
    pub fn result(&self) -> &CommitResult {
        &self.result
    }

    /// Run the part that must not hold the room lock. The publication stays
    /// with the commit, so a retryable failure can be queued and run again with
    /// exactly the same payload (`Room::finish` hands it back through
    /// [`Room::take_retry`]).
    pub fn publish(&self) -> PublishOutcome {
        match &self.publication {
            Some(publication) => publication.run(),
            None => Ok(None),
        }
    }
}

#[derive(Debug, Clone)]
pub struct RoomConfig {
    pub limits: Limits,
    /// How many applied operation ids are remembered for de-duplication.
    pub op_window: usize,
    /// How many journal entries the tail keeps before compaction must use a base.
    pub journal_len: usize,
    pub lease_ttl_secs: u64,
    /// Compact the journal with a server-initiated commit after this many
    /// accepted operations, even while clients are connected (RFC §4.6,
    /// limitation 10). `0` disables the operation threshold.
    pub forced_commit_ops: u64,
    /// The same in bytes of journal records appended since the last base.
    /// `0` disables the byte threshold.
    pub forced_commit_bytes: u64,
    /// Compact a room whose journal has not grown past either threshold but has
    /// seen no accepted operation for this long: a small journal that nobody is
    /// editing would otherwise wait for a join or the last leave. "Idle" is the
    /// time since the newest journaled operation, kept as room state (a
    /// reconnect does not reset it) and read back from the journal's records
    /// after a restart. `0` disables the time threshold.
    pub forced_commit_idle_secs: u64,
}

impl Default for RoomConfig {
    fn default() -> Self {
        Self {
            limits: Limits::default(),
            op_window: 10_000,
            journal_len: 10_000,
            lease_ttl_secs: 120,
            // A live session is compacted after a few thousand operations (roughly
            // 600 KiB of comment records) or 8 MiB of journal, whichever comes
            // first, instead of waiting for the last client to leave.
            forced_commit_ops: 2_000,
            forced_commit_bytes: 8 << 20,
            // Five minutes without an accepted operation: a comment written and
            // then left alone reaches the cabinet without waiting for a join or
            // for the last participant to leave.
            forced_commit_idle_secs: 300,
        }
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
    /// The version of the last base written locally. Operations after it are
    /// what the journal on disk holds, so this is what a forced commit counts.
    base_version: u64,
    /// Bytes of journal records appended since that base, as the adapter
    /// serialises them. An estimate of the on-disk journal, kept by the room so
    /// the threshold does not need a `stat` per operation.
    journal_bytes: u64,
    /// When the newest journaled operation was accepted, in wall-clock
    /// milliseconds. `None` when the journal is empty (the base has everything)
    /// or when a replayed record predates timestamps. This is room state, not
    /// connection state: a reconnect leaves it alone, and [`Self::replay`] reads
    /// it back from the journal after a restart.
    last_activity_ms: Option<u64>,
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
    /// A commit has written its base and its publication is running outside
    /// the room lock, or a failed publication waits for the retry queue. A
    /// second commit is refused with `busy` until [`Self::finish`] or
    /// [`Self::finish_retry`], so two publications cannot reach the cabinet out
    /// of order.
    publishing: bool,
    /// A publication that failed retryably, waiting for the transport's queue.
    /// The room keeps `publishing` while it is here.
    retry: Option<Box<dyn Publication>>,
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
            base_version: version,
            journal_bytes: 0,
            last_activity_ms: None,
            applied_order: VecDeque::new(),
            applied: HashMap::new(),
            poisoned: None,
            conflict: None,
            publishing: false,
            retry: None,
        }
    }

    /// The authoritative adapter, for server-side inspection (tests, commit tools).
    pub fn adapter(&self) -> &A {
        &self.adapter
    }

    /// The adapter, mutably, for a control plane that moves the cabinet's marker
    /// outside the commit path (an adopted tag, an operator's `resolve`).
    /// Ordinary protocol traffic reaches the adapter only through this type's
    /// own methods.
    pub fn adapter_mut(&mut self) -> &mut A {
        &mut self.adapter
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

    /// A commit's publication is running, or a failed one waits for the retry
    /// queue. A second commit is `busy` while this is true.
    pub fn is_publishing(&self) -> bool {
        self.publishing
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
        if let Err(e) = self.adapter.record(self.version, actor, &applied.op, now_ms) {
            self.poisoned = Some(e.message.clone());
            let hint = Some(VersionHint { version: self.version, epoch: self.epoch });
            return Err(Rejected { op_id: applied.op_id, code: ErrorCode::Internal, retryable: false, hint, message: e.message });
        }
        // The record is on disk now: it counts towards the forced-commit
        // thresholds. Counting after `record` keeps a poisoned room from
        // claiming bytes the journal does not hold.
        self.journal_bytes = self.journal_bytes.saturating_add(journal_line_bytes(self.version, actor, &applied.op, now_ms));
        self.last_activity_ms = Some(now_ms);
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
    ///
    /// `recorded_ms` is the wall-clock time the record was appended (the
    /// journal's `ms`). It is what makes the idle threshold a state that
    /// survives a restart: the room re-arms its clock from the newest entry
    /// instead of starting at zero. An older journal without timestamps passes
    /// `0`, which arms nothing until the next operation.
    pub fn replay(&mut self, actor: ActorId, op: Op, version: u64, recorded_ms: u64) -> Result<(), ServerError> {
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
        self.journal_bytes = self.journal_bytes.saturating_add(journal_line_bytes(version, &actor, &outcome.op, recorded_ms));
        if recorded_ms > 0 {
            self.last_activity_ms = Some(recorded_ms);
        }
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
    ///
    /// The returned [`Commit`] has already written the base; its publication
    /// (the cabinet POST) must be run by the caller **without** the room lock
    /// and reported back through [`Self::finish`].
    pub fn commit(&mut self, actor: &ActorId, expected_version: u64) -> Result<Commit, ServerError> {
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

    /// Bytes of journal records the room believes are waiting on disk.
    pub fn journal_bytes(&self) -> u64 {
        self.journal_bytes
    }

    /// The journal has grown past a configured threshold, so the room should be
    /// compacted by a server-initiated commit even though clients are connected
    /// (RFC §4.6, limitation 10). `publishing` is deliberately ignored: the
    /// caller retries when another publication is in flight, and a refused
    /// commit leaves the threshold crossed so the next call tries again.
    ///
    /// `now_ms` is the caller's wall clock. The operation and byte thresholds
    /// are counts; the time threshold is the idle time of the journal — the
    /// newest accepted operation is `forced_commit_idle_secs` old and nobody has
    /// added to it since. It needs a non-empty journal: a room whose base is
    /// written but not published is dirty without journal work, and republishing
    /// that base is the queued publication's job, not a compaction.
    pub fn forced_commit_due(&self, now_ms: u64) -> bool {
        if self.conflict.is_some() {
            // The cabinet moved on: only a re-seed can resolve it, and a commit
            // storm would not. The room stays readable and the journal keeps the
            // work until an operator acts.
            return false;
        }
        let ops = self.version.saturating_sub(self.base_version);
        let idle_ms = self.config.forced_commit_idle_secs.saturating_mul(1_000);
        (self.config.forced_commit_ops > 0 && ops >= self.config.forced_commit_ops)
            || (self.config.forced_commit_bytes > 0 && self.journal_bytes >= self.config.forced_commit_bytes)
            || (idle_ms > 0 && ops > 0 && self.last_activity_ms.is_some_and(|last| now_ms.saturating_sub(last) >= idle_ms))
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
    pub fn commit_now(&mut self) -> Result<Commit, ServerError> {
        self.write_commit()
    }

    /// Record the result of a [`Commit`]'s publication. The room lock is held
    /// again here; nothing is sent to another service.
    ///
    /// A retryable failure (the cabinet did not answer, a `5xx`) keeps the room
    /// `publishing` and hands the publication to the retry queue through
    /// [`Self::take_retry`]: a newer base must not overtake it, and the queue
    /// tries the same payload again without waiting for the next commit. A
    /// `409` or a non-retryable refusal releases the room and leaves it dirty,
    /// exactly as before (RFC §8.2, limitation 6).
    pub fn finish(&mut self, commit: Commit, outcome: PublishOutcome) -> Result<CommitResult, ServerError> {
        self.publishing = false;
        let version = commit.result.version;
        let retryable = is_retryable(&outcome);
        let retry = if retryable { commit.publication } else { None };
        let recorded = match outcome {
            Ok(published) => self.adapter.finish_publish(version, Ok(published)),
            Err(error) => {
                // A refused or failed publication never advances the marker;
                // the adapter may still want to see it, so it is told.
                let _ = self.adapter.finish_publish(version, Err(error.clone()));
                Err(error)
            }
        };
        match recorded {
            Ok(()) => {
                self.conflict = None;
                self.committed_version = self.committed_version.max(version);
                Ok(commit.result)
            }
            Err(error) => {
                if error.code == ErrorCode::Conflict {
                    self.conflict = Some(error.message.clone());
                } else if let Some(publication) = retry {
                    // The queue owns the attempt now. `publishing` stays true so
                    // no newer base can reach the cabinet before this one.
                    self.retry = Some(publication);
                    self.publishing = true;
                }
                Err(server_error(error.code, error.message, error.retryable))
            }
        }
    }

    /// The failed publication the retry queue has to try again. Taking it does
    /// not release the room: [`Self::finish_retry`] does.
    pub fn take_retry(&mut self) -> Option<Box<dyn Publication>> {
        self.retry.take()
    }

    /// Record the outcome of a queued retry. The publication belongs to the
    /// base written by the commit that queued it (`base_version`); the room is
    /// `publishing` for the whole queue, so no newer base exists.
    ///
    /// Another retryable failure leaves the room `publishing` (the queue
    /// schedules the next attempt); a success advances what the cabinet holds; a
    /// `409` flags the room `conflict` and stops the queue until an operator
    /// re-seeds it.
    pub fn finish_retry(&mut self, outcome: PublishOutcome) -> Result<(), ServerError> {
        let version = self.base_version;
        let retryable = is_retryable(&outcome);
        let recorded = match outcome {
            Ok(published) => self.adapter.finish_publish(version, Ok(published)),
            Err(error) => {
                let _ = self.adapter.finish_publish(version, Err(error.clone()));
                Err(error)
            }
        };
        match recorded {
            Ok(()) => {
                self.conflict = None;
                self.committed_version = self.committed_version.max(version);
                self.publishing = false;
                Ok(())
            }
            Err(error) => {
                if error.code == ErrorCode::Conflict {
                    self.conflict = Some(error.message.clone());
                    self.publishing = false;
                } else {
                    self.publishing = retryable;
                }
                Err(server_error(error.code, error.message, error.retryable))
            }
        }
    }

    /// Take the publication of the base the cabinet has not taken, if there is
    /// one: a base a commit wrote whose POST never landed, or one an operator
    /// just resolved (limitation 6). Neither case has a new commit to publish,
    /// so without this the room waits for the next commit, join or leave.
    ///
    /// The room is `publishing` from here until [`Self::finish_retry`] reports
    /// the outcome, so a newer base cannot overtake the queued one. The payload
    /// is the adapter's (`fileId`, the committed version, its bytes), so the
    /// retry carries the same idempotency key as the interrupted attempt.
    /// `Ok(None)` means there is nothing to republish: no publisher, no base,
    /// or a cabinet marker that already names the base.
    pub fn republish(&mut self) -> Result<Option<Box<dyn Publication>>, ServerError> {
        if self.publishing {
            return Err(server_error(ErrorCode::Busy, "another commit of this document is being published; try again", true));
        }
        let publication = self.adapter.pending_publication().map_err(|e| server_error(e.code, e.message, e.retryable))?;
        let Some(publication) = publication else { return Ok(None) };
        self.publishing = true;
        Ok(Some(publication))
    }

    /// An operator resolved a `conflict` room: the cabinet's copy now is
    /// `published` (its version and etag), so the room adopts that marker and
    /// clears the flag. The document stays dirty — the work the cabinet never
    /// took is still in the journal — and the next commit publishes it with the
    /// adopted version as `expectedVersion`.
    ///
    /// This is the only way out of `conflict`: retrying the same
    /// `expectedVersion` cannot succeed (RFC §8.2). It is refused when there is
    /// nothing to resolve, so a repeated operator call cannot move the marker
    /// again, and while a publication is in flight, because the queued attempt
    /// was addressed to the old marker.
    pub fn resolve_conflict(&mut self, published: Published) -> Result<(), ServerError> {
        if self.conflict.is_none() {
            return Err(server_error(ErrorCode::Conflict, "the session is not in conflict", false));
        }
        if self.publishing {
            return Err(server_error(ErrorCode::Busy, "a publication of this document is in flight; try again", true));
        }
        if let Some(current) = self.adapter.published()
            && published.version <= current.version
        {
            // Moving the marker backwards would make the next commit fail with
            // the same `409`: the operator has to name a version the cabinet
            // created after the one this session already holds.
            return Err(server_error(
                ErrorCode::Invalid,
                format!("the cabinet's version {} is not newer than the {} this session already holds", published.version, current.version),
                false,
            ));
        }
        self.adapter
            .set_published(Some(published))
            .map_err(|e| server_error(e.code, format!("could not record the cabinet's copy: {}", e.message), e.retryable))?;
        self.conflict = None;
        Ok(())
    }

    /// One code path for both commits: write the base and take the publication
    /// out of the adapter. An adapter that refuses because the cabinet's copy
    /// changed leaves the room dirty and flags it, so `/health` can report a
    /// session nobody can save.
    fn write_commit(&mut self) -> Result<Commit, ServerError> {
        if self.publishing {
            return Err(server_error(ErrorCode::Busy, "another commit of this document is being published; try again", true));
        }
        let version = self.version;
        let outcome = match self.adapter.commit(version, self.epoch) {
            Ok(outcome) => outcome,
            Err(e) => {
                if e.code == ErrorCode::Conflict {
                    self.conflict = Some(e.message.clone());
                }
                return Err(server_error(e.code, e.message, e.retryable));
            }
        };
        self.publishing = true;
        // The base at `version` now contains every accepted operation, so the
        // journal starts empty whatever the publication does next.
        self.base_version = version;
        self.journal_bytes = 0;
        self.last_activity_ms = None;
        Ok(Commit {
            result: CommitResult { version, sha256: outcome.sha256, bytes: outcome.bytes, conflict: outcome.conflict },
            publication: self.adapter.take_publication(),
        })
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

/// Is this failure worth another attempt later? A conflict never is (the
/// cabinet moved on, only a re-seed helps); anything the adapter marks
/// retryable is what the publication queue picks up.
fn is_retryable(outcome: &PublishOutcome) -> bool {
    matches!(outcome, Err(error) if error.code != ErrorCode::Conflict && error.retryable)
}

/// The size of the line the durable adapter writes for one accepted operation:
/// `{"version":N,"actor":"…","op":{…},"ms":T}` plus the newline
/// (`collab_pdf::store::JournalRecord`). The room keeps a byte counter so the
/// forced-commit threshold costs no `stat` per operation; the fields and their
/// values are the ones the adapter serialises, so the sum matches the file.
fn journal_line_bytes(version: u64, actor: &ActorId, op: &Op, recorded_ms: u64) -> u64 {
    serde_json::to_vec(&serde_json::json!({
        "version": version,
        "actor": actor.as_str(),
        "op": op,
        "ms": recorded_ms,
    }))
    .map(|line| line.len() as u64 + 1)
    .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::{Adapter, AdapterError, ApplyOutcome, CommitOutcome, Published, Room, RoomConfig};
    use pdfcraft_collab_proto::{
        ActorId, BlobRef, ClientInfo, CommitResult, ConflictClass, DocKey, ErrorCode, Hello, Limits, Op, OpId, OpKind, PageRef, Presence, Role,
        Selection, SessionId, Target, Viewport,
    };
    use serde_json::json;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    struct Fake {
        epoch: u64,
        applied: usize,
        /// Accepted operations as the room persisted them, when recording works.
        recorded: Vec<(u64, String, String)>,
        record_error: Option<&'static str>,
        /// What the adapter believes the cabinet holds.
        published: Option<Published>,
    }

    impl Fake {
        fn new() -> Self {
            Self { epoch: 0, applied: 0, recorded: Vec::new(), record_error: None, published: None }
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

        fn record(&mut self, version: u64, actor: &ActorId, op: &Op, _recorded_ms: u64) -> Result<(), AdapterError> {
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

        fn published(&self) -> Option<Published> {
            self.published.clone()
        }

        fn set_published(&mut self, published: Option<Published>) -> Result<(), AdapterError> {
            self.published = published;
            Ok(())
        }
    }

    fn room() -> Result<Room<Fake>, Box<dyn std::error::Error>> {
        Ok(Room::new(SessionId::new("session-1")?, DocKey::new("file-1")?, Fake::new(), RoomConfig { journal_len: 2, ..RoomConfig::default() }))
    }

    /// A room whose adapter leaves a publication for the transport, the way
    /// the cabinet adapter does.
    struct Publishing {
        inner: Fake,
        runs: Arc<AtomicUsize>,
        /// What the publication answers: `None` is a successful, empty one.
        failure: Option<AdapterError>,
    }

    struct Counting {
        runs: Arc<AtomicUsize>,
        failure: Option<AdapterError>,
    }

    impl super::Publication for Counting {
        fn run(&self) -> super::PublishOutcome {
            self.runs.fetch_add(1, Ordering::Relaxed);
            match &self.failure {
                Some(error) => Err(error.clone()),
                None => Ok(None),
            }
        }
    }

    impl Adapter for Publishing {
        fn apply(&mut self, actor: &ActorId, op: &Op) -> Result<ApplyOutcome, AdapterError> {
            self.inner.apply(actor, op)
        }
        fn commit(&mut self, version: u64, epoch: u64) -> Result<CommitOutcome, AdapterError> {
            self.inner.commit(version, epoch)
        }
        fn base(&self) -> Option<BlobRef> {
            self.inner.base()
        }
        fn published(&self) -> Option<Published> {
            self.inner.published()
        }
        fn set_published(&mut self, published: Option<Published>) -> Result<(), AdapterError> {
            self.inner.set_published(published)
        }
        fn take_publication(&mut self) -> Option<Box<dyn super::Publication>> {
            Some(Box::new(Counting { runs: self.runs.clone(), failure: self.failure.clone() }))
        }
    }

    fn publishing_room(failure: Option<AdapterError>) -> Result<(Room<Publishing>, Arc<AtomicUsize>), Box<dyn std::error::Error>> {
        publishing_room_with(failure, None)
    }

    /// An adapter whose committed base the cabinet has not taken: what a
    /// restart finds in the store, and what a `resolve` makes publishable again
    /// (limitation 6). It has no `commit` to hand out — the base is already on
    /// disk — so the publication only comes from `pending_publication`.
    struct Republishing {
        inner: Fake,
        runs: Arc<AtomicUsize>,
        /// What the restored publication answers: `None` is a successful one.
        failure: Option<AdapterError>,
        pending: bool,
    }

    impl Adapter for Republishing {
        fn apply(&mut self, actor: &ActorId, op: &Op) -> Result<ApplyOutcome, AdapterError> {
            self.inner.apply(actor, op)
        }
        fn commit(&mut self, version: u64, epoch: u64) -> Result<CommitOutcome, AdapterError> {
            self.inner.commit(version, epoch)
        }
        fn base(&self) -> Option<BlobRef> {
            self.inner.base()
        }
        fn published(&self) -> Option<Published> {
            self.inner.published()
        }
        fn set_published(&mut self, published: Option<Published>) -> Result<(), AdapterError> {
            self.inner.set_published(published)
        }
        fn pending_publication(&mut self) -> Result<Option<Box<dyn super::Publication>>, AdapterError> {
            if !self.pending {
                return Ok(None);
            }
            Ok(Some(Box::new(Counting { runs: self.runs.clone(), failure: self.failure.clone() })))
        }
        fn finish_publish(&mut self, _version: u64, outcome: super::PublishOutcome) -> Result<(), AdapterError> {
            // A base the cabinet took is not pending any more; a refused one
            // still is, which is what `resolve` re-arms.
            if outcome.is_ok() {
                self.pending = false;
            }
            Ok(())
        }
    }

    /// A room resumed from a base at `base` that the cabinet holds at
    /// `published`: the commit that wrote the base never reached it.
    fn restored_room(
        base: u64,
        published: u64,
        failure: Option<AdapterError>,
    ) -> Result<(Room<Republishing>, Arc<AtomicUsize>), Box<dyn std::error::Error>> {
        let runs = Arc::new(AtomicUsize::new(0));
        let adapter = Republishing { inner: Fake::new(), runs: runs.clone(), failure, pending: true };
        let mut room = Room::resume(SessionId::new("session-1")?, DocKey::new("file-1")?, adapter, base, 0, RoomConfig::default());
        room.mark_published(published);
        Ok((room, runs))
    }

    fn publishing_room_with(
        failure: Option<AdapterError>,
        published: Option<Published>,
    ) -> Result<(Room<Publishing>, Arc<AtomicUsize>), Box<dyn std::error::Error>> {
        let runs = Arc::new(AtomicUsize::new(0));
        let mut inner = Fake::new();
        inner.published = published;
        let adapter = Publishing { inner, runs: runs.clone(), failure };
        Ok((Room::new(SessionId::new("session-1")?, DocKey::new("file-1")?, adapter, RoomConfig::default()), runs))
    }

    fn actor(name: &str) -> Result<ActorId, Box<dyn std::error::Error>> {
        Ok(ActorId::new(name)?)
    }

    /// A commit the way the transport runs it: prepare under the lock, publish
    /// (nothing to do for these adapters), record.
    fn commit_and_publish<A: Adapter>(room: &mut Room<A>, actor: &ActorId, expected: u64) -> Result<CommitResult, super::ServerError> {
        let commit = room.commit(actor, expected)?;
        let outcome = commit.publish();
        room.finish(commit, outcome)
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
            selection: Some(Selection::Annotation { nm: "ann-1".into(), also: Vec::new() }),
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
        assert_eq!(
            commit_and_publish(&mut room, &viewer, 0).map(|_| ()),
            Err(super::server_error(ErrorCode::Forbidden, "this role cannot commit the file", false))
        );
        room.hello(editor.clone(), Role::Editor, &hello, 0)?;
        assert!(commit_and_publish(&mut room, &editor, 0).is_ok());
        assert!(matches!(commit_and_publish(&mut room, &editor, 7), Err(e) if e.code == ErrorCode::Stale && e.retryable));
        Ok(())
    }

    #[test]
    fn a_commit_publishes_after_the_room_is_released() -> Result<(), Box<dyn std::error::Error>> {
        let (mut room, runs) = publishing_room(None)?;
        let hello = hello()?;
        let editor = actor("editor")?;
        room.hello(editor.clone(), Role::Editor, &hello, 0)?;
        room.apply(&editor, &op("p1", OpKind::Primitive, ConflictClass::Merge, 0, 0)?, 0)?;

        let commit = room.commit(&editor, 1)?;
        assert_eq!(runs.load(Ordering::Relaxed), 0, "the cabinet call is not made while the room is held");
        assert!(room.is_dirty(), "the base is written but the publication is not done");
        // Two versions must not reach the cabinet out of order: a second commit
        // waits for the first publication.
        assert!(matches!(room.commit(&editor, 1), Err(e) if e.code == ErrorCode::Busy && e.retryable));

        let outcome = commit.publish();
        assert_eq!(runs.load(Ordering::Relaxed), 1);
        let result = room.finish(commit, outcome)?;
        assert_eq!(result.version, 1);
        assert!(!room.is_dirty(), "the publication advanced what the cabinet holds");
        assert!(commit_and_publish(&mut room, &editor, 1).is_ok(), "the room is free again");
        Ok(())
    }

    #[test]
    fn a_failed_publication_leaves_the_room_dirty_and_flags_it() -> Result<(), Box<dyn std::error::Error>> {
        let (mut room, runs) = publishing_room(Some(AdapterError::new(ErrorCode::Conflict, "the cabinet holds another version", false)))?;
        let hello = hello()?;
        let editor = actor("editor")?;
        room.hello(editor.clone(), Role::Editor, &hello, 0)?;
        room.apply(&editor, &op("p1", OpKind::Primitive, ConflictClass::Merge, 0, 0)?, 0)?;

        let commit = room.commit(&editor, 1)?;
        let outcome = commit.publish();
        assert_eq!(runs.load(Ordering::Relaxed), 1);
        assert!(matches!(room.finish(commit, outcome), Err(e) if e.code == ErrorCode::Conflict && !e.retryable));
        assert!(room.is_dirty(), "a refused publication leaves the room dirty so the next commit retries it");
        assert_eq!(room.conflict(), Some("the cabinet holds another version"));
        assert!(room.take_retry().is_none(), "a 409 stops the queue: only a re-seed can resolve it");
        assert!(!room.is_publishing(), "a 409 releases the room for the operator's own commit");
        Ok(())
    }

    /// A publication that fails retryably the first time and succeeds after.
    struct Flaky {
        inner: Fake,
        runs: Arc<AtomicUsize>,
    }

    struct FlakyPublication {
        runs: Arc<AtomicUsize>,
    }

    impl super::Publication for FlakyPublication {
        fn run(&self) -> super::PublishOutcome {
            if self.runs.fetch_add(1, Ordering::Relaxed) == 0 {
                Err(AdapterError::new(ErrorCode::Internal, "the cabinet did not answer", true))
            } else {
                Ok(None)
            }
        }
    }

    impl Adapter for Flaky {
        fn apply(&mut self, actor: &ActorId, op: &Op) -> Result<ApplyOutcome, AdapterError> {
            self.inner.apply(actor, op)
        }
        fn commit(&mut self, version: u64, epoch: u64) -> Result<CommitOutcome, AdapterError> {
            self.inner.commit(version, epoch)
        }
        fn base(&self) -> Option<BlobRef> {
            self.inner.base()
        }
        fn take_publication(&mut self) -> Option<Box<dyn super::Publication>> {
            Some(Box::new(FlakyPublication { runs: self.runs.clone() }))
        }
    }

    #[test]
    fn a_retryable_publication_failure_is_queued_and_keeps_the_room_busy() -> Result<(), Box<dyn std::error::Error>> {
        let runs = Arc::new(AtomicUsize::new(0));
        let adapter = Flaky { inner: Fake::new(), runs: runs.clone() };
        let mut room = Room::new(SessionId::new("session-1")?, DocKey::new("file-1")?, adapter, RoomConfig::default());
        let hello = hello()?;
        let editor = actor("editor")?;
        room.hello(editor.clone(), Role::Editor, &hello, 0)?;
        room.apply(&editor, &op("p1", OpKind::Primitive, ConflictClass::Merge, 0, 0)?, 0)?;

        let commit = room.commit(&editor, 1)?;
        let outcome = commit.publish();
        assert!(matches!(room.finish(commit, outcome), Err(e) if e.code == ErrorCode::Internal && e.retryable));
        assert!(room.is_dirty());
        assert!(room.is_publishing(), "a queued retry keeps the room publishing");
        assert_eq!(room.conflict(), None, "a transport failure is not a conflict");
        assert!(
            matches!(room.commit(&editor, 1), Err(e) if e.code == ErrorCode::Busy && e.retryable),
            "a newer base must not overtake the queued publication"
        );

        let retry = room.take_retry().ok_or("the failed publication is queued")?;
        assert!(room.take_retry().is_none(), "the queue owns exactly one attempt");
        room.finish_retry(retry.run())?;
        assert!(!room.is_dirty(), "the retry advanced what the cabinet holds");
        assert!(!room.is_publishing());
        assert!(commit_and_publish(&mut room, &editor, 1).is_ok(), "the room is free again");
        Ok(())
    }

    #[test]
    fn an_operator_resolution_adopts_the_cabinets_marker_and_clears_the_conflict() -> Result<(), Box<dyn std::error::Error>> {
        let (mut conflicted, _runs) = publishing_room(Some(AdapterError::new(ErrorCode::Conflict, "the cabinet holds another version", false)))?;
        let hello = hello()?;
        let editor = actor("editor")?;
        conflicted.hello(editor.clone(), Role::Editor, &hello, 0)?;
        conflicted.apply(&editor, &op("p1", OpKind::Primitive, ConflictClass::Merge, 0, 0)?, 0)?;
        let commit = conflicted.commit(&editor, 1)?;
        let outcome = commit.publish();
        assert!(conflicted.finish(commit, outcome).is_err());
        assert!(conflicted.conflict().is_some());

        // A resolve of a room that is not in conflict is refused and changes
        // nothing: the marker is only adopted once.
        let mut clean = room()?;
        assert!(matches!(clean.resolve_conflict(Published { version: 9, etag: "sha256:99".into() }), Err(e) if e.code == ErrorCode::Conflict));
        assert_eq!(clean.adapter().published(), None, "a refused resolve must not move the marker");

        let cabinet = Published { version: 7, etag: "sha256:77".into() };
        conflicted.resolve_conflict(cabinet.clone())?;
        assert_eq!(conflicted.conflict(), None, "the operator resolved it");
        assert_eq!(conflicted.adapter().published(), Some(cabinet), "the room adopted what the cabinet holds");
        assert!(conflicted.is_dirty(), "the work the cabinet never took is still unpublished");
        assert!(!conflicted.is_publishing(), "nothing holds the room back from publishing again");

        // A repeated resolve finds no conflict and is refused without a change.
        assert!(matches!(conflicted.resolve_conflict(Published { version: 11, etag: "sha256:11".into() }), Err(e) if e.code == ErrorCode::Conflict));
        assert_eq!(conflicted.adapter().published().map(|p| p.version), Some(7), "the refused resolve left the marker alone");
        Ok(())
    }

    #[test]
    fn a_resolution_must_name_a_version_the_cabinet_moved_to() -> Result<(), Box<dyn std::error::Error>> {
        let (mut room, _runs) = publishing_room_with(
            Some(AdapterError::new(ErrorCode::Conflict, "the cabinet holds another version", false)),
            Some(Published { version: 5, etag: "sha256:55".into() }),
        )?;
        let hello = hello()?;
        let editor = actor("editor")?;
        room.hello(editor.clone(), Role::Editor, &hello, 0)?;
        room.apply(&editor, &op("p1", OpKind::Primitive, ConflictClass::Merge, 0, 0)?, 0)?;
        let commit = room.commit(&editor, 1)?;
        let outcome = commit.publish();
        assert!(room.finish(commit, outcome).is_err());

        // The session already knows the cabinet holds version 5: a resolve that
        // repeats it (or goes back) would just fail again.
        for version in [5, 4, 0] {
            let refused = room.resolve_conflict(Published { version, etag: "sha256:66".into() }).expect_err("not newer");
            assert_eq!(refused.code, ErrorCode::Invalid, "version {version}");
            assert!(room.conflict().is_some(), "a refused resolve changes nothing");
        }
        room.resolve_conflict(Published { version: 6, etag: "sha256:66".into() })?;
        assert_eq!(room.conflict(), None);
        assert_eq!(room.adapter().published().map(|p| p.version), Some(6));
        Ok(())
    }

    #[test]
    fn a_resolution_waits_for_a_publication_in_flight() -> Result<(), Box<dyn std::error::Error>> {
        // A conflicted room may still be committed by its editor (a `409` does
        // not block edits or a retry of the commit). While that publication is
        // in flight, the marker must not move under it.
        let (mut room, _runs) = publishing_room(Some(AdapterError::new(ErrorCode::Conflict, "the cabinet holds another version", false)))?;
        let hello = hello()?;
        let editor = actor("editor")?;
        room.hello(editor.clone(), Role::Editor, &hello, 0)?;
        room.apply(&editor, &op("p1", OpKind::Primitive, ConflictClass::Merge, 0, 0)?, 0)?;
        let commit = room.commit(&editor, 1)?;
        let outcome = commit.publish();
        assert!(room.finish(commit, outcome).is_err());
        assert!(room.conflict().is_some());
        let second = room.commit(&editor, 1)?;
        assert!(room.is_publishing());

        let refused = room.resolve_conflict(Published { version: 4, etag: "sha256:44".into() }).expect_err("a publication is in flight");
        assert_eq!(refused.code, ErrorCode::Busy);
        assert!(refused.retryable);
        assert!(room.conflict().is_some(), "the refusal changed nothing");
        assert_eq!(room.adapter().published(), None);

        let failed = second.publish();
        assert!(room.finish(second, failed).is_err());
        assert!(room.conflict().is_some(), "the publication's own answer decides, not the refused resolve");
        Ok(())
    }

    #[test]
    fn a_failure_no_retry_would_fix_is_not_queued() -> Result<(), Box<dyn std::error::Error>> {
        let (mut room, _runs) = publishing_room(Some(AdapterError::new(ErrorCode::Internal, "the cabinet refused the body", false)))?;
        let hello = hello()?;
        let editor = actor("editor")?;
        room.hello(editor.clone(), Role::Editor, &hello, 0)?;
        room.apply(&editor, &op("p1", OpKind::Primitive, ConflictClass::Merge, 0, 0)?, 0)?;

        let commit = room.commit(&editor, 1)?;
        let outcome = commit.publish();
        assert!(matches!(room.finish(commit, outcome), Err(e) if !e.retryable));
        assert!(room.take_retry().is_none());
        assert!(!room.is_publishing(), "the next commit retries it, not the queue");
        assert!(room.is_dirty());
        Ok(())
    }

    #[test]
    fn a_restored_base_is_published_without_a_commit_and_blocks_newer_ones() -> Result<(), Box<dyn std::error::Error>> {
        let (mut room, runs) = restored_room(3, 1, None)?;
        assert!(room.is_dirty(), "the base at 3 never reached the cabinet, which holds 1");

        let publication = room.republish()?.ok_or("the unpublished base is published")?;
        assert_eq!(runs.load(Ordering::Relaxed), 0, "taking it out does not run it");
        assert!(room.is_publishing(), "a newer base must not overtake the restored one");
        assert!(matches!(room.commit_now(), Err(e) if e.code == ErrorCode::Busy));
        assert!(matches!(room.republish(), Err(e) if e.code == ErrorCode::Busy), "one attempt per base");

        room.finish_retry(publication.run())?;
        assert_eq!(runs.load(Ordering::Relaxed), 1);
        assert!(!room.is_publishing());
        assert!(!room.is_dirty(), "the cabinet now holds the base at 3");
        assert!(room.republish()?.is_none(), "nothing is left to publish");
        Ok(())
    }

    #[test]
    fn a_restored_base_the_cabinet_refuses_flags_the_room_and_stops() -> Result<(), Box<dyn std::error::Error>> {
        let (mut room, _runs) = restored_room(2, 1, Some(AdapterError::new(ErrorCode::Conflict, "the cabinet holds another version", false)))?;
        let publication = room.republish()?.ok_or("the unpublished base is published")?;
        let refused = room.finish_retry(publication.run()).expect_err("the cabinet refused it");
        assert_eq!(refused.code, ErrorCode::Conflict);
        assert!(!refused.retryable, "a 409 is never retried");
        assert!(room.conflict().is_some(), "/health has to name the room");
        assert!(!room.is_publishing(), "the queue is released, not kept busy");
        assert!(room.is_dirty(), "the work the cabinet never took is still here");
        Ok(())
    }

    #[test]
    fn a_restored_base_that_failed_retryably_keeps_the_room_publishing() -> Result<(), Box<dyn std::error::Error>> {
        let (mut room, _runs) = restored_room(2, 1, Some(AdapterError::new(ErrorCode::Internal, "the cabinet did not answer", true)))?;
        let publication = room.republish()?.ok_or("the unpublished base is published")?;
        let failed = room.finish_retry(publication.run()).expect_err("the cabinet did not answer");
        assert!(failed.retryable);
        assert!(room.is_publishing(), "the queue schedules the next attempt");
        assert!(room.conflict().is_none());
        assert_eq!(room.adapter().published().map(|published| published.version), None, "a failed attempt advances nothing");
        Ok(())
    }

    #[test]
    fn an_adapter_with_nothing_unpublished_republishes_nothing() -> Result<(), Box<dyn std::error::Error>> {
        let runs = Arc::new(AtomicUsize::new(0));
        let adapter = Republishing { inner: Fake::new(), runs, failure: None, pending: false };
        let mut room = Room::new(SessionId::new("session-1")?, DocKey::new("file-1")?, adapter, RoomConfig::default());
        assert!(room.republish()?.is_none());
        assert!(!room.is_publishing(), "nothing was taken, nothing is held");
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
        assert!(commit_and_publish(&mut room, &editor, 1).is_ok());
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
        assert!(matches!(commit_and_publish(&mut room, &editor, 1), Err(e) if e.code == ErrorCode::Conflict && !e.retryable));
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
        room.replay(editor.clone(), op("a1", OpKind::Primitive, ConflictClass::Merge, 0, 0)?, 1, 0)?;
        room.replay(editor.clone(), op("s1", OpKind::Structural, ConflictClass::Structural, 1, 0)?, 2, 0)?;
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
        assert!(room.replay(editor.clone(), op("a2", OpKind::Primitive, ConflictClass::Merge, 4, 2)?, 3, 0).is_err(), "version 3 is behind the base");
        let mut foreign = op("a5", OpKind::Primitive, ConflictClass::Merge, 4, 2)?;
        foreign.doc = DocKey::new("file-2")?;
        assert!(room.replay(editor, foreign, 5, 0).is_err());
        assert_eq!(room.version(), 4);
        Ok(())
    }

    #[test]
    fn the_journal_threshold_turns_into_a_server_commit_and_resets() -> Result<(), Box<dyn std::error::Error>> {
        let mut room = Room::new(
            SessionId::new("session-1")?,
            DocKey::new("file-1")?,
            Fake::new(),
            RoomConfig { forced_commit_ops: 2, forced_commit_bytes: 0, ..RoomConfig::default() },
        );
        let hello = hello()?;
        let editor = actor("editor")?;
        room.hello(editor.clone(), Role::Editor, &hello, 0)?;
        room.apply(&editor, &op("f1", OpKind::Primitive, ConflictClass::Merge, 0, 0)?, 0)?;
        assert!(!room.forced_commit_due(0), "one operation is below the threshold");
        room.apply(&editor, &op("f2", OpKind::Primitive, ConflictClass::Merge, 0, 0)?, 0)?;
        assert!(room.forced_commit_due(0), "the journal reached the operation threshold");

        let commit = room.commit_now()?;
        let outcome = commit.publish();
        room.finish(commit, outcome)?;
        assert!(!room.forced_commit_due(0), "the base contains both operations, so the journal is empty again");
        assert_eq!(room.version(), 2);

        // The counter is real state, not a one-shot latch: the next two
        // operations fill the journal again.
        room.apply(&editor, &op("f3", OpKind::Primitive, ConflictClass::Merge, 0, 0)?, 0)?;
        room.apply(&editor, &op("f4", OpKind::Primitive, ConflictClass::Merge, 0, 0)?, 0)?;
        assert!(room.forced_commit_due(0));
        Ok(())
    }

    #[test]
    fn an_idle_journal_compacts_without_a_new_edit_and_a_reconnect_keeps_its_clock() -> Result<(), Box<dyn std::error::Error>> {
        let config = RoomConfig { forced_commit_ops: 1_000, forced_commit_bytes: 0, forced_commit_idle_secs: 120, ..RoomConfig::default() };
        let mut room = Room::new(SessionId::new("session-1")?, DocKey::new("file-1")?, Fake::new(), config);
        let hello = hello()?;
        let editor = actor("editor")?;
        room.hello(editor.clone(), Role::Editor, &hello, 1_000)?;
        room.apply(&editor, &op("i1", OpKind::Primitive, ConflictClass::Merge, 0, 0)?, 1_000)?;
        // One operation, far below the operation and byte thresholds: only time
        // can make this room compact itself.
        assert!(!room.forced_commit_due(1_000 + 119_999), "less than the idle time has passed");

        // A reconnect is not an edit: the clock belongs to the journal, so it
        // keeps running from the operation, not from the join.
        room.disconnect(&editor);
        room.hello(editor.clone(), Role::Editor, &hello, 60_000)?;
        assert!(!room.forced_commit_due(60_000), "the rejoined room is still within its idle window");
        assert!(room.forced_commit_due(1_000 + 120_000), "the journal has been idle long enough");

        // The compaction writes the base and empties the journal: nothing is
        // due until somebody edits again.
        let commit = room.commit_now()?;
        let outcome = commit.publish();
        room.finish(commit, outcome)?;
        assert!(!room.forced_commit_due(1_000 + 10 * 120_000), "an empty journal has no idle clock");

        // `0` disables the time threshold; the operation count still works.
        let mut off = Room::new(
            SessionId::new("session-2")?,
            DocKey::new("file-1")?,
            Fake::new(),
            RoomConfig { forced_commit_ops: 0, forced_commit_bytes: 0, forced_commit_idle_secs: 0, ..RoomConfig::default() },
        );
        off.hello(editor.clone(), Role::Editor, &hello, 0)?;
        off.apply(&editor, &op("i2", OpKind::Primitive, ConflictClass::Merge, 0, 0)?, 0)?;
        assert!(!off.forced_commit_due(u64::MAX), "a disabled time threshold never fires");
        Ok(())
    }

    #[test]
    fn a_replayed_journal_carries_its_idle_clock_across_a_restart() -> Result<(), Box<dyn std::error::Error>> {
        let config = RoomConfig { forced_commit_ops: 1_000, forced_commit_bytes: 0, forced_commit_idle_secs: 120, ..RoomConfig::default() };
        let editor = actor("editor")?;
        let mut room = Room::resume(SessionId::new("session-1")?, DocKey::new("file-1")?, Fake::new(), 0, 0, config.clone());
        room.replay(editor.clone(), op("r1", OpKind::Primitive, ConflictClass::Merge, 0, 0)?, 1, 5_000)?;
        assert!(!room.forced_commit_due(5_000 + 119_999), "the restart did not reset the clock to zero");
        assert!(room.forced_commit_due(5_000 + 120_000), "the record's timestamp is the idle start");

        // A journal written before records carried timestamps arms nothing:
        // the next edit starts the clock, and an operator can still commit.
        let mut old = Room::resume(SessionId::new("session-2")?, DocKey::new("file-1")?, Fake::new(), 0, 0, config);
        old.replay(editor, op("r2", OpKind::Primitive, ConflictClass::Merge, 0, 0)?, 1, 0)?;
        assert!(!old.forced_commit_due(u64::MAX / 2), "a record without a timestamp does not guess one");
        Ok(())
    }

    #[test]
    fn the_byte_threshold_counts_what_the_adapter_writes() -> Result<(), Box<dyn std::error::Error>> {
        let mut room = Room::new(
            SessionId::new("session-1")?,
            DocKey::new("file-1")?,
            Fake::new(),
            RoomConfig { forced_commit_ops: 0, forced_commit_bytes: 1, ..RoomConfig::default() },
        );
        let hello = hello()?;
        let editor = actor("editor")?;
        room.hello(editor.clone(), Role::Editor, &hello, 0)?;
        let first = op("b1", OpKind::Primitive, ConflictClass::Merge, 0, 0)?;
        room.apply(&editor, &first, 0)?;
        assert!(room.journal_bytes() > 0, "the room counts the line the adapter appended");
        assert!(room.forced_commit_due(0), "one byte is past a one-byte threshold");
        Ok(())
    }

    #[test]
    fn a_room_in_conflict_does_not_force_commits_but_keeps_the_journal() -> Result<(), Box<dyn std::error::Error>> {
        // A conflict is set either by `Adapter::commit` or by a refused
        // publication; here the simplest route: the adapter refuses the base.
        struct Conflicting(Fake);
        impl Adapter for Conflicting {
            fn apply(&mut self, actor: &ActorId, op: &Op) -> Result<ApplyOutcome, AdapterError> {
                self.0.apply(actor, op)
            }
            fn commit(&mut self, _version: u64, _epoch: u64) -> Result<CommitOutcome, AdapterError> {
                Err(AdapterError::new(ErrorCode::Conflict, "the cabinet holds another version", false))
            }
            fn base(&self) -> Option<BlobRef> {
                self.0.base()
            }
        }

        let mut room = Room::new(
            SessionId::new("session-1")?,
            DocKey::new("file-1")?,
            Conflicting(Fake::new()),
            RoomConfig { forced_commit_ops: 1, forced_commit_bytes: 0, ..RoomConfig::default() },
        );
        let hello = hello()?;
        let editor = actor("editor")?;
        room.hello(editor.clone(), Role::Editor, &hello, 0)?;
        room.apply(&editor, &op("c1", OpKind::Primitive, ConflictClass::Merge, 0, 0)?, 0)?;
        assert!(room.forced_commit_due(0));
        assert!(room.commit_now().is_err());
        assert_eq!(room.conflict(), Some("the cabinet holds another version"));
        assert!(!room.forced_commit_due(0), "a conflict cannot be committed away: a retry storm would help nobody");
        assert_eq!(room.version(), 1, "the edit and its journal entry are still there for an operator");
        Ok(())
    }

    #[test]
    fn a_commit_in_flight_makes_the_threshold_wait_for_the_next_call() -> Result<(), Box<dyn std::error::Error>> {
        let runs = Arc::new(AtomicUsize::new(0));
        let adapter = Publishing { inner: Fake::new(), runs: runs.clone(), failure: None };
        let mut room = Room::new(
            SessionId::new("session-1")?,
            DocKey::new("file-1")?,
            adapter,
            RoomConfig { forced_commit_ops: 1, forced_commit_bytes: 0, ..RoomConfig::default() },
        );
        let hello = hello()?;
        let editor = actor("editor")?;
        room.hello(editor.clone(), Role::Editor, &hello, 0)?;
        room.apply(&editor, &op("p1", OpKind::Primitive, ConflictClass::Merge, 0, 0)?, 0)?;
        // A client commit is publishing; the threshold is still crossed but the
        // room answers `busy` until that publication finishes.
        let commit = room.commit(&editor, 1)?;
        room.apply(&editor, &op("p2", OpKind::Primitive, ConflictClass::Merge, 0, 0)?, 0)?;
        assert!(room.forced_commit_due(0));
        assert!(matches!(room.commit_now(), Err(e) if e.code == ErrorCode::Busy && e.retryable));
        let outcome = commit.publish();
        room.finish(commit, outcome)?;
        // The base written by the client commit holds version 1, not version 2,
        // so the threshold is still crossed and the next call succeeds.
        let forced = room.commit_now()?;
        let outcome = forced.publish();
        room.finish(forced, outcome)?;
        assert!(!room.forced_commit_due(0));
        Ok(())
    }
}
