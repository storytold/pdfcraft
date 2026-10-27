//! The window's saved size and place across starts. eframe stores them just before
//! `App::save`, from the window as it is at that moment. A minimized window has neither: Windows
//! reports it as 0 × 0 at (-32000, -32000), and the next start then opened at egui's 64-point
//! floor in the screen's corner. So while the window is minimized the last settings it was shown
//! with are put back, and a window that opens at that floor is given a usable size.

/// eframe's storage key for the window settings.
const WINDOW_KEY: &str = "window";
/// The size egui opens a stored size below (its floor for "ridiculously small" windows), in points.
const EGUI_FLOOR: f32 = 64.0;
/// The size a window that opened at [`EGUI_FLOOR`] is given (apps/pdfcraft opens a first start at
/// this size too), at most nine tenths of the monitor.
const USABLE: egui::Vec2 = egui::vec2(1440.0, 920.0);

#[derive(Debug, Default)]
pub struct WindowState {
    /// The settings eframe last stored for a shown (not minimized) window.
    shown: Option<String>,
    /// The size at start has been checked ([`Self::repair`]).
    checked: bool,
}

impl WindowState {
    /// The settings the window started with: what to put back if it is minimized before eframe
    /// stores a shown one.
    pub fn started_with(storage: Option<&dyn eframe::Storage>) -> Self {
        Self { shown: storage.and_then(|s| s.get_string(WINDOW_KEY)), checked: false }
    }

    /// After eframe has stored the window: remember it while the window is shown, and while it
    /// is minimized put the last shown settings back (or none, so the next start opens at the
    /// default size).
    pub fn keep(&mut self, storage: &mut dyn eframe::Storage, minimized: bool) {
        if !minimized {
            self.shown = storage.get_string(WINDOW_KEY);
        } else if let Some(shown) = &self.shown {
            storage.set_string(WINDOW_KEY, shown.clone());
        } else {
            storage.remove_string(WINDOW_KEY);
        }
    }

    /// Once the window's size is known: a shown window at egui's floor in both directions came
    /// from settings stored while it was minimized (before this fix), so give it a usable size.
    pub fn repair(&mut self, ctx: &egui::Context) {
        if self.checked {
            return;
        }
        let (rect, minimized, monitor) = ctx.input(|i| (i.viewport().inner_rect, i.viewport().minimized, i.viewport().monitor_size));
        let Some(rect) = rect else { return };
        if minimized == Some(true) {
            return;
        }
        self.checked = true;
        if rect.width() <= EGUI_FLOOR + 0.5 && rect.height() <= EGUI_FLOOR + 0.5 {
            // The stored settings are the broken ones: never put them back.
            self.shown = None;
            let size = monitor.map_or(USABLE, |m| USABLE.min(m * 0.9));
            ctx.send_viewport_cmd(egui::ViewportCommand::InnerSize(size));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use eframe::Storage as _;

    #[derive(Default)]
    struct Store(std::collections::HashMap<String, String>);

    impl eframe::Storage for Store {
        fn get_string(&self, key: &str) -> Option<String> {
            self.0.get(key).cloned()
        }
        fn set_string(&mut self, key: &str, value: String) {
            self.0.insert(key.to_string(), value);
        }
        fn remove_string(&mut self, key: &str) {
            self.0.remove(key);
        }
        fn flush(&mut self) {}
    }

    const SHOWN: &str = "(outer_position_pixels:Some((x:200.0,y:200.0)),inner_size_points:Some((x:1320.0,y:940.0)))";
    const MINIMIZED: &str = "(outer_position_pixels:Some((x:-32000.0,y:-32000.0)),inner_size_points:Some((x:0.0,y:0.0)))";

    /// Closing PdfCraft while it was minimized stored a 0 × 0 window off screen.
    #[test]
    fn a_minimized_window_keeps_the_last_shown_settings() {
        let mut store = Store::default();
        store.set_string(WINDOW_KEY, SHOWN.into());
        let mut state = WindowState::started_with(Some(&store));
        // eframe stores the minimized window, then the app saves.
        store.set_string(WINDOW_KEY, MINIMIZED.into());
        state.keep(&mut store, true);
        assert_eq!(store.get_string(WINDOW_KEY).as_deref(), Some(SHOWN));
        // Shown again and resized: that is what is kept from then on.
        let moved = SHOWN.replace("200.0", "300.0");
        store.set_string(WINDOW_KEY, moved.clone());
        state.keep(&mut store, false);
        store.set_string(WINDOW_KEY, MINIMIZED.into());
        state.keep(&mut store, true);
        assert_eq!(store.get_string(WINDOW_KEY), Some(moved));
    }

    #[test]
    fn with_nothing_shown_yet_a_minimized_save_stores_no_window() {
        let mut store = Store::default();
        let mut state = WindowState::started_with(Some(&store));
        store.set_string(WINDOW_KEY, MINIMIZED.into());
        state.keep(&mut store, true);
        assert_eq!(store.get_string(WINDOW_KEY), None, "the next start opens at the default size");
    }

    fn repaired(size: egui::Vec2, minimized: Option<bool>) -> Vec<egui::ViewportCommand> {
        let ctx = egui::Context::default();
        let mut input = egui::RawInput::default();
        let info = input.viewports.entry(egui::ViewportId::ROOT).or_default();
        info.inner_rect = Some(egui::Rect::from_min_size(egui::Pos2::ZERO, size));
        info.minimized = minimized;
        info.monitor_size = Some(egui::vec2(2674.0, 1504.0));
        let mut state = WindowState { shown: Some(MINIMIZED.into()), checked: false };
        let out = ctx.run_logic(&input, |ctx| state.repair(ctx));
        let commands = out.viewport_commands.get(&egui::ViewportId::ROOT).cloned().unwrap_or_default();
        // Repaired: the settings it started with are not put back by a minimized close.
        assert_eq!(state.shown.is_none(), !commands.is_empty());
        commands
    }

    #[test]
    fn a_window_at_eguis_floor_gets_a_usable_size() {
        assert_eq!(repaired(egui::vec2(64.0, 64.0), Some(false)), [egui::ViewportCommand::InnerSize(USABLE)]);
        assert!(repaired(egui::vec2(820.0, 520.0), Some(false)).is_empty(), "a small window someone chose stays");
        assert!(repaired(egui::vec2(64.0, 64.0), Some(true)).is_empty(), "not while minimized");
    }
}
