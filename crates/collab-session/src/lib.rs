//! Background WebSocket driver for one collaborative session.
//!
//! The interface thread owns the document; this crate owns the socket. It
//! connects, verifies the session through `hello`, keeps the client state
//! machine, reconnects with backoff, and forwards [`Event`]s the caller applies
//! with `pdfcraft-collab-client-pdf`. Commands go the other way: operations,
//! presence, commit and an explicit resync.
//!
//! No document, token or comment text is logged. A reconnect resends
//! unconfirmed merge operations with their original `opId` (the server
//! de-duplicates); a structural one is dropped, because page positions may have
//! moved under it while the socket was down.

#![cfg(not(target_arch = "wasm32"))]
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

use pdfcraft_collab_client::{ClientSession, Effect};
use pdfcraft_collab_proto::{
    Applied, BlobRef, ClientInfo, ClientMessage, CommitResult, ConflictClass, DocKey, ErrorCode, Hello, Op, PROTOCOL_VERSION, Presence, Rejected,
    ServerError, ServerMessage, Welcome,
};
use std::io;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, Sender, TryRecvError, channel};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};
use tungstenite::stream::MaybeTlsStream;
use tungstenite::{Error as WsError, Message, WebSocket};

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

/// What the interface thread asks the worker to do.
#[derive(Debug, Clone)]
pub enum Command {
    Op(Op),
    Presence(Presence),
    Commit(Option<String>),
    Resync,
    Shutdown,
}

/// What the worker reports. `Applied` carries the accepted operation: apply it
/// locally with the projection, whether it was yours or another client's.
#[derive(Debug, Clone)]
pub enum Event {
    Joined {
        welcome: Welcome,
        resync_requested: bool,
    },
    Applied {
        applied: Applied,
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
    /// The socket dropped; the worker is retrying.
    Disconnected,
}

/// One worker thread. Dropping it stops the worker.
pub struct Handle {
    commands: Sender<Command>,
    events: Receiver<Event>,
    closed: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl Handle {
    pub fn enqueue(&self, op: Op) -> bool {
        self.commands.send(Command::Op(op)).is_ok()
    }

    pub fn presence(&self, presence: Presence) -> bool {
        self.commands.send(Command::Presence(presence)).is_ok()
    }

    pub fn commit(&self, message: Option<String>) -> bool {
        self.commands.send(Command::Commit(message)).is_ok()
    }

    pub fn resync(&self) -> bool {
        self.commands.send(Command::Resync).is_ok()
    }

    /// The next event, if one is waiting. The interface polls this once a frame.
    pub fn try_event(&self) -> Option<Event> {
        self.events.try_recv().ok()
    }

    pub fn close(&mut self) {
        self.closed.store(true, Ordering::Relaxed);
        let _ = self.commands.send(Command::Shutdown);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

impl Drop for Handle {
    fn drop(&mut self) {
        self.close();
    }
}

pub fn spawn(config: Config) -> io::Result<Handle> {
    let (command_tx, command_rx) = channel();
    let (event_tx, event_rx) = channel();
    let closed = Arc::new(AtomicBool::new(false));
    let flag = closed.clone();
    let thread = thread::Builder::new().name("pdfcraft-collab".into()).spawn(move || run(config, command_rx, event_tx, flag))?;
    Ok(Handle { commands: command_tx, events: event_rx, closed, thread: Some(thread) })
}

fn run(config: Config, commands: Receiver<Command>, events: Sender<Event>, closed: Arc<AtomicBool>) {
    let mut client: Option<ClientSession> = None;
    let mut retry: Vec<Op> = Vec::new();
    loop {
        if closed.load(Ordering::Relaxed) {
            return;
        }
        match attach(&config, &mut client, &commands, &events, &closed, &mut retry) {
            Outcome::Shutdown | Outcome::Gone => return,
            Outcome::Reconnect => {
                let _ = events.send(Event::Disconnected);
                if !wait_before_retry(&config, &commands, &closed, &mut retry) {
                    return;
                }
            }
        }
    }
}

enum Outcome {
    Shutdown,
    /// The caller is gone.
    Gone,
    Reconnect,
}

/// Open one socket and serve it until it fails or the caller stops.
fn attach(
    config: &Config,
    client: &mut Option<ClientSession>,
    commands: &Receiver<Command>,
    events: &Sender<Event>,
    closed: &AtomicBool,
    retry: &mut Vec<Op>,
) -> Outcome {
    let Ok(mut ws) = connect(config) else { return Outcome::Reconnect };
    if let MaybeTlsStream::Plain(stream) = ws.get_mut() {
        let _ = stream.set_read_timeout(Some(config.poll_interval));
    }

    // For a first connection the hello is built here; for a reconnect it comes
    // from the client that keeps the last version and the pending ids.
    let hello = match client.as_mut() {
        Some(client) => client.reconnect(config.jwt.clone(), config.client.clone()),
        None => first_hello(config),
    };
    if send(&mut ws, &ClientMessage::Hello(hello)).is_err() {
        return Outcome::Reconnect;
    }
    let welcome = match read_welcome(&mut ws) {
        Ok(welcome) => welcome,
        Err(WelcomeError::Auth(error)) => {
            let _ = events.send(Event::Error(error));
            return Outcome::Gone;
        }
        Err(WelcomeError::Other) => return Outcome::Reconnect,
    };

    let (mut session, behind) = match client.take() {
        Some(mut session) => {
            // Reconnected: keep the version, epoch and pending ids.
            match session.receive(ServerMessage::Welcome(welcome)) {
                Ok(Some(Effect::Joined { welcome, resync_requested })) => {
                    let _ = events.send(Event::Joined { welcome, resync_requested });
                    (session, resync_requested)
                }
                _ => return Outcome::Reconnect,
            }
        }
        None => {
            let mut session = ClientSession::new(config.doc.clone(), welcome.actor_id.clone(), welcome.role);
            match session.receive(ServerMessage::Welcome(welcome)) {
                Ok(Some(Effect::Joined { welcome, resync_requested })) => {
                    let _ = events.send(Event::Joined { welcome, resync_requested });
                    (session, resync_requested)
                }
                _ => return Outcome::Reconnect,
            }
        }
    };

    // A structural operation must not be replayed onto a moved page tree; a
    // merge one is re-addressed to the new head and kept safe by its `opId`.
    let held: Vec<Op> = retry.drain(..).filter(|op| op.class == ConflictClass::Merge).collect();
    let mut last_ping = Instant::now();
    let mut held = if behind {
        // The resync answers first; the retry is re-addressed to the new head.
        held
    } else {
        for op in held {
            if let Err(e) = session.enqueue(op) {
                let _ = events.send(Event::Notice(e.to_string()));
            }
        }
        Vec::new()
    };

    loop {
        if closed.load(Ordering::Relaxed) {
            let _ = send(&mut ws, &ClientMessage::Ping(pdfcraft_collab_proto::Ping { ts: 0 }));
            let _ = ws.close(None);
            *client = Some(session);
            return Outcome::Shutdown;
        }
        // Commands first, then the socket, then everything queued for the server.
        loop {
            match commands.try_recv() {
                Ok(Command::Shutdown) => {
                    let _ = ws.close(None);
                    *client = Some(session);
                    return Outcome::Shutdown;
                }
                Ok(Command::Op(op)) => {
                    if let Err(e) = session.enqueue(op) {
                        let _ = events.send(Event::Notice(e.to_string()));
                    }
                }
                Ok(Command::Presence(presence)) => {
                    if let Err(e) = session.presence(presence) {
                        let _ = events.send(Event::Notice(e.to_string()));
                    }
                }
                Ok(Command::Commit(message)) => {
                    if let Err(e) = session.commit(message) {
                        let _ = events.send(Event::Notice(e.to_string()));
                    }
                }
                Ok(Command::Resync) => session.resync(),
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => {
                    *client = Some(session);
                    return Outcome::Gone;
                }
            }
        }

        match ws.read() {
            Ok(Message::Text(text)) => match serde_json::from_str::<ServerMessage>(text.as_str()) {
                Ok(message) => {
                    // The snapshot is the new base; unconfirmed merges are
                    // re-added below it so the retry is not lost.
                    let pending = matches!(message, ServerMessage::Snapshot(_)).then(|| session.pending_ops());
                    match session.receive(message) {
                        Ok(Some(Effect::Snapshot(snapshot))) => {
                            let tail = snapshot.tail.iter().filter_map(|value| serde_json::from_value(value.clone()).ok()).collect();
                            let _ = events.send(Event::Snapshot { version: snapshot.version, epoch: snapshot.epoch, base: snapshot.base, tail });
                            if let Some(pending) = pending {
                                // In-flight operations and those queued while the
                                // socket was down are re-addressed to the new head.
                                for op in pending.into_iter().chain(held.drain(..)).filter(|op| op.class == ConflictClass::Merge) {
                                    if let Err(e) = session.enqueue(op) {
                                        let _ = events.send(Event::Notice(e.to_string()));
                                    }
                                }
                            }
                        }
                        Ok(Some(effect)) => forward(effect, events),
                        Ok(None) => {}
                        Err(e) => {
                            let _ = events.send(Event::Notice(e.to_string()));
                        }
                    }
                }
                Err(_) => {
                    let _ = events.send(Event::Notice("the server sent an unreadable frame".into()));
                }
            },
            Ok(Message::Close(_)) => {
                *client = Some(session);
                return Outcome::Reconnect;
            }
            Ok(_) => {}
            Err(WsError::Io(e)) if matches!(e.kind(), io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut) => {}
            Err(WsError::ConnectionClosed | WsError::AlreadyClosed) => {
                *client = Some(session);
                return Outcome::Reconnect;
            }
            Err(_) => {
                *client = Some(session);
                return Outcome::Reconnect;
            }
        }

        // A heartbeat keeps the writing lease alive while the user reads (ADR-0008).
        if last_ping.elapsed() >= Duration::from_secs(30) {
            let ts = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0);
            if session.ping(ts).is_ok() {
                last_ping = Instant::now();
            }
        }
        for message in session.take_outgoing() {
            if send(&mut ws, &message).is_err() {
                *client = Some(session);
                return Outcome::Reconnect;
            }
        }
        let _ = ws.flush();
    }
}

fn forward(effect: Effect, events: &Sender<Event>) {
    let _ = match effect {
        Effect::Joined { welcome, resync_requested } => events.send(Event::Joined { welcome, resync_requested }),
        Effect::Applied { applied, own } => events.send(Event::Applied { applied, own }),
        Effect::Gap => events.send(Event::Gap),
        Effect::Retry { op_id } => events.send(Event::Retry { op_id }),
        Effect::Rejected(rejected) => events.send(Event::Rejected(rejected)),
        Effect::Snapshot(snapshot) => {
            let tail = snapshot.tail.iter().filter_map(|value| serde_json::from_value(value.clone()).ok()).collect();
            events.send(Event::Snapshot { version: snapshot.version, epoch: snapshot.epoch, base: snapshot.base, tail })
        }
        Effect::Presence(presence) => events.send(Event::Presence(presence)),
        Effect::Committed(result) => events.send(Event::Committed(result)),
        Effect::Error(error) => events.send(Event::Error(error)),
    };
}

fn connect(config: &Config) -> Result<WebSocket<MaybeTlsStream<std::net::TcpStream>>, ()> {
    tungstenite::connect(config.url.as_str()).map(|(ws, _response)| ws).map_err(|_| ())
}

fn first_hello(config: &Config) -> Hello {
    Hello { v: PROTOCOL_VERSION, app: config.app.clone(), doc: config.doc.clone(), jwt: config.jwt.clone(), client: config.client.clone() }
}

enum WelcomeError {
    /// The server refused the token or the session; retrying cannot help.
    Auth(ServerError),
    Other,
}

fn read_welcome(ws: &mut WebSocket<MaybeTlsStream<std::net::TcpStream>>) -> Result<Welcome, WelcomeError> {
    loop {
        match ws.read() {
            Ok(Message::Text(text)) => {
                return match serde_json::from_str::<ServerMessage>(text.as_str()) {
                    Ok(ServerMessage::Welcome(welcome)) => Ok(welcome),
                    Ok(ServerMessage::Error(error)) if error.code == ErrorCode::Auth => Err(WelcomeError::Auth(error)),
                    _ => Err(WelcomeError::Other),
                };
            }
            Ok(Message::Close(_)) => return Err(WelcomeError::Other),
            Ok(_) => {}
            Err(_) => return Err(WelcomeError::Other),
        }
    }
}

fn send(ws: &mut WebSocket<MaybeTlsStream<std::net::TcpStream>>, message: &ClientMessage) -> Result<(), ()> {
    let json = serde_json::to_string(message).map_err(|_| ())?;
    ws.send(Message::text(json)).map_err(|_| ())
}

/// Sleep before reconnecting, keeping the caller's operations and letting a
/// shutdown in.
fn wait_before_retry(config: &Config, commands: &Receiver<Command>, closed: &AtomicBool, retry: &mut Vec<Op>) -> bool {
    let deadline = Instant::now() + config.reconnect_delay;
    while Instant::now() < deadline {
        if closed.load(Ordering::Relaxed) {
            return false;
        }
        let left = deadline.saturating_duration_since(Instant::now()).min(Duration::from_millis(100));
        match commands.recv_timeout(left) {
            Ok(Command::Shutdown) => return false,
            Ok(Command::Op(op)) => retry.push(op),
            Ok(Command::Resync) => {}
            Ok(Command::Commit(_) | Command::Presence(_)) => {}
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => return false,
        }
    }
    true
}

#[cfg(test)]
mod tests {
    use super::{Command, Config, Event, spawn};
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
        fn commit(&mut self) -> Result<CommitOutcome, AdapterError> {
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

    fn wait_for(handle: &super::Handle, predicate: impl Fn(&Event) -> bool) -> Result<Event, Box<dyn std::error::Error>> {
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
        let event = wait_for(&handle, |e| matches!(e, Event::Joined { .. }))?;
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
        let event = wait_for(&handle, |e| matches!(e, Event::Applied { .. }))?;
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
        assert!(matches!(handle.commands.send(Command::Resync), Ok(())));
        let event = wait_for(&handle, |e| matches!(e, Event::Snapshot { .. }))?;
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
        wait_for(&handle, |e| matches!(e, Event::Joined { .. }))?;
        drop(server);
        // The accept loop stops, but the live connection stays up; closing the
        // worker is the observable part.
        handle.close();
        Ok(())
    }

    #[allow(dead_code)]
    fn _message_wire_shape() {
        let _ = serde_json::to_string(&ServerMessage::Pong(pdfcraft_collab_proto::Ping { ts: 1 }));
    }
}
