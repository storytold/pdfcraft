use super::{PdfCraftApp, QuickTool};

impl PdfCraftApp {
    /// Whether the app draws the window's title bar, buttons and resize edges: the desktop leaves
    /// the frame to the app and Use system title bar is off.
    pub fn draws_window_frame(&self) -> bool {
        self.window_controls.is_some() && !self.system_title_bar
    }

    /// Follow Use system title bar at once: the window starts without winit's frame wherever the
    /// app may draw its own, and gets it back (or loses it again) when the preference changes.
    pub(super) fn sync_window_frame(&mut self, ctx: &egui::Context) {
        if self.window_controls.is_none() {
            return;
        }
        let app_frame = self.draws_window_frame();
        if self.app_frame_shown != Some(app_frame) {
            // The window opens undecorated, so only a change from that needs a command.
            if self.app_frame_shown.is_some() || !app_frame {
                ctx.send_viewport_cmd(egui::ViewportCommand::Decorations(!app_frame));
            }
            self.app_frame_shown = Some(app_frame);
        }
    }

    /// Enter or leave full-screen reading (Acrobat: View ▸ Full Screen Mode, ⌘L).
    pub fn set_full_screen(&mut self, ctx: &egui::Context, on: bool) {
        if on && !self.full_screen {
            self.full_screen_maximized = ctx.input(|i| i.viewport().maximized);
        }
        self.apply_full_screen(on);
        ctx.send_viewport_cmd(egui::ViewportCommand::Fullscreen(on));
    }

    /// Keep Windows' borderless full-screen state aligned with PdfCraft's Full Screen Mode (#8010).
    ///
    /// Full Screen Mode is a per-session document view, not a saved window style:
    /// - If `app.ron` from an earlier build restored a borderless fullscreen window (which strips
    ///   `WS_OVERLAPPEDWINDOW` on Windows) while `self.full_screen` is off, exit OS fullscreen so
    ///   the native title bar, caption buttons, resizing and Snap return automatically.
    /// - If the last document tab closes while in Full Screen Mode, leave Full Screen Mode so the
    ///   Home tab is not shown inside an undecorated fullscreen window where `view.full_screen` is
    ///   disabled.
    #[cfg(not(target_arch = "wasm32"))]
    pub(super) fn sync_full_screen(&mut self, ctx: &egui::Context) {
        if cfg!(target_os = "windows") {
            self.reconcile_full_screen(ctx);
        }
    }

    /// Platform-independent implementation of [`Self::sync_full_screen`], exposed so regression
    /// tests run on every OS.
    pub fn reconcile_full_screen(&mut self, ctx: &egui::Context) {
        if self.full_screen && self.active.is_none() {
            self.set_full_screen(ctx, false);
            return;
        }
        if !self.full_screen && ctx.input(|i| i.viewport().fullscreen == Some(true)) {
            ctx.send_viewport_cmd(egui::ViewportCommand::Fullscreen(false));
        }
    }

    /// The full-screen state without the window: every document shows one page (or spread) at a
    /// time fitted to the screen while it is on (Acrobat's Full Screen Mode, #379, #1459), and
    /// gets its display back when it is off.
    pub fn apply_full_screen(&mut self, on: bool) {
        // A click turns the page only with the Select tool, and full screen hides the tools:
        // select it there, and give the tool back on leaving (review on #2070).
        if on && !self.full_screen {
            self.full_screen_tool = Some(self.quick_tool);
            self.quick_tool = QuickTool::Select;
        } else if !on {
            self.full_screen_maximized = None;
            if let Some(tool) = self.full_screen_tool.take() {
                self.quick_tool = tool;
            }
        }
        self.full_screen = on;
        for view in &mut self.views {
            if on {
                view.full_screen_fill = self.view_defaults.full_screen_fill;
                view.enter_presentation();
            } else {
                view.leave_presentation();
            }
        }
    }

    /// Answer the Full Screen mode prompt a document raised on opening: enter it, or leave the
    /// window as it is.
    pub fn resolve_full_screen_prompt(&mut self, ctx: &egui::Context, allow: bool) {
        let Some(id) = self.pending_full_screen.take() else { return };
        if allow && self.session.get(id).is_some() {
            self.set_full_screen(ctx, true);
        }
    }
}
