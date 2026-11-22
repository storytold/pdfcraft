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
pub const UPDATE_APP: craft::update::app::App = craft::update::app::App {
    icon_png: Some(include_bytes!("../../../assets/app-icon/hicolor/128x128/apps/ai.storyteller.pdfcraft.png")),
    accent: Some([0xb7, 0x02, 0x28]),
    ..craft::update::suite::PDFCRAFT
};

/// PdfCraft as the framework's services know it (the host, About, recovery).
pub const APP_INFO: craft::host::AppInfo = craft::host::AppInfo { id: "pdfcraft", name: "PdfCraft", version: env!("CARGO_PKG_VERSION") };

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
pub use combine_ui::{CombineView, Lock as CombineLock, SortKey};
mod commands;
pub mod control;
mod menus;
mod properties_session;
/// Header & footer / watermark / background dialog types (tests and automation).
pub mod marks {
    pub use crate::marks_ui::MarksDraft;
    pub use craft::pages::{PageRange, PageScope, Subset};
}
pub use content_ui::TextDraft;
pub use create_ui::Clip;
pub use sign_ui::{DigitalIdEntry, SignDraft, SignGraphic, SignStep};
mod bidi;
mod dialogs;
pub use edit_text_ui::{CropMode, ImageSelection};
mod editing;
mod home;
mod hotbar;
mod panels;
pub mod panes;
pub use print_ui::{
    DefaultPaperSource, Handling as PrintHandling, OptionsSource, PrintDraft, PrinterPapersSource, PrinterSetup, PrinterSource, Spooler,
    TwoSidedSource, Which as PrintWhich, system_printers, system_two_sided,
};
pub use redact_ui::{HiddenDraft, RedactPrefs, SearchDraft as RedactSearchDraft, SearchMode as RedactSearchMode};
pub mod i18n;
pub mod viewer_prefs;
// PLAN: the lead adds `mod notices; mod icons; mod brand; mod colour_picker; pub mod fonts;` here, and `pub mod settings;` in app.rs, at combine (L-212).

use pdfcraft_engine::{DocId, Session};

pub use app::AppServices;
#[cfg(not(target_arch = "wasm32"))]
pub use app::UpdateSource;
pub use canvas::DocView;

/// Smallest window the shell is laid out for, in egui points (logical pixels, so the OS scale
/// factor is already applied: 480 points are 600 px at 125 % and 720 px at 150 %). 480 is half of a
/// 1440-wide screen at 150 % scaling, the tightest common half-screen split on Windows; the mode
/// bar, tab strip and notice bars adapt below the old 820 minimum (#1913).
pub const MIN_WINDOW_SIZE: [f32; 2] = [480.0, 520.0];
pub use dialogs::properties::PropsTab;
pub use editing::SaveTarget;
pub use files::{ExtractOptions, FilePurpose, SplitPlan};
pub use recovery::AUTOSAVE_SECS;
pub use workflows::js_dialogs::JsDialog;
pub use workflows::ocr_verify::{OcrVerify, Phase};

mod app;
pub(crate) mod brand;
pub(crate) mod colour_picker;
pub(crate) mod credits_data;
pub mod fonts;
mod frame_guard;
mod hooks;
pub(crate) mod icons;
pub(crate) mod notices;
mod persist;
mod platform;
#[cfg(not(target_arch = "wasm32"))]
mod script_dialog;
mod state;
mod tools;
mod workflows;

pub(crate) use app::same_file;
pub use app::settings;
pub(crate) use app::{MAX_AUTHOR_CHARS, copy_refused, date_text_for_pdf, print_refused};
use canvas::crop;
use canvas::image_pick;
use canvas::page_parts;
pub use canvas::scroll_speed;
use canvas::signature_drag;
use canvas::zoom_snap;
pub use dialogs::bookmark_styles_ui;
use dialogs::comment_props;
use dialogs::convert_colours_ui;
use dialogs::hairlines_ui;
use dialogs::marks_ui;
use dialogs::pageboxes;
use dialogs::printer_marks_ui;
use dialogs::protect;
use dialogs::standards_ui;
use dialogs::word_count_ui;
pub use home::folders_ui;
pub use hooks::HostDirtyFn;
use menus::tab_context;
use panels::all_tools_ui;
use panels::comments_panel;
use panels::format_text_ui;
use panels::organize_panel;
use panels::search_ui;
pub use panels::side_panels;
pub use persist::last_session;
pub use persist::last_view;
pub use persist::paper;
use persist::recovery;
pub use persist::units;
#[cfg(target_os = "macos")]
use platform::macos_association;
#[cfg(not(target_arch = "wasm32"))]
use platform::open_worker;
pub use platform::ui_scale;
#[cfg(test)]
use state::CopyPage;
use state::DocumentCopyJob;
#[cfg(not(target_arch = "wasm32"))]
use state::OpenRequest;
pub use state::{
    CommentClip, LeftPanel, LinkOrigin, Mode, OpenOutcome, PageClip, PagePropsDraft, PasswordPrompt, PendingLink, PendingRemoteLink, QuickTool,
    RECENT_FILES_DEFAULT, RECENT_FILES_MAX, RecentFile, RightPanel, UnlockPrompt,
};
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
use workflows::a11y_ui;
use workflows::actions_ui;
use workflows::combine_grid;
use workflows::combine_ui;
use workflows::compare_ui;
use workflows::create_ui;
use workflows::export_ui;
use workflows::files;
pub(crate) use workflows::job_count::JobCount;
use workflows::js_dialogs;
use workflows::js_ui;
#[cfg(target_arch = "wasm32")]
use workflows::ocr_route;
use workflows::ocr_ui;
use workflows::ocr_verify;
#[cfg(target_arch = "wasm32")]
use workflows::ocr_web;
use workflows::optimize_ui;
use workflows::print_paper;
use workflows::print_ui;
use workflows::scan_clean_ui;
use workflows::web_print;

/// A background job queue of the app, built over the host's jobs and the app's wake.
type Queue<K, R> = craft::host::queue::JobQueue<K, R>;

/// A file being opened in the background, waiting for the open queue's answer.
#[cfg(not(target_arch = "wasm32"))]
struct PendingOpen {
    token: u64,
    name: String,
    path: Option<String>,
    stamp: Option<craft::platform::watch::FileStamp>,
    activate: bool,
    after_open: Option<pdfcraft_render::RemoteDestination>,
    recovered: Option<craft::platform::autosave::RecoveryEntry>,
    then: Option<&'static str>,
    display_override: Option<(craft::pages::PageDisplay, bool)>,
    /// An answer taken from the queue and kept back for later (a password prompt waiting for an
    /// earlier file's password retry).
    ready: Option<open_worker::ResultKind>,
    /// Reopen Closed Tab asked for it: its closed-tab entry goes once it has opened, and stays when it fails.
    reopen: bool,
}

/// A named destination being looked up in a document.
#[cfg(not(target_arch = "wasm32"))]
struct PendingDestination {
    doc: DocId,
    name: String,
}

pub struct PdfCraftApp {
    /// The app's services: jobs, files, settings sections, commands. Every queue is built from it.
    pub(crate) host: std::sync::Arc<craft::host::Host>,
    /// Asks for a frame from any thread; every queue calls it when a job finishes.
    pub(crate) wake: craft::host::Wake,
    /// Files the operating system, another launch, a drop or the embedding page asked to open.
    intake: craft::platform::open::FileIntake,
    /// The native file panels the host's file service asks through; pumped every pass.
    #[cfg(not(target_arch = "wasm32"))]
    file_panels: Option<craft::dialogs::service::FilePanels>,
    /// The settings store the preferences are read from and written to.
    pub(crate) settings: Option<settings::Store>,
    /// The dialogs, prompts and notices the app has open, and their results.
    pub dialogs: craft::ui::dialog::DialogHost,
    /// The questions an open asks (password, unlock, links, full screen) on the dialog host.
    pub(crate) prompts: dialogs::prompts::OpenPrompts,
    /// Preferences, while its window is open.
    pub(crate) preferences: Option<dialogs::preferences::PreferencesSession>,
    /// Keyboard Shortcuts, while its window is open.
    pub(crate) shortcuts: Option<dialogs::shortcuts::ShortcutsSession>,
    /// Split view (`panes`); which pane shows each tab is its view's.
    pub panes: panes::Panes,
    /// Where the side panels are docked, their sizes and which are open.
    pub dock: craft::behavior::dock::Workspace<side_panels::PanelId>,
    /// The side the rail of panel buttons sits on.
    pub rail_side: craft::behavior::DockSide,
    pub session: Session,
    /// The interface language signature status captions were last drawn in.
    pub(crate) status_captions_lang: Option<i18n::Lang>,
    pub views: Vec<DocView>,
    pub(crate) closed_tabs: Vec<String>,
    /// `None` shows the Home tab.
    pub active: Option<usize>,
    /// The active document during the previous frame, so a deferred keyboard Save from that frame
    /// never saves a different document that became active before the next frame's `before_input`.
    pub(crate) last_frame_active: Option<DocId>,
    pub mode: Mode,
    /// Workspace used for newly opened PDFs; independent of PDF Initial View metadata.
    pub default_mode: Mode,
    /// Page display and zoom for newly opened PDFs that don't ask for their own (Preferences ▸
    /// Documents and view). Continuous scrolling at fit width by default, which never snaps
    /// between pages.
    pub view_defaults: canvas::ViewDefaults,
    /// Preferences ▸ Highlight fields for newly opened forms (saved as `highlight_fields`).
    pub(crate) highlight_fields: bool,
    /// The unit page sizes and margins are shown in (Preferences ▸ Documents and view).
    pub units: units::Unit,
    /// The paper Create ▸ Blank page makes (#5644): the system region's.
    pub blank_paper: paper::Paper,
    /// Explicit CLI/control mode lasts for this session and is never persisted.
    mode_override: Option<Mode>,
    pub left: LeftPanel,
    /// The side panel F4 reopens: the one open last.
    pub(crate) last_right: RightPanel,
    /// The height of the notice bars over the document, as the last pass drew them: the dock keeps the floating
    /// toolbar below them.
    pub(crate) notice_inset: f32,
    /// Whether the Tools pane was in the dock when the floating toolbar was last placed: the toolbar follows the page area's
    /// left edge as the pane closes and opens.
    pub(crate) toolbar_tools_open: bool,
    /// Whether the window was narrow (under [`panels::side_panels::NARROW_WINDOW`]) when the Tools pane's width was last set: a
    /// narrow window keeps the pane at production's width, a wide one gives it the width Format text needs.
    pub(crate) window_narrow: bool,
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
    /// A portfolio that just opened and the name-tree key of the attachment it presents first
    /// (`/Collection /D`), opened by [`Self::present_portfolios`] on the next frame while the
    /// portfolio is still the active tab.
    portfolio_initial: Option<(DocId, String)>,
    /// The document whose tab [`Self::present_portfolios`] last saw active.
    portfolio_seen: Option<DocId>,
    /// A portfolio's file list is showing in place of this panel (`None`: the panel was
    /// closed), given back when its tab is left.
    portfolio_panel: Option<Option<RightPanel>>,
    pub quick_tool: QuickTool,
    /// The tool before full screen, given back on leaving it (see [`Self::apply_full_screen`]).
    full_screen_tool: Option<QuickTool>,
    /// Comment author, per-tool colours and widths, pin.
    pub comment_prefs: comments::CommentPrefs,
    /// The colours picked last in the comment tools' colour pickers.
    pub(crate) recent_colours: comments::RecentColours,
    /// The continuations of the colour pickers open on the dialog host.
    pub(crate) colour_picks: colour_picker::ColourPicks,
    pub(crate) hotbar: hotbar::Hotbar,
    /// Preferences ▸ Appearance: light, dark, or as the system says.
    pub theme_preference: craft::theme::ModeChoice,
    /// The suite the Appearance page reads its themes from: an in-memory one, as PdfCraft keeps
    /// its own appearance in its settings rather than in the suite's shared selection.
    pub(crate) suite: craft::theme_store::SuiteClient,
    /// The reading-colours preference (Preferences ▸ Reading colours): applied to every open
    /// document, and documents open with it (Acrobat's Replace Document Colors).
    pub reading_colors: pdfcraft_render::ReadingColors,
    /// App-wide page decoration only; never part of a PDF edit or render request.
    pub show_page_edge_shadows: bool,
    /// How far a mouse-wheel notch scrolls.
    pub scroll_speed: scroll_speed::ScrollSpeed,
    /// Preferences ▸ Invert middle-button drag: the middle button's pan or auto-scroll moves the
    /// view the other way. Off by default.
    pub invert_middle_drag: bool,
    /// Preferences ▸ Animation effects: on, off, or as the system's animation setting says.
    pub animation_effects: craft::theme::MotionChoice,
    /// Interface language preference: `auto` (follow the system) or a code from [`i18n::LANGUAGES`].
    pub language: String,
    /// Preferences ▸ Interface size as a factor, or `None` for the usual size.
    pub ui_scale: Option<f32>,
    /// Preferences ▸ Use system title bar.
    pub title_bar_choice: craft::window::policy::TitleBarPref,
    /// Preferences: bake Fill & Sign marks into the page when saving. Off, so a normal save stays editable.
    pub flatten_fill_sign_on_save: bool,
    /// Ask on the first save after Fill & Sign places a signature or initials whether to lock
    /// them into the page (#2149). Off once the user says not to ask again.
    pub ask_to_lock_signatures: bool,
    /// Documents with a Fill & Sign signature placed since their last save, asked about on
    /// saving (#2149). Not part of the recovery data: a recovered document isn't asked.
    pub(crate) placed_signatures: std::collections::HashSet<pdfcraft_engine::DocId>,
    /// A save waiting behind the Lock signatures question.
    pub(crate) pending_lock: Option<editing::PendingLock>,
    /// The document whose lock question was just answered: the save that follows the answer
    /// doesn't ask again. The placed-signature mark itself stays until that save succeeds, so a
    /// save that fails or is cancelled asks again next time.
    pub(crate) lock_answered: Option<pdfcraft_engine::DocId>,
    /// Point size for new Fill & Sign text and dates, kept across launches.
    pub fill_text_size: f64,
    pub fill_tool: fill_sign::FillTool,
    /// Colour for new Fill & Sign text and dates, kept across launches.
    pub fill_text_color: [f64; 3],
    /// Border around new Fill & Sign text and dates, in points (0: none), until PdfCraft quits:
    /// each launch starts from [`Self::fill_text_default`].
    pub fill_text_border: f64,
    /// That border's own colour (`None`: the text's).
    pub fill_text_border_color: Option<pdfcraft_engine::Rgb>,
    /// That border's shape (an oval or a circle goes around the text).
    pub fill_text_border_shape: pdfcraft_engine::BorderShape,
    /// The border new Fill & Sign text starts each launch with, kept across launches: what Make
    /// Current Properties Default last took from typed text (none at first).
    pub fill_text_default: fill_sign::TextBorder,
    /// The update check, its prompt and the self-update.
    #[cfg(not(target_arch = "wasm32"))]
    pub(crate) updater: craft::update::updater::Updater,
    /// The desktop app asks for a newer release once it has started (Preferences permitting).
    pub(crate) startup_check: bool,
    /// Lists the Print dialog's printers in the background (the desktop app sets it; see
    /// [`PrinterSource`]).
    pub printer_source: Option<PrinterSource>,
    /// Where Properties… reads a printer's driver options: the spooler's unless a test stands in.
    pub options_source: Option<OptionsSource>,
    /// Where a printer's configured default paper comes from: DEVMODE / driver settings unless a test stands in.
    pub default_paper_source: Option<DefaultPaperSource>,
    /// Where a printer's supported paper sizes come from: driver capabilities unless a test stands in.
    pub printer_papers_source: Option<PrinterPapersSource>,
    /// What print jobs go to: the system spooler unless a test stands in ([`Spooler`]).
    pub spooler: Option<Spooler>,
    /// Reads the picked printer's two-sided support in the background (the desktop app sets it;
    /// see [`TwoSidedSource`]). Without one the dialog goes by the printer list alone.
    pub two_sided_source: Option<TwoSidedSource>,
    // A browser has no background print work: the type is empty there.
    #[cfg_attr(target_arch = "wasm32", allow(dead_code))]
    pub(crate) print_tasks: print_ui::PrintTasks,
    pub palette_open: bool,
    pub all_tools_expanded: bool,
    /// The All tools view fills the workspace (its sidebar's expand button).
    pub all_tools_full: bool,
    pub all_tools_query: String,
    /// The tab that showed when the expanded All tools view opened ([`Self::shown_tab`]). Another
    /// tab coming to the front (Home, a document, Combine files) puts the view away.
    all_tools_tab: (Option<DocId>, bool),
    pub recent: Vec<RecentFile>,
    /// How many recent files to keep, 0 for no list (Preferences ▸ Documents and view, #5572).
    pub recent_limit: usize,
    /// Home's recent files as cards or as a list.
    pub recent_view: craft::ui::shell::RecentView,
    /// Home's recent files' order.
    pub recent_sort: craft::ui::shell::RecentSort,
    /// Preferences: reopen the files that were open when PdfCraft last closed (#442).
    pub reopen_last_session: bool,
    /// Restore last view settings when reopening documents (Preferences ▸ Documents and view;
    /// #1767): the page, zoom and page display a file was last shown with. On by default.
    pub restore_last_view: bool,
    /// How files were last shown, most recent first (see `last_view`).
    pub last_views: Vec<last_view::LastView>,
    /// The files open when PdfCraft last closed, read from the settings for
    /// [`PdfCraftApp::reopen_last_files`].
    pub last_session: craft::platform::session::Session,
    /// What a quit that closes unsaved tabs one by one began with.
    session_keeper: craft::platform::session::SessionKeeper,
    /// Preferences: reload a document when its file changes on disk (#431).
    pub reload_changed_files: bool,
    /// Preferences: ask for a newer release when the desktop app starts (#5737). Off, the app
    /// asks only from Help ▸ Check for updates.
    pub update_check_at_start: bool,
    /// Preferences: tell the user, above the pages, that a document is secured (its security
    /// settings restrict changes). Off only hides that notice (#4035); kept in the settings.
    pub show_secured_notice: bool,
    /// Preferences: web and email addresses written as plain page text open on a click, like
    /// links (#4207).
    pub text_links: bool,
    /// Preferences: open every document at Default zoom and Default page display, whatever
    /// layout or zoom it asks for (#1618).
    pub always_default_view: bool,
    /// Open documents' files, watched for changes on disk.
    #[cfg(not(target_arch = "wasm32"))]
    pub(crate) watch: craft::platform::watch::FileWatch<DocId>,
    // PLAN: assumed Option<DocId>, the document whose changed file waits to be reloaded (pdf-shell-w14).
    /// A document whose changed file waits to be reloaded.
    #[cfg(not(target_arch = "wasm32"))]
    pub(crate) reload_request: Option<DocId>,
    /// Opened files read and prepared off the UI thread, by open token.
    #[cfg(not(target_arch = "wasm32"))]
    pub(crate) opens: Queue<u64, open_worker::ResultKind>,
    #[cfg(not(target_arch = "wasm32"))]
    pending_opens: Vec<PendingOpen>,
    /// The open retrying a closed password prompt's file with its password: replies behind it
    /// wait for it.
    #[cfg(not(target_arch = "wasm32"))]
    password_retry: Option<u64>,
    /// Named destinations being looked up off the UI thread: reading a large document's name tree
    /// takes a noticeable moment.
    #[cfg(not(target_arch = "wasm32"))]
    pub(crate) named_destinations: Queue<DocId, Option<(usize, pdfcraft_render::DestView)>>,
    /// The documents a lookup in `named_destinations` is for, and the name asked.
    #[cfg(not(target_arch = "wasm32"))]
    pending_destinations: Vec<PendingDestination>,
    #[cfg(not(target_arch = "wasm32"))]
    next_open_token: u64,
    /// Explicit CLI layout waits for flagged launch opens, never targeting restored tabs.
    #[cfg(not(target_arch = "wasm32"))]
    launch_layout: Option<(craft::pages::PageDisplay, bool)>,
    #[cfg(not(target_arch = "wasm32"))]
    launch_display_target: Option<DocId>,
    /// The file picks asked of the host's file service and not answered yet.
    pub(crate) picks: app::picks::Picks,
    /// Where a save that asks first (Save As, a replace question) stands.
    pub(crate) save_flow: craft::dialogs::save::SaveFlow<DocId>,
    /// A save waiting for the answer to a question it asked.
    pub(crate) pending_save: Option<editing::save::PendingSave>,
    /// Folders pinned to Home, and what they held when last listed.
    pub pinned: folders_ui::PinnedFolders,
    /// The pinned folders being listed off the UI thread.
    pub(crate) pinned_jobs: Queue<(), Vec<folders_ui::FolderListing>>,
    /// The recent files restored from settings, checked off the UI thread (#5682).
    recent_check: persist::recent_check::RecentCheck,
    /// Tools pinned to Home, in user-defined order.
    pub pinned_tools: Vec<String>,
    /// Form script messages waiting for a notice, in the order the scripts showed them, so every
    /// one is seen (#4294).
    pub script_messages: std::collections::VecDeque<pdfcraft_engine::js::Message>,
    /// Form script messages the waiting queue had no room for, since a notice last said how many.
    pub script_messages_dropped: u64,
    /// The form script message a notice shows now, drawn with its title and icon (#4293).
    pub(crate) script_shown: Option<js_ui::ScriptShown>,
    /// The notices posted since the last pass, which `notices::show` hands to the toast service.
    pub(crate) posted_notices: Vec<craft::ui::notices::PostedNotice>,
    /// The text of the last refused edit's notice, for a dialog that shows the refusal in place.
    pub(crate) last_edit_refusal: Option<String>,
    /// The notice center that posts them and shows the cards.
    pub(crate) notices: craft::ui::notices::NoticeCenter,
    /// The changes to progress cards made since the last pass, which `notices::show` applies in order.
    pub(crate) progress_requests: Vec<notices::ProgressRequest>,
    /// The progress card of each background job that shows one.
    pub(crate) progress_cards: std::collections::HashMap<crate::notices::ProgressKey, craft::ui::runtime::toast::ToastId>,
    pub password_prompt: Option<PasswordPrompt>,
    /// Whole-document text copies (#2639) reading their pages off the UI thread.
    pub(crate) text_copies: Queue<DocId, String>,
    /// Reveal-in-file-manager calls, which can block on a slow mount; their failures are shown by `drain_reveals`.
    pub(crate) reveal_jobs: Queue<std::path::PathBuf, Result<(), String>>,
    pub unlock_prompt: Option<UnlockPrompt>,
    /// Standard, full screen with the menu bar, or full screen.
    pub screen_mode: craft::ui::shell::ScreenMode,
    /// Reports unsaved work, or an open OCR review, to the embedding page (web, see [`HostDirtyFn`]).
    pub host_dirty: Option<HostDirtyFn>,
    /// The "save changes?" questions of a closing tab or a quit, asked one document at a time.
    pub(crate) close_guard: Option<craft::quit::CloseGuard<DocId>>,
    /// The open `UnsavedChangesDialog` belonging to `close_guard`, kept across a frame panic.
    pub(crate) close_guard_dialog: Option<craft::ui::dialog::DialogId>,
    /// Original values and document revision for the editable Properties tabs.
    pub(crate) props_session: Option<properties_session::Draft>,
    /// Summarize Comments: sort order.
    pub summary_sort: pdfcraft_engine::SummarySort,
    /// The Summarize Comments dialog, while it is open.
    pub(crate) summarize_dialog: Option<craft::ui::dialog::DialogId>,
    /// The signing dialogs' state.
    pub sign_draft: Option<SignDraft>,
    /// A dynamic form's preSign step, run when signing a field began: the document and its undo
    /// depth after the step. Taken back if signing ends without a signature.
    pub(crate) presign: Option<(DocId, usize)>,
    /// Digital ID files the user has created or added.
    pub digital_ids: Vec<DigitalIdEntry>,
    /// Signatures panel: expanded entries (field names).
    pub sig_open: craft::ui::list::OpenSet<String>,
    /// Accessibility Checker: options, the last check, and rules skipped by hand.
    pub a11y_options: a11y_ui::A11yOptions,
    /// Scan & OCR: the Recognize Text choices and the running job.
    pub ocr_draft: ocr_ui::OcrDraft,
    pub ocr_run: Option<ocr_ui::OcrRun>,
    pub ocr_batch: Option<std::sync::Arc<JobCount>>,
    /// Pages being recognized, by document.
    pub(crate) ocr_runs: Queue<DocId, Result<Vec<pdfcraft_engine::ocr::OcrPage>, String>>,
    /// Recognize Text in Multiple Files: the batch's summary.
    pub(crate) ocr_batches: Queue<(), String>,
    /// The web build's files being read in Web Workers (progress in `ocr_batch`).
    #[cfg(target_arch = "wasm32")]
    ocr_web_batch: Option<ocr_web::Batch>,
    /// The web build's files chosen before the text recognition models were downloaded, with
    /// the settings they were chosen with: read once the models are ready.
    #[cfg(target_arch = "wasm32")]
    ocr_web_waiting: Option<ocr_ui::WaitingFiles>,
    /// Scan & OCR ▸ Correct recognized text: the OCR Verify screen (its state, never persisted).
    pub ocr_verify: ocr_verify::OcrVerify,
    /// The OCR Verify screen's runs and re-reads.
    pub(crate) verify_jobs: Queue<ocr_verify::VerifyJob, ocr_verify::VerifyDone>,
    /// Frames in a row whose logic or drawing panicked, and whether this frame's did
    /// (`frame_guard`, #3914).
    panicking_frames: u32,
    frame_panicked: bool,
    /// The last `logic` ran without a `ui` after it yet (eframe skips `ui` while the window is
    /// hidden).
    logic_without_ui: bool,
    /// Tests: the part of the next frame that panics on purpose.
    #[cfg(test)]
    panic_next: Option<frame_guard::FramePart>,
    /// Action Wizard: the user's actions and the running action.
    pub custom_actions: Vec<pdfcraft_engine::actions::Action>,
    pub action_run: Option<std::sync::Arc<JobCount>>,
    pub(crate) action_jobs: actions_ui::ActionJobs,
    /// Standards ▸ PDF/A: the level.
    pub pdfa: standards_ui::PdfaState,
    /// Compare files: the chosen older document and the last result.
    pub compare_old: Option<DocId>,
    /// The Compare Files dialog's Pages field: the new file's pages to compare, empty for all.
    pub compare_pages: String,
    pub compare: Option<compare_ui::CompareState>,
    /// The JavaScript console.
    pub js_console: js_ui::JsConsole,
    #[cfg(not(target_arch = "wasm32"))]
    pub(crate) script_pending: Option<script_dialog::Pending>,
    /// The JavaScript dialog open (the console, the document's scripts), if any.
    pub js_dialog: Option<js_dialogs::JsDialog>,
    pub a11y: a11y_ui::A11yState,
    pub(crate) tags_state: tags_ui::State,
    /// Rules skipped by hand, for the document they were skipped in.
    pub a11y_skipped: std::collections::HashSet<(DocId, pdfcraft_engine::a11y::Rule)>,
    /// List the OS key store's signing identities among the digital IDs (the desktop app).
    pub os_key_store_ids: bool,
    pub cert_viewer: Option<sign_ui::CertViewer>,
    /// The last space audit, written by its job and read by the dialog.
    pub space_audit: std::sync::Arc<std::sync::RwLock<Vec<pdfcraft_engine::optimize::SpaceUse>>>,
    /// The document and edit generation `space_audit` was measured from.
    pub(crate) space_audit_of: Option<(DocId, u64)>,
    pub(crate) space_audit_jobs: optimize_ui::SpaceAuditJobs,
    /// Combine files: the files staged so far.
    pub combine_draft: Vec<combine_ui::CombineFile>,
    /// The combine list's field-name conflict preview (computed once per list, never saved).
    pub combine_conflicts: combine_ui::ConflictPreview,
    /// The Combine files tab: whether it is open, shown, its selection and undo history.
    pub combine_tab: combine_ui::CombineTab,
    /// The Combine files table's column order and widths (kept in the settings).
    pub combine_columns: craft::ui::contract::ColumnLayout,
    /// Combine files as thumbnails or as the table (kept in the settings).
    pub combine_view: combine_ui::CombineView,
    /// How large the Combine grid draws its cards (1.0 = the usual; kept in the settings).
    pub combine_zoom: f64,
    /// How large Organize pages draws its grid, for every document (1.0 = the usual; kept in
    /// the settings).
    pub organize_zoom: f64,
    /// Images waiting for the resolution choice (released on cancel).
    pub image_import: Option<create_ui::ImageImport>,
    /// The custom stamp library.
    pub custom_stamps: Vec<stamps_ui::CustomStamp>,
    /// Create custom stamp, while it is open.
    pub(crate) stamp_dialog: Option<stamps_ui::CreateStampDialog>,
    /// The running optimization (Optimize PDF ▸ Advanced optimization).
    pub optimize_run: Option<optimize_ui::OptimizeRun>,
    pub(crate) optimize_jobs: optimize_ui::OptimizeJobs,
    /// Scan & OCR ▸ Enhance scanned file: the dialog's choices and the running clean-up.
    pub scan_clean_draft: scan_clean_ui::ScanCleanDraft,
    pub scan_clean_run: Option<scan_clean_ui::ScanCleanRun>,
    pub(crate) scan_cleans: Queue<DocId, Result<pdfcraft_engine::scan_clean::ScanCleaned, pdfcraft_engine::EditError>>,
    /// Pages copied or cut in Organize Pages, ready to paste (into any document).
    pub page_clipboard: Option<PageClip>,
    /// Copied comments (XFDF), to paste into this or another document (M14).
    pub comment_clipboard: Option<CommentClip>,
    /// Objects copied together in Edit a PDF, to paste into this or another document (#5989).
    pub object_clipboard: Option<pdfcraft_engine::ObjectClipboard>,
    /// What copying them put on the system clipboard (see [`app::ObjectClipMark`]).
    pub(crate) object_clip_mark: app::ObjectClipMark,
    /// The last snapshot (width, height, RGBA), also put on the clipboard.
    pub last_snapshot: Option<(u32, u32, pdfcraft_render::Pixels)>,
    /// What `last_snapshot` was taken of: (document, page, rectangle in page view points).
    last_snapshot_of: Option<(DocId, usize, [f32; 4])>,
    /// The last image copied with Copy Image (width, height, straight RGBA), also put on the
    /// clipboard.
    pub last_copied_image: Option<(u32, u32, Vec<u8>)>,
    /// The last image copied in its original format (file name, bytes), also put on the
    /// clipboard as a file.
    pub last_copied_file: Option<(String, Vec<u8>)>,
    /// The clipboard every copy and paste goes through.
    pub(crate) clipboard: craft::platform::clipboard::Clipboard,
    /// Page Properties: which pages, and the tab order they have.
    pub page_props_draft: Option<PagePropsDraft>,
    /// Prepare a form ▸ Preview: fill the form instead of editing its fields.
    pub form_preview: bool,
    /// Where autosaves go (`None`: autosave off, e.g. on the web and in tests).
    pub(crate) recovery: Option<std::sync::Arc<craft::platform::autosave::RecoveryStore>>,
    /// Copies an earlier run left, offered for recovery.
    pub(crate) recovered: Vec<craft::platform::autosave::RecoveryEntry>,
    /// When each document is due for its next autosave.
    pub(crate) autosave: craft::platform::autosave::Autosave,
    /// Recovered documents whose path came from a recovery sidecar and no save has confirmed
    /// yet: their first Save asks where, starting at that path, instead of writing it (#3923).
    recovered_paths: std::collections::HashSet<DocId>,
    /// The native menu bar (macOS), while it is installed; the in-window bar shows otherwise.
    pub(crate) native_menu: Option<craft::menu::NativeMenu>,
    /// The window title last sent to the platform.
    pub window_title: String,
    /// The web page's title last set: the window title, but always with the file's name.
    pub page_title: String,
    /// The UI control channel, when enabled (`--control`; off by default).
    control: Option<craft::automation::ControlChannel>,
    /// The control server, while it listens (the desktop app's `--control`).
    control_server: Option<craft::automation::server::Listening>,
    /// Changes whenever what injected input aims at changes, so the control channel withdraws
    /// input aimed at what was there before.
    pub(crate) control_epoch: u64,
    /// Bookmarks copied or cut in the Bookmarks panel, with their subtrees, ready to paste into
    /// any document. Destinations are page indices.
    pub bookmark_clipboard: Option<Vec<pdfcraft_engine::OutlineNode>>,
    /// Word Count's options, kept between showings.
    pub(crate) word_count_options: word_count_ui::WordCountOptions,
    pub printer_marks_draft: printer_marks_ui::PrinterMarksDraft,
    pub convert_colours_draft: convert_colours_ui::ConvertColoursDraft,
    pub hairlines_draft: hairlines_ui::HairlinesDraft,
    /// New bookmarks from text styles dialog state (#1693).
    pub text_style_draft: bookmark_styles_ui::TextStyleDraft,
    /// Set Page Boxes dialog state.
    pub boxes_draft: pageboxes::BoxesDraft,
    /// Header & footer / watermark / background dialog state.
    pub marks_draft: marks_ui::MarksDraft,
    /// The Update dialogs' preview page without the marks they replace.
    pub(crate) marks_preview: Option<marks_ui::MarksPreview>,
    /// An attachment's Edit description dialog, and what it was opened for.
    pub(crate) attachment_description: Option<(craft::ui::dialog::DialogId, dialogs::attachment_description::AttachmentOpen)>,
    /// Export dialog settings.
    pub export_draft: export_ui::ExportDraft,
    /// A running export's progress.
    export_status: Option<std::sync::Arc<JobCount>>,
    pub(crate) export_jobs: export_ui::ExportJobs,
    /// The saved Fill & Sign signature and initials (drawn, typed or an image).
    pub signature: Option<craft::review::SavedSignature>,
    pub initials: Option<craft::review::SavedSignature>,
    /// What visible digital signatures show on their left.
    pub sign_graphic: sign_ui::SignGraphic,
    /// The Comment Properties dialog, while it is open.
    pub comment_props: Option<comment_props::CommentPropsDialog>,
    pub redact_prefs: RedactPrefs,
    /// The Redaction Tool Properties kept for the next run (what the last OK with "Make
    /// properties default" ticked left).
    pub redact_defaults: RedactPrefs,
    /// Redaction Tool Properties, while it is open: the values before it opened, for Cancel.
    pub redact_props: Option<RedactPrefs>,
    pub redact_search: RedactSearchDraft,
    /// Find text to redact, while it is open.
    pub(crate) redact_search_open: bool,
    /// Remove hidden information, while it is open.
    pub hidden_draft: Option<HiddenDraft>,
    pub print_draft: PrintDraft,
    /// The style new text gets (Edit a PDF ▸ Format text).
    pub text_style: pdfcraft_engine::AddedText,
    /// Edit a PDF shows its More tools.
    pub(crate) edit_more: bool,
    /// The file Replace pages takes its pages from, once picked.
    pub(crate) replace_source: Option<std::sync::Arc<dialogs::page_dialogs::ReplaceSource>>,
    /// The fonts installed on this machine, for Add text and font embedding (desktop).
    #[cfg(not(target_arch = "wasm32"))]
    pub(crate) document_fonts: Option<files::InstalledFonts>,
    /// The installed fonts being listed off the UI thread.
    pub(crate) font_scan: Queue<(), Vec<pdfcraft_engine::text_fonts::FontFile>>,
    /// Web links the app asked the system to open, waiting for the pass that hands them to the
    /// runtime.
    pub(crate) pending_urls: Vec<String>,
    /// A file the user opened, or a dialog or document that came up, since the app started: the
    /// browser's startup `?file=` download then opens in the background instead of in front
    /// (#171).
    pub(crate) startup_superseded: bool,
    /// Files that arrived as bytes through the intake (the browser's startup `?file=` download, a
    /// host's file), held while a prompt, dialog or open is under way so that they don't take its
    /// place.
    pub(crate) startup_files: Vec<(String, std::sync::Arc<[u8]>)>,
    /// The home screen's community card was closed; it comes back next launch (not remembered).
    pub community_card_closed: bool,
    /// A document asked to open this address; the user hasn't answered yet (#90, #91).
    pub pending_link: Option<PendingLink>,
    /// A document just opened asks for its window to fit its first page or to be centred
    /// (`FitWindow`, `CenterWindow`): done once the page is laid out.
    pub pending_window: Option<viewer_prefs::PendingWindow>,
    /// An absolute GoToR target waiting for approval, before any target-file I/O (#1364).
    pub pending_remote_link: Option<PendingRemoteLink>,
}
