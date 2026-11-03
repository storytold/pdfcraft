//! What a decoded answer stream says, provider-neutral, in the order it arrived.

use serde::Serialize;

use crate::model::Usage;
use crate::source::Source;

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(tag = "event", rename_all = "snake_case")]
pub enum Event {
    Started {
        id: String,
        model: String,
    },
    /// More text for answer block `block` (the stream's content block index).
    Text {
        block: u64,
        text: String,
    },
    /// A citation supporting the text of answer block `block`.
    Citation {
        block: u64,
        source: Source,
    },
    /// The provider began a web search.
    WebSearch {
        query: String,
    },
    /// A web search finished: the pages found, or the provider's error code.
    WebResults {
        results: Vec<WebResult>,
        error: Option<String>,
    },
    /// Token counts so far (cumulative).
    Usage {
        usage: Usage,
    },
    /// The answer ended.
    Stop {
        reason: StopReason,
    },
    /// The provider reported an error (an HTTP error body, or an `error` event mid-stream).
    ProviderError {
        kind: String,
        message: String,
    },
    /// Something in the stream was malformed or over a cap and was skipped.
    Skipped {
        what: String,
    },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct WebResult {
    pub url: String,
    pub title: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum StopReason {
    /// The answer is complete.
    EndTurn,
    /// The answer hit `max_tokens` and is cut off.
    MaxTokens,
    /// The model declined to answer.
    Refusal,
    /// A server tool (web search) is still running: send the assistant message back to continue.
    PauseTurn,
    /// The stream ended before the provider said it was done.
    Truncated,
    /// The answer passed this crate's caps and the rest was dropped.
    TooLong,
    Other(String),
}

impl StopReason {
    pub fn parse(s: &str) -> Self {
        match s {
            "end_turn" | "stop_sequence" => Self::EndTurn,
            "max_tokens" => Self::MaxTokens,
            "refusal" => Self::Refusal,
            "pause_turn" => Self::PauseTurn,
            other => Self::Other(crate::text::capped(other, 64)),
        }
    }
}
