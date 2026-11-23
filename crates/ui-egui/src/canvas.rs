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
    /// Full screen fills the screen with one page (or spread) at a time, edge to edge (the
    /// default); off, it keeps the page display it was entered from. Set from the app's
    /// preference before full screen is entered.
    pub full_screen_fill: bool,
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
            full_screen_fill: true,
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
        fresh.full_screen_fill = self.full_screen_fill;
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
        v.full_screen_fill = self.full_screen_fill;
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

    let preparing = app.is_preparing();
    // Prepare a form ▸ Calculation order: the order's steps and links on the page.
    let calc_order = preparing && crate::side_panels::panel_visible(app, crate::LeftPanel::CalculationOrder);
    // Edit a PDF: added text and images can be selected, moved and edited.
    let editing_content =
        crate::side_panels::panel_visible(app, crate::LeftPanel::Tool("edit")) || matches!(app.quick_tool, QuickTool::AddText | QuickTool::EditText);
    let chosen = app.quick_tool;
    let navigation = app.navigation_options();
    let presenting = app.views[index].presenting();
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
    let present_at = presenting.then_some(presenter::PresentAt {
        current: view.view.current,
        display: view.view.display,
        continuous: view.view.continuous,
        rotation: view.view.rotation,
    });
    let veiled = page_overlays::veiled(view);
    // Shadows only with the preference on, and only between gaps: they would darken the page before when pages touch.
    let border = if view.view.page_gaps && app.show_page_edge_shadows { PageBorder::DropShadow } else { PageBorder::None };
    // Full screen filling the screen: the page alone on black, edge to edge (#576).
    let fill_screen = view.fills_screen();
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
                .fill_screen(fill_screen)
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
    let mut image_action = pick.image;
    match pick.canvas {
        Some(comments::CanvasAction::Edit(e)) => app.views[index].session.pending_edit = Some(*e),
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
