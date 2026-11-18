//! The optional document assistant: summaries of, and questions about, the open PDF, answered by a
//! language model the person already runs on this computer (Ollama, or any server that speaks the
//! OpenAI chat API).
//!
//! What it promises, and how:
//!
//! - **Nothing leaves the computer by default.** An endpoint whose host is not a loopback address
//!   is refused unless the caller passes `allow_external` (the person's explicit choice for this
//!   session; never stored). HTTP proxies from the environment and redirects are turned off, so a
//!   loopback request can't be carried elsewhere ([`provider`]).
//! - **No downloads.** The assistant only lists and uses models already installed; it never asks
//!   the server to pull one.
//! - **Every answer says where it comes from.** Each page's text is labelled as extracted text,
//!   an existing OCR layer, or text recognised now ([`pages`]); the model must back each point with
//!   quotations, and a quotation is shown only when it is found, word for word, in that page's text
//!   ([`verify`]). Unverifiable quotations are dropped and listed, never displayed as quotes.

#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

pub mod cli;
pub mod context;
pub(crate) mod local_http;
pub mod pages;
pub mod provider;
pub mod verify;

#[cfg(test)]
mod tests;

use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

pub use context::{Prompt, Task};
pub use pages::{DocText, Origin, PageSource};
pub use provider::{Endpoint, Kind, ModelInfo};
pub use verify::{Answer, Point, Quote, Rejected};

/// Tunables, with the values the app uses by default.
#[derive(Clone, Debug)]
pub struct Settings {
    /// Characters of document text sent with one request.
    pub budget_chars: usize,
    /// Context window asked of Ollama (tokens).
    pub num_ctx: u32,
    /// How long Ollama keeps the model loaded after the call.
    pub keep_alive: String,
    /// The longest a model may take to answer.
    pub request_timeout: Duration,
    /// The person chose, for this session, to let this endpoint receive the document text even
    /// though it is not on this computer.
    pub allow_external: bool,
    /// Answer summaries and questions in Spanish; false selects English.
    pub spanish: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            budget_chars: 36_000,
            num_ctx: 16_384,
            keep_alive: "2m".into(),
            request_timeout: Duration::from_secs(600),
            allow_external: false,
            spanish: true,
        }
    }
}

/// Where a running request is.
#[derive(Clone, Debug, PartialEq)]
pub enum Stage {
    /// Waiting for the shared inference lock (seconds so far).
    Queued(u64),
    /// The model is answering.
    Answering,
}

/// One finished request.
#[derive(Clone, Debug)]
pub struct Outcome {
    pub answer: Answer,
    pub prompt: Prompt,
    /// Seconds spent waiting for the lock, and answering.
    pub queue_seconds: f64,
    pub answer_seconds: f64,
    /// Whether the endpoint was on this computer.
    pub local: bool,
}

/// Run `task` over `doc` with `endpoint`. `cancel` stops a wait for the lock at once; a request
/// already sent is waited for (up to the timeout) and its answer discarded by the caller.
pub fn run(
    endpoint: &Endpoint,
    doc: &DocText,
    task: &Task,
    settings: &Settings,
    cancel: &AtomicBool,
    mut stage: impl FnMut(Stage),
) -> Result<Outcome, String> {
    let local = !endpoint.kind.is_cli() && provider::is_local(&endpoint.base_url)?;
    if !local && !settings.allow_external {
        return Err(format!(
            "{} no está en esta Mac. El documento no sale de la computadora salvo que usted elija expresamente este proveedor para la sesión.",
            if endpoint.kind.is_cli() {
                endpoint.kind.label().into()
            } else {
                provider::host_of(&endpoint.base_url).unwrap_or_else(|| endpoint.base_url.clone())
            }
        ));
    }
    let prompt = context::build(doc, task, settings.budget_chars, settings.spanish);
    if prompt.included.is_empty() {
        return Err(
            "Ninguna página tiene texto legible: no hay nada que enviar al modelo. Si el PDF está escaneado, reconozca el texto con OCR primero."
                .into(),
        );
    }
    let started = std::time::Instant::now();
    let queue_seconds = started.elapsed().as_secs_f64();
    if cancel.load(Ordering::Relaxed) {
        return Err("Cancelado.".into());
    }
    stage(Stage::Answering);
    let answered = std::time::Instant::now();
    let schema = if matches!(task, Task::Translate { .. }) { verify::translation_schema() } else { verify::schema() };
    let raw = provider::chat(endpoint, &prompt.system, &prompt.user, &schema, settings, cancel);
    let raw = raw?;
    let answer_seconds = answered.elapsed().as_secs_f64();
    log::info!("assistant: {} pages sent, {:.1}s queued, {:.1}s answering, local={local}", prompt.included.len(), queue_seconds, answer_seconds);
    let answer = match task {
        Task::Translate { .. } => verify::check_translation(&raw, &prompt.included),
        _ => verify::check(&raw, doc, &prompt.included),
    };
    Ok(Outcome { answer, prompt, queue_seconds, answer_seconds, local })
}
