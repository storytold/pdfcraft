//! The sans-IO core of PdfCraft's AI assistant (ADR "AI assistant", M13).
//!
//! Nothing in this crate opens a connection, reads a file or a clock, so it builds and runs on
//! wasm and in tests unchanged.
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

pub mod text;
