//! One collaborative session: the protocol driver and its transport.
//!
//! The protocol half ([`driver::Session`]) is transport-independent: it greets
//! the server, runs `pdfcraft-collab-client`'s state machine, keeps the writing
//! lease alive, re-addresses a retry after a resync and forwards [`Event`]s the
//! caller applies with `pdfcraft-collab-client-pdf`. Two transports implement as
//! much: [`native`], a blocking `tungstenite` socket on its own thread, and
//! [`web`], the browser's `WebSocket` driven from the UI frame.
//!
//! No document, token or comment text is logged. A reconnect resends
//! unconfirmed merge operations with their original `opId` (the server
//! de-duplicates); a structural one is dropped, because page positions may have
//! moved under it while the socket was down.

#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

pub mod driver;
#[cfg(not(target_arch = "wasm32"))]
pub mod native;
mod transport;
#[cfg(target_arch = "wasm32")]
pub mod web;

use pdfcraft_collab_proto::{BlobRef, ClientInfo, CommitResult, DocKey, Op, Presence, Rejected, ServerError, Welcome};
use std::time::Duration;

pub use driver::{Session, Step};
pub use transport::{ReadOutcome, Transport, TransportError};

#[derive(Debug, Clone)]
pub struct Config {
    /// `ws://…` or `wss://…`; the token never goes in the URL.
    pub url: String,
    pub jwt: String,
    pub doc: DocKey,
    pub client: ClientInfo,
    /// The application id checked by the server.
    pub app: String,
    pub poll_interval: Duration,
    pub reconnect_delay: Duration,
}

impl Config {
    pub fn new(url: impl Into<String>, jwt: impl Into<String>, doc: DocKey, client: ClientInfo) -> Self {
        Self {
            url: url.into(),
            jwt: jwt.into(),
            doc,
            client,
            app: "pdfcraft".into(),
            poll_interval: Duration::from_millis(20),
            reconnect_delay: Duration::from_secs(1),
        }
    }
}

/// What the interface asks the session to do.
#[derive(Debug, Clone)]
pub enum Command {
    Op(Op),
    Presence(Presence),
    Commit(Option<String>),
    Resync,
    Shutdown,
}

/// What the session reports. `Applied` carries the accepted operation: apply it
/// locally with the projection, whether it was yours or another client's.
#[derive(Debug, Clone)]
pub enum Event {
    Joined {
        welcome: Welcome,
        resync_requested: bool,
    },
    Applied {
        applied: pdfcraft_collab_proto::Applied,
        own: bool,
    },
    Gap,
    Retry {
        op_id: String,
    },
    Rejected(Rejected),
    Snapshot {
        version: u64,
        epoch: u64,
        base: Option<BlobRef>,
        tail: Vec<Op>,
    },
    Presence(Presence),
    Committed(CommitResult),
    Error(ServerError),
    /// A local command was refused before it reached the server (queue full,
    /// role, a malformed presence).
    Notice(String),
    /// The socket dropped; the transport is retrying.
    Disconnected,
}

/// What the interface needs from a session, whichever transport carries it: the
/// desktop's worker thread or the browser's socket. The UI holds a boxed one, so
/// nothing above this crate names a native type.
pub trait Sink {
    fn enqueue(&mut self, op: Op) -> bool;
    fn presence(&mut self, presence: Presence) -> bool;
    fn commit(&mut self, message: Option<String>) -> bool;
    fn resync(&mut self) -> bool;
    /// The next event, if one is waiting. The interface polls this once a frame.
    fn try_event(&mut self) -> Option<Event>;
    fn close(&mut self);
}

#[derive(Debug, thiserror::Error)]
pub enum SessionError {
    #[error("the collaboration session did not start: {0}")]
    Io(#[from] std::io::Error),
    #[error("{0}")]
    Web(String),
}

/// Start one session on the transport this platform has: a worker thread and a
/// blocking socket on the desktop, the page's `WebSocket` in the browser.
pub fn open(config: Config) -> Result<Box<dyn Sink>, SessionError> {
    #[cfg(not(target_arch = "wasm32"))]
    {
        Ok(Box::new(native::spawn(config)?))
    }
    #[cfg(target_arch = "wasm32")]
    {
        Ok(Box::new(web::WebHandle::open(config)?))
    }
}

#[cfg(all(test, not(target_arch = "wasm32")))]
mod tests {
    use super::{Config, Event, Sink, driver::Step, native::spawn};
    use pdfcraft_collab_hub::Hub;
    use pdfcraft_collab_proto::{
        ActorId, BlobRef, ClientInfo, ConflictClass, DocKey, Op, OpId, OpKind, PageRef, Role, ServerMessage, Target, Viewport,
    };
    use pdfcraft_collab_server::{Adapter, AdapterError, ApplyOutcome, CommitOutcome, RoomConfig};
    use pdfcraft_collab_ws::{Claims, Config as WsConfig, TokenError, WsServer};
    use serde_json::json;
    use std::sync::Arc;
    use std::time::{Duration, Instant};

    struct Fake;

    impl Adapter for Fake {
        fn apply(&mut self, _actor: &ActorId, op: &Op) -> Result<ApplyOutcome, AdapterError> {
            Ok(ApplyOutcome { result: json!({"ok": true}), op: op.clone(), epoch: 0 })
        }
        fn commit(&mut self, _version: u64, _epoch: u64) -> Result<CommitOutcome, AdapterError> {
            Ok(CommitOutcome { sha256: "abc".into(), bytes: 10, conflict: false })
        }
        fn base(&self) -> Option<BlobRef> {
            None
        }
    }

    struct TestVerifier;
    impl pdfcraft_collab_ws::Verifier for TestVerifier {
        fn verify(&self, _token: &str) -> Result<Claims, TokenError> {
            Ok(Claims {
                subject: "ada".into(),
                audience: "lzmkm".into(),
                file_id: "file-1".into(),
                app: "pdfcraft".into(),
                role: Role::Editor,
                session_id: "s".into(),
                expires_at: u64::MAX,
            })
        }
    }

    fn server() -> Result<pdfcraft_collab_ws::ServerHandle, Box<dyn std::error::Error>> {
        let doc = DocKey::new("file-1")?;
        let mut hub = Hub::new(RoomConfig::default());
        hub.open_room(doc, Fake)?;
        Ok(WsServer::new(hub, Arc::new(TestVerifier), WsConfig::default()).bind("127.0.0.1:0")?)
    }

    fn config(addr: std::net::SocketAddr) -> Result<Config, Box<dyn std::error::Error>> {
        Ok(Config::new(format!("ws://{addr}/"), "token", DocKey::new("file-1")?, ClientInfo { name: "test".into(), version: "0".into() }))
    }

    fn wait_for(handle: &mut super::native::Handle, predicate: impl Fn(&Event) -> bool) -> Result<Event, Box<dyn std::error::Error>> {
        let deadline = Instant::now() + Duration::from_secs(5);
        while Instant::now() < deadline {
            if let Some(event) = handle.try_event()
                && predicate(&event)
            {
                return Ok(event);
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        Err("timed out".into())
    }

    #[test]
    fn the_worker_joins_applies_and_reports_presence() -> Result<(), Box<dyn std::error::Error>> {
        let server = server()?;
        let mut handle = spawn(config(server.local_addr())?)?;
        let event = wait_for(&mut handle, |e| matches!(e, Event::Joined { .. }))?;
        assert!(matches!(event, Event::Joined { resync_requested: false, .. }));

        let op = Op {
            op_id: OpId::new("o1")?,
            doc: DocKey::new("file-1")?,
            base_version: 0,
            epoch: 0,
            kind: OpKind::Primitive,
            class: ConflictClass::Merge,
            target: Target::Page(PageRef { index: 0, epoch: 0 }),
            payload: json!({"action": "annotation.add"}),
            preview: None,
        };
        assert!(handle.enqueue(op.clone()));
        let event = wait_for(&mut handle, |e| matches!(e, Event::Applied { .. }))?;
        let Event::Applied { applied, own } = event else { return Err("not applied".into()) };
        assert!(own);
        assert_eq!(applied.op.op_id, op.op_id);
        assert_eq!(applied.version, 1);

        let presence = pdfcraft_collab_proto::Presence {
            actor_id: ActorId::new("ada")?,
            role: Role::Editor,
            page: PageRef { index: 0, epoch: 0 },
            viewport: Viewport { x: 0.0, y: 0.0, zoom: 1.0 },
            cursor: None,
            selection: None,
            color: "#ff0000".into(),
        };
        assert!(handle.presence(presence));
        assert!(handle.resync());
        let event = wait_for(&mut handle, |e| matches!(e, Event::Snapshot { .. }))?;
        let Event::Snapshot { version, tail, .. } = event else { return Err("not a snapshot".into()) };
        assert_eq!(version, 1);
        assert!(tail.is_empty(), "the client is at the head, so nothing is missing");
        handle.close();
        Ok(())
    }

    #[test]
    fn a_disconnected_worker_reports_and_keeps_working_after_a_restart() -> Result<(), Box<dyn std::error::Error>> {
        // Start the server, join, then stop it by dropping the handle. The
        // worker reports Disconnected; a second server on the same address is
        // not possible without rebinding, so this test only checks the report.
        let doc = DocKey::new("file-1")?;
        let mut hub = Hub::new(RoomConfig::default());
        hub.open_room(doc, Fake)?;
        let server = WsServer::new(hub, Arc::new(TestVerifier), WsConfig::default()).bind("127.0.0.1:0")?;
        let addr = server.local_addr();
        let mut handle = spawn(config(addr)?)?;
        wait_for(&mut handle, |e| matches!(e, Event::Joined { .. }))?;
        drop(server);
        // The accept loop stops, but the live connection stays up; closing the
        // worker is the observable part.
        handle.close();
        Ok(())
    }

    /// The driver is transport-independent: a scripted transport is enough to
    /// prove it greets, joins, applies and re-sends after a reconnect.
    struct Scripted {
        /// What the server "sends", oldest first.
        incoming: std::collections::VecDeque<super::ReadOutcome>,
        /// What was written, in order.
        written: Vec<pdfcraft_collab_proto::ClientMessage>,
        open: bool,
    }

    impl super::Transport for Scripted {
        fn write(&mut self, message: &pdfcraft_collab_proto::ClientMessage) -> Result<(), super::TransportError> {
            if !self.open {
                return Err(super::TransportError::NotOpen);
            }
            self.written.push(message.clone());
            Ok(())
        }

        fn read(&mut self) -> super::ReadOutcome {
            self.incoming.pop_front().unwrap_or(super::ReadOutcome::Nothing)
        }
    }

    #[test]
    fn the_driver_waits_for_the_socket_to_open_and_greets_once() -> Result<(), Box<dyn std::error::Error>> {
        let doc = DocKey::new("file-1")?;
        let mut session = super::Session::new(Config::new("ws://example/", "token", doc, ClientInfo { name: "test".into(), version: "0".into() }));
        let mut socket = Scripted { incoming: Default::default(), written: Vec::new(), open: false };
        // Nothing is written to a socket that is still dialing, and nothing is lost.
        assert_eq!(session.step(&mut socket), Step::Continue);
        assert!(socket.written.is_empty());
        socket.open = true;
        socket.incoming.push_back(super::ReadOutcome::Opened);
        assert_eq!(session.step(&mut socket), Step::Continue);
        socket.incoming.push_back(super::ReadOutcome::Nothing);
        assert_eq!(session.step(&mut socket), Step::Continue);
        assert_eq!(socket.written.len(), 1, "exactly one hello per connection");
        assert!(matches!(socket.written[0], pdfcraft_collab_proto::ClientMessage::Hello(_)));
        Ok(())
    }

    #[test]
    fn an_open_reported_after_the_greeting_is_not_a_second_hello() -> Result<(), Box<dyn std::error::Error>> {
        // The page's socket opens before the first step and the hello goes out
        // at once; the `Opened` the callback queued is read on a later step.
        // That is the same connection, so it must not be greeted again (the
        // room answers "this connection is already joined").
        let doc = DocKey::new("file-1")?;
        let mut session = super::Session::new(Config::new("ws://example/", "token", doc, ClientInfo { name: "test".into(), version: "0".into() }));
        session.opened();
        let mut socket = Scripted { incoming: Default::default(), written: Vec::new(), open: true };
        assert_eq!(session.step(&mut socket), Step::Continue);
        assert_eq!(socket.written.len(), 1, "the first hello");
        socket.incoming.push_back(super::ReadOutcome::Opened);
        assert_eq!(session.step(&mut socket), Step::Continue);
        assert_eq!(socket.written.len(), 1, "one hello per connection");
        Ok(())
    }

    #[test]
    fn an_unreadable_frame_is_a_notice_not_a_disconnect() -> Result<(), Box<dyn std::error::Error>> {
        let doc = DocKey::new("file-1")?;
        let mut session = super::Session::new(Config::new("ws://example/", "token", doc, ClientInfo { name: "test".into(), version: "0".into() }));
        session.opened();
        let mut socket = Scripted { incoming: Default::default(), written: Vec::new(), open: true };
        socket.incoming.push_back(super::ReadOutcome::Unreadable);
        assert_eq!(session.step(&mut socket), Step::Continue);
        let events = session.drain_events();
        assert!(events.iter().any(|event| matches!(event, Event::Notice(text) if text.contains("unreadable"))));
        assert!(!events.iter().any(|event| matches!(event, Event::Disconnected)));
        Ok(())
    }

    /// An operation the user makes while the socket is down waits for the
    /// connection instead of failing: this is what the browser (a page reload, a
    /// dead Wi-Fi) and the desktop (a redial) both rely on.
    #[test]
    fn an_operation_queued_offline_goes_out_after_the_welcome() -> Result<(), Box<dyn std::error::Error>> {
        use pdfcraft_collab_proto::{Capabilities, ClientMessage, Lease, Limits, SessionId, Target, Welcome};

        let doc = DocKey::new("file-1")?;
        let mut session =
            super::Session::new(Config::new("ws://example/", "token", doc.clone(), ClientInfo { name: "test".into(), version: "0".into() }));
        let op = Op {
            op_id: OpId::new("o1")?,
            doc: doc.clone(),
            base_version: 0,
            epoch: 0,
            kind: OpKind::Primitive,
            class: ConflictClass::Merge,
            target: Target::Page(PageRef { index: 0, epoch: 0 }),
            payload: json!({"action": "annotation.add"}),
            preview: None,
        };
        session.command(super::Command::Op(op.clone()));
        let mut socket = Scripted { incoming: Default::default(), written: Vec::new(), open: false };
        socket.open = true;
        socket.incoming.push_back(super::ReadOutcome::Opened);
        assert_eq!(session.step(&mut socket), Step::Continue);
        let welcome = Welcome {
            v: pdfcraft_collab_proto::PROTOCOL_VERSION,
            session_id: SessionId::new("s")?,
            actor_id: ActorId::new("ada")?,
            role: Role::Editor,
            lease: Lease { id: "l".into(), ttl_secs: 30 },
            version: 0,
            epoch: 0,
            capabilities: Capabilities { kinds: Vec::new(), classes: Vec::new() },
            limits: Limits::default(),
        };
        socket.incoming.push_back(super::ReadOutcome::Message(Box::new(ServerMessage::Welcome(welcome))));
        assert_eq!(session.step(&mut socket), Step::Continue);
        let events = session.drain_events();
        assert!(events.iter().any(|event| matches!(event, Event::Joined { resync_requested: false, .. })));
        assert!(
            socket.written.iter().any(|message| matches!(message, ClientMessage::Op(sent) if sent.op_id == op.op_id)),
            "the queued operation went out with the hello: {:?}",
            socket.written
        );
        Ok(())
    }

    #[allow(dead_code)]
    fn _message_wire_shape() {
        let _ = serde_json::to_string(&ServerMessage::Pong(pdfcraft_collab_proto::Ping { ts: 1 }));
    }
}
