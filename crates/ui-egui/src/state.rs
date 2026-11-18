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

/// Page Properties: the document and pages it applies to (0-based), the document's edit
/// generation when the dialog opened, and the pages' tab order as they have it (`None` when they
/// differ). The order chosen is the hosted dialog's value.
#[derive(Clone, Debug, PartialEq)]
pub struct PagePropsDraft {
    pub doc: DocId,
    pub generation: u64,
    pub pages: Vec<usize>,
    pub before: Option<pdfcraft_engine::TabOrder>,
}

/// Copied comments: their XFDF (each with its replies), for `Edit::PasteComments`.
#[derive(Clone, Debug, PartialEq)]
pub struct CommentClip {
    pub xfdf: String,
    /// How many comments (replies not counted) were copied.
    pub count: usize,
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
/// layer as far as it was read, and the document to read the others from. A copy of a text
/// selection across pages whose pages are not all read yet is one too, limited to its `span`.
pub(crate) struct DocumentCopyJob {
    pub(crate) pages: Vec<CopyPage>,
    span: Option<crate::canvas::TextSpan>,
    bytes: std::sync::Arc<Vec<u8>>,
    password: Option<std::sync::Arc<str>>,
}

impl DocumentCopyJob {
    pub(crate) fn of(v: &DocView, doc: &pdfcraft_engine::Document) -> DocumentCopyJob {
        let pages = (0..v.page_count())
            .map(|p| match v.session.texts.get(&p) {
                Some(t) => CopyPage::Read(t.clone()),
                None if v.session.text_failed.contains(&p) => CopyPage::Skip,
                None => CopyPage::ToRead,
            })
            .collect();
        DocumentCopyJob { pages, span: None, bytes: doc.bytes.clone(), password: doc.password.as_deref().map(std::sync::Arc::from) }
    }

    /// A copy of the reading-order text `span` covers: its pages as [`Self::of`] takes them, every
    /// other page skipped.
    pub(crate) fn of_selection(v: &DocView, doc: &pdfcraft_engine::Document, span: crate::canvas::TextSpan) -> DocumentCopyJob {
        let mut job = DocumentCopyJob::of(v, doc);
        for (page, p) in job.pages.iter_mut().enumerate() {
            if page < span.start.0 || page > span.end.0 {
                *p = CopyPage::Skip;
            }
        }
        job.span = Some(span);
        job
    }

    /// The document's text in page order, a page per line run. Each page read here is let go
    /// once its text is taken, so a large document never holds every page's text layer at once.
    /// `None` when `cancel` was set.
    pub(crate) fn text(self, cancel: &craft::host::queue::Cancel) -> Option<String> {
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
        // The glyphs taken from `page`: all of them, or the part a selection covers.
        let span = self.span;
        let part = |page: usize, t: &pdfcraft_render::PageText| match span {
            Some(span) => span.on(page, t.glyphs.len()).map(|r| t.text_of(r)).unwrap_or_default(),
            None => t.text_of(0..t.glyphs.len()),
        };
        for (page, p) in self.pages.into_iter().enumerate() {
            if cancel.is_cancelled() {
                return None;
            }
            match p {
                CopyPage::Read(t) => push(part(page, &t)),
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
                        push(part(page, &t));
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
    pub(crate) stamp: Option<craft::platform::watch::FileStamp>,
    pub error: Option<String>,
    /// A late startup file must stay in the background even after it is unlocked.
    pub(crate) activate: bool,
    /// The bytes are a recovery snapshot: once the document is open it takes the snapshot's
    /// path and recovery entry. Cancelling the prompt drops the association with it (#813).
    pub(crate) recovered: Option<craft::platform::autosave::RecoveryEntry>,
    /// The tool that asked for this file (a tool chosen with no document open): it runs once
    /// the document is unlocked.
    pub(crate) then: Option<&'static str>,
    /// A link destination to apply after the linked PDF is unlocked.
    pub(crate) after_open: Option<pdfcraft_render::RemoteDestination>,
    /// A launch-time display override, applied only after this PDF is unlocked.
    pub(crate) display_override: Option<(craft::pages::PageDisplay, bool)>,
}

#[cfg(not(target_arch = "wasm32"))]
pub(crate) struct OpenRequest {
    pub(crate) name: String,
    pub(crate) path: Option<String>,
    pub(crate) stamp: Option<craft::platform::watch::FileStamp>,
    pub(crate) input: open_worker::Input,
    pub(crate) password: Option<zeroize::Zeroizing<String>>,
    pub(crate) activate: bool,
    pub(crate) after_open: Option<pdfcraft_render::RemoteDestination>,
    pub(crate) recovered: Option<craft::platform::autosave::RecoveryEntry>,
    pub(crate) then: Option<&'static str>,
    /// Where the open waits in the worker's queue: a file the user named goes before files
    /// reopened from the last session.
    pub(crate) priority: craft::host::queue::Priority,
}

/// Asks for the permissions password of an open document whose security restricts it, which
/// lifts the restrictions (#995). Acrobat asks for it when such a document's security is changed.
/// The typed password stays in the prompt's own field, never here.
pub struct UnlockPrompt {
    pub doc: DocId,
    pub name: String,
    pub error: Option<String>,
    /// The command to run once unlocked (Protect using password, Remove security), if any.
    pub then: Option<&'static str>,
}

impl UnlockPrompt {
    /// Ask for document `doc`'s permissions password, then run command `then`, if any.
    pub(crate) fn new(doc: &pdfcraft_engine::Document, then: Option<&'static str>) -> Self {
        Self { doc: doc.id, name: doc.name.clone(), error: None, then }
    }
}

/// How many recent files are kept until Preferences says otherwise.
pub const RECENT_FILES_DEFAULT: usize = 12;
/// The most Preferences can ask to keep.
pub const RECENT_FILES_MAX: usize = 50;

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

/// What [`PdfCraftApp::open_path`] did: opened the file in a new tab, focused the tab already
/// showing it, handed it to the background open worker, or failed. Failures tell the user
/// through a notice; this is the programmatic contract for callers that must know (the control
/// channel, tools that wait for a file) (#1620).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum OpenOutcome {
    /// The file opened in a new tab (or raised its password prompt).
    Opened,
    /// A tab already showed the file; it was focused and no tab was added.
    Focused,
    /// The file was accepted for opening in the background; its tab appears when the open
    /// completes (`ui.state.pending_opens` lists the files on the way).
    Pending,
    /// The file could not be read or opened; the user was told this text, which the control
    /// reply carries too.
    Failed(String),
}
