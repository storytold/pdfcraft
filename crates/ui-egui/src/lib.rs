//! pdfcraft-ui-egui — the first PdfCraft shell (L7).
//!
//! Layout grammar follows plan/acrobat/02-ui-ux.md §1: tab strip, mode bar, left tool panel,
//! floating quick-action bar, document area, right panel + right rail with page navigation.
//! Everything here is presentation: documents, rendering and the tool catalogue live in
//! `pdfcraft-engine`.

#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

mod a11y_ui;
mod actions_ui;
pub mod canvas;
mod chrome;
mod combine_ui;
mod commands;
mod comment_props;
pub mod comments;
mod comments_panel;
mod compare_ui;
pub mod control;
mod create_ui;
mod crop;
mod export_ui;
mod js_ui;
mod marks_ui;
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
pub use create_ui::Clip;
pub use link_ui::LinkDraft;
pub use optimize_ui::{OptimizeDraft, OptimizeTab};
pub use sign_ui::{DigitalIdEntry, SignDraft, SignStep};
mod dialogs;
mod edit_text_ui;
mod editing;
mod files;
pub mod fill_sign;
pub mod forms_ui;
mod home;
mod icon_data;
pub mod icons;
mod pageboxes;
mod palette;
mod panels;
pub mod prepare;
mod print_ui;
pub use print_ui::{Handling as PrintHandling, PrintDraft, Which as PrintWhich};
mod redact_ui;
pub use redact_ui::{HiddenDraft, PagesDraft as RedactPagesDraft, RedactPrefs, SearchDraft as RedactSearchDraft};
pub mod i18n;

/// The longest author name kept (Preferences ▸ Identity, restored settings).
pub(crate) const MAX_AUTHOR_CHARS: usize = 200;
mod protect;
mod recovery;
pub mod theme;
pub mod updates;
mod widgets;

use pdfcraft_engine::{DocId, Session};

pub use canvas::DocView;
pub use editing::{CloseRequest, SaveTarget};
pub use files::{ExtractDraft, FilePurpose, RotateDraft, SplitDraft, SplitMode, SplitPlan};
pub use recovery::{AUTOSAVE_SECS, RecoveryMeta, RecoveryStore};
use theme::ThemeKind;

/// Top-level workspace modes (Acrobat's mode bar).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    AllTools,
    Read,
    Edit,
    Convert,
    Sign,
}

/// What the left panel shows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LeftPanel {
    AllTools,
    Tool(&'static str),
}

/// Right-hand panels, opened from the rail.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
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
    /// Crop pages by dragging a rectangle.
    Crop,
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
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Dialog {
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
    /// Redact a PDF ▸ Redact pages, Find text and redact, Set properties, apply confirmation.
    RedactPages,
    RedactSearch,
    RedactProps,
    RedactApply,
    /// File ▸ Print.
    Print,
    /// File ▸ Revert confirmation.
    Revert,
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
    /// Combine files: the files, their order and pages.
    Combine,
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
    pub name: String,
    pub bytes: std::sync::Arc<Vec<u8>>,
    pub pages: Vec<usize>,
}

/// Files delivered asynchronously: (name, bytes).
pub type Inbox = std::sync::Arc<std::sync::Mutex<Vec<(String, Vec<u8>)>>>;

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

pub struct PasswordPrompt {
    pub name: String,
    pub path: Option<String>,
    pub bytes: std::sync::Arc<Vec<u8>>,
    pub input: String,
    pub error: Option<String>,
}

#[derive(serde::Serialize, serde::Deserialize)]
pub struct RecentFile {
    pub name: String,
    pub path: String,
    pub pages: usize,
    pub size: usize,
}

pub struct PdfCraftApp {
    pub session: Session,
    pub views: Vec<DocView>,
    /// `None` shows the Home tab.
    pub active: Option<usize>,
    pub mode: Mode,
    pub left: LeftPanel,
    pub left_open: bool,
    pub right: Option<RightPanel>,
    pub quick_tool: QuickTool,
    /// Comment author, per-tool colours and widths, pin.
    pub comment_prefs: comments::CommentPrefs,
    pub theme: ThemeKind,
    pub language: i18n::Language,
    /// Follow the operating system's light/dark setting.
    pub follow_system_theme: bool,
    pub dialog: Option<Dialog>,
    /// How to ask for the latest release (the desktop app sets it; see `updates`).
    pub update_source: Option<updates::UpdateSource>,
    pub(crate) updates: updates::Updates,
    pub palette_open: bool,
    pub palette_query: String,
    pub all_tools_expanded: bool,
    pub recent: Vec<RecentFile>,
    pub toast: Option<(String, f64)>,
    /// Whether the macOS title bar is drawn by us (traffic lights over our tab strip).
    pub integrated_titlebar: bool,
    pub password_prompt: Option<PasswordPrompt>,
    pub full_screen: bool,
    /// Files delivered asynchronously (web drag-and-drop, web file picker).
    pub inbox: Inbox,
    /// Requests from the operating system, polled every frame (macOS Apple events).
    pub os_events: Option<OsEventsFn>,
    /// A pending "save changes?" question (closing a dirty tab or quitting).
    pub close_request: Option<CloseRequest>,
    /// Save to this path instead of asking (tests and automation).
    pub save_override: Option<String>,
    /// Document Properties ▸ Description fields being edited: (document, Title/Author/Subject/Keywords).
    pub props_draft: Option<(DocId, [String; 4])>,
    /// Document Properties ▸ Initial View (and reading options) being edited.
    pub view_draft: Option<(DocId, pdfcraft_engine::InitialView)>,
    /// Files picked asynchronously for combine / insert (web).
    pub requests: files::Requests,
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
    /// Background jobs (OCR, actions) run inline instead (tests).
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
    pub a11y_skipped: std::collections::BTreeSet<pdfcraft_engine::a11y::Rule>,
    pub alt_draft: a11y_ui::AltDraft,
    /// List the macOS Keychain's signing identities among the digital IDs (the desktop app).
    pub keychain_ids: bool,
    pub cert_viewer: Option<sign_ui::CertViewer>,
    /// The last space audit.
    pub space_audit: Vec<pdfcraft_engine::optimize::SpaceUse>,
    /// Combine files: the files staged so far.
    pub combine_draft: Vec<combine_ui::CombineFile>,
    /// The custom stamp library, and the stamp being created.
    pub custom_stamps: Vec<stamps_ui::CustomStamp>,
    pub stamp_draft: stamps_ui::StampDraft,
    /// PDF Optimizer choices.
    pub optimize_draft: OptimizeDraft,
    /// Pages copied or cut in Organize Pages, ready to paste (into any document).
    pub page_clipboard: Option<PageClip>,
    /// The last snapshot (width, height, RGBA); `system_clipboard` also puts it on the
    /// system clipboard (tests turn that off).
    pub last_snapshot: Option<(u32, u32, Vec<u8>)>,
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
    pending_recovered: Option<RecoveryMeta>,
    allow_quit: bool,
    /// The egui context, for commands that change window or theme state.
    ctx: Option<egui::Context>,
    pending_theme: Option<ThemeKind>,
    styled: bool,
    fonts_ready: bool,
    /// The window title last sent to the platform.
    pub window_title: String,
    /// The UI control channel, when enabled (`--control`; off by default).
    control: Option<control::Control>,
    /// A bookmark being renamed in the Bookmarks panel: (path, text so far).
    pub bookmark_rename: Option<(Vec<usize>, String)>,
    /// Number pages dialog settings (1-based pages).
    pub number_draft: NumberDraft,
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
    pub(crate) signature_preview: Option<(String, egui::TextureHandle)>,
    /// The Comment Properties dialog's state.
    pub comment_props: Option<comment_props::PropsDraft>,
    pub field_props: Option<prepare::FieldDraft>,
    pub redact_prefs: RedactPrefs,
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

impl PdfCraftApp {
    pub fn new() -> Self {
        Self {
            session: Session::new(),
            views: Vec::new(),
            active: None,
            mode: Mode::AllTools,
            left: LeftPanel::AllTools,
            left_open: true,
            right: None,
            quick_tool: QuickTool::Select,
            comment_prefs: Default::default(),
            theme: ThemeKind::Light,
            language: i18n::Language::default(),
            follow_system_theme: false,
            dialog: None,
            update_source: None,
            updates: updates::Updates::default(),
            palette_open: false,
            palette_query: String::new(),
            all_tools_expanded: false,
            recent: Vec::new(),
            toast: None,
            integrated_titlebar: false,
            password_prompt: None,
            full_screen: false,
            inbox: Default::default(),
            os_events: None,
            close_request: None,
            save_override: None,
            props_draft: None,
            view_draft: None,
            requests: Default::default(),
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
            a11y_skipped: Default::default(),
            alt_draft: Default::default(),
            keychain_ids: false,
            cert_viewer: None,
            space_audit: Vec::new(),
            combine_draft: Vec::new(),
            custom_stamps: Vec::new(),
            stamp_draft: Default::default(),
            optimize_draft: OptimizeDraft::default(),
            page_clipboard: None,
            last_snapshot: None,
            system_clipboard: true,
            attach_override: None,
            duplicate_draft: None,
            form_preview: false,
            window_title: String::new(),
            recovery: None,
            recoverable: Vec::new(),
            recovery_keys: Default::default(),
            last_autosave: 0.0,
            pending_recovered: None,
            allow_quit: false,
            ctx: None,
            pending_theme: None,
            styled: false,
            fonts_ready: false,
            control: None,
            bookmark_rename: None,
            last_opened_url: None,
            pending_link: None,
            protect_draft: Default::default(),
            boxes_draft: Default::default(),
            marks_draft: Default::default(),
            export_draft: Default::default(),
            export_status: None,
            signature: None,
            initials: None,
            signature_draft: Default::default(),
            signature_preview: None,
            comment_props: None,
            field_props: None,
            redact_prefs: RedactPrefs::default(),
            redact_pages_draft: RedactPagesDraft::default(),
            redact_search: RedactSearchDraft::default(),
            hidden_draft: HiddenDraft::default(),
            print_draft: PrintDraft::default(),
            link_draft: None,
            text_style: content_ui::default_style(),
            replace_draft: None,
            number_draft: NumberDraft { from: 1, to: 1, style: pdfcraft_engine::LabelStyle::Decimal, prefix: String::new(), start: 1 },
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
        match v.navigation {
            N::PageOnly => {}
            N::Bookmarks => self.right = Some(RightPanel::Bookmarks),
            N::Pages => self.right = Some(RightPanel::Pages),
            N::Attachments => self.right = Some(RightPanel::Attachments),
            N::Layers => self.right = Some(RightPanel::Layers),
        }
        let view = &mut self.views[index];
        match v.layout {
            L::Default => {}
            L::SinglePage => view.layout = canvas::PageLayout::Single,
            L::SinglePageContinuous => view.layout = canvas::PageLayout::Continuous,
            L::TwoUp | L::TwoUpContinuous => view.layout = canvas::PageLayout::TwoUp,
            L::TwoUpCoverPage | L::TwoUpContinuousCoverPage => {
                view.layout = canvas::PageLayout::TwoUp;
                view.cover = true;
            }
        }
        match v.magnification {
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
        // Images and text files become new, unsaved PDFs (Create a PDF).
        if let Some(r) = self.open_converted(name, &bytes) {
            return r;
        }
        self.try_open(name, path, std::sync::Arc::new(bytes), None)
    }

    fn try_open(&mut self, name: &str, path: Option<String>, bytes: std::sync::Arc<Vec<u8>>, password: Option<&str>) -> Result<(), String> {
        use pdfcraft_render::OpenError;
        let size = bytes.len();
        let id = match self.session.open(name, path.clone(), bytes.clone(), password) {
            Ok(id) => id,
            Err(e @ (OpenError::NeedsPassword | OpenError::WrongPassword)) => {
                let error = matches!(e, OpenError::WrongPassword).then(|| "Incorrect password. Try again.".to_string());
                self.password_prompt = Some(PasswordPrompt { name: name.to_string(), path, bytes, input: String::new(), error });
                return Ok(());
            }
            Err(e) => return Err(e.to_string()),
        };
        self.password_prompt = None;
        let doc = self.session.get(id).ok_or("the document could not be opened")?;
        let pages = doc.info.pages.len();
        // Acrobat opens straight to the Comments panel when a document has comments.
        if self.right.is_none() {
            self.right = if !doc.info.annotations.is_empty() {
                Some(RightPanel::Comments)
            } else if !doc.info.outline.is_empty() {
                Some(RightPanel::Bookmarks)
            } else {
                None
            };
        }
        let initial = doc.initial_view();
        self.views.push(DocView::new(id, &doc.info));
        self.active = Some(self.views.len() - 1);
        self.apply_initial_view(self.views.len() - 1, &initial);
        if let Some(p) = path {
            self.recent.retain(|r| r.path != p);
            self.recent.insert(0, RecentFile { name: name.to_string(), path: p, pages, size });
            self.recent.truncate(12);
        }
        Ok(())
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
                if let Err(e) = self.open_bytes(&name, None, bytes) {
                    self.notify(format!("Couldn't open {name}: {e}"));
                }
            }
            Err(e) => self.notify(format!("Couldn't read {name}: {e}")),
        }
    }

    /// Browsers read dropped files asynchronously; the bytes land in `inbox` and open next frame.
    #[cfg(target_arch = "wasm32")]
    fn open_dropped(&mut self, f: egui::DroppedFileHandle, ctx: &egui::Context) {
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

    /// Save an attachment to disk, or open a PDF attachment in a new tab.
    pub fn attachment_action(&mut self, doc: DocId, index: usize, open: bool) {
        let Some(d) = self.session.get(doc) else { return };
        let Some(att) = d.info.attachments.get(index).cloned() else { return };
        let data = pdfcraft_render::attachment_data(&d.bytes, d.password.as_deref(), &att);
        match (data, open) {
            (Err(e), _) => self.notify(format!("Couldn't read {}: {e}", att.name)),
            (Ok(bytes), true) => {
                if let Err(e) = self.open_bytes(&att.name, None, bytes) {
                    self.notify(format!("Couldn't open {}: {e}", att.name));
                }
            }
            (Ok(bytes), false) => {
                #[cfg(not(target_arch = "wasm32"))]
                if let Some(path) = rfd::FileDialog::new().set_file_name(&att.name).save_file() {
                    match std::fs::write(&path, &bytes) {
                        Ok(()) => self.notify(format!("Saved {}", path.display())),
                        Err(e) => self.notify(format!("Couldn't save: {e}")),
                    }
                }
                #[cfg(target_arch = "wasm32")]
                self.notify(format!("Downloading attachments on the web arrives with M3.10 ({} bytes ready)", bytes.len()));
            }
        }
    }

    /// Enter or leave full-screen reading (Acrobat: View ▸ Full Screen Mode, ⌘L).
    pub fn set_full_screen(&mut self, ctx: &egui::Context, on: bool) {
        self.full_screen = on;
        ctx.send_viewport_cmd(egui::ViewportCommand::Fullscreen(on));
    }

    /// Answer the password prompt (`None` cancels).
    pub fn submit_password(&mut self, password: Option<String>) {
        let Some(p) = self.password_prompt.take() else { return };
        let Some(pw) = password else { return };
        match self.try_open(&p.name, p.path, p.bytes, Some(&pw)) {
            Err(e) => self.notify(format!("Couldn't open {}: {e}", p.name)),
            // A recovered encrypted document is open once the prompt is gone.
            Ok(()) if self.password_prompt.is_none() => {
                if let Some(meta) = self.pending_recovered.clone() {
                    self.finish_recovery(&meta);
                }
            }
            Ok(()) => {}
        }
    }

    #[cfg(not(target_arch = "wasm32"))]
    pub fn open_path(&mut self, path: &str) {
        let name = std::path::Path::new(path).file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| path.to_string());
        match std::fs::read(path) {
            Ok(bytes) => {
                if let Err(e) = self.open_bytes(&name, Some(path.to_string()), bytes) {
                    self.notify(format!("Couldn't open {name}: {e}"));
                }
            }
            Err(e) => self.notify(format!("Couldn't read {name}: {e}")),
        }
    }

    pub fn open_dialog(&mut self) {
        #[cfg(not(target_arch = "wasm32"))]
        if let Some(p) =
            rfd::FileDialog::new().add_filter("PDF", &["pdf"]).add_filter("Images and text (converted to PDF)", &create_ui::CONVERTIBLE).pick_file()
        {
            self.open_path(&p.to_string_lossy());
        }
        // Browsers pick files asynchronously; the bytes arrive through `inbox`.
        #[cfg(target_arch = "wasm32")]
        {
            let inbox = self.inbox.clone();
            wasm_bindgen_futures::spawn_local(async move {
                if let Some(h) =
                    rfd::AsyncFileDialog::new().add_filter("PDF", &["pdf"]).add_filter("Images and text", &create_ui::CONVERTIBLE).pick_file().await
                {
                    let bytes = h.read().await;
                    if let Ok(mut q) = inbox.lock() {
                        q.push((h.file_name(), bytes));
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
        self.forget_recovery(id);
        self.session.close(id);
        self.active = match self.active {
            _ if self.views.is_empty() => None,
            Some(a) if a >= self.views.len() => Some(self.views.len() - 1),
            other => other,
        };
    }

    pub fn active_ids(&self) -> Option<(usize, DocId)> {
        self.active.and_then(|i| self.views.get(i).map(|v| (i, v.id)))
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
                if self.pending_link.is_none() {
                    self.pending_link = Some(PendingLink { url, origin });
                }
            }
            Err(e) => self.notify(format!(
                "{} in this document tried to open an address PdfCraft won't open: {e}. Only web (http, https) and email (mailto) links open from documents.",
                origin.noun()
            )),
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

    pub fn set_theme(&mut self, ctx: &egui::Context, kind: ThemeKind) {
        self.theme = kind;
        theme::apply(ctx, kind);
    }

    /// Run a catalogue command. Commands that aren't implemented yet say which milestone ships them.
    pub fn run_command(&mut self, command: &str) {
        if pdfcraft_engine::commands::command(command).is_some() {
            self.execute(command);
            return;
        }
        // Not implemented yet: say which milestone ships it.
        let when = pdfcraft_engine::catalog::TOOL_GROUPS
            .iter()
            .flat_map(|g| g.sections.iter().flat_map(|s| s.items.iter()))
            .find(|i| i.command == command)
            .map(|i| match i.availability {
                pdfcraft_engine::catalog::Availability::Planned(m) => format!("ships in milestone {m}"),
                pdfcraft_engine::catalog::Availability::Provider => "needs an AI provider (off by default)".to_string(),
                pdfcraft_engine::catalog::Availability::Ready => "is available".to_string(),
            })
            .unwrap_or_else(|| "is not available yet".into());
        self.notify(format!("`{command}` {when}"));
    }

    /// Serialize the user's persistent state (recent files, theme). Local only.
    pub fn persist(&self) -> String {
        let trusted: Vec<String> = self.session.trusted_certificates().iter().map(pdfcraft_engine::sign::x509::to_pem).collect();
        serde_json::json!({
            "recent": self.recent,
            "theme": self.theme,
            "language": self.language,
            "author": self.comment_prefs.author,
            // Drawn signatures keep their original form (older settings read the same).
            "signature": match &self.signature { Some(fill_sign::SavedSig::Drawn(s)) => Some(s), _ => None },
            "signature_text": match &self.signature { Some(fill_sign::SavedSig::Typed(t)) => Some(t), _ => None },
            "initials": self.initials,
            // Keychain identities are read from macOS each time.
            "digital_ids": self.digital_ids.iter().filter(|e| !e.path.starts_with("keychain:")).collect::<Vec<_>>(),
            "trusted": trusted,
            "custom_stamps": stamps_ui::encode(&self.custom_stamps),
            "javascript": self.session.javascript(),
            "actions": actions_ui::encode(&self.custom_actions),
        })
        .to_string()
    }

    /// Restore state written by `persist`. Unknown or malformed data is ignored.
    pub fn restore(&mut self, json: &str) {
        let Ok(v) = serde_json::from_str::<serde_json::Value>(json) else { return };
        if let Ok(r) = serde_json::from_value::<Vec<RecentFile>>(v["recent"].clone()) {
            // Only keep entries whose files still exist.
            #[cfg(not(target_arch = "wasm32"))]
            let r: Vec<RecentFile> = r.into_iter().filter(|f| std::path::Path::new(&f.path).exists()).collect();
            self.recent = r;
        }
        if let Ok(t) = serde_json::from_value::<ThemeKind>(v["theme"].clone()) {
            self.theme = t;
        }
        if let Ok(language) = serde_json::from_value::<i18n::Language>(v["language"].clone()) {
            self.language = language;
        }
        // An empty or missing name keeps the login-name default; settings are untrusted, so the
        // name is cut to a sane length.
        if let Some(author) = v["author"].as_str().map(str::trim).filter(|a| !a.is_empty()) {
            self.comment_prefs.author = author.chars().take(MAX_AUTHOR_CHARS).collect();
        }
        if let Ok(s) = serde_json::from_value::<Vec<Vec<[f32; 2]>>>(v["signature"].clone())
            && s.iter().all(|st| st.iter().all(|p| p.iter().all(|x| x.is_finite())))
        {
            self.signature = Some(fill_sign::SavedSig::Drawn(s));
        }
        if let Some(t) = v["signature_text"].as_str().filter(|t| !t.trim().is_empty()) {
            self.signature = Some(fill_sign::SavedSig::Typed(t.to_string()));
        }
        if let Ok(i) = serde_json::from_value::<fill_sign::SavedSig>(v["initials"].clone()) {
            self.initials = Some(i);
        }
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
            self.session.set_trusted_certificates(certs);
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
                self.language = i18n::Language::parse(value).ok_or("language must be en or ja")?;
            }
            ("theme", _) => {
                self.follow_system_theme = value == "system";
                self.pending_theme = Some(if value == "dark" { ThemeKind::Dark } else { ThemeKind::Light });
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
                    "search" => Some(RightPanel::Search),
                    "compare" => Some(RightPanel::Compare),
                    "none" => None,
                    other => return Err(format!("unknown panel {other}")),
                }
            }
            ("mode", _) => {
                self.mode = match value {
                    "read" => Mode::Read,
                    "edit" => Mode::Edit,
                    "convert" => Mode::Convert,
                    "sign" => Mode::Sign,
                    _ => Mode::AllTools,
                }
            }
            ("tool", _) => {
                let g = pdfcraft_engine::catalog::group(value).ok_or_else(|| format!("unknown tool {value}"))?;
                self.left = LeftPanel::Tool(g.id);
                self.left_open = true;
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
                v.layout = match value {
                    "two-up" => canvas::PageLayout::TwoUp,
                    "single" => canvas::PageLayout::Single,
                    _ => canvas::PageLayout::Continuous,
                }
            }
            ("organize", Some(v)) => v.organize = value != "off",
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
                // `--select 2,3,5` (1-based) selects pages in the organize grid.
                let pages: Result<Vec<usize>, _> = value.split(',').map(|p| p.trim().parse::<usize>().map(|n| n.saturating_sub(1))).collect();
                v.select_pages(&pages.map_err(|_| "select: comma-separated page numbers")?);
            }
            ("notice", Some(v)) => v.notice_dismissed = value == "off",
            ("quick", _) => {
                // `--quick select|hand|note|freetext|highlight|underline|strikeout|ink|line|arrow|square|circle`
                self.quick_tool = match value {
                    "select" => QuickTool::Select,
                    "hand" => QuickTool::Hand,
                    "crop" => QuickTool::Crop,
                    "redact" => QuickTool::Redact,
                    "add-text" => QuickTool::AddText,
                    "edit-text" => QuickTool::EditText,
                    "link" => QuickTool::Link,
                    "sign" => QuickTool::SignArea { certify: false },
                    "marquee-zoom" => QuickTool::MarqueeZoom,
                    "snapshot" => QuickTool::Snapshot,
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
            ("comment", Some(v)) => {
                // `--comment 2:4` selects the 4th annotation of page 2 (1-based, as comment_list reports).
                let (p, i) = value.split_once(':').ok_or("comment: PAGE:INDEX")?;
                let (p, i): (usize, usize) =
                    (p.trim().parse().map_err(|_| "comment: PAGE:INDEX")?, i.trim().parse().map_err(|_| "comment: PAGE:INDEX")?);
                v.comments.selected = Some((p.saturating_sub(1), i.saturating_sub(1)));
                v.comments.reveal = true;
            }
            (k, None) if ["page", "zoom", "layout", "organize", "fields", "find", "rotate", "select", "notice", "comment"].contains(&k) => {
                return Err(format!("`{k}` needs an open document"));
            }
            (other, _) => return Err(format!("unknown option {other}")),
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
        self.registry_shortcuts(ctx);
        if self.full_screen && ctx.input(|i| i.key_pressed(Key::Escape)) {
            self.set_full_screen(ctx, false);
        }
        if let Some(i) = self.active {
            canvas::shortcuts(&mut self.views[i], ctx);
        }
    }
}

impl eframe::App for PdfCraftApp {
    fn save(&mut self, storage: &mut dyn eframe::Storage) {
        storage.set_string("pdfcraft", self.persist());
    }

    fn logic(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.ctx = Some(ctx.clone());
        if !self.styled {
            egui_extras::install_image_loaders(ctx);
            theme::install_fonts(ctx);
            theme::apply(ctx, self.theme);
            self.styled = true;
        } else {
            self.fonts_ready = true;
        }
        if let Some(k) = self.pending_theme.take() {
            self.set_theme(ctx, k);
        }
        if self.follow_system_theme
            && let Some(sys) = ctx.system_theme()
        {
            let want = if sys == egui::Theme::Dark { ThemeKind::Dark } else { ThemeKind::Light };
            if want != self.theme {
                self.set_theme(ctx, want);
            }
        }
        let dropped = ctx.input(|i| i.raw.dropped_files.clone());
        for f in dropped {
            self.open_dropped(f, ctx);
        }
        let arrived: Vec<(String, Vec<u8>)> = self.inbox.lock().map(|mut q| std::mem::take(&mut *q)).unwrap_or_default();
        for (name, bytes) in arrived {
            if let Err(e) = self.open_bytes(&name, None, bytes) {
                self.notify(format!("Couldn't open {name}: {e}"));
            }
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
        if let Some(mut control) = self.control.take() {
            control.tick(ctx, self);
            self.control = Some(control);
        }
        self.guard_quit(ctx);
        let now = ctx.input(|i| i.time);
        self.autosave_tick(now);
        self.poll_updates();
        self.shortcuts(ctx);
        self.process_pending_edits();
        self.poll_export();
        self.poll_ocr();
        self.poll_action();
        self.process_file_requests();
        // Pull finished renders into textures for every open document.
        for view in &mut self.views {
            if let Some(doc) = self.session.get(view.id) {
                view.receive(ctx, &doc.renderer);
            }
        }
    }

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        // Fonts registered via set_fonts only take effect next frame; named families would panic now.
        if !self.fonts_ready {
            ctx.request_repaint();
            return;
        }
        // The window shows the active document's name (or title, if it asks for that).
        let title = self
            .active
            .and_then(|i| self.session.get(self.views[i].id))
            .map_or_else(|| "PdfCraft".to_owned(), |d| format!("{} — PdfCraft", d.display_name()));
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
                        canvas::document_area(self, i, ui);
                    }
                },
            );
            dialogs::show(self, &ctx);
            return;
        }
        chrome::tab_strip(self, ui);
        chrome::mode_bar(self, ui);
        if self.active.is_some() {
            chrome::right_rail(self, ui);
            if self.right.is_some() && self.mode != Mode::Read {
                panels::right_panel(self, ui);
            }
        }
        if self.left_open && self.mode != Mode::Read {
            panels::left_panel(self, ui);
        }
        let t = theme::Tokens::get(&ctx);
        egui::CentralPanel::default().frame(egui::Frame::NONE.fill(t.pasteboard)).show(ui, |ui| match self.active {
            None => home::show(self, ui),
            Some(i) => canvas::document_area(self, i, ui),
        });
        self.process_pending_edits();
        palette::show(self, &ctx);
        dialogs::show(self, &ctx);
        widgets::toast(self, &ctx);
    }
}
