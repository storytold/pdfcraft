//! The browser transport: the page's `WebSocket`, driven from the UI frame.
//!
//! There are no threads on wasm32, so the socket's callbacks only push what
//! arrived into a queue and [`crate::driver::Session`] is stepped from
//! `try_event` once a frame. A close schedules a redial with the same idea as
//! the desktop worker (a fixed delay, doubling up to 10 s); the redial itself
//! happens on the next frame where the wait is over, so no timer and no
//! callback cycle is needed.
//!
//! The token is never part of the URL: it travels in the first `hello` frame,
//! exactly as on the desktop (ADR-0008).

use std::cell::RefCell;
use std::collections::VecDeque;
use std::rc::Rc;
use std::time::Duration;

use pdfcraft_collab_proto::{ClientMessage, Op, Presence};
use wasm_bindgen::JsCast as _;
use wasm_bindgen::closure::Closure;
use web_sys::{CloseEvent, Event as WebEvent, MessageEvent, WebSocket};
use web_time::Instant;

use crate::driver::Session;
use crate::transport::{ReadOutcome, Transport, TransportError, decode, encode};
use crate::{Command, Config, Event, SessionError, Sink};

/// The longest wait between redials.
const MAX_RETRY: Duration = Duration::from_secs(10);

/// What the socket's callbacks queued, and the socket they came from. The queue
/// itself lives on [`WebTransport`]: the callbacks own a clone of that `Rc`, and
/// `read` drains the same one (a second deque here left every frame unread — the
/// page dialled, the room said `welcome`, and the app waited forever).
struct Inner {
    url: String,
    base_delay: Duration,
    socket: Option<WebSocket>,
    /// Redials so far, for the backoff.
    attempt: u32,
    /// When the next redial may happen; `None` while the socket is up.
    due: Option<Instant>,
    closures: Closers,
    live: bool,
}

/// Kept so the JavaScript callbacks stay alive: dropping a `Closure` removes it.
#[allow(dead_code, reason = "the closures are kept alive by being stored; JavaScript calls them")]
#[derive(Default)]
struct Closers {
    open: Option<Closure<dyn FnMut(WebEvent)>>,
    message: Option<Closure<dyn FnMut(MessageEvent)>>,
    close: Option<Closure<dyn FnMut(CloseEvent)>>,
    error: Option<Closure<dyn FnMut(WebEvent)>>,
}

/// One browser socket.
pub struct WebTransport {
    inner: Rc<RefCell<Inner>>,
    /// The queue the callbacks fill; a clone of the one in `inner`, so a
    /// callback never holds a reference back to `Inner` (no cycle, no leak).
    queue: Rc<RefCell<VecDeque<ReadOutcome>>>,
}

impl WebTransport {
    /// Dial `url`. A failure here is not fatal: the caller's first `read`
    /// redials after the backoff.
    pub fn open(url: &str, base_delay: Duration) -> Result<Self, SessionError> {
        let queue = Rc::new(RefCell::new(VecDeque::new()));
        let inner = Rc::new(RefCell::new(Inner {
            url: url.to_string(),
            base_delay: base_delay.max(Duration::from_millis(50)),
            socket: None,
            attempt: 0,
            due: None,
            closures: Closers::default(),
            live: true,
        }));
        let transport = Self { inner, queue };
        // A browser refuses a bad URL by throwing here; the message names no
        // credential, so it can be shown.
        transport.dial()?;
        Ok(transport)
    }

    fn dial(&self) -> Result<(), SessionError> {
        let mut inner = self.inner.borrow_mut();
        if !inner.live {
            return Ok(());
        }
        if let Some(old) = inner.socket.take() {
            old.set_onopen(None);
            old.set_onmessage(None);
            old.set_onclose(None);
            old.set_onerror(None);
            let _ = old.close();
        }
        let socket = WebSocket::new(&inner.url).map_err(|error| SessionError::Web(format!("the collaboration socket did not open: {error:?}")))?;
        let push = {
            let queue = self.queue.clone();
            move |outcome: ReadOutcome| {
                queue.borrow_mut().push_back(outcome);
            }
        };
        let onopen = {
            let push = push.clone();
            Closure::<dyn FnMut(WebEvent)>::new(move |_| push(ReadOutcome::Opened))
        };
        let onmessage = {
            let push = push.clone();
            Closure::<dyn FnMut(MessageEvent)>::new(move |event: MessageEvent| {
                let outcome = match event.data().as_string() {
                    Some(text) => match decode(&text) {
                        Some(message) => ReadOutcome::Message(Box::new(message)),
                        None => ReadOutcome::Unreadable,
                    },
                    // The protocol is text; a binary frame is not one of its messages.
                    None => ReadOutcome::Unreadable,
                };
                push(outcome);
            })
        };
        let onclose = {
            let push = push.clone();
            Closure::<dyn FnMut(CloseEvent)>::new(move |_| {
                push(ReadOutcome::Closed);
            })
        };
        let onerror = {
            let push = push.clone();
            Closure::<dyn FnMut(WebEvent)>::new(move |_| {
                push(ReadOutcome::Closed);
            })
        };
        socket.set_onopen(Some(onopen.as_ref().unchecked_ref()));
        socket.set_onmessage(Some(onmessage.as_ref().unchecked_ref()));
        socket.set_onclose(Some(onclose.as_ref().unchecked_ref()));
        socket.set_onerror(Some(onerror.as_ref().unchecked_ref()));
        inner.closures = Closers { open: Some(onopen), message: Some(onmessage), close: Some(onclose), error: Some(onerror) };
        inner.socket = Some(socket);
        inner.due = None;
        Ok(())
    }

    fn is_open(&self) -> bool {
        self.inner.borrow().socket.as_ref().is_some_and(|socket| socket.ready_state() == WebSocket::OPEN)
    }
}

impl Transport for WebTransport {
    fn write(&mut self, message: &ClientMessage) -> Result<(), TransportError> {
        let json = encode(message)?;
        let inner = self.inner.borrow();
        let Some(socket) = inner.socket.as_ref() else { return Err(TransportError::NotOpen) };
        if socket.ready_state() != WebSocket::OPEN {
            return Err(TransportError::NotOpen);
        }
        socket.send_with_str(&json).map_err(|_| TransportError::Closed)
    }

    fn read(&mut self) -> ReadOutcome {
        loop {
            let outcome = self.queue.borrow_mut().pop_front();
            if let Some(outcome) = outcome {
                {
                    let mut inner = self.inner.borrow_mut();
                    match &outcome {
                        ReadOutcome::Opened => {
                            inner.attempt = 0;
                            inner.due = None;
                        }
                        ReadOutcome::Closed => {
                            // `onerror` and `onclose` both report a failure;
                            // the socket is only redialed once either way.
                            inner.socket = None;
                            inner.attempt = inner.attempt.saturating_add(1);
                            let due = Instant::now() + inner.delay();
                            inner.due = Some(due);
                        }
                        _ => {}
                    }
                }
                return outcome;
            }
            {
                let inner = self.inner.borrow();
                if inner.socket.is_some() {
                    return ReadOutcome::Nothing;
                }
                if let Some(due) = inner.due
                    && Instant::now() < due
                {
                    return ReadOutcome::Nothing;
                }
            }
            // No socket and the wait is over: dial again.
            if self.dial().is_err() {
                let mut inner = self.inner.borrow_mut();
                inner.attempt = inner.attempt.saturating_add(1);
                let due = Instant::now() + inner.delay();
                inner.due = Some(due);
                return ReadOutcome::Nothing;
            }
        }
    }
}

impl Inner {
    /// The wait before the next redial: the configured delay, doubling per
    /// failure, capped at [`MAX_RETRY`].
    fn delay(&self) -> Duration {
        let factor = 1u32.checked_shl(self.attempt.min(8)).unwrap_or(u32::MAX);
        self.base_delay.saturating_mul(factor).min(MAX_RETRY)
    }
}

impl Drop for WebTransport {
    fn drop(&mut self) {
        let mut inner = self.inner.borrow_mut();
        inner.live = false;
        if let Some(socket) = inner.socket.take() {
            socket.set_onopen(None);
            socket.set_onmessage(None);
            socket.set_onclose(None);
            socket.set_onerror(None);
            let _ = socket.close();
        }
    }
}

/// The web session: the shared driver over a browser socket, polled once a frame.
pub struct WebHandle {
    transport: WebTransport,
    session: Session,
    events: VecDeque<Event>,
}

impl WebHandle {
    pub fn open(config: Config) -> Result<Self, SessionError> {
        let transport = WebTransport::open(&config.url, config.reconnect_delay)?;
        let mut session = Session::new(config);
        session.opened();
        Ok(Self { transport, session, events: VecDeque::new() })
    }

    /// Drive the socket and the protocol forward by one read, and collect what
    /// the interface must apply.
    fn pump(&mut self) {
        let _ = self.session.step(&mut self.transport);
        self.events.extend(self.session.drain_events());
    }
}

impl Sink for WebHandle {
    fn enqueue(&mut self, op: Op) -> bool {
        self.session.command(Command::Op(op));
        self.session.write_pending(&mut self.transport);
        true
    }

    fn presence(&mut self, presence: Presence) -> bool {
        self.session.command(Command::Presence(presence));
        self.session.write_pending(&mut self.transport);
        true
    }

    fn commit(&mut self, message: Option<String>) -> bool {
        self.session.command(Command::Commit(message));
        self.session.write_pending(&mut self.transport);
        true
    }

    fn resync(&mut self) -> bool {
        self.session.command(Command::Resync);
        self.session.write_pending(&mut self.transport);
        true
    }

    fn try_event(&mut self) -> Option<Event> {
        if let Some(event) = self.events.pop_front() {
            return Some(event);
        }
        self.pump();
        self.events.pop_front()
    }

    fn close(&mut self) {
        // Dropping the transport closes the socket; nothing else to tell the page.
    }
}

/// Whether the browser socket is open (shown in the participants window).
impl WebHandle {
    pub fn connected(&self) -> bool {
        self.transport.is_open()
    }
}
