//! pdfcraft-ui-egui — the first PdfCraft shell (L7).
//!
//! Layout grammar follows plan/acrobat/02-ui-ux.md §1: tab strip, mode bar, left tool panel,
//! floating quick-action bar, document area, right panel + right rail with page navigation.
//! Everything here is presentation: documents, rendering and the tool catalogue live in
//! `pdfcraft-engine`.

#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]
// The settings object in `persist` is one `json!` literal past the default limit of 128.
#![recursion_limit = "256"]

/// PdfCraft as craft-update knows it, with its 128 px icon and accent colour: the update
/// prompt shows both, and an AppImage's menu entry gets the icon.
#[cfg(not(target_arch = "wasm32"))]
pub const UPDATE_APP: craft_update::app::App = craft_update::app::App {
    icon_png: Some(include_bytes!("../../../assets/app-icon/hicolor/128x128/apps/ai.storyteller.pdfcraft.png")),
    accent: Some([0xb7, 0x02, 0x28]),
    ..craft_update::suite::PDFCRAFT
};

use crate::control::Host as _;

/// Translate an English UI string into the current language (see [`i18n`]).
macro_rules! tl {
    ($s:expr) => {
        $crate::i18n::t($s)
    };
}

/// [`tl!`] for a label whose meaning depends on where it appears ("Type" is a column and a
/// button): a catalog can translate it under `context`, otherwise the plain translation is used.
macro_rules! tl_ctx {
    ($context:expr, $s:expr) => {
        $crate::i18n::tr_ctx($crate::i18n::current(), $context, $s)
    };
}
pub mod canvas;
mod chrome;
pub use combine_grid::ThumbState as CombineThumb;
pub use combine_ui::{Columns as CombineColumns, CombineView, Lock as CombineLock, SortKey};
mod commands;
pub mod control;
pub use control::PdfCraftControl;
mod menus;
/// Header & footer / watermark / background dialog types (tests and automation).
pub mod marks {
    pub use crate::marks_ui::{MarksDraft, PageRange, Subset};
}
pub use create_ui::Clip;
pub use link_ui::LinkDraft;
pub use optimize_ui::{OptimizeDraft, OptimizeTab};
pub use sign_ui::{DigitalIdEntry, SignDraft, SignGraphic, SignStep};
mod bidi;
mod dialogs;
pub use edit_text_ui::CropMode;
mod editing;
mod home;
mod panels;
pub use print_ui::{Handling as PrintHandling, OptionsSource, PrintDraft, PrinterSource, Which as PrintWhich, system_printers};
pub use redact_ui::{HiddenDraft, PagesDraft as RedactPagesDraft, RedactPrefs, SearchDraft as RedactSearchDraft, SearchMode as RedactSearchMode};
pub mod i18n;
pub mod theme;
mod widgets;

use pdfcraft_engine::{DocId, Session};

pub use canvas::{DocView, RasterMemory};
pub use editing::{CloseRequest, SaveTarget};
pub use files::{ExtractDraft, FilePurpose, FileRequest, RotateDraft, SplitDraft, SplitMode, SplitPlan};
pub use recovery::{AUTOSAVE_SECS, RecoveryMeta, RecoveryStore};
use theme::{ThemeKind, ThemePreference};

mod app;
mod hooks;
mod persist;
mod platform;
mod state;
mod tools;
mod workflows;

#[cfg(not(target_arch = "wasm32"))]
pub(crate) use app::same_file;
pub(crate) use app::{MAX_AUTHOR_CHARS, copy_refused, date_text_for_pdf, print_refused};
use canvas::autoscroll;
use canvas::crop;
use canvas::fling;
use canvas::image_pick;
use canvas::page_cursor;
use canvas::page_pointer;
use canvas::page_router;
pub use canvas::scroll_speed;
use canvas::signature_drag;
use canvas::wheel_boost;
use canvas::wheel_pager;
use canvas::zoom_snap;
use dialogs::appearance;
pub use dialogs::bookmark_styles_ui;
use dialogs::comment_props;
use dialogs::credits;
use dialogs::marks_ui;
use dialogs::pageboxes;
use dialogs::palette;
use dialogs::protect;
use dialogs::standards_ui;
use dialogs::word_count_ui;
pub use home::folders_ui;
pub use hooks::{FailedInbox, HostDirtyFn, HostSaveFn, Inbox, OsEvent, OsEventsFn};
use menus::tab_context;
use panels::all_tools_ui;
use panels::comments_panel;
use panels::format_text_ui;
use panels::organize_panel;
use panels::page_selection_ui;
use panels::search_ui;
pub use panels::side_panels;
pub use persist::last_session;
pub use persist::last_view;
use persist::recovery;
pub use persist::units;
#[cfg(not(target_arch = "wasm32"))]
pub use persist::window_state;
use platform::drag_pointer;
#[cfg(not(target_arch = "wasm32"))]
pub use platform::file_watch;
#[cfg(target_os = "macos")]
use platform::macos_association;
pub use platform::motion;
#[cfg(not(target_arch = "wasm32"))]
use platform::open_worker;
#[cfg(not(target_arch = "wasm32"))]
use platform::pickers;
pub use platform::portable;
#[cfg(not(target_arch = "wasm32"))]
use platform::system_fonts;
pub use platform::ui_scale;
pub use platform::updates;
pub use platform::window_controls;
#[cfg(test)]
use state::CopyPage;
#[cfg(not(target_arch = "wasm32"))]
use state::OpenRequest;
pub use state::{
    CustomDraft, Dialog, DuplicateDraft, LeftPanel, LinkOrigin, Mode, NumberDraft, OpenOutcome, PageClip, PagePropsDraft, PasswordPrompt,
    PendingLink, PendingRemoteLink, PropsTab, QuickTool, RecentFile, RightPanel, UnlockPrompt,
};
use state::{DocumentCopyJob, GridFile};
pub use tools::bulk_fields;
use tools::calc_order_ui;
use tools::camera_ui;
pub use tools::comments;
use tools::content_ui;
use tools::edit_text_ui;
pub use tools::field_table_ui;
pub use tools::fill_sign;
pub use tools::forms_ui;
use tools::link_ui;
use tools::measure_ui;
use tools::object_ui;
pub use tools::prepare;
use tools::redact_ui;
use tools::sign_ui;
use tools::stamps_ui;
use tools::tags_ui;
use widgets::icon_data;
pub use widgets::icons;
use workflows::a11y_ui;
use workflows::actions_ui;
use workflows::combine_grid;
use workflows::combine_ui;
use workflows::compare_ui;
use workflows::create_ui;
use workflows::export_ui;
use workflows::files;
use workflows::js_ui;
use workflows::ocr_ui;
use workflows::optimize_ui;
use workflows::print_ui;
use workflows::web_print;

/// Files dropped on a document's page grid or its Pages panel.
struct GridDrop {
    doc: pdfcraft_engine::DocId,
    /// Dropped on the Pages panel (its gap decides where they go), not the page grid.
    panel: bool,
    files: Vec<GridFile>,
    /// When to stop waiting for the pointer (monotonic: `logic` may run without a UI pass).
    deadline: web_time::Instant,
}

/// A whole-document copy reading its pages off the UI thread (in place on the web, which has no
/// threads). Dropping it (a new copy, the document closed) stops the reading.
struct DocumentCopy {
    doc: DocId,
    rx: std::sync::mpsc::Receiver<String>,
    cancel: std::sync::Arc<std::sync::atomic::AtomicBool>,
}

impl DocumentCopy {
    fn start(doc: DocId, job: DocumentCopyJob, ctx: &egui::Context) -> DocumentCopy {
        let (tx, rx) = std::sync::mpsc::channel();
        let cancel = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        #[cfg(not(target_arch = "wasm32"))]
        {
            let (stop, ctx) = (cancel.clone(), ctx.clone());
            let spawned = std::thread::Builder::new().name("copy-document-text".into()).spawn(move || {
                if let Some(text) = job.text(&stop) {
                    let _ = tx.send(text);
                    ctx.request_repaint();
                }
            });
            if let Err(e) = spawned {
                log::warn!("copying the document's text: {e}");
            }
        }
        #[cfg(target_arch = "wasm32")]
        {
            let _ = ctx;
            if let Some(text) = job.text(&cancel) {
                let _ = tx.send(text);
            }
        }
        DocumentCopy { doc, rx, cancel }
    }
}

impl Drop for DocumentCopy {
    fn drop(&mut self) {
        self.cancel.store(true, std::sync::atomic::Ordering::Relaxed);
    }
}

#[cfg(not(target_arch = "wasm32"))]
struct PendingOpen {
    token: u64,
    receiver: std::sync::mpsc::Receiver<open_worker::ResultMessage>,
    name: String,
    path: Option<String>,
    stamp: Option<file_watch::Stamp>,
    activate: bool,
    after_open: Option<pdfcraft_render::RemoteDestination>,
    recovered: Option<RecoveryMeta>,
    then: Option<&'static str>,
    display_override: Option<(canvas::PageLayout, bool)>,
}

pub struct PdfCraftApp {
    /// Tools side and pane widths; which panes are open is `left_open` and `right`.
    pub side_panels: side_panels::SidePanels,
    pub session: Session,
    pub views: Vec<DocView>,
    pub(crate) closed_tabs: Vec<String>,
    /// `None` shows the Home tab.
    pub active: Option<usize>,
    pub mode: Mode,
    /// Workspace used for newly opened PDFs; independent of PDF Initial View metadata.
    pub default_mode: Mode,
    /// Page display and zoom for newly opened PDFs that don't ask for their own (Preferences ▸
    /// Documents and view). Continuous scrolling at fit width by default, which never snaps
    /// between pages.
    pub view_defaults: canvas::ViewDefaults,
    /// The unit page sizes and margins are shown in (Preferences ▸ Documents and view).
    pub units: units::Unit,
    /// The Preferences ▸ Date format box while it's being edited, or while it holds an invalid
    /// pattern (the session keeps the last valid one).
    pub date_format_draft: Option<String>,
    /// Explicit CLI/control mode lasts for this session and is never persisted.
    mode_override: Option<Mode>,
    pub left: LeftPanel,
    pub left_open: bool,
    pub right: Option<RightPanel>,
    /// The side panel F4 reopens: the one open last.
    pub(crate) last_right: RightPanel,
    /// The right panel that was open when the user switched to Read mode, which starts with the
    /// panes closed; leaving Read mode brings it back unless the user opened or closed a pane in
    /// Read mode meanwhile.
    right_before_read: Option<RightPanel>,
    /// The user closed the Comments panel, so picking a comment tool leaves it closed until they
    /// open it again (#225). Remembered across restarts.
    pub comments_panel_closed: bool,
    /// The user closed the right panel, so opening a document leaves it closed instead of
    /// showing its comments or bookmarks (#1590). Remembered across restarts.
    pub right_panel_closed: bool,
    pub quick_tool: QuickTool,
    /// The tool to go back to when Space, held for a temporary Hand, is released.
    space_hand: Option<QuickTool>,
    /// The tool before full screen, given back on leaving it (see [`Self::apply_full_screen`]).
    full_screen_tool: Option<QuickTool>,
    /// Comment author, per-tool colours and widths, pin.
    pub comment_prefs: comments::CommentPrefs,
    /// Resolved colours, including the current OS theme when following the system.
    pub theme: ThemeKind,
    pub theme_preference: ThemePreference,
    /// The reading-colours preference (Preferences ▸ Reading colours): applied to every open
    /// document, and documents open with it (Acrobat's Replace Document Colors).
    pub reading_colors: pdfcraft_render::ReadingColors,
    /// App-wide page decoration only; never part of a PDF edit or render request.
    pub show_page_edge_shadows: bool,
    /// The Linux portal is enabled only by the native host, and starts on the first System choice.
    #[cfg(target_os = "linux")]
    desktop_theme: Option<pdfcraft_platform::desktop_theme::DesktopTheme>,
    #[cfg(target_os = "linux")]
    desktop_theme_enabled: bool,
    /// How far a mouse-wheel notch scrolls.
    pub scroll_speed: scroll_speed::ScrollSpeed,
    /// Preferences ▸ Animation effects: on, off, or as the system's animation setting says.
    pub animation_effects: motion::AnimationEffects,
    /// Whether the system asked for no animations, and how the desktop app asks it.
    pub motion: motion::Motion,
    /// Interface language preference: `auto` (follow the system) or a code from [`i18n::LANGUAGES`].
    pub language: String,
    /// Preferences ▸ Interface size as a factor, or `None` to follow [`Self::system_text_scale`].
    pub ui_scale: Option<f32>,
    /// The desktop's text scaling (GNOME's text scaling factor), which the desktop app reads at
    /// launch. 1.0 elsewhere, so tests and screenshots don't depend on the machine.
    pub system_text_scale: f32,
    /// The interface size last given to egui ([`ui_scale::sync`]).
    ui_scale_applied: Option<f32>,
    /// Preferences: bake Fill & Sign marks into the page when saving. Off, so a normal save stays editable.
    pub flatten_fill_sign_on_save: bool,
    /// Point size for new Fill & Sign text and dates, kept across launches.
    pub fill_text_size: f64,
    /// Colour for new Fill & Sign text and dates, kept across launches.
    pub fill_text_color: [f64; 3],
    pub dialog: Option<Dialog>,
    /// How to ask for the latest release (the desktop app sets it; see `updates`).
    pub(crate) updates: updates::Updates,
    /// Lists the Print dialog's printers in the background (the desktop app sets it; see
    /// [`PrinterSource`]).
    pub printer_source: Option<PrinterSource>,
    /// Where Properties… reads a printer's driver options: the spooler's unless a test stands in.
    pub options_source: Option<OptionsSource>,
    // A browser has no background print work: the type is empty there.
    #[cfg_attr(target_arch = "wasm32", allow(dead_code))]
    pub(crate) print_tasks: print_ui::PrintTasks,
    pub palette_open: bool,
    pub palette_query: String,
    pub all_tools_expanded: bool,
    /// The All tools view fills the workspace (its sidebar's expand button).
    pub all_tools_full: bool,
    pub all_tools_query: String,
    /// The tab that showed when the expanded All tools view opened ([`Self::shown_tab`]). Another
    /// tab coming to the front (Home, a document, Combine files) puts the view away.
    all_tools_tab: (Option<DocId>, bool),
    pub recent: Vec<RecentFile>,
    /// Preferences: reopen the files that were open when PdfCraft last closed (#442).
    pub reopen_last_session: bool,
    /// Restore last view settings when reopening documents (Preferences ▸ Documents and view;
    /// #1767): the page, zoom and page display a file was last shown with. On by default.
    pub restore_last_view: bool,
    /// How files were last shown, most recent first (see `last_view`).
    pub last_views: Vec<last_view::LastView>,
    /// The files open when PdfCraft last closed, read from the settings for
    /// [`PdfCraftApp::reopen_last_files`].
    pub last_session: last_session::LastSession,
    /// Quitting closes unsaved tabs one by one: what was open when the quit began.
    quit_session: Option<last_session::LastSession>,
    /// Preferences: reload a document when its file changes on disk (#431).
    pub reload_changed_files: bool,
    /// Preferences: tell the user, above the pages, that a document is secured (its security
    /// settings restrict changes). Off only hides that notice (#4035); kept in the settings.
    pub show_secured_notice: bool,
    /// Preferences: open every document at Default zoom and Default page display, whatever
    /// layout or zoom it asks for (#1618).
    pub always_default_view: bool,
    /// Open documents' files, watched for changes on disk.
    #[cfg(not(target_arch = "wasm32"))]
    pub(crate) watch: file_watch::FileWatch,
    /// One native worker prepares opened documents without owning app or session state.
    #[cfg(not(target_arch = "wasm32"))]
    open_worker: Option<open_worker::OpenWorker>,
    #[cfg(not(target_arch = "wasm32"))]
    pending_opens: Vec<PendingOpen>,
    #[cfg(not(target_arch = "wasm32"))]
    next_open_token: u64,
    /// Explicit CLI layout waits for flagged launch opens, never targeting restored tabs.
    #[cfg(not(target_arch = "wasm32"))]
    launch_layout: Option<(canvas::PageLayout, bool)>,
    #[cfg(not(target_arch = "wasm32"))]
    launch_display_target: Option<DocId>,
    #[cfg(all(test, not(target_arch = "wasm32")))]
    open_prepare_gate: Option<std::sync::Arc<open_worker::PrepareGate>>,
    /// Folders pinned to Home, and what they held when last listed.
    pub pinned: folders_ui::PinnedFolders,
    pub toast: Option<(String, f64)>,
    /// The form script message the toast shows now, if it shows one: drawn with its title and
    /// icon (#4293).
    pub toast_script: Option<pdfcraft_engine::js::Message>,
    /// Form script messages waiting for the toast, in the order the scripts showed them: each
    /// takes the toast when the one before it is gone, so every one is seen (#4294).
    pub script_messages: std::collections::VecDeque<pdfcraft_engine::js::Message>,
    /// Form script messages the waiting queue had no room for, since the toast last said how many.
    pub script_messages_dropped: u64,
    /// The left-out count the toast is saying now ([`Self::next_script_message`]): put back to be
    /// said again if another notice replaces the toast before it has been shown its time.
    pub toast_dropped: u64,
    /// Whether the macOS title bar is drawn by us (traffic lights over our tab strip).
    pub integrated_titlebar: bool,
    /// Set when the desktop leaves the window frame to the app (GNOME on Wayland): the tab strip
    /// carries these window buttons and the window's edges resize it.
    pub window_controls: Option<window_controls::Layout>,
    /// Preferences ▸ Use system title bar: where `window_controls` is set, give the window back
    /// winit's frame instead of drawing the header bar (#1001).
    pub system_title_bar: bool,
    /// Whether the window currently has the app's header bar (`None` until the first frame).
    app_frame_shown: Option<bool>,
    pub password_prompt: Option<PasswordPrompt>,
    /// Copy of a whole document's text (#2639) whose pages are being read off the UI thread.
    document_copy: Option<DocumentCopy>,
    pub unlock_prompt: Option<UnlockPrompt>,
    pub full_screen: bool,
    /// Files delivered asynchronously (web drag-and-drop, web file picker).
    pub inbox: Inbox,
    /// The browser's startup `?file=` download. Unlike a deliberate Open or drop, a late
    /// arrival opens in the background once the user has started working (#171).
    pub startup_inbox: Inbox,
    startup_superseded: bool,
    /// Asynchronous opens that failed (web `?file=` fetches), shown as a notice.
    pub failed_inbox: FailedInbox,
    /// Mirrors "some document has unsaved work" each frame, so the web page's `beforeunload` handler can read it (#812).
    pub unsaved_flag: std::sync::Arc<std::sync::atomic::AtomicBool>,
    /// Requests from the operating system, polled every frame (macOS Apple events).
    pub os_events: Option<OsEventsFn>,
    /// Save delivers to the embedding page instead of downloading (web, see [`HostSaveFn`]).
    pub host_save: Option<HostSaveFn>,
    /// Reports unsaved work to the embedding page (web, see [`HostDirtyFn`]).
    pub host_dirty: Option<HostDirtyFn>,
    /// A pending "save changes?" question (closing a dirty tab or quitting).
    pub close_request: Option<CloseRequest>,
    /// The button of the "save changes?" question the keyboard has chosen; Save when it opens.
    pub(crate) save_prompt_button: dialogs::PromptButton,
    /// `attend_save_prompt` has seen the pending "save changes?" question. It is false again once
    /// the question is answered, so the next document's question is told apart from it.
    save_prompt_seen: bool,
    /// The "save changes?" question asked for the user's attention and the request still stands
    /// (see `attend_save_prompt`).
    save_prompt_attention: bool,
    /// A save waiting behind [`Dialog::OverwriteChanged`] (the web build has no files to check).
    #[cfg(not(target_arch = "wasm32"))]
    pub pending_overwrite: Option<editing::PendingOverwrite>,
    /// Save to this path instead of asking (tests and automation).
    pub save_override: Option<String>,
    /// Document Properties ▸ Description fields being edited: (document, Title/Author/Subject/Keywords).
    pub props_draft: Option<(DocId, [String; 4])>,
    /// Document Properties ▸ Initial View (and reading options) being edited.
    pub view_draft: Option<(DocId, pdfcraft_engine::InitialView)>,
    /// Document Properties ▸ Custom being edited.
    pub custom_draft: Option<(DocId, CustomDraft)>,
    /// Files picked asynchronously for combine / insert (web).
    pub requests: files::Requests,
    /// Native file pickers in flight (they never block the frame; see `pickers`).
    #[cfg(not(target_arch = "wasm32"))]
    pickers: pickers::Pickers,
    /// Pick these files instead of showing a picker (tests and automation).
    #[cfg(not(target_arch = "wasm32"))]
    pub pick_override: Option<Vec<String>>,
    /// Write exported files (split) here instead of asking (tests and automation).
    pub export_dir_override: Option<String>,
    /// Split dialog settings.
    pub split_draft: SplitDraft,
    pub extract_draft: ExtractDraft,
    pub rotate_draft: RotateDraft,
    /// Summarize Comments: sort order.
    pub summary_sort: pdfcraft_engine::SummarySort,
    /// The signing dialogs' state.
    pub sign_draft: Option<SignDraft>,
    /// Digital ID files the user has created or added.
    pub digital_ids: Vec<DigitalIdEntry>,
    /// Signatures panel: expanded entries (field names).
    pub sig_expanded: Vec<String>,
    /// Accessibility Checker: options, the last check, and rules skipped by hand.
    pub a11y_options: a11y_ui::A11yOptions,
    /// Scan & OCR: the Recognize Text choices, the running job, and (tests) run it inline.
    pub ocr_draft: ocr_ui::OcrDraft,
    pub ocr_run: Option<ocr_ui::OcrRun>,
    pub ocr_batch: Option<std::sync::Arc<std::sync::Mutex<ocr_ui::BatchProgress>>>,
    /// Background jobs (OCR, actions, listing pinned folders) run inline instead (tests).
    pub run_inline: bool,
    /// Action Wizard: the user's actions, the dialog state, the running action and (tests) the
    /// files to use instead of a picker.
    pub custom_actions: Vec<pdfcraft_engine::actions::Action>,
    pub wizard: actions_ui::Wizard,
    pub action_run: Option<std::sync::Arc<std::sync::Mutex<actions_ui::RunProgress>>>,
    pub action_files_override: Option<Vec<String>>,
    /// Standards ▸ PDF/A: level and last result.
    pub pdfa: standards_ui::PdfaState,
    /// Compare files: the chosen older document and the last result.
    pub compare_old: Option<DocId>,
    pub compare: Option<compare_ui::CompareState>,
    /// The JavaScript console and the Document JavaScripts draft.
    pub js_console: js_ui::JsConsole,
    pub doc_js: js_ui::DocJsDraft,
    pub a11y: a11y_ui::A11yState,
    pub(crate) tags_state: tags_ui::State,
    /// Rules skipped by hand, for the document they were skipped in.
    pub a11y_skipped: std::collections::HashSet<(DocId, pdfcraft_engine::a11y::Rule)>,
    pub alt_draft: a11y_ui::AltDraft,
    /// List the OS key store's signing identities among the digital IDs (the desktop app).
    pub os_key_store_ids: bool,
    pub cert_viewer: Option<sign_ui::CertViewer>,
    /// The last space audit.
    pub space_audit: Vec<pdfcraft_engine::optimize::SpaceUse>,
    /// The document and edit generation `space_audit` was measured from.
    pub(crate) space_audit_of: Option<(DocId, u64)>,
    /// Combine files: the files staged so far.
    pub combine_draft: Vec<combine_ui::CombineFile>,
    /// The combine list's field-name conflict preview (computed once per list, never saved).
    pub combine_conflicts: combine_ui::ConflictPreview,
    /// The Combine files tab: whether it is open, shown, its selection and undo history.
    pub combine_tab: combine_ui::CombineTab,
    /// The Combine files table's column order and widths (kept in the settings).
    pub combine_columns: combine_ui::Columns,
    /// Combine files as thumbnails or as the table (kept in the settings).
    pub combine_view: combine_ui::CombineView,
    /// How large the Combine grid draws its cards (1.0 = the usual; kept in the settings). Set
    /// with [`PdfCraftApp::set_combine_zoom`].
    pub combine_zoom: f32,
    /// How large Organize pages draws its grid, for every document (1.0 = the usual; kept in
    /// the settings).
    pub organize_zoom: f32,
    /// The Combine grid's thumbnails: rendered off the UI thread, a few at a time.
    pub(crate) combine_thumbs: combine_grid::Thumbs,
    /// Images waiting for the resolution choice (released on cancel).
    pub image_import: Option<create_ui::ImageImport>,
    /// The custom stamp library, and the stamp being created.
    pub custom_stamps: Vec<stamps_ui::CustomStamp>,
    pub stamp_draft: stamps_ui::StampDraft,
    /// PDF Optimizer choices.
    pub optimize_draft: OptimizeDraft,
    /// The running optimization (Optimize PDF ▸ Advanced optimization).
    pub optimize_run: Option<optimize_ui::OptimizeRun>,
    /// A background job's progress card (see [`widgets::progress_notice`]).
    pub progress_notice: Option<widgets::ProgressNotice>,
    /// Pages copied or cut in Organize Pages, ready to paste (into any document).
    pub page_clipboard: Option<PageClip>,
    /// Files dropped on the page grid, waiting for the pointer to say which gap they go to.
    grid_drop: Option<GridDrop>,
    /// The last snapshot (width, height, RGBA); `system_clipboard` also puts it on the
    /// system clipboard (tests turn that off).
    pub last_snapshot: Option<(u32, u32, pdfcraft_render::Pixels)>,
    /// The last image copied with Copy Image (width, height, straight RGBA), also put on the
    /// system clipboard when `system_clipboard` is on.
    pub last_copied_image: Option<(u32, u32, Vec<u8>)>,
    /// The last image copied in its original format (file name, bytes), also put on the
    /// system clipboard as a file when `system_clipboard` is on.
    pub last_copied_file: Option<(String, Vec<u8>)>,
    pub system_clipboard: bool,
    /// What ⌘V reads instead of the system clipboard (tests).
    pub clipboard_override: Option<create_ui::Clip>,
    /// V is down as a plain key press (see [`Self::paste_key_released`]).
    v_pressed: bool,
    /// Attach file: the file to attach instead of asking (tests, automation).
    pub attach_override: Option<(String, Vec<u8>)>,
    /// Duplicate Field: which field and onto which pages.
    pub duplicate_draft: Option<DuplicateDraft>,
    /// Page Properties: which pages, and the tab order chosen for them.
    pub page_props_draft: Option<PagePropsDraft>,
    /// Text field table: the table being set up, and the page it goes on.
    pub field_table_draft: Option<field_table_ui::FieldTableDraft>,
    /// Prepare a form ▸ Preview: fill the form instead of editing its fields.
    pub form_preview: bool,
    /// Where autosaves go (`None`: autosave off, e.g. on the web and in tests).
    pub recovery: Option<RecoveryStore>,
    /// Entries left by a previous session, offered in the Recovery dialog.
    pub recoverable: Vec<RecoveryMeta>,
    recovery_keys: std::collections::HashMap<DocId, String>,
    last_autosave: Option<web_time::Instant>,
    pub allow_quit: bool,
    /// Shortcuts pressed while a text field had the keyboard, run on the next frame (see
    /// `registry_shortcuts`).
    deferred_commands: Vec<(&'static str, Option<DocId>)>,
    /// The native menu bar (macOS), while it is installed; the in-window bar shows otherwise.
    pub(crate) native_menu: Option<craft_menu::NativeMenu>,
    /// Input a menu click stands for (⌘C, Delete…), added to the next frame's input in `logic`.
    pub(crate) menu_input: Vec<egui::Event>,
    /// The dialog seen at the last check, and a counter bumped whenever it changes (see
    /// [`Self::dialog_epoch`]).
    dialog_seen: Option<Dialog>,
    dialog_epoch: u64,
    /// The egui context, for commands that change window or theme state.
    ctx: Option<egui::Context>,
    /// The window's size and place kept across a minimized close (desktop).
    #[cfg(not(target_arch = "wasm32"))]
    pub window_state: window_state::WindowState,
    styled: bool,
    fonts_ready: bool,
    /// The CJK fallback order the installed fonts follow (see `theme::installed_font_definitions`).
    fonts_cjk: theme::CjkPreference,
    /// The window title last sent to the platform.
    pub window_title: String,
    /// The web page's title last set: the window title, but always with the file's name.
    pub page_title: String,
    /// The UI control channel, when enabled (`--control`; off by default).
    control: Option<control::Control>,
    /// A bookmark being renamed in the Bookmarks panel: (path, text so far).
    pub bookmark_rename: Option<(Vec<usize>, String)>,
    /// Number pages dialog settings (1-based pages).
    pub number_draft: NumberDraft,
    pub(crate) word_count_draft: word_count_ui::Draft,
    /// Protect Using Password dialog state.
    pub protect_draft: protect::ProtectDraft,
    /// New bookmarks from text styles dialog state (#1693).
    pub text_style_draft: bookmark_styles_ui::TextStyleDraft,
    /// Set Page Boxes dialog state.
    pub boxes_draft: pageboxes::BoxesDraft,
    /// Header & footer / watermark / background dialog state.
    pub marks_draft: marks_ui::MarksDraft,
    /// The Update dialogs' preview page without the marks they replace.
    pub(crate) unmarked_page: marks_ui::UnmarkedPage,
    /// Export dialog settings.
    pub export_draft: export_ui::ExportDraft,
    /// A running export's progress.
    export_status: Option<export_ui::ExportStatus>,
    /// The saved Fill & Sign signature and initials (drawn or typed).
    pub signature: Option<fill_sign::SavedSig>,
    pub initials: Option<fill_sign::SavedSig>,
    /// The Create signature / initials dialog, and its typed preview.
    pub signature_draft: fill_sign::SigDraft,
    pub(crate) signature_preview: Option<(fill_sign::SavedSig, egui::TextureHandle)>,
    /// The Scan tab's photo, keyed by its version.
    pub(crate) signature_photo: Option<(u64, egui::TextureHandle)>,
    pub(crate) saved_signature_previews: [Option<(fill_sign::SavedSig, egui::TextureHandle)>; 2],
    /// What visible digital signatures show on their left, and its preview in Sign as.
    pub sign_graphic: sign_ui::SignGraphic,
    pub(crate) sign_graphic_preview: Option<(fill_sign::SavedSig, egui::TextureHandle)>,
    #[cfg(target_arch = "wasm32")]
    pub(crate) signature_images: fill_sign::ImageInbox,
    /// The Comment Properties dialog's state.
    pub comment_props: Option<comment_props::PropsDraft>,
    pub field_props: Option<prepare::FieldDraft>,
    pub bulk_field_props: Option<bulk_fields::Draft>,
    pub redact_prefs: RedactPrefs,
    /// The Redaction Tool Properties kept for the next run (what the last OK with "Make
    /// properties default" ticked left), and the values before the dialog opened, for Cancel.
    pub redact_defaults: RedactPrefs,
    pub redact_before: Option<RedactPrefs>,
    pub redact_pages_draft: RedactPagesDraft,
    pub redact_search: RedactSearchDraft,
    pub hidden_draft: HiddenDraft,
    pub print_draft: PrintDraft,
    pub link_draft: Option<LinkDraft>,
    /// The style new text gets (Edit a PDF ▸ Format text).
    pub text_style: pdfcraft_engine::AddedText,
    /// The Replace Pages dialog's state.
    pub replace_draft: Option<files::ReplaceDraft>,
    /// The last web link the app asked the system to open (tests and automation).
    pub last_opened_url: Option<String>,
    /// The home screen's community card was closed; it comes back next launch (not remembered).
    pub community_card_closed: bool,
    /// A document asked to open this address; the user hasn't answered yet (#90, #91).
    pub pending_link: Option<PendingLink>,
    /// A document just opened asks to be shown in Full Screen mode (`/PageMode /FullScreen`):
    /// the person is asked first, as in Acrobat, and this is the document waiting for the answer.
    pub pending_full_screen: Option<DocId>,
    /// An absolute GoToR target waiting for approval, before any target-file I/O (#1364).
    pub pending_remote_link: Option<PendingRemoteLink>,
}
