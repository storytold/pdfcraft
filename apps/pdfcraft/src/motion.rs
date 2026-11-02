//! The system's "no animations" setting, which Preferences ▸ Animation effects follows unless it is
//! set to On or Off (`pdfcraft_ui_egui::motion`).
//!
//! Windows: Settings ▸ Accessibility ▸ Visual effects ▸ Animation effects (in the Control Panel,
//! "Show animations in Windows"), read through WinRT's `UISettings.AnimationsEnabled`. Other
//! systems aren't asked yet: there the window animates as before.

use pdfcraft_ui_egui::motion::MotionSource;

/// Asks the system each time, and logs the answer when it changes.
pub fn source() -> MotionSource {
    let last = std::cell::Cell::new(None);
    Box::new(move || {
        let reduced = reduced();
        if reduced.is_some() && last.replace(reduced) != reduced {
            log::info!("system animation effects: {}", if reduced == Some(true) { "off" } else { "on" });
        }
        reduced
    })
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
    fn the_answer_is_the_systems_animation_effects_setting() {
        let answer = super::reduced();
        // The first answer is logged and later ones only when they change; the answers don't.
        let source = super::source();
        assert_eq!((source(), source()), (answer, answer));
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
