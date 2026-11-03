//! The sans-IO core of PdfCraft's AI assistant (ADR "AI assistant", M13).
//!
//! - [`claude`] builds Claude Messages API requests and decodes their streamed answers into
//!   [`event::Event`]s, assembling the assistant message so a conversation can continue.
//! - [`source`] names what an answer can cite: this document, a library document or a web page.
//! - [`library`] is the personal PDF library: passages, a BM25 index and its saved form.
//! - [`model`] lists the models with their prices, and turns [`model::Usage`] into a cost.
//! - [`transport`] is the seam the app fills with a network client. Nothing in this crate opens a
//!   connection, reads a file or a clock, so it builds and runs on wasm and in tests unchanged.
//!
//! Every byte from a provider and every saved file is untrusted: decoders cap what they keep and
//! turn anything malformed into an event or an error, never a panic.
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

pub mod claude;
pub mod event;
pub mod library;
pub mod model;
pub mod source;
pub mod sse;
pub mod text;
pub mod transport;
