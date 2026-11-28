use super::{PdfCraftApp, QuickTool};
use craft::host::target::Backend;
use craft::ui::runtime::windows::WindowHost;
use craft::ui::shell::ScreenMode;
use craft::window::Prefix;
use craft::window::policy::{self, TitleBarPref};
use pdfcraft_engine::DocId;

/// eframe's storage key for the native window settings in `app.ron`.
#[cfg(not(target_arch = "wasm32"))]
const WINDOW_KEY: &str = "window";

impl PdfCraftApp {
    /// Follow Use system title bar at once: the window gets the frame the policy decides for `pref`
    /// (or loses it again). The backend is the one winit opens by default, as at start-up; the
    /// host sends nothing when the decision is unchanged.
    pub fn set_title_bar_choice(&self, ctx: &egui::Context, pref: TitleBarPref) {
        let decision = policy::decide_for(Prefix::new("PDFCRAFT"), Backend::from_env(), pref);
        WindowHost::of(ctx).set_root_frame(decision);
    }

    /// Whether Use system title bar changes anything here: the policy gives the desktop the window
    /// was framed for another frame for each setting. It doesn't on the web, on a tiling
    /// compositor, or where `PDFCRAFT_TITLEBAR` decides; nor before the window's frame is known.
    pub(crate) fn system_title_bar_available(&self, ctx: &egui::Context) -> bool {
        let Some(desktop) = WindowHost::of(ctx).desktop() else { return false };
        let env = std::env::var(Prefix::new("PDFCRAFT").var_name()).or_else(|_| std::env::var(Prefix::default().var_name())).ok();
        let frame = |pref| {
            policy::decide(
                policy::Os::current(),
                policy::Runtime::current(),
                Backend::from_env(),
                &desktop,
                policy::WindowKind::Main,
                pref,
                env.as_deref(),
            )
            .frame
        };
        frame(TitleBarPref::Integrated) != frame(TitleBarPref::System)
    }

    /// Whether the window shows the body only (Acrobat's Full Screen Mode).
    pub fn full_screen(&self) -> bool {
        self.screen_mode.body_only()
    }

    /// The one entry point for a screen-mode change: the shell's commands, the window's own button
    /// and the app's commands all write the mode here. The shell sends the OS window action when
    /// the mode it follows changes.
    pub(crate) fn set_screen_mode(&mut self, mode: ScreenMode) {
        self.screen_mode = mode;
    }

    /// Enter or leave full-screen reading (Acrobat: View ▸ Full Screen Mode, ⌘L).
    pub fn set_full_screen(&mut self, on: bool) {
        self.set_screen_mode(if on { ScreenMode::FullScreen } else { ScreenMode::Standard });
    }

    /// Before `AppShell` draws: on Windows, exit full screen as soon as no document remains so
    /// `AppShell` restores the window frame and any pre-fullscreen maximized state on this pass
    /// (#8010).
    #[cfg(not(target_arch = "wasm32"))]
    pub(super) fn sync_full_screen_before_shell(&mut self) {
        if cfg!(target_os = "windows") && self.screen_mode.is_full_screen() && self.active.is_none() {
            self.set_screen_mode(ScreenMode::Standard);
        }
    }

    /// After `AppShell`, shortcuts, and [`Self::follow_screen_mode`]: capture the pre-fullscreen
    /// maximize state on entry and reconcile Windows' borderless full-screen state (#8010).
    ///
    /// Full Screen Mode is a per-session document view, not a saved window style:
    /// - If `app.ron` from an earlier build restored a borderless fullscreen window (which strips
    ///   `WS_OVERLAPPEDWINDOW` on Windows) while PdfCraft is in `ScreenMode::Standard`, exit OS
    ///   fullscreen so the window frame, caption controls, resizing and Snap return automatically.
    /// - If the last document tab closes while in Full Screen Mode, leave Full Screen Mode so the
    ///   Home tab is not stranded inside an undecorated borderless fullscreen window where
    ///   `view.full_screen` is disabled.
    #[cfg(not(target_arch = "wasm32"))]
    pub(super) fn sync_full_screen_after_shell(&mut self, ctx: &egui::Context, shell_saw_full: bool) {
        let was_shell = self.shell_full_screen;
        let is_full = self.screen_mode.is_full_screen();
        if !was_shell && shell_saw_full {
            self.full_screen_maximized = ctx.input(|i| i.viewport().maximized);
        } else if !is_full && !shell_saw_full {
            self.full_screen_maximized = None;
        }
        self.shell_full_screen = shell_saw_full;
        if shell_saw_full && !is_full {
            // Leaving full screen after `AppShell` ran (e.g. Escape): repaint so `AppShell` exits
            // OS full screen and restores any pre-fullscreen maximized state on the next pass.
            ctx.request_repaint();
            return;
        }
        if cfg!(target_os = "windows") {
            if is_full && self.active.is_none() {
                self.set_screen_mode(ScreenMode::Standard);
                self.follow_screen_mode();
                self.full_screen_maximized = None;
                ctx.request_repaint();
                return;
            }
            if !was_shell && !shell_saw_full {
                self.reconcile_full_screen(ctx);
            }
        }
    }

    /// Platform-independent full-screen reconciliation (#8010), exposed so regression tests run on
    /// every OS.
    pub fn reconcile_full_screen(&mut self, ctx: &egui::Context) {
        if self.screen_mode.is_full_screen() && self.active.is_none() {
            self.set_screen_mode(ScreenMode::Standard);
            self.follow_screen_mode();
            ctx.send_viewport_cmd(egui::ViewportCommand::Fullscreen(false));
            return;
        }
        if !self.screen_mode.is_full_screen() && ctx.input(|i| i.viewport().fullscreen == Some(true)) {
            ctx.send_viewport_cmd(egui::ViewportCommand::Fullscreen(false));
        }
    }

    /// Reacts to the screen mode as it stands after the shell's pass, whoever wrote it: every
    /// document shows one page (or spread) at a time fitted to the screen while the body is
    /// alone (Acrobat's Full Screen Mode, #379, #1459), and gets its display back afterwards.
    pub(crate) fn follow_screen_mode(&mut self) {
        let on = self.full_screen();
        // A click turns the page only with the Select tool, and full screen hides the tools:
        // select it there, and give the tool back on leaving (review on #2070).
        if on && self.full_screen_tool.is_none() {
            self.full_screen_tool = Some(self.quick_tool);
            self.quick_tool = QuickTool::Select;
        } else if !on && let Some(tool) = self.full_screen_tool.take() {
            self.quick_tool = tool;
        }
        for view in &mut self.views {
            if on {
                view.enter_full_screen(self.full_screen_fill);
            } else {
                view.leave_presentation();
            }
        }
    }

    /// Answer the Full Screen mode prompt raised for document `id` on opening: enter it, or leave
    /// the window as it is.
    pub fn resolve_full_screen_prompt(&mut self, id: DocId, allow: bool) {
        if allow && self.session.get(id).is_some() {
            self.set_full_screen(true);
        }
    }
}

/// Clear a persisted `fullscreen:true` flag from eframe's `"window"` entry while preserving
/// `normal_geometry` and applying the captured pre-fullscreen `maximized` state (#8010).
///
/// When `minimized` is true and a pre-fullscreen maximize state was captured, apply it even if the
/// stored snapshot already carries `fullscreen:false` without touching ordinary non-fullscreen
/// saves.
#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn clear_saved_fullscreen(storage: &mut dyn eframe::Storage, minimized: bool, restore_maximized: Option<bool>) {
    let Some(saved) = storage.get_string(WINDOW_KEY) else { return };
    let cleared_fullscreen = set_ron_bool_field(&saved, "fullscreen", false);
    let mut updated = cleared_fullscreen.clone();
    if let Some(maximized) = restore_maximized
        && (cleared_fullscreen.is_some() || minimized)
    {
        let base = updated.as_deref().unwrap_or(&saved);
        if let Some(with_max) = set_ron_bool_field(base, "maximized", maximized) {
            updated = Some(with_max);
        }
    }
    if let Some(cleaned) = updated {
        storage.set_string(WINDOW_KEY, cleaned);
    }
}

/// Whether `offset` lies directly inside the top-level `(...)` RON struct (nesting depth 1,
/// outside any `"..."` string literal).
#[cfg(not(target_arch = "wasm32"))]
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
#[cfg(not(target_arch = "wasm32"))]
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

#[cfg(all(test, not(target_arch = "wasm32")))]
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

    /// Regression (#8010): closing while in full screen must not persist `fullscreen:true`, while
    /// keeping the saved normal geometry and the pre-fullscreen maximize state intact.
    #[test]
    fn clear_saved_fullscreen_keeps_geometry_and_restores_maximized() {
        let normal_fs = "(normal_geometry:(outer_position_pixels:Some((x:96.0,y:96.0)),scale_factor:1.25,inner_size_points:(x:1440.0,y:864.0)),fullscreen:true,maximized:false)";
        let mut store = Store::default();
        store.set_string(WINDOW_KEY, normal_fs.into());
        clear_saved_fullscreen(&mut store, false, Some(false));
        let cleaned = store.get_string(WINDOW_KEY).unwrap_or_default();
        assert!(cleaned.contains("fullscreen:false"), "fullscreen cleared: {cleaned}");
        assert!(cleaned.contains("maximized:false"), "normal window stays unmaximized: {cleaned}");
        assert!(cleaned.contains("outer_position_pixels:Some((x:96.0,y:96.0))"), "normal geometry kept: {cleaned}");

        // A maximized window closed during full screen preserves `maximized:true`.
        store.set_string(WINDOW_KEY, normal_fs.into());
        clear_saved_fullscreen(&mut store, false, Some(true));
        let max_cleaned = store.get_string(WINDOW_KEY).unwrap_or_default();
        assert!(max_cleaned.contains("fullscreen:false"), "fullscreen cleared: {max_cleaned}");
        assert!(max_cleaned.contains("maximized:true"), "pre-fullscreen maximize preserved: {max_cleaned}");
        assert!(max_cleaned.contains("inner_size_points:(x:1440.0,y:864.0)"), "normal geometry kept: {max_cleaned}");

        // A non-minimized, non-fullscreen save is never altered, even if `restore_maximized` were set.
        let untouched = "(normal_geometry:(outer_position_pixels:Some((x:96.0,y:96.0)),scale_factor:1.25,inner_size_points:(x:1440.0,y:864.0)),fullscreen:false,maximized:false)";
        store.set_string(WINDOW_KEY, untouched.into());
        clear_saved_fullscreen(&mut store, false, Some(true));
        assert_eq!(store.get_string(WINDOW_KEY).as_deref(), Some(untouched));

        // Optional whitespace / pretty-printed multiline RON is handled; similarly named fields and
        // unrelated storage keys are untouched.
        let pretty = "(\n    not_fullscreen: true,\n    fullscreen : true,\n    maximized: false,\n)";
        store.set_string("pdfcraft", r#"{"theme":"Dark"}"#.into());
        store.set_string(WINDOW_KEY, pretty.into());
        clear_saved_fullscreen(&mut store, false, Some(true));
        assert_eq!(store.get_string(WINDOW_KEY).as_deref(), Some("(\n    not_fullscreen: true,\n    fullscreen : false,\n    maximized: true,\n)"));
        assert_eq!(store.get_string("pdfcraft").as_deref(), Some(r#"{"theme":"Dark"}"#));

        // Matches inside quoted strings or nested RON values are skipped; only top-level fields change.
        let nested_and_string = "(note:\"(fullscreen:true)\",nested:(fullscreen:true),fullscreen:true,maximized:false)";
        store.set_string(WINDOW_KEY, nested_and_string.into());
        clear_saved_fullscreen(&mut store, false, Some(false));
        assert_eq!(
            store.get_string(WINDOW_KEY).as_deref(),
            Some("(note:\"(fullscreen:true)\",nested:(fullscreen:true),fullscreen:false,maximized:false)")
        );
        let only_nested = "(note:\"(fullscreen:true)\",nested:(fullscreen:true),fullscreen:false,maximized:false)";
        store.set_string(WINDOW_KEY, only_nested.into());
        clear_saved_fullscreen(&mut store, false, Some(true));
        assert_eq!(store.get_string(WINDOW_KEY).as_deref(), Some(only_nested));

        // Malformed, truncated, quoted, or non-boolean RON never panics and leaves storage intact.
        for malformed in ["", "(fullscreen:", "(fullscreen:123)", "\"fullscreen:true\"", "(fullscreen:true!)", "🔥fullscreen:tru"] {
            store.set_string(WINDOW_KEY, malformed.into());
            clear_saved_fullscreen(&mut store, false, Some(true));
            assert_eq!(store.get_string(WINDOW_KEY).as_deref(), Some(malformed), "malformed input left alone: {malformed}");
        }
    }

    /// Regression (PR #8021 review): if a minimized save holds a snapshot that already carries
    /// `fullscreen:false,maximized:false` after the user maximized and entered full screen, the
    /// captured pre-fullscreen maximize state must still be applied while preserving
    /// `normal_geometry` and unrelated keys.
    #[test]
    fn minimized_save_during_full_screen_applies_captured_maximize_to_restored_snapshot() {
        let initial = "(normal_geometry:(outer_position_pixels:Some((x:96.0,y:96.0)),scale_factor:1.25,inner_size_points:(x:1440.0,y:864.0)),fullscreen:false,maximized:false)";
        let mut store = Store::default();
        store.set_string("pdfcraft", r#"{"theme":"Dark"}"#.into());
        store.set_string(WINDOW_KEY, initial.into());

        // Ordinary minimized save with no captured full-screen state leaves the snapshot untouched.
        clear_saved_fullscreen(&mut store, true, None);
        assert_eq!(store.get_string(WINDOW_KEY).as_deref(), Some(initial));

        // User maximizes, enters full screen (captured Some(true)), then minimizes before save.
        clear_saved_fullscreen(&mut store, true, Some(true));
        let saved = store.get_string(WINDOW_KEY).unwrap_or_default();
        assert!(saved.contains("fullscreen:false"), "fullscreen remains false: {saved}");
        assert!(saved.contains("maximized:true"), "captured pre-fullscreen maximize applied: {saved}");
        assert!(saved.contains("outer_position_pixels:Some((x:96.0,y:96.0))"), "normal geometry kept: {saved}");
        assert_eq!(store.get_string("pdfcraft").as_deref(), Some(r#"{"theme":"Dark"}"#));

        // Reverse transition: restored down before entering full screen (Some(false)), then minimized.
        clear_saved_fullscreen(&mut store, true, Some(false));
        let unmaxed = store.get_string(WINDOW_KEY).unwrap_or_default();
        assert!(unmaxed.contains("fullscreen:false"), "fullscreen remains false: {unmaxed}");
        assert!(unmaxed.contains("maximized:false"), "captured pre-fullscreen unmaximize applied: {unmaxed}");
        assert!(unmaxed.contains("inner_size_points:(x:1440.0,y:864.0)"), "normal geometry kept: {unmaxed}");
    }

    #[test]
    fn reconcile_full_screen_handles_stale_startup_and_empty_tabs() {
        let ctx = egui::Context::default();
        PdfCraftApp::install_runtime(&ctx);
        let mut app = PdfCraftApp::for_unit_tests();

        let run_reconcile = |app: &mut PdfCraftApp, os_fullscreen: bool| -> Vec<egui::ViewportCommand> {
            let mut input = egui::RawInput::default();
            input.viewports.entry(egui::ViewportId::ROOT).or_default().fullscreen = Some(os_fullscreen);
            let out = ctx.run_logic(&input, |ctx| app.reconcile_full_screen(ctx));
            out.viewport_commands.get(&egui::ViewportId::ROOT).cloned().unwrap_or_default()
        };

        // Stale OS fullscreen at startup while app is in Standard mode -> exits OS fullscreen.
        assert!(!app.full_screen());
        assert_eq!(run_reconcile(&mut app, true), [egui::ViewportCommand::Fullscreen(false)]);

        // Full screen with no active document -> exits full screen and sends Fullscreen(false).
        app.set_full_screen(true);
        assert!(app.full_screen());
        assert!(app.active.is_none());
        assert_eq!(run_reconcile(&mut app, true), [egui::ViewportCommand::Fullscreen(false)]);
        assert!(!app.full_screen());
    }

    #[test]
    fn sync_full_screen_captures_maximized_and_save_scrubs_storage() {
        use eframe::App as _;

        let ctx = egui::Context::default();
        PdfCraftApp::install_runtime(&ctx);
        let mut app = PdfCraftApp::for_unit_tests();
        let pdf = b"%PDF-1.4\n1 0 obj << /Type /Catalog /Pages 2 0 R >> endobj\n2 0 obj << /Type /Pages /Kids [3 0 R] /Count 1 >> endobj\n3 0 obj << /Type /Page /Parent 2 0 R /MediaBox [0 0 200 200] >> endobj\nxref\n0 4\n0000000000 65535 f \n0000000009 00000 n \n0000000058 00000 n \n0000000115 00000 n \ntrailer << /Size 4 /Root 1 0 R >>\nstartxref\n190\n%%EOF\n";
        assert!(app.open_bytes(&ctx, "doc.pdf", None, pdf.to_vec()).is_ok());

        // Simulate entering full screen while the OS window reports maximized = Some(true).
        app.set_full_screen(true);
        let mut input = egui::RawInput::default();
        input.viewports.entry(egui::ViewportId::ROOT).or_default().maximized = Some(true);
        let _ = ctx.run_logic(&input, |ctx| app.sync_full_screen_after_shell(ctx, true));
        assert_eq!(app.full_screen_maximized, Some(true));

        // Saving during full screen on Windows clears fullscreen:true and preserves maximized:true.
        let mut store = Store::default();
        let fs_entry = "(normal_geometry:(outer_position_pixels:Some((x:96.0,y:96.0)),scale_factor:1.25,inner_size_points:(x:1440.0,y:864.0)),fullscreen:true,maximized:false)";
        store.set_string(WINDOW_KEY, fs_entry.into());
        app.save(&mut store);
        let saved = store.get_string(WINDOW_KEY).unwrap_or_default();
        if cfg!(target_os = "windows") {
            assert!(saved.contains("fullscreen:false"), "fullscreen cleared: {saved}");
            assert!(saved.contains("maximized:true"), "maximized preserved: {saved}");
            assert!(saved.contains("outer_position_pixels:Some((x:96.0,y:96.0))"), "normal geometry kept: {saved}");
        }

        // Closing the final document tab causes `sync_full_screen_before_shell` to exit full screen on Windows.
        app.close_tab(0);
        assert!(app.active.is_none());
        app.sync_full_screen_before_shell();
        if cfg!(target_os = "windows") {
            assert!(!app.full_screen(), "closing last tab exits full screen before AppShell draws");
        }
    }

    /// Regression (PR #8021 CodeRabbit review): when Escape leaves full screen after `AppShell`
    /// has already run for the pass (`shell_saw_full == true`), `AppShell` still holds OS
    /// fullscreen until the next pass. A save landing in that gap must retain the captured
    /// pre-fullscreen maximize state, and the capture must be cleared once `AppShell` finishes
    /// exiting on the subsequent pass.
    #[test]
    fn escape_exit_preserves_captured_maximize_until_shell_leaves_full_screen() {
        use eframe::App as _;

        let ctx = egui::Context::default();
        PdfCraftApp::install_runtime(&ctx);
        let mut app = PdfCraftApp::for_unit_tests();
        let pdf = b"%PDF-1.4\n1 0 obj << /Type /Catalog /Pages 2 0 R >> endobj\n2 0 obj << /Type /Pages /Kids [3 0 R] /Count 1 >> endobj\n3 0 obj << /Type /Page /Parent 2 0 R /MediaBox [0 0 200 200] >> endobj\nxref\n0 4\n0000000000 65535 f \n0000000009 00000 n \n0000000058 00000 n \n0000000115 00000 n \ntrailer << /Size 4 /Root 1 0 R >>\nstartxref\n190\n%%EOF\n";
        assert!(app.open_bytes(&ctx, "doc.pdf", None, pdf.to_vec()).is_ok());

        // Pass 1: enter full screen from a maximized window; AppShell saw full screen.
        app.set_full_screen(true);
        let mut input = egui::RawInput::default();
        input.viewports.entry(egui::ViewportId::ROOT).or_default().maximized = Some(true);
        let _ = ctx.run_logic(&input, |ctx| app.sync_full_screen_after_shell(ctx, true));
        assert_eq!(app.full_screen_maximized, Some(true));

        // Pass 2: Escape leaves full screen *after* AppShell already ran with full screen.
        // OS fullscreen exit is pending until Pass 3, so the captured maximize state must stay.
        app.set_full_screen(false);
        let _ = ctx.run_logic(&egui::RawInput::default(), |ctx| app.sync_full_screen_after_shell(ctx, true));
        assert_eq!(app.full_screen_maximized, Some(true), "captured maximize state must remain until AppShell exits");

        // A save (autosave or exit) landing before Pass 3 still sees `fullscreen:true,maximized:false`
        // from eframe and must preserve `maximized:true` on Windows.
        let mut store = Store::default();
        let pending_exit = "(normal_geometry:(outer_position_pixels:Some((x:96.0,y:96.0)),scale_factor:1.25,inner_size_points:(x:1440.0,y:864.0)),fullscreen:true,maximized:false)";
        store.set_string(WINDOW_KEY, pending_exit.into());
        app.save(&mut store);
        if cfg!(target_os = "windows") {
            let saved = store.get_string(WINDOW_KEY).unwrap_or_default();
            assert!(saved.contains("fullscreen:false"), "fullscreen cleared: {saved}");
            assert!(saved.contains("maximized:true"), "pre-fullscreen maximize preserved during pending exit: {saved}");
            assert!(saved.contains("outer_position_pixels:Some((x:96.0,y:96.0))"), "normal geometry kept: {saved}");
        }

        // Pass 3: AppShell now runs in Standard mode (`shell_saw_full == false`) and exits OS
        // fullscreen; the captured maximize state is cleared now.
        let _ = ctx.run_logic(&egui::RawInput::default(), |ctx| app.sync_full_screen_after_shell(ctx, false));
        assert_eq!(app.full_screen_maximized, None, "capture cleared once AppShell leaves full screen");

        // Subsequent ordinary non-fullscreen save is left untouched.
        let normal = "(normal_geometry:(outer_position_pixels:Some((x:96.0,y:96.0)),scale_factor:1.25,inner_size_points:(x:1440.0,y:864.0)),fullscreen:false,maximized:false)";
        store.set_string(WINDOW_KEY, normal.into());
        app.save(&mut store);
        assert_eq!(store.get_string(WINDOW_KEY).as_deref(), Some(normal));
    }
}
