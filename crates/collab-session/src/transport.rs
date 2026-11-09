//! What the session driver needs from a socket, and nothing else.
//!
//! There are two implementations: `native` (a blocking `tungstenite` socket on a
//! thread) and `web` (the browser's `WebSocket`, driven from the UI frame). The
//! driver in [`crate::driver`] is written against this trait alone, so the
//! protocol — hello/welcome, versions, resync, presence, commit — exists once
//! and both platforms run the same code.

use pdfcraft_collab_proto::{ClientMessage, ServerMessage};

/// What one `read` found. The driver never blocks: the transport decides how
/// long a read may wait (the native socket waits `poll_interval`, the browser's
/// returns at once and the frame loop calls again).
#[derive(Debug)]
pub enum ReadOutcome {
    /// A new connection just opened. The driver sends `hello` again, then goes on.
    Opened,
    /// One server message. Boxed: a `ServerMessage` is much larger than the other variants, and
    /// the queue holds many of them.
    Message(Box<ServerMessage>),
    /// A text frame that is not a protocol message. The driver reports it once.
    Unreadable,
    /// Nothing to read yet.
    Nothing,
    /// The connection ended. The driver reports it and the transport redials.
    Closed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum TransportError {
    /// The socket is still connecting (the browser's `WebSocket`): keep the
    /// frame for the next step instead of treating it as a lost connection.
    #[error("the collaboration socket is not open yet")]
    NotOpen,
    /// The socket is gone; the session is disconnected.
    #[error("the collaboration socket is closed")]
    Closed,
    /// The frame could not be serialised or sent.
    #[error("the collaboration socket refused the frame")]
    Frame,
}

/// One socket, as the session driver sees it.
pub trait Transport {
    /// Write one wire message. `NotOpen` means "try again": the driver keeps the
    /// message.
    fn write(&mut self, message: &ClientMessage) -> Result<(), TransportError>;

    /// One read that does not block longer than the transport's poll interval.
    fn read(&mut self) -> ReadOutcome;
}

/// Serialise one wire message. The token travels in `hello` only (ADR-0008).
pub(crate) fn encode(message: &ClientMessage) -> Result<String, TransportError> {
    serde_json::to_string(message).map_err(|_| TransportError::Frame)
}

/// Parse one text frame; `None` is an unreadable frame, not a lost connection.
pub(crate) fn decode(text: &str) -> Option<ServerMessage> {
    serde_json::from_str(text).ok()
}
