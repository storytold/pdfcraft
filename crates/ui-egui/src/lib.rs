//! pdfcraft-ui-egui — the first PdfCraft shell (L7).
//!
//! Layout grammar follows plan/acrobat/02-ui-ux.md §1: tab strip, mode bar, left tool panel,
//! floating quick-action bar, document area, right panel + right rail with page navigation.
//! Everything here is presentation: documents, rendering and the tool catalogue live in
//! `pdfcraft-engine`.

#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

/// Whether `a` and `b` name the same file: equal, or equal once made absolute against the
/// current directory (links are not resolved, as in `last_session`).
pub(crate) fn same_file(a: &str, b: &str) -> bool {
    a == b || matches!((std::path::absolute(a), std::path::absolute(b)), (Ok(x), Ok(y)) if x == y)
}

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

mod a11y_ui;
mod actions_ui;
mod appearance;
pub mod canvas;
mod chrome;
mod combine_grid;
mod image_pick;
mod tags_ui;
pub use combine_grid::ThumbState as CombineThumb;
mod combine_ui;
pub mod docking;
pub use combine_ui::{Columns as CombineColumns, CombineView, Lock as CombineLock, SortKey};
mod camera_ui;
mod commands;
mod comment_props;
pub mod comments;
mod comments_panel;
mod compare_ui;
pub mod control;
mod create_multiple_ui;
mod create_ui;
mod credits;
mod crop;
mod drag_pointer;
mod export_ui;
mod js_ui;
mod marks_ui;
mod measure_ui;
mod ocr_ui;
mod optimize_ui;
mod search_ui;
mod sign_ui;
mod stamps_ui;
mod standards_ui;
mod zoom_snap;
/// Header & footer / watermark / background dialog types (tests and automation).
pub mod marks {
    pub use crate::marks_ui::{MarksDraft, PageRange, Subset};
}
mod content_ui;
mod link_ui;
mod object_ui;
pub use create_ui::Clip;
pub use link_ui::LinkDraft;
pub use optimize_ui::{OptimizeDraft, OptimizeTab};
pub use sign_ui::{DigitalIdEntry, SignDraft, SignGraphic, SignStep};
mod autoscroll;
mod bidi;
pub mod bulk_fields;
mod dialogs;
mod edit_text_ui;
pub use edit_text_ui::CropMode;
mod editing;
#[cfg(not(target_arch = "wasm32"))]
pub mod file_watch;
mod files;
pub mod fill_sign;
mod fling;
pub mod folders_ui;
pub mod forms_ui;
mod home;
mod icon_data;
pub mod icons;
pub mod last_session;
pub mod motion;
mod page_selection_ui;
mod pageboxes;
mod palette;
mod panels;
#[cfg(not(target_arch = "wasm32"))]
mod pickers;
pub mod prepare;
mod print_ui;
mod signature_drag;
pub mod units;
mod web_print;
mod word_count_ui;
pub use print_ui::{Handling as PrintHandling, PrintDraft, PrinterSource, Which as PrintWhich, system_printers};
mod redact_ui;
pub use redact_ui::{HiddenDraft, PagesDraft as RedactPagesDraft, RedactPrefs, SearchDraft as RedactSearchDraft, SearchMode as RedactSearchMode};
pub mod i18n;

/// [`PdfCraftApp::date_text`] for callers that already borrow other parts of the app.
pub(crate) fn date_text(session: &Session, fmt: Option<&str>) -> Result<String, String> {
    let lang = session.date_language().unwrap_or(i18n::current().code());
    session.today_text(fmt, Some(lang))
}

/// Today in Preferences ▸ Date format for stamping into a PDF; refused when it has characters
/// Fill & Sign can't write yet (see [`pdfcraft_engine::dates::unwritable`]).
pub(crate) fn date_text_for_pdf(session: &Session) -> Result<String, String> {
    let lang = session.date_language().unwrap_or(i18n::current().code());
    session.today_text_for_pdf(None, Some(lang))
}

/// The longest author name kept (Preferences ▸ Identity, restored settings).
pub(crate) const MAX_AUTHOR_CHARS: usize = 200;

#[cfg(not(target_arch = "wasm32"))]
mod open_worker;
mod organize_panel;
pub mod portable;
mod protect;
mod recovery;
pub mod scroll_speed;
#[cfg(not(target_arch = "wasm32"))]
mod system_fonts;
mod tab_context;
pub mod theme;
pub mod ui_scale;
pub mod updates;
mod wheel_pager;
mod widgets;
pub mod window_controls;

#[cfg(all(test, not(target_arch = "wasm32")))]
mod open_worker_tests;

use pdfcraft_engine::{DocId, Session};

pub use canvas::{DocView, RasterMemory};
pub use editing::{CloseRequest, SaveTarget};
pub use files::{ExtractDraft, FilePurpose, FileRequest, RotateDraft, SplitDraft, SplitMode, SplitPlan};
pub use recovery::{AUTOSAVE_SECS, RecoveryMeta, RecoveryStore};
use theme::{ThemeKind, ThemePreference};

/// Top-level workspace modes (Acrobat's mode bar).
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Mode {
    #[serde(rename = "all")]
    AllTools,
    Read,
    Edit,
    Convert,
    Sign,
}

impl Mode {
    fn parse(value: &str) -> Option<Self> {
        match value {
            "all" => Some(Self::AllTools),
            "read" => Some(Self::Read),
            "edit" => Some(Self::Edit),
            "convert" => Some(Self::Convert),
            "sign" => Some(Self::Sign),
            _ => None,
        }
    }
}

/// What the left panel shows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LeftPanel {
    AllTools,
    Tool(&'static str),
}

/// Right-hand panels, opened from the rail.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum RightPanel {
    Comments,
    Bookmarks,
    Pages,
    Fields,
    Layers,
    Attachments,
    Signatures,
    /// Accessibility Checker results.
    Accessibility,
    /// Reachable PDF structure tags and their properties.
    Tags,
    /// Advanced Search results.
    Search,
    /// Compare files: the differences.
    Compare,
}

/// Quick-action bar tools (the vertical floating strip).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum QuickTool {
    Select,
    Hand,
    /// A commenting tool (Add comments).
    Comment(comments::CommentTool),
    Measure(measure_ui::Tool),
    /// Crop pages by dragging a rectangle.
    Crop,
    /// Scan & OCR ▸ Enhance ▸ Camera image: drag four corners, then Enhance page.
    EnhanceCamera,
    /// A Fill & Sign tool.
    Fill(fill_sign::FillTool),
    /// A Prepare a form field tool.
    Field(prepare::FieldTool),
    /// Redact text and images (drag across text or draw a box).
    Redact,
    /// Edit a PDF ▸ Add content ▸ Text.
    AddText,
    /// Edit a PDF ▸ Edit text: click a line of existing text to edit it.
    EditText,
    /// Add a stamp: click to place this stamp.
    Stamp(pdfcraft_engine::StampKind),
    /// A custom stamp from the library (its index).
    CustomStamp(usize),
    /// Edit a PDF ▸ Link: draw link areas, select and edit links.
    Link,
    /// Use a certificate ▸ Digitally sign / Certify (visible): drag the signature's rectangle.
    SignArea {
        certify: bool,
    },
    /// View ▸ Zoom ▸ Marquee Zoom.
    MarqueeZoom,
    /// Edit ▸ Take a Snapshot.
    Snapshot,
    /// Edit ▸ Column Select: drag a rectangle to select only the text inside it (#740). The
    /// Select tool does the same while Alt/Option is held at the start of a drag.
    ColumnSelect,
}

/// A file dropped on a page grid: its name, and its bytes or, for a file on disk, the path they are
/// read from when the file's turn to be inserted comes (so a long drop holds one file at a time).
type GridFile = (String, Result<Vec<u8>, std::path::PathBuf>);

/// Read a file dropped on a page grid (see [`GridFile`]).
fn read_grid_file((name, source): GridFile) -> Result<(String, Vec<u8>), (String, String)> {
    let bytes = match source {
        Ok(bytes) => bytes,
        Err(path) => std::fs::read(&path).map_err(|e| (name.clone(), e.to_string()))?,
    };
    Ok((name, bytes))
}

/// Files dropped on a document's page grid or its Pages panel.
struct GridDrop {
    doc: pdfcraft_engine::DocId,
    /// Dropped on the Pages panel (its gap decides where they go), not the page grid.
    panel: bool,
    files: Vec<GridFile>,
    /// When to stop waiting for the pointer (egui time, seconds).
    deadline: f64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Dialog {
    WordCount,
    CreateImages,
    Properties(PropsTab),
    About,
    Shortcuts,
    Split,
    /// Pages ▸ Number pages… (page labels).
    NumberPages,
    /// Protect Using Password.
    Protect,
    /// Set Page Boxes (crop, trim, bleed, art, media).
    PageBoxes,
    /// Add / Update Header and Footer, Watermark, Background.
    Marks(pdfcraft_engine::MarkKind),
    /// Export a PDF ▸ Image / Text.
    Export(export_ui::ExportKind),
    /// Fill & Sign ▸ Create signature (the drawing pad).
    Signature,
    /// Comment Properties.
    CommentProps,
    /// Replace Pages (after choosing the file).
    ReplacePages,
    /// Prepare a form ▸ Field Properties.
    FieldProps,
    /// Shared properties of several selected form fields.
    BulkFieldProps,
    /// Redact a PDF ▸ Redact pages, Find text and redact, Set properties, apply confirmation.
    RedactPages,
    RedactSearch,
    RedactProps,
    RedactApply,
    /// File ▸ Print.
    Print,
    /// File ▸ Revert confirmation.
    Revert,
    /// Save over a file that changed on disk since the document came from it (#2166).
    OverwriteChanged,
    /// Link Properties.
    LinkProps,
    /// Organize ▸ Extract (options), Rotate Pages.
    Extract,
    RotatePages,
    /// Remove Hidden Information and Sanitize Document.
    RemoveHidden,
    Sanitize,
    /// Documents from a session that ended unexpectedly.
    Recovery,
    /// Comments ▸ Summarize comments (options).
    SummarizeComments,
    /// Sign with a Digital ID ▸ Configure ▸ Sign as.
    Sign,
    /// Optimize PDF ▸ Advanced optimization.
    Optimize,
    /// Prepare a form ▸ right-click a field ▸ Duplicate.
    DuplicateField,
    /// Check for accessibility ▸ Accessibility Checker Options.
    AccessibilityOptions,
    /// Scan & OCR ▸ Recognize text.
    RecognizeText,
    /// The JavaScript console (⌘J).
    JsConsole,
    /// Document JavaScripts.
    DocumentJs,
    /// Preferences.
    Preferences,
    /// Compare files: choose the older version.
    CompareFiles,
    /// Action Wizard.
    ActionWizard,
    /// Standards ▸ PDF/A.
    PdfA,
    /// Custom stamps ▸ Create.
    CreateStamp,
    /// Prepare for accessibility ▸ Add alternate text.
    AltText,
    /// PDF Optimizer ▸ Audit space usage (then back to the optimizer).
    AuditSpace,
    /// Signatures ▸ Show certificate.
    CertificateViewer,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PropsTab {
    Description,
    InitialView,
    Security,
    Fonts,
    Advanced,
}

/// Duplicate Field: the field and the pages (all, or a range, 1-based).
#[derive(Clone, Debug, PartialEq)]
pub struct DuplicateDraft {
    pub name: String,
    pub all: bool,
    pub from: usize,
    pub to: usize,
}

/// Copied pages: their document's name and bytes (as it was when copied) and the pages.
#[derive(Clone, Debug)]
pub struct PageClip {
    /// The document the pages were copied from: pasting back into it keeps field names.
    pub from: DocId,
    pub name: String,
    pub bytes: std::sync::Arc<Vec<u8>>,
    pub pages: Vec<usize>,
}

/// Files delivered asynchronously: (name, bytes).
pub type Inbox = std::sync::Arc<std::sync::Mutex<Vec<(String, Vec<u8>)>>>;

/// Files that failed to arrive asynchronously (a browser `?file=` URL that couldn't be fetched):
/// `(name, error)`, reported to the user on the next frame (#173).
pub type FailedInbox = std::sync::Arc<std::sync::Mutex<Vec<(String, String)>>>;

/// A request the operating system sends the running app, outside its window: on macOS, Finder
/// double-clicks, Open With and drops on the Dock icon arrive as Apple events, not arguments.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum OsEvent {
    /// Open these files.
    Open(Vec<String>),
    /// Quit (the Dock's Quit, logging out), asking about unsaved changes first.
    Quit,
}

/// Returns the [`OsEvent`]s that arrived since it was last called (set by the desktop app).
pub type OsEventsFn = Box<dyn FnMut() -> Vec<OsEvent>>;

/// Hands a saved PDF to the page embedding PdfCraft (web): `(name, bytes, save_as)`. `save_as`
/// is `true` for Save As. Set by the web app when a host page (such as a Nextcloud app) asked
/// for saves; without it the browser downloads the file.
pub type HostSaveFn = Box<dyn Fn(&str, &[u8], bool) -> Result<(), String>>;

/// Told every frame whether any tab has unsaved work, so a host page can warn before leaving.
pub type HostDirtyFn = Box<dyn FnMut(bool)>;

pub struct PasswordPrompt {
    pub name: String,
    pub path: Option<String>,
    pub bytes: std::sync::Arc<Vec<u8>>,
    #[cfg(not(target_arch = "wasm32"))]
    stamp: Option<file_watch::Stamp>,
    pub input: String,
    pub error: Option<String>,
    /// A late startup file must stay in the background even after it is unlocked.
    activate: bool,
    /// The bytes are a recovery snapshot: once the document is open it takes the snapshot's
    /// path and recovery entry. Cancelling the prompt drops the association with it (#813).
    recovered: Option<RecoveryMeta>,
    /// The tool that asked for this file (a tool chosen with no document open): it runs once
    /// the document is unlocked.
    then: Option<&'static str>,
    /// A link destination to apply after the linked PDF is unlocked.
    after_open: Option<pdfcraft_render::RemoteDestination>,
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
}

#[cfg(not(target_arch = "wasm32"))]
struct OpenRequest {
    name: String,
    path: Option<String>,
    stamp: Option<file_watch::Stamp>,
    input: open_worker::Input,
    password: Option<String>,
    activate: bool,
    after_open: Option<pdfcraft_render::RemoteDestination>,
    recovered: Option<RecoveryMeta>,
    then: Option<&'static str>,
}

/// Asks for the permissions password of an open document whose security restricts it, which
/// lifts the restrictions (#995). Acrobat asks for it when such a document's security is changed.
pub struct UnlockPrompt {
    pub doc: DocId,
    pub name: String,
    pub input: String,
    pub error: Option<String>,
    /// The command to run once unlocked (Protect using password, Remove security), if any.
    pub then: Option<&'static str>,
}

impl UnlockPrompt {
    pub(crate) fn new(doc: &pdfcraft_engine::Document, then: Option<&'static str>) -> Self {
        Self { doc: doc.id, name: doc.name.clone(), input: String::new(), error: None, then }
    }
}

#[derive(serde::Serialize, serde::Deserialize)]
pub struct RecentFile {
    pub name: String,
    pub path: String,
    pub pages: usize,
    pub size: usize,
}

pub struct PdfCraftApp {
    pub docking: docking::Workspace,
    pub(crate) pending_panel_reveals: Vec<docking::Panel>,
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
    /// Comment author, per-tool colours and widths, pin.
    pub comment_prefs: comments::CommentPrefs,
    /// Resolved colours, including the current OS theme when following the system.
    pub theme: ThemeKind,
    pub theme_preference: ThemePreference,
    /// The reading-colours preference (Preferences ▸ Reading colours): applied to every open
    /// document, and documents open with it (Acrobat's Replace Document Colors).
    pub reading_colors: pdfcraft_render::ReadingColors,
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
    pub dialog: Option<Dialog>,
    /// How to ask for the latest release (the desktop app sets it; see `updates`).
    pub update_source: Option<updates::UpdateSource>,
    pub(crate) updates: updates::Updates,
    /// Lists the Print dialog's printers in the background (the desktop app sets it; see
    /// [`PrinterSource`]).
    pub printer_source: Option<PrinterSource>,
    // A browser has no background print work: the type is empty there.
    #[cfg_attr(target_arch = "wasm32", allow(dead_code))]
    pub(crate) print_tasks: print_ui::PrintTasks,
    pub palette_open: bool,
    pub palette_query: String,
    pub all_tools_expanded: bool,
    pub recent: Vec<RecentFile>,
    /// Preferences: reopen the files that were open when PdfCraft last closed (#442).
    pub reopen_last_session: bool,
    /// The files open when PdfCraft last closed, read from the settings for
    /// [`PdfCraftApp::reopen_last_files`].
    pub last_session: last_session::LastSession,
    /// Quitting closes unsaved tabs one by one: what was open when the quit began.
    quit_session: Option<last_session::LastSession>,
    /// Preferences: reload a document when its file changes on disk (#431).
    pub reload_changed_files: bool,
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
    #[cfg(all(test, not(target_arch = "wasm32")))]
    open_prepare_gate: Option<std::sync::Arc<open_worker::PrepareGate>>,
    /// Folders pinned to Home, and what they held when last listed.
    pub pinned: folders_ui::PinnedFolders,
    pub toast: Option<(String, f64)>,
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
    /// A save waiting behind [`Dialog::OverwriteChanged`] (the web build has no files to check).
    #[cfg(not(target_arch = "wasm32"))]
    pub pending_overwrite: Option<editing::PendingOverwrite>,
    /// Save to this path instead of asking (tests and automation).
    pub save_override: Option<String>,
    /// Document Properties ▸ Description fields being edited: (document, Title/Author/Subject/Keywords).
    pub props_draft: Option<(DocId, [String; 4])>,
    /// Document Properties ▸ Initial View (and reading options) being edited.
    pub view_draft: Option<(DocId, pdfcraft_engine::InitialView)>,
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
    /// Attach file: the file to attach instead of asking (tests, automation).
    pub attach_override: Option<(String, Vec<u8>)>,
    /// Duplicate Field: which field and onto which pages.
    pub duplicate_draft: Option<DuplicateDraft>,
    /// Prepare a form ▸ Preview: fill the form instead of editing its fields.
    pub form_preview: bool,
    /// Where autosaves go (`None`: autosave off, e.g. on the web and in tests).
    pub recovery: Option<RecoveryStore>,
    /// Entries left by a previous session, offered in the Recovery dialog.
    pub recoverable: Vec<RecoveryMeta>,
    recovery_keys: std::collections::HashMap<DocId, String>,
    last_autosave: f64,
    allow_quit: bool,
    /// Shortcuts pressed while a text field had the keyboard, run on the next frame (see
    /// `registry_shortcuts`).
    deferred_commands: Vec<(&'static str, Option<DocId>)>,
    /// The dialog seen at the last check, and a counter bumped whenever it changes (see
    /// [`Self::dialog_epoch`]).
    dialog_seen: Option<Dialog>,
    dialog_epoch: u64,
    /// The egui context, for commands that change window or theme state.
    ctx: Option<egui::Context>,
    styled: bool,
    fonts_ready: bool,
    /// The CJK fallback order the installed fonts follow (see `theme::installed_font_definitions`).
    fonts_cjk: theme::CjkPreference,
    /// The window title last sent to the platform.
    pub window_title: String,
    /// The UI control channel, when enabled (`--control`; off by default).
    control: Option<control::Control>,
    /// A bookmark being renamed in the Bookmarks panel: (path, text so far).
    pub bookmark_rename: Option<(Vec<usize>, String)>,
    /// Number pages dialog settings (1-based pages).
    pub number_draft: NumberDraft,
    pub(crate) word_count_draft: word_count_ui::Draft,
    /// Protect Using Password dialog state.
    pub protect_draft: protect::ProtectDraft,
    /// Set Page Boxes dialog state.
    pub boxes_draft: pageboxes::BoxesDraft,
    /// Header & footer / watermark / background dialog state.
    pub marks_draft: marks_ui::MarksDraft,
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
    /// Preferences ▸ Signatures ▸ Time server: none (the default), or the RFC 3161 server a
    /// person chose. Nothing reaches the network unless one is chosen.
    pub time_server: sign_ui::TimeServer,
    /// The Sign dialog's time server hint opened Preferences: closing them goes back to it.
    pub(crate) preferences_then_sign: bool,
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
    /// A document asked to open this address; the user hasn't answered yet (#90, #91).
    pub pending_link: Option<PendingLink>,
    /// A document just opened asks to be shown in Full Screen mode (`/PageMode /FullScreen`):
    /// the person is asked first, as in Acrobat, and this is the document waiting for the answer.
    pub pending_full_screen: Option<DocId>,
    /// An absolute GoToR target waiting for approval, before any target-file I/O (#1364).
    pub pending_remote_link: Option<PendingRemoteLink>,
}

/// Where in a document a request to open an address came from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LinkOrigin {
    /// A link on the page.
    Link,
    /// A push button's URI action.
    Button,
    /// A script (`app.launchURL`).
    Script,
}

impl LinkOrigin {
    fn noun(self) -> &'static str {
        match self {
            Self::Link => "A link",
            Self::Button => "A button",
            Self::Script => "A script",
        }
    }
}

/// An address a document asked to open, waiting for the user to allow or cancel it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PendingLink {
    pub url: String,
    pub origin: LinkOrigin,
}

/// A resolved local target and its original source/destination, independent of the active tab.
#[derive(Clone, Debug)]
pub struct PendingRemoteLink {
    pub source: DocId,
    pub path: std::path::PathBuf,
    pub destination: pdfcraft_render::RemoteDestination,
}

/// Settings for the Number pages dialog.
#[derive(Clone, Debug, PartialEq)]
pub struct NumberDraft {
    pub from: usize,
    pub to: usize,
    pub style: pdfcraft_engine::LabelStyle,
    pub prefix: String,
    pub start: u32,
}

impl Default for PdfCraftApp {
    fn default() -> Self {
        Self::new()
    }
}

/// A restored colour: an `[r, g, b]` array of finite numbers, each clamped to 0–1 (restored
/// settings are untrusted). Anything else (a string, a wrong length, a null or a non-finite
/// number) is refused whole, so the caller keeps its default.
fn rgb_triple(v: &serde_json::Value) -> Option<[f64; 3]> {
    let a = v.as_array()?;
    if a.len() != 3 {
        return None;
    }
    let mut out = [0.0f64; 3];
    for (slot, x) in out.iter_mut().zip(a.iter()) {
        let n = x.as_f64()?;
        if !n.is_finite() {
            return None;
        }
        *slot = n.clamp(0.0, 1.0);
    }
    Some(out)
}

impl PdfCraftApp {
    pub fn new() -> Self {
        Self {
            session: Session::new(),
            views: Vec::new(),
            closed_tabs: Vec::new(),
            active: None,
            mode: Mode::AllTools,
            default_mode: Mode::AllTools,
            view_defaults: Default::default(),
            units: Default::default(),
            date_format_draft: None,
            mode_override: None,
            left: LeftPanel::AllTools,
            docking: docking::Workspace::default(),
            pending_panel_reveals: Vec::new(),
            left_open: true,
            right: None,
            right_before_read: None,
            comments_panel_closed: false,
            right_panel_closed: false,
            quick_tool: QuickTool::Select,
            space_hand: None,
            comment_prefs: Default::default(),
            theme: ThemeKind::Light,
            theme_preference: ThemePreference::Light,
            reading_colors: Default::default(),
            #[cfg(target_os = "linux")]
            desktop_theme: None,
            #[cfg(target_os = "linux")]
            desktop_theme_enabled: false,
            scroll_speed: scroll_speed::ScrollSpeed::default(),
            animation_effects: motion::AnimationEffects::default(),
            motion: motion::Motion::default(),
            language: i18n::AUTO.to_string(),
            ui_scale: None,
            system_text_scale: 1.0,
            ui_scale_applied: None,
            flatten_fill_sign_on_save: false,
            fill_text_size: fill_sign::TEXT_SIZE,
            dialog: None,
            update_source: None,
            updates: updates::Updates::default(),
            printer_source: None,
            print_tasks: print_ui::PrintTasks::default(),
            palette_open: false,
            palette_query: String::new(),
            all_tools_expanded: false,
            recent: Vec::new(),
            reopen_last_session: false,
            last_session: Default::default(),
            quit_session: None,
            reload_changed_files: true,
            #[cfg(not(target_arch = "wasm32"))]
            watch: Default::default(),
            #[cfg(not(target_arch = "wasm32"))]
            open_worker: None,
            #[cfg(not(target_arch = "wasm32"))]
            pending_opens: Vec::new(),
            #[cfg(not(target_arch = "wasm32"))]
            next_open_token: 0,
            #[cfg(all(test, not(target_arch = "wasm32")))]
            open_prepare_gate: None,
            pinned: Default::default(),
            toast: None,
            integrated_titlebar: false,
            window_controls: None,
            system_title_bar: false,
            app_frame_shown: None,
            password_prompt: None,
            unlock_prompt: None,
            full_screen: false,
            inbox: Default::default(),
            startup_inbox: Default::default(),
            startup_superseded: false,
            failed_inbox: Default::default(),
            unsaved_flag: Default::default(),
            os_events: None,
            host_save: None,
            host_dirty: None,
            close_request: None,
            #[cfg(not(target_arch = "wasm32"))]
            pending_overwrite: None,
            save_override: None,
            props_draft: None,
            view_draft: None,
            requests: Default::default(),
            #[cfg(not(target_arch = "wasm32"))]
            pickers: Default::default(),
            #[cfg(not(target_arch = "wasm32"))]
            pick_override: None,
            export_dir_override: None,
            split_draft: SplitDraft::default(),
            extract_draft: ExtractDraft::default(),
            rotate_draft: RotateDraft::default(),
            summary_sort: Default::default(),
            sign_draft: None,
            digital_ids: Vec::new(),
            sig_expanded: Vec::new(),
            a11y_options: a11y_ui::A11yOptions::default(),
            ocr_draft: ocr_ui::OcrDraft::default(),
            ocr_run: None,
            ocr_batch: None,
            run_inline: false,
            custom_actions: Vec::new(),
            wizard: Default::default(),
            action_run: None,
            action_files_override: None,
            pdfa: Default::default(),
            compare_old: None,
            compare: None,
            js_console: Default::default(),
            doc_js: Default::default(),
            a11y: a11y_ui::A11yState::default(),
            tags_state: Default::default(),
            a11y_skipped: Default::default(),
            alt_draft: Default::default(),
            os_key_store_ids: false,
            cert_viewer: None,
            space_audit: Vec::new(),
            combine_draft: Vec::new(),
            combine_conflicts: Default::default(),
            combine_tab: Default::default(),
            combine_columns: Default::default(),
            combine_view: Default::default(),
            combine_zoom: 1.0,
            combine_thumbs: Default::default(),
            image_import: None,
            custom_stamps: Vec::new(),
            stamp_draft: Default::default(),
            optimize_draft: OptimizeDraft::default(),
            optimize_run: None,
            progress_notice: None,
            page_clipboard: None,
            grid_drop: None,
            last_snapshot: None,
            last_copied_image: None,
            last_copied_file: None,
            system_clipboard: true,
            attach_override: None,
            duplicate_draft: None,
            form_preview: false,
            window_title: String::new(),
            recovery: None,
            recoverable: Vec::new(),
            recovery_keys: Default::default(),
            last_autosave: 0.0,
            allow_quit: false,
            deferred_commands: Vec::new(),
            dialog_seen: None,
            dialog_epoch: 0,
            ctx: None,
            styled: false,
            fonts_ready: false,
            fonts_cjk: theme::CjkPreference::Default,
            control: None,
            bookmark_rename: None,
            last_opened_url: None,
            pending_link: None,
            pending_full_screen: None,
            pending_remote_link: None,
            protect_draft: Default::default(),
            boxes_draft: Default::default(),
            marks_draft: Default::default(),
            export_draft: Default::default(),
            export_status: None,
            signature: None,
            initials: None,
            signature_draft: Default::default(),
            signature_preview: None,
            signature_photo: None,
            saved_signature_previews: [None, None],
            sign_graphic: sign_ui::SignGraphic::Name,
            time_server: sign_ui::TimeServer::default(),
            preferences_then_sign: false,
            sign_graphic_preview: None,
            #[cfg(target_arch = "wasm32")]
            signature_images: Default::default(),
            comment_props: None,
            field_props: None,
            bulk_field_props: None,
            redact_prefs: RedactPrefs::default(),
            redact_defaults: RedactPrefs::default(),
            redact_before: None,
            redact_pages_draft: RedactPagesDraft::default(),
            redact_search: RedactSearchDraft::default(),
            hidden_draft: HiddenDraft::default(),
            print_draft: PrintDraft::default(),
            link_draft: None,
            text_style: content_ui::default_style(),
            replace_draft: None,
            number_draft: NumberDraft { from: 1, to: 1, style: pdfcraft_engine::LabelStyle::Decimal, prefix: String::new(), start: 1 },
            word_count_draft: word_count_ui::Draft::default(),
        }
    }

    /// Fields are being edited (Prepare a form is open or a field tool is picked), unless the
    /// form is being previewed.
    pub fn is_preparing(&self) -> bool {
        ((self.left_open && self.left == LeftPanel::Tool("form")) || matches!(self.quick_tool, QuickTool::Field(_))) && !self.form_preview
    }

    /// Open a document the way it asks to be opened: navigation panel, layout, magnification,
    /// page (Document Properties ▸ Initial View).
    fn apply_initial_view(&mut self, index: usize, v: &pdfcraft_engine::InitialView) {
        use pdfcraft_engine::{InitialLayout as L, Magnification as M, Navigation as N};
        let pages = self.session.get(self.views[index].id).map_or(0, |d| d.info.pages.len());
        if self.active == Some(index) {
            match v.navigation {
                N::PageOnly => {}
                N::Bookmarks => self.right = Some(RightPanel::Bookmarks),
                N::Pages => self.right = Some(RightPanel::Pages),
                N::Attachments => self.right = Some(RightPanel::Attachments),
                N::Layers => self.right = Some(RightPanel::Layers),
            }
        }
        let view = &mut self.views[index];
        match v.layout {
            // The view opened in Default page display already.
            L::Default => {}
            L::SinglePage => (view.layout, view.continuous) = (canvas::PageLayout::Single, false),
            L::SinglePageContinuous => (view.layout, view.continuous) = (canvas::PageLayout::Single, true),
            L::TwoUp => (view.layout, view.continuous) = (canvas::PageLayout::TwoUp, false),
            L::TwoUpContinuous => (view.layout, view.continuous) = (canvas::PageLayout::TwoUp, true),
            L::TwoUpCoverPage => {
                (view.layout, view.continuous, view.cover) = (canvas::PageLayout::TwoUp, false, true);
            }
            L::TwoUpContinuousCoverPage => {
                (view.layout, view.continuous, view.cover) = (canvas::PageLayout::TwoUp, true, true);
            }
        }
        match v.magnification {
            // The view opened at the default zoom already.
            M::Default => {}
            M::ActualSize => view.set_zoom(1.0),
            M::Percent(p) => view.set_zoom((p / 100.0) as f32),
            M::FitPage | M::FitVisible => view.fit = canvas::Fit::Page,
            M::FitWidth => view.fit = canvas::Fit::Width,
            M::FitHeight => view.fit = canvas::Fit::Height,
        }
        if v.page > 0 && v.page < pages {
            view.go_to_page(v.page);
        }
    }

    /// Open a document and make it the active tab. Encrypted files raise the password prompt.
    pub fn open_bytes(&mut self, name: &str, path: Option<String>, bytes: Vec<u8>) -> Result<(), String> {
        self.startup_superseded = true;
        self.open_bytes_with_focus(name, path, bytes, true)
    }

    fn open_bytes_with_focus(&mut self, name: &str, path: Option<String>, bytes: Vec<u8>, activate: bool) -> Result<(), String> {
        // Images and text files become new, unsaved PDFs (Create a PDF).
        let active = self.active;
        if let Some(r) = self.open_converted(name, &bytes) {
            if !activate {
                self.active = active;
            }
            return r;
        }
        self.try_open(name, path, std::sync::Arc::new(bytes), None, activate)
    }

    #[cfg(not(target_arch = "wasm32"))]
    fn schedule_open(&mut self, request: OpenRequest) -> Result<u64, String> {
        if self.open_worker.is_none() {
            self.open_worker = Some(
                open_worker::OpenWorker::new(
                    #[cfg(test)]
                    self.open_prepare_gate.clone(),
                )
                .map_err(|e| format!("couldn't start the document-open worker: {e}"))?,
            );
        }
        let token = self.next_open_token.wrapping_add(1);
        let worker = self.open_worker.as_ref().ok_or("the document-open worker is unavailable")?;
        if let Some(ctx) = &self.ctx {
            worker.set_context(ctx);
        }
        let receiver =
            worker.submit(token, self.session.open_context(), request.name.clone(), request.path.clone(), request.password, request.input)?;
        self.next_open_token = token;
        self.pending_opens.push(PendingOpen {
            token,
            receiver,
            name: request.name,
            path: request.path,
            stamp: request.stamp,
            activate: request.activate,
            after_open: request.after_open,
            recovered: request.recovered,
            then: request.then,
        });
        Ok(token)
    }

    #[cfg(not(target_arch = "wasm32"))]
    fn poll_pending_opens(&mut self) {
        let pending_opens = std::mem::take(&mut self.pending_opens);
        let mut remaining = pending_opens.into_iter();
        let mut waiting = Vec::new();
        while let Some(pending) = remaining.next() {
            // A tool, password prompt or file picker belongs to the document just admitted.
            // Keep later replies queued so another open cannot change its recipient.
            if self.modal_open() || self.pickers.pending() {
                waiting.push(pending);
                waiting.extend(remaining);
                break;
            }
            match pending.receiver.try_recv() {
                Ok(result) if result.token == pending.token => self.finish_pending_open(pending, result.kind),
                Ok(_) => waiting.push(pending),
                Err(std::sync::mpsc::TryRecvError::Empty) => waiting.push(pending),
                Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                    self.notify_fmt("Couldn't open {name}: the background worker stopped", &[("name", &pending.name)]);
                }
            }
        }
        // A completion may schedule another open; preserve those requests as well as the
        // earlier waiting replies, without consuming a blocked receiver's result.
        waiting.append(&mut self.pending_opens);
        self.pending_opens = waiting;
    }

    #[cfg(not(target_arch = "wasm32"))]
    fn finish_pending_open(&mut self, pending: PendingOpen, kind: open_worker::ResultKind) {
        match kind {
            open_worker::ResultKind::ReadFailed(reason) => {
                self.notify_fmt("Couldn't read {name}: {e}", &[("name", &pending.name), ("e", &reason)]);
            }
            open_worker::ResultKind::RemoteFailed(reason) => {
                self.notify_fmt("Couldn't follow linked PDF: {reason}", &[("reason", &reason)]);
            }
            open_worker::ResultKind::RecoveryFailed(reason) => {
                self.notify_fmt("Couldn't recover {name}: {e}", &[("name", &pending.name), ("e", &reason)]);
            }
            open_worker::ResultKind::Failed(reason) => {
                self.notify_fmt("Couldn't open {name}: {reason}", &[("name", &pending.name), ("reason", &reason)]);
            }
            open_worker::ResultKind::Conversion(bytes) => match self.open_converted(&pending.name, &bytes) {
                Some(Ok(())) => {
                    if let Some(command) = pending.then {
                        self.execute(command);
                    }
                }
                Some(Err(error)) => self.notify_fmt("Couldn't open {name}: {e}", &[("name", &pending.name), ("e", &error)]),
                None => {}
            },
            open_worker::ResultKind::Prepared { bytes, result } => match *result {
                Ok(prepared) => match self.finish_open(&pending.name, pending.path.clone(), bytes.len(), prepared, pending.activate) {
                    Ok(id) => {
                        if pending.path.is_some() {
                            self.watch.known_as(id, pending.stamp);
                        }
                        if let Some(destination) = pending.after_open {
                            self.apply_remote_destination(destination);
                        }
                        if let Some(meta) = &pending.recovered {
                            self.finish_recovery(meta);
                        }
                        if let Some(command) = pending.then {
                            self.execute(command);
                        }
                    }
                    Err(error) => self.notify_fmt("Couldn't open {name}: {e}", &[("name", &pending.name), ("e", &error)]),
                },
                Err(error @ (pdfcraft_render::OpenError::NeedsPassword | pdfcraft_render::OpenError::WrongPassword)) => {
                    let password_error =
                        matches!(error, pdfcraft_render::OpenError::WrongPassword).then(|| "Incorrect password. Try again.".to_string());
                    self.password_prompt = Some(PasswordPrompt {
                        name: pending.name,
                        path: pending.path,
                        bytes,
                        stamp: pending.stamp,
                        input: String::new(),
                        error: password_error,
                        activate: pending.activate,
                        recovered: pending.recovered,
                        then: pending.then,
                        after_open: pending.after_open,
                    });
                }
                Err(error) => self.notify_fmt("Couldn't open {name}: {e}", &[("name", &pending.name), ("e", &error.to_string())]),
            },
        }
    }

    fn try_open(
        &mut self,
        name: &str,
        path: Option<String>,
        bytes: std::sync::Arc<Vec<u8>>,
        password: Option<&str>,
        activate: bool,
    ) -> Result<(), String> {
        use pdfcraft_render::OpenError;
        let prepared = match self.session.prepare_open(name, path.clone(), bytes.clone(), password) {
            Ok(prepared) => prepared,
            Err(e @ (OpenError::NeedsPassword | OpenError::WrongPassword)) => {
                let error = matches!(e, OpenError::WrongPassword).then(|| "Incorrect password. Try again.".to_string());
                #[cfg(not(target_arch = "wasm32"))]
                let stamp = path.as_deref().and_then(file_watch::Stamp::at);
                self.password_prompt = Some(PasswordPrompt {
                    name: name.to_string(),
                    path,
                    bytes,
                    #[cfg(not(target_arch = "wasm32"))]
                    stamp,
                    input: String::new(),
                    error,
                    activate,
                    recovered: None,
                    then: None,
                    after_open: None,
                });
                return Ok(());
            }
            Err(e) => return Err(e.to_string()),
        };
        self.finish_open(name, path, bytes.len(), prepared, activate).map(|_| ())
    }

    fn finish_open(
        &mut self,
        name: &str,
        path: Option<String>,
        size: usize,
        prepared: pdfcraft_engine::PreparedDocument,
        activate: bool,
    ) -> Result<DocId, String> {
        let id = self.session.commit_open(prepared);
        self.password_prompt = None;
        // The reading-colours preference is app-wide: a newly opened document starts with it.
        // Idempotent: the engine returns `false` when the document has the setting already.
        self.session.set_reading_colors(id, self.reading_colors);
        let doc = self.session.get(id).ok_or("the document could not be opened")?;
        let pages = doc.info.pages.len();
        // Acrobat opens straight to the Comments panel when a document has comments.
        // Read mode opens with the panes closed; the rail still opens them (#932). A right panel
        // the user closed stays closed (#1590).
        let opening_mode = self.mode_override.unwrap_or(self.default_mode);
        if activate && self.right.is_none() && !self.right_panel_closed && opening_mode != Mode::Read {
            self.right = if !doc.info.annotations.is_empty() {
                Some(RightPanel::Comments)
            } else if !doc.info.outline.is_empty() {
                Some(RightPanel::Bookmarks)
            } else {
                None
            };
        }
        let initial = doc.initial_view();
        self.views.push(DocView::new(id, &doc.info, self.view_defaults));
        if activate {
            self.active = Some(self.views.len() - 1);
        }
        self.apply_initial_view(self.views.len() - 1, &initial);
        // Full screen is asked for, never taken: a document can't put the app in full screen on
        // its own (the prompt is `dialogs::full_screen_prompt`).
        if activate && initial.full_screen {
            self.pending_full_screen = Some(id);
        }
        if activate
            && self.left_open
            && self.left == LeftPanel::Tool("organize")
            && let Some(view) = self.views.last_mut()
        {
            view.open_organize();
        }
        if activate && let Some(mode) = self.mode_override {
            // Explicit mode options do not reset independent --tool / --left choices.
            self.mode = mode;
        } else if activate && self.mode != self.default_mode {
            // Switch workspace like the mode bar, but a left panel the user (or `--left closed`)
            // closed stays closed, and an unchanged mode keeps the tool panel the user chose.
            let left_open = self.left_open;
            self.select_mode(self.default_mode);
            self.left_open = left_open;
        }
        if let Some(p) = path {
            self.recent.retain(|r| r.path != p);
            self.recent.insert(0, RecentFile { name: name.to_string(), path: p, pages, size });
            self.recent.truncate(12);
        }
        // What the form's scripts said while it opened (messages, errors) shows now, not with
        // the next edit.
        // Background documents keep their script output until their tab is selected: a print
        // or save request must never act on the current document instead.
        if activate {
            let out = self.session.take_js_output(id);
            self.handle_js(id, out);
        }
        Ok(id)
    }

    #[cfg(not(target_arch = "wasm32"))]
    fn open_dropped(&mut self, f: egui::DroppedFileHandle, _ctx: &egui::Context) {
        let p = f.path().to_string_lossy().into_owned();
        if !p.is_empty() && f.path().is_absolute() {
            self.open_path(&p);
            return;
        }
        let name = f.path().file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| "dropped.pdf".into());
        match f.bytes() {
            Ok(bytes) => {
                if self.ctx.is_some() && !self.run_inline {
                    let result = self.schedule_open(OpenRequest {
                        name: name.clone(),
                        path: None,
                        stamp: None,
                        input: open_worker::Input::Bytes(std::sync::Arc::new(bytes)),
                        password: None,
                        activate: true,
                        after_open: None,
                        recovered: None,
                        then: None,
                    });
                    if let Err(error) = result {
                        self.notify_fmt("Couldn't open {name}: {e}", &[("name", &name), ("e", &error)]);
                    }
                } else if let Err(error) = self.open_bytes(&name, None, bytes) {
                    self.notify_fmt("Couldn't open {name}: {e}", &[("name", &name), ("e", &error)]);
                }
            }
            Err(e) => self.notify_fmt("Couldn't read {name}: {e}", &[("name", &name), ("e", &e.to_string())]),
        }
    }

    /// The document whose page grid is showing and may take pages, if any.
    fn grid_target(&self) -> Option<pdfcraft_engine::DocId> {
        let view = self.active.and_then(|i| self.views.get(i)).filter(|v| v.organize)?;
        (self.dialog.is_none() && self.session.get(view.id)?.allows_assembly()).then_some(view.id)
    }

    /// The document whose Pages panel is under the pointer and may take pages, if any.
    #[cfg(not(target_arch = "wasm32"))]
    fn panel_target(&self, ctx: &egui::Context) -> Option<pdfcraft_engine::DocId> {
        let view = self.active.and_then(|i| self.views.get(i))?;
        let pointer = ctx.input(|i| i.pointer.hover_pos())?;
        let over = view.pages_panel.is_some_and(|panel| panel.contains(pointer));
        (over && self.dialog.is_none() && self.session.get(view.id)?.allows_assembly()).then_some(view.id)
    }

    /// Files dropped on the Pages panel, or while the page grid shows, are inserted into the
    /// document rather than opened: they are kept until [`Self::finish_grid_drop`] knows the
    /// gap. Returns the files not taken.
    #[cfg(not(target_arch = "wasm32"))]
    fn drop_on_grid(&mut self, dropped: Vec<egui::DroppedFileHandle>, ctx: &egui::Context) -> Vec<egui::DroppedFileHandle> {
        if dropped.is_empty() {
            return dropped;
        }
        // The panel first: the grid takes files dropped anywhere in the window.
        let (id, panel) = match (self.panel_target(ctx), self.grid_target()) {
            (Some(id), _) => (id, true),
            (None, Some(id)) => (id, false),
            (None, None) => return dropped,
        };
        let mut files = Vec::new();
        for f in &dropped {
            let name = f.path().file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| "dropped.pdf".into());
            if f.path().is_absolute() {
                files.push((name, Err(f.path().to_path_buf())));
                continue;
            }
            match f.bytes() {
                Ok(b) => files.push((name, Ok(b))),
                Err(e) => self.notify_fmt("Couldn't read {name}: {e}", &[("name", &name), ("e", &e)]),
            }
        }
        if !files.is_empty() {
            // Where there is no telling where the pointer is during the drag, its place arrives
            // with the first move after the drop.
            ctx.request_repaint();
            self.grid_drop = Some(GridDrop { doc: id, panel, files, deadline: ctx.input(|i| i.time) + 1.0 });
        }
        Vec::new()
    }

    /// Insert files dropped on the page grid or the Pages panel at the gap under the pointer,
    /// or at the end when the pointer hasn't shown up in time.
    fn finish_grid_drop(&mut self, ctx: &egui::Context) {
        let Some(GridDrop { doc: id, panel, deadline, .. }) = &self.grid_drop else { return };
        let (id, panel, deadline) = (*id, *panel, *deadline);
        let still = if panel {
            self.dialog.is_none()
                && self.active_ids().map(|(_, active)| active) == Some(id)
                && self.session.get(id).is_some_and(|d| d.allows_assembly())
        } else {
            self.grid_target() == Some(id)
        };
        if !still {
            self.grid_drop = None;
            return self.notify_tr("The document changed while you were choosing a file, so nothing was changed.");
        }
        let Some(view) = self.active.and_then(|i| self.views.get_mut(i)) else { return };
        let gap = match if panel { view.panel_gap } else { view.grid_gap } {
            Some(gap) => gap,
            None if ctx.input(|i| i.time) >= deadline => usize::MAX,
            None => return ctx.request_repaint(),
        };
        view.insert_at = Some(gap);
        if let Some(drop) = self.grid_drop.take() {
            self.insert_each(drop.files.into_iter().map(read_grid_file));
        }
    }

    /// Browsers read dropped files asynchronously; the bytes land in `inbox` and open next frame.
    #[cfg(target_arch = "wasm32")]
    fn open_dropped(&mut self, f: egui::DroppedFileHandle, ctx: &egui::Context) {
        self.startup_superseded = true;
        let inbox = self.inbox.clone();
        let ctx = ctx.clone();
        wasm_bindgen_futures::spawn_local(async move {
            let name = f.path().file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| "dropped.pdf".into());
            if let Ok(bytes) = f.bytes_async().await
                && let Ok(mut q) = inbox.lock()
            {
                q.push((name, bytes));
                ctx.request_repaint();
            }
        });
    }

    /// A number that changes whenever the open dialog changes (opened, closed or replaced by
    /// another), checked every frame. A picker started from a dialog's Browse… button answers
    /// only into the same showing of that dialog.
    pub(crate) fn dialog_epoch(&mut self) -> u64 {
        if self.dialog != self.dialog_seen {
            self.dialog_seen = self.dialog;
            self.dialog_epoch = self.dialog_epoch.wrapping_add(1);
        }
        self.dialog_epoch
    }

    /// Save an attachment to disk, or open a PDF attachment in a new tab.
    pub fn attachment_action(&mut self, doc: DocId, index: usize, open: bool) {
        let Some(d) = self.session.get(doc) else { return };
        let Some(att) = d.info.attachments.get(index).cloned() else { return };
        let data = pdfcraft_render::attachment_data(&d.bytes, d.password.as_deref(), &att);
        match (data, open) {
            (Err(e), _) => self.notify_fmt("Couldn't read {name}: {e}", &[("name", &att.name), ("e", &e.to_string())]),
            (Ok(bytes), true) => {
                // The tab name later names files written from the tab (Split, Extract pages).
                if let Err(e) = self.open_bytes(&crate::files::attachment_file_name(&att.name), None, bytes) {
                    self.notify_fmt("Couldn't open {name}: {e}", &[("name", &att.name), ("e", &e.to_string())]);
                }
            }
            (Ok(bytes), false) => {
                #[cfg(not(target_arch = "wasm32"))]
                self.ask_one(
                    pickers::Ask::Save(rfd::AsyncFileDialog::new().set_file_name(crate::files::attachment_file_name(&att.name))),
                    None,
                    move |app, path| match crate::editing::write_atomically(&path.to_string_lossy(), &bytes) {
                        Ok(()) => app.notify_fmt("Saved {name}", &[("name", &path.display().to_string())]),
                        Err(e) => app.notify_fmt("Couldn't save: {e}", &[("e", &e.to_string())]),
                    },
                );
                #[cfg(target_arch = "wasm32")]
                self.notify_fmt("Downloading attachments on the web arrives with M3.10 ({n} bytes ready)", &[("n", &bytes.len().to_string())]);
            }
        }
    }

    /// Whether the app draws the window's title bar, buttons and resize edges: the desktop leaves
    /// the frame to the app and Use system title bar is off.
    pub fn draws_window_frame(&self) -> bool {
        self.window_controls.is_some() && !self.system_title_bar
    }

    /// Follow Use system title bar at once: the window starts without winit's frame wherever the
    /// app may draw its own, and gets it back (or loses it again) when the preference changes.
    fn sync_window_frame(&mut self, ctx: &egui::Context) {
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
        self.full_screen = on;
        ctx.send_viewport_cmd(egui::ViewportCommand::Fullscreen(on));
    }

    /// Answer the Full Screen mode prompt a document raised on opening: enter it, or leave the
    /// window as it is.
    pub fn resolve_full_screen_prompt(&mut self, ctx: &egui::Context, allow: bool) {
        let Some(id) = self.pending_full_screen.take() else { return };
        if allow && self.session.get(id).is_some() {
            self.set_full_screen(ctx, true);
        }
    }

    /// Answer the password prompt (`None` cancels).
    pub fn submit_password(&mut self, password: Option<String>) {
        let Some(p) = self.password_prompt.take() else { return };
        let Some(pw) = password else {
            // A cancelled recovery goes back on offer; its snapshot stays in the store and must
            // not attach itself to whatever document is unlocked next (#813).
            if let Some(meta) = p.recovered {
                self.recoverable.push(meta);
            }
            return;
        };
        #[cfg(not(target_arch = "wasm32"))]
        if self.ctx.is_some() && !self.run_inline {
            let result = self.schedule_open(OpenRequest {
                name: p.name.clone(),
                path: p.path.clone(),
                stamp: p.stamp,
                input: open_worker::Input::Bytes(p.bytes.clone()),
                password: Some(pw),
                activate: p.activate,
                after_open: p.after_open.clone(),
                recovered: p.recovered.clone(),
                then: p.then,
            });
            if let Err(error) = result {
                let mut prompt = p;
                prompt.error = Some(error);
                self.password_prompt = Some(prompt);
            }
            return;
        }
        #[cfg(not(target_arch = "wasm32"))]
        let (path_for_watch, stamp_for_watch) = (p.path.clone(), p.stamp);
        let after_open = p.after_open.clone();
        match self.try_open(&p.name, p.path, p.bytes, Some(&pw), p.activate) {
            Err(e) => self.notify_fmt("Couldn't open {name}: {e}", &[("name", &p.name), ("e", &e.to_string())]),
            // A recovered encrypted document is open once its prompt is gone.
            Ok(()) if self.password_prompt.is_none() => {
                #[cfg(not(target_arch = "wasm32"))]
                if let Some(path) = path_for_watch.as_deref()
                    && let Some(id) = self
                        .views
                        .iter()
                        .rev()
                        .find_map(|view| self.session.get(view.id).and_then(|doc| (doc.path.as_deref() == Some(path)).then_some(view.id)))
                {
                    self.watch.known_as(id, stamp_for_watch);
                }
                if let Some(destination) = after_open {
                    self.apply_remote_destination(destination);
                }
                if let Some(meta) = &p.recovered {
                    self.finish_recovery(meta);
                }
                if let Some(command) = p.then {
                    self.execute(command);
                }
            }
            // Wrong password: the new prompt is for the same bytes, so it keeps the snapshot
            // and the tool waiting for it.
            Ok(()) => {
                if let Some(prompt) = self.password_prompt.as_mut() {
                    prompt.recovered = p.recovered;
                    prompt.then = p.then;
                    prompt.after_open = after_open;
                }
            }
        }
    }

    /// Ask for the active document's permissions password, then run `then` (if any).
    pub(crate) fn request_unlock(&mut self, then: Option<&'static str>) {
        let Some(doc) = self.active_ids().and_then(|(_, id)| self.session.get(id)) else { return };
        self.unlock_prompt = Some(UnlockPrompt::new(doc, then));
    }

    /// Answer the permissions password prompt (`None` cancels).
    pub fn submit_unlock(&mut self, password: Option<String>) {
        let Some(mut p) = self.unlock_prompt.take() else { return };
        let Some(pw) = password else { return };
        match self.session.unlock(p.doc, &pw) {
            // Carry on with what asked for the password, if its document is still the one shown.
            Ok(()) => match p.then {
                Some(command) if self.active_ids().is_some_and(|(_, id)| id == p.doc) => {
                    self.execute(command);
                }
                _ => self.notify_tr("Restrictions lifted"),
            },
            Err(pdfcraft_engine::EditError::WrongPassword) => {
                p.input.clear();
                p.error = Some("Incorrect password. Try again.".to_string());
                self.unlock_prompt = Some(p);
            }
            Err(e) => self.notify_fmt("Couldn't unlock {name}: {e}", &[("name", &p.name), ("e", &e.to_string())]),
        }
    }

    /// The tab already showing the file at `path` (the same path, or one that names the same
    /// file once made absolute).
    pub(crate) fn tab_showing(&self, path: &str) -> Option<usize> {
        self.views.iter().position(|v| self.session.get(v.id).and_then(|d| d.path.as_deref()).is_some_and(|p| same_file(p, path)))
    }

    /// Open a file from a recent list: focus the tab already showing it, else open it (File ▸
    /// Open Recent and the Home view's list share this).
    pub fn open_recent(&mut self, path: &str) {
        if let Some(i) = self.tab_showing(path) {
            self.active = Some(i);
        } else {
            #[cfg(not(target_arch = "wasm32"))]
            self.open_path(path);
        }
    }

    #[cfg(not(target_arch = "wasm32"))]
    pub(crate) fn open_remote_link(&mut self, source: DocId, file: String, destination: pdfcraft_render::RemoteDestination) {
        if self.pending_remote_link.is_some() || self.pending_link.is_some() {
            return;
        }
        let Some(source_doc) = self.session.get(source) else { return };
        let source_path = source_doc.path.clone();
        let path = match pdfcraft_engine::resolve_remote_document_path(source_path.as_deref().map(std::path::Path::new), &file) {
            Ok(path) => path,
            Err(e) => {
                self.notify_fmt("Couldn't follow linked PDF: {reason}", &[("reason", &e.to_string())]);
                return;
            }
        };
        if pdfcraft_engine::remote_document_path_is_absolute(&file) {
            self.pending_remote_link = Some(PendingRemoteLink { source, path, destination });
            return;
        }
        self.open_resolved_remote_link(path, destination);
    }

    /// Take a pending request exactly once. Closing its source invalidates the request.
    #[cfg(not(target_arch = "wasm32"))]
    pub fn resolve_pending_remote_link(&mut self, open: bool) {
        if let Some(pending) = self.pending_remote_link.take()
            && open
            && self.session.get(pending.source).is_some()
        {
            self.open_resolved_remote_link(pending.path, pending.destination);
        }
    }

    #[cfg(not(target_arch = "wasm32"))]
    fn open_resolved_remote_link(&mut self, path: std::path::PathBuf, destination: pdfcraft_render::RemoteDestination) {
        let path = path.to_string_lossy().into_owned();
        let name = std::path::Path::new(&path).file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| path.clone());
        let stamp = file_watch::Stamp::at(&path);
        self.startup_superseded = true;
        let result = self.schedule_open(OpenRequest {
            name: name.clone(),
            path: Some(path.clone()),
            stamp,
            input: open_worker::Input::RemotePath(std::path::PathBuf::from(path)),
            password: None,
            activate: true,
            after_open: Some(destination),
            recovered: None,
            then: None,
        });
        if let Err(error) = result {
            self.notify_fmt("Couldn't follow linked PDF: {reason}", &[("reason", &error)]);
        }
    }

    #[cfg(target_arch = "wasm32")]
    pub(crate) fn open_remote_link(&mut self, _source: DocId, _file: String, _destination: pdfcraft_render::RemoteDestination) {
        self.notify_tr("Local cross-PDF links require the desktop app");
    }

    #[cfg(target_arch = "wasm32")]
    pub fn resolve_pending_remote_link(&mut self, _open: bool) {
        self.pending_remote_link = None;
    }

    fn apply_remote_destination(&mut self, destination: pdfcraft_render::RemoteDestination) {
        let Some(index) = self.views.len().checked_sub(1) else { return };
        let Some(doc) = self.session.get(self.views[index].id) else { return };
        let warning = match destination {
            pdfcraft_render::RemoteDestination::Default => None,
            pdfcraft_render::RemoteDestination::Named(_) => Some("The linked PDF opened, but its named destination was not applied."),
            pdfcraft_render::RemoteDestination::Page { page, view } if page < doc.info.pages.len() => {
                self.views[index].go_to_dest(page, view, &doc.info);
                None
            }
            pdfcraft_render::RemoteDestination::Page { .. } => Some("The linked PDF opened, but its page destination was out of range."),
        };
        if let Some(warning) = warning {
            self.notify_tr(warning);
        }
    }

    #[cfg(not(target_arch = "wasm32"))]
    pub fn open_path(&mut self, path: &str) {
        self.open_path_then(path, None);
    }

    /// Keep a chosen tool with its file until preparation and any password entry finish.
    #[cfg(not(target_arch = "wasm32"))]
    fn open_path_then(&mut self, path: &str, then: Option<&'static str>) {
        // One file, one tab (#2166): a file that is open already comes to the front instead,
        // and the tool it was opened for runs on it. One still being opened is left to that.
        if let Some(i) = self.tab_showing(path) {
            self.active = Some(i);
            if let Some(command) = then {
                self.execute(command);
            }
            return;
        }
        if self.pending_opens.iter().any(|p| p.path.as_deref().is_some_and(|p| same_file(p, path))) {
            return;
        }
        let name = std::path::Path::new(path).file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| path.to_string());
        // Taken before reading: a build that rewrites the file meanwhile still counts as a change.
        let stamp = file_watch::Stamp::at(path);
        // Before the first frame there is no interaction to keep responsive; this also preserves
        // deterministic inline startup and test flows. Once live, reading and preparation use
        // the bounded worker and commit on the UI thread.
        if self.ctx.is_none() || self.run_inline {
            match std::fs::read(path) {
                Ok(bytes) => {
                    let before = self.views.len();
                    if let Err(error) = self.open_bytes(&name, Some(path.to_string()), bytes) {
                        self.notify_fmt("Couldn't open {name}: {e}", &[("name", &name), ("e", &error)]);
                    } else if let Some(prompt) = self.password_prompt.as_mut().filter(|p| p.path.as_deref() == Some(path)) {
                        prompt.then = then;
                    } else if self.views.len() > before {
                        if let Some(doc) = self.views.last().map(|view| view.id) {
                            self.watch.known_as(doc, stamp);
                        }
                        if let Some(command) = then {
                            self.execute(command);
                        }
                    }
                }
                Err(error) => self.notify_fmt("Couldn't read {name}: {e}", &[("name", &name), ("e", &error.to_string())]),
            }
            return;
        }
        self.startup_superseded = true;
        if let Err(error) = self.schedule_open(OpenRequest {
            name: name.clone(),
            path: Some(path.to_string()),
            stamp,
            input: open_worker::Input::Path(std::path::PathBuf::from(path)),
            password: None,
            activate: true,
            after_open: None,
            recovered: None,
            then,
        }) {
            self.notify_fmt("Couldn't open {name}: {e}", &[("name", &name), ("e", &error)]);
        }
    }

    /// The File ▸ Open picker: PDFs, and images and text that become PDFs.
    #[cfg(not(target_arch = "wasm32"))]
    fn open_picker() -> rfd::AsyncFileDialog {
        rfd::AsyncFileDialog::new().add_filter("PDF", &["pdf"]).add_filter(tl!("Images and text (converted to PDF)"), &create_ui::CONVERTIBLE)
    }

    pub fn open_dialog(&mut self) {
        self.startup_superseded = true;
        #[cfg(not(target_arch = "wasm32"))]
        self.pick(
            pickers::PickFor::Open,
            rfd::AsyncFileDialog::new().add_filter("PDF", &["pdf"]).add_filter(tl!("Images and text (converted to PDF)"), &create_ui::CONVERTIBLE),
            true,
        );
        // Browsers pick files asynchronously; the bytes arrive through `inbox`.
        #[cfg(target_arch = "wasm32")]
        {
            let inbox = self.inbox.clone();
            let ctx = self.ctx.clone();
            wasm_bindgen_futures::spawn_local(async move {
                if let Some(h) = rfd::AsyncFileDialog::new()
                    .add_filter("PDF", &["pdf"])
                    .add_filter(tl!("Images and text"), &create_ui::CONVERTIBLE)
                    .pick_file()
                    .await
                {
                    let bytes = h.read().await;
                    if let Ok(mut q) = inbox.lock() {
                        q.push((h.file_name(), bytes));
                    }
                    // The read may finish while the app is idle: wake it to open the file.
                    if let Some(ctx) = ctx {
                        ctx.request_repaint();
                    }
                }
            });
        }
    }

    pub fn close_tab(&mut self, index: usize) {
        if index >= self.views.len() {
            return;
        }
        let id = self.views.remove(index).id;
        #[cfg(not(target_arch = "wasm32"))]
        if let Some(path) = self.session.get(id).and_then(|d| d.path.clone()) {
            if self.closed_tabs.len() == 10 {
                self.closed_tabs.remove(0);
            }
            self.closed_tabs.push(path);
        }
        self.forget_recovery(id);
        self.session.close(id);
        self.active = match self.active {
            _ if self.views.is_empty() => None,
            Some(a) if a > index => Some(a - 1),
            Some(a) if a >= self.views.len() => Some(self.views.len() - 1),
            other => other,
        };
    }

    /// Moves the tab at `from` so it sits at `to` (#1537). The same document stays active
    /// wherever it lands; an index out of range changes nothing.
    pub fn move_tab(&mut self, from: usize, to: usize) {
        if from >= self.views.len() || to >= self.views.len() || from == to {
            return;
        }
        let active = self.active_ids().map(|(_, id)| id);
        let view = self.views.remove(from);
        self.views.insert(to, view);
        if let Some(id) = active {
            self.active = self.views.iter().position(|v| v.id == id);
        }
    }

    pub fn active_ids(&self) -> Option<(usize, DocId)> {
        self.active.and_then(|i| self.views.get(i).map(|v| (i, v.id)))
    }

    /// A dialog or prompt is open over the window: the keyboard is its own, and nothing may act
    /// on the document underneath it.
    pub(crate) fn modal_open(&self) -> bool {
        self.dialog.is_some()
            || self.close_request.is_some()
            || self.password_prompt.is_some()
            || self.unlock_prompt.is_some()
            || self.pending_link.is_some()
            || self.pending_remote_link.is_some()
            || self.updates.open
    }

    /// Enable the UI control channel on `ctx` (opt-in; see [`control`]). Returns a client that
    /// sends requests to this app; [`control::serve`] exposes it on loopback.
    pub fn attach_control(&mut self, ctx: &egui::Context) -> control::ControlClient {
        let (control, client) = control::attach(ctx);
        self.control = Some(control);
        client
    }

    /// Open a web link in the system browser (a new tab on the web). Only for PdfCraft's own
    /// links; an address that came from a document goes through [`Self::request_document_url`].
    pub fn open_url(&mut self, url: &str) {
        if let Some(ctx) = &self.ctx {
            ctx.open_url(egui::OpenUrl::new_tab(url));
        }
        self.last_opened_url = Some(url.to_string());
    }

    /// A document asks to open `url` (a link, a button's URI action or `app.launchURL`). Web and
    /// email addresses wait for the user to allow them; anything else is refused with a notice
    /// (#90, #91). While one request is waiting, further ones are dropped, so a script can't
    /// queue up a stream of dialogs.
    pub fn request_document_url(&mut self, url: &str, origin: LinkOrigin) {
        match pdfcraft_engine::links::document_url(url) {
            Ok(url) => {
                if self.pending_link.is_none() && self.pending_remote_link.is_none() {
                    self.pending_link = Some(PendingLink { url, origin });
                }
            }
            Err(e) => {
                use pdfcraft_engine::links::BlockedLink;
                let template = if matches!(e, BlockedLink::MailFile | BlockedLink::MailEncodedWord) {
                    "{who} in this document tried to open an address PdfCraft won't open: {e}."
                } else {
                    "{who} in this document tried to open an address PdfCraft won't open: {e}. Only web (http, https) and email (mailto) links open from documents."
                };
                self.notify_fmt(template, &[("who", tl!(origin.noun())), ("e", &e.to_string())]);
            }
        }
    }

    /// The user's answer to [`Self::pending_link`]: open it, or drop it.
    pub fn resolve_pending_link(&mut self, open: bool) {
        if let Some(p) = self.pending_link.take()
            && open
        {
            self.open_url(&p.url);
        }
    }

    pub fn notify(&mut self, msg: impl Into<String>) {
        self.toast = Some((msg.into(), 0.0));
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

    pub fn set_theme_preference(&mut self, preference: ThemePreference) {
        self.theme_preference = preference;
        if let Some(ctx) = self.ctx.clone() {
            self.sync_theme(&ctx);
            theme::apply(&ctx, self.theme);
        } else {
            self.theme = preference.resolve(None, ThemeKind::Light);
        }
    }

    /// The app-wide reading-colours preference: sets it and applies it to every open document,
    /// not just the active one (Preferences ▸ Reading colours, Acrobat's Replace Document
    /// Colors). A document the change actually touches re-renders.
    pub fn set_reading_colors(&mut self, colors: pdfcraft_render::ReadingColors) {
        self.reading_colors = colors;
        for i in 0..self.views.len() {
            // Idempotent per document: the engine says whether anything changed.
            if self.session.set_reading_colors(self.views[i].id, colors) {
                self.views[i].invalidate_content();
            }
        }
    }

    /// Enable the Linux desktop portal in a native window. Headless contexts keep
    /// their supplied input and never connect to the user's session bus.
    #[cfg(target_os = "linux")]
    pub fn enable_desktop_theme(&mut self) {
        self.desktop_theme_enabled = true;
    }

    /// Draw the running job's progress card and pass a Cancel click on to the job.
    fn show_progress(&mut self, ctx: &egui::Context) {
        if widgets::progress_notice(self, ctx) {
            self.cancel_optimize();
        }
    }

    fn sync_theme(&mut self, ctx: &egui::Context) {
        #[cfg_attr(not(target_os = "linux"), expect(unused_mut))]
        let mut system = ctx.system_theme().map(|theme| theme == egui::Theme::Dark);
        #[cfg(target_os = "linux")]
        {
            let active = self.theme_preference == ThemePreference::System;
            if self.desktop_theme_enabled && active && self.desktop_theme.is_none() {
                let ctx = ctx.clone();
                self.desktop_theme = Some(pdfcraft_platform::desktop_theme::DesktopTheme::start(move || ctx.request_repaint()));
            }
            if let Some(desktop) = &self.desktop_theme {
                desktop.set_active(active);
                // The standard portal supplies Linux desktop appearance. No preference
                // must clear an earlier answer rather than retaining stale dark chrome.
                system = desktop.get().map(|appearance| appearance == pdfcraft_platform::desktop_theme::DesktopAppearance::Dark);
            }
        }
        let kind = self.theme_preference.resolve(system, ThemeKind::Light);
        if kind != self.theme {
            self.theme = kind;
            theme::apply(ctx, kind);
        }
    }

    /// Run a catalogue command. Commands that aren't implemented yet say which milestone ships them.
    pub fn run_command(&mut self, command: &str) {
        if let Some(spec) = pdfcraft_engine::commands::command(command) {
            use pdfcraft_engine::commands::Needs;
            // A tool that needs a document, chosen with none open, asks for the file and then
            // runs on it, instead of only saying to open one first.
            let needs_document = !matches!(spec.needs, Needs::Nothing | Needs::Undo | Needs::Redo);
            let is_tool = pdfcraft_engine::catalog::TOOL_GROUPS
                .iter()
                .flat_map(|g| g.sections.iter().flat_map(|s| s.items.iter()))
                .any(|i| i.command == spec.id);
            if self.active.is_none() && needs_document && is_tool && !self.command_enabled(spec) {
                self.open_then_run(spec);
                return;
            }
            self.execute(command);
            return;
        }
        // Not implemented yet: say which milestone ships it.
        let when = pdfcraft_engine::catalog::TOOL_GROUPS
            .iter()
            .flat_map(|g| g.sections.iter().flat_map(|s| s.items.iter()))
            .find(|i| i.command == command)
            .map(|i| match i.availability {
                pdfcraft_engine::catalog::Availability::Planned(m) => crate::i18n::fmt(tl!("ships in milestone {m}"), &[("m", m)]),
                pdfcraft_engine::catalog::Availability::Provider => tl!("needs an AI provider (off by default)").to_string(),
                pdfcraft_engine::catalog::Availability::Ready => tl!("is available").to_string(),
            })
            .unwrap_or_else(|| tl!("is not available yet").to_string());
        self.notify_fmt("`{command}` {when}", &[("command", command), ("when", &when)]);
    }

    /// Ask for a file to open, then run `spec` on it once it is open (or unlocked). Nothing runs
    /// when the picker is cancelled or the file doesn't open.
    #[cfg(not(target_arch = "wasm32"))]
    fn open_then_run(&mut self, spec: &'static pdfcraft_engine::commands::CommandSpec) {
        self.startup_superseded = true;
        let dialog = Self::open_picker().set_title(tl!(spec.label).trim_end_matches('…'));
        let command = spec.id;
        self.ask_one(pickers::Ask::File(dialog), None, move |app, path| {
            let path = path.to_string_lossy().into_owned();
            app.open_path_then(&path, Some(command));
        });
    }

    /// Browsers read the picked file asynchronously; it opens, and the tool can be chosen again.
    #[cfg(target_arch = "wasm32")]
    fn open_then_run(&mut self, _spec: &'static pdfcraft_engine::commands::CommandSpec) {
        self.open_dialog();
    }

    /// Today in `fmt` (or Preferences ▸ Date format), with month and weekday names in the date
    /// language, or the interface language when it follows that.
    pub fn date_text(&self, fmt: Option<&str>) -> Result<String, String> {
        date_text(&self.session, fmt)
    }

    pub(crate) fn reveal_tools(&mut self) {
        self.left_open = true;
        self.pending_panel_reveals.push(docking::Panel::Tools);
    }

    /// Enter a tool from the catalogue, Home, palette or control channel. Organize is a
    /// workspace, so opening its panel also opens the page grid (without toggling it off).
    pub(crate) fn select_tool(&mut self, id: &'static str) {
        self.left = LeftPanel::Tool(id);
        self.reveal_tools();
        if let Some(view) = self.active.and_then(|i| self.views.get_mut(i)) {
            if id == "organize" {
                view.open_organize();
            } else {
                view.organize = false;
            }
        }
    }

    /// Select the workspace and its matching tool panel, just like the mode bar. Edit turns on
    /// Edit text & images at once, as Acrobat does: every paragraph and image on the page gets its
    /// box and is editable with one click. Leaving Edit puts the editing tools down again.
    pub(crate) fn select_mode(&mut self, mode: Mode) {
        self.mode = mode;
        self.reveal_tools();
        if let Some(view) = self.active.and_then(|i| self.views.get_mut(i)) {
            view.organize = false;
        }
        self.left = match mode {
            Mode::Edit => LeftPanel::Tool("edit"),
            Mode::Convert => LeftPanel::Tool("export"),
            Mode::Sign => LeftPanel::Tool("fill_sign"),
            _ => LeftPanel::AllTools,
        };
        if mode == Mode::Edit {
            self.quick_tool = QuickTool::EditText;
        } else if matches!(self.quick_tool, QuickTool::EditText | QuickTool::AddText) {
            self.put_down_edit_text();
        }
    }

    /// Back to the Select tool from an editing tool. A paragraph still open in the editor is
    /// applied, as clicking away from it does; left open, its page would keep showing the
    /// paragraph's text removed with no editor over it.
    pub(crate) fn put_down_edit_text(&mut self) {
        self.quick_tool = QuickTool::Select;
        if let Some(ed) = self.active.and_then(|i| self.views.get_mut(i)).and_then(|v| v.line_editor.take())
            && let Some(edit) = edit_text_ui::finished_edit(ed)
        {
            self.apply_edit(edit);
        }
    }

    /// Switches workspace because the user chose to (mode bar, Read mode command). Entering Read
    /// mode closes the right panel, as Acrobat's Read mode hides its panes, and leaving it reopens
    /// that panel. The rail stays in Read mode, and its buttons open panels there too (#932).
    pub(crate) fn switch_mode(&mut self, mode: Mode, select_tools: bool) {
        let was_read = self.mode == Mode::Read;
        if select_tools {
            self.select_mode(mode);
        } else {
            self.mode = mode;
        }
        if mode == Mode::Read && !was_read {
            self.right_before_read = self.right.take();
        } else if mode != Mode::Read && was_read {
            let before = self.right_before_read.take();
            if self.right.is_none() {
                self.right = before;
            }
            if self.right.is_some() {
                self.pending_panel_reveals.push(docking::Panel::Inspector);
            }
        }
    }

    /// Opens a right panel, or closes it with `None`, because the user chose to. Closing Comments
    /// keeps comment tools from reopening it; opening it again lets them (#225). A choice made in
    /// Read mode replaces the pane saved on the way in, so leaving Read mode doesn't undo it.
    pub fn choose_right_panel(&mut self, panel: Option<RightPanel>) {
        if self.mode == Mode::Read {
            self.right_before_read = None;
        }
        if panel == Some(RightPanel::Comments) {
            self.comments_panel_closed = false;
        } else if panel.is_none() && self.right == Some(RightPanel::Comments) {
            self.comments_panel_closed = true;
        }
        if panel.is_some() {
            self.right_panel_closed = false;
        } else if self.right.is_some() {
            self.right_panel_closed = true;
        }
        self.right = panel;
        if panel.is_some() {
            self.pending_panel_reveals.push(docking::Panel::Inspector);
        }
    }

    /// Serialize the user's persistent state (recent files, theme). Local only.
    pub fn persist(&self) -> String {
        let trusted: Vec<String> = self.session.trusted_certificates().iter().map(pdfcraft_engine::sign::x509::to_pem).collect();
        let mut v = serde_json::json!({
            "docking": self.docking,
            "docking_left_open": self.left_open,
            "docking_right": self.right,
            "recent": self.recent,
            "reopen_last_session": self.reopen_last_session,
            // Kept only while the preference is on.
            "last_session": self.reopen_last_session.then(|| self.session_to_save()),
            "reload_changed_files": self.reload_changed_files,
            "system_title_bar": self.system_title_bar,
            "pinned_folders": self.pinned.folders,
            "theme": self.theme_preference,
            "scroll_speed": self.scroll_speed,
            "animation_effects": self.animation_effects,
            "default_mode": self.default_mode,
            "default_layout": canvas::display_token(self.view_defaults.layout, self.view_defaults.continuous),
            "default_zoom": self.view_defaults.zoom_name(),
            "highlight_fields": self.view_defaults.highlight_fields,
            "units": self.units,
            "language": self.language,
            "reading_colors": self.reading_colors,
            // Null follows the system's text scaling.
            "ui_scale": self.ui_scale,
            "flatten_fill_sign": self.flatten_fill_sign_on_save,
            "fill_text_size": self.fill_text_size,
            "date_format": self.session.date_format(),
            // Null follows the interface language.
            "date_language": self.session.date_language(),
            "author": self.comment_prefs.author,
            // Per-tool comment defaults (colours, opacity, widths), so Make Current
            // Properties Default survives a restart (#340).
            "comment_styles": self
                .comment_prefs
                .styles()
                .map(|(t, s)| serde_json::json!({
                    "tool": t.command(),
                    "color": s.color,
                    "opacity": s.opacity,
                    "width": s.width,
                    "fill": s.fill,
                }))
                .collect::<Vec<_>>(),
            // Drawn signatures keep their original form (older settings read the same).
            "signature": match &self.signature { Some(fill_sign::SavedSig::Drawn(s)) => Some(s), _ => None },
            "signature_text": match &self.signature { Some(fill_sign::SavedSig::Typed(t)) => Some(t), _ => None },
            "signature_image": match &self.signature { Some(s @ fill_sign::SavedSig::Image(_)) => Some(s), _ => None },
            "initials": self.initials,
            "sign_graphic": self.sign_graphic,
            // macOS Keychain and Windows store identities are read from their OS key stores each time.
            "digital_ids": self.digital_ids.iter().filter(|e| !e.path.starts_with("keychain:") && !e.path.starts_with("windows:")).collect::<Vec<_>>(),
            "trusted": trusted,
            "custom_stamps": stamps_ui::encode(&self.custom_stamps),
            "javascript": self.session.javascript(),
            "actions": actions_ui::encode(&self.custom_actions),
            "combine_columns": self.combine_columns.to_json(),
            "combine_view": self.combine_view.as_str(),
            "combine_zoom": self.combine_zoom,
            "comments_panel_closed": self.comments_panel_closed,
            "right_panel_closed": self.right_panel_closed,
        });
        // Set apart: one more key inside the macro above passes serde_json's recursion limit.
        v["redact_prefs"] = self.redact_defaults.to_json();
        // Set outside json! above, which is at serde_json's macro recursion limit.
        v["hide_empty_highlight_hover"] = serde_json::Value::Bool(self.comment_prefs.hide_empty_highlight_hover);
        // Null: no time server (signing uses this computer's clock).
        v["time_server"] = self.time_server.url().map_or(serde_json::Value::Null, |url| serde_json::Value::String(url.to_string()));
        v.to_string()
    }

    /// Restore state written by `persist`. Unknown or malformed data is ignored.
    pub fn restore(&mut self, json: &str) {
        let Ok(v) = serde_json::from_str::<serde_json::Value>(json) else { return };
        if let Some(saved) = v.get("docking")
            && let Ok(workspace) = serde_json::from_value::<docking::Workspace>(saved.clone())
            && workspace.validate().is_ok()
        {
            self.docking = workspace;
            self.left_open =
                v.get("docking_left_open").and_then(serde_json::Value::as_bool).unwrap_or(self.docking.layout.contains(&docking::Panel::Tools));
            self.right = v.get("docking_right").and_then(|value| serde_json::from_value(value.clone()).ok()).flatten();
        }
        self.combine_columns = combine_ui::Columns::from_json(&v["combine_columns"]);
        if let Some(view) = v["combine_view"].as_str().and_then(combine_ui::CombineView::parse) {
            self.combine_view = view;
        }
        if let Some(zoom) = v["combine_zoom"].as_f64() {
            self.set_combine_zoom(zoom as f32);
        }
        if let Ok(r) = serde_json::from_value::<Vec<RecentFile>>(v["recent"].clone()) {
            // Only keep entries whose files still exist.
            #[cfg(not(target_arch = "wasm32"))]
            let r: Vec<RecentFile> = r.into_iter().filter(|f| std::path::Path::new(&f.path).exists()).collect();
            self.recent = r;
        }
        if let Some(on) = v["reopen_last_session"].as_bool() {
            self.reopen_last_session = on;
        }
        self.last_session = last_session::LastSession::from_json(&v["last_session"]);
        if let Some(on) = v["system_title_bar"].as_bool() {
            self.system_title_bar = on;
        }
        if let Some(on) = v["reload_changed_files"].as_bool() {
            self.reload_changed_files = on;
        }
        self.pinned.restore(&v["pinned_folders"]);
        if let Ok(preference) = serde_json::from_value::<ThemePreference>(v["theme"].clone()) {
            self.set_theme_preference(preference);
        }
        if let Ok(speed) = serde_json::from_value::<scroll_speed::ScrollSpeed>(v["scroll_speed"].clone()) {
            self.scroll_speed = speed;
        }
        if let Ok(effects) = serde_json::from_value::<motion::AnimationEffects>(v["animation_effects"].clone()) {
            self.animation_effects = effects;
        }
        if let Ok(mode) = serde_json::from_value::<Mode>(v["default_mode"].clone()) {
            self.default_mode = mode;
        }
        if let Some((layout, continuous)) = v["default_layout"].as_str().and_then(canvas::parse_display_token) {
            (self.view_defaults.layout, self.view_defaults.continuous) = (layout, continuous);
        }
        if let Ok(units) = serde_json::from_value::<units::Unit>(v["units"].clone()) {
            self.units = units;
        }
        if let Some(on) = v["highlight_fields"].as_bool() {
            self.view_defaults.highlight_fields = on;
        }
        if let Some(closed) = v["comments_panel_closed"].as_bool() {
            self.comments_panel_closed = closed;
        }
        if let Some(closed) = v["right_panel_closed"].as_bool() {
            self.right_panel_closed = closed;
        }
        if let Some(defaults) = v["default_zoom"].as_str().and_then(|zoom| self.view_defaults.with_zoom(zoom)) {
            self.view_defaults = defaults;
        }
        if let Some(language) = v["language"].as_str().and_then(i18n::normalize_pref) {
            self.language = language.to_string();
        }
        if let Some(on) = v["flatten_fill_sign"].as_bool() {
            self.flatten_fill_sign_on_save = on;
        }
        if let Some(size) = v["fill_text_size"].as_f64().and_then(fill_sign::valid_text_size) {
            self.fill_text_size = size;
        }
        if let Some(prefs) = RedactPrefs::from_json(&v["redact_prefs"]) {
            self.redact_prefs = prefs.clone();
            self.redact_defaults = prefs;
        }
        // Settings are untrusted: an unreadable value (a colour that isn't three bytes, say)
        // fails to deserialize and keeps Off.
        if let Ok(colors) = serde_json::from_value::<pdfcraft_render::ReadingColors>(v["reading_colors"].clone()) {
            self.reading_colors = colors;
        }
        // Settings are untrusted: a size out of range follows the system instead.
        self.ui_scale = v["ui_scale"].as_f64().and_then(ui_scale::valid);
        // Settings are untrusted: an unusable pattern keeps the default.
        if let Some(f) = v["date_format"].as_str() {
            let _ = self.session.set_date_format(f);
        }
        if let Some(l) = v["date_language"].as_str() {
            let _ = self.session.set_date_language(Some(l));
        }
        // An empty or missing name keeps the login-name default; settings are untrusted, so the
        // name is cut to a sane length.
        if let Some(author) = v["author"].as_str().map(str::trim).filter(|a| !a.is_empty()) {
            self.comment_prefs.author = author.chars().take(MAX_AUTHOR_CHARS).collect();
        }
        if let Some(hide) = v["hide_empty_highlight_hover"].as_bool() {
            self.comment_prefs.hide_empty_highlight_hover = hide;
        }
        // Per-tool comment styles (#340): each field goes through its clamping setter, so a
        // malformed or hostile entry is clamped or ignored rather than trusted.
        if let Some(styles) = v["comment_styles"].as_array() {
            for e in styles {
                let Some(tool) = e["tool"].as_str().and_then(crate::comments::CommentTool::from_command) else { continue };
                if let Some(c) = rgb_triple(&e["color"]) {
                    self.comment_prefs.set_color(tool, c);
                }
                if let Some(o) = e["opacity"].as_f64() {
                    self.comment_prefs.set_opacity(tool, o);
                }
                if let Some(w) = e["width"].as_f64() {
                    self.comment_prefs.set_width(tool, w);
                }
                // `"fill": null` is a saved "no fill", which clears a tool's default fill; a
                // missing or malformed fill keeps it.
                let fill = match e.get("fill") {
                    Some(serde_json::Value::Null) => Some(None),
                    Some(v) => rgb_triple(v).map(Some),
                    None => None,
                };
                if let Some(fill) = fill {
                    let mut s = self.comment_prefs.style(tool);
                    s.fill = fill;
                    self.comment_prefs.set_style(tool, s);
                }
            }
        }
        if let Ok(s) = serde_json::from_value::<Vec<Vec<[f32; 2]>>>(v["signature"].clone())
            && s.iter().all(|st| st.iter().all(|p| p.iter().all(|x| x.is_finite())))
        {
            self.signature = Some(fill_sign::SavedSig::Drawn(s));
        }
        if let Some(t) = v["signature_text"].as_str().filter(|t| !t.trim().is_empty()) {
            self.signature = Some(fill_sign::SavedSig::Typed(t.to_string()));
        }
        if let Ok(s @ fill_sign::SavedSig::Image(_)) = serde_json::from_value::<fill_sign::SavedSig>(v["signature_image"].clone()) {
            self.signature = Some(s);
        }
        if let Ok(i) = serde_json::from_value::<fill_sign::SavedSig>(v["initials"].clone()) {
            self.initials = Some(i);
        }
        // An unreadable choice (a damaged image) falls back to the name.
        self.sign_graphic = serde_json::from_value(v["sign_graphic"].clone()).unwrap_or_default();
        // Settings are untrusted: an address that isn't a valid http(s) one leaves no server.
        self.time_server = sign_ui::TimeServer::restore(&v["time_server"]);
        if let Ok(ids) = serde_json::from_value::<Vec<DigitalIdEntry>>(v["digital_ids"].clone()) {
            self.digital_ids = ids;
        }
        self.custom_stamps = stamps_ui::decode(&v["custom_stamps"]);
        self.custom_actions = actions_ui::decode(&v["actions"]);
        if let Some(on) = v["javascript"].as_bool() {
            self.session.set_javascript(on);
        }
        if let Ok(pems) = serde_json::from_value::<Vec<String>>(v["trusted"].clone()) {
            let certs = pems.iter().filter_map(|p| pdfcraft_engine::sign::x509::load_certificates(p.as_bytes()).ok()).flatten().collect();
            self.apply_trusted_certificates(certs);
        }
    }

    /// `true` while any open document still waits for page renders (used by headless capture).
    pub fn render_pending(&self) -> bool {
        self.views.iter().any(|v| v.render_pending())
    }

    /// Apply a named view option (`--page 3`, `--panel bookmarks`, `--theme dark`, …).
    ///
    /// This is the seed of the UI control channel (M3.9): the same verbs become `ui.set` calls.
    pub fn set_option(&mut self, key: &str, value: &str) -> Result<(), String> {
        let view = self.active.and_then(|i| self.views.get_mut(i));
        match (key, view) {
            ("language", _) => {
                let language = i18n::normalize_pref(value).ok_or_else(|| {
                    let codes: Vec<&str> = std::iter::once(i18n::AUTO).chain(i18n::Lang::all().map(i18n::Lang::code)).collect();
                    format!("language must be one of {}", codes.join(", "))
                })?;
                self.language = language.to_string();
            }
            ("theme", _) => {
                let preference = match value {
                    "system" => ThemePreference::System,
                    "light" => ThemePreference::Light,
                    "dark" => ThemePreference::Dark,
                    _ => return Err("theme must be light, dark, or system".into()),
                };
                self.set_theme_preference(preference);
            }
            ("scroll_speed", _) => {
                self.scroll_speed = scroll_speed::ScrollSpeed::parse(value).ok_or("scroll_speed must be slow, normal, or fast")?;
            }
            ("animation_effects", _) => {
                self.animation_effects = motion::AnimationEffects::parse(value).ok_or("animation_effects must be system, on, or off")?;
            }
            ("panel", _) => {
                self.right = match value {
                    "comments" => Some(RightPanel::Comments),
                    "bookmarks" => Some(RightPanel::Bookmarks),
                    "pages" => Some(RightPanel::Pages),
                    "fields" => Some(RightPanel::Fields),
                    "layers" => Some(RightPanel::Layers),
                    "attachments" => Some(RightPanel::Attachments),
                    "signatures" => Some(RightPanel::Signatures),
                    "accessibility" => Some(RightPanel::Accessibility),
                    "tags" => Some(RightPanel::Tags),
                    "search" => Some(RightPanel::Search),
                    "compare" => Some(RightPanel::Compare),
                    "none" => None,
                    other => return Err(format!("unknown panel {other}")),
                }
            }
            ("mode", _) => {
                let mode = Mode::parse(value).unwrap_or(Mode::AllTools);
                self.mode_override = Some(mode);
                self.mode = mode;
            }
            ("default-mode", _) => {
                self.default_mode = Mode::parse(value).ok_or("default-mode must be all, read, edit, convert or sign")?;
            }
            ("date-format", _) => self.session.set_date_format(value)?,
            ("date-language", _) => self.session.set_date_language(Some(value).filter(|v| *v != "auto"))?,
            ("tool", _) => {
                let g = pdfcraft_engine::catalog::group(value).ok_or_else(|| format!("unknown tool {value}"))?;
                self.select_tool(g.id);
            }
            ("left", _) => self.left_open = value != "closed",
            ("home", _) => self.active = None,
            ("dialog", _) => {
                self.dialog = match value {
                    "properties" => Some(Dialog::Properties(PropsTab::Description)),
                    "security" => Some(Dialog::Properties(PropsTab::Security)),
                    "fonts" => Some(Dialog::Properties(PropsTab::Fonts)),
                    "advanced" => Some(Dialog::Properties(PropsTab::Advanced)),
                    "shortcuts" => Some(Dialog::Shortcuts),
                    "split" => Some(Dialog::Split),
                    "protect" => Some(Dialog::Protect),
                    "page-boxes" => Some(Dialog::PageBoxes),
                    "header-footer" => Some(Dialog::Marks(pdfcraft_engine::MarkKind::HeaderFooter)),
                    "watermark" => Some(Dialog::Marks(pdfcraft_engine::MarkKind::Watermark)),
                    "background" => Some(Dialog::Marks(pdfcraft_engine::MarkKind::Background)),
                    "export-image" => Some(Dialog::Export(export_ui::ExportKind::Image)),
                    "word-count" => Some(Dialog::WordCount),
                    "export-text" => Some(Dialog::Export(export_ui::ExportKind::Text)),
                    "export-all-images" => Some(Dialog::Export(export_ui::ExportKind::AllImages)),
                    "accessibility-options" => Some(Dialog::AccessibilityOptions),
                    "recognize-text" => Some(Dialog::RecognizeText),
                    "js-console" => Some(Dialog::JsConsole),
                    "document-js" => Some(Dialog::DocumentJs),
                    "preferences" => Some(Dialog::Preferences),
                    "compare-files" => Some(Dialog::CompareFiles),
                    "action-wizard" => Some(Dialog::ActionWizard),
                    "pdfa" => Some(Dialog::PdfA),
                    "signature" => Some(Dialog::Signature),
                    "optimize" => Some(Dialog::Optimize),
                    "sign" | "certify" => {
                        // Sign with a Digital ID for an invisible signature on the current page.
                        let page = self.active.map_or(0, |i| self.views[i].current);
                        self.start_signing(page, None, None, (value == "certify").then_some(2));
                        Some(Dialog::Sign)
                    }
                    "number-pages" => {
                        // Same path as the menu, so the page range is seeded.
                        self.execute("page.number");
                        Some(Dialog::NumberPages)
                    }
                    "none" => None,
                    _ => Some(Dialog::About),
                }
            }
            ("tools", _) => self.all_tools_expanded = value != "collapsed",
            ("palette", _) => {
                self.palette_open = true;
                self.palette_query = value.to_string();
            }
            ("page", Some(v)) => v.go_to_page(value.parse::<usize>().map_err(|e| e.to_string())?.saturating_sub(1)),
            ("zoom", Some(v)) => v.set_zoom(value.trim_end_matches('%').parse::<f32>().map_err(|e| e.to_string())? / 100.0),
            ("layout", Some(v)) => {
                let (layout, continuous) = canvas::parse_display_token(value).ok_or("layout must be single, continuous, two-up or two-page")?;
                v.set_display(layout, continuous);
            }
            ("continuous", Some(v)) => match value {
                "on" => v.set_continuous(true),
                "off" => v.set_continuous(false),
                _ => return Err("continuous must be on or off".into()),
            },
            ("cover", Some(v)) => {
                let on = match value {
                    "on" => true,
                    "off" => false,
                    _ => return Err("cover must be on or off".into()),
                };
                if on && !v.cover_applies() {
                    return Err("switch to two-page view first to show the cover page".into());
                }
                v.set_cover(on);
            }
            ("default-layout", _) => {
                let (layout, continuous) =
                    canvas::parse_display_token(value).ok_or("default-layout must be single, continuous, two-up or two-page")?;
                (self.view_defaults.layout, self.view_defaults.continuous) = (layout, continuous);
            }
            ("default-continuous", _) => {
                self.view_defaults.continuous = match value {
                    "on" => true,
                    "off" => false,
                    _ => return Err("default-continuous must be on or off".into()),
                };
            }
            ("default-zoom", _) => {
                self.view_defaults =
                    self.view_defaults.with_zoom(value).ok_or("default-zoom must be fit-width, fit-page or a percentage from 8 to 6400")?;
            }
            ("tools-side", _) => self.docking.set_tools_on_right(match value {
                "left" => false,
                "right" => true,
                _ => return Err("tools-side must be left or right".into()),
            })?,
            ("organize", Some(v)) => {
                if value == "off" {
                    v.organize = false;
                } else {
                    v.open_organize();
                }
            }
            // `--grid-zoom 150`: the size of the pages in the organize grid, in percent.
            ("grid-zoom", Some(v)) => {
                let percent = value.trim_end_matches('%').parse::<f32>().map_err(|e| e.to_string())?;
                if !percent.is_finite() || !canvas::GRID_ZOOM_RANGE.contains(&(percent / 100.0)) {
                    return Err("grid-zoom must be between 50 and 300".into());
                }
                v.set_grid_zoom(percent / 100.0);
            }
            ("rotate", Some(v)) => {
                let deg: u16 = value.parse().map_err(|_| "rotate: 0, 90, 180 or 270")?;
                if !deg.is_multiple_of(90) {
                    return Err("rotate: 0, 90, 180 or 270".into());
                }
                v.rotation = deg % 360;
            }
            ("layer", _) => {
                // `--layer "Name=off"` / `"Name=on"`
                let (name, state) = value.rsplit_once('=').ok_or("expected NAME=on|off")?;
                let (i, id) = self.active_ids().ok_or("`layer` needs an open document")?;
                let idx = self
                    .session
                    .get(id)
                    .and_then(|d| d.info.layers.iter().position(|l| l.name == name))
                    .ok_or_else(|| format!("no layer named {name}"))?;
                if self.session.set_layer_visible(id, idx, state != "off") {
                    self.views[i].invalidate_content();
                }
            }
            ("find", Some(v)) => {
                v.open_find();
                if let Some(f) = v.find.as_mut() {
                    f.query = value.to_string();
                }
                v.rerun_find();
            }
            ("fields", Some(v)) => v.highlight_fields = value != "off",
            ("select", Some(v)) => {
                // The same physical page ranges as the Organize Pages selection field.
                let pages = crate::page_selection_ui::parse(value, v.page_count()).map_err(|e| e.to_string())?;
                v.select_pages(&pages);
            }
            ("notice", Some(v)) => v.notice_dismissed = value == "off",
            ("quick", _) => {
                // `--quick select|hand|note|freetext|highlight|underline|strikeout|ink|line|arrow|square|circle`
                self.quick_tool = match value {
                    "select" => QuickTool::Select,
                    measure if measure.starts_with("measure-") => QuickTool::Measure(
                        measure_ui::Tool::from_name(measure.trim_start_matches("measure-")).ok_or_else(|| format!("unknown tool {measure}"))?,
                    ),
                    "hand" => QuickTool::Hand,
                    "crop" => QuickTool::Crop,
                    "enhance-camera" => QuickTool::EnhanceCamera,
                    "redact" => QuickTool::Redact,
                    "add-text" => QuickTool::AddText,
                    "edit-text" => QuickTool::EditText,
                    "link" => QuickTool::Link,
                    "sign" => QuickTool::SignArea { certify: false },
                    "marquee-zoom" => QuickTool::MarqueeZoom,
                    "snapshot" => QuickTool::Snapshot,
                    "column-select" => QuickTool::ColumnSelect,
                    "certify" => QuickTool::SignArea { certify: true },
                    custom if custom.starts_with("custom-stamp-") => {
                        let i: usize = custom[13..].parse().map_err(|_| format!("bad stamp {custom}"))?;
                        if i >= self.custom_stamps.len() {
                            return Err(format!("there are {} custom stamps", self.custom_stamps.len()));
                        }
                        QuickTool::CustomStamp(i)
                    }
                    stamp if stamp.starts_with("stamp-") => QuickTool::Stamp(
                        pdfcraft_engine::StampKind::ALL
                            .into_iter()
                            .find(|k| k.name().trim_start_matches("PC").eq_ignore_ascii_case(&stamp[6..]))
                            .ok_or_else(|| format!("unknown stamp {stamp}"))?,
                    ),
                    field if field.starts_with("field-") => QuickTool::Field(
                        prepare::FieldTool::from_command(&format!("form.add.{}", &field[6..])).ok_or_else(|| format!("unknown tool {field}"))?,
                    ),
                    fill if fill.starts_with("fill-") => QuickTool::Fill(
                        fill_sign::FillTool::from_command(&format!("sign.fill.{}", &fill[5..])).ok_or_else(|| format!("unknown tool {fill}"))?,
                    ),
                    other => {
                        let t = comments::CommentTool::from_command(&format!("comment.{other}")).ok_or_else(|| format!("unknown tool {other}"))?;
                        self.comment_prefs.group_tool[t.group()] = t;
                        QuickTool::Comment(t)
                    }
                };
            }
            ("author", _) => self.comment_prefs.author = value.to_string(),
            ("hide-empty-highlight-hover", _) => {
                self.comment_prefs.hide_empty_highlight_hover = match value {
                    "true" => true,
                    "false" => false,
                    _ => return Err("hide-empty-highlight-hover must be true or false".into()),
                };
            }
            ("ui-scale", _) => self.ui_scale = ui_scale::parse(value)?,
            ("system-title-bar", _) => {
                self.system_title_bar = match value {
                    "true" => true,
                    "false" => false,
                    _ => return Err("system-title-bar must be true or false".into()),
                };
            }
            // `--time-server none|<http(s) address>`: Preferences ▸ Signatures ▸ Time server.
            ("time-server", _) => {
                self.time_server = if value.eq_ignore_ascii_case("none") {
                    sign_ui::TimeServer::default()
                } else {
                    sign_ui::TimeServer::chosen(value).map_err(|e| e.to_string())?
                };
            }
            ("flatten-fill-sign", _) => {
                self.flatten_fill_sign_on_save = match value {
                    "true" => true,
                    "false" => false,
                    _ => return Err("flatten-fill-sign must be true or false".into()),
                };
            }
            ("comment", Some(v)) => {
                // `--comment 2:4` selects the 4th annotation of page 2 (1-based, as comment_list reports).
                let (p, i) = value.split_once(':').ok_or("comment: PAGE:INDEX")?;
                let (p, i): (usize, usize) =
                    (p.trim().parse().map_err(|_| "comment: PAGE:INDEX")?, i.trim().parse().map_err(|_| "comment: PAGE:INDEX")?);
                v.comments.selected = Some((p.saturating_sub(1), i.saturating_sub(1)));
                v.comments.reveal = true;
            }
            // `--combine-view list`: Combine files as thumbnails (grid) or as the table (list).
            ("combine-view", _) => {
                self.combine_view = combine_ui::CombineView::parse(value).ok_or("combine-view must be grid or list")?;
            }
            // `--combine-zoom 150`: the size of the cards in the Combine grid, in percent.
            ("combine-zoom", _) => {
                let percent = value.trim_end_matches('%').parse::<f32>().map_err(|e| e.to_string())?;
                if !percent.is_finite() || !combine_grid::ZOOM_RANGE.contains(&(percent / 100.0)) {
                    return Err("combine-zoom must be between 60 and 200".into());
                }
                self.set_combine_zoom(percent / 100.0);
            }
            (k, None)
                if [
                    "page",
                    "zoom",
                    "layout",
                    "continuous",
                    "cover",
                    "organize",
                    "grid-zoom",
                    "fields",
                    "find",
                    "rotate",
                    "select",
                    "notice",
                    "comment",
                ]
                .contains(&k) =>
            {
                return Err(format!("`{k}` needs an open document"));
            }
            (other, _) => return Err(format!("unknown option {other}")),
        }
        if key == "panel" && self.right.is_some() {
            self.pending_panel_reveals.push(docking::Panel::Inspector);
        }
        if key == "left" && self.left_open {
            self.pending_panel_reveals.push(docking::Panel::Tools);
        }
        Ok(())
    }

    fn shortcuts(&mut self, ctx: &egui::Context) {
        use egui::Key;
        // "Save changes?" is modal: its keys are its own (⌘D is Don't save there, not Document
        // properties; Escape cancels it rather than clearing a selection), and nothing may run
        // underneath it. They are read here, before the canvas can consume them.
        if self.close_request.is_some() {
            if let Some(choice) = dialogs::save_prompt_key(ctx) {
                self.resolve_close(ctx, choice);
            }
            return;
        }
        // The other dialogs and prompts are modal too (⌘W closed the file behind Preferences,
        // #870).
        if self.modal_open() {
            return;
        }
        if let Some(view) = self.active.and_then(|i| self.views.get_mut(i))
            && view.auto_scroll.escape(ctx)
        {
            return;
        }
        self.registry_shortcuts(ctx);
        // (Esc closes a Combine file shown large first.)
        if self.full_screen && self.combine_tab.preview.is_none() && ctx.input(|i| i.key_pressed(Key::Escape)) {
            self.set_full_screen(ctx, false);
        }
        if let Some(i) = self.active {
            // Select all belongs to the document or page grid, unless a text field or
            // overlay owns the keyboard. Other canvas shortcuts keep their own handling.
            if self.dialog.is_none()
                && !self.palette_open
                && !ctx.egui_wants_keyboard_input()
                && ctx.input_mut(|input| input.consume_shortcut(&egui::KeyboardShortcut::new(egui::Modifiers::COMMAND, Key::A)))
            {
                if self.is_preparing() {
                    if let Some(doc) = self.session.get(self.views[i].id) {
                        prepare::select_all(&mut self.views[i], &doc.form);
                    }
                } else {
                    self.views[i].select_all();
                }
            }
            self.tool_keys(i, ctx);
            if self.quick_tool == QuickTool::EditText
                && let Some(doc) = self.session.get(self.views[i].id).filter(|d| d.allows_modification())
            {
                object_ui::nudge_keys(ctx, &mut self.views[i], doc);
            }
            canvas::shortcuts(&mut self.views[i], ctx, self.scroll_speed);
            self.copy_key(i, ctx);
            self.image_pick_keys(i, ctx);
        }
    }
}

/// Why nothing can be copied out of the document: its security doesn't allow it (Table 22,
/// bit 5; #568).
pub(crate) fn copy_refused() -> &'static str {
    tl!("The document's security settings don't allow copying text and images")
}

impl PdfCraftApp {
    /// Whether text and images may be copied out of document `id`; says why not when they may
    /// not. Reading aloud and accessibility tools don't ask (assistive technology may read
    /// anything, §7.6.4.2).
    pub(crate) fn may_copy(&mut self, id: DocId) -> bool {
        if self.session.get(id).is_none_or(pdfcraft_engine::Document::allows_copying) {
            return true;
        }
        self.notify(copy_refused().to_string());
        false
    }

    /// ⌘C copies the selected text, unless a text field has the keyboard (it copies its own).
    fn copy_key(&mut self, i: usize, ctx: &egui::Context) {
        // ⌘C arrives as a Copy event on most platforms.
        let copy = ctx.input(|input| input.events.iter().any(|e| matches!(e, egui::Event::Copy)));
        if !copy || ctx.egui_wants_keyboard_input() {
            return;
        }
        self.copy_selected_text(i, ctx);
    }

    /// Shared permission gate for copying selected page text from menus and keyboard events.
    pub(crate) fn copy_selected_text(&mut self, i: usize, ctx: &egui::Context) -> bool {
        let Some((id, text)) = self.views.get(i).and_then(|v| Some((v.id, v.selected_text()?))).filter(|(_, text)| !text.is_empty()) else {
            return false;
        };
        if !self.may_copy(id) {
            return false;
        }
        ctx.copy_text(text);
        true
    }

    /// The quick tools' keys, as in Acrobat: V selects, H pans, and holding Space pans until it
    /// is released. Plain letters, so not while a text field, a form field, a dialog or the
    /// palette has the keyboard.
    fn tool_keys(&mut self, i: usize, ctx: &egui::Context) {
        use egui::Key;
        let free = self.dialog.is_none() && !self.palette_open && !ctx.egui_wants_keyboard_input() && self.views[i].forms.focus.is_none();
        let (plain, space) = ctx.input(|input| (input.modifiers.is_none(), input.key_down(Key::Space)));
        if let Some(previous) = self.space_hand
            && !space
        {
            self.space_hand = None;
            if self.quick_tool == QuickTool::Hand {
                self.quick_tool = previous;
            }
        }
        if !free || !plain {
            return;
        }
        if ctx.input(|input| input.key_pressed(Key::V)) {
            self.quick_tool = QuickTool::Select;
            self.space_hand = None;
        }
        if ctx.input(|input| input.key_pressed(Key::H)) {
            self.quick_tool = QuickTool::Hand;
            self.space_hand = None;
        }
        if space && self.space_hand.is_none() && self.quick_tool != QuickTool::Hand {
            self.space_hand = Some(self.quick_tool);
            self.quick_tool = QuickTool::Hand;
        }
    }
}

impl eframe::App for PdfCraftApp {
    /// While files are dragged over the window the system sends no pointer moves, so egui
    /// would keep the place the pointer entered at: tell it where the pointer really is, so the
    /// page grid can show (and use) the gap under it.
    fn raw_input_hook(&mut self, ctx: &egui::Context, raw: &mut egui::RawInput) {
        // Before egui reads this frame's wheel input.
        self.scroll_speed.apply(ctx, raw);
        if raw.hovered_files.is_empty() && raw.dropped_files.is_empty() {
            return;
        }
        if let Some(pos) = drag_pointer::in_window(ctx) {
            raw.events.push(egui::Event::PointerMoved(pos));
        }
        ctx.request_repaint();
    }

    fn save(&mut self, storage: &mut dyn eframe::Storage) {
        storage.set_string("pdfcraft", self.persist());
    }

    fn logic(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.ctx = Some(ctx.clone());
        self.dialog_epoch();
        // Notices raised outside `ui` (opened files, OS events, the control channel) translate too.
        let lang = i18n::Lang::from_pref(&self.language);
        i18n::set_current(lang);
        #[cfg(not(target_arch = "wasm32"))]
        {
            if let Some(worker) = &self.open_worker {
                worker.set_context(ctx);
            }
            self.poll_pending_opens();
        }
        // Chinese wants its own faces before the Japanese ones (one baseline per line).
        let cjk = theme::CjkPreference::for_language(lang.code());
        if !self.styled {
            egui_extras::install_image_loaders(ctx);
            theme::install_fonts_for(ctx, cjk);
            self.fonts_cjk = cjk;
            theme::apply(ctx, self.theme);
            self.styled = true;
        } else {
            self.fonts_ready = true;
            if cjk != self.fonts_cjk {
                // Same family names as before, so named fonts stay valid while the new set loads.
                theme::install_fonts_for(ctx, cjk);
                self.fonts_cjk = cjk;
            }
        }
        self.sync_theme(ctx);
        let native = ctx.native_pixels_per_point().unwrap_or(1.0);
        ui_scale::sync(ctx, ui_scale::zoom(self.ui_scale, self.system_text_scale, native), &mut self.ui_scale_applied);
        // Remember user work even if its tab is subsequently closed or Home is selected.
        self.startup_superseded |= !self.views.is_empty() || self.combine_showing() || self.dialog.is_some() || self.password_prompt.is_some();
        // Also done in `raw_input_hook`, before input; here for hosts that don't call the hook.
        self.scroll_speed.set_line_speed(ctx);
        // Before anything reads this frame's wheel input or draws, and with the line speed just set.
        self.motion.begin_pass(ctx, self.animation_effects);
        // Before taking this frame's drop: the grid must be drawn once with the pointer where
        // the files were let go before the gap is read.
        self.finish_grid_drop(ctx);
        // Showing a document hides the Combine files tab.
        if self.active.is_some() {
            self.combine_tab.focused = false;
        }
        #[cfg(target_arch = "wasm32")]
        self.process_signature_images();
        let dropped = ctx.input(|i| i.raw.dropped_files.clone());
        #[cfg(not(target_arch = "wasm32"))]
        let dropped = self.drop_on_signature_pad(dropped);
        #[cfg(not(target_arch = "wasm32"))]
        let dropped = self.drop_on_grid(dropped, ctx);
        for f in dropped {
            // Files dropped on the Combine files tab join its list instead of opening.
            if self.combine_showing() {
                self.drop_into_combine(f, ctx);
            } else {
                self.open_dropped(f, ctx);
            }
        }
        let arrived: Vec<(String, Vec<u8>)> = self.inbox.lock().map(|mut q| std::mem::take(&mut *q)).unwrap_or_default();
        for (name, bytes) in arrived {
            if let Err(e) = self.open_bytes(&name, None, bytes) {
                self.notify_fmt("Couldn't open {name}: {e}", &[("name", &name), ("e", &e.to_string())]);
            }
        }
        // Don't replace a pending prompt or import. A startup file may need its own password
        // prompt, so its bytes stay queued until the current interaction finishes.
        #[cfg(not(target_arch = "wasm32"))]
        let opening = !self.pending_opens.is_empty();
        #[cfg(target_arch = "wasm32")]
        let opening = false;
        if !opening
            && self.password_prompt.is_none()
            && self.image_import.is_none()
            && self.dialog.is_none()
            && self.close_request.is_none()
            && self.pending_link.is_none()
            && self.pending_remote_link.is_none()
        {
            let startup = self.startup_inbox.lock().map(|mut q| std::mem::take(&mut *q)).unwrap_or_default();
            for (name, bytes) in startup {
                let activate = !self.startup_superseded;
                if let Err(e) = self.open_bytes_with_focus(&name, None, bytes, activate) {
                    self.notify_fmt("Couldn't open {name}: {e}", &[("name", &name), ("e", &e)]);
                }
            }
        }
        let failed: Vec<(String, String)> = self.failed_inbox.lock().map(|mut q| std::mem::take(&mut *q)).unwrap_or_default();
        for (name, e) in failed {
            self.notify_fmt("Couldn't open {name}: {e}", &[("name", &name), ("e", &e)]);
        }
        let os_events = self.os_events.as_mut().map(|poll| poll()).unwrap_or_default();
        for e in os_events {
            match e {
                #[cfg(not(target_arch = "wasm32"))]
                OsEvent::Open(paths) => paths.iter().for_each(|p| self.open_path(p)),
                #[cfg(target_arch = "wasm32")]
                OsEvent::Open(_) => {}
                // Like closing the window: `guard_quit` asks about unsaved changes.
                OsEvent::Quit => ctx.send_viewport_cmd(egui::ViewportCommand::Close),
            }
        }
        if let Some((_, id)) = self.active_ids() {
            let out = self.session.take_js_output(id);
            self.handle_js(id, out);
        }
        if let Some(mut control) = self.control.take() {
            control.tick(ctx, self);
            self.control = Some(control);
        }
        self.guard_quit(ctx);
        // macOS: quit through `terminate:` instead of dropping the window (Touch Bar crash, #1136).
        craft_quit::on_close_request(ctx);
        let now = ctx.input(|i| i.time);
        self.autosave_tick(now);
        // Reload documents whose files a build rewrote (#431).
        #[cfg(not(target_arch = "wasm32"))]
        self.watch_files();
        if self.host_dirty.is_some() {
            let dirty = self.first_dirty().is_some();
            if let Some(report) = self.host_dirty.as_mut() {
                report(dirty);
            }
        }
        self.poll_updates();
        self.poll_print();
        // Shortcuts deferred last frame: the text field has taken that frame's typing since.
        let deferred = std::mem::take(&mut self.deferred_commands);
        self.shortcuts(ctx);
        // Scrolling and group-drag previews are transient: never resume after changing tabs,
        // opening a modal/palette, or returning to a window that lost focus.
        let blocked = self.modal_open() || self.palette_open || !ctx.input(|i| i.focused);
        for (index, view) in self.views.iter_mut().enumerate() {
            view.objects.set_input_blocked(blocked || self.active != Some(index));
            if blocked || self.active != Some(index) {
                view.auto_scroll.cancel();
            }
        }
        // A field that refused its value this frame keeps the shortcut from saving or printing
        // behind the user's back; so does a save prompt opened since.
        if self.process_pending_edits() && self.close_request.is_none() {
            for (id, doc) in deferred {
                // Only in the document the shortcut was pressed in.
                if self.active_ids().map(|(_, active)| active) == doc {
                    self.execute(id);
                }
            }
        }
        self.poll_export();
        self.poll_ocr();
        self.poll_optimize();
        self.poll_action();
        self.process_file_requests();
        #[cfg(not(target_arch = "wasm32"))]
        self.process_picked();
    }

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        i18n::set_current(i18n::Lang::from_pref(&self.language));
        // Fonts registered via set_fonts only take effect next frame; named families would panic now.
        if !self.fonts_ready {
            ctx.request_repaint();
            return;
        }
        for view in &mut self.views {
            view.begin_render_frame();
        }
        // The window shows the active document's name (or title, if it asks for that).
        let title = self
            .active
            .and_then(|i| crate::canvas::tab_label(self, i))
            .or_else(|| self.combine_showing().then(|| tl!("Combine files").to_owned()))
            .map_or_else(|| "PdfCraft".to_owned(), |name| format!("{name} — PdfCraft"));
        if title != self.window_title {
            ctx.send_viewport_cmd(egui::ViewportCommand::Title(title.clone()));
            self.window_title = title;
        }
        if self.full_screen && self.active.is_some() {
            // Full screen: the page, nothing else (Esc or ⌘L to leave).
            let t = theme::Tokens::get(&ctx);
            egui::CentralPanel::default().frame(egui::Frame::NONE.fill(if t.dark() { t.pasteboard } else { egui::Color32::from_gray(32) })).show(
                ui,
                |ui| {
                    if let Some(i) = self.active {
                        match compare_ui::side_by_side(self, i) {
                            Some((old, new)) => compare_ui::compare_area(self, old, new, ui),
                            None => canvas::document_area(self, i, ui),
                        }
                    }
                },
            );
            dialogs::show(self, &ctx);
            // Notices too: a refused field value or a failed save must be seen in full screen.
            self.show_progress(&ctx);
            widgets::toast(self, &ctx);
            self.finish_render_frame(&ctx);
            return;
        }
        self.sync_window_frame(&ctx);
        if self.draws_window_frame() {
            window_controls::resize_edges(&ctx);
        }
        chrome::tab_strip(self, ui);
        chrome::mode_bar(self, ui);
        if self.active.is_some() {
            chrome::right_rail(self, ui);
            // Read mode hides the tools, not the navigation panes: the rail stays visible
            // there, so its buttons (Bookmarks, Page thumbnails, ...) must open their panels.
            // Outside Read mode the docking workspace draws the inspector.
            if self.mode == Mode::Read && self.right.is_some() {
                egui::Panel::right("read_right_panel")
                    .resizable(true)
                    .default_size(330.0)
                    .size_range(260.0..=520.0)
                    .frame(egui::Frame::NONE)
                    .show(ui, |ui| panels::right_panel(self, ui));
            }
        }
        let t = theme::Tokens::get(&ctx);
        egui::CentralPanel::default().frame(egui::Frame::NONE.fill(t.pasteboard)).show(ui, |ui| {
            if self.mode == Mode::Read {
                match self.active {
                    None if self.combine_showing() => combine_ui::page(self, ui),
                    None => home::show(self, ui),
                    Some(index) => match compare_ui::side_by_side(self, index) {
                        Some((old, new)) => compare_ui::compare_area(self, old, new, ui),
                        None => canvas::document_area(self, index, ui),
                    },
                }
            } else {
                docking::show(self, ui);
            }
        });
        self.process_pending_edits();
        palette::show(self, &ctx);
        dialogs::show(self, &ctx);
        self.show_progress(&ctx);
        widgets::toast(self, &ctx);
        self.finish_render_frame(&ctx);
    }
}

impl PdfCraftApp {
    fn finish_render_frame(&mut self, ctx: &egui::Context) {
        // The Combine grid's thumbnails, whichever tab shows (renders under way still finish).
        self.combine_thumbs_frame(ctx);
        // Retire hidden documents before admitting the visible document's textures, so the
        // three cache limits apply to the application, regardless of how many tabs are open.
        // Side by side: the comparison's old file renders alongside the active one.
        let partner = self.active.and_then(|i| compare_ui::side_by_side(self, i)).map(|(old, _)| old);
        for (i, view) in self.views.iter_mut().enumerate() {
            if Some(i) != self.active
                && Some(i) != partner
                && let Some(doc) = self.session.get(view.id)
            {
                view.suspend_rendering(&doc.renderer);
            }
        }
        for i in self.active.into_iter().chain(partner) {
            if let Some(view) = self.views.get_mut(i)
                && let Some(doc) = self.session.get(view.id)
            {
                view.finish_render_frame(ctx, &doc.info, &doc.renderer);
            }
        }
    }
}
