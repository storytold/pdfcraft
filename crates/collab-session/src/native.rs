//! The desktop transport: one blocking WebSocket on its own thread.
//!
//! The thread owns the socket and the [`Session`] that drives it; the interface
//! thread talks to it through a command channel and reads the events back. On
//! wasm there are no threads, and [`crate::web`] drives the same session from
//! the frame loop instead.
//!
//! No document, token or comment text is logged.

use std::io;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, RecvTimeoutError, Sender, TryRecvError, channel};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use pdfcraft_collab_proto::{ClientMessage, Op, Presence};
use tungstenite::stream::MaybeTlsStream;
use tungstenite::{Error as WsError, Message, WebSocket};

use crate::driver::{Session, Step};
use crate::transport::{ReadOutcome, Transport, TransportError, decode, encode};
use crate::{Command, Config, Event, Sink};

/// One worker thread. Dropping it stops the worker.
pub struct Handle {
    commands: Sender<Command>,
    events: Receiver<Event>,
    closed: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl Handle {
    pub fn close(&mut self) {
        self.closed.store(true, Ordering::Relaxed);
        let _ = self.commands.send(Command::Shutdown);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

impl Sink for Handle {
    fn enqueue(&mut self, op: Op) -> bool {
        self.commands.send(Command::Op(op)).is_ok()
    }

    fn presence(&mut self, presence: Presence) -> bool {
        self.commands.send(Command::Presence(presence)).is_ok()
    }

    fn commit(&mut self, message: Option<String>) -> bool {
        self.commands.send(Command::Commit(message)).is_ok()
    }

    fn resync(&mut self) -> bool {
        self.commands.send(Command::Resync).is_ok()
    }

    /// The next event, if one is waiting. The interface polls this once a frame.
    fn try_event(&mut self) -> Option<Event> {
        self.events.try_recv().ok()
    }

    fn close(&mut self) {
        Handle::close(self);
    }
}

impl Drop for Handle {
    fn drop(&mut self) {
        self.close();
    }
}

/// Start the worker for one session.
pub fn spawn(config: Config) -> io::Result<Handle> {
    let (command_tx, command_rx) = channel();
    let (event_tx, event_rx) = channel();
    let closed = Arc::new(AtomicBool::new(false));
    let flag = closed.clone();
    let thread = thread::Builder::new().name("pdfcraft-collab".into()).spawn(move || run(config, command_rx, event_tx, flag))?;
    Ok(Handle { commands: command_tx, events: event_rx, closed, thread: Some(thread) })
}

/// The blocking socket, as the session driver sees it.
struct Socket {
    ws: WebSocket<MaybeTlsStream<std::net::TcpStream>>,
}

impl Transport for Socket {
    fn write(&mut self, message: &ClientMessage) -> Result<(), TransportError> {
        let json = encode(message)?;
        self.ws.send(Message::text(json)).map_err(|_| TransportError::Closed)
    }

    fn read(&mut self) -> ReadOutcome {
        match self.ws.read() {
            Ok(Message::Text(text)) => match decode(text.as_str()) {
                Some(message) => ReadOutcome::Message(Box::new(message)),
                None => ReadOutcome::Unreadable,
            },
            Ok(Message::Close(_)) => ReadOutcome::Closed,
            Ok(_) => ReadOutcome::Nothing,
            // The read timeout is the poll interval: nothing arrived, not a failure.
            Err(WsError::Io(error)) if matches!(error.kind(), io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut) => ReadOutcome::Nothing,
            Err(_) => ReadOutcome::Closed,
        }
    }
}

fn run(config: Config, commands: Receiver<Command>, events: Sender<Event>, closed: Arc<AtomicBool>) {
    let mut session = Session::new(config.clone());
    while !closed.load(Ordering::Relaxed) {
        let Ok(mut socket) = connect(&config) else {
            let _ = events.send(Event::Disconnected);
            if !wait_before_retry(&config, &commands, &closed, &mut session) {
                return;
            }
            continue;
        };
        session.opened();
        loop {
            match drain_commands(&commands, &mut session) {
                Drained::More => {}
                Drained::Shutdown | Drained::Gone => {
                    let _ = socket.ws.close(None);
                    return;
                }
            }
            if closed.load(Ordering::Relaxed) {
                let _ = socket.ws.close(None);
                return;
            }
            let step = session.step(&mut socket);
            for event in session.drain_events() {
                // The interface is gone (its receiver dropped): stop the worker.
                if events.send(event).is_err() {
                    return;
                }
            }
            match step {
                Step::Continue => {}
                Step::Closed => break,
                Step::Gone => {
                    let _ = socket.ws.close(None);
                    return;
                }
            }
        }
        if !wait_before_retry(&config, &commands, &closed, &mut session) {
            return;
        }
    }
}

enum Drained {
    More,
    Shutdown,
    /// The interface is gone.
    Gone,
}

fn drain_commands(commands: &Receiver<Command>, session: &mut Session) -> Drained {
    loop {
        match commands.try_recv() {
            Ok(Command::Shutdown) => return Drained::Shutdown,
            Ok(command) => session.command(command),
            Err(TryRecvError::Empty) => return Drained::More,
            Err(TryRecvError::Disconnected) => return Drained::Gone,
        }
    }
}

fn connect(config: &Config) -> Result<Socket, ()> {
    let (mut ws, _response) = tungstenite::connect(config.url.as_str()).map_err(|_| ())?;
    if let MaybeTlsStream::Plain(stream) = ws.get_mut() {
        let _ = stream.set_read_timeout(Some(config.poll_interval));
    }
    Ok(Socket { ws })
}

/// Sleep before reconnecting, keeping the caller's operations and letting a
/// shutdown in.
fn wait_before_retry(config: &Config, commands: &Receiver<Command>, closed: &AtomicBool, session: &mut Session) -> bool {
    let deadline = Instant::now() + config.reconnect_delay;
    while Instant::now() < deadline {
        if closed.load(Ordering::Relaxed) {
            return false;
        }
        let left = deadline.saturating_duration_since(Instant::now()).min(Duration::from_millis(100));
        match commands.recv_timeout(left) {
            Ok(Command::Shutdown) => return false,
            Ok(command) => session.command(command),
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => return false,
        }
    }
    true
}
