//! In-memory delivery hub for collaboration sessions.
//!
//! The hub is the transport seam: it owns the clients of a document, passes
//! each [`ClientMessage`] to the room and delivers the resulting
//! [`ServerMessage`]s. A WebSocket server (or a host bridge) only has to turn
//! frames into `send`/`receive` calls; the ordering, roles, versions and
//! presence rules stay in [`pdfcraft_collab_server::Room`].
//!
//! Delivery is synchronous and single-threaded, so two or three headless
//! clients are tested without sockets, timers or async runtimes (RFC §11).

#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

use pdfcraft_collab_proto::{ActorId, ClientMessage, DocKey, ErrorCode, Hello, Role, ServerError, ServerMessage, SessionId};
use pdfcraft_collab_server::{Adapter, Room, RoomConfig};
use std::collections::{HashMap, VecDeque};

pub type ClientId = u64;

struct Client {
    actor: ActorId,
    doc: DocKey,
    inbox: VecDeque<ServerMessage>,
}

struct RoomEntry<A: Adapter> {
    room: Room<A>,
    members: Vec<ClientId>,
}

/// One process's rooms and connected clients.
pub struct Hub<A: Adapter> {
    next: u64,
    config: RoomConfig,
    clients: HashMap<ClientId, Client>,
    rooms: HashMap<DocKey, RoomEntry<A>>,
}

impl<A: Adapter> Hub<A> {
    pub fn new(config: RoomConfig) -> Self {
        Self { next: 1, config, clients: HashMap::new(), rooms: HashMap::new() }
    }

    /// Open the authoritative document for `doc`. A room outlives its members
    /// (reconnect and resync), so it is only removed by [`Self::close_room`].
    pub fn open_room(&mut self, doc: DocKey, adapter: A) -> Result<(), ServerError> {
        if self.rooms.contains_key(&doc) {
            return Err(error(ErrorCode::Invalid, "the document already has a room", false));
        }
        let session = SessionId::new(format!("session-{}", self.next)).map_err(|e| error(ErrorCode::Invalid, e.to_string(), false))?;
        self.next = self.next.saturating_add(1);
        let room = Room::new(session, doc.clone(), adapter, self.config.clone());
        self.rooms.insert(doc, RoomEntry { room, members: Vec::new() });
        Ok(())
    }

    pub fn close_room(&mut self, doc: &DocKey) -> bool {
        self.rooms.remove(doc).is_some()
    }

    /// Join a room. The caller has already verified the JWT and passes its
    /// actor and role; the room checks the lease and records the participant.
    pub fn connect(&mut self, actor: ActorId, role: Role, hello: &Hello, now_ms: u64) -> Result<ClientId, ServerError> {
        let doc = hello.doc.clone();
        let Some(entry) = self.rooms.get_mut(&doc) else {
            return Err(error(ErrorCode::Invalid, "no such document session", false));
        };
        let welcome = entry.room.hello(actor.clone(), role, hello, now_ms)?;
        let id = self.next;
        self.next = self.next.saturating_add(1);
        self.clients.insert(id, Client { actor, doc, inbox: VecDeque::from([ServerMessage::Welcome(welcome)]) });
        if let Some(entry) = self.rooms.get_mut(&hello.doc) {
            entry.members.push(id);
        }
        Ok(id)
    }

    /// Turn one client message into room actions and delivered messages.
    /// Rejections are delivered to the sender; applied operations are broadcast
    /// to every member, including the author.
    pub fn send(&mut self, id: ClientId, message: ClientMessage, now_ms: u64) -> Result<(), ServerError> {
        let (actor, doc) = {
            let client = self.clients.get(&id).ok_or_else(|| error(ErrorCode::Forbidden, "not connected", false))?;
            (client.actor.clone(), client.doc.clone())
        };
        let Some(entry) = self.rooms.get_mut(&doc) else {
            return Err(error(ErrorCode::Invalid, "no such document session", false));
        };
        let members = entry.members.clone();
        match message {
            ClientMessage::Op(op) => match entry.room.apply(&actor, &op, now_ms) {
                Ok(applied) => broadcast(&mut self.clients, &members, &ServerMessage::Applied(applied)),
                Err(rejected) => deliver(&mut self.clients, id, ServerMessage::Rejected(rejected)),
            },
            ClientMessage::Presence(presence) => match entry.room.presence(&actor, &presence, now_ms) {
                Ok(true) => broadcast_except(&mut self.clients, &members, id, &ServerMessage::Presence(presence)),
                Ok(false) => {}
                Err(e) => deliver(&mut self.clients, id, ServerMessage::Error(e)),
            },
            ClientMessage::Resync(req) => match entry.room.resync(&actor, &req) {
                Ok(snapshot) => deliver(&mut self.clients, id, ServerMessage::Snapshot(snapshot)),
                Err(e) => deliver(&mut self.clients, id, ServerMessage::Error(e)),
            },
            ClientMessage::Commit(req) => match entry.room.commit(&actor, req.expected_version) {
                Ok(result) => deliver(&mut self.clients, id, ServerMessage::CommitResult(result)),
                Err(e) => deliver(&mut self.clients, id, ServerMessage::Error(e)),
            },
            ClientMessage::Ping(ping) => {
                // A ping is the heartbeat that keeps the writing lease alive.
                entry.room.heartbeat(&actor, now_ms);
                deliver(&mut self.clients, id, ServerMessage::Pong(ping))
            }
            ClientMessage::Hello(_) => {
                deliver(&mut self.clients, id, ServerMessage::Error(error(ErrorCode::Invalid, "this connection is already joined", false)))
            }
        }
        Ok(())
    }

    /// Drain the messages waiting for `id`.
    pub fn receive(&mut self, id: ClientId) -> Vec<ServerMessage> {
        self.clients.get_mut(&id).map(|c| c.inbox.drain(..).collect()).unwrap_or_default()
    }

    pub fn disconnect(&mut self, id: ClientId) {
        let Some(client) = self.clients.remove(&id) else { return };
        if let Some(entry) = self.rooms.get_mut(&client.doc) {
            entry.members.retain(|member| *member != id);
            entry.room.disconnect(&client.actor);
        }
    }

    pub fn room(&self, doc: &DocKey) -> Option<&Room<A>> {
        self.rooms.get(doc).map(|entry| &entry.room)
    }

    pub fn room_mut(&mut self, doc: &DocKey) -> Option<&mut Room<A>> {
        self.rooms.get_mut(doc).map(|entry| &mut entry.room)
    }

    pub fn members(&self, doc: &DocKey) -> usize {
        self.rooms.get(doc).map_or(0, |entry| entry.members.len())
    }

    /// The document a connected client belongs to.
    pub fn client_doc(&self, id: ClientId) -> Option<DocKey> {
        self.clients.get(&id).map(|client| client.doc.clone())
    }

    pub fn docs(&self) -> impl Iterator<Item = &DocKey> {
        self.rooms.keys()
    }

    pub fn rooms(&self) -> usize {
        self.rooms.len()
    }

    pub fn clients(&self) -> usize {
        self.clients.len()
    }

    /// Write the session's head to its file when it changed and nobody reconnected.
    /// Returns the commit result, or `None` when the room is clean or unknown.
    pub fn commit_if_dirty(&mut self, doc: &DocKey) -> Option<Result<pdfcraft_collab_proto::CommitResult, ServerError>> {
        let entry = self.rooms.get_mut(doc)?;
        if !entry.room.is_dirty() {
            return None;
        }
        Some(entry.room.commit_now())
    }
}

fn deliver(clients: &mut HashMap<ClientId, Client>, id: ClientId, message: ServerMessage) {
    if let Some(client) = clients.get_mut(&id) {
        client.inbox.push_back(message);
    }
}

fn broadcast(clients: &mut HashMap<ClientId, Client>, members: &[ClientId], message: &ServerMessage) {
    for id in members {
        deliver(clients, *id, message.clone());
    }
}

fn broadcast_except(clients: &mut HashMap<ClientId, Client>, members: &[ClientId], except: ClientId, message: &ServerMessage) {
    for id in members.iter().filter(|id| **id != except) {
        deliver(clients, *id, message.clone());
    }
}

fn error(code: ErrorCode, message: impl Into<String>, retryable: bool) -> ServerError {
    ServerError { code, message: message.into(), retryable }
}

#[cfg(test)]
mod tests {
    use super::{ClientId, Hub};
    use pdfcraft_collab_proto::{
        ActorId, BlobRef, ClientInfo, ClientMessage, ConflictClass, DocKey, ErrorCode, Hello, Op, OpId, OpKind, PageRef, Presence, Role, Selection,
        ServerMessage, Target, Viewport,
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

        fn commit(&mut self) -> Result<CommitOutcome, AdapterError> {
            Ok(CommitOutcome { sha256: "abc".into(), bytes: 10, conflict: false })
        }

        fn base(&self) -> Option<BlobRef> {
            Some(BlobRef { sha256: "base".into(), len: 1 })
        }
    }

    fn hub() -> Result<(Hub<Fake>, DocKey), Box<dyn std::error::Error>> {
        let doc = DocKey::new("file-1")?;
        let mut hub = Hub::new(RoomConfig::default());
        hub.open_room(doc.clone(), Fake { epoch: 0 })?;
        Ok((hub, doc))
    }

    fn hello(doc: &DocKey) -> Result<Hello, Box<dyn std::error::Error>> {
        Ok(Hello {
            v: pdfcraft_collab_proto::PROTOCOL_VERSION,
            app: "pdfcraft".into(),
            doc: doc.clone(),
            jwt: "token".into(),
            client: ClientInfo { name: "test".into(), version: "0".into() },
        })
    }

    fn join(hub: &mut Hub<Fake>, doc: &DocKey, name: &str, role: Role) -> Result<ClientId, Box<dyn std::error::Error>> {
        let id = hub.connect(ActorId::new(name)?, role, &hello(doc)?, 0)?;
        let welcome = hub.receive(id);
        assert!(matches!(welcome.as_slice(), [ServerMessage::Welcome(_)]));
        Ok(id)
    }

    fn op(id: &str, class: ConflictClass, target: Target, base: u64, epoch: u64) -> Result<Op, Box<dyn std::error::Error>> {
        Ok(Op {
            op_id: OpId::new(id)?,
            doc: DocKey::new("file-1")?,
            base_version: base,
            epoch,
            kind: if class == ConflictClass::Structural { OpKind::Structural } else { OpKind::Primitive },
            class,
            target,
            payload: json!({"action": "test"}),
            preview: None,
        })
    }

    #[test]
    fn comments_reach_every_member_and_viewers_cannot_write() -> Result<(), Box<dyn std::error::Error>> {
        let (mut hub, doc) = hub()?;
        let editor = join(&mut hub, &doc, "editor", Role::Editor)?;
        let commenter = join(&mut hub, &doc, "commenter", Role::Commenter)?;
        let viewer = join(&mut hub, &doc, "viewer", Role::Viewer)?;
        assert_eq!(hub.members(&doc), 3);

        let add = op("add", ConflictClass::Merge, Target::Page(PageRef { index: 0, epoch: 0 }), 0, 0)?;
        hub.send(commenter, ClientMessage::Op(add), 0)?;
        for id in [editor, commenter, viewer] {
            assert!(hub.receive(id).iter().any(|m| matches!(m, ServerMessage::Applied(_))));
        }

        let forbidden = op("v1", ConflictClass::Merge, Target::Page(PageRef { index: 0, epoch: 0 }), 1, 0)?;
        hub.send(viewer, ClientMessage::Op(forbidden), 0)?;
        assert!(matches!(hub.receive(viewer).as_slice(), [ServerMessage::Rejected(e)] if e.code == ErrorCode::Forbidden));
        assert!(hub.receive(editor).is_empty());
        Ok(())
    }

    #[test]
    fn structural_is_serialized_and_presence_is_broadcast() -> Result<(), Box<dyn std::error::Error>> {
        let (mut hub, doc) = hub()?;
        let editor = join(&mut hub, &doc, "editor", Role::Editor)?;
        let commenter = join(&mut hub, &doc, "commenter", Role::Commenter)?;

        let rotate = op("rot", ConflictClass::Structural, Target::Page(PageRef { index: 0, epoch: 0 }), 0, 0)?;
        hub.send(editor, ClientMessage::Op(rotate), 0)?;
        for id in [editor, commenter] {
            let applied = hub.receive(id);
            assert!(matches!(applied.as_slice(), [ServerMessage::Applied(a)] if a.epoch == 1));
        }

        let old_epoch = op("m1", ConflictClass::Merge, Target::Page(PageRef { index: 0, epoch: 0 }), 1, 0)?;
        hub.send(commenter, ClientMessage::Op(old_epoch), 0)?;
        assert!(matches!(hub.receive(commenter).as_slice(), [ServerMessage::Rejected(e)] if e.code == ErrorCode::Stale && e.retryable));

        let presence = Presence {
            actor_id: ActorId::new("commenter")?,
            role: Role::Commenter,
            page: PageRef { index: 0, epoch: 1 },
            viewport: Viewport { x: 0.0, y: 0.0, zoom: 1.0 },
            cursor: Some([10, 20]),
            selection: Some(Selection::Annotation { nm: "ann".into() }),
            color: "#112233".into(),
        };
        hub.send(commenter, ClientMessage::Presence(presence), 1_000)?;
        assert!(matches!(hub.receive(editor).as_slice(), [ServerMessage::Presence(_)]));
        assert!(hub.receive(commenter).is_empty());
        Ok(())
    }

    #[test]
    fn a_second_editor_comments_but_waits_for_the_lease() -> Result<(), Box<dyn std::error::Error>> {
        let (mut hub, doc) = hub()?;
        let first = join(&mut hub, &doc, "first", Role::Editor)?;
        let second = join(&mut hub, &doc, "second", Role::Editor)?;

        let rotate = op("rot", ConflictClass::Structural, Target::Page(PageRef { index: 0, epoch: 0 }), 0, 0)?;
        hub.send(second, ClientMessage::Op(rotate.clone()), 0)?;
        assert!(matches!(hub.receive(second).as_slice(), [ServerMessage::Rejected(e)] if e.code == ErrorCode::Lease && e.retryable));

        let comment = op("c1", ConflictClass::Merge, Target::Page(PageRef { index: 0, epoch: 0 }), 0, 0)?;
        hub.send(second, ClientMessage::Op(comment), 0)?;
        assert!(matches!(hub.receive(second).as_slice(), [ServerMessage::Applied(_)]));
        let _ = hub.receive(first);

        let rotate_now = op("rot-now", ConflictClass::Structural, Target::Page(PageRef { index: 0, epoch: 0 }), 1, 0)?;
        hub.send(first, ClientMessage::Op(rotate_now), 0)?;
        assert!(matches!(hub.receive(first).as_slice(), [ServerMessage::Applied(a)] if a.epoch == 1));
        Ok(())
    }

    #[test]
    fn resync_and_commit_answer_the_requester() -> Result<(), Box<dyn std::error::Error>> {
        let (mut hub, doc) = hub()?;
        let editor = join(&mut hub, &doc, "editor", Role::Editor)?;
        let add = op("add", ConflictClass::Merge, Target::Page(PageRef { index: 0, epoch: 0 }), 0, 0)?;
        hub.send(editor, ClientMessage::Op(add), 0)?;
        let _ = hub.receive(editor);

        hub.send(editor, ClientMessage::Resync(pdfcraft_collab_proto::Resync { last_version: 0 }), 0)?;
        assert!(matches!(hub.receive(editor).as_slice(), [ServerMessage::Snapshot(s)] if s.version == 1));

        hub.send(editor, ClientMessage::Commit(pdfcraft_collab_proto::CommitRequest { expected_version: 1, message: None }), 0)?;
        assert!(matches!(hub.receive(editor).as_slice(), [ServerMessage::CommitResult(r)] if r.sha256 == "abc"));
        Ok(())
    }
}
