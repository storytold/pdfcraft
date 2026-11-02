//! The system's choice for Preferences ▸ "Reopen the files that were open when PdfCraft last closed".
//!
//! On macOS, System Settings ▸ Desktop & Dock ▸ Windows ▸ "Close windows when quitting an
//! application" is on by default; turning it off stores `NSQuitAlwaysKeepsWindows = 1`, and apps
//! then reopen their windows on the next launch. A user who turned it off expects PdfCraft to
//! bring its documents back too (#871), so that setting is the preference's starting value. It
//! is read once at launch through `defaults`; the user's own choice in Preferences, once saved,
//! wins over it. Other systems have no such setting, and the preference starts off there.

/// How long `defaults` may take before launch goes on without it.
#[cfg(target_os = "macos")]
const TIMEOUT: std::time::Duration = std::time::Duration::from_millis(500);

/// Whether reopening the last session starts on, before the saved preferences are applied.
pub fn system_default() -> bool {
    #[cfg(target_os = "macos")]
    if let Some(output) = defaults(&["read", "-g", "NSQuitAlwaysKeepsWindows"]) {
        return keeps_windows(&output);
    }
    false
}

/// `defaults read` output for a boolean (`1`, `0`, `true`, `false`): true only when it says so.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
fn keeps_windows(output: &str) -> bool {
    matches!(output.trim(), "1" | "true" | "yes" | "YES")
}

/// Run `defaults` and return what it printed, or `None` if it fails or hangs. Fails when the key
/// was never set, which means the system default: close windows when quitting.
#[cfg(target_os = "macos")]
fn defaults(args: &[&str]) -> Option<String> {
    use std::io::Read;
    use std::process::{Command, Stdio};
    // The absolute path keeps a `defaults` earlier on PATH from running.
    let mut child = Command::new("/usr/bin/defaults").args(args).stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::null()).spawn().ok()?;
    let start = std::time::Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(status)) if status.success() => break,
            Ok(None) if start.elapsed() < TIMEOUT => std::thread::sleep(std::time::Duration::from_millis(5)),
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                return None;
            }
        }
    }
    let mut out = String::new();
    // A boolean fits in far less; a misbehaving `defaults` can't make launch read without end.
    child.stdout.take()?.take(256).read_to_string(&mut out).ok()?;
    Some(out)
}

#[cfg(test)]
mod tests {
    #[test]
    fn only_a_set_flag_keeps_windows() {
        for on in ["1\n", "1", "true", "YES"] {
            assert!(super::keeps_windows(on), "{on:?}");
        }
        for off in ["0\n", "0", "false", "NO", "", "garbage"] {
            assert!(!super::keeps_windows(off), "{off:?}");
        }
    }

    #[test]
    fn reading_the_system_default_never_fails() {
        // Whatever this machine is set to, the read returns a value within the timeout.
        let _ = super::system_default();
    }
}
