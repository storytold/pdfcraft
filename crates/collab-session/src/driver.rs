//! The protocol half of a session: what to send, what an answer means, what the
//! interface must apply. It knows nothing about sockets, threads or the browser;
//! [`crate::transport::Transport`] is the only thing it talks to.
//!
//! This is the code that used to live inside the socket loop in this crate's
//! `lib.rs`. Moving it behind the trait is what lets the web build run the same
//! hello/version/resync/presence/commit logic as the desktop instead of a second
//! implementation of the protocol.

use std::collections::VecDeque;

use pdfcraft_collab_client::{ClientSession, Effect};
use pdfcraft_collab_proto::{ClientMessage, ConflictClass, ErrorCode, Hello, Op, PROTOCOL_VERSION, Presence, ServerMessage, Welcome};
use web_time::Instant;

use crate::transport::{ReadOutcome, Transport};
use crate::{Command, Config, Event};

/// A heartbeat every 30 s renews the writing lease while the user reads (ADR-0008).
const PING_INTERVAL: std::time::Duration = std::time::Duration::from_secs(30);
/// Messages a single step may take, so a burst cannot starve the frame or the
/// command queue.
const MAX_FRAMES_PER_STEP: usize = 8;

/// What one step did to the connection.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Step {
    /// The socket is up (or still dialing); carry on.
    Continue,
    /// The connection ended. The native worker redials after a wait; the
    /// browser's transport redials on its own.
    Closed,
    /// The token or the session was refused: retrying cannot help.
    Gone,
}

/// One session, driven one message at a time.
pub struct Session {
    config: Config,
    client: Option<ClientSession>,
    /// Operations that arrived while the socket was down. A merge operation is
    /// re-addressed to the new head after the resync; a structural one is
    /// dropped, because page positions may have moved under it.
    retry: Vec<Op>,
    /// The socket is open.
    live: bool,
    /// The hello for this connection has gone out.
    greeted: bool,
    /// The token or the session was refused; retrying cannot help.
    refused: bool,
    /// Frames the socket has not taken yet (it is still dialing). Kept here so a
    /// presence sent while connecting is written once the socket opens instead of
    /// being dropped or read as a lost connection.
    outbox: VecDeque<ClientMessage>,
    events: Vec<Event>,
    last_ping: Instant,
    started: Instant,
}

impl Session {
    pub fn new(config: Config) -> Self {
        let now = Instant::now();
        Self {
            config,
            client: None,
            retry: Vec::new(),
            live: false,
            greeted: false,
            refused: false,
            outbox: VecDeque::new(),
            events: Vec::new(),
            last_ping: now,
            started: now,
        }
    }

    /// A connection opened: greet it again, keeping the version and the
    /// unconfirmed `opId`s so the server's de-duplication still recognises them.
    pub fn opened(&mut self) {
        self.live = true;
        self.greeted = false;
        self.last_ping = Instant::now();
    }

    /// Feed one command. A command that arrives while the socket is down waits
    /// in `retry` instead of failing the interface.
    pub fn command(&mut self, command: Command) {
        match command {
            Command::Op(op) => {
                if self.live {
                    self.enqueue(op);
                } else {
                    self.retry.push(op);
                }
            }
            Command::Presence(presence) => self.presence(presence),
            Command::Commit(message) => {
                if let Some(client) = self.client.as_mut()
                    && let Err(error) = client.commit(message)
                {
                    self.events.push(Event::Notice(error.to_string()));
                }
            }
            Command::Resync => {
                if let Some(client) = self.client.as_mut() {
                    client.resync();
                }
            }
            // The worker that owns the socket answers `Shutdown` itself.
            Command::Shutdown => {}
        }
    }

    /// Take what the interface must apply. The caller forwards these; nothing
    /// here touches the document.
    pub fn drain_events(&mut self) -> Vec<Event> {
        std::mem::take(&mut self.events)
    }

    /// One read (and everything that follows from it): greet an opened socket,
    /// apply the answer, renew the lease and write what is queued.
    pub fn step(&mut self, socket: &mut dyn Transport) -> Step {
        if self.refused {
            return Step::Gone;
        }
        if self.live && !self.greet(socket) {
            return self.lost();
        }
        let mut outcome = Step::Continue;
        for _ in 0..MAX_FRAMES_PER_STEP {
            match socket.read() {
                ReadOutcome::Opened => {
                    self.opened();
                    if !self.greet(socket) {
                        outcome = self.lost();
                        break;
                    }
                }
                ReadOutcome::Message(message) => {
                    outcome = self.message(*message);
                    if outcome != Step::Continue {
                        break;
                    }
                }
                ReadOutcome::Unreadable => self.events.push(Event::Notice("the server sent an unreadable frame".into())),
                ReadOutcome::Nothing => break,
                ReadOutcome::Closed => {
                    outcome = self.lost();
                    break;
                }
            }
        }
        if self.live {
            self.heartbeat();
            self.write_pending(socket);
        }
        outcome
    }

    /// Write what the client has queued (a presence sent this frame, a commit).
    /// The browser calls this after a command so a presence is not a frame late.
    /// A socket that is still dialing keeps the frames for the next step.
    pub fn write_pending(&mut self, socket: &mut dyn Transport) {
        loop {
            if self.outbox.is_empty() {
                let Some(client) = self.client.as_mut() else { return };
                self.outbox.extend(client.take_outgoing());
                if self.outbox.is_empty() {
                    return;
                }
            }
            let Some(message) = self.outbox.front().cloned() else { return };
            match socket.write(&message) {
                Ok(()) => {
                    self.outbox.pop_front();
                }
                Err(crate::transport::TransportError::NotOpen) => return,
                Err(_) => {
                    self.lost();
                    return;
                }
            }
        }
    }

    /// Send the hello for this connection once; `false` when the socket failed
    /// (the caller reports the disconnect). `NotOpen` is not a failure: the
    /// frame is kept for the next step.
    fn greet(&mut self, socket: &mut dyn Transport) -> bool {
        if self.greeted {
            return true;
        }
        let hello = match self.client.as_mut() {
            Some(client) => client.reconnect(self.config.jwt.clone(), self.config.client.clone()),
            None => self.first_hello(),
        };
        match socket.write(&ClientMessage::Hello(hello)) {
            Ok(()) => {
                self.greeted = true;
                true
            }
            Err(crate::transport::TransportError::NotOpen) => true,
            Err(_) => false,
        }
    }

    fn first_hello(&self) -> Hello {
        Hello {
            v: PROTOCOL_VERSION,
            app: self.config.app.clone(),
            doc: self.config.doc.clone(),
            jwt: self.config.jwt.clone(),
            client: self.config.client.clone(),
        }
    }

    fn enqueue(&mut self, op: Op) {
        let Some(client) = self.client.as_mut() else {
            self.retry.push(op);
            return;
        };
        if let Err(error) = client.enqueue(op) {
            self.events.push(Event::Notice(error.to_string()));
        }
    }

    fn presence(&mut self, presence: Presence) {
        let Some(client) = self.client.as_mut() else { return };
        if let Err(error) = client.presence(presence) {
            self.events.push(Event::Notice(error.to_string()));
        }
    }

    fn heartbeat(&mut self) {
        if self.last_ping.elapsed() < PING_INTERVAL {
            return;
        }
        let ts = u64::try_from(self.started.elapsed().as_millis()).unwrap_or(u64::MAX);
        if let Some(client) = self.client.as_mut()
            && client.ping(ts).is_ok()
        {
            self.last_ping = Instant::now();
        }
    }

    /// The connection is gone: report it once and let the caller redial.
    fn lost(&mut self) -> Step {
        if self.live {
            self.live = false;
            self.greeted = false;
            // Presence is refreshed every frame and a commit is an explicit
            // click; neither survives the connection that was carrying it.
            self.outbox.clear();
            self.events.push(Event::Disconnected);
        }
        Step::Closed
    }

    fn joined(&self) -> bool {
        self.client.as_ref().is_some_and(|client| client.is_joined())
    }

    fn message(&mut self, message: ServerMessage) -> Step {
        match message {
            ServerMessage::Welcome(welcome) => return self.welcome(welcome),
            // Before the welcome, an error is the server's answer to `hello`;
            // afterwards it belongs to the operation and is forwarded below.
            ServerMessage::Error(error) if !self.joined() => {
                self.events.push(Event::Error(error.clone()));
                if error.code == ErrorCode::Auth {
                    self.refused = true;
                    return Step::Gone;
                }
                return self.lost();
            }
            _ => {}
        }
        // A snapshot is the new base: unconfirmed merge operations are
        // re-addressed under it, with their `opId`, so the retry is not lost.
        let pending = matches!(message, ServerMessage::Snapshot(_)).then(|| self.client.as_ref().map(|client| client.pending_ops())).flatten();
        let Some(client) = self.client.as_mut() else { return Step::Continue };
        match client.receive(message) {
            Ok(Some(Effect::Snapshot(snapshot))) => {
                self.events.push(Event::Snapshot {
                    version: snapshot.version,
                    epoch: snapshot.epoch,
                    base: snapshot.base,
                    tail: tail_ops(&snapshot.tail),
                });
                let retry = std::mem::take(&mut self.retry);
                for op in pending.unwrap_or_default().into_iter().chain(retry).filter(|op| op.class == ConflictClass::Merge) {
                    self.enqueue(op);
                }
                Step::Continue
            }
            Ok(Some(effect)) => {
                if let Some(event) = event_of(effect) {
                    self.events.push(event);
                }
                Step::Continue
            }
            Ok(None) => Step::Continue,
            Err(error) => {
                self.events.push(Event::Notice(error.to_string()));
                Step::Continue
            }
        }
    }

    fn welcome(&mut self, welcome: Welcome) -> Step {
        let mut client = match self.client.take() {
            Some(client) => client,
            None => ClientSession::new(self.config.doc.clone(), welcome.actor_id.clone(), welcome.role),
        };
        let effect = client.receive(ServerMessage::Welcome(welcome));
        self.client = Some(client);
        match effect {
            Ok(Some(Effect::Joined { welcome, resync_requested })) => {
                self.events.push(Event::Joined { welcome, resync_requested });
                if !resync_requested {
                    // Nothing is missing: what waited for a socket can go now.
                    let retry = std::mem::take(&mut self.retry);
                    for op in retry {
                        self.enqueue(op);
                    }
                }
                Step::Continue
            }
            Ok(_) => Step::Continue,
            Err(error) => {
                self.events.push(Event::Notice(error.to_string()));
                self.lost()
            }
        }
    }
}

/// The journal tail of a snapshot as operations. An entry that does not parse
/// is dropped: the snapshot's base and version still describe the document.
fn tail_ops(tail: &[serde_json::Value]) -> Vec<Op> {
    tail.iter().filter_map(|value| serde_json::from_value(value.clone()).ok()).collect()
}

fn event_of(effect: Effect) -> Option<Event> {
    Some(match effect {
        Effect::Joined { welcome, resync_requested } => Event::Joined { welcome, resync_requested },
        Effect::Applied { applied, own } => Event::Applied { applied, own },
        Effect::Gap => Event::Gap,
        Effect::Retry { op_id } => Event::Retry { op_id },
        Effect::Rejected(rejected) => Event::Rejected(rejected),
        Effect::Snapshot(snapshot) => {
            Event::Snapshot { version: snapshot.version, epoch: snapshot.epoch, base: snapshot.base, tail: tail_ops(&snapshot.tail) }
        }
        Effect::Presence(presence) => Event::Presence(presence),
        Effect::Committed(result) => Event::Committed(result),
        Effect::Error(error) => Event::Error(error),
    })
}
