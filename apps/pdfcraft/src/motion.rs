//! The system's "no animations" setting, which Preferences ▸ Animation effects follows unless it is
//! set to On or Off (`pdfcraft_ui_egui::motion`).
//!
//! Windows: Settings ▸ Accessibility ▸ Visual effects ▸ Animation effects (in the Control Panel,
//! "Show animations in Windows"), read through WinRT's `UISettings.AnimationsEnabled`. Other
//! systems aren't asked yet: there the window animates as before.
//!
//! A Remote Desktop session (Azure Virtual Desktop included) counts as "no animations" too, whatever
//! the setting says: every animated frame is a whole window drawn, encoded and sent over the
//! network, so a wheel notch eased over a dozen frames scrolls far slower there than one that
//! lands at once. Preferences ▸ Animation effects ▸ On still animates.

use pdfcraft_ui_egui::motion::MotionSource;

/// Asks the system each time, and logs the answer when it changes.
pub fn source() -> MotionSource {
    let last = std::cell::Cell::new(None);
    Box::new(move || {
        let remote = remote_session();
        let reduced = reduced_in(reduced, remote);
        if reduced.is_some() && last.replace(reduced) != reduced {
            let why = if remote { " (Remote Desktop session)" } else { "" };
            log::info!("system animation effects: {}{why}", if reduced == Some(true) { "off" } else { "on" });
        }
        reduced
    })
}

/// The answer for the window: the system's own (`system`), except that a remote session never
/// animates. The system is asked only when its answer counts: every two seconds, a remote session
/// would otherwise make a Windows Runtime call for nothing.
fn reduced_in(system: impl FnOnce() -> Option<bool>, remote: bool) -> Option<bool> {
    if remote { Some(true) } else { system() }
}

/// Whether the system asks for no animations; `None` when it can't be asked.
pub fn reduced() -> Option<bool> {
    match animations_enabled() {
        Ok(enabled) => Some(!enabled),
        Err(e) => {
            log::debug!("system animation effects: {e}");
            None
        }
    }
}

/// Whether this process draws to a Remote Desktop session rather than the machine's own display
/// (`SM_REMOTESESSION`). It changes when a session is taken over from the console or handed back,
/// so it is asked each time.
pub fn remote_session() -> bool {
    winsafe::GetSystemMetrics(winsafe::co::SM::REMOTESESSION) != 0
}

fn animations_enabled() -> windows::core::Result<bool> {
    windows::UI::ViewManagement::UISettings::new()?.AnimationsEnabled()
}

#[cfg(test)]
mod tests {
    /// The user's saved Animation effects choice, or `None` when `reg` can't read it. Windows
    /// keeps its on/off system parameters as the bits of `UserPreferencesMask`: bit n is the
    /// parameter `SPI_GET…` 0x1000 + 2n, and client-area animation is 0x1042, so bit 33.
    fn saved_choice() -> Option<bool> {
        let out = std::process::Command::new("reg").args(["query", r"HKCU\Control Panel\Desktop", "/v", "UserPreferencesMask"]).output().ok()?;
        if !out.status.success() {
            return None;
        }
        let text = String::from_utf8_lossy(&out.stdout);
        let hex = text.split_whitespace().skip_while(|word| *word != "REG_BINARY").nth(1)?;
        let byte = u8::from_str_radix(hex.get(8..10)?, 16).ok()?;
        Some(byte & 0b10 != 0)
    }

    #[test]
    fn a_remote_session_never_animates_whatever_the_system_says() {
        for system in [None, Some(false), Some(true)] {
            assert_eq!(super::reduced_in(|| system, true), Some(true), "remote, system says {system:?}");
            assert_eq!(super::reduced_in(|| system, false), system, "at the machine's own display");
        }
    }

    #[test]
    fn a_remote_session_doesnt_ask_the_system() {
        let asked = std::cell::Cell::new(false);
        let system = || {
            asked.set(true);
            Some(false)
        };
        assert_eq!(super::reduced_in(system, true), Some(true));
        assert!(!asked.get(), "the system's answer doesn't count over Remote Desktop");
    }

    #[test]
    fn the_answer_is_the_systems_animation_effects_setting() {
        let answer = super::reduced();
        // The first answer is logged and later ones only when they change; the answers don't.
        let source = super::source();
        let expected = super::reduced_in(|| answer, super::remote_session());
        assert_eq!((source(), source()), (expected, expected));
        match super::animations_enabled() {
            // With Animation effects turned off, the answer must be "reduced". Only this way
            // round: the session's own value may be off for other reasons (a policy, a remote
            // session) while the saved choice is on.
            Ok(enabled) => {
                assert_eq!(answer, Some(!enabled));
                if saved_choice() == Some(false) {
                    assert!(!enabled, "Animation effects are off in Settings, but the answer was that animations are on");
                }
            }
            Err(e) => {
                eprintln!("UISettings isn't available here ({e}); not compared with the saved choice");
                assert_eq!(answer, None);
            }
        }
    }
}
