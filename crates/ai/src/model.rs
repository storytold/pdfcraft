//! The models the assistant offers, their prices, and the cost of an answer.
//!
//! Prices are display-only estimates in nano-dollars per token, copied from Anthropic's model
//! overview on [`PRICES_AS_OF`]. The provider's bill is the authority; the panel says so.

use serde::{Deserialize, Serialize};

/// When the prices below were last checked.
pub const PRICES_AS_OF: &str = "2026-11-03";

/// Effort (`output_config.effort`): how much the model thinks before answering.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Effort {
    Low,
    #[default]
    Medium,
    High,
    Xhigh,
    Max,
}

impl Effort {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Low => "low",
            Self::Medium => "medium",
            Self::High => "high",
            Self::Xhigh => "xhigh",
            Self::Max => "max",
        }
    }
}

/// Prices in nano-dollars (10⁻⁹ USD) per token.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Price {
    pub input: u64,
    pub output: u64,
    pub cache_read: u64,
    /// A five-minute cache write (1.25 × input).
    pub cache_write: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Model {
    /// The API model id.
    pub id: &'static str,
    /// The name shown in Preferences.
    pub name: &'static str,
    pub price: Price,
    /// A higher price when the prompt passes [`Model::long_prompt_tokens`], if the model has one.
    pub long_prompt_price: Option<Price>,
    pub long_prompt_tokens: u64,
    /// Whether the server can re-run a refused request on another model (`fallbacks`).
    pub fallbacks: bool,
    pub max_output_tokens: u32,
}

/// Web search, per search: $10 per 1,000.
pub const WEB_SEARCH_NANODOLLARS: u64 = 10_000_000;

pub const OPUS: Model = Model {
    id: "claude-opus-5-5",
    name: "Claude Opus 5.5",
    price: Price { input: 4_000, output: 20_000, cache_read: 200, cache_write: 5_000 },
    long_prompt_price: None,
    long_prompt_tokens: 0,
    fallbacks: true,
    max_output_tokens: 128_000,
};

pub const SONNET: Model = Model {
    id: "claude-sonnet-5-5",
    name: "Claude Sonnet 5.5",
    price: Price { input: 2_000, output: 10_000, cache_read: 200, cache_write: 2_500 },
    long_prompt_price: None,
    long_prompt_tokens: 0,
    fallbacks: true,
    max_output_tokens: 128_000,
};

pub const HAIKU: Model = Model {
    id: "claude-haiku-5-5",
    name: "Claude Haiku 5.5",
    price: Price { input: 100, output: 500, cache_read: 10, cache_write: 125 },
    long_prompt_price: Some(Price { input: 500, output: 2_500, cache_read: 50, cache_write: 625 }),
    long_prompt_tokens: 100_000,
    // A `fallbacks` field is a 400 on Haiku 5.5.
    fallbacks: false,
    max_output_tokens: 128_000,
};

/// Every model offered, the default first.
pub const MODELS: &[Model] = &[OPUS, SONNET, HAIKU];

pub fn default_model() -> &'static Model {
    &OPUS
}

/// The model with this id, if PdfCraft knows it.
pub fn find(id: &str) -> Option<&'static Model> {
    MODELS.iter().find(|m| m.id == id)
}

/// Token counts from a response's `usage`. Counts in `message_delta` are cumulative, so each
/// update replaces the fields it carries.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Usage {
    /// Input tokens after the last cache breakpoint.
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cache_creation_input_tokens: u64,
    pub cache_read_input_tokens: u64,
    pub web_search_requests: u64,
}

impl Usage {
    /// Update from a `usage` object, keeping fields it doesn't carry.
    pub fn update(&mut self, usage: &serde_json::Value) {
        let read = |key: &str| usage.get(key).and_then(serde_json::Value::as_u64);
        if let Some(n) = read("input_tokens") {
            self.input_tokens = n;
        }
        if let Some(n) = read("output_tokens") {
            self.output_tokens = n;
        }
        if let Some(n) = read("cache_creation_input_tokens") {
            self.cache_creation_input_tokens = n;
        }
        if let Some(n) = read("cache_read_input_tokens") {
            self.cache_read_input_tokens = n;
        }
        if let Some(n) = usage.get("server_tool_use").and_then(|s| s.get("web_search_requests")).and_then(serde_json::Value::as_u64) {
            self.web_search_requests = n;
        }
    }

    /// Every input token the request was billed for, cached or not.
    pub fn prompt_tokens(&self) -> u64 {
        self.input_tokens.saturating_add(self.cache_creation_input_tokens).saturating_add(self.cache_read_input_tokens)
    }

    /// The sum of two requests (a conversation, or a request resent after `pause_turn`).
    pub fn plus(&self, other: &Usage) -> Usage {
        Usage {
            input_tokens: self.input_tokens.saturating_add(other.input_tokens),
            output_tokens: self.output_tokens.saturating_add(other.output_tokens),
            cache_creation_input_tokens: self.cache_creation_input_tokens.saturating_add(other.cache_creation_input_tokens),
            cache_read_input_tokens: self.cache_read_input_tokens.saturating_add(other.cache_read_input_tokens),
            web_search_requests: self.web_search_requests.saturating_add(other.web_search_requests),
        }
    }
}

impl Model {
    /// The estimated cost of one request, in nano-dollars. Saturates instead of overflowing.
    pub fn cost_nanodollars(&self, usage: &Usage) -> u64 {
        let price = match self.long_prompt_price {
            Some(long) if usage.prompt_tokens() > self.long_prompt_tokens => long,
            _ => self.price,
        };
        let parts = [
            usage.input_tokens.saturating_mul(price.input),
            usage.output_tokens.saturating_mul(price.output),
            usage.cache_read_input_tokens.saturating_mul(price.cache_read),
            usage.cache_creation_input_tokens.saturating_mul(price.cache_write),
            usage.web_search_requests.saturating_mul(WEB_SEARCH_NANODOLLARS),
        ];
        parts.iter().fold(0u64, |sum, p| sum.saturating_add(*p))
    }
}

/// A cost for display: `$0.0123`, with four decimals (a tenth of a cent).
pub fn format_dollars(nanodollars: u64) -> String {
    // Round to the nearest 1/10,000 of a dollar (100,000 nano-dollars).
    let units = nanodollars.saturating_add(50_000) / 100_000;
    format!("${}.{:04}", units / 10_000, units % 10_000)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn costs() {
        let usage = Usage { input_tokens: 1_000_000, output_tokens: 1_000_000, ..Default::default() };
        assert_eq!(format_dollars(OPUS.cost_nanodollars(&usage)), "$24.0000");
        assert_eq!(format_dollars(SONNET.cost_nanodollars(&usage)), "$12.0000");
        // Haiku 5.5 is billed at its higher rate once the prompt passes 100K tokens.
        assert_eq!(format_dollars(HAIKU.cost_nanodollars(&Usage { input_tokens: 100_000, output_tokens: 1_000, ..Default::default() })), "$0.0105");
        assert_eq!(format_dollars(HAIKU.cost_nanodollars(&Usage { input_tokens: 100_001, output_tokens: 1_000, ..Default::default() })), "$0.0525");
        let cached =
            Usage { cache_read_input_tokens: 1_000_000, cache_creation_input_tokens: 1_000_000, web_search_requests: 3, ..Default::default() };
        assert_eq!(format_dollars(OPUS.cost_nanodollars(&cached)), "$5.2300");
        let huge = Usage { input_tokens: u64::MAX, output_tokens: u64::MAX, ..Default::default() };
        assert_eq!(OPUS.cost_nanodollars(&huge), u64::MAX);
        assert_eq!(format_dollars(0), "$0.0000");
        assert_eq!(format_dollars(u64::MAX), "$18446744073.7095");
    }

    #[test]
    fn usage_updates_are_cumulative_replacements() {
        let mut u = Usage::default();
        u.update(&serde_json::json!({"input_tokens": 25, "output_tokens": 1, "cache_read_input_tokens": 1800}));
        u.update(&serde_json::json!({"output_tokens": 15, "server_tool_use": {"web_search_requests": 2}}));
        u.update(&serde_json::json!({"output_tokens": -3, "input_tokens": "x"}));
        assert_eq!(
            u,
            Usage { input_tokens: 25, output_tokens: 15, cache_creation_input_tokens: 0, cache_read_input_tokens: 1800, web_search_requests: 2 }
        );
    }

    #[test]
    fn model_table() {
        assert_eq!(default_model().id, "claude-opus-5-5");
        assert_eq!(find("claude-haiku-5-5").map(|m| m.fallbacks), Some(false));
        assert!(find("gpt-4").is_none());
    }
}
