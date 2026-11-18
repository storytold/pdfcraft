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
use craft::ui::canvas::{ContextRequest, Momentary, NavigationOptions, PageBorder, ToolHost};
use craft::ui::contract::Show as _;
use craft::ui::display::Text;
use craft::ui::list::OpenSet;
use craft::ui::primitives::TextureStatistics;
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
mod organize;
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

pub(crate) use area::SignTab;
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
pub(crate) use selection::ToolMarks;
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
    /// Transient keyboard owner for this document's Pages sidebar.
    pub(crate) pages_focus_id: Option<egui::Id>,
    /// Scale of the thumbnails in the Pages inspector, independent of document and grid zoom.
    thumbnail_zoom: f32,
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
    /// A queued Fill & Sign signature: select it after its edit succeeds, then leave placement.
    pub(crate) fill_signature_page: Option<usize>,
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
    /// Redact tool: the marks to add (page, quads) for the app to style, one per page (a text
    /// selection can cover several), added in one undo step.
    /// With the tool that made them, read when they are styled: a key handled later in the same
    /// frame (Escape back to Select) must not turn an area highlight into a redaction.
    /// The last field is the page the marks are followed up on (where a selection ended).
    pub pending_redaction: Option<(QuickTool, PageMarks, usize)>,
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
    /// What the last secondary press on a page went to, and on which page (`None` off the pages): the page's menu
    /// shows its entries when it has a menu and is the one selected, or else the page's own.
    pub(crate) menu_for: Option<(usize, Owner)>,
}

impl PageSession {
    /// The session of a newly opened document.
    fn new(info: &DocInfo, highlight_fields: bool) -> Self {
        Self {
            placed: Vec::new(),
            text: TextSelectState::default(),
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
            pending_redaction: None,
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
            menu_for: None,
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
        self.cancel_organize_press(ctx);
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
            pages_focus_id: None,
            thumbnail_zoom: 1.0,
            grid_reveal: None,
            bookmarks_open: OpenSet::default(),
            redactions_applied: 0,
            redactions_undone_after_save: 0,
            saved_redactions_cut_off: false,
            redactions_downloaded: 0,
            fill_signature_page: None,
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
        self.cancel_organize_press(ctx);
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
fn apply_notice(app: &mut PdfCraftApp, index: usize, notice: Option<Notice>) {
    match notice {
        #[cfg(not(target_arch = "wasm32"))]
        Some(Notice::Reload) => app.reload_request = app.views.get(index).map(|view| view.id),
        #[cfg(target_arch = "wasm32")]
        Some(Notice::Reload) => {}
        Some(Notice::Repairs) => app.open_properties(crate::PropsTab::Advanced),
        Some(Notice::Security) => app.open_properties(crate::PropsTab::Security),
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
        // PLAN: assumed `side_panels::set_panel_visible(app, panel: RightPanel, visible: bool)` (pdf-shell-w13,
        // pdf-chrome-w13 name no panel type).
        Some(Notice::Signatures) => crate::side_panels::set_panel_visible(app, RightPanel::Signatures, true),
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
fn close_requested_tab(app: &mut PdfCraftApp, index: usize) -> bool {
    let pending = app.views.get(index).filter(|v| v.close_requested).map(|v| v.view_key()).and_then(|key| app.tab_with_key(key));
    let Some(i) = pending else { return false };
    app.views[i].close_requested = false;
    app.request_close_tab(i);
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
fn finish_sign_requests(app: &mut PdfCraftApp, index: usize, mut tool: QuickTool, area_choice: Option<crate::zoom_snap::AreaChoice>) -> QuickTool {
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
            crate::zoom_snap::AreaChoice::Save => app.save_snapshot_image(index, page, area),
        }
    }
    // A signature rectangle was drawn, or an empty signature field clicked.
    let drawn = app.views[index].session.sign.drawn.take();
    if let (Some((page, rect)), QuickTool::SignArea { certify }) = (drawn, tool) {
        tool = QuickTool::Select;
        app.quick_tool = tool;
        app.start_signing(page, Some(rect), None, certify.then_some(2));
    }
    if let Some(field) = app.views[index].session.sign.field.take() {
        let signed = app.session.get(app.views[index].id).is_some_and(|d| d.signatures.iter().any(|s| s.field == field && s.signed));
        if signed {
            app.sig_open = Some(field);
            crate::side_panels::set_panel_visible(app, RightPanel::Signatures, true);
        } else {
            let page = app.views[index].view.current;
            app.start_signing(page, None, Some(field), None);
        }
    }
    tool
}

/// What the page body reads besides the session, settled before the page view is shown: the document, the tool and
/// the frame's facts. One per pass; the body never changes it.
struct PageBodyCx<'a> {
    doc: &'a pdfcraft_engine::Document,
    info: &'a DocInfo,
    /// The tool the page sees ([`page_tool`] keeps the rule that copied comments on the pointer make it Select).
    tool: QuickTool,
    /// The tool the user picked, before that rule.
    chosen: QuickTool,
    /// The view's rotation this pass: the page view holds the view state while the body runs.
    rotation: u16,
    /// The view's current page this pass.
    current: usize,
    /// Full screen: the page, display and rotation it shows, captured before the page view holds the view state;
    /// `None` outside full screen.
    present_at: Option<presenter::PresentAt>,
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
    /// What the page tool reads besides the session; its `pages` is filled from the body each run.
    tools: page_tools::PageToolCx<'a>,
    /// Animation effects apply (the preference after Reduce Motion), for full screen's transitions.
    animates: bool,
    prefs: &'a comments::CommentPrefs,
    /// The user's Redaction Tool Properties hover colour and opacity, a data colour, not a theme role (L-90).
    redact_hover: ([f64; 3], f64),
    form: &'a Arc<Vec<pdfcraft_engine::FormField>>,
    units: crate::units::Unit,
    find_marks: &'a [TextMark],
    table_preview: Option<&'a crate::field_table_ui::Preview>,
}

/// What a run of the page body reports for the rest of the pass. The body runs twice in a pass (a measuring run with
/// no tool pass, then the real one): each run starts from the default, so the real run's report is the one read.
#[derive(Default)]
struct BodyOut {
    /// The canvas view the page view showed this pass.
    shown: Option<CanvasView>,
    /// The view a tool or the canvas set this pass, for [`PageViewOutput::adopt`].
    view: Option<CanvasView>,
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
}

/// The part list of a pass, in paint order: pictures, paragraphs and added items, links and text links (only under
/// Select and Link), comments (only under Select), fields, sign tabs (painted over their fields), then prepare fields,
/// selected objects and the frame band. A later part outranks an earlier one of the same layer, so this order is the
/// stacking order hit testing ranks on.
fn page_parts_of(
    ui: &mut egui::Ui,
    body: &PageBody<'_>,
    session: &PageSession,
    cx: &PageBodyCx<'_>,
) -> (Vec<PagePart>, Vec<(usize, PageXform, Vec<SignTab>)>) {
    let mut parts = Vec::new();
    // PLAN: assumed `edit_text_ui::picture_parts(out, session, doc, pages, rotation)`, `edit_text_ui::page_parts` and
    // `content_ui::page_parts` with the same shape (pdf-edit-w7, w8, w9 give no argument list).
    if cx.editing_content {
        crate::edit_text_ui::picture_parts(&mut parts, session, cx.doc, body.pages, cx.rotation);
        crate::edit_text_ui::page_parts(&mut parts, session, cx.doc, body.pages, cx.rotation);
        crate::content_ui::page_parts(&mut parts, session, cx.doc, body.pages, cx.rotation);
    }
    if matches!(cx.tool, QuickTool::Select | QuickTool::Link) {
        links_of(session, cx, body).parts(&mut parts);
    }
    if cx.tool == QuickTool::Select && !cx.comments_hidden {
        crate::comments::parts(&mut parts, session, cx.info, body.pages, cx.rotation, cx.unobstructed);
    }
    if !cx.preparing {
        // PLAN: assumed `forms_ui::field_parts(out, session, doc, form, pages, rotation, tool)` (pdf-forms-w7 AP-5).
        crate::forms_ui::field_parts(&mut parts, session, cx.doc, cx.form, body.pages, cx.rotation, cx.tool);
    }
    // The sign tabs of each page, after the fields: a tab is painted over its field.
    // PLAN: `can_fill` is `can_modify` (pdf-canvas-w15 does not say which right a click that signs needs).
    let shown = cx.tool == QuickTool::Select && !cx.preparing;
    let mut tabs = Vec::new();
    for placed in body.pages {
        let size = cx.sizes.get(placed.page).copied().unwrap_or_default();
        let xf = PageXform::of(placed, cx.rotation, size);
        let overlay = overlay_parts(ui, &xf, cx.doc, placed.page, cx.can_modify, shown);
        parts.extend(overlay.parts);
        tabs.push((placed.page, xf, overlay.tabs));
    }
    if cx.preparing {
        // PLAN: assumed `prepare::tool::page_parts(out, session, form, info, pages, rotation)` (pdf-forms-w9).
        crate::prepare::tool::page_parts(&mut parts, session, cx.form, cx.info, body.pages, cx.rotation);
    }
    if cx.tool == QuickTool::EditText && cx.can_modify {
        // PLAN: assumed `object_ui::page_parts(out, session, pages, rotation)` for the selected objects and the
        // frame band (pdf-edit-w10).
        crate::object_ui::page_parts(&mut parts, session, body.pages, cx.rotation);
    }
    (parts, tabs)
}

/// The links of the pages: followed under Select, edited under the Link tool.
fn links_of<'a>(session: &'a PageSession, cx: &'a PageBodyCx<'_>, body: &'a PageBody<'_>) -> PageLinks<'a> {
    let mode = if cx.tool == QuickTool::Link { LinkMode::Edit } else { LinkMode::Follow };
    // PLAN: assumed `PageLinks::new(mode, session, info, doc.links(), pages, rotation).text_links(on).tips(on)`
    // (pdf-canvas-w15 names `parts` and `read` only).
    PageLinks::new(mode, session, cx.info, cx.doc.links(), body.pages, cx.rotation).text_links(cx.text_links).tips(cx.unobstructed)
}

/// Whether `phase` is the first pass of a press of `button`.
fn pressed_by(phase: &PressPhase, button: PointerButton) -> bool {
    matches!(phase, PressPhase::Down { start, .. } if start.button == button)
}

/// What a press on a part does, routed by its owner.
fn route_press(session: &mut PageSession, press: &PagePress, cx: &PageBodyCx<'_>, body: &PageBody<'_>, out: &mut BodyOut) {
    if pressed_by(&press.phase, PointerButton::Secondary) {
        // The page's menu reads what the secondary press went to.
        session.menu_for = Some((press.page, press.owner.clone()));
        return;
    }
    let clicked = matches!(press.phase, PressPhase::Release { click: true, .. });
    match &press.owner {
        Owner::Link { .. } | Owner::TextLink { .. } => {
            // Read before acting: the links borrow the session.
            let event = links_of(session, cx, body).read(press);
            if let Some(event) = event {
                // A click on a link is not a picture pick (pdf-canvas-w12 note 5).
                session.pending_pick = None;
                match event {
                    LinkEvent::Follow { target, javascript } => out.link = Some((target, javascript)),
                    LinkEvent::Select { .. } | LinkEvent::Context { .. } => crate::link_ui::link_edit_press(&mut session.links, &event),
                }
            }
        }
        // PLAN: assumed `comments::tool::part_pressed(session, info, press)` (pdf-comments-w8).
        Owner::Comment { .. } => crate::comments::part_pressed(session, cx.info, press),
        // PLAN: assumed `forms_ui::field_press(session, doc, form, press)` (pdf-forms-w5).
        Owner::Field { .. } => crate::forms_ui::field_press(session, cx.doc, cx.form, press),
        Owner::SignTab { field } => {
            if clicked && let Some(field) = cx.form.get(*field) {
                session.sign.field = Some(field.name.clone());
            }
        }
        // PLAN: assumed `prepare::tool::field_press(session, form, press)` (pdf-forms-w9).
        Owner::PrepareField { .. } => crate::prepare::tool::field_press(session, cx.form, press),
        // PLAN: assumed `edit_text_ui::paragraph_press(session, doc, press)` and `content_ui::added_press(session,
        // doc, press)` (pdf-edit-w7, w8).
        Owner::Paragraph { .. } => crate::edit_text_ui::paragraph_press(session, cx.doc, press),
        Owner::Added { .. } => crate::content_ui::added_press(session, cx.doc, press),
        // PLAN: assumed `edit_text_ui::picture_press(session, doc, press)` and `object_ui::objects_press(session,
        // press)` (pdf-edit-w9, w10).
        Owner::Picture { .. } => crate::edit_text_ui::picture_press(session, cx.doc, press),
        Owner::Objects => crate::object_ui::objects_press(session, press),
    }
}

/// The page body, run by the page view on every run of its canvas body. Its order is the paint order, and with it
/// the stacking order hit testing ranks on: the placed pages recorded, the parts registered, the tool host, then the
/// overlays.
fn page_body(ui: &mut egui::Ui, body: &PageBody<'_>, session: &mut PageSession, momentary: &mut Momentary, cx: &PageBodyCx<'_>, out: &mut BodyOut) {
    *out = BodyOut { shown: Some(body.view), ..BodyOut::default() };
    // 1. This pass's placement, before anything that maps through it.
    session.placed.clear();
    session.placed.extend_from_slice(body.pages);

    // 2-3. Every part on the pages, in one control: hit priority ranks parts inside one control only.
    let (parts, tabs) = page_parts_of(ui, body, session, cx);
    // PLAN: assumed `forms_ui::focus_owner(&FormView) -> Option<Owner>` for the focused field (pdf-forms-w7 AP-6).
    let focus = crate::forms_ui::focus_owner(&session.forms);
    let parts_out = PageParts::new(&parts).focus(focus).show_at(ui, body.rect);
    if let Some(press) = &parts_out.press {
        // Any press on a page ends Select All of the document, on a link too.
        if matches!(press.phase, PressPhase::Down { .. }) {
            session.all_text = false;
        }
        route_press(session, press, cx, body, out);
    }
    if let Some((owner, page)) = &parts_out.focused {
        // Focus arrived on a field (Tab, a request or a click): the field opens, a text field with its text selected.
        // PLAN: assumed `forms_ui::field_focused(session, doc, form, owner, page)` (pdf-forms-w5 AP-6).
        crate::forms_ui::field_focused(session, cx.doc, cx.form, owner, *page);
    }
    if let Some((owner, key)) = &parts_out.key {
        // PLAN: assumed `forms_ui::field_key(session, doc, form, owner, key)` (pdf-forms-w5).
        crate::forms_ui::field_key(session, cx.doc, cx.form, owner, key);
    }

    // 4. The tool host, in the real run only: a measuring run has no tool pass.
    if let Some(pass) = body.pass {
        if matches!(pass.press.phase, PressPhase::Down { .. }) {
            // A press nobody's part took, on the page or the gap between pages, ends Select All of the document.
            session.all_text = false;
        }
        let tool_cx = PageToolCx { pages: body.pages, ..cx.tools };
        let mut bench = page_tools::ToolBench::default();
        let mut fill = None;
        let mut tool = page_tool(cx.chosen, tool_cx, &mut *session, &mut bench, &mut fill);
        let reply = ToolHost::new().momentary(momentary).run(ui, pass, &mut tool);
        let mut finish = tool.finish();
        finish.apply(fill, session, &tool_cx);
        if let Some(why) = finish.refused.take() {
            session.objects.notice = Some(why.into());
        }
        out.view = reply.view;
        out.context = reply.context;
        out.back_to_select = finish.back_to_select;
        out.snapshot = finish.snapshot;
        out.open_boxes = finish.open_boxes;
        out.open_signature = finish.open_signature;
        let outcome = session.prepare.take_outcome();
        out.back_to_select |= outcome.placed;
        out.prepare = Some(outcome);
    }

    // 5. The overlays.
    for (page, xf, page_tabs) in &tabs {
        PageOverlays::new(*page, *xf, cx.doc, page_tabs, &session.compare_marks, &mut session.flash)
            .highlight_fields(session.highlight_fields)
            .show_at(ui, body.rect);
    }
    if cx.editing_content {
        // PLAN: assumed `(ui, body, session, doc, rotation)` for the edit controls (pdf-edit-w7..w11).
        crate::edit_text_ui::picture_controls(ui, body, session, cx.doc, cx.rotation);
        crate::object_ui::object_controls(ui, body, session, cx.doc, cx.rotation);
        crate::content_ui::content_controls(ui, body, session, cx.doc, cx.rotation);
        crate::edit_text_ui::paragraph_controls(ui, body, session, cx.doc, cx.rotation);
    }
    if cx.tool == QuickTool::Link {
        crate::link_ui::link_outlines(ui, body, session, cx.doc, cx.rotation);
    }
    if !cx.comments_hidden {
        // PLAN: assumed `comments::overlay::show(ui, body, session, info, prefs, tool, redact_hover)` (pdf-comments-w7,
        // L-90: the redaction hover is the user's setting, passed as a data colour).
        crate::comments::show(ui, body, session, cx.info, cx.prefs, cx.tool, cx.redact_hover);
    }
    if cx.calc_order {
        let hovered = match &parts_out.hovered {
            Some(Owner::Field { field, .. } | Owner::PrepareField { field, .. }) => cx.form.get(*field).map(crate::calc_order_ui::Focus::of_field),
            _ => None,
        };
        let selected = session.prepare.selected.as_ref().map(|(name, _)| name.as_str());
        // PLAN: assumed `CalcOrderOverlay::new(&CalcOrderView, pages, rotation, generation, selected, hovered,
        // viewport)` (pdf-forms-w12 gives `new(.., viewport: body.rect)`).
        crate::calc_order_ui::CalcOrderOverlay::new(
            &session.calc_order,
            body.pages,
            cx.rotation,
            cx.doc.edit_generation(),
            selected,
            hovered,
            body.rect,
        )
        .show_at(ui, body.rect);
    }
    if let Some(preview) = cx.table_preview {
        // PLAN: assumed `field_table_ui::TablePreview::new(&Preview, pages, rotation)` (pdf-forms-w11).
        crate::field_table_ui::TablePreview::new(preview, body.pages, cx.rotation).show_at(ui, body.rect);
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
    image_pick::show_picked(ui, body, session, cx.rotation, cx.info);
    if let Some(at) = cx.present_at {
        // PLAN: `PresentFrame::hide_pointer` asks for `CursorPriority::Hidden`; the page body has no `BodyCx` to claim
        // it through, so the pointer is not hidden yet (pdf-canvas-w18).
        let (now, pointer) = ui.input(|i| (i.time, i.pointer.hover_pos()));
        let frame = presenter::frame(&mut session.presenter, body, at, cx.info, ui.painter(), now, cx.animates, pointer);
        out.turn = frame.step;
    }
    if cx.tool == QuickTool::EnhanceCamera {
        crate::camera_ui::show_quad(ui, body, session, cx.can_modify);
        crate::camera_ui::bar(ui, body, session, cx.can_modify);
    }
    if cx.chosen == QuickTool::Snapshot {
        let sizes = cx.sizes;
        out.area_choice = crate::zoom_snap::area_bar(ui, body.rect, body.pages, session.area_pick, cx.rotation, |page| {
            sizes.get(page).copied().unwrap_or_default()
        });
    }
    if cx.unobstructed && matches!(cx.tool, QuickTool::Select | QuickTool::ColumnSelect) {
        // PLAN: assumed `selection_bar(ui, body, session, info, prefs, may_mark, can_copy)`, the old
        // `selection_actions` arguments after `body` (pdf-canvas-w12 note 4 gives `(ui, body, view, ..)`).
        out.copy_selected = selection::selection_bar(ui, body, session, cx.info, cx.prefs, cx.allowed && !cx.comments_hidden, cx.can_copy);
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
    let search_open = app.right == Some(crate::RightPanel::Search);
    if let Some(f) = app.views.get_mut(index).and_then(|v| v.find.as_mut())
        && f.in_panel
        && !search_open
    {
        f.in_panel = false;
    }
    if close_requested_tab(app, index) {
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
    let notice = notices(view, doc, secured, repaired, lock_offer, crate::sign_ui::banner(&doc.signatures), ui);
    apply_notice(app, index, notice);
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
    // PLAN: assumed `side_panels::panel_visible(app, panel) -> bool` for the left panel's visibility (`left_open` is
    // not a field after L-136; pdf-shell-w13 names only the setter).
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
    let border = if view.view.page_gaps { PageBorder::DropShadow } else { PageBorder::None };
    let keep = view.session.presenter.keep_pages();
    if view.momentary_options != navigation {
        view.momentary = Momentary::new(navigation);
        view.momentary_options = navigation;
    }
    let mut body_out = BodyOut::default();
    let DocView { view: state, session, momentary, revisions, reports, page_sizes, .. } = &mut *view;
    // PLAN: assumed app fields (pdf-hub-w1, pdf-forms-w5): `host_scope: HostScope`, `wake: Wake`, `fill_text_size`,
    // `fill_text_color`, `fill_text_border`, `fill_signature`, `fill_initials`, `fill_date_text`, `stamp_by_line`,
    // `custom_stamp_choice: Option<(CustomStamp, Option<String>)>`, `animation_effects` and `motion`.
    let tools = page_tools::PageToolCx {
        doc,
        info,
        form: form.as_slice(),
        pages: &[],
        sizes: &page_sizes[..],
        rotation: state.rotation,
        allowed,
        can_modify,
        can_crop: can_modify,
        preparing,
        presenting,
        units: app.units,
        navigation,
        host: &app.host_scope,
        wake: &app.wake,
        author: &app.comment_prefs.author,
        date_text: &app.fill_date_text,
        fill: page_tools::FillPrefs {
            text_size: app.fill_text_size,
            text_color: app.fill_text_color,
            text_border: app.fill_text_border,
            signature: app.fill_signature.as_ref(),
            initials: app.fill_initials.as_ref(),
        },
        by_line: &app.stamp_by_line,
        custom_stamp: app.custom_stamp_choice.as_ref().map(|(stamp, line)| (stamp, line.as_deref())),
    };
    let cx = PageBodyCx {
        doc,
        info,
        tool,
        chosen,
        rotation: state.rotation,
        current: state.current,
        sizes: &page_sizes[..],
        tools,
        animates: app.animation_effects.animates(app.motion.reduced),
        present_at,
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
        units: app.units,
        find_marks: &find_marks,
        table_preview: table_preview.as_ref(),
    };
    let source = DocPages::new(&app.session, doc_id).rotated(state.rotation).revisions(revisions).reports(reports);
    // A modal over the pages blocks the canvas's keys and presses in the framework, so the view needs no gate here.
    let out = PageView::new(tl!("Pages"), &source, state, DataColor::new(paper)).navigation(navigation).border(border).keep(&keep).show_at(
        ui,
        rect,
        |ui, body| page_body(ui, body, session, momentary, &cx, &mut body_out),
    );
    if let Some(canvas) = &body_out.view {
        out.adopt(&mut view.view, canvas);
    }
    if out.toggle_white_space {
        view.view.hide_white_space = !view.view.hide_white_space;
    }
    // A turn the presenter's display time or a full-screen key asked for, then a click's turn once the
    // double-click time has passed with no object clicked and no selection made (#2070).
    let now = ui.input(|i| i.time);
    let has_selection = view.session.text.selection.is_some();
    let click = (presenting && matches!(out.canvas.press.phase, PressPhase::Release { click: true, .. }))
        .then(|| presenter::PresentClick { forward: !out.canvas.press.mods.shift, clicks: if out.canvas.press.double_pressed() { 2 } else { 1 } });
    let delay = ui.ctx().options(|o| o.input_options.max_double_click_delay);
    let cancels = body_out.link.is_some() || has_selection;
    let click_turn = if presenting { view.session.presenter.click_turn(ui.ctx(), click, cancels, now, delay) } else { None };
    if let Some(forward) = body_out.turn.or(click_turn) {
        view.view.step(forward);
    }
    // The records readers between frames take; no pass reads them back.
    view.textures = out.textures;
    view.spacing = Some(out.spacing);
    view.viewport = rect;
    view.canvas = body_out.view.or(body_out.shown);
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
    // Whether the clipboard holds a picture, read only when this pass opens the page menu (a clipboard read is slow).
    // PLAN: a menu left open from an earlier pass keeps its row only if `page_menu` reads the clipboard itself.
    let can_paste_image = body_out.context.is_some() && app.can_paste_image();
    let view = &mut app.views[index];
    // The in-place editors and comment windows over the pages, in their old order, mapped through this pass's
    // placement.
    let session = &mut view.session;
    // PLAN: assumed the comments A-4 shapes `composer(ui, session, info, prefs)` and `note_popups(ui, session, info,
    // prefs, hidden, allowed)` (note 18 names `composer(ui, view, info, prefs)` and `note_popups(ui, ..)`).
    if let Some(e) = crate::comments::composer(ui, session, info, &app.comment_prefs) {
        session.pending_edit = Some(e);
    }
    if let Some(e) = crate::comments::note_popups(ui, session, info, &app.comment_prefs, doc.comments_hidden(), allowed) {
        // A click on the composer's Post also takes the focus from a pop-up's box, saving what
        // was typed there: both edits land in this pass, so both are kept, in one batch.
        session.pending_edit = Some(match session.pending_edit.take() {
            Some(first) => Edit::Batch { label: "Edit comment".into(), edits: vec![first, e] },
            None => e,
        });
    }
    let text_box = crate::forms_ui::text_box(doc, session, &form);
    // PLAN: assumed `forms_ui::editor::show(ui, session, info, form, text_box, today)` and
    // `fill_sign::type_box::show(ui, session, info, author)` (pdf-forms-w5, w9).
    if let Some(e) = crate::forms_ui::editor::show(ui, session, info, &form, text_box.as_ref(), app.session.today()) {
        session.pending_edit = Some(e);
    }
    if let Some(e) = crate::fill_sign::type_box::show(ui, session, info, &app.comment_prefs.author) {
        session.pending_edit = Some(e);
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
    // The page's menu, opened by the context request of this pass or still open from an earlier one.
    let menu_for = view.session.menu_for.as_ref().map(|(_, owner)| owner.clone());
    let pick = page_menu::show(
        ui,
        body_out.context.as_ref(),
        &out.canvas.response,
        menu_for,
        &mut app.views[index],
        &page_menu::PageMenuCx {
            doc,
            info,
            prefs: &app.comment_prefs,
            rights: comments::MenuRights { allowed, can_paste: app.comment_clipboard.is_some(), can_modify },
            recent: &app.recent_colours,
            tool,
            preparing,
            form: &form,
            can_copy: doc.allows_copying(),
            can_paste_image,
        },
    );
    let mut open_props: Option<(usize, usize)> = None;
    let mut recolour: Option<pdfcraft_engine::Rgb> = None;
    let mut image_action = pick.image;
    match pick.canvas {
        Some(comments::CanvasAction::Edit(e)) => app.views[index].session.pending_edit = Some(*e),
        Some(comments::CanvasAction::OpenComments) => {
            app.comments_panel_closed = false;
            crate::side_panels::set_panel_visible(app, RightPanel::Comments, true);
        }
        Some(comments::CanvasAction::Properties(p, i)) => open_props = Some((p, i)),
        Some(comments::CanvasAction::Colour(c)) => recolour = Some(c),
        Some(comments::CanvasAction::CopyImage(p, i)) => image_action = Some(crate::edit_text_ui::ImageAction::Copy(p, i)),
        Some(comments::CanvasAction::CopyImageFile(p, i)) => image_action = Some(crate::edit_text_ui::ImageAction::CopyFile(p, i)),
        None => {}
    }
    if let Some(why) = refused {
        app.notify_error(why);
    }
    if let Some((page, area)) = body_out.snapshot {
        app.finish_marquee(index, (page, area));
    }
    let remote_open = follow_link(app, index, clicked_link);
    if let Some((page, link)) = javascript_link {
        app.run_link_script(doc_id, page, link);
    }
    let tool = finish_sign_requests(app, index, tool, body_out.area_choice);
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
    apply_frame_requests(app, index, requests);
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
fn apply_frame_requests(app: &mut PdfCraftApp, index: usize, requests: FrameRequests) {
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
        app.open_comment_props(p, i);
    }
    if let Some(c) = recolour {
        app.colour_selected_comments(c);
    }
    if let Some((p, i)) = app.views[index].session.comments.default_request.take() {
        app.make_comment_default(p, i);
    }
    if let Some((p, i, open)) = app.views[index].session.comments.attachment_request.take() {
        let doc = app.views[index].id;
        app.comment_attachment_action(doc, p, i, open);
    }
    if let Some((name, w)) = field_props {
        crate::prepare::dialog::open(app, &name, w, crate::prepare::FieldTab::General);
    }
    if let Some(FieldMenu::Duplicate(name)) = field_menu {
        crate::dialogs::duplicate_field::open(app, name);
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
    if let Some((made_by, marks, head)) = app.views[index].session.pending_redaction.take() {
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
            app.cut_page_image(page, image);
        }
        Some(crate::edit_text_ui::ImageAction::Paste(page, at)) => {
            if let Some(pt) = at {
                app.paste_image_at(page, pt);
            } else {
                app.paste_image(page, None);
            }
        }
        Some(crate::edit_text_ui::ImageAction::DrawFreehand) => {
            app.quick_tool = crate::QuickTool::Comment(crate::comments::CommentTool::Ink);
        }
        Some(crate::edit_text_ui::ImageAction::Redact(page, index)) => app.redact_page_image(page, index),
        Some(crate::edit_text_ui::ImageAction::Link(page, index)) => app.link_page_image(page, index),
        Some(crate::edit_text_ui::ImageAction::Recognize(page)) => app.recognize_text_on_page(page),
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
                app.execute("edit.copy");
            } else if let Some((page, at)) = app.views[index].session.comments.context_at
                && let Some(doc) = app.session.get(app.views[index].id)
                && let Some(image) = doc.page_image_at(page, at)
            {
                app.copy_page_image(page, image);
            }
        }
        Some(PageMenuAction::Paste(page, at)) => {
            app.paste_image_at(page, at);
        }
        Some(PageMenuAction::PasteComments(page, at)) => {
            app.paste_comments(index, None, Some((page, at)));
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
            app.bookmark_action(crate::panels::BmAction::New);
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
            app.select_tool("export");
        }
        Some(PageMenuAction::Find) => {
            app.views[index].open_find();
        }
        Some(PageMenuAction::DocProperties) => {
            app.open_properties(crate::PropsTab::Description);
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
        app.open_page_boxes(true);
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
        run_button(app, index, &name, identity, action);
    }
    if copy_selected {
        app.execute("edit.copy");
    }
    let copy_comments = std::mem::take(&mut app.views[index].session.comments.copy_request);
    if copy_comments {
        app.execute("comment.copy");
    }
    if let Some(at) = app.views[index].session.comments.paste_request.take() {
        app.paste_comments(index, None, Some(at));
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
