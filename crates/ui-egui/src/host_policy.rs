//! A web host's policy: the `policy.json` file a site that serves PdfCraft's web build puts
//! beside it, the browser's counterpart of an administrator's policy on a desktop. It is a JSON
//! object of the same keys, such as `{"ForceSecureRedaction": true}`.
//!
//! A missing file (HTTP 404) means no policy. A file that is there but can't be read (another
//! HTTP error, a network or body failure, damaged JSON, a value of the wrong type, more than
//! [`MAX_BYTES`]) counts as the most restrictive setting, and says why: a host who put a policy
//! there meant to manage the setting, and silently falling back would leave a feature on where
//! they believe it is off.

/// The file, relative to the page.
pub const FILE: &str = "policy.json";
/// Requires secure redacted copies instead of redaction in place (a flag: `true`/`false`, or
/// `1`/`0`).
pub const FORCE_SECURE_REDACTION: &str = "ForceSecureRedaction";
/// A policy larger than this is refused (it is a handful of flags).
pub const MAX_BYTES: usize = 64 * 1024;

/// How the fetch of [`FILE`] ended.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Fetched {
    /// An HTTP response: its status and, for a success, its body (or why it couldn't be read).
    Response { status: u16, body: Result<Vec<u8>, String> },
    /// No response at all (offline, blocked, …).
    Failed(String),
}

/// Who requires secure redacted copies, for [`pdfcraft_engine::Session::require_secure_redaction`]:
/// `None` when the policy is absent or turns it off.
pub fn secure_redaction(fetched: &Fetched) -> Option<String> {
    let unreadable = |why: String| Some(format!("this site's {FILE} (it can't be read: {why})"));
    let body = match fetched {
        Fetched::Response { status: 404, .. } => return None,
        Fetched::Response { status: 200..=299, body: Ok(body) } => body,
        Fetched::Response { status: 200..=299, body: Err(why) } => return unreadable(why.clone()),
        Fetched::Response { status, .. } => return unreadable(format!("HTTP {status}")),
        Fetched::Failed(why) => return unreadable(why.clone()),
    };
    if body.len() > MAX_BYTES {
        return unreadable(format!("it is larger than {} KiB", MAX_BYTES / 1024));
    }
    let json: serde_json::Value = match serde_json::from_slice(body) {
        Ok(v) => v,
        Err(e) => return unreadable(format!("not JSON: {e}")),
    };
    let Some(object) = json.as_object() else { return unreadable("not a JSON object".into()) };
    match object.get(FORCE_SECURE_REDACTION) {
        None | Some(serde_json::Value::Bool(false)) => None,
        Some(serde_json::Value::Bool(true)) => Some(format!("this site's {FILE}")),
        Some(serde_json::Value::Number(n)) if n.as_i64() == Some(0) => None,
        Some(serde_json::Value::Number(n)) if n.as_i64() == Some(1) => Some(format!("this site's {FILE}")),
        Some(other) => unreadable(format!("{FORCE_SECURE_REDACTION} must be true or false, not {other}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ok(body: &str) -> Fetched {
        Fetched::Response { status: 200, body: Ok(body.as_bytes().to_vec()) }
    }

    #[test]
    fn a_policy_is_on_off_absent_or_unreadable() {
        assert_eq!(secure_redaction(&Fetched::Response { status: 404, body: Ok(Vec::new()) }), None);
        assert_eq!(secure_redaction(&ok(r#"{"ForceSecureRedaction": true}"#)).as_deref(), Some("this site's policy.json"));
        assert!(secure_redaction(&ok(r#"{"ForceSecureRedaction": 1, "DisableJavaScript": true}"#)).is_some());
        assert_eq!(secure_redaction(&ok(r#"{"ForceSecureRedaction": false}"#)), None);
        assert_eq!(secure_redaction(&ok(r#"{"ForceSecureRedaction": 0}"#)), None);
        assert_eq!(secure_redaction(&ok(r#"{"DisableJavaScript": true}"#)), None);
        // Anything that is there but can't be read keeps secure copies required, and says why.
        for (fetched, why) in [
            (ok(r#"{"ForceSecureRedaction": "yes"}"#), "must be true or false"),
            (ok(r#"{"ForceSecureRedaction": 2}"#), "must be true or false"),
            (ok("{not json"), "not JSON"),
            (ok("[true]"), "not a JSON object"),
            (ok(&" ".repeat(MAX_BYTES + 1)), "larger than"),
            (Fetched::Response { status: 500, body: Ok(Vec::new()) }, "HTTP 500"),
            (Fetched::Response { status: 403, body: Ok(Vec::new()) }, "HTTP 403"),
            (Fetched::Response { status: 200, body: Err("the body stopped".into()) }, "the body stopped"),
            (Fetched::Failed("offline".into()), "offline"),
        ] {
            let got = secure_redaction(&fetched).unwrap_or_default();
            assert!(got.contains("can't be read") && got.contains(why), "{fetched:?}: {got}");
        }
    }
}
