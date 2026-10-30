use super::*;

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
    pub(crate) fn parse(value: &str) -> Option<Self> {
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
    /// Prepare a form ▸ Calculation order, a page of its own; Back returns to Prepare a form.
    CalculationOrder,
}

/// Right-hand panels, opened from the rail.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
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
pub(crate) type GridFile = (String, Result<Vec<u8>, std::path::PathBuf>);

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
    /// Organize pages ▸ Page properties (the tab order of the selected pages).
    PageProps,
    /// Protect Using Password.
    Protect,
    /// Bookmarks ▸ New bookmarks from text styles (#1693).
    BookmarksFromTextStyles,
    /// Set Page Boxes (crop, trim, bleed, art, media).
    PageBoxes,
    /// Add / Update Header and Footer, Watermark, Background.
    Marks(pdfcraft_engine::MarkKind),
    /// Export a PDF ▸ Image / Text.
    Export(export_ui::ExportKind),
    /// Export form data / Export comments: the format, before the save panel (#5215).
    ExportData(data_export_ui::DataKind),
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
    /// Prepare a form ▸ Text field table: a movable window rather than a modal (`field_table_ui`).
    FieldTable,
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
    Custom,
    Advanced,
    SpaceAudit,
}

/// Document Properties ▸ Custom being edited: the properties as they will be after OK, in order,
/// and the Name and Value fields.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct CustomDraft {
    pub props: Vec<(String, String)>,
    pub name: String,
    pub value: String,
}

/// Duplicate Field: the field and the pages (all, or a range, 1-based).
#[derive(Clone, Debug, PartialEq)]
pub struct DuplicateDraft {
    pub name: String,
    pub all: bool,
    pub from: usize,
    pub to: usize,
}

/// Page Properties: the document and pages it applies to (0-based), the document's edit
/// generation when the dialog opened, and the pages' tab order as they have it (`None` when they
/// differ) and as chosen (`None` until one is chosen).
#[derive(Clone, Debug, PartialEq)]
pub struct PagePropsDraft {
    pub doc: DocId,
    pub generation: u64,
    pub pages: Vec<usize>,
    pub before: Option<pdfcraft_engine::TabOrder>,
    pub tab_order: Option<pdfcraft_engine::TabOrder>,
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

/// One page of a whole-document copy: its text as read for display, to read, or not to try
/// (reading it failed before).
pub(crate) enum CopyPage {
    Read(std::sync::Arc<pdfcraft_render::PageText>),
    ToRead,
    Skip,
}

/// What a whole-document copy needs, taken from the view on the UI thread: each page's text
/// layer as far as it was read, and the document to read the others from.
pub(crate) struct DocumentCopyJob {
    pub(crate) pages: Vec<CopyPage>,
    bytes: std::sync::Arc<Vec<u8>>,
    password: Option<std::sync::Arc<str>>,
}

impl DocumentCopyJob {
    pub(crate) fn of(v: &DocView, doc: &pdfcraft_engine::Document) -> DocumentCopyJob {
        let pages = (0..v.page_count())
            .map(|p| match v.texts.get(&p) {
                Some(t) => CopyPage::Read(t.clone()),
                None if v.text_failed.contains(&p) => CopyPage::Skip,
                None => CopyPage::ToRead,
            })
            .collect();
        DocumentCopyJob { pages, bytes: doc.bytes.clone(), password: doc.password.as_deref().map(std::sync::Arc::from) }
    }

    /// The document's text in page order, a page per line run. Each page read here is let go
    /// once its text is taken, so a large document never holds every page's text layer at once.
    /// `None` when `cancel` was set.
    pub(crate) fn text(self, cancel: &std::sync::atomic::AtomicBool) -> Option<String> {
        let mut renderer: Option<pdfcraft_render::PageRenderer> = None;
        let mut out = String::new();
        let mut push = |text: String| {
            if !text.is_empty() {
                if !out.is_empty() {
                    out.push('\n');
                }
                out.push_str(&text);
            }
        };
        for (page, p) in self.pages.into_iter().enumerate() {
            if cancel.load(std::sync::atomic::Ordering::Relaxed) {
                return None;
            }
            match p {
                CopyPage::Read(t) => push(t.text_of(0..t.glyphs.len())),
                CopyPage::ToRead => {
                    let r = renderer.get_or_insert_with(|| {
                        let config = pdfcraft_render::RenderConfig { password: self.password.clone(), ..Default::default() };
                        pdfcraft_render::PageRenderer::new(self.bytes.clone(), config)
                    });
                    let req = pdfcraft_render::RenderRequest {
                        page,
                        kind: pdfcraft_render::RequestKind::Text,
                        tile: None,
                        scale: 1.0,
                        tag: 0,
                        preview: false,
                    };
                    if let Some(t) = r.render(req).text {
                        push(t.text_of(0..t.glyphs.len()));
                    }
                }
                CopyPage::Skip => {}
            }
        }
        Some(out)
    }
}

pub struct PasswordPrompt {
    pub name: String,
    pub path: Option<String>,
    pub bytes: std::sync::Arc<Vec<u8>>,
    #[cfg(not(target_arch = "wasm32"))]
    pub(crate) stamp: Option<file_watch::Stamp>,
    pub input: String,
    pub error: Option<String>,
    /// A late startup file must stay in the background even after it is unlocked.
    pub(crate) activate: bool,
    /// The bytes are a recovery snapshot: once the document is open it takes the snapshot's
    /// path and recovery entry. Cancelling the prompt drops the association with it (#813).
    pub(crate) recovered: Option<RecoveryMeta>,
    /// The tool that asked for this file (a tool chosen with no document open): it runs once
    /// the document is unlocked.
    pub(crate) then: Option<&'static str>,
    /// A link destination to apply after the linked PDF is unlocked.
    pub(crate) after_open: Option<pdfcraft_render::RemoteDestination>,
}

#[cfg(not(target_arch = "wasm32"))]
pub(crate) struct OpenRequest {
    pub(crate) name: String,
    pub(crate) path: Option<String>,
    pub(crate) stamp: Option<file_watch::Stamp>,
    pub(crate) input: open_worker::Input,
    pub(crate) password: Option<String>,
    pub(crate) activate: bool,
    pub(crate) after_open: Option<pdfcraft_render::RemoteDestination>,
    pub(crate) recovered: Option<RecoveryMeta>,
    pub(crate) then: Option<&'static str>,
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
    pub(crate) fn noun(self) -> &'static str {
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

/// What [`PdfCraftApp::open_path`] did: opened the file in a new tab, focused the tab already
/// showing it, handed it to the background open worker, or failed. Failures tell the user
/// through a toast; this is the programmatic contract for callers that must know (the control
/// channel, tools that wait for a file) (#1620).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OpenOutcome {
    /// The file opened in a new tab (or raised its password prompt).
    Opened,
    /// A tab already showed the file; it was focused and no tab was added.
    Focused,
    /// The file was accepted for opening in the background; its tab appears when the open
    /// completes (`ui.state.pending_opens` lists the files on the way).
    Pending,
    /// The file could not be read or opened; the user was told.
    Failed,
}
