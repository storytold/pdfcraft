//! Administrators' policies: settings an organisation sets for everyone on a machine, which
//! people can't change in Preferences. They are read once, at launch, from where each system's
//! management tools write them:
//!
//! | System | Where |
//! |---|---|
//! | Windows | `HKLM\SOFTWARE\Policies\PdfCraft`, then `HKCU\SOFTWARE\Policies\PdfCraft` (Group Policy, Intune, an RMM's `reg add`); flags are `REG_DWORD` 0 or 1 |
//! | macOS | managed preferences for `ai.storyteller.pdfcraft` (a configuration profile from an MDM); flags are booleans |
//! | Linux, BSD | `/etc/pdfcraft/policy.json` (`/usr/local/etc/pdfcraft/policy.json` on FreeBSD), a JSON object such as `{"DisableJavaScript": true}` |
//!
//! Only administrators can write any of these (`HKCU\SOFTWARE\Policies` included), and a value a
//! user sets in their own preferences domain on macOS isn't "forced", so it's ignored.
//!
//! With no policy, nothing changes. A policy that is there but can't be read (the wrong type,
//! an unknown value, a damaged file, no permission to read it) counts as the most restrictive
//! setting, and says why: an administrator who put something there meant to manage the
//! setting, and a policy that silently fell back to the default would leave a feature on
//! where they believe it is off, with nothing to show it.

/// Turns JavaScript in documents off, and keeps it off (Acrobat's `bDisableJavaScript`).
pub const DISABLE_JAVASCRIPT: &str = "DisableJavaScript";

/// The macOS preferences domain (the app's bundle identifier).
pub const MACOS_DOMAIN: &str = "ai.storyteller.pdfcraft";

/// The Windows policy key, under both `HKLM` and `HKCU`.
pub const WINDOWS_KEY: &str = r"SOFTWARE\Policies\PdfCraft";

/// The policy file on Linux and the BSDs.
pub const POLICY_FILE: &str = if cfg!(target_os = "freebsd") { "/usr/local/etc/pdfcraft/policy.json" } else { "/etc/pdfcraft/policy.json" };

/// A policy file larger than this is refused (it is a handful of flags).
pub const MAX_POLICY_FILE_BYTES: u64 = 64 * 1024;

/// What an administrator decided for one setting.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum Managed<T> {
    /// No policy: the person using the app decides.
    #[default]
    NotSet,
    /// Set by the policy at `source` (e.g. `HKLM\SOFTWARE\Policies\PdfCraft\DisableJavaScript`).
    Set { value: T, source: String },
    /// A policy is at `source` but couldn't be read; the setting is treated as its most
    /// restrictive value.
    Unreadable { source: String, reason: String },
}

/// Every policy PdfCraft knows.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Policy {
    /// [`DISABLE_JAVASCRIPT`].
    pub disable_javascript: Managed<bool>,
}

impl Policy {
    /// The machine's policies. Never fails: what can't be read is [`Managed::Unreadable`].
    pub fn load() -> Policy {
        #[cfg(windows)]
        {
            windows::load()
        }
        #[cfg(target_os = "macos")]
        {
            macos::load()
        }
        #[cfg(any(target_os = "linux", target_os = "freebsd", target_os = "openbsd", target_os = "netbsd", target_os = "dragonfly"))]
        {
            from_file(std::path::Path::new(POLICY_FILE))
        }
        #[cfg(not(any(
            windows,
            target_os = "macos",
            target_os = "linux",
            target_os = "freebsd",
            target_os = "openbsd",
            target_os = "netbsd",
            target_os = "dragonfly"
        )))]
        {
            Policy::default()
        }
    }

    /// When a policy keeps JavaScript off: where it is, and what was wrong with it if it
    /// couldn't be read.
    pub fn javascript_lock(&self) -> Option<(String, Option<String>)> {
        match &self.disable_javascript {
            Managed::Set { value: true, source } => Some((source.clone(), None)),
            Managed::Unreadable { source, reason } => Some((source.clone(), Some(reason.clone()))),
            Managed::Set { value: false, .. } | Managed::NotSet => None,
        }
    }
}

/// A value as a policy store holds it, before it is checked.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Raw {
    Missing,
    Bool(bool),
    Integer(i64),
    /// Anything else, described for the message ("REG_SZ", "a string", …).
    Other(String),
    /// The store couldn't be read.
    Error(String),
}

/// An on/off policy: a boolean, or the number 0 or 1.
pub fn flag(raw: Raw, source: String) -> Managed<bool> {
    match raw {
        Raw::Missing => Managed::NotSet,
        Raw::Bool(value) => Managed::Set { value, source },
        Raw::Integer(0) => Managed::Set { value: false, source },
        Raw::Integer(1) => Managed::Set { value: true, source },
        Raw::Integer(n) => Managed::Unreadable { source, reason: format!("expected 0 or 1, found {n}") },
        Raw::Other(what) => Managed::Unreadable { source, reason: format!("expected 0 or 1, found {what}") },
        Raw::Error(reason) => Managed::Unreadable { source, reason },
    }
}

/// Two policies for the same "disable" flag (machine and user): the one that restricts more
/// wins, and the first one on a tie.
pub fn stricter(a: Managed<bool>, b: Managed<bool>) -> Managed<bool> {
    fn rank(m: &Managed<bool>) -> u8 {
        match m {
            Managed::NotSet => 0,
            Managed::Set { value: false, .. } => 1,
            Managed::Set { value: true, .. } => 2,
            Managed::Unreadable { .. } => 3,
        }
    }
    if rank(&b) > rank(&a) { b } else { a }
}

/// The policies in a policy file's text. Unknown keys are ignored, so a newer policy file
/// works with an older PdfCraft; a file that isn't a JSON object makes every policy
/// unreadable, since there is no telling what it meant to set.
pub fn from_json(text: &[u8], source: &str) -> Policy {
    let source_of = |key: &str| format!("{source} ({key})");
    // Editors on Windows may start the file with a byte-order mark.
    let text = text.strip_prefix(b"\xEF\xBB\xBF").unwrap_or(text);
    let object = match serde_json::from_slice::<serde_json::Value>(text) {
        Ok(serde_json::Value::Object(o)) => o,
        Ok(_) => return unreadable(source_of, "it isn't a JSON object"),
        Err(e) => return unreadable(source_of, &format!("it isn't valid JSON ({e})")),
    };
    let raw = |key: &str| match object.get(key) {
        None => Raw::Missing,
        Some(serde_json::Value::Bool(b)) => Raw::Bool(*b),
        Some(serde_json::Value::Number(n)) => n.as_i64().map(Raw::Integer).unwrap_or_else(|| Raw::Other(format!("the number {n}"))),
        Some(serde_json::Value::String(_)) => Raw::Other("a string".into()),
        Some(serde_json::Value::Null) => Raw::Other("null".into()),
        Some(serde_json::Value::Array(_)) => Raw::Other("a list".into()),
        Some(serde_json::Value::Object(_)) => Raw::Other("an object".into()),
    };
    Policy { disable_javascript: flag(raw(DISABLE_JAVASCRIPT), source_of(DISABLE_JAVASCRIPT)) }
}

/// Every policy unreadable, for the same reason.
fn unreadable(source_of: impl Fn(&str) -> String, reason: &str) -> Policy {
    Policy { disable_javascript: Managed::Unreadable { source: source_of(DISABLE_JAVASCRIPT), reason: reason.to_string() } }
}

/// The policies in the file at `path`; no file means no policies.
pub fn from_file(path: &std::path::Path) -> Policy {
    use std::io::Read;
    let source = path.display().to_string();
    let source_of = |key: &str| format!("{source} ({key})");
    let file = match std::fs::File::open(path) {
        Ok(f) => f,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Policy::default(),
        Err(e) => return unreadable(source_of, &format!("it couldn't be opened ({e})")),
    };
    let mut text = Vec::new();
    // One byte past the cap tells a file at the cap from one over it.
    match file.take(MAX_POLICY_FILE_BYTES.saturating_add(1)).read_to_end(&mut text) {
        Ok(n) if u64::try_from(n).unwrap_or(u64::MAX) > MAX_POLICY_FILE_BYTES => {
            unreadable(source_of, &format!("it is larger than {} KB", MAX_POLICY_FILE_BYTES / 1024))
        }
        Ok(_) => from_json(&text, &source),
        Err(e) => unreadable(source_of, &format!("it couldn't be read ({e})")),
    }
}

#[cfg(windows)]
mod windows {
    use winsafe::{HKEY, RegistryValue, co};

    use super::{DISABLE_JAVASCRIPT, Policy, Raw, WINDOWS_KEY, flag, stricter};

    pub(super) fn load() -> Policy {
        let machine = flag(value(&HKEY::LOCAL_MACHINE, WINDOWS_KEY, DISABLE_JAVASCRIPT), source("HKLM", DISABLE_JAVASCRIPT));
        let user = flag(value(&HKEY::CURRENT_USER, WINDOWS_KEY, DISABLE_JAVASCRIPT), source("HKCU", DISABLE_JAVASCRIPT));
        Policy { disable_javascript: stricter(machine, user) }
    }

    fn source(root: &str, name: &str) -> String {
        format!(r"{root}\{WINDOWS_KEY}\{name}")
    }

    /// Value `name` of `root\key`; a missing key or value is [`Raw::Missing`].
    pub(super) fn value(root: &HKEY, key: &str, name: &str) -> Raw {
        match root.RegGetValue(Some(key), Some(name), co::RRF::RT_ANY) {
            Ok(RegistryValue::Dword(n)) => Raw::Integer(i64::from(n)),
            Ok(RegistryValue::Qword(n)) => i64::try_from(n).map(Raw::Integer).unwrap_or_else(|_| Raw::Other(format!("the REG_QWORD {n}"))),
            Ok(RegistryValue::Sz(s) | RegistryValue::ExpandSz(s)) => {
                Raw::Other(format!("the REG_SZ \"{}\" (use a REG_DWORD)", s.chars().take(40).collect::<String>()))
            }
            Ok(RegistryValue::MultiSz(_)) => Raw::Other("a REG_MULTI_SZ (use a REG_DWORD)".into()),
            Ok(RegistryValue::Binary(_)) => Raw::Other("a REG_BINARY (use a REG_DWORD)".into()),
            Ok(RegistryValue::None) => Raw::Other("a value with no type (use a REG_DWORD)".into()),
            Err(e) if e == co::ERROR::FILE_NOT_FOUND || e == co::ERROR::PATH_NOT_FOUND => Raw::Missing,
            Err(e) => Raw::Error(format!("it couldn't be read ({e})")),
        }
    }
}

#[cfg(target_os = "macos")]
mod macos {
    use objc2_core_foundation::{CFBoolean, CFNumber, CFPreferencesAppValueIsForced, CFPreferencesCopyAppValue, CFString};

    use super::{DISABLE_JAVASCRIPT, MACOS_DOMAIN, Policy, Raw, flag};

    pub(super) fn load() -> Policy {
        let source = format!("managed preferences for {MACOS_DOMAIN} ({DISABLE_JAVASCRIPT})");
        Policy { disable_javascript: flag(value(DISABLE_JAVASCRIPT), source) }
    }

    /// A key's value, but only when an administrator forced it (a configuration profile):
    /// what a person writes with `defaults write` isn't a policy.
    fn value(key: &'static str) -> Raw {
        let domain = CFString::from_static_str(MACOS_DOMAIN);
        let key = CFString::from_static_str(key);
        if !CFPreferencesAppValueIsForced(&key, &domain) {
            return Raw::Missing;
        }
        let Some(v) = CFPreferencesCopyAppValue(&key, &domain) else { return Raw::Missing };
        if let Some(b) = v.downcast_ref::<CFBoolean>() {
            Raw::Bool(b.as_bool())
        } else if let Some(n) = v.downcast_ref::<CFNumber>() {
            n.as_i64().map(Raw::Integer).unwrap_or_else(|| Raw::Other("a number that isn't 0 or 1".into()))
        } else {
            Raw::Other("a value that isn't a boolean or a number".into())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn source() -> String {
        "policy (DisableJavaScript)".into()
    }

    #[test]
    fn a_flag_is_a_boolean_or_zero_or_one() {
        assert_eq!(flag(Raw::Missing, source()), Managed::NotSet);
        assert_eq!(flag(Raw::Bool(true), source()), Managed::Set { value: true, source: source() });
        assert_eq!(flag(Raw::Bool(false), source()), Managed::Set { value: false, source: source() });
        assert_eq!(flag(Raw::Integer(1), source()), Managed::Set { value: true, source: source() });
        assert_eq!(flag(Raw::Integer(0), source()), Managed::Set { value: false, source: source() });
    }

    #[test]
    fn anything_else_is_unreadable_and_says_why() {
        for (raw, why) in [
            (Raw::Integer(2), "expected 0 or 1, found 2"),
            (Raw::Integer(-1), "expected 0 or 1, found -1"),
            (Raw::Other("a string".into()), "expected 0 or 1, found a string"),
            (Raw::Error("access denied".into()), "access denied"),
        ] {
            assert_eq!(flag(raw, source()), Managed::Unreadable { source: source(), reason: why.into() });
        }
    }

    #[test]
    fn javascript_stays_off_when_the_policy_says_so_or_cant_be_read() {
        let policy = |m| Policy { disable_javascript: m };
        assert_eq!(policy(Managed::NotSet).javascript_lock(), None, "no policy: the user decides");
        assert_eq!(policy(Managed::Set { value: false, source: source() }).javascript_lock(), None);
        assert_eq!(policy(Managed::Set { value: true, source: source() }).javascript_lock(), Some((source(), None)));
        assert_eq!(policy(Managed::Unreadable { source: source(), reason: "bad".into() }).javascript_lock(), Some((source(), Some("bad".into()))));
    }

    #[test]
    fn the_stricter_of_two_policies_wins() {
        let on = |s: &str| Managed::Set { value: true, source: s.into() };
        let off = |s: &str| Managed::Set { value: false, source: s.into() };
        let bad = |s: &str| Managed::Unreadable { source: s.into(), reason: "bad".into() };
        assert_eq!(stricter(Managed::NotSet, Managed::NotSet), Managed::NotSet);
        assert_eq!(stricter(Managed::NotSet, on("user")), on("user"));
        assert_eq!(stricter(off("machine"), on("user")), on("user"), "the machine can't switch on what the user's policy turns off");
        assert_eq!(stricter(on("machine"), off("user")), on("machine"));
        assert_eq!(stricter(on("machine"), on("user")), on("machine"), "a tie names the machine policy");
        assert_eq!(stricter(on("machine"), bad("user")), bad("user"));
        assert_eq!(stricter(off("machine"), Managed::NotSet), off("machine"));
    }

    #[test]
    fn a_policy_file_sets_flags_by_name() {
        let js = |text: &str| from_json(text.as_bytes(), "/etc/pdfcraft/policy.json").disable_javascript;
        let src = "/etc/pdfcraft/policy.json (DisableJavaScript)".to_string();
        assert_eq!(js(r#"{"DisableJavaScript": true}"#), Managed::Set { value: true, source: src.clone() });
        assert_eq!(js(r#"{"DisableJavaScript": 1}"#), Managed::Set { value: true, source: src.clone() });
        assert_eq!(js(r#"{"DisableJavaScript": false}"#), Managed::Set { value: false, source: src.clone() });
        assert_eq!(js(r#"{"DisableJavaScript": 0}"#), Managed::Set { value: false, source: src.clone() });
        assert_eq!(js(r#"{}"#), Managed::NotSet);
        assert_eq!(js(r#"{"SomeLaterPolicy": "x"}"#), Managed::NotSet, "unknown keys are ignored");
        assert_eq!(
            js(r#"{"DisableJavaScript": "yes"}"#),
            Managed::Unreadable { source: src.clone(), reason: "expected 0 or 1, found a string".into() }
        );
        assert_eq!(js(r#"{"DisableJavaScript": 1.5}"#), Managed::Unreadable { source: src, reason: "expected 0 or 1, found the number 1.5".into() });
    }

    #[test]
    fn a_damaged_policy_file_keeps_everything_locked() {
        for text in ["", "{\"DisableJavaScript\": tru", "[true]", "true", "{} {}"] {
            assert!(matches!(from_json(text.as_bytes(), "p").disable_javascript, Managed::Unreadable { .. }), "{text:?} should be unreadable");
        }
        // A byte-order mark is fine.
        let bom = "\u{feff}{\"DisableJavaScript\": 1}";
        assert_eq!(from_json(bom.as_bytes(), "p").javascript_lock(), Some(("p (DisableJavaScript)".into(), None)));
        // Not UTF-8 at all.
        assert!(matches!(from_json(&[0xff, 0xfe, 0x00], "p").disable_javascript, Managed::Unreadable { .. }));
    }

    #[test]
    fn policy_files_on_disk() {
        let dir = std::env::temp_dir().join(format!("pdfcraft-policy-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        assert_eq!(from_file(&dir.join("missing.json")), Policy::default(), "no file, no policy");

        let file = dir.join("policy.json");
        std::fs::write(&file, br#"{"DisableJavaScript": true}"#).unwrap();
        assert_eq!(from_file(&file).javascript_lock().map(|(s, why)| (s.ends_with("(DisableJavaScript)"), why)), Some((true, None)));

        let cap = usize::try_from(MAX_POLICY_FILE_BYTES).unwrap();
        let mut at_cap = br#"{"DisableJavaScript": false}"#.to_vec();
        at_cap.resize(cap, b' ');
        std::fs::write(&file, &at_cap).unwrap();
        assert_eq!(from_file(&file).javascript_lock(), None, "a file at the cap is read");
        at_cap.push(b' ');
        std::fs::write(&file, &at_cap).unwrap();
        let (_, why) = from_file(&file).javascript_lock().unwrap();
        assert!(why.unwrap().contains("larger than 64 KB"));

        // A folder where the file should be can't be read: locked, with the reason.
        let folder = dir.join("folder.json");
        std::fs::create_dir_all(&folder).unwrap();
        assert!(from_file(&folder).javascript_lock().unwrap().1.is_some());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[cfg(windows)]
    #[test]
    fn a_missing_registry_key_is_no_policy() {
        // Read-only: a key nobody has, under both roots.
        let key = format!(r"SOFTWARE\Policies\PdfCraft-test-{}-absent", std::process::id());
        assert_eq!(windows::value(&winsafe::HKEY::LOCAL_MACHINE, &key, DISABLE_JAVASCRIPT), Raw::Missing);
        assert_eq!(windows::value(&winsafe::HKEY::CURRENT_USER, &key, DISABLE_JAVASCRIPT), Raw::Missing);
        // An existing key without the value.
        assert_eq!(windows::value(&winsafe::HKEY::CURRENT_USER, "Software", "PdfCraft-test-absent-value"), Raw::Missing);
    }
}
