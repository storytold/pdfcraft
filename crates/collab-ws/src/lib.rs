//! WebSocket transport for the PdfCraft collaboration hub.
//!
//! The crate keeps the protocol concerns out of the room: it turns JSON frames
//! into [`pdfcraft_collab_hub::Hub`] calls, verifies the session JWT carried by
//! `hello`, and pumps delivered messages back to each connection. The room stays
//! transport-free, so the same rules are tested with three clients in memory.
//!
//! Blocking I/O and one thread per connection are deliberate: a room has 2–5
//! participants and the engine is synchronous. The web client keeps its own
//! socket through the host bridge.

#![cfg(not(target_arch = "wasm32"))]
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

pub mod serve;
pub mod token;

pub use serve::{Config, Metrics, ServerHandle, TransportError, WsServer};
pub use token::{Claims, JwtVerifier, TokenError, Verifier};
