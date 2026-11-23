//! The document area: a document tab's notice bars, then its pages in the framework's page view (or the Organize
//! grid), with the page body that registers what is on the pages, runs the page tools and paints the overlays, and
//! what the pass asked the app to do.
//!
//! Page pictures come from the page view's texture stores over [`pictures::DocPages`]; the app holds no raster.

use std::collections::{HashMap, HashSet};
use std::ops::Range;
use std::sync::Arc;

use craft::behavior::{CanvasView, PointerButton, PressPhase, Selection};
use craft::host::queue::{JobQueue, QueueOptions};
use craft::pages::ui::{PageBody, PageView, PageViewOutput, PageViewState, PlacedPage, TextMark, TextMarks, TextSelectState, TextSelection};
use craft::review::ui::annotate::AnnotateWords;
use craft::ui::canvas::{ContextRequest, Momentary, NavigationOptions, PageBorder, PasteboardColor, ToolHost};
use craft::ui::contract::{Props as _, Show as _};
use craft::ui::display::Text;
use craft::ui::list::OpenSet;
use craft::ui::primitives::TextureStatistics;
use craft::ui::primitives::textures::TextureState;
use craft::ui::runtime::env::Env;
use craft::ui::runtime::focus::{self, FocusTarget};
use craft::ui::style::DataColor;
use egui::{Color32, Pos2, Rect, Vec2, pos2, vec2};
use pdfcraft_engine::text_links::TextLink;
use pdfcraft_engine::{DocId, Edit};
use pdfcraft_render::{DestView, DocInfo, LayerOp, LinkTarget, PageText, RenderRequest, RequestKind};

use crate::{PdfCraftApp, QuickTool, RightPanel, comments};

mod area;
pub(crate) mod bars;
mod cache;
pub(super) mod crop;
mod find;
pub(super) mod image_pick;
mod navigation;
pub(crate) mod organize;
// PLAN: no sheet assigns the `mod` lines of the four new page-body files (pdf-canvas-w9 `page_parts.rs`,
// `page_tools.rs`; pdf-canvas-w10 `pictures.rs`; pdf-canvas-w15 `page_overlays.rs`); the host writes them here.
pub(crate) mod page_overlays;
pub(crate) mod page_parts;
pub(crate) mod page_tools;
pub(crate) mod pictures;
mod presenter;
pub mod scroll_speed;
mod selection;
pub(super) mod signature_drag;
mod xform;
mod zoom;
pub(super) mod zoom_snap;

pub(crate) use area::signature_badges;
pub(crate) use area::signature_states_changed;
use area::{run_button, set_layers};
use bars::{Notice, notices, page_size_hint};
pub(crate) use cache::{BASE_SIDE, TILE_THRESHOLD, texture_image, thumbnail_rows};
pub use craft::pages::{Fit, PageDisplay, PageSpacing, ViewDefaults, display_token, parse_display_token};
pub use find::Find;
use find::find_bar;
use organize::organize_grid;
use page_menu::{FieldMenu, PageMenuAction};
use page_overlays::{LinkEvent, LinkMode, PageLinks, PageOverlays, overlay_parts};
use page_parts::{Owner, PagePart, PageParts, PagePress};
use page_tools::{PageToolCx, page_tool};
use pictures::{DocPages, PageRevisions, RenderReports};
pub(crate) use selection::{TextSpan, ToolMarks};
pub(crate) use xform::PageGeometry;
pub use xform::PageXform;

/// Logical pixels per PDF point at 100% (96 dpi, like browsers).
pub const PT: f32 = 96.0 / 72.0;

/// User-space editing frame: explicit PDF/editor crops apply, but the page edge does not
/// truncate a selected image's handles. Body hit testing still respects page visibility.
pub fn image_selectable_bounds(im: &pdfcraft_engine::PageImage) -> [f64; 4] {
    im.frame_rect().unwrap_or(im.rect)
}

/// Screen-space editing frame, including selected handles outside the page.
pub fn image_screen_rect(im: &pdfcraft_engine::PageImage, xf: &PageXform, info: &DocInfo, page: usize) -> Rect {
    xf.user_rect_f64(info, page, image_selectable_bounds(im))
}

/// The page display as it was before full screen, put back on leaving it
/// ([`DocView::enter_presentation`]).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SavedDisplay {
    /// Single pages, two-up spreads or columns.
    pub display: PageDisplay,
    /// Whether the pages scrolled continuously.
    pub continuous: bool,
    /// Whether two-page spreads started with the first page alone.
    pub cover: bool,
    /// The fit the view had.
    pub fit: Fit,
    /// The zoom the view had (1.0 = 100%).
    pub zoom: f64,
}

<<<<<<< HEAD
=======
/// The base page display: one page across, or a two-page spread. Whether it scrolls continuously
/// is a separate flag (`continuous`), as in Acrobat, so every combination is reachable (#752).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PageLayout {
    Single,
    TwoUp,
}

impl PageLayout {
    /// The base page displays, in the order the View menu, the rail's Page display button and
    /// Preferences list them.
    pub const ORDER: [Self; 2] = [Self::Single, Self::TwoUp];

    /// The name settings and view options use for the base layout: `single` or `two-up`.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Single => "single",
            Self::TwoUp => "two-up",
        }
    }

    /// The base layout an [`as_str`](Self::as_str) name stands for, in any case. `None` for
    /// anything else, so callers can reject a typo rather than switch layouts silently.
    pub fn try_parse(value: &str) -> Option<Self> {
        Self::ORDER.into_iter().find(|l| l.as_str().eq_ignore_ascii_case(value.trim()))
    }

    /// Rail icon for this layout.
    pub fn icon(self) -> &'static str {
        match self {
            Self::Single => "file-text",
            Self::TwoUp => "columns-2",
        }
    }

    /// Registry command that switches to this base layout (`PdfCraftApp::execute`).
    pub fn command(self) -> &'static str {
        match self {
            Self::Single => "view.layout.single",
            Self::TwoUp => "view.layout.two_up",
        }
    }

    /// The base layout a [`command`](Self::command) id switches to.
    pub fn from_command(id: &str) -> Option<Self> {
        Self::ORDER.into_iter().find(|l| l.command() == id)
    }

    /// Menu label: the label of [`command`](Self::command), which the catalogs translate.
    pub fn label(self) -> &'static str {
        match self {
            Self::Single => "Single page",
            Self::TwoUp => "Two-page view",
        }
    }
}

/// The single page-display token settings and the control channel use, combining the base layout
/// with continuous scrolling. `single` and `two-page` show one page or one spread at a time;
/// `continuous` (Single, scrolling) and `two-up` (Two-page, scrolling) keep their earlier names
/// for back-compat.
pub fn display_token(layout: PageLayout, continuous: bool) -> &'static str {
    match (layout, continuous) {
        (PageLayout::Single, false) => "single",
        (PageLayout::Single, true) => "continuous",
        (PageLayout::TwoUp, true) => "two-up",
        (PageLayout::TwoUp, false) => "two-page",
    }
}

/// The base layout and continuous flag a [`display_token`] names, in any case. `None` for a typo.
pub fn parse_display_token(value: &str) -> Option<(PageLayout, bool)> {
    match value.trim().to_ascii_lowercase().as_str() {
        "single" => Some((PageLayout::Single, false)),
        "continuous" => Some((PageLayout::Single, true)),
        "two-up" => Some((PageLayout::TwoUp, true)),
        "two-page" | "two-up-single" => Some((PageLayout::TwoUp, false)),
        _ => None,
    }
}

/// How a newly opened document is shown (Preferences ▸ Documents and view). A PDF that asks
/// for its own layout or zoom gets that instead.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ViewDefaults {
    pub layout: PageLayout,
    /// Whether the base layout scrolls continuously.
    pub continuous: bool,
    /// Separate pages with a gutter and shadow; off joins adjacent page edges.
    pub show_page_gaps: bool,
    /// [`Fit::Width`] or [`Fit::Page`], or [`Fit::None`] for a fixed `zoom`.
    pub fit: Fit,
    /// The zoom with [`Fit::None`] (1.0 = 100%).
    pub zoom: f32,
    /// Highlight existing fields: a preference in Acrobat, so it carries over to the next
    /// document and the next session. On for new settings; a stored choice wins.
    pub highlight_fields: bool,
    /// Full screen fills the screen with one page (or spread) at a time, edge to edge; off, it
    /// keeps the page display it is entered from (continuous scrolling, say: #4273).
    pub full_screen_fill: bool,
}

impl Default for ViewDefaults {
    fn default() -> Self {
        // Single page, scrolling continuously at fit width: the earlier "continuous" default.
        Self {
            layout: PageLayout::Single,
            continuous: true,
            show_page_gaps: true,
            fit: Fit::Width,
            zoom: 1.0,
            highlight_fields: true,
            full_screen_fill: true,
        }
    }
}

impl ViewDefaults {
    /// The zoom as settings and view options name it: `fit-width`, `fit-page` or a percentage.
    pub fn zoom_name(&self) -> String {
        match self.fit {
            Fit::Page => "fit-page".into(),
            Fit::None => format!("{}%", (self.zoom * 100.0).round()),
            Fit::Width | Fit::Height => "fit-width".into(),
        }
    }

    /// These defaults with the zoom a [`zoom_name`](Self::zoom_name) names, in any case. `None`
    /// for anything else, including a percentage outside the 8–6400% the view can show.
    pub fn with_zoom(self, name: &str) -> Option<Self> {
        let name = name.trim();
        let (fit, zoom) = if name.eq_ignore_ascii_case("fit-width") {
            (Fit::Width, self.zoom)
        } else if name.eq_ignore_ascii_case("fit-page") {
            (Fit::Page, self.zoom)
        } else {
            let percent = name.strip_suffix('%').unwrap_or(name).trim().parse::<f32>().ok().filter(|p| (8.0..=6400.0).contains(p))?;
            (Fit::None, percent / 100.0)
        };
        Some(Self { fit, zoom, ..self })
    }
}

/// A historical snapshot opened from another document: View signed version opens the bytes
/// a signature covers, View revision an earlier incremental revision. Repeats reuse the tab,
/// and the tab and notice bar name what it is, instead of piling up identical tabs.
>>>>>>> upstream/main
/// What a historical snapshot shows: the bytes a signature covers, or an earlier
/// incremental revision.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SnapshotKind {
    SignedVersion,
    Revision,
}

/// A historical snapshot opened from another document: View signed version opens the bytes
/// a signature covers, View revision an earlier incremental revision. Repeats reuse the tab,
/// and the tab and notice bar name what it is, instead of piling up identical tabs.
#[derive(Clone, Debug)]
pub struct Snapshot {
    pub kind: SnapshotKind,
    /// The 1-based revision shown.
    pub revision: usize,
    /// The name of the document it was opened from.
    pub from: String,
}

/// The name shown for a document tab, the window title and the Combine picker.
/// A snapshot reads like its source document, plus the snapshot suffix that tells
/// the tabs apart. Without a title the file name (which already carries the suffix)
/// is enough.
pub(crate) fn tab_label(app: &PdfCraftApp, index: usize) -> Option<String> {
    let view = app.views.get(index)?;
    let doc = app.session.get(view.id)?;
    let Some(snap) = view.snapshot.as_ref() else { return Some(doc.display_name()) };
    Some(snapshot_tab_name(&doc.display_name(), &doc.name, snap))
}

/// A snapshot's tab name from its source title, file name and marker. Pure so the
/// tab strip, window title and picker cannot disagree with each other.
pub(crate) fn snapshot_tab_name(title: &str, file_name: &str, snapshot: &Snapshot) -> String {
    if title == file_name {
        return file_name.into();
    }
    if snapshot.kind == SnapshotKind::SignedVersion {
        format!("{title} (signed version)")
    } else {
        format!("{title} (revision {})", snapshot.revision)
    }
}

/// Markup quadrilaterals in user space, one entry per page: (page, quads).
pub type PageMarks = Vec<(usize, Vec<[f64; 8]>)>;

/// One document tab: what the page view shows ([`PageViewState`]), the data its page source reads, the records the
/// last pass left for readers between frames, and the [`PageSession`] the page body works on.
///
/// The three are separate fields so the page view can hold `&mut view` while the page body holds `&mut session`.
pub struct DocView {
    /// Where the tab shows (split view's panes) and the key that tells two
    /// views of one document apart (`crate::panes`).
    pub(crate) pane: crate::panes::ViewPane,
    pub id: DocId,
    /// The document and attached file this tab was opened from, when a form or the Attachments
    /// panel opened it: asking for that attachment again comes back to this tab.
    pub from_attachment: Option<(DocId, String)>,
    /// What the page view shows: the page, display, zoom, fit, rotation and scroll offset.
    pub view: PageViewState,
    /// Everything the page body touches: the tools' and editors' state, the text selection, the pages placed.
    pub session: PageSession,
    /// The momentary tools (Space for the Hand, with Cmd or Alt the Zoom), held across passes by the tool host.
    pub(crate) momentary: Momentary,
    /// The navigation options `momentary` was made with: its Hand and Zoom tools keep them, so a change of the user's
    /// options makes it again.
    momentary_options: NavigationOptions,
    /// Each page's picture revision, which the page source stamps its renders with.
    pub(crate) revisions: PageRevisions,
    /// Pages the renderer could not draw or cut at its limits, written by the render jobs.
    pub(crate) reports: RenderReports,
    /// What the page view's picture stores held and did in the last pass (written by [`document_area_at`]), for
    /// automation between frames. No pass reads it to compute its result.
    pub textures: TextureStatistics,
    /// The spacing the last pass laid the pages out with (written by [`document_area_at`]); `None` before the first.
    // PLAN: `PageSpacing` has no default (it comes from the page view's tokens), so the record is an `Option`.
    pub spacing: Option<PageSpacing>,
    /// The document area on screen in the last pass (written by [`document_area_at`]).
    viewport: Rect,
    /// The canvas view of the last pass (written by [`document_area_at`]), for the zoom field between frames.
    canvas: Option<CanvasView>,
    /// A latched or held middle-button scroll ran in the last pass (written by [`document_area_at`]).
    middle_scrolling: bool,
    pub organize: bool,
    pub page_input: String,
    /// Text was typed into the page box since it last showed a page's label: while the box keeps the focus,
    /// navigation leaves that text alone.
    pub(crate) page_input_draft: bool,
    /// Select the replacement page label once if its editor remains focused.
    pub(crate) page_input_reselect: bool,
    pub notice_dismissed: bool,
    /// The user has been told that the form's scripts were turned off (it is said once).
    pub scripts_off_noted: bool,
    /// The file changed on disk while this document had unsaved changes: the notice bar asks
    /// before replacing it (#431).
    pub disk_changed: bool,
    /// The snapshot notice bar's Close was clicked: the tab closes at the top of the next
    /// frame, where no document borrow is held (the bar itself can't close it).
    pub close_requested: bool,
    /// A historical snapshot of another document (see [`Snapshot`]): `None` for real documents.
    pub snapshot: Option<Snapshot>,
    /// Previous view / Next view: pages visited before (and after, once going back).
    pub back: Vec<usize>,
    pub forward: Vec<usize>,
    /// The interface the document hid in this tab, and what is hidden now (ISO 32000-2 Table 147
    /// `HideToolbar`, `HideMenubar`, `HideWindowUI`). Never saved.
    pub interface: crate::viewer_prefs::TabInterface,
    /// Full screen (Acrobat's Full Screen Mode): the display saved on entering it, while the view
    /// shows one page or spread at a time, fitted to the screen. `None` outside full screen.
    presentation: Option<SavedDisplay>,
<<<<<<< HEAD
=======
    /// Full screen fills the screen with one page (or spread) at a time, edge to edge (the
    /// default); off, it keeps the page display it was entered from. Set from the app's
    /// preference before full screen is entered.
    pub full_screen_fill: bool,
    /// Full screen's transitions, display times and pointer (see [`presenter`]).
    pub(crate) presenter: presenter::Presenter,
    pages: HashMap<usize, PageTex>,
    /// Pages the renderer could not draw, with the reason (never re-requested).
    errors: HashMap<usize, String>,
    /// Pages whose content was cut at the renderer's limits (#623): drawn and searched in part.
    cut_pages: std::collections::BTreeSet<usize>,
>>>>>>> upstream/main
    /// Cut pages whose notice the reader dismissed: a page cut later shows it again.
    cut_notice_dismissed: std::collections::BTreeSet<usize>,
    pub find: Option<Find>,
    page_count: usize,
    /// Page sizes (width, height) in points (view space, unrotated by the view), for the page transforms.
    page_sizes: Vec<(f32, f32)>,
    /// Pages that Edit already tried to turn into editable text (once each, so Undo sticks).
    pub(crate) auto_ocr_tried: std::collections::HashSet<usize>,
    /// Pages Edit looked at and found not to be a scan, with the document's edit generation then:
    /// they are looked at again only after the document changes (an image added, say).
    pub(crate) auto_ocr_skipped: std::collections::HashMap<usize, u64>,
    /// Scans Edit found while no engine could read them, with the document's edit generation
    /// then: they wait for one (the web build downloads the models when first needed), and are
    /// read once the models are in (see `PdfCraftApp::ocr_models_loaded`) or the document changes.
    pub(crate) auto_ocr_awaiting_models: std::collections::HashMap<usize, u64>,
    /// Pages selected in the organize grid or the Pages panel (0-based). Empty means "the current page".
    pub selected: Selection<usize>,
    /// A non-edit action requested by the organize toolbar, handled by the app.
    pub pending_action: Option<ViewAction>,
    /// The grid gap the next inserted files go to (set by a "+" between pages); otherwise they
    /// go after the selection.
    pub insert_at: Option<usize>,
    /// The page-grid gap under the pointer this frame (where dropped files go), when it can
    /// take pages.
    pub grid_gap: Option<usize>,
    /// The visible part of the Pages panel as last drawn, when it can take dropped files
    /// (cleared each frame before the panels draw).
    pub pages_panel: Option<Rect>,
    /// The Pages-panel gap under the pointer (where dropped files go), when it can take pages.
    pub panel_gap: Option<usize>,
    /// Files dropped on a gap of the page grid or the Pages panel, with the gap, until the app
    /// inserts them.
    pub(crate) dropped_files: Option<(usize, Vec<craft::ui::runtime::drag::OsFile>)>,
    /// Transient keyboard owner for this document's Pages sidebar.
    pub(crate) pages_focus_id: Option<egui::Id>,
    /// Scale of the thumbnails in the Pages inspector, independent of document and grid zoom.
    thumbnail_zoom: f32,
    /// The current page the Pages panel last scrolled into view.
    pub(crate) panel_followed: Option<usize>,
    /// The thumbnail zoom the Pages panel last scrolled for.
    pub(crate) panel_zoom_followed: f32,
    /// The page the grid scrolls into view on its next pass (Organize just opened on it).
    grid_reveal: Option<usize>,
    /// The bookmarks open in the Bookmarks panel, by their path in the outline.
    pub(crate) bookmarks_open: OpenSet<Vec<usize>>,
    /// How many Apply redactions steps are in effect in this view. While it is above 0, Save
    /// proposes the name with `_Redacted` ([`pdfcraft_engine::redacted_file_name`]), and while it is
    /// above `redactions_downloaded`, Save asks where.
    /// Undo of an Apply redactions step takes one off and Redo adds it back. Reload and a desktop
    /// save set it to 0, and Revert sets it to `redactions_downloaded`; after a desktop save the document has the name it was saved
    /// under. A web download keeps it, so later downloads carry the `_Redacted` name.
    pub redactions_applied: usize,
    /// How many Apply redactions steps saved into the document's file are undone in this view.
    /// While it is above 0, Save asks where, so the restored content is not written over the
    /// redacted file. Redo takes one off. Revert, Reload and a desktop save set it to 0.
    pub redactions_undone_after_save: usize,
    /// Set when Apply redactions runs while saved Apply redactions steps are undone: Redo can no
    /// longer bring those steps back, so Save asks where until a desktop save or Revert clears it.
    pub saved_redactions_cut_off: bool,
    /// `redactions_applied` at the last web download, which Revert restores. While
    /// `redactions_applied` is below it, a delivered step is undone and Save asks where. A desktop
    /// save leaves it at 0.
    pub redactions_downloaded: usize,
    /// The "Signature added … Lock now" notice was dismissed; placing another signature shows it
    /// again (#2149).
    pub(crate) lock_offer_dismissed: bool,
}

/// What the page body works on: the pages placed this pass, the text selection, and the state of every tool,
/// editor and overlay drawn over the pages. The page body's functions take `&mut PageSession`, never the whole
/// [`DocView`], so the page view can hold the view state while they run.
pub struct PageSession {
    /// The pages placed in the last run of the page body (this pass's while the body runs), with their pictures.
    pub placed: Vec<PlacedPage>,
    /// The text selection, and the selection gesture in progress.
    pub text: TextSelectState,
    /// Whether the current Select-tool primary press has dragged past the click threshold.
    pub(crate) select_dragged: bool,
    /// Select All while scrolling continuously (#2639): the text of every page of the document,
    /// highlighted on each page as its text arrives and copied whole ([`DocView::whole_text`]).
    pub(crate) all_text: bool,
    /// The selection whose action bar was closed by its Copy button: the bar stays hidden
    /// until the selection changes, as it does once a markup button consumes the selection.
    pub(crate) actions_dismissed: Option<TextSelection>,
    /// A marking tool finished this text selection while a page it covers was still to be read
    /// (or none was drawn): it is marked once they are ([`DocView::tool_marks`]).
    pub(crate) marks_waiting: Option<(QuickTool, TextSelection)>,
    /// Text layers, extracted in the background on demand (selection, find, copy).
    pub(crate) texts: HashMap<usize, Arc<PageText>>,
    /// Addresses written as plain text on a page (#4207), with the text layer they were found in.
    pub(crate) text_links: HashMap<usize, (Arc<PageText>, Arc<[TextLink]>)>,
    pub(crate) text_failed: HashSet<usize>,
    /// The background jobs that extract text layers, made on the first pass that has the app's job host.
    // PLAN: pdf-canvas-w10's `want_text` and `cache.rs` read `text_queue` as a `JobQueue`; they take this `Option`
    // (`as_mut()`), since a `JobQueue` needs the host's jobs and `DocView::new` has none.
    pub(crate) text_queue: Option<JobQueue<usize, cache::TextOutcome>>,
    /// Highlight existing fields: the app's preference when the view was opened.
    pub highlight_fields: bool,
    /// Briefly outline an annotation after navigating to it from a panel.
    pub flash: Option<(usize, [f32; 4], f64)>,
    /// Compare files: differences shaded on this document's pages (page, user-space box, colour).
    pub compare_marks: Vec<(usize, [f32; 4], Color32)>,
    /// Full screen's transitions, display times, pointer and click turns (see [`presenter`]).
    pub(crate) presenter: presenter::Presenter,
    /// An edit requested by the view (organize toolbar, keys), applied by the app this frame.
    pub pending_edit: Option<Edit>,
    /// The pending edit is a Fill & Sign signature or initials placed on this page: once it applies, the app notes
    /// the document as signed and selects the new mark there.
    pub(crate) fill_signature: Option<usize>,
    /// The page a press or key made the current one (Edit text's objects selected on it, a radio button stepped to),
    /// with the user-space rectangle on it to scroll into view (a field Tab moved to), gone to by the app after the
    /// pass: the page view holds the view state while the body runs.
    pub(crate) pending_page: Option<(usize, Option<[f32; 4]>)>,
    /// Text-or-box marks (Redact or area Highlight) to add (page, quads) for the app to style,
    /// one per page (a text selection can cover several), added in one undo step.
    /// With the tool that made them, read when they are styled: a key handled later in the same
    /// frame (Escape back to Select) must not turn an area highlight into a redaction.
    /// The last field is the page the marks are followed up on (where a selection ended).
    pub pending_marks: Option<(QuickTool, PageMarks, usize)>,
    /// A text selection Redact did not mark: it covers a page whose text could not be read
    /// ([`ToolMarks::Unreadable`]). The app says so.
    pub(crate) redaction_refused: bool,
    /// The next edit applied is markup on a text selection across pages, one comment per page in
    /// one batch ([`DocView::selection_marks_edit`]): `apply_edit` follows it up as one markup and
    /// takes this back, whatever becomes of the edit. It holds the page the selection ends on,
    /// whose markup is followed up.
    pub(crate) marks_across_pages: Option<usize>,
    /// Edit text: the lines per page (with the document generation they were read at), and the
    /// line being edited.
    pub(crate) edit_lines: HashMap<usize, (u64, Vec<pdfcraft_engine::TextBlock>)>,
    /// Edit text: a click on grouped artwork with text queued [`Edit::PrepareText`] for its page:
    /// the page and the point clicked (user space), whose paragraph opens once the page is
    /// prepared, so one click on grouped text starts editing it.
    pub(crate) grouped_text_click: Option<(usize, [f64; 2])>,
    pub line_editor: Option<crate::edit_text_ui::LineEditor>,
    /// The paragraph editor whose edit was just queued. It opens again with its typing if the
    /// edit fails, as a refused form field does.
    pub(crate) applied_editor: Option<crate::edit_text_ui::LineEditor>,
    /// The box a click switched to while another paragraph was being edited: it opens once the
    /// queued edit has applied, so its index and geometry come from the document it now is (#2022).
    /// It is held as the paragraph's text and box, not an index: the edit can regroup the page.
    pub(crate) pending_text_open: Option<(usize, pdfcraft_engine::BlockKey)>,
    pub(crate) text_background: crate::edit_text_ui::TextBackground,
    pub(crate) move_background: crate::content_ui::MoveBackground,
    pub(crate) objects: crate::object_ui::ObjectSelection,
    pub image_selection: Option<crate::edit_text_ui::ImageSelection>,
    /// Select tool: the picture clicked, to copy in its original format.
    pub picked_image: Option<crate::image_pick::PickedImage>,
    /// A click (page, user-space point, document generation) whose picture is still being looked for.
    pub(crate) pending_pick: Option<(usize, [f64; 2], u64)>,
    /// The pictures of the page last asked about.
    pub(crate) pictures: crate::image_pick::PagePictures,
    /// Commenting state: selected comment, the comments tool, composer.
    pub comments: crate::comments::CommentView,
    pub measure: crate::measure_ui::MeasureView,
    /// Form filling state: the focused field.
    pub forms: crate::forms_ui::FormView,
    /// Prepare a form: the selected field and the gesture in progress.
    pub prepare: crate::prepare::PrepareView,
    /// Prepare a form ▸ Calculation order.
    pub(crate) calc_order: crate::calc_order_ui::CalcOrderView,
    /// Edit a PDF: the selected added item, the text being typed.
    pub content: crate::content_ui::ContentView,
    /// Edit a PDF ▸ Link tool state.
    pub links: crate::link_ui::LinkView,
    /// Enhance ▸ Camera image: the quadrilateral being placed.
    pub camera: Option<crate::camera_ui::CameraQuad>,
    /// Enhance ▸ Camera image: the page just enhanced, while the bar offers rotate, crop and
    /// organize.
    pub camera_done: Option<crate::camera_ui::CameraDone>,
    /// Use a certificate: a signature rectangle drawn, or an empty signature field clicked.
    pub sign: crate::sign_ui::SignView,
    /// Snapshot: the area just copied (page, view-space rect), kept with a small bar under it
    /// (Print area…, Save as image…).
    pub area_pick: Option<(usize, [f32; 4])>,
    /// Fill & Sign text being typed.
    pub fill_text: Option<crate::fill_sign::TypeBox>,
    /// The caret and selection in the Fill & Sign text being typed.
    pub fill_selection: craft::text::ui::editor::TextSelection,
    pub(crate) signature_drag: crate::signature_drag::SignatureDrag,
    /// The rectangle being dragged on a page by Snapshot, Crop, Redact or a signature area: the page tools are built
    /// each pass, so the drag lives here from its press to its release.
    pub(crate) box_drag: PageBoxDrag,
    /// The Hand tool and Marquee Zoom's Zoom tool: each keeps its gesture (the press, the last
    /// point, a flick's glide) from pass to pass, so the page tool lends them rather than building them.
    pub(crate) hand: craft::ui::canvas::HandTool,
    pub(crate) marquee_zoom: craft::ui::canvas::ZoomTool,
    /// What an area-mark press (Redact or Highlight off text) is doing, from the press to its release.
    pub(crate) mark_gesture: crate::tools::redact_ui::tool::AreaMarkGesture,
    /// The Stamp tool's press, from the press to its release.
    pub(crate) stamp_press: Option<crate::stamps_ui::StampPress>,
    /// What the last secondary press on a page went to, and on which page (`None` off the pages): the page's menu
    /// shows its entries when it has a menu and is the one selected, or else the page's own.
    pub(crate) menu_for: Option<(usize, Owner)>,
    /// A right-click of this pass that a page control kept for itself (a picture's crop): the page's menu stays shut.
    pub(crate) context_held: bool,
}

impl PageSession {
    /// The session of a newly opened document.
    fn new(info: &DocInfo, highlight_fields: bool) -> Self {
        Self {
            placed: Vec::new(),
            text: TextSelectState::default(),
            select_dragged: false,
            all_text: false,
            actions_dismissed: None,
            marks_waiting: None,
            texts: HashMap::new(),
            text_links: HashMap::new(),
            text_failed: HashSet::new(),
            text_queue: None,
            highlight_fields,
            flash: None,
            compare_marks: Vec::new(),
            presenter: Default::default(),
            pending_edit: None,
            fill_signature: None,
            pending_page: None,
            pending_marks: None,
            redaction_refused: false,
            marks_across_pages: None,
            edit_lines: HashMap::new(),
            grouped_text_click: None,
            line_editor: None,
            applied_editor: None,
            pending_text_open: None,
            text_background: Default::default(),
            move_background: Default::default(),
            objects: Default::default(),
            image_selection: None,
            picked_image: None,
            pending_pick: None,
            pictures: Default::default(),
            comments: crate::comments::CommentView::from_info(info),
            measure: Default::default(),
            forms: Default::default(),
            prepare: Default::default(),
            calc_order: Default::default(),
            content: Default::default(),
            links: Default::default(),
            camera: None,
            camera_done: None,
            sign: Default::default(),
            area_pick: None,
            fill_text: None,
            fill_selection: Default::default(),
            signature_drag: Default::default(),
            box_drag: PageBoxDrag::default(),
            hand: craft::ui::canvas::HandTool::new(Default::default()),
            marquee_zoom: craft::ui::canvas::ZoomTool::new(Default::default()),
            mark_gesture: Default::default(),
            stamp_press: None,
            menu_for: None,
            context_held: false,
        }
    }
}

/// Organize-toolbar actions that need the app (file pickers, new tabs, dialogs).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ViewAction {
    InsertFromFile,
    /// Insert files at a gap of the page grid (0 = before the first page).
    InsertFromFileAt(usize),
    /// Replace the selected pages with pages from another file.
    ReplacePages,
    /// Save the document as the grid shows it.
    Save,
    Extract,
    Split,
    /// Split before each selected page: opens Split on that mode.
    SplitBeforeSelected,
    /// Copy the selected pages (Cut also deletes them).
    CopyPages {
        cut: bool,
    },
    /// Paste copied pages after the selection.
    PastePages,
    /// Enhance ▸ Camera image: straighten what the corners enclose.
    EnhanceCamera,
    /// Leave Enhance ▸ Camera image (Cancel before Enhance page, Done after it).
    CancelCamera,
    /// After Enhance page: turn the enhanced page a quarter.
    CameraRotate {
        clockwise: bool,
    },
    /// After Enhance page: enhance again at the slider's clean-up level.
    CameraLevel,
    /// After Enhance page: leave the tool for this command (Crop pages, Organize pages).
    CameraNext(&'static str),
    /// The find bar's Replace (the highlighted match) or Replace all.
    Replace {
        all: bool,
    },
    /// Lock now, on the notice shown after a signature is placed (#2149).
    LockSignatures,
}

impl DocView {
    /// Whether a middle-button scrolling gesture is active (tests and automation).
    pub fn auto_scrolling(&self) -> bool {
        self.middle_scrolling
    }

    /// The tab is off screen: a middle-button gesture its last pass recorded has ended with it, as
    /// production's scroll did when the active view changed.
    pub(crate) fn left_screen(&mut self) {
        self.middle_scrolling = false;
    }

    /// Whether a held middle-button pan is active, outside Linux (tests and automation).
    pub fn middle_panning(&self) -> bool {
        // PLAN: `CanvasOutput` reports one `auto_scrolling` for a latched and a held middle scroll (CV-C6) and no
        // separate held-pan fact, so both readers read that record.
        self.middle_scrolling
    }

    /// The current page: the one the page box shows and paging moves from.
    pub fn current(&self) -> usize {
        self.view.current
    }

    /// The fit the view keeps as it resizes.
    pub fn fit(&self) -> Fit {
        self.view.fit
    }

    /// The zoom (1.0 = 100%), equal to what a fit shows.
    pub fn zoom(&self) -> f64 {
        self.view.zoom
    }

    /// The canvas view of the last pass, as the framework's zoom field reads it: its zoom is the view's own zoom, so
    /// the field's points scale shows it as the percentage the view shows. `None` before the view was first shown.
    pub fn canvas_view(&self) -> Option<CanvasView> {
        self.canvas
    }

    /// Takes the zoom of a view the zoom field committed an edit of (from [`canvas_view`](Self::canvas_view)) into the
    /// view: a typed or stepped zoom ends the fit; the next pass clamps it to the zoom range and keeps the view centre.
    /// The caller calls it only for the field's committed edit.
    pub fn apply_canvas_view(&mut self, edited: &CanvasView) {
        if edited.zoom.is_finite() {
            self.view.fit = Fit::None;
            self.view.zoom = edited.zoom;
        }
    }

    /// Abandon only pointer previews: clearing egui's held button must not commit an app-owned
    /// grab through a tool's `!primary_down` fallback. Keep selections, text drafts and edits
    /// that were already completed before cancellation.
    pub(crate) fn cancel_pointer_gestures(&mut self, ctx: &egui::Context) {
        self.session.comments.tool.cancel_drawing();
        self.cancel_page_drags(ctx);
        craft::ui::primitives::cancel_press_of(ctx, page_parts::control_id(self.view_key()));
    }

    /// A view of a newly opened document, shown as `defaults` say, with fields highlighted when `highlight_fields`
    /// (the app's preference). Opening a file then applies any layout or zoom the PDF itself asks for.
    pub fn new(id: DocId, info: &DocInfo, defaults: ViewDefaults, highlight_fields: bool) -> Self {
        Self {
            pane: Default::default(),
            id,
            from_attachment: None,
            view: PageViewState::from_defaults(&defaults),
            session: PageSession::new(info, highlight_fields),
            momentary: Momentary::new(NavigationOptions::default()),
            momentary_options: NavigationOptions::default(),
            revisions: PageRevisions::default(),
            reports: RenderReports::default(),
            textures: TextureStatistics::default(),
            spacing: None,
            viewport: Rect::NOTHING,
            canvas: None,
            middle_scrolling: false,
            organize: false,
            page_input: "1".into(),
            page_input_draft: false,
            page_input_reselect: false,
            notice_dismissed: false,
            scripts_off_noted: false,
            disk_changed: false,
            close_requested: false,
            snapshot: None,
            back: Vec::new(),
            forward: Vec::new(),
            interface: Default::default(),
            presentation: None,
<<<<<<< HEAD
=======
            full_screen_fill: defaults.full_screen_fill,
            presenter: Default::default(),
            pages: HashMap::new(),
            errors: HashMap::new(),
            cut_pages: Default::default(),
>>>>>>> upstream/main
            cut_notice_dismissed: Default::default(),
            find: None,
            page_count: info.pages.len(),
            page_sizes: info.pages.iter().map(|p| (p.width, p.height)).collect(),
            auto_ocr_tried: Default::default(),
            auto_ocr_skipped: Default::default(),
            auto_ocr_awaiting_models: Default::default(),
            selected: Selection::new(Vec::new(), None, None),
            pending_action: None,
            insert_at: None,
            grid_gap: None,
            pages_panel: None,
            panel_gap: None,
            dropped_files: None,
            pages_focus_id: None,
            thumbnail_zoom: 1.0,
            panel_followed: None,
            panel_zoom_followed: 1.0,
            grid_reveal: None,
            bookmarks_open: crate::panels::bookmarks::opened_as_filed(&info.outline),
            redactions_applied: 0,
            redactions_undone_after_save: 0,
            saved_redactions_cut_off: false,
            redactions_downloaded: 0,
            lock_offer_dismissed: false,
        }
    }

    /// The document's content or page list changed (an edit, undo or redo): drop caches and
    /// adopt the new page geometry, keeping the reader's place where possible. The Organize grid's open press ends:
    /// it holds page indexes from before the change.
    pub fn document_changed(&mut self, ctx: &egui::Context, info: &DocInfo) {
        self.invalidate_content();
        self.page_count = info.pages.len();
        self.page_sizes = info.pages.iter().map(|p| (p.width, p.height)).collect();
        let last = self.page_count.saturating_sub(1);
        self.view.current = self.view.current.min(last);
        let gone = |p: &usize| *p > last;
        self.selected.items.retain(|p| !gone(p));
        if self.selected.lead.as_ref().is_some_and(gone) {
            self.selected.lead = None;
        }
        if self.selected.anchor.as_ref().is_some_and(gone) {
            self.selected.anchor = None;
        }
        // Text being typed on a page that no longer exists goes with the page.
        if self.session.content.draft.as_ref().is_some_and(|d| d.page > last || self.page_count == 0) {
            self.session.content.draft = None;
        }
        // A Snapshot area names a page by its index and a rectangle on it: after pages were
        // deleted or moved it would outline, print and save a different page.
        self.session.area_pick = None;
        self.view.go_to = None;
        self.session.flash = None;
        // Thumbnail drags hold page indexes from before the change.
        self.cancel_page_drags(ctx);
        self.session.comments.document_changed(info);
    }

    /// The document was replaced by a newer version of its file (#431). The reader's place
    /// stays: zoom, layout, rotation, page, scroll, full screen, view history and the find bar,
    /// the snapshot marker and its pending close. Everything that pointed into the old version (a selected
    /// comment, a field being typed in, text being edited, a drag, page selection) starts
    /// afresh, so no stale index reaches the new one.
    pub fn document_replaced(&mut self, ctx: &egui::Context, info: &DocInfo) {
        let mut fresh = Self::new(self.id, info, ViewDefaults::default(), self.session.highlight_fields);
        // Display, page and scroll offset: the reader's place.
        fresh.view = self.view.clone();
        fresh.organize = self.organize;
        fresh.thumbnail_zoom = self.thumbnail_zoom;
        fresh.notice_dismissed = self.notice_dismissed;
        fresh.cut_notice_dismissed = std::mem::take(&mut self.cut_notice_dismissed);
        fresh.snapshot = self.snapshot.clone();
        fresh.close_requested = self.close_requested;
        // What the document asked to hide, and what is hidden now (F8), belong to the tab.
        fresh.interface = self.interface;
        // Still presenting, with the display to go back to on leaving.
        fresh.presentation = self.presentation.take();
<<<<<<< HEAD
=======
        fresh.full_screen_fill = self.full_screen_fill;
        // A new document: none of the old one's transition, page or timer carries over.
        fresh.presenter = Default::default();
>>>>>>> upstream/main
        fresh.back = std::mem::take(&mut self.back);
        fresh.forward = std::mem::take(&mut self.forward);
        fresh.find = self.find.take();
        // The revision stamps carry on, so the pictures on screen stay until their replacements land.
        fresh.revisions = std::mem::take(&mut self.revisions);
        fresh.viewport = self.viewport;
        // Same view key (`pane`): the page view's remembered state and the grid's press id are still this view's.
        fresh.pane = self.pane.clone();
        *self = fresh;
        // Marks the kept pictures out of date and clamps the page to the new page count.
        self.document_changed(ctx, info);
        self.page_input = self.current_page_label(info.pages.get(self.view.current).map(|page| page.label.as_str()));
        self.page_input_reselect = true;
    }

    /// Another view of this document changed it (split view: an edit, Undo,
    /// a reload): start afresh as a reload does, keeping the reader's place, so nothing here
    /// points into the old version.
    pub(crate) fn follow_document(&mut self, ctx: &egui::Context, info: &DocInfo) {
        let continuous = self.view.continuous;
        self.document_replaced(ctx, info);
        self.view.continuous = continuous;
    }

    /// A second view of this document (split view), showing what this one shows.
    pub(crate) fn twin(&self, info: &DocInfo) -> Self {
        let mut v = Self::new(self.id, info, ViewDefaults::default(), self.session.highlight_fields);
        v.view = self.view.clone();
        v.view.go_to(self.view.current);
        v.page_input = self.page_input.clone();
        v.notice_dismissed = self.notice_dismissed;
        v.scripts_off_noted = self.scripts_off_noted;
<<<<<<< HEAD
=======
        v.full_screen_fill = self.full_screen_fill;
        v.goto = Some((self.current, 0.0));
>>>>>>> upstream/main
        v
    }

    /// The key that tells this view apart from another view of the same document (split view):
    /// unique for the app's life. Ids of what is drawn over the page are keyed by it.
    pub fn view_key(&self) -> u64 {
        self.pane.key
    }

    /// Where this tab shows: the main pane or split view's second pane.
    pub fn place(&self) -> crate::panes::Place {
        self.pane.place
    }

    /// The number of pages the view was last given.
    pub(crate) fn page_count(&self) -> usize {
        self.page_count
    }
<<<<<<< HEAD
=======

    /// Drops wheel state after a discontinuous jump so the destination cannot
    /// inherit momentum from the previous viewport.
    fn reset_coarse_state(&mut self) {
        self.wheel.clear_motion();
        self.wheel_boost_x.reset();
        self.wheel_boost_y.reset();
        self.wheel_motion.reset();
    }
}

/// The renders a frame wants (page, scale, tag, tile), and the pages visible.
type PageRenders = (Vec<(usize, f32, u64, Option<Tile>)>, Vec<usize>);

/// What the page-scroll closure reads: this frame's zoom, scrolling, tool and document state.
#[derive(Clone, Copy)]
struct PageEnv<'a> {
    added: &'a Vec<pdfcraft_engine::Added>,
    allowed: bool,
    author: &'a String,
    auto_delta: Vec2,
    avail: Rect,
    coarse_scroll: Vec2,
    by_line: &'a String,
    calc_order: bool,
    can_copy: bool,
    /// Copied comments wait to be pasted.
    can_paste_comments: bool,
    /// Copied comments hang on the pointer (`Gesture::Place`): the click is theirs.
    placing: bool,
    can_crop: bool,
    can_fill: bool,
    can_modify: bool,
    comments_hidden: bool,
    content_h: f32,
    content_w: f32,
    /// The custom stamp's line, filled in now (#1741).
    custom_line: &'a Option<String>,
    custom_stamp: &'a Option<crate::stamps_ui::CustomStamp>,
    date_text: &'a Result<String, String>,
    doc: &'a pdfcraft_engine::Document,
    doc_links: &'a Vec<pdfcraft_engine::LinkItem>,
    edge_delta: Vec2,
    editing_content: bool,
    form: &'a Arc<Vec<pdfcraft_engine::FormField>>,
    generation: u64,
    glide: Vec2,
    hand: bool,
    info: &'a DocInfo,
    /// The Animation effects preference as it applies now (full screen's transitions follow it).
    animates: bool,
    initials: &'a Option<crate::fill_sign::SavedSig>,
    key_scroll: f32,
    key_side: f32,
    /// This frame's keys as the pages see them.
    keys: crate::page_keys::PageKeys,
    middle_gesture: bool,
    modal: bool,
    navigation_enabled: bool,
    paper: Color32,
    show_page_edge_shadows: bool,
    ppp: f32,
    precise_scroll: Vec2,
    prefs: &'a comments::CommentPrefs,
    preparing: bool,
    prepare_tab_order: Option<&'a [(usize, usize)]>,
    rects: &'a Vec<Rect>,
    redact_mark: ([f64; 3], f64),
    scale: f32,
    selection_wheel: bool,
    selects_text: bool,
    signature: &'a Option<crate::fill_sign::SavedSig>,
    single_paging: bool,
    t: &'a Tokens,
    table_preview: &'a Option<crate::field_table_ui::Preview>,
    tag: u64,
    text_style: &'a pdfcraft_engine::AddedText,
    /// Preferences: addresses written as plain page text open like links (#4207).
    text_links: bool,
    tool: QuickTool,
    units: crate::units::Unit,
    unobstructed: bool,
    visible_pages: &'a Vec<usize>,
    y_shift: f32,
    fill_text_size: f64,
    fill_text_color: [f64; 3],
    fill_text_border: crate::fill_sign::TextBorder,
    today: (i64, u32, u32),
    can_paste_image: bool,
}

/// What the page-scroll closure produces for the rest of the frame.
#[derive(Default)]
struct PageOut {
    canvas_action: Option<comments::CanvasAction>,
    clicked_link: Option<LinkTarget>,
    /// The JavaScript link clicked, by page and annotation index (0-based), to run its action.
    javascript_link: Option<(usize, usize)>,
    comment_notice: Option<&'static str>,
    copy_comments: bool,
    copy_selected: bool,
    field_menu: Option<FieldMenu>,
    field_placed: bool,
    field_props: bool,
    hover_text: Option<(Pos2, String)>,
    image_action: Option<crate::edit_text_ui::ImageAction>,
    open_boxes: bool,
    open_initials: bool,
    open_signature: bool,
    open_table: Option<(usize, [f64; 4])>,
    page_action: Option<PageMenuAction>,
    paste_comments_at: Option<(usize, [f64; 2])>,
    primary_down: bool,
    /// The pointer is over a link, comment, field or what is placed on the page: a click there is
    /// for it, and turns no page in full screen.
    on_object: bool,
    refused: Option<String>,
    /// The current page or spread where it is drawn, every page of it: one off screen (the second
    /// page of a zoomed spread) has no screen transform, but still decides where the row ends.
    row_on_screen: Option<Rect>,
    stamp_placed: bool,
}

/// The page-scroll closure's body: lays out and draws the visible pages, their tools and overlays, and
/// returns the renders wanted and the pages visible.
/// The state of one pass over the visible pages: what is read, and what the pages add up to.
struct PageFrame<'a> {
    env: PageEnv<'a>,
    resp: &'a egui::Response,
    ptr: &'a PagePointer,
    origin: Pos2,
    visible: Rect,
    pressed: bool,
    pointer: Option<Pos2>,
    signature_preview: &'a mut Option<(crate::fill_sign::SavedSig, egui::TextureHandle)>,
    out: PageOut,
    wanted: Vec<(usize, f32, u64, Option<Tile>)>,
    visible_now: Vec<usize>,
    current: usize,
    best_overlap: f32,
    current_overlap: f32,
    current_height: f32,
    pressed_page: Option<usize>,
    calc_hover: Option<crate::calc_order_ui::Focus>,
    /// The form field widget under the pointer on any page, for its Mouse Enter and Exit scripts.
    field_hover: Option<crate::forms_ui::WidgetRef>,
    /// A page took this frame's right-click.
    right_clicked: bool,
}

impl PageFrame<'_> {
    /// Where this frame's right-click is, for the first page it is `on`: pages that touch (the gaps
    /// hidden) can share a row of pixels, and a right-click there is one page's, as a press is.
    fn right_click_on(&mut self, on: impl FnOnce(Pos2) -> bool) -> Option<Pos2> {
        let p = self.pointer.filter(|p| !self.right_clicked && self.resp.secondary_clicked() && on(*p))?;
        self.right_clicked = true;
        Some(p)
    }
}

/// Visible heights (points) closer than this are equal: one page size can meet the viewport a
/// fraction of a point differently once the scroll offset is applied.
const PAGE_TIE: f32 = 0.5;

/// Draws page `i` of the visible pages: its raster, the tools' input and overlays.
/// One visible page: its index, its rectangle on screen and the transform to and from the page.
#[derive(Clone, Copy)]
struct PageSlot {
    i: usize,
    r: Rect,
    xf: PageXform,
}

/// Paints the page's paper, shadow and raster (or why it can't be shown), and asks for the renders
/// it wants.
#[allow(clippy::too_many_arguments)]
fn paint_image_suppressed(
    xf: &PageXform,
    painter: &egui::Painter,
    tex: egui::TextureId,
    u0: f32,
    v0: f32,
    u1: f32,
    v1: f32,
    suppress: Option<[f32; 4]>,
    is_full_page_tex: bool,
) {
    let Some([su0, sv0, su1, sv1]) = suppress else {
        if is_full_page_tex {
            xf.paint_page_texture_rect(painter, tex, u0, v0, u1, v1);
        } else {
            xf.paint_image(painter, tex, u0, v0, u1, v1);
        }
        return;
    };
    let iu0 = u0.max(su0);
    let iu1 = u1.min(su1);
    let iv0 = v0.max(sv0);
    let iv1 = v1.min(sv1);
    if iu0 >= iu1 || iv0 >= iv1 {
        if is_full_page_tex {
            xf.paint_page_texture_rect(painter, tex, u0, v0, u1, v1);
        } else {
            xf.paint_image(painter, tex, u0, v0, u1, v1);
        }
        return;
    }
    let map_uv = |pu: f32, pv: f32| -> (f32, f32) {
        if is_full_page_tex {
            (pu, pv)
        } else {
            let du = (u1 - u0).max(1e-6);
            let dv = (v1 - v0).max(1e-6);
            ((pu - u0) / du, (pv - v0) / dv)
        }
    };
    let draw_strip = |pu0: f32, pv0: f32, pu1: f32, pv1: f32| {
        if pu1 <= pu0 || pv1 <= pv0 {
            return;
        }
        let (tu0, tv0) = map_uv(pu0, pv0);
        let (tu1, tv1) = map_uv(pu1, pv1);
        xf.paint_image_region(painter, tex, pu0, pv0, pu1, pv1, tu0, tv0, tu1, tv1);
    };

    if iv0 > v0 {
        draw_strip(u0, v0, u1, iv0);
    }
    if v1 > iv1 {
        draw_strip(u0, iv1, u1, v1);
    }
    if iu0 > u0 {
        draw_strip(u0, iv0, iu0, iv1);
    }
    if u1 > iu1 {
        draw_strip(iu1, iv0, u1, iv1);
    }
}

fn paint_page_surface(ui: &mut egui::Ui, view: &mut DocView, frame: &mut PageFrame<'_>, slot: &PageSlot) {
    let PageEnv { paper, ppp, scale, t, tag, .. } = frame.env;
    let PageSlot { i, r, xf } = *slot;
    let visible = frame.visible;
    let info = frame.env.info;
    let painter = ui.painter();
    // Shadows would darken the preceding page when pages touch; full screen filling the screen
    // shows the page alone on black (#576).
    if view.show_page_gaps && frame.env.show_page_edge_shadows && !view.fills_screen() {
        painter.add(egui::epaint::Shadow { offset: [0, 3], blur: 14, spread: 0, color: t.page_shadow }.as_shape(r, CornerRadius::ZERO));
    }
    painter.rect_filled(r, CornerRadius::ZERO, paper);
    if let Some(err) = view.errors.get(&i) {
        painter.rect_filled(r, CornerRadius::ZERO, Color32::from_rgb(0xFB, 0xF4, 0xF4));
        icons::paint(
            ui,
            Rect::from_center_size(r.center() - vec2(0.0, 26.0), vec2(28.0, 28.0)),
            "triangle-alert",
            26.0,
            Color32::from_rgb(0xC8, 0x3A, 0x3A),
        );
        let message = crate::i18n::fmt(tl!("This page couldn't be displayed.\n{e}"), &[("e", err)]);
        let msg = ui.fonts_mut(|f| f.layout(message, theme::regular(12.5), Color32::from_rgb(0x6A, 0x2A, 0x2A), (r.width() - 40.0).max(80.0)));
        painter.galley(pos2(r.center().x - msg.size().x / 2.0, r.center().y), msg, Color32::BLACK);
    } else {
        let (pw_pt, ph_pt) = (info.pages[i].width.max(1.0), info.pages[i].height.max(1.0));
        // A GPU may take smaller textures than these (OpenGL drivers report as little as
        // 2048 pixels, and uploading a bigger one aborts), so both stay within its limit.
        let max_side = ui.ctx().input(|inp| inp.max_texture_side) as f32;
        let tiled = pw_pt.max(ph_pt) * scale > TILE_THRESHOLD.min(max_side);
        // Whole-page raster: sharp when small, a low-res backdrop when tiled.
        let (want_scale, want_tag) = if tiled {
            let bs = BASE_SIDE.min(max_side) / pw_pt.max(ph_pt);
            (bs, scale_tag(bs))
        } else {
            (scale, tag)
        };
        // Under tiles, a whole-page raster sharper than the backdrop (from before the page
        // grew past the tiling size) is a better backdrop: keep demanding it as it is.
        match view.pages.get(&i).filter(|p| tiled && p.tag != STALE_TAG && tag_scale(p.tag) >= want_scale) {
            Some(p) => frame.wanted.push((i, tag_scale(p.tag), p.tag, None)),
            None => frame.wanted.push((i, want_scale, want_tag, None)),
        }
        let doc = frame.env.doc;
        let active_drag_suppress = if let Some(sel) = view.image_selection.clone()
            && sel.page == i
            && sel.drag.is_some()
        {
            let images = cached_edit_images(view, doc, i);
            if let Some(img) = images.get(sel.index) {
                let obb = ImageObb::from_image(img, &xf, info, i);
                let uvs = obb.corners().map(|c| xf.screen_to_norm(c));
                let u0 = uvs.iter().map(|(u, _)| *u).fold(1.0f32, f32::min).clamp(0.0, 1.0);
                let u1 = uvs.iter().map(|(u, _)| *u).fold(0.0f32, f32::max).clamp(0.0, 1.0);
                let v0 = uvs.iter().map(|(_, v)| *v).fold(1.0f32, f32::min).clamp(0.0, 1.0);
                let v1 = uvs.iter().map(|(_, v)| *v).fold(0.0f32, f32::max).clamp(0.0, 1.0);
                if u1 > u0 && v1 > v0 { Some([u0, v0, u1, v1]) } else { None }
            } else {
                None
            }
        } else {
            None
        };
        match view.pages.get(&i) {
            Some(p) => {
                if !tiled && p.tag == want_tag {
                    paint_image_suppressed(&xf.texel_aligned(p.tex.size(), ppp), painter, p.tex.id(), 0.0, 0.0, 1.0, 1.0, active_drag_suppress, true);
                } else {
                    // A backdrop, or a raster at an older scale until the new one arrives.
                    paint_image_suppressed(&xf, painter, p.tex.id(), 0.0, 0.0, 1.0, 1.0, active_drag_suppress, true);
                }
            }
            None => {
                let now = ui.input(|inp| inp.time);
                let since = *view.waiting_since.entry(i).or_insert(now);
                let msg = if now - since > 6.0 { "Still rendering — this page is unusually complex…" } else { "Rendering…" };
                painter.text(r.center(), Align2::CENTER_CENTER, tl!(msg), theme::regular(12.0), t.text_faint);
            }
        }
        if tiled && r.intersects(visible) {
            paint_page_tiles(ui, view, frame, slot, (pw_pt, ph_pt, max_side), active_drag_suppress);
        }
    }
}

/// Paints the tiles of a page too big to draw whole: the tiles of earlier zooms first, stretched,
/// then the visible tiles of this one, nearest the viewport centre first, asking for those missing.
fn paint_page_tiles(
    ui: &egui::Ui,
    view: &mut DocView,
    frame: &mut PageFrame<'_>,
    slot: &PageSlot,
    (pw_pt, ph_pt, max_side): (f32, f32, f32),
    active_drag_suppress: Option<[f32; 4]>,
) {
    let PageEnv { ppp, scale, tag, .. } = frame.env;
    let PageSlot { i, r, xf } = *slot;
    let visible = frame.visible;
    let painter = ui.painter();
    // Device-pixel geometry of the scaled page, and the visible part of it
    // (found by mapping the visible screen corners back into the page). Sides round
    // up as whole-page rasters do, so the last partial row and column get tiles.
    let (dw, dh) = (device_pixels(pw_pt, scale), device_pixels(ph_pt, scale));
    // Adaptive tile side for this zoom and page, within the GPU texture limit:
    // a zoomed viewport or a giant sheet is covered by a few large tiles
    // instead of many small ones, and each tile costs one full page interpret.
    // Never zero (u32 division below).
    let ts = tile_side(scale, dw.max(dh)).min(max_side as u32).max(64);
    let vis = r.intersect(visible);
    let corners = [vis.left_top(), vis.right_top(), vis.right_bottom(), vis.left_bottom()].map(|c| xf.screen_to_norm(c));
    let (u0, u1) = corners.iter().fold((1.0f32, 0.0f32), |(a, b), c| (a.min(c.0), b.max(c.0)));
    let (v0, v1) = corners.iter().fold((1.0f32, 0.0f32), |(a, b), c| (a.min(c.1), b.max(c.1)));
    let (vx0, vy0) = ((u0.max(0.0) * dw as f32) as u32, (v0.max(0.0) * dh as f32) as u32);
    let (vx1, vy1) = (((u1.min(1.0) * dw as f32).ceil() as u32).min(dw), ((v1.min(1.0) * dh as f32).ceil() as u32).min(dh));
    // Tiles of earlier zooms first, stretched, the nearest scale last (on top):
    // until the current tiles arrive, a zoom shows them rather than the backdrop.
    let mut older: Vec<(f32, u32, u32, &TextureHandle)> =
        view.tiles.iter().filter(|((p, t, _, _), _)| *p == i && *t != tag).map(|((_, t, ox, oy), tex)| (tag_scale(*t), *ox, *oy, tex)).collect();
    older.sort_by(|a, b| (b.0 / scale).ln().abs().total_cmp(&(a.0 / scale).ln().abs()));
    for (s, ox, oy, tex) in older {
        let (ow, oh) = (device_pixels(pw_pt, s) as f32, device_pixels(ph_pt, s) as f32);
        let [w, h] = tex.size();
        let (x, y) = (ox as f32, oy as f32);
        paint_image_suppressed(&xf, painter, tex.id(), x / ow, y / oh, (x + w as f32) / ow, (y + h as f32) / oh, active_drag_suppress, false);
    }
    // Visible tiles, nearest the viewport centre first: each tile costs a full
    // page interpret, so the workers sharpen what is looked at before the edges.
    let (vcx, vcy) = ((vx0 + vx1) as f32 / 2.0, (vy0 + vy1) as f32 / 2.0);
    let mut coords: Vec<(u32, u32)> = Vec::new();
    for ty in vy0 / ts..=(vy1.saturating_sub(1)) / ts {
        for tx in vx0 / ts..=(vx1.saturating_sub(1)) / ts {
            coords.push((tx, ty));
        }
    }
    coords.sort_by(|a, b| {
        let da = ((a.0 * ts + ts / 2) as f32 - vcx).abs() + ((a.1 * ts + ts / 2) as f32 - vcy).abs();
        let db = ((b.0 * ts + ts / 2) as f32 - vcx).abs() + ((b.1 * ts + ts / 2) as f32 - vcy).abs();
        da.total_cmp(&db)
    });
    for (tx, ty) in coords {
        let (x, y) = (tx * ts, ty * ts);
        let (w, h) = (ts.min(dw.saturating_sub(x)), ts.min(dh.saturating_sub(y)));
        if w == 0 || h == 0 {
            continue;
        }
        frame.wanted.push((i, scale, tag, Some(Tile { x, y, w, h })));
        if let Some(tex) = view.tiles.get(&(i, tag, x, y)) {
            let (fw, fh) = (dw as f32, dh as f32);
            paint_image_suppressed(
                &xf.texel_aligned([dw as usize, dh as usize], ppp),
                painter,
                tex.id(),
                x as f32 / fw,
                y as f32 / fh,
                (x + w) as f32 / fw,
                (y + h) as f32 / fh,
                active_drag_suppress,
                false,
            );
        }
    }
}

/// Images parsed once per page and document edit generation, shared by painting and menus.
fn cached_edit_images<'a>(view: &'a mut DocView, doc: &pdfcraft_engine::Document, page: usize) -> &'a [pdfcraft_engine::PageImage] {
    let generation = doc.edit_generation();
    let entry = view.edit_images.entry(page).or_insert_with(|| (generation, doc.page_images(page)));
    if entry.0 != generation {
        *entry = (generation, doc.page_images(page));
    }
    &entry.1
}

fn draw_page(ui: &mut egui::Ui, view: &mut DocView, frame: &mut PageFrame<'_>, i: usize) {
    let PageEnv { allowed, comments_hidden, doc, editing_content, info, keys, ppp, prefs, rects, redact_mark, scale, t, tool, .. } = frame.env;
    let PageFrame { ptr, origin, visible, pressed, pointer, .. } = *frame;
    let r = snap_to_pixels(rects[i].translate(origin.to_vec2()), ppp);
    if !r.intersects(visible.expand(400.0)) {
        return;
    }
    if pressed && frame.pressed_page.is_none() && pointer.is_some_and(|p| r.contains(p)) {
        frame.pressed_page = Some(i);
    }
    if r.intersects(visible) {
        frame.visible_now.push(i);
        view.screen_rects.push((i, r));
        view.screen_xforms.push((i, PageXform { rect: r, rot: view.rotation, pw: info.pages[i].width.max(1.0), ph: info.pages[i].height.max(1.0) }));
    }
    let overlap = r.intersect(visible).height();
    if i == view.current {
        frame.current_overlap = overlap;
        frame.current_height = r.height();
    }
    // Pages shown equally (two rows wholly on screen) differ only by rounding: the
    // topmost one counts.
    if overlap > frame.best_overlap + PAGE_TIE {
        frame.best_overlap = overlap;
        frame.current = i;
    }
    let xf = PageXform { rect: r, rot: view.rotation, pw: info.pages[i].width.max(1.0), ph: info.pages[i].height.max(1.0) };
    paint_page_surface(ui, view, frame, &PageSlot { i, r, xf });
    let painter = ui.painter();
    view.text_background.paint(painter, &xf, i, view.line_editor.as_ref(), view.content.draft.as_ref());
    if editing_content {
        view.move_background.paint(painter, &xf, i, info, &view.content, pointer);
    }
    if view.show_page_gaps {
        painter.rect_stroke(r, CornerRadius::ZERO, Stroke::new(0.5, t.border.gamma_multiply(0.8)), egui::StrokeKind::Outside);
    }

    let slot = PageSlot { i, r, xf };
    // Addresses written as plain text are found once per text layer, before the tools hit-test them.
    if frame.env.text_links {
        view.refresh_text_links(i);
    }
    let pcx = comments::PageCx { page: i, xf: &xf, ptr, info, tool, prefs, allowed, hidden: comments_hidden, redact_mark, keys };
    // A cancelled press takes nothing until the button is up, though the page's menu still opens
    // where a right-click is. A page full screen doesn't show yet takes nothing at all. (A press
    // there gives nothing on the page an owner, so its release, once the page is in, takes nothing
    // either.)
    let veiled = page_input::veiled(view);
    let input = if view.press_cancelled || veiled {
        if !veiled && let Some(p) = frame.right_click_on(|p| xf.rect.contains(p)) {
            comments::anchor(&pcx, p, view);
        }
        PageInput {
            press: crate::page_router::PagePress::Elsewhere,
            text_drag: false,
            consumed: true,
            area_tool: false,
            hover: None,
            released_over: false,
            sign_tabs: Vec::new(),
        }
    } else {
        page_tool_input(ui, view, frame, &slot)
    };
    let painter = ui.painter();
    let preview_target = (matches!(tool, QuickTool::Select | QuickTool::Fill(_)) && !comments_hidden).then_some(view.comments.selected).flatten();
    view.signature_drag.prepare(ui.ctx(), doc, preview_target, scale);
    view.signature_drag.paint(painter, &pcx, &view.comments, view.pending_edit.as_ref());
    // Any other comment being moved: drawn where it goes, over the page that still shows it
    // where it was.
    let stale = view.signature_drag.moving_page().is_some_and(|p| view.page_out_of_date(p));
    view.signature_drag.prepare_move(ui.ctx(), doc, &view.comments, view.pending_edit.as_ref(), stale, scale);
    view.signature_drag.paint_move(painter, &pcx, &view.comments, view.pending_edit.as_ref());

    // Any press on the page ends Select All of the document: text read or not, any tool,
    // on a link too.
    // A press that lands on something drawn over the page (a menu, a dialog) is not one.
    if view.all_text && frame.pressed_page == Some(i) {
        view.all_text = false;
    }
    text_layer(ui, view, frame, &slot, &input);
    page_overlays(ui, view, frame, &slot, &input);
}

#[allow(clippy::cognitive_complexity)]
fn draw_pages(
    ui: &mut egui::Ui,
    viewport: Rect,
    view: &mut DocView,
    env: &PageEnv<'_>,
    out: &mut PageOut,
    signature_preview: &mut Option<(crate::fill_sign::SavedSig, egui::TextureHandle)>,
) -> PageRenders {
    let PageEnv {
        allowed,
        auto_delta,
        avail,
        coarse_scroll,
        calc_order,
        can_copy,
        can_paste_comments,
        can_modify,
        content_h,
        content_w,
        doc,
        edge_delta,
        form,
        generation,
        glide,
        hand,
        info,
        key_scroll,
        key_side,
        middle_gesture,
        modal,
        navigation_enabled,
        precise_scroll,
        prefs,
        preparing,
        selection_wheel,
        single_paging,
        tool,
        unobstructed,
        visible_pages,
        y_shift,
        ..
    } = *env;
    let animates = env.animates;
    // egui's drag responses accept every pointer button. Keep a wheel gesture from also
    // selecting text, drawing a mark, or panning with the Hand tool.
    if middle_gesture {
        let opacity = ui.opacity();
        ui.disable();
        // Only interactions pause; the document must keep its original colours.
        ui.set_opacity(opacity);
    }
    // These deltas already contain their motion policy. Positive y moves the content down;
    // applying another ScrollArea animation would ease the wheel twice.
    // ScrollArea ignores wheel input during a widget drag. Defer the wheel contribution
    // until after the text widgets when a selection owns that drag.
    let wheel_delta = if selection_wheel { Vec2::ZERO } else { precise_scroll + coarse_scroll };
    let delta = auto_delta + edge_delta - vec2(key_side, key_scroll) + wheel_delta + glide;
    if delta != Vec2::ZERO && delta.is_finite() {
        ui.scroll_with_delta_animation(delta, egui::style::ScrollAnimation::none());
    }
    // A resized handle can sit in the window beside a page that does not fill it. The
    // response has to cover that visible area or the next press never starts: the pointer
    // is reported only inside this rectangle once the previous drag has been released.
    // Page positions stay on `content_w`/`content_h`; the extra area is not a new page.
    let response_size = vec2(content_w.max(viewport.width()), content_h.max(viewport.height()));
    let (resp_rect, resp) = ui.allocate_exact_size(response_size, Sense::click_and_drag());
    view.page_widget = Some(resp.id);
    // A drag lasts only while egui reports it, so the mark can't outlive one.
    view.selection_drag_ended = view.selection_drag && resp.drag_stopped_by(egui::PointerButton::Primary);
    if !resp.dragged_by(egui::PointerButton::Primary) {
        view.selection_drag = false;
    }
    // The Hand tool pans by this widget's drag, before the pages are laid out (above).
    if hand {
        if resp.dragged() {
            view.cursor.claim(Layer::Drag, egui::CursorIcon::Grabbing);
        } else if resp.hovered() {
            view.cursor.claim(Layer::ToolArea, egui::CursorIcon::Grab);
        }
    }
    let origin = resp_rect.min - vec2(0.0, y_shift);
    let visible = viewport.translate(resp_rect.min.to_vec2());
    view.screen_rects.clear();
    view.screen_ppp = ui.ctx().pixels_per_point();
    view.screen_xforms.clear();
    // The pointer as the pages see it: nothing over them (a dialog from the frame it opens),
    // or a press they hold. The page modules read it rather than egui's raw pointer state.
    let ptr = PagePointer::read(ui, &resp, modal);
    cancel_on_escape(ui.ctx(), view, &ptr);
    view.router.settle(ptr.any_down, !ptr.edges.is_empty());
    // A new press is no longer the last one's, wherever it lands (the page it is on gives it
    // its owner; in the gap between pages it is no page's).
    if ptr.pressed().is_some() {
        view.router.begin();
        // The page widget is not focusable. Explicitly return keyboard ownership from Pages
        // when the user presses the document, without stealing focus through a modal.
        if let Some(id) = view.pages_focus_id {
            ui.memory_mut(|m| m.surrender_focus(id));
        }
    }
    let pointer = ptr.pos;
    // A press on a page makes it the current one (as in Acrobat), so a page command with
    // no selection (Rotate, Delete) acts on the page the reader pointed at, not on a page
    // gone to earlier that still shows whole beside it.
    let pressed = unobstructed && ptr.pressed().is_some();
    if resp.secondary_clicked() {
        // Set again below when the right-click lands on a link.
        view.links.context = None;
        // Set again below by the page right-clicked: a right-click off the pages (between them,
        // beside one) anchors no page's menu, which then acts on the current page. It deselects the
        // comment, and Edit text's picture, too (below), as one on a page's empty part does. A
        // selected Prepare field stays selected, and the menu is the page's there too.
        view.comments.context_at = None;
        view.menu_for = None;
    }
    let mut frame = PageFrame {
        env: *env,
        resp: &resp,
        ptr: &ptr,
        origin,
        visible,
        pressed,
        pointer,
        signature_preview,
        out: std::mem::take(out),
        wanted: Vec::new(),
        visible_now: Vec::new(),
        current: view.current,
        best_overlap: -1.0,
        current_overlap: -1.0,
        current_height: f32::INFINITY,
        pressed_page: None,
        calc_hover: None,
        field_hover: None,
        right_clicked: false,
    };
    for &i in visible_pages {
        draw_page(ui, view, &mut frame, i);
    }
    let PageFrame {
        wanted,
        visible_now,
        mut current,
        best_overlap,
        current_overlap,
        current_height,
        pressed_page,
        calc_hover,
        field_hover,
        out: page_out,
        right_clicked,
        ..
    } = frame;
    crate::forms_ui::hover_moved(view, field_hover);
    // A right-click no page took deselects the comment, so the page's menu opens, not the comment's,
    // and with Edit text the picture (one on a page went to that page, `page_input`).
    if resp.secondary_clicked() && !right_clicked {
        view.comments.selected = None;
        if tool == QuickTool::EditText {
            view.image_selection = None;
        }
    }
    let PageOut {
        mut canvas_action,
        clicked_link,
        javascript_link,
        copy_selected,
        mut field_menu,
        field_placed,
        field_props,
        hover_text: _,
        mut image_action,
        open_boxes,
        open_initials,
        open_signature,
        open_table,
        page_action: _,
        primary_down: _,
        on_object: _,
        refused,
        row_on_screen: _,
        stamp_placed,
        ..
    } = page_out;
    let mut page_action: Option<PageMenuAction> = None;
    let painter = ui.painter();
    if calc_order {
        view.calc_order.set_hover_page(calc_hover);
        let selected = view.prepare.selected.as_ref().map(|(name, _)| name.as_str());
        crate::calc_order_ui::paint_trace(ui, painter, visible, &view.calc_order, generation, selected);
    }
    // Full screen plays transitions over the page just drawn, and keeps time (#379).
    let mut hide_pointer = false;
    if view.presentation.is_some() {
        let now = ui.input(|i| i.time);
        hide_pointer = presenter::frame(view, info, painter, ui.ctx(), now, animates, pointer);
    }
    // Read before the claims are spent below, and before the pointer hides, which claims the top layer.
    let on_object = view.cursor.over_object(tool);
    // A still pointer hides over a link too, and its tip with it: the top layer, which every tool reaches.
    if hide_pointer {
        view.cursor.claim(Layer::Drag, egui::CursorIcon::None);
    }
    // The pages' one cursor, from the claim on the highest layer the tool reaches, and the tip.
    let hover_text = pointer.filter(|_| unobstructed && !hide_pointer).zip(view.cursor.resolve(ui.ctx(), tool));
    // ScrollArea ignores the wheel while any widget is dragged, so a text-selection drag
    // scrolls the page itself (#721). After the page loop, so it keeps scrolling once the
    // pointer leaves the selection's page for the gap or the next page; over the whole
    // document area. The viewer owns both precise and coarse wheel motion, so selection uses
    // the same distance as ordinary document scrolling.
    if selection_wheel {
        let delta = precise_scroll + coarse_scroll;
        if delta != Vec2::ZERO && delta.is_finite() {
            ui.scroll_with_delta_animation(delta, egui::style::ScrollAnimation::none());
        }
    }
    // Held at the top or bottom edge, or dragged past it, a text selection scrolls the view
    // so it can reach text off screen (#1670); the selection follows the pointer, onto the
    // pages it scrolls to.
    if view.selection_drag
        && navigation_enabled
        && unobstructed
        && !single_paging
        && let Some(p) = ptr.pos
    {
        let dy = crate::autoscroll::edge_delta(p.y, avail, ui.input(|i| i.stable_dt));
        if dy != 0.0 {
            view.end_find_following();
            ui.scroll_with_delta_animation(vec2(0.0, dy), egui::style::ScrollAnimation::none());
            // Keep scrolling while the pointer rests there and sends no events.
            ui.ctx().request_repaint();
        }
    }
    // The page navigated to stays current while no page shows more of itself, so the next
    // page step starts from it rather than from a page further down the screen (#188).
    // It also stays current while it is wholly on screen: a short page (a cheque) gone to
    // from the Pages panel shows less of itself than the tall page below it, and must not
    // hand the highlight to that page.
    if current_overlap >= best_overlap - PAGE_TIE || current_overlap >= current_height - PAGE_TIE {
        current = view.current;
    }
    if let Some(p) = pressed_page {
        current = p;
    }
    if view.continuous {
        view.current = current;
    }
    // The menu of what the right-click went to (`menu_for`), or else the page's.
    use crate::page_router::{Owner, PicturePart};
    let menu_for = view.menu_for.map(|(_, owner)| owner);
    // A right-click while a press is held leaves the selections alone, so the menu is its owner's only
    // when that owner is the one selected.
    let on_selected_field = match menu_for {
        Some(Owner::PrepareField { field, widget }) => {
            form.get(field).is_some_and(|f| view.prepare.selected.as_ref().is_some_and(|(name, w)| *name == f.name && *w == widget))
        }
        Some(Owner::PrepareCorner { .. }) => true,
        _ => false,
    };
    // The row as it is now: in continuous view the current page may have changed above.
    let row_on_screen =
        shown_row(view, env.rects).iter().filter_map(|&i| env.rects.get(i)).map(|r| r.translate(origin.to_vec2())).reduce(Rect::union);
    let presenting = view.presentation.is_some();
    page_context_menu(&resp, presenting, |ui| {
        // Crop mode has no menu: a right-click on the picture being cropped, its box or its grips is
        // the crop's (#5835).
        if matches!(menu_for, Some(Owner::Picture { part: PicturePart::Crop(_), .. })) {
            egui::Popup::close_all(ui.ctx());
            ui.close();
            return;
        }
        // Preparing a form: the selected field's menu, on a field.
        if preparing
            && on_selected_field
            && let Some((name, wi)) = view.prepare.selected.clone()
        {
            // Several fields: Align, Center, Distribute, Set Fields to Same Size.
            if !view.prepare.also.is_empty() {
                use crate::prepare::Arrange as A;
                let others = view.prepare.also.clone();
                let anchor = (name.clone(), wi);
                let mut pick = |ui: &mut egui::Ui, op: A, label: &str| {
                    if ui.add_enabled(can_modify, egui::Button::new(tl!(label))).clicked() {
                        view.pending_edit = crate::prepare::arrange(form, &anchor, &others, op);
                        ui.close();
                    }
                };
                ui.menu_button(tl!("Align"), |ui| {
                    pick(ui, A::AlignLeft, "Left");
                    pick(ui, A::AlignRight, "Right");
                    pick(ui, A::AlignTop, "Top");
                    pick(ui, A::AlignBottom, "Bottom");
                    pick(ui, A::AlignCenterV, "Center vertically");
                    pick(ui, A::AlignCenterH, "Center horizontally");
                });
                ui.menu_button(tl!("Distribute"), |ui| {
                    pick(ui, A::DistributeH, "Horizontally");
                    pick(ui, A::DistributeV, "Vertically");
                });
                ui.menu_button(tl!("Set Fields to Same Size"), |ui| {
                    pick(ui, A::SameHeight, "Height");
                    pick(ui, A::SameWidth, "Width");
                    pick(ui, A::SameSize, "Both");
                });
                ui.separator();
            }
            if ui.button(tl!("Properties…")).clicked() {
                field_menu = Some(FieldMenu::Properties);
                ui.close();
            }
            if ui.add_enabled(can_modify, egui::Button::new(tl!("Duplicate…"))).clicked() {
                field_menu = Some(FieldMenu::Duplicate(name.clone()));
                ui.close();
            }
            ui.separator();
            let delete = if view.prepare.also.is_empty() { "Delete" } else { "Delete selected fields" };
            if ui.add_enabled(can_modify, egui::Button::new(tl!(delete))).clicked() {
                view.pending_edit = crate::prepare::delete_selected(view);
                ui.close();
            }
            return;
        }
        if matches!(menu_for, Some(Owner::Link { .. }))
            && let Some((page, index)) = view.links.context
        {
            if ui.button(tl!("Properties…")).clicked() {
                view.links.open_existing = Some((page, index));
                ui.close();
            }
            if ui.add_enabled(can_modify, egui::Button::new(tl!("Delete"))).clicked() {
                view.pending_edit = Some(Edit::DeleteLink { page, index });
                view.links.selected = None;
                view.links.context = None;
                ui.close();
            }
            ui.separator();
            if ui.add_enabled(can_modify, egui::Button::new(tl!("Remove all links"))).clicked() {
                view.pending_edit = Some(Edit::RemoveLinks { pages: None });
                view.links.selected = None;
                view.links.context = None;
                ui.close();
            }
            return;
        }
        let can_paste_image = env.can_paste_image;
        if tool == QuickTool::EditText
            && let Some((page, Owner::Picture { index, .. })) = view.menu_for
            && let Some(sel) = view.image_selection.clone().filter(|s| (s.page, s.index) == (page, index))
        {
            let images = cached_edit_images(view, doc, sel.page).to_vec();
            // Paste lands where the menu opened, on the picture's page: the right-click that opened
            // it went to that page (`page_input`), and another page's point means nothing there.
            let at = view.comments.context_at.filter(|(page, _)| *page == sel.page).map(|(_, at)| at);
            let rotation = (view.rotation % 360 + info.pages.get(sel.page).map_or(0, |p| p.rotation % 360)) % 360;
            crate::edit_text_ui::image_context_menu(ui, view, &images, sel.page, sel.index, can_paste_image, &mut image_action, at, rotation);
            return;
        }
        // The comment the right-click went to, when it is the one selected, or connected lines being
        // drawn (Complete, Undo last point): the comment menu.
        let on_selected_comment =
            matches!(view.menu_for, Some((page, Owner::Comment { index, .. })) if view.comments.selected == Some((page, index)));
        let drawing_points = matches!(&view.comments.gesture, Some(comments::Gesture::Draw { tool, .. }) if tool.clicks_points());
        if on_selected_comment || drawing_points {
            canvas_action = comments::context_menu(
                ui,
                view,
                info,
                &|page, index| doc.annotation_deletable(page, index),
                prefs,
                comments::MenuRights { allowed, can_paste: can_paste_comments, can_modify },
            );
            return;
        }
        // Over a picture (and no comment): Copy Image, as in Acrobat's page menu.
        let mut copy_image = None;
        if let Some((page, at)) = view.comments.context_at {
            // On a page still being read, these items appear when the answer comes.
            let hit = view.pictures.get(ui.ctx(), doc, page).and_then(|pictures| pdfcraft_engine::picture_at(pictures, at)).map(|(index, _)| *index);
            if let Some(image) = hit {
                if ui.button(tl!("Copy Image")).clicked() {
                    copy_image = Some(comments::CanvasAction::CopyImage(page, image));
                    ui.close();
                }
                if ui.button(tl!("Copy Image in Original Format")).clicked() {
                    copy_image = Some(comments::CanvasAction::CopyImageFile(page, image));
                    ui.close();
                }
                ui.separator();
            }
        }
        let has_copy_image = copy_image.is_some();
        if copy_image.is_some() {
            canvas_action = copy_image;
        }
        let (page, at) = view.comments.context_at.unwrap_or((view.current, [0.0, 0.0]));
        // A text selection whose pages are still being read copies once they are.
        let has_text = view.selected_text().is_some() || view.selection_waits_for_text().is_some();
        let can_copy_items = can_copy && (view.image_selection.is_some() || has_text || has_copy_image);
        let can_paste = can_paste_image && can_modify;
        let perms = PageMenuPerms {
            can_modify,
            can_assemble: doc.allows_assembly(),
            can_annotate: allowed,
            can_print: doc.allows_printing(),
            can_copy,
            can_copy_items,
            can_paste,
            can_paste_comments: allowed && can_paste_comments,
            has_annotations: !info.annotations.is_empty(),
        };
        default_page_context_menu(ui, page, at, perms, &mut page_action);
    });
    // Copy comments / Paste comments from the page's menu or a Comments panel card.
    let copy_comments = std::mem::take(&mut view.comments.copy_request);
    let paste_comments_at = view.comments.paste_request.take();
    let comment_notice = view.comments.notice.take();
    *out = PageOut {
        canvas_action,
        clicked_link,
        javascript_link,
        comment_notice,
        copy_comments,
        copy_selected,
        field_menu,
        field_placed,
        field_props,
        hover_text,
        image_action,
        open_boxes,
        open_initials,
        open_signature,
        open_table,
        page_action,
        paste_comments_at,
        primary_down: ptr.primary_down,
        on_object,
        refused,
        row_on_screen,
        stamp_placed,
    };
    (wanted, visible_now)
}

/// A zoom the fit mode changed keeps the page at the top of the view in place, or, just after a
/// find step, the match in the middle of the view (`avail` is the view's size).
fn anchor_zoom_change(view: &mut DocView, avail: Vec2, old_zoom: f32) {
    let avail_width = avail.x;
    if view.shown && view.zoom_anchor.is_none() && view.goto.is_none() && view.goto_point.is_none() && (view.zoom - old_zoom).abs() > 1e-4 {
        let prev_viewport = view.viewport_screen;
        let top_y = prev_viewport.min.y;
        let anchor = view
            .screen_rects
            .iter()
            .find(|(_, r)| r.min.y <= top_y && r.max.y > top_y)
            .map(|(p, r)| {
                let fy = if r.height() > 0.0 { ((top_y - r.min.y) / r.height()).clamp(0.0, 1.0) } else { 0.0 };
                (*p, fy, 0.0f32)
            })
            .or_else(|| view.screen_rects.iter().find(|(_, r)| r.min.y >= top_y).map(|(p, r)| (*p, 0.0f32, (r.min.y - top_y).max(0.0))))
            .or_else(|| view.screen_rects.last().map(|(p, r)| (*p, 1.0f32, (r.max.y - top_y).max(0.0))));
        if let Some((page, fx, fy)) = view.find_hold {
            // A fit zoom that changes just after a find step keeps the match in the middle too.
            view.zoom_anchor = Some((page, fx, fy, avail / 2.0));
        } else if let Some((page, fy, rel_y)) = anchor {
            view.zoom_anchor = Some((page, 0.5, fy, vec2(avail_width / 2.0, rel_y)));
        } else {
            view.zoom_anchor = Some((view.current, 0.5, 0.0, vec2(avail_width / 2.0, view.margin())));
        }
    }
}

/// What the pager needs to know about the page row on screen.
#[derive(Clone, Copy)]
struct PagingCx<'a> {
    row: &'a [usize],
    rects: &'a [Rect],
    avail: Rect,
    middle_gesture: bool,
    unobstructed: bool,
}

/// Not scrolling: when the whole page or spread fits, the wheel turns pages instead. Returns whether
/// paging owns the wheel, and whether it turned the page at an edge.
fn single_page_paging(ui: &egui::Ui, view: &mut DocView, cx: &PagingCx<'_>) -> (bool, bool) {
    let (mut single_paging, mut edge_turned) = (false, false);
    let PagingCx { row, rects, avail, middle_gesture, unobstructed } = *cx;
    if !view.continuous {
        // The row's bounding box on screen (one page, or a two-page spread).
        let row_rect = row.iter().filter_map(|&i| rects.get(i)).copied().reduce(Rect::union);
        let fits = row_rect.is_some_and(|r| fits_height(r, avail.height(), ui.ctx().pixels_per_point()));
        let can_turn = !middle_gesture && unobstructed && ui.rect_contains_pointer(avail);
        single_paging = can_turn && fits;
        // Every wheel event goes to the pager, so it follows each trackpad touch to its end
        // even while the page can't turn.
        let now = ui.input(|i| i.time);
        for e in &crate::motion::gesture_wheel_events(ui.ctx()) {
            if let egui::Event::MouseWheel { unit, delta, phase, modifiers, .. } = e {
                // Zooming (⌘ or Ctrl) and sideways scrolling (Shift) never turn pages.
                let dy = if modifiers.command || modifiers.ctrl || modifiers.shift { 0.0 } else { delta.y };
                if fits {
                    view.single_page_wheel(*unit, dy, *phase, now, can_turn);
                } else {
                    edge_turned |= view.single_page_wheel_edge(*unit, dy, *phase, now, can_turn);
                }
            }
        }
        if single_paging {
            // Paging owns vertical wheel motion here. A diagonal gesture keeps its sideways
            // part, as `ScrollArea` itself handles each axis.
            ui.input_mut(|i| i.smooth_scroll_delta.y = 0.0);
        }
    }
    (single_paging, edge_turned)
}

/// This frame's wheel motion over the page view, after paging and motion integration.
struct Wheel {
    precise_scroll: Vec2,
    line_base: Vec2,
    coarse_scroll: Vec2,
    has_page: bool,
    /// The pager owns the vertical axis (single-page view, or the rest of a gesture that turned the page).
    paging_owns_vertical: bool,
}

/// Cancels older motion before the pager reads new input, without discarding that input.
fn reset_interrupted_wheel(ui: &egui::Ui, view: &mut DocView, other_motion: bool) {
    let frame = ui.ctx().cumulative_frame_nr();
    let resumed = view.wheel_frame.is_some_and(|last| last != frame && last.checked_add(1) != Some(frame));
    if resumed || glide_interrupted(ui) || other_motion {
        view.reset_coarse_state();
    }
}

/// Integrates the whole coarse-wheel distance once, independently of egui's wheel easing.
/// Precise input remains direct, and interrupts clear retained motion before accepting new input.
fn read_wheel(ui: &mut egui::Ui, view: &mut DocView, avail: Rect, single_paging: bool, edge_turned: bool, enabled: bool, animate: bool) -> Wheel {
    let now = ui.input(|i| i.time);
    let hovered = ui.rect_contains_pointer(avail);
    let frame = ui.ctx().cumulative_frame_nr();
    let repeated = view.wheel_frame == Some(frame);
    view.wheel_frame = Some(frame);
    // ScrollArea must not add egui's independently eased copy to the viewer's own motion.
    // Keep draining that copy on later frames and discarded passes too.
    if hovered {
        ui.input_mut(|i| i.smooth_scroll_delta = Vec2::ZERO);
    }
    let paging_owns_vertical = single_paging || (!view.continuous && view.wheel.gesture_turned());
    if repeated || !hovered || !enabled {
        if !hovered || !enabled {
            view.reset_coarse_state();
        }
        return Wheel { precise_scroll: Vec2::ZERO, line_base: Vec2::ZERO, coarse_scroll: Vec2::ZERO, has_page: false, paging_owns_vertical };
    }
    // A touch gesture zooms to its end once it began with ⌘ or Ctrl, the keys let go or not.
    let events = crate::motion::gesture_wheel_events(ui.ctx());
    let consumed = events.iter().any(|event| {
        matches!(event, egui::Event::MouseWheel { delta, modifiers, .. } if *delta != Vec2::ZERO && delta.is_finite() && !modifiers.command && !modifiers.ctrl)
    });
    if consumed {
        wheel_reset::consumed(ui.ctx());
    }
    let input = {
        // Match the line distance used by egui's other scroll areas.
        let live = ui.ctx().options(|o| o.input_options.line_scroll_speed);
        let line_points = if live.is_finite() && live > 0.0 { live } else { crate::scroll_speed::EGUI_LINE_POINTS };
        crate::wheel_boost::accumulate(&events, line_points, single_paging)
    };
    let mut precise_scroll = input.precise;
    let mut line_base = input.line;
    if precise_scroll.x != 0.0 {
        view.wheel_boost_x.reset();
    }
    if precise_scroll.y != 0.0 {
        view.wheel_boost_y.reset();
    }
    let mut coarse_input = line_base + crate::wheel_boost::boost_extra(input, &mut view.wheel_boost_x, &mut view.wheel_boost_y, now);
    let has_page = input.page != Vec2::ZERO;
    if edge_turned {
        view.fling.stop();
    }
    let page_scale = ui.input(|i| i.viewport_rect().height());
    if page_scale.is_finite() && page_scale > 0.0 {
        coarse_input += input.page * page_scale;
    }
    // Paging owns the vertical axis in single-page view; leave it to the pager.
    // The rest of a gesture that turned the page is swallowed too, or it would pan the new page
    // away from the edge it was shown at.
    if paging_owns_vertical {
        precise_scroll.y = 0.0;
        line_base.y = 0.0;
        coarse_input.y = 0.0;
        view.wheel_motion.cancel_axes(vec2(0.0, 1.0));
    }
    view.wheel_motion.cancel_axes(precise_scroll);
    let coarse_scroll = view.wheel_motion.step(coarse_input, now, ui.input(|i| i.stable_dt), animate);
    if view.wheel_motion.has_pending() {
        ui.ctx().request_repaint();
    }
    Wheel { precise_scroll, line_base, coarse_scroll, has_page, paging_owns_vertical }
}

/// Rows of pages that continuous view prefetches past the window in the direction it last
/// moved.
const PREFETCH_ROWS: usize = 2;

/// The pages continuous view prefetches past the window: the next [`PREFETCH_ROWS`] rows the way
/// it last moved, nearest first, and none before it moves. The web build renders on the UI
/// thread, so it prefetches none.
fn scroll_prefetch(view: &DocView, visible_now: &[usize]) -> Vec<usize> {
    if !view.continuous || cfg!(target_arch = "wasm32") {
        return Vec::new();
    }
    let n = PREFETCH_ROWS * if view.layout == PageLayout::Single { 1 } else { 2 };
    // At high zoom the other page of a two-page row can be off to the side and not on screen, so
    // the rows are counted from the edges of the rows shown, and that page is prefetched too.
    match (view.scroll_dir, visible_now.iter().min(), visible_now.iter().max()) {
        (1, _, Some(&last)) => (last.saturating_add(1)..=row_bounds(view, last).1.saturating_add(n)).collect(),
        (-1, Some(&first), _) => (row_bounds(view, first).0.saturating_sub(n)..first).rev().collect(),
        _ => Vec::new(),
    }
}

/// The first and last page of the row that page `i` is in, paired as [`shown_row`] pairs them.
fn row_bounds(view: &DocView, i: usize) -> (usize, usize) {
    match view.layout {
        PageLayout::Single => (i, i),
        PageLayout::TwoUp if view.cover && i == 0 => (0, 0),
        PageLayout::TwoUp => {
            let first = view.spread_first(i);
            (first, first.saturating_add(1))
        }
    }
}

/// The whole-page raster a prefetch asks for: the page at this scale, or the backdrop of a page
/// that is tiled at this scale. A cached raster sharper than the backdrop stays demanded as it
/// is, as `paint_page_surface` demands it under tiles.
fn prefetch_demand(view: &DocView, p: &pdfcraft_render::PageInfo, i: usize, scale: f32, tag: u64, max_side: f32) -> (usize, f32, u64, Option<Tile>) {
    let side = p.width.max(p.height).max(1.0);
    if side * scale <= TILE_THRESHOLD.min(max_side) {
        return (i, scale, tag, None);
    }
    let bs = BASE_SIDE.min(max_side) / side;
    match view.pages.get(&i).filter(|c| c.tag != STALE_TAG && tag_scale(c.tag) >= bs) {
        Some(c) => (i, tag_scale(c.tag), c.tag, None),
        None => (i, bs, scale_tag(bs), None),
    }
}

/// Whether `page` lies behind the way continuous view last moved: above the current page while
/// scrolling down, below it while scrolling up. `false` outside continuous view.
fn behind_scroll(continuous: bool, scroll_dir: i8, current: usize, page: usize) -> bool {
    continuous && ((scroll_dir > 0 && page < current) || (scroll_dir < 0 && page > current))
}

/// Orders a frame's demand: pages on screen first, then off-screen pages ahead of the way the
/// view last moved, then those behind it, each nearest the current page first.
fn order_demand(view: &DocView, wanted: &mut [(usize, f32, u64, Option<Tile>)], visible_now: &[usize]) {
    let cur = view.current;
    wanted.sort_by_key(|w| {
        let shown = visible_now.contains(&w.0);
        (!shown, !shown && behind_scroll(view.continuous, view.scroll_dir, cur, w.0), w.0.abs_diff(cur), w.3.is_some())
    });
}

/// What decides which renders a frame asks for.
#[derive(Clone, Copy)]
struct RenderPlan<'a> {
    visible_pages: &'a [usize],
    tag: u64,
    scale: f32,
    sharp_held: bool,
    pan_held: bool,
    hand: bool,
}

/// Queues the renders for the pages and tiles `renders` wants (nearest the view first), prefetches the
/// rows either side (in continuous view, the rows ahead in the scroll direction), frees tiles of far
/// pages and old zooms, and asks for the text layers.
fn plan_renders(ui: &egui::Ui, view: &mut DocView, info: &DocInfo, renders: PageRenders, plan: &RenderPlan<'_>) {
    let (mut wanted, visible_now) = renders;
    let RenderPlan { visible_pages, tag, scale, sharp_held, pan_held, hand } = *plan;
    // Single-page and two-page views draw only the current row, so the next one was first asked
    // for on the turn and showed blank paper until it rendered. Prefetch the rows either side,
    // as continuous scrolling does for the pages just off screen.
    if !view.continuous
        && let (Some(&first), Some(&last)) = (visible_pages.first(), visible_pages.last())
    {
        let n = if view.layout == PageLayout::Single { 1 } else { 2 };
        let max_side = ui.ctx().input(|inp| inp.max_texture_side) as f32;
        for i in (last.saturating_add(1)..=last.saturating_add(n)).chain(first.saturating_sub(n)..first) {
            let Some(p) = info.pages.get(i).filter(|_| !view.errors.contains_key(&i)) else { continue };
            // As drawn: the whole page at this scale, or the backdrop of a tiled page.
            wanted.push(prefetch_demand(view, p, i, scale, tag, max_side));
        }
    }
    // Continuous view also asks for the rows ahead of the way it last moved.
    let max_side = ui.ctx().input(|inp| inp.max_texture_side) as f32;
    for i in scroll_prefetch(view, &visible_now) {
        if view.errors.contains_key(&i) || wanted.iter().any(|w| w.0 == i) {
            continue;
        }
        let Some(p) = info.pages.get(i) else { continue };
        wanted.push(prefetch_demand(view, p, i, scale, tag, max_side));
    }
    let cur = view.current;
    order_demand(view, &mut wanted, &visible_now);
    // Tiles of far-away pages are useless: free them. Tiles of an earlier zoom stay for a page
    // that is on screen, whether or not the page is tiled at this zoom (#2196): a zoom out below
    // the tiling threshold used to drop the tiles of the zoom just left, so coming back to it
    // was a cold miss and the page re-blurred through the backdrop while every tile was
    // interpreted again. `OLD_TILES` still bounds how many older tiles are kept, nearest scale
    // first, and admission keeps them inside `TILE_BYTES`.
    let missing: HashSet<usize> = wanted.iter().filter(|w| w.3.is_some()).map(|w| w.0).collect();
    let on_screen: HashSet<usize> = visible_now.iter().copied().collect();
    view.tiles.retain(|(p, t, _, _), _| p.abs_diff(cur) <= 2 && (*t == tag || missing.contains(p) || on_screen.contains(p)));
    let mut older: Vec<(f32, (usize, u64, u32, u32))> =
        view.tiles.keys().filter(|k| k.1 != tag).map(|k| ((tag_scale(k.1) / scale).ln().abs(), *k)).collect();
    // Tiles of earlier zooms are kept nearest scale first, and bounded by count *and* bytes: a
    // large scan renders a tile up to 4096² (about 67 MiB), so a count alone let a few tiles of
    // a few zooms run far past the tile allowance. #2196 keeps a zoom already visited, which
    // makes that reachable in ordinary zooming, so the byte bound has to be here too.
    older.sort_by(|a, b| a.0.total_cmp(&b.0));
    let mut kept = 0usize;
    let mut kept_bytes = 0usize;
    let mut drop: Vec<(usize, u64, u32, u32)> = Vec::new();
    for (_dist, key) in older {
        let bytes = view.tiles.get(&key).map_or(0, texture_bytes);
        if kept >= OLD_TILES || kept_bytes.saturating_add(bytes) > TILE_BYTES {
            drop.push(key);
        } else {
            kept = kept.saturating_add(1);
            kept_bytes = kept_bytes.saturating_add(bytes);
        }
    }
    for key in drop {
        view.tiles.remove(&key);
    }
    let mut queue: Vec<RenderRequest> = wanted
        .iter()
        .map(|&(page, wanted_scale, wanted_tag, tile)| RenderRequest {
            page,
            kind: RequestKind::Pixels,
            tile,
            scale: wanted_scale,
            tag: wanted_tag,
            // The low-res backdrop under tiles is a preview: tiny images are skipped for it.
            // It is the whole-page request whose scale is not the current one; sharp tiles
            // and whole-page finals always draw everything.
            preview: tile.is_none() && (wanted_scale != scale || wanted_tag != tag),
        })
        .collect();
    // While the zoom or pan is settling, hold back sharp re-renders of pages that already
    // show something (a stale whole-page raster or older tiles, drawn stretched): each new
    // scale would cancel the running full-page interpret and restart it from scratch, and
    // each pan frame would queue a storm of full-page tile interprets.
    // Pages with nothing on screen yet still queue, so panning to a fresh page draws it.
    if sharp_held || pan_held {
        let main_tag = tag;
        queue.retain(|req| {
            // Fast pages render straight through: the hold only protects heavy pages
            // whose full-page interpret costs seconds.
            if !view.slow_pages.contains(&req.page) {
                return true;
            }
            if req.tile.is_some() {
                let has_current_tile = view.tiles.keys().any(|(p, t, _, _)| *p == req.page && *t == req.tag);
                if has_current_tile {
                    return false;
                }
                let has_any_surface = view.pages.contains_key(&req.page) || view.tiles.keys().any(|(p, _, _, _)| *p == req.page);
                !has_any_surface
            } else if req.tag != main_tag {
                // The low-res backdrop under tiles is zoom-independent: let it through,
                // it is the stretched surface the hold relies on.
                true
            } else {
                // The stale raster stays stretched until the zoom or pan settles; a page with
                // nothing on screen yet still renders.
                !view.pages.contains_key(&req.page)
            }
        });
    }
    // Text layers: visible pages for selection, every page while a search is active. (Select All
    // of the document needs no more: shown pages highlight as their text comes, and Copy reads
    // the rest, as text far away is let go to bound memory.)
    let need_text = |p: &usize| !view.texts.contains_key(p) && !view.text_failed.contains(p);
    let mut text_pages: Vec<usize> = if hand { Vec::new() } else { visible_now.iter().copied().filter(need_text).collect() };
    // A selection across pages copies and marks every page it covers, scrolled past or not.
    if let Some(s) = view.selection {
        let rest: Vec<usize> = s.pages().filter(|p| *p < info.pages.len()).filter(need_text).filter(|p| !text_pages.contains(p)).collect();
        text_pages.extend(rest);
    }
    if view.find.as_ref().is_some_and(|f| !f.case_query.trim().is_empty()) {
        let n = info.pages.len();
        let rest: Vec<usize> = (0..n).map(|k| (cur + k) % n).filter(need_text).filter(|p| !text_pages.contains(p)).collect();
        text_pages.extend(rest);
    }
    queue.extend(text_pages.into_iter().map(|page| RenderRequest {
        page,
        kind: RequestKind::Text,
        tile: None,
        scale: 1.0,
        tag: TEXT_TAG,
        preview: false,
    }));
    view.frame_queue = queue;
    view.frame_visible = visible_now.iter().copied().collect();
>>>>>>> upstream/main
}

/// Acts on the notice bar button that was clicked.
fn apply_notice(app: &mut PdfCraftApp, ctx: &egui::Context, index: usize, notice: Option<Notice>) {
    match notice {
        #[cfg(not(target_arch = "wasm32"))]
        Some(Notice::Reload) => app.reload_request = app.views.get(index).map(|view| view.id),
        #[cfg(target_arch = "wasm32")]
        Some(Notice::Reload) => {}
        Some(Notice::Repairs) => app.open_properties(ctx, crate::PropsTab::Advanced),
        Some(Notice::Security) => app.open_properties(ctx, crate::PropsTab::Security),
        Some(Notice::Unlock) => {
            if let Some(doc) = app.views.get(index).and_then(|view| app.session.get(view.id)) {
                app.unlock_prompt = Some(crate::UnlockPrompt::new(doc, None));
            }
        }
        // Flags the view: the tab closes at the top of the next frame, where no document
        // borrow is held.
        Some(Notice::Close) => {
            if let Some(view) = app.views.get_mut(index) {
                view.close_requested = true;
            }
        }
        Some(Notice::Signatures) => crate::side_panels::set_panel_visible(app, RightPanel::Signatures.into(), true),
        Some(Notice::FieldHighlights(on)) => app.highlight_fields = on,
        Some(Notice::Console) => crate::js_dialogs::open_console(app),
        // Back on in Preferences ▸ General.
        Some(Notice::HideSecured) => app.show_secured_notice = false,
        // Handled by the app with the other view actions (`ViewAction`).
        Some(Notice::LockSignatures) => {
            if let Some(view) = app.views.get_mut(index) {
                view.pending_action = Some(ViewAction::LockSignatures);
            }
        }
        None => {}
    }
}

/// The snapshot notice bar's Close was clicked: close here, where no document borrow is held. The
/// tab is looked up by its view key (indices may have shifted since the click, and split view can
/// show one document in two tabs), and the flag rides its own view, so a stale flag can never
/// close the wrong tab. Whether a tab was closed (or asked about).
fn close_requested_tab(ctx: &egui::Context, app: &mut PdfCraftApp, index: usize) -> bool {
    let pending = app.views.get(index).filter(|v| v.close_requested).map(|v| v.view_key()).and_then(|key| app.tab_with_key(key));
    let Some(i) = pending else { return false };
    app.views[i].close_requested = false;
    app.request_close_tab(ctx, i);
    true
}

<<<<<<< HEAD
=======
/// Where the pages are laid out, for placing the scroll area.
#[derive(Clone, Copy)]
struct ScrollGeometry<'a> {
    rects: &'a [Rect],
    y_shift: f32,
    content_w: f32,
    content_h: f32,
    avail: Rect,
    middle_gesture: bool,
    hand: bool,
}

/// The page view's scroll area, with the offset this frame's jumps, drags and zoom anchor ask for.
fn page_scroll_area(ui: &egui::Ui, view: &mut DocView, geometry: &ScrollGeometry<'_>) -> egui::ScrollArea {
    let ScrollGeometry { rects, y_shift, content_w, content_h, avail, middle_gesture, hand } = *geometry;
    let mut scroll = egui::ScrollArea::both().auto_shrink([false, false]).scroll_source(egui::scroll_area::ScrollSource {
        drag: if middle_gesture {
            egui::scroll_area::DragScroll::Never
        } else if hand {
            egui::scroll_area::DragScroll::Always
        } else {
            egui::scroll_area::DragScroll::OnTouch
        },
        ..Default::default()
    });
    // Each tab keeps its own scroll position when switching tabs (#189), and so does each view of
    // one document in split view.
    scroll = scroll.id_salt(("page-view", view.view_key()));
    // Every scroll here asks for no animation. An animated area would still queue a zero-length
    // offset target for the next frame, which overwrites the jump a page turn just set.
    scroll = scroll.animated(false);
    // A document that hides the window's interface (`HideWindowUI`) hides the scroll bars too;
    // the wheel, keys and Hand tool still scroll.
    if view.interface.hidden.window_ui {
        scroll = scroll.scroll_bar_visibility(egui::scroll_area::ScrollBarVisibility::AlwaysHidden);
    }
    // A newly opened document starts at the top. eframe saves egui's memory with the settings
    // and document ids start again at 1 every launch, so the offset stored under this id can be
    // where an earlier session left another document.
    if !std::mem::replace(&mut view.shown, true) {
        scroll = scroll.scroll_offset(Vec2::ZERO);
    }
    // The Hand tool pans here, before the pages are laid out, so the page moves with the pointer
    // in the frame the pointer moves (#924). Scrolling by the drag from inside the area moved the
    // page only after that frame was drawn, a frame behind. The frame egui tells the drag from a
    // click, the page catches up with all the pointer has moved since the press. A jump asked for
    // below still wins.
    if hand
        && !middle_gesture
        && let Some(id) = view.page_widget.filter(|id| ui.ctx().is_being_dragged(*id))
    {
        let moved = if ui.ctx().drag_started_id() == Some(id) {
            ui.input(|i| i.pointer.total_drag_delta()).unwrap_or(Vec2::ZERO)
        } else {
            ui.input(|i| i.pointer.delta())
        };
        if moved != Vec2::ZERO {
            view.end_find_following();
        }
        // Within the content, as egui keeps it: past an end the page would show there for a
        // frame and spring back. `max` is never NaN or below zero, so the clamp can't panic.
        let max = vec2((content_w - avail.width()).max(0.0), (content_h - avail.height()).max(0.0));
        scroll = scroll.scroll_offset((view.last_scroll - moved).clamp(Vec2::ZERO, max));
    }
    if let Some(v) = view.sync_scroll.take() {
        scroll = scroll.scroll_offset(v);
    }
    if let Some((page, fx, fy, rel)) = view.zoom_anchor.take() {
        let r = rects[page.min(rects.len() - 1)];
        let point = pos2(r.left() + fx * r.width(), r.top() - y_shift + fy * r.height());
        scroll = scroll.scroll_offset(anchored_scroll(point, rel, vec2(content_w, content_h), avail.size()));
    } else if let Some((page, frac)) = view.goto.take() {
        let side = view.goto_side.take();
        let page = page.min(rects.len() - 1);
        let r = rects[page];
        // Clamped here: egui clamps only after drawing, so a page that fits was drawn MARGIN - page_gap()
        // too high for a frame and then hopped into place. (`avail` is the viewport, as the
        // scroll bars float.)
        let max_y = (content_h - avail.height()).max(0.0);
        scroll = scroll.vertical_scroll_offset((r.top() - y_shift - view.page_gap() + frac * r.height()).max(0.0).min(max_y));
        // Horizontally too, as Acrobat does (#2665): beside a much wider page the others sit off
        // to one side. A page already shown whole, or one filling the window's width, keeps the
        // scroll, so paging through a zoomed-in document stays on the same column. Otherwise a
        // page that fits is centred, and a wider one scrolled just far enough to fill the window.
        // The page counts with its side gutters, which keep its edge clear of the floating
        // quick-action bar as the layout does.
        let (x, w) = (view.last_scroll.x, avail.width());
        let gutter = view.side();
        let (left, right) = (r.left() - gutter, r.right() + gutter);
        // Turned by → / ← at an edge (`goto_side`): a page or spread wider than the window starts at
        // the edge the reader comes from.
        let row = shown_row(view, rects).iter().filter_map(|&i| rects.get(i)).copied().reduce(Rect::union).unwrap_or(r);
        let (row_left, row_right) = (row.left() - gutter, row.right() + gutter);
        let target = match side {
            Some(from_right) if row_right - row_left > w => {
                if from_right {
                    row_right - w
                } else {
                    row_left
                }
            }
            _ if right - left <= w => {
                if left >= x && right <= x + w {
                    x
                } else {
                    r.center().x - w / 2.0
                }
            }
            _ => x.clamp(left, right - w),
        };
        if (target - x).abs() > 0.5 {
            scroll = scroll.horizontal_scroll_offset(target.clamp(0.0, (content_w - w).max(0.0)));
        }
    }
    scroll
}

/// Notes how far and which way the page view scrolled this frame: reader-owned scrolling ends
/// search following, a pan holds back fresh tile renders until it settles, a glide keeps going or
/// stops, and the vertical direction is kept for continuous view's prefetch. Whether a pan is
/// still settling.
fn track_scroll<T>(ui: &egui::Ui, view: &mut DocView, out: &egui::containers::scroll_area::ScrollAreaOutput<T>, glide: Vec2) -> bool {
    let scroll_offset = out.state.clamped_offset();
    // The document ScrollArea's own scroll surfaces move the offset without the wheel deltas
    // above: its two scroll-bar tracks/handles and, for touch or the Hand tool, its drag
    // background. The output gives us the area id, and egui's pinned ScrollArea registers those
    // widgets under `id.with(0)` (horizontal), `id.with(1)` (vertical) and `id.with("area")`.
    // Ask for their exact responses instead of treating any held drag as scrolling: a text
    // selection, a panel splitter or another widget's drag must not excuse the programmatic jump
    // a search flash makes, while a track click (which moves the offset with no pointer motion)
    // must still count as the reader scrolling.
    let scroll_surface = |id: egui::Id| ui.ctx().read_response(id).is_some_and(|r| r.is_pointer_button_down_on() || r.dragged() || r.clicked());
    let pointer_scrolling = scroll_surface(out.id.with(0usize)) || scroll_surface(out.id.with(1usize)) || scroll_surface(out.id.with("area"));
    if pointer_scrolling && (scroll_offset - view.last_scroll).length() > 2.0 {
        view.end_find_following();
    }
    let now_settle = ui.input(|i| i.time);
    if (scroll_offset - view.last_scroll).length() > 2.0 {
        view.pan_hold_until = now_settle + 0.25;
        ui.ctx().request_repaint_after(std::time::Duration::from_millis(280));
    }
    if glide != Vec2::ZERO {
        view.fling.moved((scroll_offset - view.last_scroll).length() > 0.01);
    }
    view.scroll_dir = scroll_direction(view.scroll_dir, scroll_offset.y - view.last_scroll.y, out.content_size != view.scroll_content);
    view.scroll_content = out.content_size;
    view.last_scroll = scroll_offset;
    now_settle < view.pan_hold_until
}

/// The way the view moves after a frame whose offset changed by `dy` points: down (1) or up (-1)
/// for a move of more than half a point, `dir` otherwise, and 0 when the content's size changed
/// (a zoom or a layout change).
fn scroll_direction(dir: i8, dy: f32, resized: bool) -> i8 {
    if resized {
        0
    } else if dy > 0.5 {
        1
    } else if dy < -0.5 {
        -1
    } else {
        dir
    }
}

/// The hover tip, find bar, size hint and the in-place editors drawn over the pages. Whether the
/// text editor was finished.
fn frame_overlays(ui: &mut egui::Ui, view: &mut DocView, env: &PageEnv<'_>, primary_down: bool, hover_text: Option<(Pos2, String)>) -> bool {
    let PageEnv {
        added, allowed, author, avail, can_modify, comments_hidden, doc, form, info, keys, prefs, t, today, tool, units, unobstructed, ..
    } = *env;
    if let Some((pos, text)) = hover_text {
        // Click-through, as a tooltip is: the pages lose the pointer under anything that isn't.
        let tip = egui::Area::new(egui::Id::unique("canvas-hover")).order(egui::Order::Tooltip).interactable(false);
        tip.fixed_pos(pos + vec2(14.0, 16.0)).show(ui.ctx(), |ui| {
            egui::Frame::popup(ui.style()).show(ui, |ui| {
                ui.set_max_width(320.0);
                ui.label(text);
            });
        });
    }
    find_bar(view, info.pages.len(), avail, tool == QuickTool::EditText && can_modify, ui, t);
    page_size_hint(view, info, units, avail, unobstructed, ui, t);
    if tool == QuickTool::EnhanceCamera {
        crate::camera_ui::bar(ui.ctx(), view, avail, t, can_modify);
    }
    if let Some(e) = comments::composer(ui.ctx(), keys, view, info, prefs) {
        view.pending_edit = Some(e);
    }
    if let Some(e) = comments::popups(ui.ctx(), view, info, prefs, comments_hidden, allowed) {
        // A click on the composer's Post also takes the focus from a pop-up's box, saving what
        // was typed there: both edits land in this frame, so both are kept, in one batch.
        view.pending_edit = Some(match view.pending_edit.take() {
            Some(first) => Edit::Batch { label: "Edit comment".into(), edits: vec![first, e] },
            None => e,
        });
    }
    if let Some(e) = crate::edit_text_ui::overlay(ui.ctx(), keys, view, info) {
        view.pending_edit = Some(e);
    }
    let text_box = crate::forms_ui::text_box(doc, view, form);
    if let Some(e) = crate::forms_ui::overlay(ui.ctx(), keys, view, info, form, primary_down, text_box.as_ref(), today) {
        view.pending_edit = Some(e);
    }
    if let Some(e) = crate::fill_sign::type_box(ui.ctx(), keys, view, info, author) {
        view.pending_edit = Some(e);
    }
    let (typed, text_done) = crate::content_ui::editor(ui.ctx(), keys, view, info, added);
    if typed.is_some() {
        view.pending_edit = typed;
    }
    text_done
}

/// Full screen: the click turns the page unless it did something else first. It waits out the
/// double-click time: turning on the first click's release sent a double-click's second click,
/// meant to select a word, to the next page (review on #2070). A selection cancels it, and so do a
/// click on a link, field or comment (`clicked_object`) and leaving full screen: the pointer stays
/// active "so that you can click links and open notes" (*Adobe Acrobat Help*, 2019, p. 65), and
/// fields to fill. A double-click elsewhere still turns once.
/// `click` is `Some(back)` on the frame of a click, `back` when it was a shift-click.
fn schedule_presentation_turn(ui: &egui::Ui, view: &mut DocView, click: Option<bool>, clicked_object: bool) {
    let now = ui.input(|i| i.time);
    if let Some(back) = click
        && !clicked_object
        && view.selection.is_none()
        && view.clicks < 2
    {
        let delay = ui.ctx().options(|o| o.input_options.max_double_click_delay);
        view.present_turn = Some((now + delay, !back));
    }
    if let Some((due, forward)) = view.present_turn {
        if clicked_object || view.selection.is_some() || view.presentation.is_none() {
            view.present_turn = None;
        } else if now >= due {
            view.present_turn = None;
            view.step_page(forward);
        } else {
            ui.ctx().request_repaint_after(std::time::Duration::from_secs_f64((due - now).max(0.0)));
        }
    }
}

>>>>>>> upstream/main
/// Follows the link, or the bookmark's action, that was clicked. A link to another file comes
/// back, to be opened once the document is no longer borrowed.
pub(crate) fn follow_link(
    app: &mut PdfCraftApp,
    index: usize,
    target: Option<LinkTarget>,
) -> Option<(pdfcraft_engine::DocId, String, pdfcraft_render::RemoteDestination)> {
    let view_id = app.views[index].id;
    match target? {
        LinkTarget::Page(p, dest) => {
            let view = &mut app.views[index];
            if let Some(doc) = app.session.get(view.id) {
                view.go_to_dest(p, dest, &doc.info);
            }
        }
        LinkTarget::Uri(u) => app.request_document_url(&u, crate::LinkOrigin::Link),
        LinkTarget::SetLayers { changes, preserve_rb } => set_layers(app, index, &changes, preserve_rb),
        LinkTarget::RemoteGoTo { file, destination, .. } => return Some((view_id, file, destination)),
        // Run by `PdfCraftApp::run_link_script`, which needs the link's page and index.
        LinkTarget::JavaScript(_) => {}
        LinkTarget::Other(s) => app.notify_fmt("This PDF uses the unsupported {s} action.", &[("s", &s)]),
    }
    None
}

/// A Snapshot area's bar choice, a signature rectangle drawn, or an empty signature field clicked.
/// Returns the tool to go on with.
fn finish_sign_requests(
    app: &mut PdfCraftApp,
    ctx: &egui::Context,
    index: usize,
    mut tool: QuickTool,
    area_choice: Option<crate::zoom_snap::AreaChoice>,
) -> QuickTool {
    // The copied area and its bar belong to Snapshot: another tool puts them away. The tool is
    // the app's, not the tab's, so every tab's area goes, not only the one on screen.
    if tool != QuickTool::Snapshot {
        for v in &mut app.views {
            v.session.area_pick = None;
        }
    }
    if let Some(choice) = area_choice
        && let Some((page, area)) = app.views[index].session.area_pick
    {
        match choice {
            crate::zoom_snap::AreaChoice::Print => app.open_print_area(page, area),
            crate::zoom_snap::AreaChoice::Save => app.save_snapshot_image(ctx, index, page, area),
        }
    }
    // A signature rectangle was drawn, or an empty signature field clicked.
    let drawn = app.views[index].session.sign.drawn.take();
    if let (Some((page, rect)), QuickTool::SignArea { certify }) = (drawn, tool) {
        tool = QuickTool::Select;
        app.quick_tool = tool;
        app.start_signing(ctx, page, Some(rect), None, certify.then_some(2));
    }
    if let Some(field) = app.views[index].session.sign.field.take() {
        let signed = app.session.get(app.views[index].id).is_some_and(|d| d.signatures.iter().any(|s| s.field == field && s.signed));
        if signed {
            app.sig_open.set(&field, true);
            crate::side_panels::set_panel_visible(app, RightPanel::Signatures.into(), true);
        } else {
            let page = app.views[index].view.current;
            app.start_signing(ctx, page, None, Some(field), None);
        }
    }
    tool
}

/// What the page body reads besides the session, settled before the page view is shown: the document, the tool and
/// the frame's facts. One per pass; the body never changes it.
struct PageBodyCx<'a> {
    doc: &'a pdfcraft_engine::Document,
    info: &'a DocInfo,
    revisions: &'a PageRevisions,
    /// The tool the page sees ([`page_tool`] keeps the rule that copied comments on the pointer make it Select).
    tool: QuickTool,
    /// The tool the user picked, before that rule.
    chosen: QuickTool,
    /// The view's rotation this pass: the page view holds the view state while the body runs.
    rotation: u16,
    /// The view's current page this pass.
    current: usize,
    /// The scale the pages render at this pass, which the dragged comment's layers render at too.
    render_scale: f32,
    /// Full screen: the page, display and rotation it shows, captured before the page view holds the view state;
    /// `None` outside full screen.
    present_at: Option<presenter::PresentAt>,
    /// The incoming page cannot take input until its transition finishes.
    veiled: bool,
    preparing: bool,
    calc_order: bool,
    editing_content: bool,
    allowed: bool,
    can_modify: bool,
    can_copy: bool,
    comments_hidden: bool,
    text_links: bool,
    /// No dialog, close prompt or palette over the pages.
    unobstructed: bool,
    /// Every page's size in view space, unturned by the view (`DocView::page_sizes`).
    sizes: &'a [(f32, f32)],
    /// The view's key ([`DocView::view_key`]), which keys what is drawn over the pages.
    view_key: u64,
    /// What the page tool reads besides the session; its `pages` is filled from the body each run.
    tools: page_tools::PageToolCx<'a>,
    /// Animation effects apply (the preference after Reduce Motion), for full screen's transitions.
    animates: bool,
    prefs: &'a comments::CommentPrefs,
    /// The user's Redaction Tool Properties hover colour and opacity, a data colour, not a theme role (L-90).
    redact_hover: ([f64; 3], f64),
    form: &'a Arc<Vec<pdfcraft_engine::FormField>>,
    today: (i64, u32, u32),
    units: crate::units::Unit,
    find_marks: &'a [TextMark],
    table_preview: Option<&'a crate::field_table_ui::Preview>,
}

impl PageBodyCx<'_> {
    /// The page sizes and rotation the body's pages are placed with, and the body's rect as the document area.
    fn geometry(&self, body: &PageBody<'_>) -> PageGeometry<'_> {
        PageGeometry { sizes: self.sizes, rotation: self.rotation, viewport: body.rect }
    }
}

/// What a run of the page body reports for the rest of the pass. The body runs twice in a pass (a measuring run with
/// no tool pass, then the real one): each run starts from the default, so the real run's report is the one read.
#[derive(Default)]
struct BodyOut {
    /// The canvas view the page view showed this pass.
    shown: Option<CanvasView>,
    /// The Crop tool's double-click: open Set Page Boxes on that page.
    open_boxes: bool,
    /// Fill & Sign asked for a signature (`false`) or initials (`true`) before it can place one.
    open_signature: Option<bool>,
    /// Where the canvas menu opens: a secondary press the tool did not take, or the Menu key.
    context: Option<ContextRequest>,
    /// The link clicked, and the JavaScript link by page and annotation index (0-based).
    link: Option<(LinkTarget, Option<(usize, usize)>)>,
    /// The running tool finished and the app goes back to the Select tool (a stamp placed, a crop committed, Add
    /// text done, a field placed).
    back_to_select: bool,
    /// A Snapshot area drawn: (page, view-space rect).
    snapshot: Option<(usize, [f32; 4])>,
    /// What the prepare tool asked for this pass.
    prepare: Option<crate::prepare::PrepareOutcome>,
    /// Full screen: the page or spread to turn to, forward when `true`.
    turn: Option<bool>,
    /// The Snapshot bar's choice.
    area_choice: Option<crate::zoom_snap::AreaChoice>,
    /// The selection bar's Copy.
    copy_selected: bool,
    /// What the Camera image controls asked the app for.
    action: Option<ViewAction>,
    /// The comment under the pointer (page, index): its pop-up's connector shows while it is.
    hovered_comment: Option<(usize, usize)>,
    /// A part of the page (a link, a field, a comment…) is under the pointer: in full screen a click there is the
    /// part's and turns no page.
    over_part: bool,
    /// The page a primary press started on in this pass, so clicking a page in continuous view makes it current.
    pressed_page: Option<usize>,
}

/// The part list of a pass, in paint order: pictures, paragraphs and added items, links and text links (only under
/// Select and Link), comments and fields (only under Select), sign tabs (painted over their fields), then prepare fields,
/// selected objects and the frame band. A later part outranks an earlier one of the same layer, so this order is the
/// stacking order hit testing ranks on.
fn page_parts_of(ui: &mut egui::Ui, body: &PageBody<'_>, session: &mut PageSession, cx: &PageBodyCx<'_>) -> Vec<PagePart> {
    let geo = cx.geometry(body);
    let mut parts = Vec::new();
    // Copied comments hanging on the pointer take the next click anywhere on a page, even over an
    // existing comment, link or field, so no page part may steal it from the paste tool.
    if cx.veiled || session.comments.paste.is_some() {
        return parts;
    }
    if cx.editing_content {
        if cx.tool == QuickTool::EditText && cx.can_modify {
            for placed in body.pages {
                parts.extend(crate::edit_text_ui::picture_parts(session, cx.doc, placed.page, placed, geo));
            }
        }
        // A picture being cropped holds every press on its page: its paragraphs and added text take none there.
        let cropping = session.image_selection.as_ref().filter(|s| s.crop.is_some()).map(|s| s.page);
        for placed in body.pages.iter().filter(|placed| Some(placed.page) != cropping) {
            if cx.can_modify && matches!(cx.tool, QuickTool::EditText | QuickTool::AddText) {
                parts.extend(crate::edit_text_ui::page_parts(session, cx.doc, placed.page, placed, geo, cx.tool == QuickTool::AddText));
            }
        }
        for placed in body.pages.iter().filter(|placed| Some(placed.page) != cropping) {
            parts.extend(crate::content_ui::page_parts(session, cx.doc, placed.page, placed));
        }
    }
    if matches!(cx.tool, QuickTool::Select | QuickTool::Link) {
        links_of(session, cx, body).parts(&mut parts);
    }
    if !cx.comments_hidden {
        // The comment a press under the pointer would take, hit within the theme's reach and with its markers at the
        // icon size, both as the tools on the canvas read them.
        let env = Env::of(ui);
        let map = comments::PageMap::new(cx.info, body.pages, cx.rotation, body.view);
        let pointer = ui.input(|i| i.pointer.hover_pos()).map(|p| craft::behavior::kurbo::Point::new(p.x.into(), p.y.into()));
        let (tolerance, icon_side) = (map.doc_len(env.hit_reach()), map.doc_len(env.tokens.size.icon.md));
        let on_link = ui
            .input(|i| i.pointer.hover_pos())
            .is_some_and(|p| parts.iter().any(|part| matches!(part.owner, Owner::Link { .. } | Owner::TextLink { .. }) && part.shape.hit(p)));
        if cx.tool == QuickTool::Select
            || cx.unobstructed && session.comments.tool.in_flight().is_none() && session.comments.tool.drawing().is_empty()
        {
            parts.extend(comments::parts(cx.info, &map, cx.prefs, cx.tool, pointer, &session.comments.tool, tolerance, icon_side, on_link));
        }
    }
    // Fields take the page's presses under Select only: another tool's press over a field is the tool's (a new
    // text box, a mark, a stamp).
    let badge_tips = cx.unobstructed
        && !matches!(
            cx.tool,
            QuickTool::Fill(_)
                | QuickTool::Comment(comments::CommentTool::Ink | comments::CommentTool::FreehandHighlight | comments::CommentTool::Eraser)
        )
        && session.comments.tool.in_flight().is_none()
        && session.comments.tool.drawing().is_empty();
    if cx.tool == QuickTool::Select && !cx.preparing {
        let can_fill = cx.doc.allows_form_filling();
        for placed in body.pages {
            parts.extend(crate::forms_ui::field_parts(session, cx.info, cx.doc, placed.page, can_fill, badge_tips, geo));
        }
    }
    // The sign tabs of each page, after the fields: a tab is painted over its field.
    // PLAN: `can_fill` is `can_modify` (pdf-canvas-w15 does not say which right a click that signs needs).
    let shown = cx.tool == QuickTool::Select && !cx.preparing;
    for placed in body.pages {
        let size = cx.sizes.get(placed.page).copied().unwrap_or_default();
        let xf = PageXform::of(placed, cx.rotation, size);
        let overlay = overlay_parts(ui, &xf, cx.doc, placed.page, cx.can_modify, shown, badge_tips);
        parts.extend(overlay.parts);
    }
    if cx.preparing {
        let field_tool = matches!(cx.tool, QuickTool::Field(_));
        crate::prepare::tool::page_parts(&mut parts, session, cx.form, cx.info, body.pages, cx.rotation, field_tool);
    }
    if cx.tool == QuickTool::EditText && cx.can_modify {
        crate::object_ui::page_parts(&mut parts, session, cx.doc, cx.info, body.pages, geo);
    }
    parts
}

/// The links of the pages: followed under Select, edited under the Link tool.
fn links_of<'a>(session: &'a PageSession, cx: &'a PageBodyCx<'_>, body: &'a PageBody<'_>) -> PageLinks<'a> {
    let mode = if cx.tool == QuickTool::Link { LinkMode::Edit } else { LinkMode::Follow };
    PageLinks::new(mode, session, cx.info, cx.doc.links(), body.pages, cx.rotation, cx.sizes).text_links(cx.text_links).tips(cx.unobstructed)
}

/// Whether `phase` is a phase of a press of `button`: a press and release in one pass report only the release.
fn pressed_by(phase: &PressPhase, button: PointerButton) -> bool {
    phase.start().is_some_and(|start| start.button == button)
}

/// The topmost picture part of `page` under `at` among `parts`, given in paint order.
fn picture_under(parts: &[PagePart], page: usize, at: egui::Pos2) -> Option<usize> {
    parts.iter().rev().find_map(|part| match part.owner {
        Owner::Picture { index } if part.page == page && part.shape.hit(at) => Some(index),
        _ => None,
    })
}

/// What a press on a part does, routed by its owner. `parts` are the pass's parts, in paint order.
fn route_press(
    ui: &egui::Ui,
    session: &mut PageSession,
    press: &PagePress,
    parts: &[PagePart],
    cx: &PageBodyCx<'_>,
    body: &PageBody<'_>,
    out: &mut BodyOut,
) {
    let geo = cx.geometry(body);
    if pressed_by(&press.phase, PointerButton::Secondary) {
        // With the primary button held (a comment being moved, a box drawn) the selections stay as they are.
        let held = ui.input(|i| i.pointer.primary_down());
        let mut owner = press.owner.clone();
        if cx.tool == QuickTool::EditText && cx.can_modify {
            // Paragraphs, added text and an open paragraph's frame have no menu: the picture under them takes the
            // right-click (the user's ruling).
            if matches!(owner, Owner::Paragraph { .. } | Owner::Added { .. } | Owner::Frame { .. })
                && let Some(start) = press.phase.start()
                && let Some(index) = picture_under(parts, press.page, start.origin)
            {
                owner = Owner::Picture { index };
            }
            // A picture is selected for its menu only when the right-click lands on its visible body
            // on the page; a right-click on its out-of-page frame or anything else lets it go.
            let on_page = press
                .phase
                .start()
                .is_some_and(|start| body.pages.iter().find(|p| p.rect.contains(start.origin)).is_some_and(|p| p.page == press.page));
            if !held {
                let picture = match owner {
                    Owner::Picture { index } if on_page => Some(index),
                    _ => None,
                };
                crate::edit_text_ui::picture_context(press.page, picture, session);
            }
            if matches!(owner, Owner::Picture { .. }) && !on_page {
                if let Some(start) = press.phase.start() {
                    background_context(ui, session, cx, body, start.origin);
                }
                return;
            }
        }
        let mut menu_for = Some((press.page, owner.clone()));
        // The menu is anchored on the page the right-click lies on, the first where two pages touch, whichever part
        // took it. A right-click on another part is the comments' too, unless the tool takes it (Add text, the Link
        // tool on a link, a field tool): the comment over the part is selected for its menu, or the selection cleared.
        if let Some(start) = press.phase.start()
            && let Some(page) = body.pages.iter().find(|placed| placed.rect.contains(start.origin)).map(|placed| placed.page)
            && let Some(xf) = session.page_xform(page, geo)
        {
            let page_cx = comments::PageCx { page, xf: &xf, info: cx.info, prefs: cx.prefs, allowed: cx.allowed, hidden: cx.comments_hidden };
            let tool_takes_it = match cx.tool {
                QuickTool::AddText => cx.can_modify,
                QuickTool::Link => cx.can_modify && matches!(press.owner, Owner::Link { .. } | Owner::TextLink { .. }),
                QuickTool::Field(_) => true,
                _ => false,
            };
            // Added text and the objects selected together lie over the comments: a right-click on them
            // reaches no comment under them and leaves the selection cleared.
            let over_comments = matches!(owner, Owner::Added { .. } | Owner::Objects { .. } | Owner::Frame { .. });
            if held || tool_takes_it || matches!(press.owner, Owner::Comment { .. }) {
                comments::context_anchor(&page_cx, &mut session.comments, start.origin);
            } else if over_comments {
                comments::context_anchor(&page_cx, &mut session.comments, start.origin);
                session.comments.select(None);
            } else if let Some(index) = comments::context_target(&page_cx, &mut session.comments, start.origin) {
                menu_for = Some((page, Owner::Comment { index }));
                if cx.tool == QuickTool::EditText {
                    crate::edit_text_ui::picture_context(page, None, session);
                }
            }
        }
        // The page's menu reads what the secondary press went to; a comment is selected for its menu, as a comment's
        // menu acts on the selection.
        if let Owner::Comment { index } = press.owner
            && !session.comments.is_selected((press.page, index))
        {
            session.comments.select(Some((press.page, index)));
        }
        // Add text's menu is the page's.
        session.menu_for = menu_for.filter(|_| !(cx.tool == QuickTool::AddText && cx.can_modify));
        // A Prepare field is selected for its menu, under a field tool too.
        if let Owner::PrepareField { .. } = press.owner
            && cx.can_modify
        {
            crate::prepare::tool::field_press(session, cx.form, press);
        }
        // A link reads its own right-click too: its context event selects it for the Link tool's menu.
        if !matches!(owner, Owner::Link { .. } | Owner::TextLink { .. }) || menu_for != Some((press.page, owner)) {
            return;
        }
    }
    let clicked = matches!(press.phase, PressPhase::Release { click: true, .. });
    // A press a part takes is no picture pick, and takes back an earlier click whose picture is still being found.
    session.pending_pick = None;
    // A press on anything but a comment (a field, added text, a link) leaves the selected comments unselected, as a
    // press on the page does: the part takes it, so the comments tool never sees it.
    let primary = pressed_by(&press.phase, PointerButton::Primary);
    if primary && (matches!(press.phase, PressPhase::Down { .. }) || press.phase.is_click()) && !matches!(press.owner, Owner::Comment { .. }) {
        session.comments.clear_selection();
    }
    match &press.owner {
        Owner::Link { .. } | Owner::TextLink { .. } => {
            // Read before acting: the links borrow the session.
            let event = links_of(session, cx, body).read(press);
            if let Some(event) = event {
                match event {
                    LinkEvent::Follow { target, javascript } => out.link = Some((target, javascript)),
                    LinkEvent::Select { .. } | LinkEvent::Context { .. } => crate::link_ui::link_edit_press(&mut session.links, &event),
                }
            }
        }
        Owner::Comment { .. } => {
            if let Some(xf) = session.page_xform(press.page, geo) {
                let page_cx =
                    comments::PageCx { page: press.page, xf: &xf, info: cx.info, prefs: cx.prefs, allowed: cx.allowed, hidden: cx.comments_hidden };
                let words = Env::of(ui).words::<AnnotateWords>();
                let map = comments::PageMap::new(cx.info, body.pages, cx.rotation, body.view);
                comments::part_pressed(&mut session.comments, press, &page_cx, &map, cx.tool, &words);
            }
        }
        Owner::Field { .. } => crate::forms_ui::field_press(ui.ctx(), session, cx.doc, cx.info, cx.doc.allows_form_filling(), press, geo),
        Owner::SignTab { field } => {
            if clicked && let Some(field) = cx.form.get(*field) {
                session.sign.field = Some(field.name.clone());
            }
        }
        Owner::Badge { .. } => {}
        // PLAN: assumed `prepare::tool::field_press(session, form, press)` (pdf-forms-w9).
        Owner::PrepareField { .. } => crate::prepare::tool::field_press(session, cx.form, press),
        // The Edit text families' presses, a modified press first taking it for the objects selected together.
        Owner::Paragraph { .. } | Owner::Added { .. } | Owner::Picture { .. } | Owner::Objects { .. } | Owner::Frame { .. } => {
            if let Some(edit) = page_tools::edit_text_press(session, cx.doc, cx.info, press, body.pages, geo, cx.tool) {
                session.pending_edit = Some(edit);
            }
        }
    }
}

/// Request the clipboard at a page-space menu point.
fn request_paste_at_page(app: &PdfCraftApp, ctx: &egui::Context, index: usize, page: usize, at: [f64; 2]) {
    let Some(view) = app.views.get(index) else { return };
    let Some(info) = app.session.get(view.id).and_then(|doc| doc.info.pages.get(page)) else { return };
    let Some(xf) = view.page_xform(page) else { return };
    let point = info.user_to_view(at[0] as f32, at[1] as f32);
    craft::ui::runtime::commands::request_paste_at(ctx, xf.norm_to_screen(point[0] / xf.pw, point[1] / xf.ph));
}

/// A right-click no part took, or the Menu key, at `at`. The page's menu is anchored on the page under it, the first
/// where two pages touch, and on none between the pages. With every tool but Add text and the field tools it is the
/// comments': the comment under it is selected for its menu, or the selection cleared, between the pages too. With
/// the primary button held (a comment being moved, a box drawn) the selections stay as they are. Under Edit text it
/// unselects the picture.
fn background_context(ui: &egui::Ui, session: &mut PageSession, cx: &PageBodyCx<'_>, body: &PageBody<'_>, at: Pos2) {
    let held = ui.input(|i| i.pointer.primary_down());
    let tool_takes_it = match cx.tool {
        QuickTool::AddText => cx.can_modify,
        QuickTool::Field(_) => true,
        _ => false,
    };
    if !held && cx.tool == QuickTool::EditText && cx.can_modify {
        crate::edit_text_ui::picture_context(cx.current, None, session);
    }
    let geo = cx.geometry(body);
    let under =
        body.pages.iter().find(|placed| placed.rect.contains(at)).and_then(|placed| Some((placed.page, session.page_xform(placed.page, geo)?)));
    let Some((page, xf)) = under else {
        session.comments.context_at = None;
        session.menu_for = None;
        if !held && !tool_takes_it {
            session.comments.select(None);
        }
        return;
    };
    let page_cx = comments::PageCx { page, xf: &xf, info: cx.info, prefs: cx.prefs, allowed: cx.allowed, hidden: cx.comments_hidden };
    let comment = if tool_takes_it {
        comments::context_anchor(&page_cx, &mut session.comments, at);
        None
    } else if held {
        comments::context_anchor(&page_cx, &mut session.comments, at);
        comments::context_comment(&page_cx, at)
    } else {
        comments::context_target(&page_cx, &mut session.comments, at)
    };
    session.menu_for = comment.map(|index| (page, Owner::Comment { index }));
}

/// The page body, run by the page view on every run of its canvas body. Its order is the paint order, and with it
/// the stacking order hit testing ranks on: the placed pages recorded, the parts registered, the tool host, then the
/// overlays.
struct PageCallbacks<'a, 'b> {
    session: &'a mut PageSession,
    momentary: &'a mut Momentary,
    cx: &'a PageBodyCx<'b>,
    out: &'a mut BodyOut,
    parts: Option<page_parts::PagePartsOutput>,
    field_arrived: bool,
}

fn page_before(ui: &mut egui::Ui, body: &PageBody<'_>, callbacks: &mut PageCallbacks<'_, '_>) -> Option<CanvasView> {
    let PageCallbacks { session, momentary, cx, out, parts: saved_parts, field_arrived: saved_arrived } = callbacks;
    **out = BodyOut::default();
    // 1. This pass's placement, before anything that maps through it.
    session.placed.clear();
    session.placed.extend_from_slice(body.pages);

    // 2-3. Every part on the pages, in one control: hit priority ranks parts inside one control only.
    let parts = page_parts_of(ui, body, session, cx);
    let focus = crate::forms_ui::focus_owner(&session.forms, cx.form);
    // A focused field whose page has scrolled out of view lets go, unless Tab is scrolling to it.
    if !session.forms.refocus
        && session.pending_page.is_none()
        && let Some(Owner::Field { field, widget }) = focus
        && let Some(page) = cx.form.get(field).and_then(|f| f.widgets.get(widget)).and_then(|w| w.page)
        && !body.pages.iter().any(|p| p.page == page)
    {
        session.forms.focus = None;
    }
    let focus = crate::forms_ui::focus_owner(&session.forms, cx.form);
    let parts_out = PageParts::new(&parts).id(page_parts::control_id(cx.view_key)).focus(focus).show_at(ui, body.rect);
    if session.forms.refocus
        && let Some(at) = focus.and_then(|owner| page_parts::focus_stop(&parts, owner))
    {
        session.forms.refocus = false;
        // The editor handed the keys on with Tab: the field the session moved to takes them, as Tab onto its part would.
        focus::request(ui.ctx(), FocusTarget::Part(parts_out.response.id, at));
    }
    out.hovered_comment = match parts_out.hovered {
        Some((Owner::Comment { index }, page)) => Some((page, index)),
        _ => None,
    };
    out.over_part = parts_out.hovered.is_some_and(|(owner, _)| !matches!(owner, Owner::Badge { .. }));
    // The pointer entering or leaving a form widget runs its Mouse Enter and Exit scripts, once per real run.
    if body.pass.is_some() {
        let widget = crate::forms_ui::hovered(cx.form, parts_out.hovered.as_ref().map(|(owner, _)| owner));
        crate::forms_ui::hover_moved(session, widget);
    }
    if let Some(press) = &parts_out.press {
        // Any press on a page ends Select All of the document, on a link too.
        if matches!(press.phase, PressPhase::Down { .. }) {
            session.all_text = false;
        }
        route_press(ui, session, press, &parts, cx, body, out);
    }
    if let Some(at) = parts_out.context {
        background_context(ui, session, cx, body, at);
    }
    // Focus arrived on a field (Tab, a request or a click): the field opens, a text field with its text selected.
    let field_arrived = match &parts_out.focused {
        Some((owner @ Owner::Field { .. }, _)) => {
            crate::forms_ui::field_focused(session, cx.form, owner);
            true
        }
        _ => false,
    };
    if let Some((Owner::Field { .. }, forward)) = &parts_out.tab {
        if let Some(edit) = crate::forms_ui::field_tab(session, cx.info, cx.form, *forward, cx.geometry(body)) {
            session.pending_edit = Some(edit);
        }
    } else if let Some((Owner::Field { .. }, key)) = &parts_out.key
        && let Some(edit) = crate::forms_ui::field_key(session, cx.info, cx.form, key, cx.geometry(body))
    {
        session.pending_edit = Some(edit);
    }

    // 4. The tool host, in the real run only: a measuring run has no tool pass.
    let mut tool_view = None;
    if let Some(pass) = body.pass {
        if let PressPhase::Down { start, .. } = &pass.press.phase {
            // A press nobody's part took, on the page or the gap between pages, ends Select All of the document.
            session.all_text = false;
            if start.button == PointerButton::Primary {
                out.pressed_page = body.pages.iter().find(|p| p.rect.contains(start.origin)).map(|p| p.page);
            }
        }
        let button_held = ui.input(|i| i.pointer.any_down());
        let tool_cx = PageToolCx { pages: body.pages, viewport: body.rect, view: body.view, button_held, ..cx.tools };
        let mut bench = page_tools::ToolBench::default();
        let mut fill = None;
        let mut area_mark = None;
        let mut tool = page_tool(cx.chosen, tool_cx, &mut *session, &mut bench, &mut fill, &mut area_mark, &pass.press);
        let reply = ToolHost::new().momentary(momentary).run(ui, pass, &mut tool);
        let mut finish = tool.finish();
        // The comment gestures the run finished: each committed one is an engine edit, one undo step per drag.
        let map = comments::PageMap::new(cx.info, body.pages, cx.rotation, body.view);
        let now = ui.input(|i| i.time);
        if let Some(edit) = comments::take_edits(&mut session.comments, &map, cx.info, cx.prefs, now) {
            session.pending_edit = Some(edit);
        }
        finish.apply(fill, area_mark, session, &tool_cx);
        if let Some(why) = finish.refused.take() {
            session.objects.notice = Some(why.into());
        }
        tool_view = reply.view;
        // The Menu key's request hangs the menu at the view's centre and leaves the comments alone.
        let secondary = egui::PointerButton::Secondary;
        let by_pointer = ui.input(|i| i.pointer.button_pressed(secondary) || i.pointer.button_released(secondary));
        if let Some(request) = reply.context.as_ref().filter(|_| by_pointer) {
            background_context(ui, session, cx, body, request.at);
        }
        out.context = reply.context;
        out.back_to_select = finish.back_to_select;
        out.snapshot = finish.snapshot;
        // Set Page Boxes opens on the current page: the page double-clicked becomes it first.
        if let Some(page) = finish.open_boxes {
            session.pending_page = Some((page, None));
            out.open_boxes = true;
        }
        out.turn = finish.turn;
        out.open_signature = finish.open_signature;
        let mut outcome = session.prepare.take_outcome();
        if let Some(edit) = outcome.edit.take() {
            session.pending_edit = Some(edit);
        }
        out.back_to_select |= outcome.placed;
        out.prepare = Some(outcome);
    }

    *saved_parts = Some(parts_out);
    *saved_arrived = field_arrived;
    tool_view
}

fn page_body(ui: &mut egui::Ui, body: &PageBody<'_>, callbacks: &mut PageCallbacks<'_, '_>) {
    let PageCallbacks { session, cx, out, parts, field_arrived, .. } = callbacks;
    session.placed.clear();
    session.placed.extend_from_slice(body.pages);
    out.shown = Some(body.view);
    let Some(parts_out) = parts.as_ref() else { return };
    let shown = cx.tool == QuickTool::Select && !cx.preparing;
    let tabs: Vec<_> = body
        .pages
        .iter()
        .map(|placed| {
            let size = cx.sizes.get(placed.page).copied().unwrap_or_default();
            let xf = PageXform::of(placed, cx.rotation, size);
            let overlay = overlay_parts(ui, &xf, cx.doc, placed.page, cx.can_modify, shown, false);
            (placed.page, xf, overlay.tabs)
        })
        .collect();
    // 5. The overlays.
    page_overlays::PageBackdrop::new(
        body.pages,
        cx.sizes,
        cx.rotation,
        cx.info,
        session,
        cx.form,
        parts_out.hovered.as_ref().map(|(owner, _)| owner),
    )
    .show_at(ui, body.rect);
    let view_picked = session.picked_image;
    let kept = session.area_pick.filter(|_| !session.box_drag.is_active());
    for (page, xf, page_tabs) in &tabs {
        let mut overlays =
            PageOverlays::new(*page, *xf, cx.doc, page_tabs, &session.compare_marks, &mut session.flash).highlight_fields(session.highlight_fields);
        if let Some(picked) = view_picked.filter(|p| p.page == *page) {
            overlays = overlays.picked(picked.rect);
        }
        if let Some((_, area)) = kept.filter(|(p, _)| p == page) {
            overlays = overlays.kept_area(area);
        }
        overlays.show_at(ui, body.rect);
    }
    if cx.preparing {
        crate::prepare::tool::show_overlays(ui, body.rect, body.view, session, cx.form, cx.info, body.pages, cx.rotation);
    }
    if cx.editing_content {
        page_overlays::PageDraft::new(&session.content).show_at(ui, body.rect);
        let geo = cx.geometry(body);
        let added_hovered = match parts_out.hovered {
            Some((Owner::Added { index }, page)) => Some((page, index)),
            _ => None,
        };
        let edits = [
            crate::edit_text_ui::picture_controls(ui, body, session, cx.doc, cx.info, geo),
            crate::object_ui::object_controls(ui, body, session, cx.info, parts_out.press.as_slice(), geo),
            crate::content_ui::content_controls(ui, body, session, cx.doc, added_hovered, cx.view_key),
            crate::edit_text_ui::paragraph_controls(ui, body, session, cx.doc, &parts_out, geo, cx.tool == QuickTool::AddText),
        ];
        // One edit per pass, the first control's that made one, as each control's own `or` keeps its first.
        if let Some(edit) = edits.into_iter().flatten().next() {
            session.pending_edit = Some(edit);
        }
        // Done on the open box keeps its text and goes back to Select, the new text selected; read in the real run,
        // whose report is the one kept.
        if body.pass.is_some() {
            out.back_to_select |= session.content.take_done();
        }
    }
<<<<<<< HEAD
    // The focused field's editor, and the Fill & Sign text being typed, over their pages.
    if let Some(editor) = crate::forms_ui::editor::show(ui, body, session, cx.doc, cx.form, cx.info, cx.today, cx.geometry(body))
        && *field_arrived
    {
        // The keys of a field that takes typing are its editor's: it takes the focus over from the field's part.
        focus::request(ui.ctx(), FocusTarget::Id(editor));
    }
    if let Some(e) = crate::fill_sign::type_box::show(ui, body, session, cx.info, &cx.prefs.author) {
        session.pending_edit = Some(e);
    }
    if cx.tool == QuickTool::Link {
        crate::link_ui::link_outlines(ui, body, &session.links, cx.doc.links(), cx.info);
    }
    // The comment being dragged, drawn from its own layers where it goes, over the page that still shows it where it
    // was. A page whose picture is of this revision has the released move: its layer settles.
    {
        let ctx = ui.ctx().clone();
        let movable = matches!(cx.tool, QuickTool::Select | QuickTool::Fill(_)) && !cx.comments_hidden;
        let target = movable.then(|| session.comments.primary()).flatten();
        let loading = |page: usize| body.pages.iter().any(|p| p.page == page && matches!(p.picture, TextureState::Loading { .. }));
        let drag = &mut session.signature_drag;
        drag.prepare(&ctx, cx.doc, target, cx.render_scale);
        if session.pending_edit.is_none() {
            for placed in body.pages.iter().filter(|p| !loading(p.page)) {
                drag.page_received(placed.page);
            }
        }
        for placed in body.pages.iter().filter(|p| matches!(p.picture, TextureState::Ready(_))) {
            session.forms.unbaked.retain(|(_, p)| *p != placed.page);
            let (document, page) = cx.revisions.of(placed.page);
            if placed.picture_revision == Some(craft::pages::ui::PageRevision { document, page }) {
                if session.pending_edit.is_none() {
                    session.comments.page_rendered(placed.page);
                }
                session.move_background.page_received(placed.page);
                session.text_background.page_received(placed.page);
            }
        }
        let stale = drag.moving_page().is_some_and(loading);
        drag.prepare_move(&ctx, cx.doc, &session.comments, session.pending_edit.as_ref(), stale, cx.render_scale);
        let map = comments::PageMap::new(cx.info, body.pages, cx.rotation, body.view);
        for placed in body.pages {
            let size = cx.sizes.get(placed.page).copied().unwrap_or_default();
            let xf = PageXform::of(placed, cx.rotation, size);
            let pcx =
                comments::PageCx { page: placed.page, xf: &xf, info: cx.info, prefs: cx.prefs, allowed: cx.allowed, hidden: cx.comments_hidden };
            signature_drag::DragLayer::new(&session.signature_drag, &pcx, &map, cx.tool, &session.comments, session.pending_edit.as_ref())
                .show_at(ui, body.rect);
        }
    }
    if !cx.comments_hidden {
        // The comment under the pointer is hit as the page's parts hit it: within the theme's reach, with its markers
        // at the icon size. The redaction hover is the user's setting, passed as a data colour (L-90).
        let env = Env::of(ui);
        let map = comments::PageMap::new(cx.info, body.pages, cx.rotation, body.view);
        let pick = (map.doc_len(env.hit_reach()), map.doc_len(env.tokens.size.icon.md));
        let pointer = ui.input(|i| i.pointer.hover_pos().filter(|_| !i.pointer.any_down()));
        let hover = comments::Hover { pointer, redact: cx.redact_hover, pick, allowed: cx.allowed };
        comments::show(ui, body, session, cx.rotation, cx.info, cx.prefs, cx.tool, hover);
    }
    if cx.calc_order {
        let hovered = match &parts_out.hovered {
            Some((Owner::Field { field, .. } | Owner::PrepareField { field, .. }, _)) => {
                cx.form.get(*field).map(crate::calc_order_ui::Focus::of_field)
            }
            _ => None,
        };
        let selected = session.prepare.selected.as_ref().map(|(name, _)| name.as_str());
        let mut overlay = crate::calc_order_ui::CalcOrderOverlay::new(&session.calc_order, cx.form, cx.info)
            .placed(body.pages)
            .rotation(cx.rotation)
            .generation(cx.doc.edit_generation())
            .viewport(body.rect);
        if let Some(name) = selected {
            overlay = overlay.selected(name);
        }
        if let Some(focus) = &hovered {
            overlay = overlay.hovered(focus);
        }
        overlay.show_at(ui, body.rect);
    }
    // The table being set up, on its page when that page is placed.
    if let Some(preview) = cx.table_preview
        && let Some(xf) = session.page_xform(preview.page(), cx.geometry(body))
    {
        crate::field_table_ui::TablePreview::new(tl!("Table cells"), preview, xf).show_at(ui, body.rect);
    }
    // The find marks under any tool, and the text selection where the Select tool is not the one painting it.
    let texts = selection::DocTexts::new(&session.texts, &session.text_failed, cx.rotation, cx.sizes);
    let paints_selection = !page_tools::tool_selects_text(cx.tool, cx.preparing);
    let marks = TextMarks::new(body.pages, &texts, cx.find_marks);
    let marks = match session.text.selection.filter(|_| paints_selection) {
        Some(selection) => marks.selection(selection),
        None => marks,
    };
    marks.show_at(ui, body.rect);
    image_pick::drop_stale(cx.doc, session);
    image_pick::resolve_pending(session, cx.geometry(body), cx.doc, cx.info, cx.tools.host.jobs_shared(), cx.tools.wake);
    image_pick::show_picked(ui, body, session, cx.rotation, cx.info);
    if let Some(at) = cx.present_at {
        let (now, pointer) = ui.input(|i| (i.time, i.pointer.hover_pos()));
        let frame = presenter::frame(
            &mut session.presenter,
            presenter::Shown { pages: body.pages, rect: body.rect },
            at,
            cx.info,
            ui.painter(),
            now,
            cx.animates,
            pointer,
        );
        // A still pointer stays hidden over every cursor, a link's included, until it moves.
        if frame.hide_pointer
            && let Some(pass) = body.pass
        {
            pass.hide_pointer(ui);
        }
        // A full-screen key's turn first, then the display time's.
        out.turn = out.turn.or(frame.step);
    }
    // A document that forbids changes has nothing to enhance: production shows the page without a quad or the bar.
    if cx.tool == QuickTool::EnhanceCamera && cx.can_modify {
        let quad = crate::camera_ui::show_quad(ui, body, session, cx.can_modify, cx.current);
        let bar = crate::camera_ui::bar(ui, body, session, cx.can_modify);
        out.action = bar.or(quad);
    }
    if cx.chosen == QuickTool::Snapshot {
        let sizes = cx.sizes;
        out.area_choice =
            crate::zoom_snap::area_bar(ui, body, body.pages, session.area_pick, cx.rotation, |page| sizes.get(page).copied().unwrap_or_default());
    }
    if cx.unobstructed && matches!(cx.tool, QuickTool::Select | QuickTool::ColumnSelect) {
        out.copy_selected =
            selection::selection_bar(ui, body, session, cx.info, cx.rotation, cx.prefs, cx.allowed && !cx.comments_hidden, cx.can_copy);
    }
    if cx.unobstructed
        && let Some(page) = cx.info.pages.get(cx.current)
    {
        page_size_hint(ui, body, page, cx.units);
    }
=======
    scroll
}

/// Where the `shown` pages sit in the scroll content, as how far the content starts into the
/// layout and how tall it is. Scrolling continuously every page scrolls; otherwise only the
/// current row does, the content being its bounding box plus `margin` (none while full screen
/// fills the screen). Pages that fit a view
/// `view_height` tall sit in its middle, the spare height shared above and below (#5708): the
/// content fills the view, so there is nothing to scroll, and starts on a whole physical pixel
/// as the layout's pages do, or their edges would blur and the seams between them open.
fn vertical_placement(continuous: bool, rects: &[Rect], shown: &[usize], view_height: f32, margin: f32, pixels_per_point: f32) -> (f32, f32) {
    let Some(r) = shown.iter().filter_map(|&i| rects.get(i)).copied().reduce(Rect::union) else { return (0.0, 0.0) };
    if fits_height(r, view_height, pixels_per_point) {
        return (((r.center().y - view_height / 2.0) * pixels_per_point).round() / pixels_per_point, view_height);
    }
    if continuous { (0.0, r.bottom() + margin) } else { (r.top() - margin, r.height() + 2.0 * margin) }
>>>>>>> upstream/main
}

/// The text layers the pass asks for, as `(visible, rest)` for [`DocView::want_text`], which skips pages whose layer is
/// held, failed or on its way: the pages in view (none under the Hand tool, which selects nothing), then every page a
/// selection covers, and while a search runs every page from the current one on, so a match far off is found.
fn text_demand(view: &DocView, visible: std::ops::Range<usize>, hand: bool) -> (Vec<usize>, Vec<usize>) {
    let count = view.page_count;
    let visible: Vec<usize> = if hand { Vec::new() } else { visible.collect() };
    let mut rest: Vec<usize> = view.session.text.selection.iter().flat_map(|s| s.pages()).collect();
    if count > 0 && view.find.as_ref().is_some_and(|f| !f.case_query.trim().is_empty()) {
        let current = view.view.current;
        rest.extend((0..count).map(|k| (current + k) % count));
    }
    (visible, rest)
}

/// Draws the document tab at `index` in the room left in `ui`: [`document_area_at`] with the available rect.
pub fn document_area(app: &mut PdfCraftApp, index: usize, ui: &mut egui::Ui) {
    let rect = ui.available_rect_before_wrap();
    // The output is for callers that place two views (Compare); a whole tab has nothing to sync.
    let _ = document_area_at(app, index, rect, ui);
}

/// Draws the document tab at `index` in `rect`: its notice bars, then its pages in the page view (or the Organize
/// grid), with the page body's parts, tools and overlays, and acts on what the pass asked for. Returns the page view's
/// output, for callers that place several views (Compare).
///
/// It records the pass's facts on the view for readers between frames (`textures`, `spacing`, the viewport, the
/// canvas view, the middle-button scroll); nothing in the next pass reads them to compute its result.
pub fn document_area_at(app: &mut PdfCraftApp, index: usize, rect: Rect, ui: &mut egui::Ui) -> Option<PageViewOutput> {
    // The seam returns `None` for a tab that closes, has no pages or shows the Organize grid: no page view runs there,
    // and `PageViewOutput` cannot be built outside its crate.
    // The Search panel closed: its search moves to the find bar.
    let search_open = app.right() == Some(crate::RightPanel::Search);
    if let Some(f) = app.views.get_mut(index).and_then(|v| v.find.as_mut())
        && f.in_panel
        && !search_open
    {
        f.in_panel = false;
    }
    if close_requested_tab(ui.ctx(), app, index) {
        return None;
    }
    let doc_id = app.views.get(index)?.id;
    let doc = app.session.get(doc_id)?;
    // PLAN: the print preview is the dialog's own `Thumbnails<PrintPage>` store (pdf-canvas-w10 note 2); no view keeps
    // print rasters to drop while `HostedPrint` is closed, so the old `Dialog::Print` check has nothing left to guard.
    let show_secured_notice = app.show_secured_notice;
    // Offer to lock a just-placed signature (#2149) on the same terms the save-time question
    // asks: one is still there, saving doesn't lock it anyway, and the document isn't signed.
    let lock_offer = !app.flatten_fill_sign_on_save
        && app.placed_signatures.contains(&doc_id)
        && doc.info.annotations.iter().any(|a| a.fill_sign)
        && !doc.is_signed();
    let view = &mut app.views[index];
    // Opened without the owner password and something is restricted. The notice says so unless
    // the user turned it off (#4035): the restrictions themselves, and Document Properties ▸
    // Security, stay as they are.
    let secured = show_secured_notice && doc.security_summary().is_some_and(|s| !(s.owner || (s.permissions.modify() && s.permissions.assemble())));
    let repaired = !doc.repair_log().is_empty();
    // PLAN: assumed `notices(view, doc, secured, repaired, lock_offer, banner, ui)`: pdf-canvas-w16 drops the tokens
    // argument and gives no other change.
    let bars_top = ui.available_rect_before_wrap().top();
    let notice = notices(view, doc, secured, repaired, lock_offer, crate::sign_ui::banner(&doc.signatures), ui);
    // The notice bars' height keeps the floating toolbar below them (the dock's top inset, next pass).
    let inset = (ui.available_rect_before_wrap().top() - bars_top).max(0.0);
    if (inset - app.notice_inset).abs() > 0.5 {
        app.notice_inset = inset;
        ui.ctx().request_repaint();
    }
    apply_notice(app, ui.ctx(), index, notice);
    let doc = app.session.get(doc_id)?;
    let info = &doc.info;
    if info.pages.is_empty() {
        let _ = Text::new(tl!("This document has no pages.")).show(ui);
        return None;
    }
    // No dialog, close prompt or palette over the page: only then does page input count.
    let unobstructed = !app.modal_open() && !app.palette_open;
    if app.views[index].organize {
        let invert_middle = app.navigation_options().invert_middle;
        organize_grid(&mut app.views[index], doc, &mut organize::GridCx { ui, session: &app.session, zoom: &mut app.organize_zoom, invert_middle });
        return None;
    }
    // The room under the notice bars, inside the rect the caller gave.
    let rect = rect.intersect(ui.available_rect_before_wrap());

<<<<<<< HEAD
    let preparing = app.is_preparing();
    // Prepare a form ▸ Calculation order: the order's steps and links on the page.
    let calc_order = preparing && crate::side_panels::panel_visible(app, crate::LeftPanel::CalculationOrder);
    // Edit a PDF: added text and images can be selected, moved and edited.
    let editing_content =
        crate::side_panels::panel_visible(app, crate::LeftPanel::Tool("edit")) || matches!(app.quick_tool, QuickTool::AddText | QuickTool::EditText);
    let chosen = app.quick_tool;
    let navigation = app.navigation_options();
    let presenting = app.views[index].presenting();
=======
    let avail = ui.available_rect_before_wrap();
    view.viewport_w = avail.width();
    view.viewport_h = avail.height();
    let old_zoom = view.zoom;
    view.fit_zoom(info);
    anchor_zoom_change(view, avail.size(), old_zoom);

    // Pinch / ctrl+scroll zoom, anchored on the current page.
    let zoom_delta = crate::motion::zoom_delta(ui.ctx());
    // Exclude the previous frame's scrollbar tracks from gesture activation.
    let navigation_rect = if view.viewport_screen.is_positive() { avail.intersect(view.viewport_screen) } else { avail };
    // Wheel zoom works in a visible window without focus, as wheel scrolling does; the other gestures need focus.
    let zoom_enabled = unobstructed && ui.is_enabled();
    let navigation_enabled = zoom_enabled && ui.input(|i| i.focused);
    let present = presentation_click_state(ui, view, navigation_rect, navigation_enabled && app.quick_tool == QuickTool::Select);
    pinch_zoom(ui, view, zoom_delta, navigation_rect, zoom_enabled);
    view.viewport_screen = avail;
    let (auto_delta, edge_delta) = auto_scroll_deltas(ui, view, avail, unobstructed, invert_middle);

    let content_w = view.content_width(info, avail.width());
    let rects = view.layout(info, content_w, ui.ctx().pixels_per_point());
    // When not scrolling continuously, only one row is shown at a time: the current page, or the
    // current two-page spread (with the cover rule). These indices drive paging, which pages are
    // drawn, and the content height (#752). Empty for a document with no pages.
    let row = shown_row(view, &rects);
    let middle_gesture = view.auto_scroll.blocks_input();
    reset_interrupted_wheel(ui, view, zoom_delta != 1.0 || key_scroll != 0.0 || auto_delta != Vec2::ZERO || edge_delta != Vec2::ZERO);
    // Not scrolling: when the whole page or spread fits, the wheel would do nothing, so it turns
    // pages instead; zoomed in far enough to pan, it pans, and at the edge it turns the page on
    // (#1924, `single_page_wheel_edge`). Touch drags are untouched: with
    // nothing to pan, touch users turn pages with the rail buttons, the page box and the arrow
    // keys. Runs before `visible_pages` so the frame that turns the page draws it.
    let (single_paging, edge_turned) = single_page_paging(ui, view, &PagingCx { row: &row, rects: &rects, avail, middle_gesture, unobstructed });
    let now = ui.input(|i| i.time);
    let animates = app.animation_effects.animates(app.motion.reduced);
    let wheel_enabled = navigation_enabled && unobstructed && !middle_gesture;
    let Wheel { precise_scroll, line_base, coarse_scroll, has_page, paging_owns_vertical } =
        read_wheel(ui, view, avail, single_paging, edge_turned, wheel_enabled, animates);
    let may_glide = !paging_owns_vertical && !middle_gesture && navigation_enabled && unobstructed && animates;
    let wheel_scrolled = precise_scroll != Vec2::ZERO || line_base != Vec2::ZERO || coarse_scroll != Vec2::ZERO || has_page;
    let motion = GlideCx { now, may_glide, key_scroll, key_side, auto_delta, zoom_delta, edge_delta, wheel_scrolled };
    let glide = step_glide(ui, view, avail, &motion);
    // The wheel may have turned the page above: draw the row it turned to. Measured from the old
    // row, the jump to the new page scrolled both out of view, a blank frame on every turn forward.
    let row = shown_row(view, &rects);
    let visible_pages: Vec<usize> = if view.continuous { (0..rects.len()).collect() } else { row.clone() };
    let (y_shift, content_h) =
        vertical_placement(view.continuous, &rects, &visible_pages, avail.height(), view.margin(), ui.ctx().pixels_per_point());

    let mut scroll = page_scroll_area(
        ui,
        view,
        &ScrollGeometry { rects: &rects, y_shift, content_w, content_h, avail, middle_gesture, hand: app.quick_tool == QuickTool::Hand },
    );
    let ppp = ui.ctx().pixels_per_point();
    let scale = view.render_scale(ppp);
    let tag = scale_tag(scale);
    // Hold sharp re-renders while the zoom is still moving: a new scale every frame would
    // cancel the in-flight full-page interpret and restart it, so on a heavy page (a CAD
    // plot with ~1000 images renders for seconds) the sharp raster never arrives and the
    // view looks stuck on a soft image. The stale raster stays stretched meanwhile; the
    // sharp request goes out once the zoom settles.
    let sharp_held = hold_sharp_renders(ui, view, scale);
    view.current_scale_tag = tag;
    let hand = app.quick_tool == QuickTool::Hand;
    let (tool, placing) = page_tool(view, app.quick_tool);
    // Text selection runs for the Select tool and for the markup tools (highlight…).
    let selects_text = tool_selects_text(tool, preparing);
    let prefs = &app.comment_prefs;
    let redact_mark = (app.redact_prefs.hover_fill, app.redact_prefs.opacity);
    let units = app.units;
>>>>>>> upstream/main
    let allowed = doc.allows_annotation();
    let can_modify = doc.allows_modification();
    let form = doc.form.clone();
    let paper = app.session.reading_colors(doc_id).and_then(|c| c.params).map_or(Color32::WHITE, |p| {
        let [r, g, b] = p.background;
        Color32::from_rgb(r, g, b)
    });
    let table_preview = crate::field_table_ui::preview(&mut app.dialogs, doc_id);
    // Full screen shows the pages on black, as production does.
    let surround = if app.full_screen_document() { PasteboardColor::Black } else { PasteboardColor::Default };
    let view = &mut app.views[index];
    // PLAN: assumed `page_tools::seen_tool(&PageSession, QuickTool) -> QuickTool`, the placing rule `page_tool`
    // applies (copied comments on the pointer make the page see Select), so the parts gate on the same tool.
    let tool = page_tools::seen_tool(&view.session, chosen);
    if preparing {
        crate::prepare::after_refresh(&mut view.session, &form);
    } else {
        view.session.prepare.selected = None;
    }
    view.session.objects.prepare(doc, tool == QuickTool::EditText && can_modify);
    let scale = view.render_scale(ui.ctx().pixels_per_point());
    let session = &mut view.session;
    session.text_background.prepare(ui.ctx(), doc, session.line_editor.as_ref(), session.content.draft.as_ref(), scale);
    let added = if editing_content { doc.added().to_vec() } else { Vec::new() };
    if editing_content {
        crate::content_ui::after_refresh(&mut view.session, &added);
    } else {
        view.session.content.selected = None;
    }
    view.session.move_background.prepare(ui.ctx(), doc, &view.session.content, scale);

    let find_marks: Vec<TextMark> = view.find.as_ref().map(Find::marks).unwrap_or_default();
    let present_at =
        presenting.then_some(presenter::PresentAt { current: view.view.current, display: view.view.display, rotation: view.view.rotation });
    let veiled = page_overlays::veiled(view);
    // Shadows only with the preference on, and only between gaps: they would darken the page before when pages touch.
    let border = if view.view.page_gaps && app.show_page_edge_shadows { PageBorder::DropShadow } else { PageBorder::None };
    let keep = view.session.presenter.keep_pages();
    if view.momentary_options != navigation {
        view.momentary = Momentary::new(navigation);
        view.momentary_options = navigation;
    }
    let mut body_out = BodyOut::default();
    let date_text = crate::date_text_for_pdf(&app.session);
    // The dynamic stamps' lines, filled in for now: the built-in ones write the default line, a custom stamp its own.
    let by_line = app.session.stamp_line(crate::stamps_ui::DEFAULT_LINE, &app.comment_prefs.author);
    let custom_line = match tool {
        QuickTool::CustomStamp(i) => app
            .custom_stamps
            .get(i)
            .and_then(|stamp| stamp.line.as_deref())
            .filter(|line| !line.trim().is_empty())
            .map(|line| app.session.stamp_line(line, &app.comment_prefs.author)),
        _ => None,
    };
    let custom_stamp = match tool {
        QuickTool::CustomStamp(i) => app.custom_stamps.get(i).map(|stamp| (stamp, custom_line.as_deref())),
        _ => None,
    };
    let view_key = view.view_key();
    let DocView { view: state, session, momentary, revisions, reports, page_sizes, .. } = &mut *view;
    let tools = page_tools::PageToolCx {
        doc,
        info,
        form: form.as_slice(),
        pages: &[],
        sizes: &page_sizes[..],
        rotation: state.rotation,
        viewport: Rect::NOTHING,
        view: CanvasView::new(Rect::NOTHING, craft::behavior::kurbo::Point::ORIGIN, 1.0),
        button_held: false,
        text_style: &app.text_style,
        allowed,
        can_modify,
        can_crop: can_modify,
        preparing,
        presenting,
        units: app.units,
        navigation,
        host: &app.host,
        wake: &app.wake,
        author: &app.comment_prefs.author,
        comment_prefs: &app.comment_prefs,
        date_text: &date_text,
        fill: page_tools::FillPrefs {
            text_size: app.fill_text_size,
            text_color: app.fill_text_color,
            text_border: crate::fill_sign::TextBorder {
                width: app.fill_text_border,
                color: app.fill_text_border_color,
                shape: app.fill_text_border_shape,
            },
            signature: app.signature.as_ref(),
            initials: app.initials.as_ref(),
        },
        by_line: &by_line,
        custom_stamp,
    };
    let cx = PageBodyCx {
        doc,
        info,
        revisions,
        tool,
        chosen,
        rotation: state.rotation,
        current: state.current,
        render_scale: scale,
        sizes: &page_sizes[..],
        view_key,
        tools,
        animates: !app.animation_effects.reduced(None),
        present_at,
        veiled,
        preparing,
        calc_order,
        editing_content,
        allowed,
        can_modify,
        can_copy: doc.allows_copying(),
        comments_hidden: doc.comments_hidden(),
        text_links: app.text_links,
        unobstructed,
        prefs: &app.comment_prefs,
        redact_hover: (app.redact_prefs.hover_fill, app.redact_prefs.opacity),
        form: &form,
        today: app.session.today(),
        units: app.units,
        find_marks: &find_marks,
        table_preview: table_preview.as_ref(),
    };
    let source = DocPages::new(&app.session, doc_id).rotated(state.rotation).revisions(revisions).reports(reports);
    // A modal over the pages blocks the canvas's keys and presses in the framework, so the view needs no gate here.
    // Scope each tab's PageView by its view key so per-canvas gestures (like middle-button autoscroll) stay tab-local.
    let mut callbacks = PageCallbacks { session, momentary, cx: &cx, out: &mut body_out, parts: None, field_arrived: false };
    let out = ui
        .push_id(view_key, |ui| {
            PageView::new(tl!("Pages"), &source, state, DataColor::new(paper))
                .navigation(navigation)
                .border(border)
                .pasteboard(surround)
                .keep(&keep)
                .show_at_with(ui, rect, &mut callbacks, page_before, page_body)
        })
        .inner;
    if view.view.continuous
        && let Some(page) = body_out.pressed_page
    {
        view.view.current = page;
    }
    if out.scrolled {
        view.end_find_following();
    }
    if out.toggle_white_space {
        view.view.hide_white_space = !view.view.hide_white_space;
    }
    // The reader's own scroll (the wheel, a scroll bar, the view's keys) ends following the search; a flash's go-to is
    // not theirs.
    if out.scrolled {
        view.end_find_following();
    }
    // Full screen shows the page alone, so its keys are the page's: with nothing else holding the keyboard, the page
    // view takes it, and the presenter's keys reach the running tool.
    if presenting && ui.ctx().memory(|m| m.focused()).is_none() {
        focus::request(ui.ctx(), FocusTarget::Id(out.canvas.response.id));
    }
    if let Some(action) = body_out.action.take() {
        view.pending_action = Some(action);
    }
    match view.session.pending_page.take() {
        Some((page, Some(rect))) => view.reveal_rect(page, rect, info),
        Some((page, None)) => view.go_to_page(page),
        None => {}
    }
    // A turn the presenter's display time or a full-screen key asked for, then a click's turn once the
    // double-click time has passed with no object clicked and no selection made (#2070).
    let now = ui.input(|i| i.time);
    let has_selection = view.session.text.selection.is_some();
    // A click turns forward; a Shift-click or a right-click back (*Adobe Acrobat Help*, 2019, "Full Screen
    // preferences", p. 69).
    let press = &out.canvas.press;
    let start = press.phase.start();
    let button = start.map(|start| start.button);
    let click = (presenting
        && matches!(press.phase, PressPhase::Release { click: true, .. })
        && matches!(button, Some(PointerButton::Primary | PointerButton::Secondary)))
    .then(|| presenter::PresentClick {
        forward: !press.mods.shift && button != Some(PointerButton::Secondary),
        clicks: start.map_or(1, |start| start.clicks),
    });
    let delay = ui.ctx().options(|o| o.input_options.max_double_click_delay);
    // A click on a part (a link, a field, a note) is the part's (*Adobe Acrobat Help*, 2019, p. 65: the pointer stays
    // active "so that you can click links and open notes").
    let cancels = body_out.link.is_some() || has_selection || (click.is_some() && body_out.over_part);
    let click_turn = if presenting { view.session.presenter.click_turn(ui.ctx(), click, cancels, now, delay) } else { None };
    if let Some(forward) = body_out.turn.or(click_turn) {
        view.view.step(forward);
    }
    // The records readers between frames take; no pass reads them back.
    view.textures = out.textures;
    view.spacing = Some(out.spacing);
    view.viewport = rect;
    view.canvas = body_out.shown;
    view.middle_scrolling = out.canvas.auto_scrolling;
    // Addresses written as plain text are found once per text layer that has arrived; the link parts read them.
    if app.text_links {
        for page in out.visible.clone() {
            view.refresh_text_links(page);
        }
    }
    // Text layers: the queue is made once, from the app's job host; each pass takes what has arrived and asks for what
    // it needs.
    if view.session.text_queue.is_none() {
        let ctx = ui.ctx().clone();
        let wake = craft::host::Wake::new(move || ctx.request_repaint());
        view.session.text_queue =
            Some(JobQueue::new(app.host.jobs_shared().clone(), wake, QueueOptions { name: "text-layers", ..QueueOptions::default() }));
    }
    let (visible_pages, rest_pages) = text_demand(view, out.visible.clone(), chosen == QuickTool::Hand);
    view.want_text(&app.session, &visible_pages, &rest_pages);

    find_bar(view, info.pages.len(), rect, tool == QuickTool::EditText && can_modify, ui);
    // The clipboard is read when Paste is invoked, so its menu stays enabled while the document is open.
    let can_paste_image = true;
    let doc = app.session.get(doc_id)?;
    let info = &doc.info;
    let view = &mut app.views[index];
    // The in-place editors and comment windows over the pages, in their old order, mapped through this pass's
    // placement.
    if let Some(e) = crate::comments::composer(ui, view, info, &app.comment_prefs) {
        view.session.pending_edit = Some(e);
    }
    let popups = crate::comments::note_popups(ui, view, info, &app.comment_prefs, doc.comments_hidden(), allowed, body_out.hovered_comment);
    if let Some(e) = popups {
        let session = &mut view.session;
        // A click on the composer's Post also takes the focus from a pop-up's box, saving what
        // was typed there: both edits land in this pass, so both are kept, in one batch.
        session.pending_edit = Some(match session.pending_edit.take() {
            Some(first) => Edit::Batch { label: "Edit comment".into(), edits: vec![first, e] },
            None => e,
        });
    }

    let refused = view.session.objects.notice.take();
    let form_notice = view.session.forms.notice.take();
    let mut tool = app.quick_tool;
    if tool != QuickTool::EditText {
        view.session.image_selection = None;
        crate::content_ui::release_image_selection(&mut view.session, &added);
    }
    if tool != QuickTool::Link {
        view.session.links.selected = None;
    }
    // A stamp placed, a crop committed, Add text done or a field placed: one, then back to selecting (Acrobat's
    // default without "Keep tools pinned").
    if body_out.back_to_select {
        tool = QuickTool::Select;
    }
    let prepare = body_out.prepare.take().unwrap_or_default();
    let field_props = prepare.properties.then(|| view.session.prepare.selected.clone()).flatten();
    let (clicked_link, javascript_link) = body_out.link.take().map_or((None, None), |(target, js)| (Some(target), js));
    // In full screen a secondary click turns the page, so the page menu must not open on it.
    let menu_for = view.session.menu_for.as_ref().map(|(_, owner)| owner.clone());
    let pick = if presenting {
        page_menu::PageMenuPick::default()
    } else {
        page_menu::show(
            ui,
            body_out.context.as_ref(),
            &out.canvas.response,
            menu_for,
            &mut app.views[index],
            &page_menu::PageMenuCx {
                doc,
                info,
                prefs: &app.comment_prefs,
                rights: comments::MenuRights { allowed, can_modify, can_paste: app.comment_clipboard.is_some() },
                recent: &app.recent_colours,
                tool,
                preparing,
                form: &form,
                can_copy: doc.allows_copying(),
                can_paste_image,
                can_paste_comments: app.comment_clipboard.is_some(),
                jobs: app.host.jobs_shared(),
                wake: &app.wake,
            },
        )
    };
    let mut open_props: Option<(usize, usize)> = None;
    let mut recolour: Option<pdfcraft_engine::Rgb> = None;
<<<<<<< HEAD
    let mut image_action = pick.image;
    match pick.canvas {
        Some(comments::CanvasAction::Edit(e)) => app.views[index].session.pending_edit = Some(*e),
=======
    // The table being set up in its window, shown on its own document's page.
    let table_preview = app.field_table_draft.as_mut().filter(|d| d.doc == view.id).and_then(|d| Some(d.preview(info.pages.get(d.page)?, &form)));
    let can_modify = doc.allows_modification();
    if preparing {
        crate::prepare::after_refresh(view, &form);
    } else {
        view.prepare.selected = None;
    }
    view.objects.prepare(doc, tool == QuickTool::EditText && can_modify, ui.ctx(), keys);
    view.text_background.prepare(ui.ctx(), doc, view.line_editor.as_ref(), view.content.draft.as_ref(), scale);
    let added = if editing_content { doc.added().to_vec() } else { Vec::new() };
    let doc_links = if tool == QuickTool::Link { doc.links().to_vec() } else { Vec::new() };
    if editing_content {
        crate::content_ui::after_refresh(view, &added);
    } else {
        view.content.selected = None;
    }
    view.move_background.prepare(ui.ctx(), doc, &view.content, scale);

    scroll = refine_goto_point(scroll, view, &ScrollGeometry { rects: &rects, y_shift, content_w, content_h, avail, middle_gesture, hand });
    let selection_wheel = view.selection_drag && navigation_enabled && unobstructed && !single_paging && ui.rect_contains_pointer(avail);
    let prepare_tab_order = (preparing && view.prepare.show_tab_numbers).then(|| crate::forms_ui::tab_order(&form, info));
    let env = PageEnv {
        added: &added,
        allowed,
        animates,
        author: &author,
        auto_delta,
        avail,
        coarse_scroll,
        by_line: &by_line,
        calc_order,
        can_copy,
        can_paste_comments,
        placing,
        can_crop,
        can_fill,
        can_modify,
        comments_hidden,
        content_h,
        content_w,
        custom_line: &custom_line,
        custom_stamp: &custom_stamp,
        date_text: &date_text,
        doc,
        doc_links: &doc_links,
        edge_delta,
        editing_content,
        form: &form,
        generation,
        glide,
        hand,
        info,
        initials: &initials,
        key_scroll,
        key_side,
        keys,
        middle_gesture,
        modal,
        navigation_enabled,
        paper,
        show_page_edge_shadows: app.show_page_edge_shadows,
        ppp,
        precise_scroll,
        prefs,
        preparing,
        prepare_tab_order: prepare_tab_order.as_deref(),
        rects: &rects,
        redact_mark,
        scale,
        selection_wheel,
        selects_text,
        signature: &signature,
        single_paging,
        t: &t,
        table_preview: &table_preview,
        tag,
        text_style: &text_style,
        text_links: app.text_links,
        tool,
        units,
        unobstructed,
        visible_pages: &visible_pages,
        y_shift,
        fill_text_size: app.fill_text_size,
        fill_text_color: app.fill_text_color,
        fill_text_border: crate::fill_sign::TextBorder {
            width: app.fill_text_border,
            color: app.fill_text_border_color,
            shape: app.fill_text_border_shape,
        },
        today,
        can_paste_image,
    };
    let mut page_out = PageOut::default();
    // Keyed by the view, not the document: with one document in both panes of split view, shared
    // ids gave the main pane's clicks on the page to the second pane's widgets.
    let out = page_scroll(ui, scroll, egui::Id::unique(("page-view-scope", view.view_key())), app.show_page_edge_shadows, |ui, viewport| {
        draw_pages(ui, viewport, view, &env, &mut page_out, &mut app.signature_preview)
    });
    let PageOut {
        canvas_action,
        clicked_link,
        javascript_link,
        comment_notice,
        copy_comments,
        mut copy_selected,
        field_menu,
        field_placed,
        mut field_props,
        hover_text,
        mut image_action,
        open_boxes,
        open_initials,
        open_signature,
        open_table,
        page_action,
        paste_comments_at,
        primary_down,
        on_object,
        mut refused,
        row_on_screen,
        stamp_placed,
    } = page_out;
    view.single_edges = visible_pages.first().copied().filter(|_| !view.continuous).map(|page| {
        // At the top once the page's top edge shows where going to a page puts it (a gap
        // below the window's top). Within a point, as for wheel paging, so layout rounding
        // can't hide an edge.
        let max_y = (out.content_size.y - out.inner_rect.height()).max(0.0);
        // Going to a page clamps at 0, which is where a page sits with no margin (full screen).
        (page, out.state.clamped_offset().y <= (view.margin() - view.page_gap()).max(0.0) + 1.0, out.state.clamped_offset().y >= max_y - 1.0)
    });
    // Within a point, as above: at an edge of the scrolled content, or of the current page or
    // spread shown whole that way (in continuous view the content is as wide as the widest page,
    // and a narrower current page shows whole inside it).
    let max_x = (out.content_size.x - out.inner_rect.width()).max(0.0);
    let inner = out.inner_rect;
    view.side_edges = Some((
        out.state.clamped_offset().x <= 1.0 || row_on_screen.is_some_and(|r| r.left() >= inner.left() - 1.0),
        out.state.clamped_offset().x >= max_x - 1.0 || row_on_screen.is_some_and(|r| r.right() <= inner.right() + 1.0),
    ));

    view.viewport_screen = out.inner_rect;
    view.auto_scroll.paint(ui, avail);
    if unobstructed && matches!(tool, QuickTool::Select | QuickTool::ColumnSelect) {
        copy_selected |= selection_actions(ui, avail, view, info, prefs, allowed && !comments_hidden, can_copy);
    }

    // Collect all main-page demand, including cached surfaces, for byte admission later.
    // Pan-settle debounce for tiled pages: each tile costs a full page interpret, so holding
    // fresh tiles back while the scroll offset is still moving keeps a drag on the stretched
    // backdrop/older tiles instead of queueing a storm of interprets (one per frame) that
    // the workers cannot cancel mid-operator. Fresh tiles go out once the pan settles;
    // pages with nothing on screen yet still queue.
    let pan_held = track_scroll(ui, view, &out, glide);
    plan_renders(ui, view, info, out.inner, &RenderPlan { visible_pages: &visible_pages, tag, scale, sharp_held, pan_held, hand });

    let text_done = frame_overlays(ui, view, &env, primary_down, hover_text);
    if let Some(why) = view.objects.notice.take() {
        refused = Some(why);
    }
    let form_notice = view.forms.notice.take();
    // One crop, then back to selecting (as Acrobat does).
    let cropped = view.pending_edit.as_ref().is_some_and(|e| matches!(e, pdfcraft_engine::Edit::SetPageBox { .. }));
    let mut tool = app.quick_tool;
    comments::keys(ui.ctx(), keys, view, info, &mut tool, allowed);
    if preparing {
        crate::prepare::keys(ui.ctx(), keys, view);
    }
    if tool != QuickTool::EditText {
        view.image_selection = None;
        crate::content_ui::release_image_selection(view, &added);
    }
    if editing_content {
        crate::content_ui::keys(ui.ctx(), keys, view, tool == QuickTool::EditText);
    }
    if tool == QuickTool::Link {
        crate::link_ui::keys(ui.ctx(), keys, view);
    } else {
        view.links.selected = None;
    }
    if stamp_placed {
        tool = QuickTool::Select;
    }
    // Text: each click on the page starts a box (#74); Done in the editor goes back to selecting.
    if text_done && tool == QuickTool::AddText {
        tool = QuickTool::Select;
    }
    // One field, then back to selecting (Acrobat's default without "Keep tools pinned").
    if field_placed {
        tool = QuickTool::Select;
    }
    if matches!(field_menu, Some(FieldMenu::Properties)) {
        field_props = true;
    }
    let field_props = field_props.then(|| view.prepare.selected.clone()).flatten();
    match canvas_action {
        Some(comments::CanvasAction::Edit(e)) => view.pending_edit = Some(*e),
        // `choose_right_panel` would borrow all of `app` while `view` is held; same effect.
>>>>>>> upstream/main
        Some(comments::CanvasAction::OpenComments) => {
            app.comments_panel_closed = false;
            crate::side_panels::set_panel_visible(app, RightPanel::Comments.into(), true);
        }
        Some(comments::CanvasAction::Command(id)) => {
            app.execute(ui.ctx(), id);
        }
        Some(comments::CanvasAction::Properties(p, i)) => open_props = Some((p, i)),
        Some(comments::CanvasAction::Colour(c)) => recolour = Some(c),
        Some(comments::CanvasAction::CopyText(text)) => {
            // A refused write says why itself.
            app.put_text(&text);
        }
        // Other…: the picker's answer colours every selected comment, as a colour cell does.
        Some(comments::CanvasAction::ColourRequest(request)) => {
            app.open_colour_picker(crate::colour_picker::PickerHost::App, request, |app: &mut PdfCraftApp, ctx: &egui::Context, colour| {
                app.colour_selected_comments(ctx, comments::to_rgb(colour));
            });
        }
        Some(comments::CanvasAction::CopyImage(p, i)) => image_action = Some(crate::edit_text_ui::ImageAction::Copy(p, i)),
        Some(comments::CanvasAction::CopyImageFile(p, i)) => image_action = Some(crate::edit_text_ui::ImageAction::CopyFile(p, i)),
        None => {}
    }
    if let Some(why) = refused {
        app.notify_error(why);
    }
    if let Some((page, area)) = body_out.snapshot {
        app.finish_marquee(ui.ctx(), index, (page, area));
    }
    let remote_open = follow_link(app, index, clicked_link);
    if let Some((page, link)) = javascript_link {
        app.run_link_script(ui.ctx(), doc_id, page, link);
    }
    let tool = finish_sign_requests(app, ui.ctx(), index, tool, body_out.area_choice);
    app.quick_tool = tool;
    let requests = FrameRequests {
        open_props,
        field_props,
        field_menu: pick.field,
        image_action,
        page_action: pick.page,
        form_notice,
        copy_selected: body_out.copy_selected,
        recolour,
        remote_open,
        open_table: prepare.table,
        open_boxes: body_out.open_boxes,
        open_signature: body_out.open_signature,
        form,
        generation: app.session.get(doc_id).map_or(0, |d| d.edit_generation()),
    };
    apply_frame_requests(app, ui.ctx(), index, requests);
    Some(out)
}

/// What the frame's input asked the app to do, once the document is no longer borrowed.
struct FrameRequests {
    open_props: Option<(usize, usize)>,
    field_props: Option<(String, usize)>,
    field_menu: Option<FieldMenu>,
    image_action: Option<crate::edit_text_ui::ImageAction>,
    page_action: Option<PageMenuAction>,
    form_notice: Option<crate::forms_ui::FormNotice>,
    copy_selected: bool,
    /// A colour picked for every selected comment (Colour in the page's menu).
    recolour: Option<pdfcraft_engine::Rgb>,
    remote_open: Option<(pdfcraft_engine::DocId, String, pdfcraft_render::RemoteDestination)>,
    /// A table drawn with the prepare tool: its page and user-space rectangle.
    open_table: Option<(usize, [f64; 4])>,
    /// The Crop tool's double-click: open Set Page Boxes on the current page.
    open_boxes: bool,
    /// Fill & Sign needs a signature (`false`) or initials (`true`) first.
    open_signature: Option<bool>,
    form: Arc<Vec<pdfcraft_engine::FormField>>,
    generation: u64,
}

/// Opens the dialogs and runs the app commands the frame asked for.
#[allow(clippy::cognitive_complexity)]
fn apply_frame_requests(app: &mut PdfCraftApp, ctx: &egui::Context, index: usize, requests: FrameRequests) {
    let FrameRequests {
        open_props,
        field_props,
        field_menu,
        image_action,
        page_action,
        form_notice,
        copy_selected,
        recolour,
        remote_open,
        open_table,
        open_boxes,
        open_signature,
        form,
        generation,
    } = requests;
    if let Some((page, at)) = app.views[index].session.comments.attach_at.take() {
        app.attach_file_comment(page, at);
    }
    if let Some((p, i)) = open_props {
        app.open_comment_props(ctx, p, i);
    }
    if let Some(c) = recolour {
        app.colour_selected_comments(ctx, c);
    }
    if let Some((p, i)) = app.views[index].session.comments.default_request.take() {
        app.make_comment_default(p, i);
    }
    if let Some((p, i, open)) = app.views[index].session.comments.attachment_request.take() {
        let doc = app.views[index].id;
        app.comment_attachment_action(ctx, doc, p, i, open);
    }
    if let Some((name, w)) = field_props {
        crate::prepare::dialog::open(app, &name, w, crate::prepare::FieldTab::General);
    }
    if let Some(FieldMenu::Duplicate(name)) = field_menu {
        crate::dialogs::duplicate_field::open(app, ctx, name);
    }
    if let Some((page, rect)) = app.views[index].session.links.open_new.take() {
        app.open_link_props(page, Some(rect), None);
    }
    if let Some((page, i)) = app.views[index].session.links.open_existing.take() {
        app.open_link_props(page, None, Some(i));
    }
    if std::mem::take(&mut app.views[index].session.redaction_refused) {
        app.notify_tr("Part of the selected text is on a page whose text could not be read, so nothing was marked for redaction. Mark that page with a box instead.");
    }
    if let Some((made_by, marks, head)) = app.views[index].session.pending_marks.take() {
        let author = app.comment_prefs.author.clone();
        let edits: Vec<pdfcraft_engine::Edit> = marks
            .into_iter()
            .map(|(page, quads)| {
                if made_by == QuickTool::Comment(comments::CommentTool::Highlight) {
                    // An area highlight: a highlight over the box.
                    let style = app.comment_prefs.style(comments::CommentTool::Highlight);
                    pdfcraft_engine::Edit::AddAnnotation(pdfcraft_engine::NewAnnotation {
                        page,
                        shape: pdfcraft_engine::Shape::TextMarkup { kind: pdfcraft_engine::Markup::Highlight, quads },
                        style,
                        contents: String::new(),
                        author: author.clone(),
                    })
                } else {
                    app.redact_prefs.mark(page, quads, &author)
                }
            })
            .collect();
        let edit = app.views[index].selection_marks_edit(edits, head);
        app.views[index].session.pending_edit = edit;
    }
    match image_action {
        Some(crate::edit_text_ui::ImageAction::Replace(page, index)) => app.replace_page_image_dialog(page, index),
        Some(crate::edit_text_ui::ImageAction::Save(page, index)) => app.save_page_image(page, index),
        Some(crate::edit_text_ui::ImageAction::Copy(page, index)) => {
            app.copy_page_image(page, index);
        }
        Some(crate::edit_text_ui::ImageAction::CopyFile(page, index)) => app.copy_page_image_file(page, index),
        Some(crate::edit_text_ui::ImageAction::Cut(page, image)) => {
            app.cut_page_image(ctx, page, image);
        }
        Some(crate::edit_text_ui::ImageAction::Paste(page, at)) => {
            if let Some(pt) = at {
                request_paste_at_page(app, ctx, index, page, pt);
            } else {
                app.paste_image(ctx, page, None);
            }
        }
        Some(crate::edit_text_ui::ImageAction::DrawFreehand) => {
            app.quick_tool = crate::QuickTool::Comment(crate::comments::CommentTool::Ink);
        }
        Some(crate::edit_text_ui::ImageAction::Redact(page, index)) => app.redact_page_image(ctx, page, index),
        Some(crate::edit_text_ui::ImageAction::Link(page, index)) => app.link_page_image(page, index),
        Some(crate::edit_text_ui::ImageAction::Recognize(page)) => app.recognize_text_on_page(ctx, page),
        Some(crate::edit_text_ui::ImageAction::AddComment(page, i)) => {
            if let Some(d) = app.session.get(app.views[index].id) {
                let images = d.page_images(page);
                if let Some(img) = images.get(i) {
                    let r = img.visible_rect.unwrap_or(img.rect);
                    let center = [(r[0] + r[2]) / 2.0, (r[1] + r[3]) / 2.0];
                    app.views[index].session.comments.context_at = Some((page, center));
                    app.views[index].session.comments.composer = Some(crate::comments::Composer {
                        page,
                        at: center,
                        kind: crate::comments::ComposerKind::Note,
                        text: String::new(),
                        focus: true,
                    });
                }
            }
        }
        None => {}
    }
    match page_action {
        Some(PageMenuAction::Copy) => {
            if let Some(sel) = app.views[index].session.image_selection.clone() {
                app.copy_page_image(sel.page, sel.index);
            } else if app.views[index].selected_text().is_some() || app.views[index].selection_waits_for_text().is_some() {
                app.execute(ctx, "edit.copy");
            } else if let Some((page, at)) = app.views[index].session.comments.context_at
                && let Some(doc) = app.session.get(app.views[index].id)
                && let Some(image) = doc.page_image_at(page, at)
            {
                app.copy_page_image(page, image);
            }
        }
        Some(PageMenuAction::Paste(page, at)) => {
            request_paste_at_page(app, ctx, index, page, at);
        }
        Some(PageMenuAction::PasteComments(page, at)) => {
            app.paste_comments(ctx, index, None, Some((page, at)));
        }
        // A quarter turn either way; the next pass makes the rotation one of the four and keeps the current page in
        // view.
        Some(PageMenuAction::RotateCCW) => {
            let view = &mut app.views[index].view;
            view.rotation = (view.rotation % 360 + 270) % 360;
        }
        Some(PageMenuAction::RotateCW) => {
            let view = &mut app.views[index].view;
            view.rotation = (view.rotation % 360 + 90) % 360;
        }
        Some(PageMenuAction::Print) => {
            app.open_print();
        }
        Some(PageMenuAction::SelectTool) => {
            app.quick_tool = QuickTool::Select;
        }
        Some(PageMenuAction::HandTool) => {
            app.quick_tool = QuickTool::Hand;
        }
        Some(PageMenuAction::MarqueeZoom) => {
            app.quick_tool = QuickTool::MarqueeZoom;
        }
        Some(PageMenuAction::AddComment(page, at)) => {
            app.views[index].session.comments.composer =
                Some(crate::comments::Composer { page, at, kind: crate::comments::ComposerKind::Note, text: String::new(), focus: true });
        }
        Some(PageMenuAction::FlattenComments) => {
            app.views[index].session.comments.select(None);
            app.views[index].session.pending_edit = Some(pdfcraft_engine::Edit::Flatten { comments: true, fields: false });
        }
        Some(PageMenuAction::AddBookmark) => {
            app.bookmark_action(ctx, crate::panels::BmAction::New);
        }
        Some(PageMenuAction::TakeSnapshot) => {
            app.quick_tool = QuickTool::Snapshot;
        }
        Some(PageMenuAction::EditPdf) => {
            app.quick_tool = QuickTool::EditText;
        }
        Some(PageMenuAction::AddText) => {
            app.quick_tool = QuickTool::AddText;
        }
        Some(PageMenuAction::AddImage) => {
            app.add_image_dialog();
        }
        Some(PageMenuAction::DeletePage(page)) => {
            app.views[index].session.pending_edit = Some(pdfcraft_engine::Edit::DeletePages { pages: vec![page] });
        }
        Some(PageMenuAction::ExportPdf) => {
            app.select_tool(ctx, "export");
        }
        Some(PageMenuAction::Find) => {
            app.views[index].open_find();
        }
        Some(PageMenuAction::DocProperties) => {
            app.open_properties(ctx, crate::PropsTab::Description);
        }
        None => {}
    }
    if let Some(initials) = open_signature {
        // PLAN: assumed `dialogs::signature::open(app, initials, editing: bool)` (pdf-hub-w3 gives
        // `signature::open(self, initials, editing)`, forms AP-4).
        crate::dialogs::signature::open(app, initials, false);
    }
    if open_boxes {
        // Set Page Boxes on the current page, with no draft written here.
        app.open_page_boxes(ctx, true);
    }
    if let Some((page, rect)) = open_table {
        let doc = app.views[index].id;
        crate::field_table_ui::open(app, doc, page, rect, &form, generation);
        // The window lasts while the form is prepared. Drawn with the tool picked but its panel
        // closed, the tool goes back to Select above, so the panel opens to keep it prepared.
        app.left = crate::LeftPanel::Tool("form");
        app.reveal_tools();
    }
    if let Some(n) = form_notice {
        match n {
            crate::forms_ui::FormNotice::Security => app.notify_tr("The document's security settings don't allow filling in form fields"),
            crate::forms_ui::FormNotice::ReadOnly(name) => app.notify_fmt("{name} is read-only", &[("name", &name)]),
            crate::forms_ui::FormNotice::NoAction(name) => app.notify_fmt("{name} has no action", &[("name", &name)]),
        }
    }
    if let Some((name, identity, action)) = app.views[index].session.forms.button.take() {
        run_button(app, ctx, index, &name, identity, action);
    }
    if copy_selected {
        app.execute(ctx, "edit.copy");
    }
    let copy_comments = std::mem::take(&mut app.views[index].session.comments.copy_request);
    if copy_comments {
        app.execute(ctx, "comment.copy");
    }
    if let Some(at) = app.views[index].session.comments.paste_request.take() {
        app.paste_comments(ctx, index, None, Some(at));
    }
    if let Some(notice) = app.views[index].session.comments.notice.take() {
        app.notify_tr(notice);
    }
    if let Some((source, file, destination)) = remote_open {
        app.open_remote_link(source, file, destination);
    }
}

pub(crate) mod page_box_drag;
#[cfg(test)]
mod sign_tab_tests;
#[cfg(test)]
mod tests;
pub(crate) use page_box_drag::PageBoxDrag;
pub(crate) mod page_menu;
pub(crate) mod page_select;
