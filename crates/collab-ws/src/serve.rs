//! A blocking WebSocket front end for [`Hub`].
//!
//! One thread per connection; the room and the hub are behind one mutex. Frames
//! are JSON text, checked against the protocol size limit before parsing. Each
//! connection has a bounded outbox: presence is dropped when the client falls
//! behind, and a client that cannot keep up with authoritative messages is
//! disconnected instead of growing memory (RFC §5).

use crate::token::{TokenError, Verifier};
use pdfcraft_collab_hub::{ClientId, Hub, Pending};
use pdfcraft_collab_proto::{ActorId, ClientMessage, DocKey, ErrorCode, Hello, MAX_MESSAGE_BYTES, ServerError, ServerMessage};
use pdfcraft_collab_server::Adapter;
use std::collections::HashMap;
use std::io;
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{SyncSender, TryRecvError, TrySendError, sync_channel};
use std::sync::{Arc, Mutex, MutexGuard};
use std::thread::{self, JoinHandle};
use std::time::Duration;
use tungstenite::handshake::server::{ErrorResponse, Request, Response};
use tungstenite::http::StatusCode;
use tungstenite::protocol::WebSocketConfig;
use tungstenite::{Error as WsError, Message, WebSocket, accept_hdr_with_config};

/// Transport settings. `app` is the only application this server accepts.
#[derive(Debug, Clone)]
pub struct Config {
    pub app: String,
    /// How often a connection wakes to write queued messages when idle.
    pub poll_interval: Duration,
    /// A connection that does not send `hello` in time is closed.
    pub handshake_timeout: Duration,
    /// A write that cannot finish in time closes the connection.
    pub write_timeout: Duration,
    /// When set, the `Origin` header must match (web client, RFC §12).
    pub origin: Option<String>,
    /// Outbox depth per connection.
    pub outbox: usize,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            app: "pdfcraft".into(),
            poll_interval: Duration::from_millis(10),
            handshake_timeout: Duration::from_secs(10),
            write_timeout: Duration::from_secs(10),
            origin: None,
            outbox: 256,
        }
    }
}

/// One process's hub plus the outboxes of its connected clients.
pub struct WsServer<A: Adapter> {
    shared: Arc<Mutex<Shared<A>>>,
    verifier: Arc<dyn Verifier>,
    config: Config,
}

struct Shared<A: Adapter> {
    hub: Hub<A>,
    sinks: HashMap<ClientId, SyncSender<ServerMessage>>,
    /// Commits that have written their base and wait for the cabinet call.
    /// The lock is released before they run (RFC §16, limitation 9).
    pending: Vec<Pending>,
    /// Server-initiated journal compactions so far (limitation 10). A counter,
    /// not a gauge: it only grows, and `/metrics` exposes it as a `_total`.
    forced_commits: u64,
}

impl<A: Adapter> Shared<A> {
    /// Move every queued room message to the matching connection.
    fn pump(&mut self) {
        let ids: Vec<ClientId> = self.sinks.keys().copied().collect();
        for id in ids {
            for message in self.hub.receive(id) {
                self.deliver(id, message);
            }
        }
    }

    fn deliver(&mut self, id: ClientId, message: ServerMessage) {
        let Some(sink) = self.sinks.get(&id).cloned() else { return };
        match sink.try_send(message) {
            Ok(()) => {}
            // Ephemeral state is allowed to go stale; a version is not.
            Err(TrySendError::Full(ServerMessage::Presence(_))) => {}
            Err(TrySendError::Full(_)) | Err(TrySendError::Disconnected(_)) => {
                let doc = self.hub.client_doc(id);
                self.sinks.remove(&id);
                self.hub.disconnect(id);
                if let Some(doc) = doc
                    && self.hub.members(&doc) == 0
                {
                    // A client that fell too far behind still leaves its work on disk.
                    if let Some(pending) = self.hub.commit_if_dirty(&doc) {
                        self.pending.push(pending);
                    }
                }
            }
        }
    }

    /// Take the commits whose publication has to run outside the lock.
    fn take_pending(&mut self) -> Vec<Pending> {
        std::mem::take(&mut self.pending)
    }

    /// Queue a journal compaction if the room crossed its threshold (RFC §4.6,
    /// limitation 10). The base is written here under the lock; the caller runs
    /// the returned publication outside it. A `busy` room (a publication
    /// already in flight) queues nothing: the threshold stays crossed, so the
    /// next message or the end of that publication retries.
    fn queue_forced(&mut self, doc: &DocKey) {
        if let Some(pending) = self.hub.forced_commit(doc) {
            self.forced_commits = self.forced_commits.saturating_add(1);
            self.pending.push(pending);
        }
    }
}

impl<A: Adapter> WsServer<A> {
    pub fn new(hub: Hub<A>, verifier: Arc<dyn Verifier>, config: Config) -> Self {
        Self { shared: Arc::new(Mutex::new(Shared { hub, sinks: HashMap::new(), pending: Vec::new(), forced_commits: 0 })), verifier, config }
    }

    /// Run the cabinet calls of commits that were prepared under the lock, and
    /// record their outcomes. The hub lock is released for every call: a
    /// cabinet that does not answer delays this connection, not the other
    /// documents in the process (RFC §16).
    fn run_pending(&self, mut pending: Vec<Pending>) {
        while let Some(mut item) = pending.pop() {
            let doc = item.doc().clone();
            let outcome = item.publish();
            let Ok(mut shared) = lock(&self.shared) else { return };
            let published = outcome.is_ok();
            shared.hub.finish(item, outcome);
            // Edits accepted while the publication ran may have crossed the
            // threshold again: the compaction is not lost behind the lock.
            if published && shared.hub.members(&doc) == 0 {
                // The last participant left while this publication ran: the
                // room still has to reach the cabinet (RFC §8). Only after a
                // successful publication, so a cabinet that keeps failing is
                // retried by the next commit, not in a loop here.
                if let Some(next) = shared.hub.commit_if_dirty(&doc) {
                    shared.pending.push(next);
                }
            }
            shared.queue_forced(&doc);
            shared.pump();
            pending.extend(shared.take_pending());
        }
    }

    /// Open a room before clients connect. The adapter owns the authoritative
    /// document; the transport never touches its contents.
    pub fn open_room(&self, doc: pdfcraft_collab_proto::DocKey, adapter: A) -> Result<(), TransportError> {
        lock(&self.shared)?.hub.open_room(doc, adapter).map_err(TransportError::Room)
    }

    /// Run `step` under the hub lock. This is the seam a control plane needs to
    /// check a room and open it in one atomic step (two `POST /v1/sessions` for
    /// the same file must not both create it). `step` must not call back into
    /// the server: the lock it holds is the one every method here takes.
    pub fn with_hub<T>(&self, step: impl FnOnce(&mut Hub<A>) -> T) -> Result<T, TransportError> {
        let mut shared = lock(&self.shared)?;
        Ok(step(&mut shared.hub))
    }
}

impl<A: Adapter + Send + 'static> WsServer<A> {
    /// Listen on `addr` and serve until the returned handle is dropped.
    pub fn bind(self, addr: &str) -> io::Result<ServerHandle> {
        Arc::new(self).bind_arc(addr)
    }

    /// The same, keeping the shared server so `/metrics` can read it later.
    pub fn bind_arc(self: Arc<Self>, addr: &str) -> io::Result<ServerHandle> {
        let listener = TcpListener::bind(addr)?;
        let local_addr = listener.local_addr()?;
        let stop = Arc::new(AtomicBool::new(false));
        let flag = stop.clone();
        let server = self;
        let thread = thread::Builder::new().name("collab-ws-accept".into()).spawn(move || {
            while !flag.load(Ordering::Relaxed) {
                match listener.accept() {
                    Ok((stream, _peer)) => {
                        let server = server.clone();
                        // A failed spawn drops the stream and closes the connection;
                        // one client never takes the listener down.
                        let _ = thread::Builder::new().name("collab-ws-client".into()).spawn(move || server.serve(stream));
                    }
                    Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
                    Err(_) => break,
                }
            }
        })?;
        Ok(ServerHandle { local_addr, stop, thread: Some(thread) })
    }

    fn serve(self: Arc<Self>, stream: TcpStream) {
        let _ = self.serve_inner(stream);
    }

    // The handshake callback's error type is tungstenite's `ErrorResponse`.
    #[allow(clippy::result_large_err)]
    fn serve_inner(&self, stream: TcpStream) -> Result<(), TransportError> {
        stream.set_nodelay(true).map_err(TransportError::Io)?;
        stream.set_read_timeout(Some(self.config.handshake_timeout)).map_err(TransportError::Io)?;
        stream.set_write_timeout(Some(self.config.write_timeout)).map_err(TransportError::Io)?;

        let origin = self.config.origin.clone();
        let mut ws_config = WebSocketConfig::default();
        ws_config.max_message_size = Some(MAX_MESSAGE_BYTES);
        ws_config.max_frame_size = Some(MAX_MESSAGE_BYTES);
        let mut ws = accept_hdr_with_config(
            stream,
            move |request: &Request, response: Response| {
                if let Some(expected) = origin.as_deref() {
                    let actual = request.headers().get("origin").and_then(|value| value.to_str().ok());
                    if actual != Some(expected) {
                        let mut denied = ErrorResponse::new(None);
                        *denied.status_mut() = StatusCode::FORBIDDEN;
                        return Err(denied);
                    }
                }
                Ok(response)
            },
            Some(ws_config),
        )
        .map_err(|e| TransportError::Handshake(e.to_string()))?;

        let hello = match read_hello(&mut ws) {
            Ok(hello) => hello,
            Err(message) => return reject(&mut ws, ErrorCode::Invalid, message),
        };
        if let Err(e) = ClientMessage::Hello(hello.clone()).validate() {
            return reject(&mut ws, ErrorCode::Invalid, e.to_string());
        }
        let claims = match self.verifier.verify(&hello.jwt) {
            Ok(claims) => claims,
            Err(TokenError::Expired) => return reject(&mut ws, ErrorCode::Auth, "the session token has expired; reconnect with a fresh one"),
            Err(e) => return reject(&mut ws, ErrorCode::Auth, e.to_string()),
        };
        if claims.app != self.config.app {
            return reject(&mut ws, ErrorCode::Unsupported, "the session token is for another application");
        }
        if claims.file_id != hello.doc.as_str() {
            return reject(&mut ws, ErrorCode::Forbidden, "the session token is for another file");
        }
        let actor = match ActorId::new(claims.subject.clone()) {
            Ok(actor) => actor,
            Err(e) => return reject(&mut ws, ErrorCode::Invalid, e.to_string()),
        };

        let doc = hello.doc.clone();
        let (sink, outbox) = sync_channel(self.config.outbox.max(1));
        let connected = {
            let mut shared = lock(&self.shared)?;
            match shared.hub.connect(actor, claims.role, &hello, now_ms()) {
                Ok(id) => {
                    shared.sinks.insert(id, sink);
                    shared.pump();
                    // A room resumed with a journal past the threshold is
                    // compacted as soon as somebody is there to keep the work
                    // alive (limitation 10).
                    shared.queue_forced(&doc);
                    let pending = shared.take_pending();
                    drop(shared);
                    self.run_pending(pending);
                    Ok(id)
                }
                Err(e) => Err(e),
            }
        };
        let id = match connected {
            Ok(id) => id,
            Err(e) => return reject(&mut ws, e.code, e.message),
        };

        ws.get_ref().set_read_timeout(Some(self.config.poll_interval)).map_err(TransportError::Io)?;
        let outcome = self.pump_loop(&mut ws, id, &outbox);
        let pending = {
            let mut shared = lock(&self.shared)?;
            shared.sinks.remove(&id);
            shared.hub.disconnect(id);
            if shared.hub.members(&doc) == 0 {
                // Last participant out: the server commits so the cabinet has the work (RFC §8).
                shared.hub.commit_if_dirty(&doc).into_iter().collect()
            } else {
                Vec::new()
            }
        };
        self.run_pending(pending);
        let _ = ws.close(None);
        let _ = ws.flush();
        outcome
    }

    /// Read one frame, then write whatever the room queued. The read timeout is
    /// the wake-up for outgoing messages; there is no timer thread.
    fn pump_loop(
        &self,
        ws: &mut WebSocket<TcpStream>,
        id: ClientId,
        outbox: &std::sync::mpsc::Receiver<ServerMessage>,
    ) -> Result<(), TransportError> {
        loop {
            match ws.read() {
                Ok(Message::Text(text)) => {
                    let pending = self.handle_text(ws, id, text.as_str())?;
                    // The author sees the acknowledgement of its own operation
                    // before a server-initiated compaction that operation
                    // triggered waits for the cabinet: the edit is confirmed,
                    // only the cabinet call stalls this connection.
                    if !drain_outbox(ws, outbox)? {
                        return Ok(());
                    }
                    flush(ws)?;
                    self.run_pending(pending);
                }
                Ok(Message::Binary(_)) => {
                    send_error(ws, ErrorCode::Unsupported, "binary frames are not supported")?;
                }
                Ok(Message::Close(_)) => return Ok(()),
                Ok(_) => {}
                Err(WsError::Io(e)) if matches!(e.kind(), io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut) => {}
                Err(WsError::ConnectionClosed | WsError::AlreadyClosed) => return Ok(()),
                Err(e) => return Err(TransportError::Socket(e.to_string())),
            }
            if !drain_outbox(ws, outbox)? {
                // The hub dropped this connection (backlog or shutdown).
                return Ok(());
            }
            flush(ws)?;
        }
    }

    fn handle_text(&self, ws: &mut WebSocket<TcpStream>, id: ClientId, text: &str) -> Result<Vec<Pending>, TransportError> {
        let message = match serde_json::from_str::<ClientMessage>(text) {
            Ok(message) => message,
            Err(_) => {
                send_error(ws, ErrorCode::Invalid, "the frame is not a protocol message")?;
                return Ok(Vec::new());
            }
        };
        if let Err(e) = message.validate() {
            send_error(ws, ErrorCode::Invalid, e.to_string())?;
            return Ok(Vec::new());
        }
        let pending = {
            let mut shared = lock(&self.shared)?;
            let doc = shared.hub.client_doc(id);
            let mut pending = match shared.hub.send(id, message, now_ms()) {
                Ok(pending) => pending.into_iter().collect::<Vec<_>>(),
                Err(e) => {
                    shared.deliver(id, ServerMessage::Error(e));
                    Vec::new()
                }
            };
            // The accepted operation may have filled the journal: compact it
            // while the client stays connected (limitation 10). A ping or a
            // presence frame retries a commit that was refused as busy.
            if let Some(doc) = doc {
                shared.queue_forced(&doc);
            }
            shared.pump();
            pending.extend(shared.take_pending());
            pending
        };
        // The caller runs these outside the hub lock: a commit answers only
        // after its publication ran.
        Ok(pending)
    }
}

/// Move every message the room queued to the socket. `false` means the hub
/// dropped this connection (backlog or shutdown).
fn drain_outbox(ws: &mut WebSocket<TcpStream>, outbox: &std::sync::mpsc::Receiver<ServerMessage>) -> Result<bool, TransportError> {
    loop {
        match outbox.try_recv() {
            Ok(message) => send(ws, &message)?,
            Err(TryRecvError::Empty) => return Ok(true),
            Err(TryRecvError::Disconnected) => return Ok(false),
        }
    }
}

/// Flush without treating a busy socket or an orderly close as a failure.
fn flush(ws: &mut WebSocket<TcpStream>) -> Result<(), TransportError> {
    match ws.flush() {
        Ok(()) => Ok(()),
        Err(WsError::Io(e)) if matches!(e.kind(), io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut) => Ok(()),
        Err(WsError::ConnectionClosed | WsError::AlreadyClosed) => Ok(()),
        Err(other) => Err(TransportError::Socket(other.to_string())),
    }
}

/// A point-in-time snapshot for `/health` and `/metrics`; no document content.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Metrics {
    pub rooms: usize,
    pub clients: usize,
    pub versions: Vec<(String, u64, u64)>,
    /// Rooms that refuse writes because their durable journal failed.
    pub poisoned: Vec<String>,
    /// Rooms that refuse commits because the cabinet's copy changed outside
    /// the session.
    pub conflict: Vec<String>,
    /// Server-initiated journal compactions since the process started
    /// (limitation 10), exposed as `pdfcraft_collab_forced_commits_total`.
    pub forced_commits: u64,
}

impl<A: Adapter> WsServer<A> {
    pub fn metrics(&self) -> Result<Metrics, TransportError> {
        let shared = lock(&self.shared)?;
        let mut versions = Vec::new();
        let mut poisoned = Vec::new();
        let mut conflict = Vec::new();
        for doc in shared.hub.docs() {
            if let Some(room) = shared.hub.room(doc) {
                versions.push((doc.as_str().to_owned(), room.version(), room.epoch()));
                if room.poisoned().is_some() {
                    poisoned.push(doc.as_str().to_owned());
                }
                if room.conflict().is_some() {
                    conflict.push(doc.as_str().to_owned());
                }
            }
        }
        Ok(Metrics { rooms: shared.hub.rooms(), clients: shared.sinks.len(), versions, poisoned, conflict, forced_commits: shared.forced_commits })
    }
}

/// A running listener. Dropping it stops the accept thread.
pub struct ServerHandle {
    local_addr: SocketAddr,
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl ServerHandle {
    pub fn local_addr(&self) -> SocketAddr {
        self.local_addr
    }
}

impl Drop for ServerHandle {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        // Wake the blocking `accept`; the loop sees the flag and exits.
        let _ = TcpStream::connect(self.wake_address());
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

impl ServerHandle {
    /// A wildcard bind cannot be connected to, so the wake-up goes to loopback.
    fn wake_address(&self) -> SocketAddr {
        if !self.local_addr.ip().is_unspecified() {
            return self.local_addr;
        }
        match self.local_addr {
            SocketAddr::V4(address) => SocketAddr::from(([127, 0, 0, 1], address.port())),
            SocketAddr::V6(address) => SocketAddr::from(([0, 0, 0, 0, 0, 0, 0, 1], address.port())),
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum TransportError {
    #[error("the hub is unavailable")]
    Hub,
    #[error("the handshake failed: {0}")]
    Handshake(String),
    #[error("the connection failed: {0}")]
    Socket(String),
    #[error("i/o failed: {0}")]
    Io(#[from] io::Error),
    #[error("{0}")]
    Room(ServerError),
}

fn lock<T>(mutex: &Mutex<T>) -> Result<MutexGuard<'_, T>, TransportError> {
    mutex.lock().map_err(|_| TransportError::Hub)
}

fn read_hello(ws: &mut WebSocket<TcpStream>) -> Result<Hello, String> {
    loop {
        match ws.read() {
            Ok(Message::Text(text)) => {
                let message: ClientMessage = serde_json::from_str(text.as_str()).map_err(|_| "hello must be a JSON protocol message".to_owned())?;
                return match message {
                    ClientMessage::Hello(hello) => Ok(hello),
                    _ => Err("the first message must be hello".to_owned()),
                };
            }
            Ok(Message::Ping(_) | Message::Pong(_)) => {}
            Ok(Message::Binary(_)) => return Err("binary frames are not supported".to_owned()),
            Ok(Message::Close(_)) => return Err("the connection closed before hello".to_owned()),
            Ok(_) => {}
            Err(WsError::Io(e)) if matches!(e.kind(), io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut) => {
                return Err("hello did not arrive in time".to_owned());
            }
            Err(_) => return Err("the connection failed before hello".to_owned()),
        }
    }
}

fn send(ws: &mut WebSocket<TcpStream>, message: &ServerMessage) -> Result<(), TransportError> {
    let json = serde_json::to_string(message).map_err(|e| TransportError::Socket(e.to_string()))?;
    ws.send(Message::text(json)).map_err(|e| TransportError::Socket(e.to_string()))
}

fn send_error(ws: &mut WebSocket<TcpStream>, code: ErrorCode, message: impl Into<String>) -> Result<(), TransportError> {
    send(ws, &ServerMessage::Error(ServerError { code, message: message.into(), retryable: false }))
}

fn reject(ws: &mut WebSocket<TcpStream>, code: ErrorCode, message: impl Into<String>) -> Result<(), TransportError> {
    let _ = send_error(ws, code, message);
    let _ = ws.close(None);
    let _ = ws.flush();
    Ok(())
}

/// Wall-clock milliseconds for versions, presence and the lease.
fn now_ms() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::{Config, TransportError, WsServer};
    use crate::token::{Claims, TokenError, Verifier};
    use pdfcraft_collab_hub::Hub;
    use pdfcraft_collab_proto::{
        ActorId, BlobRef, ClientMessage, ConflictClass, DocKey, ErrorCode, Op, OpId, OpKind, PageRef, Role, ServerMessage, Target,
    };
    use pdfcraft_collab_server::{Adapter, AdapterError, ApplyOutcome, CommitOutcome, RoomConfig};
    use serde_json::json;
    use std::sync::Arc;
    use std::time::Duration;
    use tungstenite::stream::MaybeTlsStream;
    use tungstenite::{Message, WebSocket};

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

    /// The transport tests do not exercise the signature; the token module does.
    struct TestVerifier;

    impl Verifier for TestVerifier {
        fn verify(&self, token: &str) -> Result<Claims, TokenError> {
            // `role` or `role:name`, so two clients can be different actors.
            let role = match token.split_once(':').map_or(token, |(role, _)| role) {
                "owner" => Role::Owner,
                "editor" => Role::Editor,
                "commenter" => Role::Commenter,
                "viewer" => Role::Viewer,
                _ => return Err(TokenError::Invalid),
            };
            Ok(Claims {
                subject: token.to_owned(),
                audience: "lzmkm".into(),
                file_id: "file-1".into(),
                app: "pdfcraft".into(),
                role,
                session_id: "session-9".into(),
                expires_at: u64::MAX,
            })
        }
    }

    fn start() -> Result<super::ServerHandle, TransportError> {
        let doc = DocKey::new("file-1").map_err(|e| TransportError::Socket(e.to_string()))?;
        let mut hub = Hub::new(RoomConfig::default());
        hub.open_room(doc, Fake { epoch: 0 }).map_err(|e| TransportError::Socket(e.message))?;
        let server = WsServer::new(hub, Arc::new(TestVerifier), Config { poll_interval: Duration::from_millis(2), ..Config::default() });
        server.bind("127.0.0.1:0").map_err(TransportError::Io)
    }

    struct Client {
        ws: WebSocket<MaybeTlsStream<std::net::TcpStream>>,
    }

    impl Client {
        fn join(addr: std::net::SocketAddr, token: &str, name: &str) -> Result<Self, Box<dyn std::error::Error>> {
            let (mut ws, _) = tungstenite::connect(format!("ws://{addr}/"))?;
            let hello = json!({
                "type": "hello",
                "v": 1,
                "app": "pdfcraft",
                "doc": "file-1",
                "jwt": token,
                "client": {"name": name, "version": "0"},
            });
            ws.send(Message::text(hello.to_string()))?;
            Ok(Self { ws })
        }

        fn next(&mut self) -> Result<ServerMessage, Box<dyn std::error::Error>> {
            loop {
                match self.ws.read()? {
                    Message::Text(text) => return Ok(serde_json::from_str(text.as_str())?),
                    Message::Close(_) => return Err("the server closed the connection".into()),
                    _ => {}
                }
            }
        }

        fn send(&mut self, message: serde_json::Value) -> Result<(), Box<dyn std::error::Error>> {
            self.ws.send(Message::text(message.to_string()))?;
            Ok(())
        }
    }

    fn comment(op_id: &str) -> Result<serde_json::Value, Box<dyn std::error::Error>> {
        let op = Op {
            op_id: OpId::new(op_id)?,
            doc: DocKey::new("file-1")?,
            base_version: 0,
            epoch: 0,
            kind: OpKind::Primitive,
            class: ConflictClass::Merge,
            target: Target::Annotation { page: PageRef { index: 0, epoch: 0 }, nm: "ann-1".into() },
            payload: json!({"action": "set_contents", "text": "hello"}),
            preview: None,
        };
        Ok(serde_json::to_value(ClientMessage::Op(op))?)
    }

    #[test]
    fn a_comment_reaches_every_connected_client() -> Result<(), Box<dyn std::error::Error>> {
        let handle = start()?;
        let mut editor = Client::join(handle.local_addr(), "editor", "a")?;
        let mut commenter = Client::join(handle.local_addr(), "commenter", "b")?;
        assert!(matches!(editor.next()?, ServerMessage::Welcome(_)));
        assert!(matches!(commenter.next()?, ServerMessage::Welcome(_)));

        editor.send(comment("c1")?)?;
        assert!(matches!(editor.next()?, ServerMessage::Applied(_)));
        assert!(matches!(commenter.next()?, ServerMessage::Applied(_)));
        Ok(())
    }

    #[test]
    fn a_bad_token_is_refused_before_joining() -> Result<(), Box<dyn std::error::Error>> {
        let handle = start()?;
        let mut client = Client::join(handle.local_addr(), "intruder", "evil")?;
        match client.next()? {
            ServerMessage::Error(e) => assert_eq!(e.code, ErrorCode::Auth),
            other => return Err(format!("expected an auth error, got {other:?}").into()),
        }
        Ok(())
    }

    #[test]
    fn a_viewer_is_rejected_by_the_room_not_the_transport() -> Result<(), Box<dyn std::error::Error>> {
        let handle = start()?;
        let mut viewer = Client::join(handle.local_addr(), "viewer", "v")?;
        let mut editor = Client::join(handle.local_addr(), "editor", "e")?;
        assert!(matches!(viewer.next()?, ServerMessage::Welcome(_)));
        assert!(matches!(editor.next()?, ServerMessage::Welcome(_)));

        viewer.send(comment("v1")?)?;
        match viewer.next()? {
            ServerMessage::Rejected(r) => assert_eq!(r.code, ErrorCode::Forbidden),
            other => return Err(format!("expected a forbidden rejection, got {other:?}").into()),
        }
        Ok(())
    }

    #[test]
    fn a_second_editor_joins_and_gets_a_zero_lease() -> Result<(), Box<dyn std::error::Error>> {
        let handle = start()?;
        // The lease goes to the first join the server processes, so the second
        // client is joined only after the first one was welcomed.
        let mut first = Client::join(handle.local_addr(), "editor:a", "a")?;
        let ServerMessage::Welcome(one) = first.next()? else {
            return Err("expected a welcome".into());
        };
        let mut second = Client::join(handle.local_addr(), "editor:b", "b")?;
        let ServerMessage::Welcome(two) = second.next()? else {
            return Err("expected a welcome".into());
        };
        assert!(one.lease.ttl_secs > 0);
        assert_eq!(two.lease.ttl_secs, 0);
        Ok(())
    }
}
