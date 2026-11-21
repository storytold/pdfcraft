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

    /// Clear a persisted `fullscreen:true` flag from eframe's `"window"` entry while preserving
    /// `normal_geometry` and restoring `maximized:true` when the window was maximized before
    /// entering full screen (#8010).
    pub fn clear_saved_fullscreen(&mut self, storage: &mut dyn eframe::Storage, restore_maximized: Option<bool>) {
        let Some(saved) = storage.get_string(WINDOW_KEY) else { return };
        let Some(mut cleaned) = set_ron_bool_field(&saved, "fullscreen", false) else { return };
        if restore_maximized == Some(true)
            && let Some(restored) = set_ron_bool_field(&cleaned, "maximized", true)
        {
            cleaned = restored;
        }
        storage.set_string(WINDOW_KEY, cleaned.clone());
        if self.shown.as_deref() == Some(saved.as_str()) {
            self.shown = Some(cleaned);
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

/// Whether `offset` lies directly inside the top-level `(...)` RON struct (nesting depth 1,
/// outside any `"..."` string literal).
fn at_top_level_field(bytes: &[u8], offset: usize) -> bool {
    let mut depth: i32 = 0;
    let mut in_string = false;
    let mut escaped = false;
    for &b in bytes.get(..offset).unwrap_or_default() {
        if in_string {
            if escaped {
                escaped = false;
            } else if b == b'\\' {
                escaped = true;
            } else if b == b'"' {
                in_string = false;
            }
            continue;
        }
        match b {
            b'"' => in_string = true,
            b'(' | b'[' | b'{' => depth = depth.saturating_add(1),
            b')' | b']' | b'}' => depth = depth.saturating_sub(1),
            _ => {}
        }
    }
    !in_string && depth == 1
}

/// Rewrite a top-level boolean field (`field:true` / `field:false`, with optional ASCII whitespace
/// around `:`) in eframe's RON window settings to `value`. Returns `None` when the field is
/// absent, malformed, nested, inside a string, or already holds `value`.
fn set_ron_bool_field(ron: &str, field: &str, value: bool) -> Option<String> {
    let (from, to) = if value { ("false", "true") } else { ("true", "false") };
    let is_before = |b: u8| matches!(b, b'(' | b',') || b.is_ascii_whitespace();
    let is_after = |b: u8| matches!(b, b',' | b')') || b.is_ascii_whitespace();
    let bytes = ron.as_bytes();
    let mut search = 0;
    while let Some(rel) = ron.get(search..)?.find(field) {
        let start = search + rel;
        let after_field = start + field.len();
        search = after_field;
        if start == 0 || !bytes.get(start - 1).copied().is_some_and(is_before) || !at_top_level_field(bytes, start) {
            continue;
        }
        let mut pos = after_field;
        while bytes.get(pos).is_some_and(u8::is_ascii_whitespace) {
            pos += 1;
        }
        if bytes.get(pos) != Some(&b':') {
            continue;
        }
        pos += 1;
        while bytes.get(pos).is_some_and(u8::is_ascii_whitespace) {
            pos += 1;
        }
        let rest = ron.get(pos..)?;
        if rest.starts_with(from) && bytes.get(pos + from.len()).copied().is_none_or(is_after) {
            let mut out = String::with_capacity(ron.len() + 1);
            out.push_str(ron.get(..pos)?);
            out.push_str(to);
            out.push_str(ron.get(pos + from.len()..)?);
            return Some(out);
        }
    }
    None
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

    /// Regression (#8010): closing while in full screen must not persist `fullscreen:true`, while
    /// keeping the saved normal geometry and the pre-fullscreen maximize state intact.
    #[test]
    fn clear_saved_fullscreen_keeps_geometry_and_restores_maximized() {
        let normal_fs = "(normal_geometry:(outer_position_pixels:Some((x:96.0,y:96.0)),scale_factor:1.25,inner_size_points:(x:1440.0,y:864.0)),fullscreen:true,maximized:false)";
        let mut store = Store::default();
        store.set_string(WINDOW_KEY, normal_fs.into());
        let mut state = WindowState::started_with(Some(&store));
        state.keep(&mut store, false);
        state.clear_saved_fullscreen(&mut store, Some(false));
        let cleaned = store.get_string(WINDOW_KEY).unwrap_or_default();
        assert!(cleaned.contains("fullscreen:false"), "fullscreen cleared: {cleaned}");
        assert!(cleaned.contains("maximized:false"), "normal window stays unmaximized: {cleaned}");
        assert!(cleaned.contains("outer_position_pixels:Some((x:96.0,y:96.0))"), "normal geometry kept: {cleaned}");

        // A later minimized save must put back the scrubbed entry, never the old `fullscreen:true`.
        store.set_string(WINDOW_KEY, MINIMIZED.into());
        state.keep(&mut store, true);
        assert_eq!(store.get_string(WINDOW_KEY).as_deref(), Some(cleaned.as_str()));

        // A maximized window closed during full screen preserves `maximized:true`.
        store.set_string(WINDOW_KEY, normal_fs.into());
        state.keep(&mut store, false);
        state.clear_saved_fullscreen(&mut store, Some(true));
        let max_cleaned = store.get_string(WINDOW_KEY).unwrap_or_default();
        assert!(max_cleaned.contains("fullscreen:false"), "fullscreen cleared: {max_cleaned}");
        assert!(max_cleaned.contains("maximized:true"), "pre-fullscreen maximize preserved: {max_cleaned}");
        assert!(max_cleaned.contains("inner_size_points:(x:1440.0,y:864.0)"), "normal geometry kept: {max_cleaned}");

        // A non-fullscreen save is never altered, even if `restore_maximized` were set.
        let untouched = "(normal_geometry:(outer_position_pixels:Some((x:96.0,y:96.0)),scale_factor:1.25,inner_size_points:(x:1440.0,y:864.0)),fullscreen:false,maximized:false)";
        store.set_string(WINDOW_KEY, untouched.into());
        state.clear_saved_fullscreen(&mut store, Some(true));
        assert_eq!(store.get_string(WINDOW_KEY).as_deref(), Some(untouched));

        // Optional whitespace / pretty-printed multiline RON is handled; similarly named fields and
        // unrelated storage keys are untouched.
        let pretty = "(\n    not_fullscreen: true,\n    fullscreen : true,\n    maximized: false,\n)";
        store.set_string("pdfcraft", r#"{"theme":"Dark"}"#.into());
        store.set_string(WINDOW_KEY, pretty.into());
        state.clear_saved_fullscreen(&mut store, Some(true));
        assert_eq!(store.get_string(WINDOW_KEY).as_deref(), Some("(\n    not_fullscreen: true,\n    fullscreen : false,\n    maximized: true,\n)"));
        assert_eq!(store.get_string("pdfcraft").as_deref(), Some(r#"{"theme":"Dark"}"#));

        // Matches inside quoted strings or nested RON values are skipped; only top-level fields change.
        let nested_and_string = "(note:\"(fullscreen:true)\",nested:(fullscreen:true),fullscreen:true,maximized:false)";
        store.set_string(WINDOW_KEY, nested_and_string.into());
        state.clear_saved_fullscreen(&mut store, Some(false));
        assert_eq!(
            store.get_string(WINDOW_KEY).as_deref(),
            Some("(note:\"(fullscreen:true)\",nested:(fullscreen:true),fullscreen:false,maximized:false)")
        );
        let only_nested = "(note:\"(fullscreen:true)\",nested:(fullscreen:true),fullscreen:false,maximized:false)";
        store.set_string(WINDOW_KEY, only_nested.into());
        state.clear_saved_fullscreen(&mut store, Some(true));
        assert_eq!(store.get_string(WINDOW_KEY).as_deref(), Some(only_nested));

        // Malformed, truncated, quoted, or non-boolean RON never panics and leaves storage intact.
        for malformed in ["", "(fullscreen:", "(fullscreen:123)", "\"fullscreen:true\"", "(fullscreen:true!)", "🔥fullscreen:tru"] {
            store.set_string(WINDOW_KEY, malformed.into());
            state.clear_saved_fullscreen(&mut store, Some(true));
            assert_eq!(store.get_string(WINDOW_KEY).as_deref(), Some(malformed), "malformed input left alone: {malformed}");
        }
    }
}
