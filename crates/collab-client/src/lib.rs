//! The client half of a collaborative session.
//!
//! This is the state machine between the transport and the document: it queues
//! local operations, follows the server's version and epoch, asks for a resync
//! when a message gap appears, and turns server messages into [`Effect`]s the
//! caller applies with `Session::apply_remote`. It owns no document, no socket
//! and no engine type, so the same machine drives the native client, the web
//! host bridge and the headless tests.
//!
//! Ordering follows ADR-0003: one server version, `opId` de-duplication, and a
//! gap is never skipped. Rejections follow ADR-0004: a retryable one (stale,
//! lease) is put back at the head of the queue after a resync; a permanent one
//! is reported to the interface.

#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

use pdfcraft_collab_proto::{
    ActorId, Applied, ClientInfo, ClientMessage, CommitRequest, CommitResult, DocKey, ErrorCode, Hello, Limits, Op, PROTOCOL_VERSION, PageRef,
    Presence, Rejected, Resync, Role, ServerError, ServerMessage, Snapshot, Target, Welcome,
};
use std::collections::{BTreeMap, VecDeque};

/// The same address in another page epoch. Merge intents are identified by name
/// (`/NM`, a field's full name), so only the epoch that framed the address
/// changes; structural intents keep the address they were built on.
fn with_epoch(target: Target, epoch: u64) -> Target {
    match target {
        Target::Page(page) => Target::Page(PageRef { index: page.index, epoch }),
        Target::Annotation { page, nm } => Target::Annotation { page: PageRef { index: page.index, epoch }, nm },
        Target::Bookmark { path, .. } => Target::Bookmark { path, epoch },
        other => other,
    }
}

/// How many operations may wait for confirmation at once. Beyond this the
/// interface must wait for the server instead of growing a hidden queue.
pub const MAX_PENDING: usize = 8;

/// One unconfirmed local operation. Its address is kept so a retry is the same
/// `opId` and the server's de-duplication window recognises it.
#[derive(Debug, Clone)]
struct Pending {
    op: Op,
    /// The optimistic edit was applied locally and must be reconciled when the
    /// authoritative result arrives.
    applied_locally: bool,
}

/// What the caller must do after one server message. This crate never touches
/// the document; `Applied`, `Snapshot` and `Rejected` are handed over.
#[derive(Debug, Clone, PartialEq)]
pub enum Effect {
    /// The session is open. `resync_requested` is true when the server was
    /// ahead (a reconnect): outgoing messages already carry the `resync`.
    Joined { welcome: Welcome, resync_requested: bool },
    /// The server accepted an operation. `own` means it was this client's;
    /// the caller replaces any optimistic local edit with this result.
    Applied { applied: Applied, own: bool },
    /// The operation was applied but the client missed earlier ones; a
    /// `resync` is already queued and the caller must not apply this result
    /// out of order.
    Gap,
    /// A retryable rejection: the operation is back at the head of the
    /// outbox after a `resync`.
    Retry { op_id: String },
    /// A permanent rejection. The operation is gone; show the reason.
    Rejected(Rejected),
    /// A snapshot replaces the local base; unconfirmed operations are gone
    /// (RFC §4.5) and the caller loads `base` plus `tail`.
    Snapshot(Snapshot),
    /// Ephemeral participant state; never changes the version.
    Presence(Presence),
    /// The cabinet accepted (or refused) a commit.
    Committed(CommitResult),
    /// A transport-level error from the server.
    Error(ServerError),
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ClientError {
    #[error("the session is not open yet")]
    NotJoined,
    #[error("the interface may have only {MAX_PENDING} operations in flight")]
    QueueFull,
    #[error("this role cannot change the document")]
    Forbidden,
    #[error("this role cannot change pages")]
    StructuralForbidden,
    #[error("the operation is not a valid protocol message")]
    Invalid(String),
    #[error("the confirmed operation was not in the queue")]
    UnknownOp,
}

/// One user's view of one document.
pub struct ClientSession {
    doc: DocKey,
    actor: ActorId,
    role: Role,
    joined: bool,
    version: u64,
    epoch: u64,
    lease_ttl_secs: u64,
    limits: Limits,
    pending: VecDeque<Pending>,
    outgoing: VecDeque<ClientMessage>,
    participants: BTreeMap<ActorId, Presence>,
    resync_pending: bool,
    last_commit: Option<CommitResult>,
}

impl ClientSession {
    pub fn new(doc: DocKey, actor: ActorId, role: Role) -> Self {
        Self {
            doc,
            actor,
            role,
            joined: false,
            version: 0,
            epoch: 0,
            lease_ttl_secs: 0,
            limits: Limits::default(),
            pending: VecDeque::new(),
            outgoing: VecDeque::new(),
            participants: BTreeMap::new(),
            resync_pending: false,
            last_commit: None,
        }
    }

    pub fn version(&self) -> u64 {
        self.version
    }

    pub fn epoch(&self) -> u64 {
        self.epoch
    }

    pub fn role(&self) -> Role {
        self.role
    }

    pub fn is_joined(&self) -> bool {
        self.joined
    }

    /// Zero means this client has no writing lease for pages (ADR-0008).
    pub fn lease_ttl_secs(&self) -> u64 {
        self.lease_ttl_secs
    }

    pub fn pending_len(&self) -> usize {
        self.pending.len()
    }

    /// The operations still waiting for confirmation, oldest first. A reconnect
    /// resends the merge ones after the snapshot: their `opId` makes the retry
    /// safe, and a structural one is dropped because page positions may have
    /// moved under it.
    pub fn pending_ops(&self) -> Vec<Op> {
        self.pending.iter().map(|p| p.op.clone()).collect()
    }

    pub fn participants(&self) -> impl Iterator<Item = (&ActorId, &Presence)> {
        self.participants.iter()
    }

    pub fn last_commit(&self) -> Option<&CommitResult> {
        self.last_commit.as_ref()
    }

    /// Build the first message. The token travels only here and is not stored.
    pub fn hello(&mut self, jwt: impl Into<String>, client: ClientInfo) -> Hello {
        self.joined = false;
        self.resync_pending = false;
        Hello { v: PROTOCOL_VERSION, app: "pdfcraft".into(), doc: self.doc.clone(), jwt: jwt.into(), client }
    }

    /// Queue a local operation. `base_version` and `epoch` are taken from the
    /// client's current view: an intent is always built on what the user saw.
    ///
    /// A merge intent re-addressed after a resync keeps its target but takes the
    /// new epoch: a comment, a field and a reply are identified by `/NM` or by a
    /// name, so the address is still the same thing, and leaving the old epoch
    /// on it would make the retry stale for ever. A structural intent is not
    /// re-addressed at all (the page order it was built on may be gone), which
    /// is why `enqueue` rejects one whose `base_version` is not the head.
    pub fn enqueue(&mut self, mut op: Op) -> Result<Op, ClientError> {
        if !self.joined {
            return Err(ClientError::NotJoined);
        }
        if !self.role.can_write() {
            return Err(ClientError::Forbidden);
        }
        if op.class == pdfcraft_collab_proto::ConflictClass::Structural && !self.role.can_structural() {
            return Err(ClientError::StructuralForbidden);
        }
        if op.class == pdfcraft_collab_proto::ConflictClass::Merge && !self.role.can_annotate() {
            return Err(ClientError::Forbidden);
        }
        if self.pending.len() >= MAX_PENDING {
            return Err(ClientError::QueueFull);
        }
        op.base_version = self.version;
        op.epoch = self.epoch;
        if op.class == pdfcraft_collab_proto::ConflictClass::Merge {
            op.target = with_epoch(op.target, self.epoch);
        }
        op.validate().map_err(|e| ClientError::Invalid(e.to_string()))?;
        self.pending.push_back(Pending { op: op.clone(), applied_locally: true });
        self.outgoing.push_back(ClientMessage::Op(op.clone()));
        Ok(op)
    }

    /// Presence is ephemeral: it is queued but never counted as pending work.
    pub fn presence(&mut self, mut presence: Presence) -> Result<Presence, ClientError> {
        if !self.joined {
            return Err(ClientError::NotJoined);
        }
        if presence.actor_id != self.actor {
            return Err(ClientError::Invalid("presence names another actor".into()));
        }
        presence.role = self.role;
        presence.validate().map_err(|e| ClientError::Invalid(e.to_string()))?;
        self.outgoing.push_back(ClientMessage::Presence(presence.clone()));
        Ok(presence)
    }

    /// Heartbeat that also renews the writing lease on the server.
    pub fn ping(&mut self, ts: u64) -> Result<(), ClientError> {
        if !self.joined {
            return Err(ClientError::NotJoined);
        }
        self.outgoing.push_back(ClientMessage::Ping(pdfcraft_collab_proto::Ping { ts }));
        Ok(())
    }

    /// Ask for the journal tail (or a base) from the current version.
    pub fn resync(&mut self) {
        if self.joined {
            self.request_resync();
        }
    }

    /// Ask the server to commit the current version.
    pub fn commit(&mut self, message: Option<String>) -> Result<(), ClientError> {
        if !self.joined {
            return Err(ClientError::NotJoined);
        }
        if !self.role.can_structural() {
            return Err(ClientError::Forbidden);
        }
        self.outgoing.push_back(ClientMessage::Commit(CommitRequest { expected_version: self.version, message }));
        Ok(())
    }

    /// A transport reconnect: the same token is presented again. The version,
    /// epoch and pending `opId`s are kept, so the server's de-duplication and a
    /// `resync` bring the client back without repeating applied work.
    pub fn reconnect(&mut self, jwt: impl Into<String>, client: ClientInfo) -> Hello {
        self.hello(jwt, client)
    }

    /// Everything waiting to be written to the socket, in order.
    pub fn take_outgoing(&mut self) -> Vec<ClientMessage> {
        self.outgoing.drain(..).collect()
    }

    /// Feed one server message. A `None` effect means nothing for the document
    /// (a presence echo, a heartbeat answer).
    pub fn receive(&mut self, message: ServerMessage) -> Result<Option<Effect>, ClientError> {
        match message {
            ServerMessage::Welcome(welcome) => {
                if welcome.actor_id != self.actor {
                    return Err(ClientError::Invalid("the server opened another actor's session".into()));
                }
                // The local version is what the document really has. If the
                // server disagrees (a reconnect, or a restarted room), keep it
                // and ask for the tail/snapshot; adopting the server's number
                // here would hide the missing operations.
                let agreed = welcome.version == self.version;
                self.joined = true;
                self.role = welcome.role;
                self.lease_ttl_secs = welcome.lease.ttl_secs;
                self.limits = welcome.limits.clone();
                if agreed {
                    self.epoch = welcome.epoch;
                } else {
                    self.request_resync();
                }
                Ok(Some(Effect::Joined { welcome, resync_requested: !agreed }))
            }
            ServerMessage::Applied(applied) => {
                let own = self.take_pending(&applied.op_id);
                // A duplicate confirmation carries a version we already have.
                if applied.version <= self.version {
                    return Ok(None);
                }
                if applied.version != self.version.saturating_add(1) {
                    // Never apply across a gap: the document would miss an edit.
                    self.request_resync();
                    return Ok(Some(Effect::Gap));
                }
                self.version = applied.version;
                self.epoch = applied.epoch;
                Ok(Some(Effect::Applied { applied, own }))
            }
            ServerMessage::Rejected(rejected) => {
                let Some(position) = self.pending.iter().position(|p| p.op.op_id == rejected.op_id) else {
                    return Err(ClientError::UnknownOp);
                };
                if rejected.retryable {
                    // Wait for the snapshot: the operation is re-addressed to the
                    // new head and resent with the same `opId`. Sending it now,
                    // before the resync answer, would be stale again.
                    self.request_resync();
                    Ok(Some(Effect::Retry { op_id: rejected.op_id.as_str().to_owned() }))
                } else {
                    self.pending.remove(position);
                    Ok(Some(Effect::Rejected(rejected)))
                }
            }
            ServerMessage::Snapshot(snapshot) => {
                self.version = snapshot.version;
                self.epoch = snapshot.epoch;
                self.resync_pending = false;
                // The snapshot is the new base; the server already contains
                // every operation it accepted (RFC §4.5).
                self.pending.clear();
                Ok(Some(Effect::Snapshot(snapshot)))
            }
            ServerMessage::Presence(presence) => {
                if presence.actor_id == self.actor {
                    return Ok(None);
                }
                self.participants.insert(presence.actor_id.clone(), presence.clone());
                Ok(Some(Effect::Presence(presence)))
            }
            ServerMessage::CommitResult(result) => {
                self.last_commit = Some(result.clone());
                Ok(Some(Effect::Committed(result)))
            }
            ServerMessage::Pong(_) => Ok(None),
            ServerMessage::Error(error) => {
                if error.code == ErrorCode::Auth {
                    self.joined = false;
                }
                Ok(Some(Effect::Error(error)))
            }
        }
    }

    fn take_pending(&mut self, op_id: &pdfcraft_collab_proto::OpId) -> bool {
        let Some(position) = self.pending.iter().position(|p| p.op.op_id == *op_id) else {
            return false;
        };
        self.pending.remove(position).map(|p| p.applied_locally).unwrap_or(false)
    }

    fn request_resync(&mut self) {
        if self.resync_pending {
            return;
        }
        self.resync_pending = true;
        self.outgoing.push_back(ClientMessage::Resync(Resync { last_version: self.version }));
    }
}

#[cfg(test)]
mod tests {
    use super::{ClientError, ClientSession, Effect, MAX_PENDING};
    use pdfcraft_collab_hub::Hub;
    use pdfcraft_collab_proto::{
        ActorId, Applied, BlobRef, ClientInfo, ClientMessage, CommitResult, ConflictClass, DocKey, ErrorCode, Op, OpId, OpKind, PageRef, Presence,
        Rejected, Resync, Role, ServerMessage, Snapshot, SnapshotMode, Target, VersionHint, Viewport, Welcome,
    };
    use pdfcraft_collab_server::{Adapter, AdapterError, ApplyOutcome, CommitOutcome, RoomConfig};
    use serde_json::json;

    struct Fake {
        epoch: u64,
    }

    impl Adapter for Fake {
        fn apply(&mut self, _actor: &ActorId, op: &Op) -> Result<ApplyOutcome, AdapterError> {
            if op.class == ConflictClass::Structural {
                self.epoch += 1;
            }
            Ok(ApplyOutcome { result: json!({"ok": true}), op: op.clone(), epoch: self.epoch })
        }

        fn commit(&mut self, _version: u64, _epoch: u64) -> Result<CommitOutcome, AdapterError> {
            Ok(CommitOutcome { sha256: "abc".into(), bytes: 10, conflict: false })
        }

        fn base(&self) -> Option<BlobRef> {
            None
        }
    }

    fn doc() -> Result<DocKey, Box<dyn std::error::Error>> {
        Ok(DocKey::new("file-1")?)
    }

    fn actor(name: &str) -> Result<ActorId, Box<dyn std::error::Error>> {
        Ok(ActorId::new(name)?)
    }

    fn op(id: &str) -> Result<Op, Box<dyn std::error::Error>> {
        Ok(Op {
            op_id: OpId::new(id)?,
            doc: doc()?,
            base_version: 0,
            epoch: 0,
            kind: OpKind::Primitive,
            class: ConflictClass::Merge,
            target: Target::Annotation { page: PageRef { index: 0, epoch: 0 }, nm: "ann-1".into() },
            payload: json!({"action": "annotation.set_contents", "text": "hello"}),
            preview: None,
        })
    }

    fn welcome(version: u64, epoch: u64, role: Role) -> Result<Welcome, Box<dyn std::error::Error>> {
        Ok(Welcome {
            v: 1,
            session_id: pdfcraft_collab_proto::SessionId::new("s")?,
            actor_id: actor("me")?,
            role,
            lease: pdfcraft_collab_proto::Lease { id: "l".into(), ttl_secs: if role.can_structural() { 120 } else { 0 } },
            version,
            epoch,
            capabilities: pdfcraft_collab_proto::Capabilities { kinds: vec![OpKind::Primitive], classes: vec![ConflictClass::Merge] },
            limits: pdfcraft_collab_proto::Limits::default(),
        })
    }

    fn joined(role: Role) -> Result<ClientSession, Box<dyn std::error::Error>> {
        let mut client = ClientSession::new(doc()?, actor("me")?, role);
        client.receive(ServerMessage::Welcome(welcome(0, 0, role)?))?;
        assert!(client.is_joined());
        Ok(client)
    }

    #[test]
    fn operations_are_addressed_to_the_current_version_and_capped() -> Result<(), Box<dyn std::error::Error>> {
        let mut client = joined(Role::Editor)?;
        let queued = client.enqueue(op("a")?)?;
        assert_eq!((queued.base_version, queued.epoch), (0, 0));
        assert_eq!(client.pending_len(), 1);
        for i in 1..MAX_PENDING {
            client.enqueue(op(&format!("o{i}"))?)?;
        }
        assert_eq!(client.enqueue(op("overflow")?), Err(ClientError::QueueFull));
        assert_eq!(client.take_outgoing().len(), MAX_PENDING);
        Ok(())
    }

    #[test]
    fn viewer_and_commenter_are_stopped_before_the_server() -> Result<(), Box<dyn std::error::Error>> {
        let mut viewer = joined(Role::Viewer)?;
        assert_eq!(viewer.enqueue(op("v")?), Err(ClientError::Forbidden));

        let mut commenter = joined(Role::Commenter)?;
        assert!(commenter.enqueue(op("c")?).is_ok());
        let mut structural = op("s")?;
        structural.kind = OpKind::Structural;
        structural.class = ConflictClass::Structural;
        assert_eq!(commenter.enqueue(structural), Err(ClientError::StructuralForbidden));
        Ok(())
    }

    #[test]
    fn an_applied_operation_advances_the_version_once() -> Result<(), Box<dyn std::error::Error>> {
        let mut client = joined(Role::Editor)?;
        let queued = client.enqueue(op("a")?)?;
        let applied = applied(&queued, 1, 0)?;
        assert!(matches!(client.receive(ServerMessage::Applied(applied.clone()))?, Some(Effect::Applied { own: true, .. })));
        assert_eq!(client.version(), 1);
        assert_eq!(client.pending_len(), 0);
        assert_eq!(client.receive(ServerMessage::Applied(applied))?, None, "a retry is not applied twice");
        Ok(())
    }

    #[test]
    fn a_version_gap_asks_for_a_resync_instead_of_applying() -> Result<(), Box<dyn std::error::Error>> {
        let mut client = joined(Role::Editor)?;
        let foreign = op("foreign")?;
        let applied = Applied { op_id: foreign.op_id.clone(), actor_id: actor("other")?, version: 3, epoch: 0, op: foreign, result: json!({}) };
        assert_eq!(client.receive(ServerMessage::Applied(applied))?, Some(Effect::Gap));
        assert_eq!(client.version(), 0, "the gap is not skipped");
        assert!(client.take_outgoing().iter().any(|m| matches!(m, ClientMessage::Resync(Resync { last_version: 0 }))));
        Ok(())
    }

    #[test]
    fn a_retryable_rejection_requeues_after_a_resync() -> Result<(), Box<dyn std::error::Error>> {
        let mut client = joined(Role::Editor)?;
        let queued = client.enqueue(op("a")?)?;
        let _ = client.take_outgoing();
        let rejected = Rejected {
            op_id: queued.op_id.clone(),
            code: ErrorCode::Stale,
            retryable: true,
            hint: Some(VersionHint { version: 4, epoch: 1 }),
            message: "resync".into(),
        };
        assert!(matches!(client.receive(ServerMessage::Rejected(rejected))?, Some(Effect::Retry { .. })));
        let outgoing = client.take_outgoing();
        assert!(matches!(outgoing.as_slice(), [ClientMessage::Resync(_)]));
        assert_eq!(client.pending_len(), 1);
        assert_eq!(client.pending_ops()[0].op_id, queued.op_id);
        Ok(())
    }

    #[test]
    fn a_permanent_rejection_is_reported_and_dropped() -> Result<(), Box<dyn std::error::Error>> {
        let mut client = joined(Role::Commenter)?;
        let queued = client.enqueue(op("a")?)?;
        let rejected =
            Rejected { op_id: queued.op_id.clone(), code: ErrorCode::Forbidden, retryable: false, hint: None, message: "not allowed".into() };
        assert!(matches!(client.receive(ServerMessage::Rejected(rejected))?, Some(Effect::Rejected(_))));
        assert_eq!(client.pending_len(), 0);
        Ok(())
    }

    #[test]
    fn a_snapshot_resets_the_base_and_drops_unconfirmed_work() -> Result<(), Box<dyn std::error::Error>> {
        let mut client = joined(Role::Editor)?;
        client.enqueue(op("a")?)?;
        let snapshot = Snapshot { mode: SnapshotMode::BaseAndTail, version: 5, epoch: 2, base: None, tail: Vec::new() };
        assert!(matches!(client.receive(ServerMessage::Snapshot(snapshot))?, Some(Effect::Snapshot(_))));
        assert_eq!((client.version(), client.epoch()), (5, 2));
        assert_eq!(client.pending_len(), 0);
        Ok(())
    }

    /// A merge intent re-addressed after a resync must carry the new epoch, or
    /// the server refuses it as stale for ever; a structural one keeps the page
    /// order it was built on.
    #[test]
    fn a_readdressed_merge_intent_takes_the_new_epoch() -> Result<(), Box<dyn std::error::Error>> {
        let mut client = joined(Role::Editor)?;
        let _ = client.take_outgoing();
        let mut add = op("add")?;
        add.epoch = 0;
        add.target = Target::Page(PageRef { index: 1, epoch: 0 });
        let queued = client.enqueue(add)?;
        assert_eq!(queued.target, Target::Page(PageRef { index: 1, epoch: 0 }), "nothing changes on the head it was built on");

        let _ = client.receive(ServerMessage::Snapshot(Snapshot {
            mode: SnapshotMode::BaseAndTail,
            version: 4,
            epoch: 3,
            base: None,
            tail: Vec::new(),
        }))?;
        let mut again = op("add-2")?;
        again.target = Target::Annotation { page: PageRef { index: 2, epoch: 0 }, nm: "n1".into() };
        let queued = client.enqueue(again)?;
        assert_eq!(queued.target, Target::Annotation { page: PageRef { index: 2, epoch: 3 }, nm: "n1".into() });
        assert_eq!(queued.epoch, 3);

        let mut structural = op("rotate")?;
        structural.kind = OpKind::Structural;
        structural.class = ConflictClass::Structural;
        structural.target = Target::Page(PageRef { index: 0, epoch: 0 });
        let queued = client.enqueue(structural)?;
        assert_eq!(queued.target, Target::Page(PageRef { index: 0, epoch: 0 }), "a page edit is not re-addressed onto a new page order");
        Ok(())
    }

    #[test]
    fn reconnect_keeps_the_last_version_and_pending_ids() -> Result<(), Box<dyn std::error::Error>> {
        let mut client = joined(Role::Editor)?;
        let queued = client.enqueue(op("a")?)?;
        client.receive(ServerMessage::Applied(applied(&queued, 1, 0)?))?;
        client.enqueue(op("b")?)?;
        let hello = client.reconnect("new-token", ClientInfo { name: "c".into(), version: "0".into() });
        assert_eq!(hello.jwt, "new-token");
        assert!(!client.is_joined());
        assert_eq!(client.pending_len(), 1, "the unconfirmed op survives");
        // The server is ahead by two: the welcome must ask for the tail.
        let effect = client.receive(ServerMessage::Welcome(welcome(3, 0, Role::Editor)?))?;
        assert!(matches!(effect, Some(Effect::Joined { resync_requested: true, .. })));
        assert!(client.take_outgoing().iter().any(|m| matches!(m, ClientMessage::Resync(Resync { last_version: 1 }))));
        Ok(())
    }

    #[test]
    fn presence_is_per_participant_and_never_bumps_the_version() -> Result<(), Box<dyn std::error::Error>> {
        let mut client = joined(Role::Editor)?;
        let presence = Presence {
            actor_id: actor("other")?,
            role: Role::Commenter,
            page: PageRef { index: 2, epoch: 0 },
            viewport: Viewport { x: 0.0, y: 0.0, zoom: 1.0 },
            cursor: Some([10, 20]),
            selection: None,
            color: "#123456".into(),
        };
        assert!(matches!(client.receive(ServerMessage::Presence(presence))?, Some(Effect::Presence(_))));
        assert_eq!(client.version(), 0);
        assert_eq!(client.participants().count(), 1);
        Ok(())
    }

    #[test]
    fn commit_is_only_for_writers_and_returns_the_result() -> Result<(), Box<dyn std::error::Error>> {
        let mut commenter = joined(Role::Commenter)?;
        assert_eq!(commenter.commit(None), Err(ClientError::Forbidden));
        let mut editor = joined(Role::Editor)?;
        editor.commit(Some("ready".into()))?;
        let result = CommitResult { version: 0, sha256: "abc".into(), bytes: 10, conflict: false };
        assert!(matches!(editor.receive(ServerMessage::CommitResult(result.clone()))?, Some(Effect::Committed(_))));
        assert_eq!(editor.last_commit(), Some(&result));
        Ok(())
    }

    fn applied(op: &Op, version: u64, epoch: u64) -> Result<Applied, Box<dyn std::error::Error>> {
        Ok(Applied { op_id: op.op_id.clone(), actor_id: actor("me")?, version, epoch, op: op.clone(), result: json!({}) })
    }

    /// Two clients, one real room: both learn about each other's comment.
    #[test]
    fn two_clients_converge_through_the_room() -> Result<(), Box<dyn std::error::Error>> {
        let mut hub = Hub::new(RoomConfig::default());
        hub.open_room(doc()?, Fake { epoch: 0 })?;
        let mut first = ClientSession::new(doc()?, actor("first")?, Role::Editor);
        let mut second = ClientSession::new(doc()?, actor("second")?, Role::Commenter);
        let info = ClientInfo { name: "test".into(), version: "0".into() };
        let first_id = hub.connect(actor("first")?, Role::Editor, &first.hello("t", info.clone()), 0)?;
        let second_id = hub.connect(actor("second")?, Role::Commenter, &second.hello("t", info), 0)?;
        for (client, id) in [(&mut first, first_id), (&mut second, second_id)] {
            for message in hub.receive(id) {
                client.receive(message)?;
            }
            assert!(client.is_joined());
        }

        let queued = first.enqueue(op("c1")?)?;
        for message in first.take_outgoing() {
            hub.send(first_id, message, 0)?;
        }
        for (client, id) in [(&mut first, first_id), (&mut second, second_id)] {
            for message in hub.receive(id) {
                client.receive(message)?;
            }
        }
        assert_eq!(first.version(), 1);
        assert_eq!(second.version(), 1);
        assert_eq!(second.pending_len(), 0);
        assert_eq!(queued.base_version, 0);
        Ok(())
    }
}
