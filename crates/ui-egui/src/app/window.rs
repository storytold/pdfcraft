use super::{PdfCraftApp, QuickTool};
use craft::host::target::Backend;
use craft::ui::runtime::windows::WindowHost;
use craft::ui::shell::ScreenMode;
use craft::window::Prefix;
use craft::window::policy::{self, TitleBarPref};
use pdfcraft_engine::DocId;

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
                view.full_screen_fill = self.view_defaults.full_screen_fill;
                view.enter_presentation();
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
