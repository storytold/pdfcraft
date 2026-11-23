use super::state::{DocumentCopyJob, LeftPanel, LinkOrigin, Mode, PasswordPrompt, PendingLink, QuickTool, RecentFile, RightPanel, UnlockPrompt};
#[cfg(not(target_arch = "wasm32"))]
use super::state::{OpenOutcome, OpenRequest, PendingRemoteLink};
use super::*;

use std::hash::Hash;
use std::path::PathBuf;
use std::sync::Arc;

use craft::host::queue::{JobQueue, QueueOptions};
use craft::host::{Host, Wake};
use craft::platform::autosave::{Autosave, RecoveryStore};
use craft::platform::clipboard::Clipboard;
use craft::platform::open::FileIntake;
use craft::ui::runtime::env::{RootEnv, Scale};
use craft::ui::shell::embed;

mod clipboard;
mod commands;
mod control;
mod files;
mod frame;
mod input;
mod keys;
mod links;
pub(crate) mod picks;
mod prefs;
pub(crate) mod reload;
pub mod settings;
mod window;

pub(crate) use clipboard::{ObjectClipMark, copy_refused, print_refused};
pub(crate) use files::same_file;
pub(crate) use prefs::{MAX_AUTHOR_CHARS, date_text_for_pdf};

/// Where the update check asks for the newest release, in place of GitHub (tests).
#[cfg(not(target_arch = "wasm32"))]
pub type UpdateSource = craft::update::updater::CheckSource;

/// The services [`PdfCraftApp::with_host`] builds the app over: the real ones from
/// [`AppServices::system`], or a test's (`Wake::none()`, an inline watch).
pub struct AppServices {
    wake: Wake,
    clipboard: Clipboard,
    recovery_dir: Option<PathBuf>,
    #[cfg(not(target_arch = "wasm32"))]
    updater: craft::update::updater::Updater,
    intake: FileIntake,
    #[cfg(not(target_arch = "wasm32"))]
    watch: craft::platform::watch::FileWatch<DocId>,
}

impl AppServices {
    /// `wake` asks for a frame; `recovery_dir` is where autosaves go (`None`: autosave off);
    /// `update_source` answers the update check in place of GitHub (`None`: GitHub); `intake`
    /// holds the files to open; `watch` reports changes to open documents' files.
    pub fn new(
        wake: Wake,
        clipboard: Clipboard,
        recovery_dir: Option<PathBuf>,
        #[cfg(not(target_arch = "wasm32"))] update_source: Option<UpdateSource>,
        intake: FileIntake,
        #[cfg(not(target_arch = "wasm32"))] watch: craft::platform::watch::FileWatch<DocId>,
    ) -> Self {
        #[cfg(not(target_arch = "wasm32"))]
        let updater = {
            let updater = craft::update::updater::Updater::new(&UPDATE_APP, pdfcraft_engine::build_info::version().to_string());
            match update_source {
                Some(source) => updater.with_source(source),
                None => updater,
            }
        };
        Self {
            wake,
            clipboard,
            recovery_dir,
            #[cfg(not(target_arch = "wasm32"))]
            updater,
            intake,
            #[cfg(not(target_arch = "wasm32"))]
            watch: reload::document_watch(watch),
        }
    }

    /// These services with `updater` in place of the one over the update source: an updater
    /// whose update has already installed (craft-update's `Updater::ready_for_tests`).
    #[cfg(not(target_arch = "wasm32"))]
    pub fn with_updater(mut self, updater: craft::update::updater::Updater) -> Self {
        self.updater = updater;
        self
    }

    /// The desktop's or the browser's own services, waking `ctx`, reading the files to open from
    /// `intake`.
    pub fn system(ctx: &egui::Context, intake: FileIntake) -> Self {
        let wake = wake_of(ctx);
        #[cfg(not(target_arch = "wasm32"))]
        {
            let clipboard = Clipboard::new(craft::platform::clipboard::SystemClipboard::new());
            let watch = craft::platform::watch::FileWatch::new(wake.clone());
            Self::new(wake, clipboard, recovery::prepare_platform_recovery_dir(), None, intake, watch)
        }
        // PLAN: assumed clipboard::web_clipboard(&egui::Context) -> Clipboard, the browser's event clipboard (pdf-shell-w17).
        #[cfg(target_arch = "wasm32")]
        {
            Self::new(wake, clipboard::web_clipboard(ctx), None, intake)
        }
    }
}

/// A wake that asks `ctx` for a frame.
fn wake_of(ctx: &egui::Context) -> Wake {
    let ctx = ctx.clone();
    Wake::new(move || ctx.request_repaint())
}

/// A queue of the app's over the host's jobs, named `name` for its lanes.
fn queue<K: Clone + Eq + Hash + Send + 'static, R: Send + 'static>(host: &Host, wake: &Wake, name: &'static str) -> JobQueue<K, R> {
    JobQueue::new(host.jobs_shared().clone(), wake.clone(), QueueOptions { name, ..QueueOptions::default() })
}

impl PdfCraftApp {
    /// The window's title and, on the web, the page's, from the active document (#3640). Done in
    /// `logic` too, which also runs while the window or browser tab is hidden: a document opened
    /// then (a `?file=` download finishing) still names the page an embedding host reads.
    fn sync_titles(&mut self, ctx: &egui::Context) {
        // The window shows the active document's name (or title, if it asks for that), with •
        // while it has unsaved changes, as the other Craft apps write it (#3640). Unsaved means
        // what closing would ask about, so a draft not yet applied counts too.
        // In split view it names the main pane's document.
        let front = self.pane_front(panes::Place::Main);
        let dirty = front.is_some_and(|i| self.has_unsaved_work(i));
        let mark = if dirty { " •" } else { "" };
        let titled = |name: Option<String>| {
            name.or_else(|| self.combine_showing().then(|| tl!("Combine files").to_owned()))
                .map_or_else(|| "PdfCraft".to_owned(), |name| format!("{name}{mark} — PdfCraft"))
        };
        let title = titled(front.and_then(|i| crate::canvas::tab_label(self, i)));
        // The web page's title is what an embedding host reads to tell which file opened, so it
        // names the file even when the document asks to show its own title.
        let page_title = titled(front.and_then(|i| self.views.get(i)).and_then(|v| self.session.get(v.id)).map(|d| d.name.clone()));
        if title != self.window_title {
            ctx.send_viewport_cmd(egui::ViewportCommand::Title(title.clone()));
            self.window_title = title;
        }
        if page_title != self.page_title {
            // eframe's web runner ignores Title: set the page's own.
            #[cfg(target_arch = "wasm32")]
            if let Some(document) = web_sys::window().and_then(|w| w.document()) {
                document.set_title(&page_title);
            }
            self.page_title = page_title;
        }
    }

    /// The app over the desktop's own services, waking `ctx`.
    #[cfg(not(target_arch = "wasm32"))]
    pub fn new(ctx: &egui::Context) -> Self {
        let (intake, _) = FileIntake::new(wake_of(ctx));
        Self::with_intake(ctx, intake)
    }

    /// [`Self::new`] reading the files to open from `intake` (another launch's, the Apple events').
    #[cfg(not(target_arch = "wasm32"))]
    pub fn with_intake(ctx: &egui::Context, intake: FileIntake) -> Self {
        let services = AppServices::system(ctx, intake);
        let (files, panels) = craft::dialogs::service::FileService::new(services.wake.clone());
        let host = Host::builder(APP_INFO).files(Arc::new(files)).build();
        let mut app = Self::with_host(Arc::new(host), services);
        app.file_panels = Some(panels);
        app
    }

    /// The app unit tests start from: the default host (threads run its jobs, pickers answer Cancelled), no
    /// wake-ups, no autosave folder, the GitHub update source, a fresh intake and an inline watch.
    #[cfg(all(test, not(target_arch = "wasm32")))]
    pub(crate) fn for_unit_tests() -> Self {
        Self::for_unit_tests_on(Arc::new(Host::builder(APP_INFO).build()))
    }

    /// [`for_unit_tests`](Self::for_unit_tests) over `host`, for a test whose jobs or files are scripted.
    #[cfg(all(test, not(target_arch = "wasm32")))]
    pub(crate) fn for_unit_tests_on(host: Arc<Host>) -> Self {
        let services = AppServices::new(
            Wake::none(),
            Clipboard::new(craft::platform::clipboard::SystemClipboard::new()),
            None,
            None,
            FileIntake::new(Wake::none()).0,
            craft::platform::watch::FileWatch::new(Wake::none()).inline(),
        );
        Self::with_host(host, services)
    }

    /// Installs the framework runtime on a unit test harness's context, as the app's window does,
    /// with a system that opens no OS windows and the interface fonts of English.
    #[cfg(test)]
    pub(crate) fn install_runtime(ctx: &egui::Context) {
        let spec = craft::ui::InstallSpec::new(craft::host::target::ShellInfo::default(), Arc::new(craft::window::system::NoWindows));
        craft::ui::shell::embed::install(ctx, spec, &crate::fonts::ui_fonts(i18n::Lang::EN)).expect("the runtime installs on a fresh context");
    }

    /// The app over `host` and `services`, with no document open: every queue runs on the host's
    /// jobs, so a test passes a host over `ManualJobs` and `ScriptedFiles`.
    pub fn with_host(host: Arc<Host>, services: AppServices) -> Self {
        crate::icons::register();
        let AppServices {
            wake,
            clipboard,
            recovery_dir,
            #[cfg(not(target_arch = "wasm32"))]
            updater,
            intake,
            #[cfg(not(target_arch = "wasm32"))]
            watch,
        } = services;
        let store = recovery_dir.and_then(|dir| match RecoveryStore::open(dir) {
            Ok(store) => Some(Arc::new(store)),
            Err(e) => {
                log::warn!("autosave is off: the recovery folder can't be used: {e}");
                None
            }
        });
        let recovered = store.as_ref().map(|s| s.leftovers()).unwrap_or_default();
        let autosave = Autosave::new(store.is_some().then(|| std::time::Duration::from_secs_f64(recovery::AUTOSAVE_SECS)));
        Self {
            #[cfg(not(target_arch = "wasm32"))]
            // PLAN: assumed open_worker::queue(&Host, Wake) -> JobQueue<u64, ResultKind> (pdf-shell-w13).
            opens: open_worker::queue(&host, wake.clone()),
            #[cfg(not(target_arch = "wasm32"))]
            named_destinations: queue(&host, &wake, "pdfcraft-destinations"),
            #[cfg(not(target_arch = "wasm32"))]
            pending_destinations: Vec::new(),
            pinned_jobs: queue(&host, &wake, "pdfcraft-pinned-folders"),
            recent_check: persist::recent_check::RecentCheck::new(queue(&host, &wake, "pdfcraft-recent-check")),
            text_copies: queue(&host, &wake, "pdfcraft-copy-text"),
            reveal_jobs: queue(&host, &wake, "pdfcraft-reveal"),
            ocr_runs: queue(&host, &wake, "pdfcraft-ocr"),
            ocr_batches: queue(&host, &wake, "pdfcraft-ocr-files"),
            verify_jobs: queue(&host, &wake, "pdfcraft-ocr-verify"),
            action_jobs: queue(&host, &wake, "pdfcraft-actions"),
            space_audit_jobs: queue(&host, &wake, "pdfcraft-space-audit"),
            optimize_jobs: queue(&host, &wake, "pdfcraft-optimize"),
            scan_cleans: queue(&host, &wake, "pdfcraft-scan-clean"),
            export_jobs: queue(&host, &wake, "pdfcraft-export"),
            font_scan: queue(&host, &wake, "pdfcraft-font-scan"),
            print_tasks: print_ui::PrintTasks::new(host.jobs_shared().clone(), wake.clone()),
            host,
            wake,
            intake,
            #[cfg(not(target_arch = "wasm32"))]
            file_panels: None,
            settings: None,
            dialogs: craft::ui::dialog::DialogHost::new(),
            prompts: Default::default(),
            preferences: None,
            shortcuts: None,
            panes: Default::default(),
            dock: side_panels::default_workspace(craft::behavior::DockSide::End),
            rail_side: craft::behavior::DockSide::End,
            session: Session::new(),
            status_captions_lang: None,
            views: Vec::new(),
            closed_tabs: Vec::new(),
            active: None,
            last_frame_active: None,
            mode: Mode::AllTools,
            default_mode: Mode::AllTools,
            view_defaults: Default::default(),
            highlight_fields: true,
            units: Default::default(),
            blank_paper: crate::paper::Paper::system(),
            mode_override: None,
            left: LeftPanel::AllTools,
            last_right: RightPanel::Comments,
            notice_inset: 0.0,
            toolbar_tools_open: true,
            window_narrow: false,
            right_before_read: None,
            comments_panel_closed: false,
            right_panel_closed: false,
            portfolio_initial: None,
            portfolio_seen: None,
            portfolio_panel: None,
            quick_tool: QuickTool::Select,
            full_screen_tool: None,
            comment_prefs: Default::default(),
            recent_colours: Default::default(),
            colour_picks: Default::default(),
            hotbar: Default::default(),
            // A fresh install follows the desktop's appearance (#1248); saved settings override.
            theme_preference: craft::theme::ModeChoice::System,
            suite: craft::theme_store::SuiteClient::memory(),
            reading_colors: Default::default(),
            show_page_edge_shadows: true,
            scroll_speed: scroll_speed::ScrollSpeed::default(),
            invert_middle_drag: false,
            animation_effects: craft::theme::MotionChoice::System,
            language: i18n::AUTO.to_string(),
            ui_scale: None,
            title_bar_choice: Default::default(),
            flatten_fill_sign_on_save: false,
            ask_to_lock_signatures: true,
            placed_signatures: Default::default(),
            pending_lock: None,
            lock_answered: None,
            fill_text_size: fill_sign::TEXT_SIZE,
            fill_tool: fill_sign::FillTool::Signature,
            fill_text_color: [0.0; 3],
            fill_text_border: 0.0,
            fill_text_border_color: None,
            fill_text_border_shape: pdfcraft_engine::BorderShape::Rectangle,
            fill_text_default: fill_sign::TextBorder::NONE,
            #[cfg(not(target_arch = "wasm32"))]
            updater,
            startup_check: false,
            printer_source: None,
            options_source: None,
            default_paper_source: None,
            printer_papers_source: None,
            spooler: None,
            two_sided_source: None,
            palette_open: false,
            all_tools_expanded: false,
            all_tools_full: false,
            all_tools_query: String::new(),
            all_tools_tab: (None, false),
            recent: Vec::new(),
            recent_limit: crate::RECENT_FILES_DEFAULT,
            recent_view: craft::ui::shell::RecentView::List,
            recent_sort: craft::ui::shell::RecentSort::Opened,
            reopen_last_session: false,
            restore_last_view: true,
            last_views: Vec::new(),
            last_session: Default::default(),
            session_keeper: Default::default(),
            reload_changed_files: true,
            update_check_at_start: true,
            show_secured_notice: true,
            text_links: true,
            always_default_view: false,
            #[cfg(not(target_arch = "wasm32"))]
            watch,
            #[cfg(not(target_arch = "wasm32"))]
            reload_request: None,
            #[cfg(not(target_arch = "wasm32"))]
            pending_opens: Vec::new(),
            #[cfg(not(target_arch = "wasm32"))]
            password_retry: None,
            #[cfg(not(target_arch = "wasm32"))]
            next_open_token: 0,
            #[cfg(not(target_arch = "wasm32"))]
            launch_layout: None,
            #[cfg(not(target_arch = "wasm32"))]
            launch_display_target: None,
            picks: Default::default(),
            save_flow: Default::default(),
            pending_save: None,
            pinned: Default::default(),
            pinned_tools: Vec::new(),
            script_messages: std::collections::VecDeque::new(),
            script_messages_dropped: 0,
            script_shown: None,
            posted_notices: Vec::new(),
            last_edit_refusal: None,
            notices: craft::ui::notices::NoticeCenter::new(),
            progress_requests: Vec::new(),
            progress_cards: Default::default(),
            password_prompt: None,
            unlock_prompt: None,
            screen_mode: Default::default(),
            host_dirty: None,
            close_guard: None,
            close_guard_dialog: None,
            props_session: None,
            summary_sort: Default::default(),
            summarize_dialog: None,
            sign_draft: None,
            presign: None,
            digital_ids: Vec::new(),
            sig_open: Default::default(),
            a11y_options: a11y_ui::A11yOptions::default(),
            ocr_draft: ocr_ui::OcrDraft::default(),
            ocr_run: None,
            ocr_batch: None,
            #[cfg(target_arch = "wasm32")]
            ocr_web_batch: None,
            #[cfg(target_arch = "wasm32")]
            ocr_web_waiting: None,
            ocr_verify: ocr_verify::OcrVerify::default(),
            panicking_frames: 0,
            frame_panicked: false,
            logic_without_ui: false,
            #[cfg(test)]
            panic_next: None,
            custom_actions: Vec::new(),
            action_run: None,
            pdfa: Default::default(),
            compare_old: None,
            compare_pages: String::new(),
            compare: None,
            js_console: Default::default(),
            #[cfg(not(target_arch = "wasm32"))]
            script_pending: None,
            js_dialog: None,
            a11y: a11y_ui::A11yState::default(),
            tags_state: Default::default(),
            a11y_skipped: Default::default(),
            os_key_store_ids: false,
            cert_viewer: None,
            space_audit: Default::default(),
            space_audit_of: None,
            combine_draft: Vec::new(),
            combine_conflicts: Default::default(),
            combine_tab: Default::default(),
            combine_columns: Default::default(),
            combine_view: Default::default(),
            combine_zoom: 1.0,
            organize_zoom: 1.0,
            image_import: None,
            custom_stamps: Vec::new(),
            stamp_dialog: None,
            optimize_run: None,
            scan_clean_draft: scan_clean_ui::ScanCleanDraft::default(),
            scan_clean_run: None,
            page_clipboard: None,
            comment_clipboard: None,
            object_clipboard: None,
            object_clip_mark: Default::default(),
            last_snapshot: None,
            last_snapshot_of: None,
            last_copied_image: None,
            last_copied_file: None,
            clipboard,
            page_props_draft: None,
            form_preview: false,
            recovery: store,
            recovered,
            autosave,
            recovered_paths: Default::default(),
            native_menu: None,
            window_title: String::new(),
            page_title: String::new(),
            control: None,
            control_server: None,
            control_epoch: 0,
            bookmark_clipboard: None,
            word_count_options: Default::default(),
            printer_marks_draft: Default::default(),
            convert_colours_draft: Default::default(),
            hairlines_draft: Default::default(),
            text_style_draft: Default::default(),
            boxes_draft: Default::default(),
            marks_draft: Default::default(),
            marks_preview: None,
            attachment_description: None,
            export_draft: Default::default(),
            export_status: None,
            signature: None,
            initials: None,
            sign_graphic: sign_ui::SignGraphic::Name,
            comment_props: None,
            redact_prefs: RedactPrefs::default(),
            redact_defaults: RedactPrefs::default(),
            redact_props: None,
            redact_search: RedactSearchDraft::default(),
            redact_search_open: false,
            hidden_draft: None,
            print_draft: PrintDraft::default(),
            text_style: content_ui::default_style(),
            edit_more: false,
            replace_source: None,
            #[cfg(not(target_arch = "wasm32"))]
            document_fonts: None,
            pending_urls: Vec::new(),
            startup_superseded: false,
            startup_files: Vec::new(),
            community_card_closed: false,
            pending_link: None,
            pending_window: None,
            pending_remote_link: None,
        }
    }

    /// Fields are being edited (Prepare a form or its Calculation order is open, or a field tool
    /// is picked), unless the form is being previewed.
    pub fn is_preparing(&self) -> bool {
        ((self.left_open() && matches!(self.left, LeftPanel::Tool("form") | LeftPanel::CalculationOrder))
            || matches!(self.quick_tool, QuickTool::Field(_)))
            && !self.form_preview
    }

    pub fn active_ids(&self) -> Option<(usize, DocId)> {
        self.active.and_then(|i| self.views.get(i).map(|v| (i, v.id)))
    }

    /// The right panel showing, as the dock has it.
    pub(crate) fn right(&self) -> Option<RightPanel> {
        side_panels::right_of(&self.dock)
    }

    /// Whether the dock has the Tools panel open; Read mode and the expanded All tools view hide it without closing it.
    pub fn left_open(&self) -> bool {
        self.dock.layout.contains(&side_panels::PanelId::Tools)
    }

    /// Opens or closes the Tools panel in the dock, the one record of it (L-136).
    pub fn set_left_open(&mut self, open: bool) {
        side_panels::set_panel_visible(self, side_panels::PanelId::Tools, open);
    }

    /// The app's root values for this frame: appearance mode, motion, interface scale and the
    /// interface language's direction. The one place the app's `RootEnv` is built.
    pub(crate) fn root_env(&self, ctx: &egui::Context) -> RootEnv {
        static SIZES: std::sync::OnceLock<Arc<craft::theme::AppSizes>> = std::sync::OnceLock::new();
        let sizes = SIZES.get_or_init(|| {
            let mut sizes = craft::theme::AppSizes::default();
            sizes.all.insert("typography.display.size".into(), craft::theme::SizeValue::Number(24.0));
            sizes.all.insert("typography.display.line_height".into(), craft::theme::SizeValue::Number(29.0));
            Arc::new(sizes)
        });
        let lang = i18n::Lang::from_pref(&self.language);
        let direction = if lang.rtl() { craft_i18n::Direction::Rtl } else { craft_i18n::Direction::Ltr };
        let system = craft::ui::runtime::env::system_text_scale(ctx).unwrap_or(1.0);
        let native = ctx.native_pixels_per_point().unwrap_or(1.0);
        let mut root = RootEnv::new()
            .mode(self.theme_preference)
            .density(craft::theme::Size::Comfortable)
            .direction(direction)
            .scale(Scale::new(crate::ui_scale::zoom(self.ui_scale, system, native)));
        if let Ok(theme) = craft::theme::ThemeId::parse("craft.graphite") {
            root.set_theme(theme);
        }
        root.sizes = Arc::clone(sizes);
        // Every parameter dialog lays out as a document dialog: inline captions, shaded groups, buttons across the foot.
        root.values.set(craft::ui::patterns::form_dialogs::DialogLayout(craft::ui::patterns::form_dialogs::ParamLayout::Document));
        root.motion = self.animation_effects;
        // Today is the session's day, the one its date formats and Fill & Sign's Date tool use: the calendars ring it
        // and pick it.
        let (year, month, day) = self.session.today();
        match craft_i18n::CalendarDate::new(year, u8::try_from(month).unwrap_or(0), u8::try_from(day).unwrap_or(0)) {
            Some(today) => root.today(today),
            None => root,
        }
    }

    /// Read mode and full screen hide the hotbar (the quick tools), and its tools window with it.
    pub(crate) fn hotbar_hidden(&self) -> bool {
        self.mode == crate::Mode::Read || self.screen_mode.body_only()
    }

    /// The pages are covered, and no page key reaches them, a menu click's included: by a dialog or
    /// prompt (`modal_open`), by the palette, or by the OCR Verify screen, which takes the pages'
    /// place.
    pub(crate) fn covers_pages(&self) -> bool {
        // Full screen draws the page alone: not the palette, nor the OCR Verify screen it was
        // entered over. Dialogs and prompts show there too.
        let shown = !(self.screen_mode.body_only() && self.active.is_some());
        self.modal_open() || self.prompts.full_screen.is_some() || (self.palette_open || self.ocr_verify.open) && shown
    }

    /// A dialog or prompt is open over the window: the keyboard is its own, and nothing may act
    /// on the document underneath it. The full-screen prompt covers the pages (`covers_pages`)
    /// while leaving the menus on, as production's `pending_full_screen` did.
    pub(crate) fn modal_open(&self) -> bool {
        // Dialogs other than the full-screen prompt (which leaves the menus on) and the startup recovery offer.
        let fs_open = self.prompts.full_screen.is_some_and(|(id, _)| self.dialogs.open_dialogs().iter().any(|d| d.id == id));
        self.dialogs.open_dialogs().len() > usize::from(fs_open) + usize::from(self.dialogs.is_open::<craft::ui::dialog::RecoveryDialog>())
            || self.password_prompt.is_some()
            || self.unlock_prompt.is_some()
            || self.pending_link.is_some()
            || self.pending_remote_link.is_some()
            || self.preferences.is_some()
            || self.shortcuts.is_some()
            || self.redact_props.is_some()
            || self.redact_search_open
            || self.hidden_draft.is_some()
            || self.sign_draft.is_some()
            || self.cert_viewer.is_some()
            || self.js_dialog.is_some()
            || self.comment_props.is_some()
            || self.stamp_dialog.is_some()
    }

    /// Show `msg` as a notice.
    pub fn notify(&mut self, msg: impl Into<String>) {
        self.posted_notices.push(craft::ui::notices::PostedNotice::new(msg));
    }

    /// Notify with a static message in the UI language.
    pub fn notify_tr(&mut self, text: &str) {
        self.notify(tl!(text).to_string());
    }

    /// Notify with an error's own text. Engine and OS messages are not translated: matching their
    /// English wording would break silently whenever it changes.
    pub fn notify_error(&mut self, error: impl std::fmt::Display) {
        self.notify(error.to_string());
    }

    /// Notify with a `{name}`-style template in the UI language (placeholders filled once).
    pub fn notify_fmt(&mut self, template: &str, args: &[(&str, &str)]) {
        self.notify(i18n::fmt(tl!(template), args));
    }

    /// `true` while any open document still waits for page renders (used by headless capture).
    pub fn render_pending(&self) -> bool {
        self.views.iter().any(|v| v.render_pending())
    }

    /// Check whether a tool group is pinned to Home.
    pub fn is_tool_pinned(&self, id: &str) -> bool {
        self.pinned_tools.iter().any(|t| t == id)
    }

    /// Pin a tool group to Home. Returns true if newly pinned.
    pub fn pin_tool(&mut self, id: &str) -> bool {
        if pdfcraft_engine::catalog::group(id).is_none() {
            return false;
        }
        if !self.is_tool_pinned(id) {
            self.pinned_tools.push(id.to_string());
            if let Some(g) = pdfcraft_engine::catalog::group(id) {
                self.notify_fmt("Pinned {tool} to Home", &[("tool", tl!(g.label))]);
            }
            return true;
        }
        false
    }

    /// Unpin a tool group from Home.
    pub fn unpin_tool(&mut self, id: &str) {
        if let Some(pos) = self.pinned_tools.iter().position(|t| t == id) {
            self.pinned_tools.remove(pos);
            if let Some(g) = pdfcraft_engine::catalog::group(id) {
                self.notify_fmt("Unpinned {tool} from Home", &[("tool", tl!(g.label))]);
            }
        }
    }

    /// Toggle a tool group's pinned state on Home. Returns whether it is now pinned.
    pub fn toggle_pin_tool(&mut self, id: &str) -> bool {
        if self.is_tool_pinned(id) {
            self.unpin_tool(id);
            false
        } else {
            self.pin_tool(id)
        }
    }
}

impl eframe::App for PdfCraftApp {
    /// The runtime's fired commands and this frame's root values, before egui reads the input.
    fn raw_input_hook(&mut self, ctx: &egui::Context, raw: &mut egui::RawInput) {
        self.prepare_raw_input(ctx, raw);
    }

    /// The preferences go to the settings store, which writes them when they changed.
    fn save(&mut self, _storage: &mut dyn eframe::Storage) {
        self.save_settings();
    }

    /// Starts an update that waits for the exit. On macOS the process ends before the app is dropped.
    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        #[cfg(not(target_arch = "wasm32"))]
        self.updater.start_pending_update();
    }

    /// Each frame's work before drawing: the update check, opens, control-channel requests, the
    /// background jobs' results, and last the close path (the "save changes?" questions).
    ///
    /// Last-resort guard (AGENTS.md §4, #3914): a panic in a frame is reported and the app, with
    /// every open tab, keeps running; see `frame_guard`.
    fn logic(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.begin_logic();
        #[cfg(not(target_arch = "wasm32"))]
        self.update_logic(ctx);
        self.guarded_frame(ctx, |app| app.logic_unguarded(ctx));
        self.run_close_path(ctx);
    }

    fn ui(&mut self, ui: &mut egui::Ui, frame: &mut eframe::Frame) {
        embed::before_ui(ui.ctx(), frame);
        self.begin_ui();
        // The native file panels the host's file service asked for, over this window.
        #[cfg(not(target_arch = "wasm32"))]
        if let Some(panels) = self.file_panels.as_mut() {
            panels.pump_with(&mut self.dialogs, Some(&*frame));
        }
        let ctx = ui.ctx().clone();
        self.guarded_frame(&ctx, |app| app.ui_unguarded(ui));
        self.end_ui();
    }
}

impl PdfCraftApp {
    fn logic_unguarded(&mut self, ctx: &egui::Context) {
        #[cfg(test)]
        self.injected_panic(crate::frame_guard::FramePart::Logic);
        #[cfg(not(target_arch = "wasm32"))]
        self.poll_script_dialog(ctx);
        // Notices raised outside `ui` (opened files, the control channel) translate too.
        let lang = i18n::Lang::from_pref(&self.language);
        i18n::set_current(lang);
        self.apply_status_captions(lang);
        // Form date fields name months in the date language (Preferences ▸ Date format), which
        // follows the interface language until one is picked (#5303).
        pdfcraft_engine::dates::use_form_date_language(self.session.date_language().unwrap_or(lang.code()));
        #[cfg(not(target_arch = "wasm32"))]
        {
            self.poll_pending_opens(ctx);
            self.poll_pending_destinations();
        }
        self.check_restored_recent();
        // Showing a document hides the Combine files tab.
        if self.active.is_some() && self.combine_tab.focused {
            self.combine_tab.focused = false;
        }
        // The page shown large, while it shows: leaving the tab, whichever way, closes it.
        if !self.combine_showing() {
            self.close_combine_preview();
        }
        // The picks first: a file the user picked opens ahead of a startup file that arrived with it, which then waits
        // for that open and goes to a background tab.
        self.process_picks(ctx);
        // PLAN: assumed PdfCraftApp::open_intake(&mut self, &egui::Context), which opens what the intake holds (pdf-shell-w13).
        self.open_intake(ctx);
        self.present_portfolios(ctx);
        if let Some((_, id)) = self.active_ids() {
            let out = self.session.take_js_output(id);
            self.handle_js(ctx, id, out);
        }
        self.script_tick(ctx);
        self.control_logic(ctx);
        self.autosave_tick(ctx, web_time::Instant::now());
        // Reload documents whose files a build rewrote (#431).
        #[cfg(not(target_arch = "wasm32"))]
        self.watch_files(ctx);
        if self.host_dirty.is_some() {
            // An OCR review not yet accepted or rejected is work the page would lose, too.
            let dirty = self.work_at_risk();
            if let Some(report) = self.host_dirty.as_mut() {
                report(dirty);
            }
        }
        // The browser asks before a reload or a tab close loses that work.
        #[cfg(target_arch = "wasm32")]
        craft::quit::web::hold_leave(self.work_at_risk());
        self.poll_print();
        // Scrolling and group-drag previews are transient: never resume after changing tabs,
        // opening a modal/palette, or returning to a window that lost focus.
        let blocked = self.modal_open() || self.palette_open || !ctx.input(|i| i.focused);
        for (index, view) in self.views.iter_mut().enumerate() {
            let shut = blocked || self.active != Some(index);
            view.session.objects.set_input_blocked(shut);
            if shut {
                view.left_screen();
            }
            // Edit text and Add text's drags end while their input is blocked rather than wait, to
            // land wherever the pointer is once it opens again.
            if shut && matches!(self.quick_tool, QuickTool::EditText | QuickTool::AddText) {
                view.cancel_pointer_gestures(ctx);
            }
        }
        self.process_pending_edits(ctx);
        self.poll_export();
        self.auto_ocr_scanned_page(ctx);
        self.poll_ocr(ctx);
        self.poll_optimize();
        self.poll_scan_clean();
        self.poll_action();
        // The OCR Verify screen's runs and re-reads may have finished while no frame was drawn.
        let verify = self.verify_jobs.counts();
        if verify.queued + verify.running + verify.finished > 0 {
            workflows::ocr_verify::with_state(self, workflows::ocr_verify::OcrVerify::poll);
        }
        // A run or re-read asked for while the web build downloaded the models starts when they land.
        #[cfg(target_arch = "wasm32")]
        if self.ocr_verify.awaiting_models.is_some() {
            workflows::ocr_verify::with_state(self, workflows::ocr_verify::OcrVerify::poll_models);
        }
        self.settle_presign(ctx);
        self.sync_titles(ctx);
    }

    fn ui_unguarded(&mut self, ui: &mut egui::Ui) {
        #[cfg(test)]
        self.injected_panic(crate::frame_guard::FramePart::Ui);
        let ctx = ui.ctx().clone();
        // The chosen language becomes the current one; when it changes, its fonts are installed with it (the
        // runtime installed the starting language's fonts at creation, L-227).
        let lang = i18n::Lang::from_pref(&self.language);
        if i18n::current().code() != lang.code() {
            i18n::set_current(lang);
            crate::fonts::ui_fonts(lang).install(&ctx);
        }
        // Another tool lets go of a click whose picture is still being found.
        if self.quick_tool != QuickTool::Select {
            for view in &mut self.views {
                view.session.pending_pick = None;
            }
        }
        panes::frame(self, &ctx);
        // Forms' scripts read Highlight fields as `app.runtimeHighlight`.
        let highlight = self.active.and_then(|i| self.views.get(i)).map_or(self.highlight_fields, |v| v.session.highlight_fields);
        self.session.set_field_highlight(highlight);
        self.sync_titles(&ctx);
        // The accessible window takes the same name, so a screen reader (AT-SPI on Linux, UIA on
        // Windows) can tell it apart (#4614). egui builds the root node afresh every pass.
        ctx.accesskit_node_builder(egui::accesskit_root_id(), |root| root.set_label(self.window_title.as_str()));
        // PLAN: assumed chrome::shell(&mut PdfCraftApp, &mut egui::Ui), the AppShell pass: menu bar, tab strip, mode bar, window edges and captions (pdf-chrome-w10).
        // The shell's body is the dock, whose document panel draws the page area or what takes its place
        // (`side_panels::document_view`).
        chrome::shell(self, ui);
        // The keys the app reads itself, once the shell's pass has fired the commands and the tool host has taken
        // its tools' keys; leaving full screen is followed in this same pass.
        self.shortcuts(&ctx);
        self.follow_screen_mode();
        if !self.full_screen_document() && !self.ocr_verify.open {
            // FitWindow and CenterWindow, once the document just opened is laid out.
            self.window_preferences_frame(&ctx);
            self.process_pending_edits(&ctx);
        }
        // Dialogs and notices show in full screen too: a refused field value or a failed save must be seen.
        dialogs::show(self, ui);
        crate::notices::show(self, ui);
        // A notice or progress change made after the notices' turn (a finished job's result) is posted next pass: ask for it.
        if !self.posted_notices.is_empty() || !self.progress_requests.is_empty() {
            ctx.request_repaint();
        }
        for view in &mut self.views {
            crate::canvas::organize::finish_reveal(view, &ctx);
        }
        // Screenshots are issued only after a settled pass that really drew the window.
        self.control_ui_pass(&ctx);
        // Addresses `open_url` queued: the runtime opens them in the browser.
        for url in self.pending_urls.drain(..) {
            ctx.open_url(egui::OpenUrl::new_tab(url));
        }
    }

    /// Whether the window shows the active document alone on black, as Acrobat's Full Screen Mode (Esc or ⌘L to
    /// leave).
    pub(crate) fn full_screen_document(&self) -> bool {
        self.screen_mode.body_only() && self.active.is_some()
    }

    /// Puts away the views the front tab no longer shows, before the document panel draws.
    pub(crate) fn settle_document_view(&mut self) {
        // Another mode or the Home tab puts the OCR Verify screen away, as Exit does; Correct recognized text brings
        // it back with its review as it was.
        if self.ocr_verify.open && (self.active.is_none() || self.ocr_verify.shown_in.is_some_and(|m| m != self.mode)) {
            self.ocr_verify.open = false;
        }
        self.ocr_verify.shown_in = self.ocr_verify.open.then_some(self.mode);
        // Whichever way another tab came to the front, it shows instead of the expanded view.
        if self.all_tools_full && self.all_tools_tab != self.shown_tab() {
            self.all_tools_full = false;
        }
    }
}
