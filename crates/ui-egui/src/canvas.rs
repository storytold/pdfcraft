//! The document area: page layout, zoom, render scheduling, overlays (links, annotation hovers,
//! form-field highlights), and the organize-pages grid.
//!
//! Page images come from the engine's render pool as whole-page rasters at the current zoom;
//! stale rasters are shown stretched until the sharp one arrives (tiling comes in M3.3).

use std::collections::{BTreeSet, HashMap, HashSet};
use std::ops::Range;
use std::sync::Arc;

use egui::{Align2, Color32, CornerRadius, Pos2, Rect, Sense, Stroke, TextureHandle, TextureOptions, Vec2, pos2, vec2};
use pdfcraft_engine::{DocId, Edit};
use pdfcraft_render::{DestView, DocInfo, LayerOp, LinkTarget, PRINT_TAG, PageText, RenderPool, RenderRequest, RequestKind, Tile, device_pixels};

use crate::theme::{self, Tokens};
use crate::{PdfCraftApp, QuickTool, RightPanel, comments, icons, widgets};

/// Logical pixels per PDF point at 100% (96 dpi, like browsers).
pub const PT: f32 = 96.0 / 72.0;
const GAP: f32 = 14.0;
const MARGIN: f32 = 28.0;
/// Horizontal gutter that keeps pages clear of the floating quick-action bar.
const SIDE: f32 = 70.0;
const THUMB_TAG: u64 = 1 << 63;
const TEXT_TAG: u64 = 1 << 62;
/// A page rendered for the organize grid at the size it is drawn there.
const GRID_TAG: u64 = 1 << 61;
/// Preview rasters tagged with this bit are converted to luminance on receipt, matching the
/// monochrome print requested by Print ▸ Print in grayscale. It is a bit of its own: the
/// renderer's print bit ([`PRINT_TAG`]) is carried by every print preview, so sharing it would
/// turn colour previews into monochrome ones too.
const GRAYSCALE_TAG: u64 = 1 << 59;
/// The tag of a raster that is out of date (shown until its replacement arrives).
const STALE_TAG: u64 = u64::MAX;
/// Pages whose raster would exceed this many device pixels on a side are drawn in tiles.
pub(crate) const TILE_THRESHOLD: f32 = 4096.0;
/// Base device-pixel side of a page tile. The actual side grows with the zoom
/// ([`tile_side`]), so a zoomed viewport is covered by a few tiles instead of many.
const TILE: u32 = 1024;
/// Longest side of the backdrop drawn under tiles, and of a live page preview (signature drag).
pub(crate) const BASE_SIDE: f32 = 2048.0;
/// Thumbnail slot before density scaling. The pages panel draws at most 150 logical points and
/// the organize grid about 146; when the byte budget allows, density reaches the screen so those
/// images are sampled down instead of stretched.
const THUMB_W: f32 = 240.0;
const THUMB_H: f32 = 320.0;
const THUMB_BYTES: usize = 24 * 1024 * 1024;
const PAGE_BYTES: usize = 128 * 1024 * 1024;
const TILE_BYTES: usize = 128 * 1024 * 1024;
const UPLOAD_BYTES_PER_FRAME: usize = 16 * 1024 * 1024;
const RESULTS_PER_FRAME: usize = 8;
/// A defensive ceiling for unusually large viewports / n-up previews.
const MAX_THUMB_DEMAND: usize = 256;
/// At most this many whole-page rasters are cached beyond the ones on screen.
const MAX_CACHED_PAGES: usize = 24;
/// At most this many text layers are cached beyond visible, selected and matched pages.
/// Text layers are glyph structs, far smaller than rasters; deliberately independent of
/// `MAX_CACHED_PAGES` so raster tuning never silently retunes text memory.
const MAX_CACHED_TEXT_PAGES: usize = 24;

fn rgba_bytes(width: usize, height: usize) -> usize {
    width.saturating_mul(height).saturating_mul(4)
}

fn texture_bytes(tex: &TextureHandle) -> usize {
    let [w, h] = tex.size();
    rgba_bytes(w, h)
}

/// Visible grid rows, plus one adjacent row in either direction. Work is independent of
/// document length, including after a large scroll jump.
pub(crate) fn thumbnail_rows(top: f32, bottom: f32, row_height: f32, rows: usize) -> Range<usize> {
    let first = (top.max(0.0) / row_height).floor() as usize;
    let end = (bottom.max(0.0) / row_height).ceil() as usize;
    first.saturating_sub(1).min(rows)..end.saturating_add(1).min(rows)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Fit {
    Width,
    Page,
    Height,
    None,
}

/// The page display as it was before full screen, put back on leaving it
/// ([`DocView::enter_presentation`]).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SavedDisplay {
    pub layout: PageLayout,
    pub continuous: bool,
    pub cover: bool,
    pub fit: Fit,
    pub zoom: f32,
}

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
}

impl Default for ViewDefaults {
    fn default() -> Self {
        // Single page, scrolling continuously at fit width: the earlier "continuous" default.
        Self { layout: PageLayout::Single, continuous: true, show_page_gaps: true, fit: Fit::Width, zoom: 1.0, highlight_fields: true }
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

/// The find bar (⌘F): query, matches across the document, current match.
#[derive(Default)]
pub struct Find {
    pub query: String,
    /// (page, glyph range) in document order.
    pub matches: Vec<(usize, Range<usize>)>,
    pub current: Option<usize>,
    pub focus: bool,
    pub case_query: String,
    /// Find options (Acrobat's: case-sensitive, whole words only).
    pub case_sensitive: bool,
    pub whole_words: bool,
    /// Shown in the Search panel (Advanced Search) instead of the find bar.
    pub in_panel: bool,
}

/// A text selection on one page.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Selection {
    page: usize,
    span: Span,
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum Span {
    /// Reading order, between two glyph indices.
    Run { anchor: usize, head: usize },
    /// Column select (#740): the glyphs inside a rectangle in page view space, drawn from the
    /// corner the drag started at (`rect[0..2]`) to the pointer (`rect[2..4]`).
    Column { rect: [f32; 4] },
}

impl Selection {
    fn run(page: usize, anchor: usize, head: usize) -> Self {
        Self { page, span: Span::Run { anchor, head } }
    }

    fn range(anchor: usize, head: usize) -> Range<usize> {
        anchor.min(head)..anchor.max(head).saturating_add(1)
    }

    /// One rectangle per selected piece of a line, in view space (highlighting, markup quads).
    fn rects(&self, text: &PageText) -> Vec<[f32; 4]> {
        match self.span {
            Span::Run { anchor, head } => text.line_rects(Self::range(anchor, head)),
            Span::Column { rect } => text.glyph_rects(&text.glyphs_in(rect)),
        }
    }

    /// The selected text, as Copy puts it on the clipboard.
    fn text(&self, text: &PageText) -> String {
        match self.span {
            Span::Run { anchor, head } => text.text_of(Self::range(anchor, head)),
            Span::Column { rect } => text.column_text(&text.glyphs_in(rect)),
        }
    }
}

/// The glyphs `clicks` quick clicks select around glyph `a`: two the word, three the line, more
/// the page.
fn click_unit(text: &PageText, clicks: u32, a: usize) -> Option<(usize, usize)> {
    match clicks {
        2 => text.word_at(a),
        3 => text.line_at(a),
        _ => text.glyphs.len().checked_sub(1).map(|last| (0, last)),
    }
}

struct PageTex {
    tag: u64,
    tex: TextureHandle,
}

/// Retained RGBA texture accounting for automation and performance regressions. These
/// surface bytes exclude driver overhead and temporary upload buffers; they are not RSS.
#[derive(Clone, Copy, Debug, Default, serde::Serialize)]
pub struct RasterMemory {
    pub thumbnail_count: usize,
    pub thumbnail_bytes: usize,
    pub page_count: usize,
    pub page_bytes: usize,
    pub tile_count: usize,
    pub tile_bytes: usize,
    pub thumbnail_demand: usize,
    /// Last submitted missing results; workers may have completed some since submission.
    pub pending_requests: usize,
}

/// A historical snapshot opened from another document: View signed version opens the bytes
/// a signature covers, View revision an earlier incremental revision. Repeats reuse the tab,
/// and the tab and notice bar name what it is, instead of piling up identical tabs.
/// What a historical snapshot shows: the bytes a signature covers, or an earlier
/// incremental revision.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SnapshotKind {
    SignedVersion,
    Revision,
}

#[derive(Clone, Debug)]
pub struct Snapshot {
    pub kind: SnapshotKind,
    /// The 1-based revision shown.
    pub revision: usize,
    /// The name of the document it was opened from.
    pub from: String,
}

/// A validation badge drawn over a signature widget: the widget rectangle (user space), the verdict it shows, and its hover text.
pub(crate) struct SignatureBadge {
    pub rect: [f64; 4],
    pub status: pdfcraft_engine::SignatureStatus,
    pub summary: &'static str,
}

/// The validation badges drawn over one page's signature widgets: signed, visible widgets
/// on this page with coordinates that survive the cast to screen space. Hostile files can
/// carry NaN, infinite or gigantic rectangles, which must never reach the painter.
pub(crate) fn signature_badges(signatures: &[pdfcraft_engine::SignatureInfo], page: usize) -> Vec<SignatureBadge> {
    signatures
        .iter()
        .filter(|s| s.signed && s.visible && s.page == Some(page))
        .filter_map(|s| {
            s.rect.filter(|r| r.iter().all(|v| v.is_finite() && v.abs() < f32::MAX as f64)).map(|rect| SignatureBadge {
                rect,
                status: s.status,
                summary: s.summary(),
            })
        })
        .collect()
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

pub struct DocView {
    pub id: DocId,
    pub zoom: f32,
    pub fit: Fit,
    pub layout: PageLayout,
    /// Whether the base layout scrolls continuously (independent of `layout`, as in Acrobat).
    pub continuous: bool,
    pub show_page_gaps: bool,
    /// View rotation in degrees clockwise (0, 90, 180, 270); display only, never saved.
    pub rotation: u16,
    pub current: usize,
    pub organize: bool,
    pub highlight_fields: bool,
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
    /// Two-page view: show the first page alone, as a cover (View ▸ Page display).
    pub cover: bool,
    /// Previous view / Next view: pages visited before (and after, once going back).
    pub back: Vec<usize>,
    pub forward: Vec<usize>,
    /// Pending navigation: page and fraction down the page to align with the viewport top.
    pub goto: Option<(usize, f32)>,
    /// Pending position within that page from a destination ([`Self::go_to_dest`]): the point
    /// (fractions of the displayed page; `None` leaves that axis to `goto`) to put `rel` points
    /// from the viewport's top-left corner.
    goto_point: Option<(usize, Option<f32>, Option<f32>, Vec2)>,
    /// Pending keyboard scrolling, in points down (negative: up): ↓ / ↑, and Page Down /
    /// Page Up where the pages scroll.
    pub key_scroll: f32,
    /// Single-page view, as last drawn: the page shown and whether the view was at its top and
    /// at its bottom, so ↑ / ↓ turn the page there instead of doing nothing (#273).
    single_edges: Option<(usize, bool, bool)>,
    /// Briefly outline an annotation after navigating to it from a panel.
    pub flash: Option<(usize, [f32; 4], f64)>,
    /// Compare files: differences shaded on this document's pages (page, user-space box, colour).
    pub compare_marks: Vec<(usize, [f32; 4], Color32)>,
    /// Full screen (Acrobat's Full Screen Mode): the display saved on entering it, while the view
    /// shows one page or spread at a time, fitted to the screen. `None` outside full screen.
    presentation: Option<SavedDisplay>,
    pages: HashMap<usize, PageTex>,
    /// Pages the renderer could not draw, with the reason (never re-requested).
    errors: HashMap<usize, String>,
    /// When each still-missing page was first shown, to flag unusually slow renders.
    waiting_since: HashMap<usize, f64>,
    thumbs: HashMap<usize, PageTex>,
    /// This frame's consumers, with visible cells ahead of prefetch.
    thumb_demand: HashMap<usize, bool>,
    frame_queue: Vec<RenderRequest>,
    /// A page-list edit after layout invalidates this frame's scales and page indexes.
    frame_geometry_changed: bool,
    /// Pages on screen this frame. Their rasters and tiles are always admitted: the viewport
    /// bounds them, and the byte and count limits apply to prefetch only.
    frame_visible: HashSet<usize>,
    /// Thumbnails that are out of date (still shown until their replacement arrives).
    stale_thumbs: HashSet<usize>,
    /// Tiles that are out of date (still shown until their replacement arrives).
    stale_tiles: HashSet<(usize, u64, u32, u32)>,
    /// Scale tag of the current zoom level, as last drawn.
    current_scale_tag: u64,
    /// Print dialog: the current sheet's pages at the preview pane's device resolution.
    print_pages: HashMap<usize, (u64, TextureHandle)>,
    /// Print dialog: preview rasters become luminance when Print ▸ Print in grayscale is on.
    grayscale: bool,
    /// Sharp tiles of large pages: (page, scale tag, tile x, tile y) → texture. Tiles of an
    /// earlier zoom stay, drawn stretched, until those of the current one cover the page.
    tiles: HashMap<(usize, u64, u32, u32), TextureHandle>,
    /// Text layers, extracted in the background on demand (selection, find, copy).
    pub(crate) texts: HashMap<usize, Arc<PageText>>,
    pub(crate) text_failed: HashSet<usize>,
    pub find: Option<Find>,
    selection: Option<Selection>,
    /// Select All while scrolling continuously (#2639): the text of every page of the document,
    /// highlighted on each page as its text arrives and copied whole ([`DocView::whole_text`]).
    pub(crate) all_text: bool,
    /// The selection whose action bar was closed by its Copy button: the bar stays hidden
    /// until the selection changes, as it does once a markup button consumes the selection.
    actions_dismissed: Option<Selection>,
    /// The drag in progress started a text selection: the wheel scrolls the page during it
    /// (#721), which egui's scroll area refuses while anything is dragged.
    selection_drag: bool,
    /// Clicks in the current run on the text layer: two select a word, three the line, four the page.
    clicks: u32,
    last_press_time: f64,
    last_press_pos: Option<Pos2>,
    /// Full screen: a click's page turn, when it falls due (input time) and which way. It waits
    /// out the double-click time, as the click may be the first of a double-click selecting a word.
    present_turn: Option<(f64, bool)>,
    last_queue: Vec<RenderRequest>,
    /// Finished renders wait in the pool instead of reaching the view (tests): what the view
    /// shows before a raster arrives can be looked at however fast the renderer is.
    pub hold_rasters: bool,
    viewport_w: f32,
    viewport_h: f32,
    page_count: usize,
    /// Page heights in points (view space), for mapping text positions to scroll offsets.
    page_heights: Vec<f32>,
    /// Screen rects of the pages drawn last frame (hit-testing, tests, automation).
    screen_rects: Vec<(usize, Rect)>,
    screen_xforms: Vec<(usize, PageXform)>,
    /// The scroll viewport on screen last frame.
    viewport_screen: Rect,
    /// Pending zoom anchor: page, fractional position (also in gutters), and viewport offset.
    zoom_anchor: Option<(usize, f32, f32, Vec2)>,
    /// The page view has been shown, so its scroll offset in egui's memory is this document's.
    shown: bool,
    /// Pages that Edit already tried to turn into editable text (once each, so Undo sticks).
    pub(crate) auto_ocr_tried: std::collections::HashSet<usize>,
    /// Pages Edit looked at and found not to be a scan, with the document's edit generation then:
    /// they are looked at again only after the document changes (an image added, say).
    pub(crate) auto_ocr_skipped: std::collections::HashMap<usize, u64>,
    /// Turns wheel input into page turns in single-page view.
    wheel: crate::wheel_pager::WheelPager,
    /// Until this time, apply precise (touchpad) wheel input 1:1 and suppress egui's eased copy.
    /// A precision touchpad sends pixel-precise `MouseWheel` events; on Windows they arrive with
    /// no `TouchPhase::Start`, so egui eases deltas of 8 px or more and the page lags the finger
    /// (#759). The window outlasts a single frame so egui's internal easing tail is suppressed as
    /// it decays, instead of adding on top of the 1:1 motion.
    precise_scroll_until: f64,
    /// Per-axis line-wheel state; horizontal input cannot extend a vertical burst.
    wheel_boost_x: crate::wheel_boost::WheelBoost,
    wheel_boost_y: crate::wheel_boost::WheelBoost,
    /// Touchpad momentum after the fingers lift (#531).
    fling: crate::fling::Fling,
    pub(crate) auto_scroll: crate::autoscroll::AutoScroll,
    /// Pages selected in the organize grid or the Pages panel (0-based). Empty means "the current page".
    pub selected: BTreeSet<usize>,
    pub(crate) organize_range: crate::page_selection_ui::PageSelectionInput,
    /// Anchor for ⇧-click range selection in the organize grid.
    select_anchor: Option<usize>,
    /// An edit requested by the view (organize toolbar, keys), applied by the app this frame.
    pub pending_edit: Option<Edit>,
    /// Edit text: the lines per page (with the document generation they were read at), and the
    /// line being edited.
    pub(crate) edit_lines: HashMap<usize, (u64, Vec<pdfcraft_engine::TextBlock>)>,
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
    /// Edit text & images: the images per page (by document generation), and the selected one.
    pub(crate) edit_images: HashMap<usize, (u64, Vec<pdfcraft_engine::PageImage>)>,
    pub(crate) objects: crate::object_ui::ObjectSelection,
    pub image_selection: Option<crate::edit_text_ui::ImageSelection>,
    /// Select tool: the picture clicked, to copy in its original format.
    pub picked_image: Option<crate::image_pick::PickedImage>,
    /// A paragraph box being dragged (moved, or resized from its right edge) in Edit text.
    pub block_drag: Option<crate::edit_text_ui::BlockDrag>,
    /// Commenting state: selected comment, gestures, composer.
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
    /// Redact tool: the box being drawn, and a mark to add (page, quads) for the app to style.
    pub redact_drag: crate::redact_ui::AreaDrag,
    pub pending_redaction: Option<(usize, Vec<[f64; 8]>)>,
    /// A crop rectangle being dragged (Crop tool).
    pub crop_drag: crate::crop::CropDrag,
    /// Enhance ▸ Camera image: the quadrilateral being placed.
    pub camera: Option<crate::camera_ui::CameraQuad>,
    /// Enhance ▸ Camera image: the page just enhanced, while the bar offers rotate, crop and
    /// organize.
    pub camera_done: Option<crate::camera_ui::CameraDone>,
    /// Use a certificate: a signature rectangle being drawn, or an empty signature field clicked.
    pub sign: crate::sign_ui::SignView,
    /// Organize: the pages being dragged to a new place.
    pub org_drag: Option<Vec<usize>>,
    /// Pages panel: the pages being dragged to a new place.
    pub panel_drag: Option<Vec<usize>>,
    /// Marquee Zoom / Snapshot: the rectangle being dragged (page, start), and a finished one.
    pub marquee: Option<(usize, Pos2)>,
    pub marquee_done: Option<crate::zoom_snap::Marquee>,
    /// Fill & Sign text being typed.
    pub fill_text: Option<crate::fill_sign::TypeBox>,
    /// A queued Fill & Sign signature: select it after its edit succeeds, then leave placement.
    pub(crate) fill_signature_page: Option<usize>,
    pub(crate) signature_drag: crate::signature_drag::SignatureDrag,
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
    /// The current page the Pages panel last followed: it scrolls to the current page's
    /// thumbnail only when the page changes.
    pub(crate) panel_followed: Option<usize>,
    /// Scale of the thumbnails in the Pages inspector, independent of document and grid zoom.
    thumbnail_zoom: f32,
    /// How large the page grid draws its pages (1.0 = the usual thumbnails).
    grid_zoom: f32,
    /// A zoom asked for this frame (toolbar, pinch, keys); the grid applies it once it has laid
    /// out, so it can keep the same pages in view.
    grid_zoom_request: Option<f32>,
    /// The page to keep in place after a grid zoom, and how far below the top of the grid its
    /// cell was.
    grid_anchor: Option<(usize, f32)>,
    /// The page the grid scrolls into view on its next frame (Organize just opened on it).
    grid_reveal: Option<usize>,
    /// Sharp renders of the pages the grid shows larger than a thumbnail: only those in view,
    /// at the size drawn (device pixels wide).
    grid_pages: HashMap<usize, (u32, TextureHandle)>,
    /// Zoom-settle debounce for heavy pages: the last requested device scale, and the egui
    /// time until which sharp whole-page/tile re-renders are held back. While the zoom is
    /// still moving, every frame would otherwise cancel the in-flight full-page interpret
    /// (seconds on image-heavy CAD plots) and start it over, so a sharp raster never
    /// arrives. The stale raster stays on screen stretched meanwhile.
    last_scale: f32,
    sharp_hold_until: f64,
    /// Pan-settle debounce for tiled pages: the scroll offset last frame, and the egui time
    /// until which sharp tile re-renders are held back. Each tile costs a full page
    /// interpret, so a drag would otherwise queue a storm of them (one per frame) while the
    /// in-flight ones cannot be cancelled mid-operator. Older tiles and the backdrop stay
    /// stretched meanwhile; fresh tiles go out once the pan settles.
    pub(crate) last_scroll: Vec2,
    /// Side-by-side compare: scroll to here this frame, following the other half.
    pub(crate) sync_scroll: Option<Vec2>,
    /// The page widget's id, by which the Hand tool finds its drag before the pages are laid out.
    page_widget: Option<egui::Id>,
    pan_hold_until: f64,
    /// Pages whose raster proved slow (see `SLOW_MILLIS`): only these get the zoom/pan
    /// settle holds. Fast pages render straight through, with no added latency.
    slow_pages: HashSet<usize>,
}

/// The page grid's zoom range and the step of its buttons and keys.
pub const GRID_ZOOM_RANGE: std::ops::RangeInclusive<f32> = 0.5..=3.0;
/// Thumbnail size in the Pages inspector, relative to its usual size.
pub const THUMBNAIL_ZOOM_RANGE: std::ops::RangeInclusive<f32> = 0.5..=1.5;
const GRID_ZOOM_STEP: f32 = 1.25;
/// How far a sharp grid render may be from the size drawn before it is redone: a pinch changes
/// the size on every frame, and a slightly soft or slightly large image is fine meanwhile.
const GRID_SHARP: std::ops::RangeInclusive<f32> = 0.9..=1.6;

/// Whether an image `have` device pixels wide is good enough to draw `want` pixels wide.
fn sharp_enough(have: u32, want: f32) -> bool {
    want > 0.0 && GRID_SHARP.contains(&(have as f32 / want))
}

/// Thumbnail device-pixel density when `demand` pages share the thumbnail allowance.
fn thumbnail_density(demand: usize, ppp: f32) -> f32 {
    let per_thumb = THUMB_BYTES / demand.max(1).next_power_of_two();
    ppp.clamp(0.25, 4.0).min((per_thumb as f32 / (4.0 * THUMB_W * THUMB_H)).sqrt() * 0.98)
}

fn thumbnail_scale(width: f32, height: f32, density: f32) -> f32 {
    (THUMB_W / width.max(1.0)).min(THUMB_H / height.max(1.0)) * density
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
}

impl DocView {
    pub fn raster_memory(&self) -> RasterMemory {
        RasterMemory {
            thumbnail_count: self.thumbs.len(),
            thumbnail_bytes: self.thumbs.values().fold(0usize, |n, p| n.saturating_add(texture_bytes(&p.tex))),
            page_count: self.pages.len(),
            page_bytes: self.pages.values().fold(0usize, |n, p| n.saturating_add(texture_bytes(&p.tex))),
            tile_count: self.tiles.len(),
            tile_bytes: self.tiles.values().fold(0usize, |n, tex| n.saturating_add(texture_bytes(tex))),
            thumbnail_demand: self.thumb_demand.len(),
            pending_requests: self.last_queue.len(),
        }
    }

    /// Where `page` is drawn on screen this frame (`None` when it is not on screen).
    pub fn page_screen_rect(&self, page: usize) -> Option<Rect> {
        self.screen_xforms.iter().find(|(p, _)| *p == page).map(|(_, xf)| xf.rect)
    }

    /// The document area on screen.
    pub fn viewport_rect(&self) -> Rect {
        self.viewport_screen
    }

    /// Whether a middle-button scrolling gesture is active (tests and automation).
    pub fn auto_scrolling(&self) -> bool {
        self.auto_scroll.active()
    }

    /// Whether a held middle-button pan is active, outside Linux (tests and automation).
    pub fn middle_panning(&self) -> bool {
        self.auto_scroll.panning()
    }

    /// Pages that could not be rendered, with the reason (for automation; 0-based pages).
    pub fn page_errors(&self) -> Vec<(usize, &str)> {
        let mut v: Vec<(usize, &str)> = self.errors.iter().map(|(p, e)| (*p, e.as_str())).collect();
        v.sort_unstable_by_key(|(p, _)| *p);
        v
    }

    /// A view of a newly opened document, shown as `defaults` say. Opening a file then applies
    /// any layout or zoom the PDF itself asks for.
    pub fn new(id: DocId, info: &DocInfo, defaults: ViewDefaults) -> Self {
        Self {
            id,
            zoom: defaults.zoom,
            fit: defaults.fit,
            layout: defaults.layout,
            continuous: defaults.continuous,
            show_page_gaps: defaults.show_page_gaps,
            rotation: 0,
            current: 0,
            organize: false,
            highlight_fields: defaults.highlight_fields,
            page_input: "1".into(),
            page_input_reselect: false,
            notice_dismissed: false,
            scripts_off_noted: false,
            disk_changed: false,
            close_requested: false,
            snapshot: None,
            cover: false,
            back: Vec::new(),
            forward: Vec::new(),
            goto: None,
            goto_point: None,
            key_scroll: 0.0,
            single_edges: None,
            flash: None,
            compare_marks: Vec::new(),
            presentation: None,
            pages: HashMap::new(),
            errors: HashMap::new(),
            waiting_since: HashMap::new(),
            thumbs: HashMap::new(),
            thumb_demand: HashMap::new(),
            frame_queue: Vec::new(),
            frame_geometry_changed: false,
            frame_visible: HashSet::new(),
            stale_thumbs: HashSet::new(),
            stale_tiles: HashSet::new(),
            current_scale_tag: 0,
            print_pages: HashMap::new(),
            grayscale: false,
            tiles: HashMap::new(),
            texts: HashMap::new(),
            text_failed: HashSet::new(),
            find: None,
            selection: None,
            all_text: false,
            actions_dismissed: None,
            selection_drag: false,
            clicks: 0,
            last_press_time: 0.0,
            last_press_pos: None,
            present_turn: None,
            last_queue: Vec::new(),
            hold_rasters: false,
            viewport_w: 800.0,
            viewport_h: 600.0,
            page_count: info.pages.len(),
            page_heights: info.pages.iter().map(|p| p.height).collect(),
            screen_rects: Vec::new(),
            screen_xforms: Vec::new(),
            viewport_screen: Rect::NOTHING,
            zoom_anchor: None,
            shown: false,
            auto_ocr_tried: Default::default(),
            auto_ocr_skipped: Default::default(),
            wheel: Default::default(),
            precise_scroll_until: 0.0,
            wheel_boost_x: Default::default(),
            wheel_boost_y: Default::default(),
            fling: Default::default(),
            auto_scroll: Default::default(),
            selected: BTreeSet::new(),
            select_anchor: None,
            organize_range: Default::default(),
            pending_edit: None,
            edit_lines: HashMap::new(),
            line_editor: None,
            applied_editor: None,
            pending_text_open: None,
            text_background: Default::default(),
            move_background: Default::default(),
            edit_images: HashMap::new(),
            objects: Default::default(),
            image_selection: None,
            picked_image: None,
            block_drag: None,
            pending_action: None,
            insert_at: None,
            grid_gap: None,
            pages_panel: None,
            panel_gap: None,
            panel_followed: None,
            thumbnail_zoom: 1.0,
            grid_zoom: 1.0,
            grid_zoom_request: None,
            grid_anchor: None,
            grid_reveal: None,
            grid_pages: HashMap::new(),
            last_scale: 0.0,
            sharp_hold_until: 0.0,
            last_scroll: Vec2::ZERO,
            sync_scroll: None,
            page_widget: None,
            pan_hold_until: 0.0,
            slow_pages: HashSet::new(),
            comments: Default::default(),
            measure: Default::default(),
            forms: Default::default(),
            prepare: Default::default(),
            calc_order: Default::default(),
            content: Default::default(),
            links: Default::default(),
            redact_drag: None,
            pending_redaction: None,
            crop_drag: None,
            camera: None,
            camera_done: None,
            sign: Default::default(),
            org_drag: None,
            panel_drag: None,
            marquee: None,
            marquee_done: None,
            fill_text: None,
            fill_signature_page: None,
            signature_drag: Default::default(),
        }
    }

    /// The document's content or page list changed (an edit, undo or redo): drop caches and
    /// adopt the new page geometry, keeping the reader's place where possible.
    pub fn document_changed(&mut self, info: &DocInfo) {
        self.invalidate_content();
        self.frame_geometry_changed = true;
        self.page_count = info.pages.len();
        self.page_heights = info.pages.iter().map(|p| p.height).collect();
        let last = self.page_count.saturating_sub(1);
        self.current = self.current.min(last);
        self.selected.retain(|p| *p <= last);
        // Text being typed on a page that no longer exists goes with the page.
        if self.content.draft.as_ref().is_some_and(|d| d.page > last || self.page_count == 0) {
            self.content.draft = None;
        }
        if self.select_anchor.is_some_and(|a| a > last) {
            self.select_anchor = None;
        }
        self.goto = None;
        self.goto_point = None;
        self.zoom_anchor = None;
        self.flash = None;
        // Thumbnail drags hold page indexes from before the change.
        self.panel_drag = None;
        self.org_drag = None;
        self.reset_coarse_state();
    }

    /// The document was replaced by a newer version of its file (#431). The reader's place
    /// stays: zoom, layout, rotation, page, scroll, full screen, view history and the find bar,
    /// the snapshot marker and its pending close, and the page images on screen until their
    /// replacements are drawn, so nothing flashes. Everything that pointed into the old version (a selected
    /// comment, a field being typed in, text being edited, a drag, page selection) starts
    /// afresh, so no stale index reaches the new one.
    pub fn document_replaced(&mut self, info: &DocInfo) {
        let mut fresh = Self::new(self.id, info, ViewDefaults::default());
        fresh.zoom = self.zoom;
        fresh.fit = self.fit;
        fresh.layout = self.layout;
        fresh.continuous = self.continuous;
        fresh.show_page_gaps = self.show_page_gaps;
        fresh.rotation = self.rotation;
        fresh.current = self.current;
        fresh.organize = self.organize;
        fresh.grid_zoom = self.grid_zoom;
        fresh.thumbnail_zoom = self.thumbnail_zoom;
        // Same document id, so the scroll offset egui keeps for it is still this document's.
        fresh.shown = self.shown;
        fresh.highlight_fields = self.highlight_fields;
        fresh.notice_dismissed = self.notice_dismissed;
        fresh.snapshot = self.snapshot.clone();
        fresh.close_requested = self.close_requested;
        fresh.cover = self.cover;
        // Still presenting, with the display to go back to on leaving.
        fresh.presentation = self.presentation.take();
        fresh.back = std::mem::take(&mut self.back);
        fresh.forward = std::mem::take(&mut self.forward);
        fresh.find = self.find.take();
        fresh.pages = std::mem::take(&mut self.pages);
        fresh.thumbs = std::mem::take(&mut self.thumbs);
        fresh.viewport_w = self.viewport_w;
        fresh.viewport_h = self.viewport_h;
        fresh.viewport_screen = self.viewport_screen;
        *self = fresh;
        // Marks the kept images out of date and clamps the page to the new page count.
        self.document_changed(info);
        self.page_input = self.current_page_label(info.pages.get(self.current).map(|page| page.label.as_str()));
        self.page_input_reselect = true;
    }

    /// Open Organize Pages on the page being read (issue #1910). An empty selection means the
    /// current page, so a selection left from earlier that does not include it is dropped, and
    /// the grid scrolls its cell into view. Every way into the grid goes through here; closing
    /// it is a plain `organize = false`.
    pub fn open_organize(&mut self) {
        self.organize = true;
        if !self.selected.is_empty() && !self.selected.contains(&self.current) {
            self.clear_page_selection(Some(self.current));
        }
        self.grid_reveal = Some(self.current);
    }

    /// Pages an organize command acts on: the selection, or the current page.
    pub fn target_pages(&self) -> Vec<usize> {
        if self.selected.is_empty() { vec![self.current] } else { self.selected.iter().copied().collect() }
    }

    /// Select pages in the organize grid (tests, automation). Empty clears the selection.
    pub(crate) fn page_count(&self) -> usize {
        self.page_count
    }

    pub fn select_pages(&mut self, pages: &[usize]) {
        self.selected = pages.iter().copied().filter(|p| *p < self.page_count).collect();
        if let Some(first) = self.selected.first() {
            self.current = *first;
        }
    }

    /// A click on page `i`'s thumbnail, in the organize grid or the Pages panel. ⇧ selects the
    /// range from the anchor (or the current page); ⌘/Ctrl toggles the page; a plain click
    /// selects only it. `seed_current` is for the Pages panel, where an empty selection means
    /// the current page: the first ⌘-click on another page keeps the current one selected too.
    pub fn click_page(&mut self, i: usize, modifiers: egui::Modifiers, seed_current: bool) {
        if i >= self.page_count {
            return;
        }
        if modifiers.shift {
            let a = self.select_anchor.unwrap_or(self.current);
            self.selected = (a.min(i)..=a.max(i)).collect();
        } else if modifiers.command {
            if seed_current && self.selected.is_empty() && i != self.current {
                self.selected.insert(self.current);
            }
            if !self.selected.remove(&i) {
                self.selected.insert(i);
            }
            self.select_anchor = Some(i);
        } else {
            self.selected = [i].into();
            self.select_anchor = Some(i);
        }
    }

    /// Drop the page selection; a later ⇧-click ranges from `anchor`.
    pub fn clear_page_selection(&mut self, anchor: Option<usize>) {
        self.selected.clear();
        self.select_anchor = anchor.filter(|a| *a < self.page_count);
    }

    pub fn render_pending(&self) -> bool {
        !self.last_queue.is_empty() || self.text_background.pending() || self.move_background.pending()
    }

    /// The cached tile keys of a page, as (scale tag, tile x, tile y) (diagnostics and tests).
    pub fn tiles_debug(&self, page: usize) -> Vec<(u64, u32, u32)> {
        let mut k: Vec<(u64, u32, u32)> = self.tiles.keys().filter(|key| key.0 == page).map(|key| (key.1, key.2, key.3)).collect();
        k.sort_unstable();
        k
    }

    /// Re-render every page and text layer (the document's appearance changed, e.g. a layer
    /// was toggled). Out-of-date rasters stay on screen until their replacements arrive, so the
    /// view never flashes blank.
    pub fn invalidate_content(&mut self) {
        // Undo, redo and other document-wide changes: a released drawing's preview would be a ghost.
        self.comments.settling.clear();
        for p in self.pages.values_mut() {
            p.tag = STALE_TAG;
        }
        self.stale_thumbs.extend(self.thumbs.keys().copied());
        for slot in self.print_pages.values_mut() {
            slot.0 = STALE_TAG;
        }
        // Pages may have moved: a sharp render of another page would be worse than a soft one.
        self.grid_pages.clear();
        self.tiles.clear();
        self.stale_tiles.clear();
        self.forms.unbaked.clear();
        self.texts.clear();
        self.text_failed.clear();
        self.errors.clear();
        self.slow_pages.clear();
        self.waiting_since.clear();
        self.last_queue.clear();
        self.selection = None;
        self.all_text = false;
        if let Some(f) = self.find.as_mut() {
            f.matches.clear();
            f.current = None;
        }
    }

    /// Only `page` changed (a comment was added, edited or removed): re-render that page and
    /// re-read its text, keep everything else. Out-of-date tiles stay on screen until replacements arrive.
    pub fn page_changed(&mut self, page: usize) {
        if let Some(p) = self.pages.get_mut(&page) {
            p.tag = STALE_TAG;
        }
        if self.thumbs.contains_key(&page) {
            self.stale_thumbs.insert(page);
        }
        if let Some(slot) = self.print_pages.get_mut(&page) {
            slot.0 = STALE_TAG;
        }
        let current_tag = if self.current_scale_tag != 0 {
            self.current_scale_tag
        } else {
            self.tiles.keys().filter(|k| k.0 == page).map(|k| k.1).max().unwrap_or(0)
        };
        self.tiles.retain(|key, _| key.0 != page || key.1 == current_tag);
        self.stale_tiles.retain(|key| key.0 != page || key.1 == current_tag);
        for &key in self.tiles.keys() {
            if key.0 == page && key.1 == current_tag {
                self.stale_tiles.insert(key);
            }
        }
        self.slow_pages.remove(&page);
        self.texts.remove(&page);
        self.text_failed.remove(&page);
        self.errors.remove(&page);
        self.last_queue.clear();
        if self.selection.is_some_and(|s| s.page == page) {
            self.selection = None;
        }
    }

    /// Pages drawn last frame and their screen rectangles.
    pub fn visible_page_rects(&self) -> Vec<(usize, Rect)> {
        self.screen_xforms.iter().map(|(p, xf)| (*p, xf.rect)).collect()
    }

    /// Where `page` is drawn this frame, with its coordinate mapping.
    pub(crate) fn page_xform(&self, page: usize) -> Option<PageXform> {
        self.screen_xforms.iter().find(|(p, _)| *p == page).map(|(_, xf)| *xf)
    }

    /// The text selection as markup quadrilaterals in user space: (page, quads).
    pub(crate) fn selection_quads(&self, info: &DocInfo) -> Option<(usize, Vec<[f64; 8]>)> {
        let s = self.selection?;
        let text = self.texts.get(&s.page)?;
        let page = info.pages.get(s.page)?;
        let quads: Vec<[f64; 8]> = s.rects(text).into_iter().map(|r| page.view_rect_to_quad(r)).collect();
        (!quads.is_empty()).then_some((s.page, quads))
    }

    /// Where a bookmark made now should go: the page and the `/XYZ` position `[left, top]` in user
    /// space. With text selected it is the corner of the selection that shows at its top-left
    /// (its page, both coordinates); otherwise the top of the window (the page showing there, and
    /// only the position along the window's vertical, so the reader's other scroll position and
    /// zoom are kept). `None` before the first frame has laid pages out, when the bookmark just
    /// goes to the current page.
    pub(crate) fn bookmark_position(&self, info: &DocInfo) -> Option<(usize, [Option<f32>; 2])> {
        if let Some(sel) = self.selection
            && let Some(text) = self.texts.get(&sel.page)
            && let Some(p) = info.pages.get(sel.page)
        {
            // The selection's bounds in view space, which View ▸ Rotate View leaves unturned: the
            // corner at the screen's top-left is a different one for each quarter turn.
            let [x0, y0, x1, y1] = sel.rects(text).iter().fold([f32::INFINITY, f32::INFINITY, f32::NEG_INFINITY, f32::NEG_INFINITY], |b, r| {
                [b[0].min(r[0]), b[1].min(r[1]), b[2].max(r[2]), b[3].max(r[3])]
            });
            let (x, y) = match self.rotation {
                90 => (x0, y1),
                180 => (x1, y1),
                270 => (x1, y0),
                _ => (x0, y0),
            };
            if x.is_finite() && y.is_finite() {
                return Some((sel.page, p.xyz_for_view(Some(x), Some(y))));
            }
        }
        // The page showing at the window's top edge: one that crosses it (the first, in a spread
        // of pages of different heights, whose shorter page can start below it), else the
        // nearest one below.
        let top = self.viewport_screen.top();
        let showing = || self.screen_xforms.iter().filter(|(_, xf)| xf.rect.bottom() > top);
        let (page, xf) = showing()
            .filter(|(_, xf)| xf.rect.top() <= top)
            .min_by_key(|(p, _)| *p)
            .or_else(|| showing().min_by(|a, b| a.1.rect.top().total_cmp(&b.1.rect.top()).then(a.0.cmp(&b.0))))?;
        let p = info.pages.get(*page)?;
        // Above the page (in the gap before it) is the page's own top. The window's vertical is
        // the view's y, or its x when the view is turned a quarter.
        let (x, y) = xf.screen_to_view(pos2(xf.rect.center().x, top.max(xf.rect.top())));
        let at = if xf.rot.is_multiple_of(180) {
            p.xyz_for_view(None, Some(y.clamp(0.0, p.height)))
        } else {
            p.xyz_for_view(Some(x.clamp(0.0, p.width)), None)
        };
        Some((*page, at))
    }

    /// The user-space `[left, top]` at the window's top-left (where `/XYZ` puts its point), on the
    /// page showing there: what a destination's null coordinate keeps (ISO 32000-2 §12.3.2.2).
    /// Acrobat keeps the place this way across pages, so a CAD sheet's cross-reference opens the
    /// next sheet where the reader was looking. A coordinate is `None` along a screen axis the
    /// page fits, which keeps the layout's own placement (centred, or the page's top); the whole
    /// place is `None` before the first frame has laid pages out.
    fn place_at_corner(&self, info: &DocInfo) -> Option<[Option<f32>; 2]> {
        let area = self.viewport_screen;
        if !area.is_positive() {
            return None;
        }
        let corner = area.min + vec2(SIDE, 0.0);
        let (page, xf) = self
            .screen_xforms
            .iter()
            .filter(|(_, xf)| xf.rect.is_positive())
            .min_by(|a, b| a.1.rect.distance_sq_to_pos(corner).total_cmp(&b.1.rect.distance_sq_to_pos(corner)).then(a.0.cmp(&b.0)))?;
        let p = info.pages.get(*page)?;
        let (x, y) = xf.screen_to_view(corner.clamp(xf.rect.min, xf.rect.max));
        // `max` drops a NaN size, so the bounds stay ordered and `clamp` can't panic.
        let (x, y) = (x.clamp(0.0, p.width.max(0.0)), y.clamp(0.0, p.height.max(0.0)));
        // The page counts with its side gutters, as the layout places it.
        let wide = xf.rect.width() + 2.0 * SIDE > area.width();
        let tall = xf.rect.height() > area.height();
        // The view's x runs across the screen unless the view is turned a quarter.
        let (keep_x, keep_y) = if xf.rot.is_multiple_of(180) { (wide, tall) } else { (tall, wide) };
        Some(p.xyz_for_view(keep_x.then_some(x), keep_y.then_some(y)))
    }

    /// A page's thumbnail texture, when rendered.
    pub(crate) fn thumb_id(&self, page: usize) -> Option<egui::TextureId> {
        self.thumbs.get(&page).map(|t| t.tex.id())
    }

    /// The whole-page raster on screen for `page` (the low-res backdrop of a tiled page), if any.
    pub(crate) fn page_raster(&self, page: usize) -> Option<egui::TextureId> {
        self.pages.get(&page).map(|p| p.tex.id())
    }

    /// Whether `page`'s whole-page raster (a tiled page's backdrop too) is still the one from
    /// before its last change, whatever its tiles are.
    pub(crate) fn page_raster_out_of_date(&self, page: usize) -> bool {
        self.pages.get(&page).is_some_and(|p| p.tag == STALE_TAG)
    }

    /// Whether `page` is still drawn, somewhere, as it was before its last change: its whole-page
    /// raster (a tiled page's backdrop too) or any of its tiles waits for a replacement.
    pub(crate) fn page_out_of_date(&self, page: usize) -> bool {
        self.pages.get(&page).is_some_and(|p| p.tag == STALE_TAG) || self.stale_tiles.iter().any(|k| k.0 == page)
    }

    /// The print preview's picture of `page`: the sheet raster when it has arrived, otherwise the
    /// thumbnail.
    pub(crate) fn page_preview(&self, page: usize) -> Option<egui::TextureId> {
        self.print_pages.get(&page).map(|(_, tex)| tex.id()).or_else(|| self.thumb_id(page))
    }

    pub(crate) fn page_text(&self, page: usize) -> Option<Arc<PageText>> {
        self.texts.get(&page).cloned()
    }

    pub(crate) fn clear_selection(&mut self) {
        self.selection = None;
        self.actions_dismissed = None;
    }

    /// Select text on `page` from glyph `from` to glyph `to` (tests and automation).
    pub fn select_text(&mut self, page: usize, from: usize, to: usize) {
        self.all_text = false;
        self.selection = Some(Selection::run(page, from, to));
    }

    /// Open the find bar (or focus it if already open).
    pub fn open_find(&mut self) {
        let f = self.find.get_or_insert_with(Find::default);
        f.focus = true;
    }

    /// Re-run the search on all pages whose text is known (after the query changed).
    pub fn rerun_find(&mut self) {
        let Some(f) = self.find.as_mut() else { return };
        f.case_query = f.query.clone();
        f.matches.clear();
        f.current = None;
        if f.case_query.trim().is_empty() {
            return;
        }
        let mut pages: Vec<usize> = self.texts.keys().copied().collect();
        pages.sort();
        for &p in &pages {
            if let Some(text) = self.texts.get(&p) {
                f.matches.extend(text.find_opts(&f.case_query, f.case_sensitive, f.whole_words).into_iter().map(|r| (p, r)));
            }
        }
        f.matches.sort_by_key(|(p, r)| (*p, r.start));
        if !f.matches.is_empty() {
            // First match at or after the page being viewed.
            let cur = self.current;
            let i = f.matches.iter().position(|(p, _)| *p >= cur).unwrap_or(0);
            f.current = Some(i);
            let (p, _) = f.matches[i];
            self.flash_match(p);
        }
    }

    fn refresh_find_page(&mut self, page: usize) {
        let (Some(f), Some(text)) = (self.find.as_mut(), self.texts.get(&page)) else { return };
        if f.case_query.trim().is_empty() {
            return;
        }
        let current_key = f.current.and_then(|c| f.matches.get(c).cloned());
        f.matches.retain(|(p, _)| *p != page);
        f.matches.extend(text.find_opts(&f.case_query, f.case_sensitive, f.whole_words).into_iter().map(|r| (page, r)));
        f.matches.sort_by_key(|(p, r)| (*p, r.start));
        f.current = current_key.as_ref().and_then(|k| f.matches.iter().position(|m| m == k));
        if f.current.is_none() && !f.matches.is_empty() {
            // First match at or after the page being viewed.
            let cur = self.current;
            let i = f.matches.iter().position(|(p, _)| *p >= cur).unwrap_or(0);
            f.current = Some(i);
            let (p, _) = f.matches[i];
            if current_key.is_none() {
                self.flash_match(p);
            }
        }
    }

    /// Go to match `i` (the Search panel's results).
    pub fn go_to_match(&mut self, i: usize) {
        let Some(f) = self.find.as_mut() else { return };
        let Some(&(p, _)) = f.matches.get(i) else { return };
        f.current = Some(i);
        self.go_to_page(p);
        self.goto_point = None;
        self.flash_match(p);
    }

    /// Move to the next/previous match.
    pub fn find_step(&mut self, forward: bool) {
        let Some(f) = self.find.as_mut() else { return };
        if f.matches.is_empty() {
            return;
        }
        let n = f.matches.len();
        let c = f.current.unwrap_or(0);
        let next = if forward { (c + 1) % n } else { (c + n - 1) % n };
        f.current = Some(next);
        let p = f.matches[next].0;
        // Only explicit search navigation joins history; background refresh uses flash_match alone.
        self.go_to_page(p);
        self.goto_point = None;
        self.flash_match(p);
    }

    fn flash_match(&mut self, page: usize) {
        let frac = self
            .find
            .as_ref()
            .and_then(|f| f.current.and_then(|c| f.matches.get(c)))
            .and_then(|(p, r)| self.texts.get(p).and_then(|t| t.glyphs.get(r.start)).map(|g| g.rect[1]))
            .zip(self.page_heights.get(page).copied())
            .map(|(y, h)| ((y - 60.0) / h).clamp(0.0, 1.0))
            .unwrap_or(0.0);
        self.goto = Some((page, frac));
        self.current = page;
        self.reset_coarse_state();
    }

    /// Screen position of the centre of glyph `glyph` on `page`, if that page is on screen and its
    /// text layer is loaded (used by UI tests and automation to aim pointer input).
    pub fn glyph_screen_pos(&self, page: usize, glyph: usize) -> Option<Pos2> {
        let (_, xf) = self.screen_xforms.iter().find(|(p, _)| *p == page)?;
        let g = self.texts.get(&page)?.glyphs.get(glyph)?;
        Some(xf.view_rect(g.rect).center())
    }

    /// Selected text, if any (⌘C).
    /// The selected text: of the pages read so far, after Select All of the document (Copy reads
    /// the others, [`DocView::pages_without_text`]).
    pub fn selected_text(&self) -> Option<String> {
        if self.all_text {
            return Some(self.whole_text(&HashMap::new())).filter(|x| !x.is_empty());
        }
        let s = self.selection?;
        let t = self.texts.get(&s.page)?;
        Some(s.text(t)).filter(|x| !x.is_empty())
    }

    pub fn thumb(&self, page: usize) -> Option<&TextureHandle> {
        self.thumbs.get(&page).map(|t| &t.tex)
    }

    /// Drops wheel state after a discontinuous jump so the destination cannot
    /// inherit momentum from the previous viewport.
    fn reset_coarse_state(&mut self) {
        self.wheel.clear_motion();
        self.wheel_boost_x.reset();
        self.wheel_boost_y.reset();
    }

    pub fn go_to_page(&mut self, page: usize) {
        let page = page.min(self.page_count.saturating_sub(1));
        if page != self.current {
            // The view history (Previous view / Next view).
            if self.back.last() != Some(&self.current) {
                self.back.push(self.current);
                if self.back.len() > 200 {
                    self.back.remove(0);
                }
            }
            self.forward.clear();
        }
        // Partial wheel motion belongs to the old place.
        self.reset_coarse_state();
        self.goto = Some((page, 0.0));
        self.current = page;
    }

    /// Reveal a user-space rectangle without changing zoom, centred where the scroll bounds allow.
    pub(crate) fn reveal_rect(&mut self, page: usize, rect: [f32; 4], info: &DocInfo) {
        self.go_to_page(page);
        self.goto_point = None;
        let Some(p) = info.pages.get(self.current) else { return };
        if !rect.iter().all(|v| v.is_finite()) {
            return;
        }
        // Halve before adding so even large finite coordinates cannot overflow the midpoint.
        let x = rect[0] * 0.5 + rect[2] * 0.5;
        let y = rect[1] * 0.5 + rect[3] * 0.5;
        let [fx, fy] = p.dest_fraction(Some(x), Some(y), self.rotation);
        self.goto_point = Some((self.current, fx, fy, vec2(self.viewport_w, self.viewport_h) / 2.0));
    }

    /// Go to a destination, a bookmark's or a link's (ISO 32000-2 §12.3.2.2): its page, scrolled
    /// and zoomed as `dest` asks, as Acrobat does. `/XYZ` puts (left, top) at the window's
    /// top-left at its zoom (null or 0 keeps the zoom); `/Fit`, `/FitH` and `/FitV` switch to fit
    /// page, width and height with `top` or `left` at the window's edge; `/FitR` zooms so the
    /// rectangle fits, centred. The `B` (content box) forms are read as the page. A null `/XYZ`
    /// coordinate keeps the reader's place: the same user-space point stays at the window's
    /// top-left on the new page, as Acrobat does. A point past the end of the document scrolls
    /// as far as it can.
    pub fn go_to_dest(&mut self, page: usize, dest: DestView, info: &DocInfo) {
        // Read before the jump: it is where the reader is now.
        let kept = match dest {
            DestView::Xyz { left, top, .. } if left.is_none() || top.is_none() => self.place_at_corner(info),
            _ => None,
        };
        self.go_to_page(page);
        let page = self.current;
        self.goto_point = None;
        let Some(p) = info.pages.get(page) else { return };
        let point = |x: Option<f32>, y: Option<f32>| p.dest_fraction(x, y, self.rotation);
        // The window's top-left as far as pages go: its left edge is the gutter that keeps them
        // clear of the floating quick-action bar, so `left` 0 shows the page's edge beside it.
        let corner = vec2(SIDE, 0.0);
        let (fx, fy, rel) = match dest {
            DestView::Top => return,
            DestView::Xyz { left, top, zoom } => {
                if let Some(z) = zoom {
                    self.zoom = z.clamp(0.08, 64.0);
                    self.fit = Fit::None;
                    self.zoom_anchor = None;
                }
                let [kept_left, kept_top] = kept.unwrap_or([None, None]);
                let [fx, fy] = point(left.or(kept_left), top.or(kept_top));
                (fx, fy, corner)
            }
            DestView::Fit => {
                self.fit = Fit::Page;
                return;
            }
            DestView::FitH { top } => {
                self.fit = Fit::Width;
                let [fx, fy] = point(None, top);
                (fx, fy, corner)
            }
            DestView::FitV { left } => {
                self.fit = Fit::Height;
                let [fx, fy] = point(left, None);
                (fx, fy, corner)
            }
            DestView::FitR { rect } => {
                let ([Some(ax), Some(ay)], [Some(bx), Some(by)]) = (point(Some(rect[0]), Some(rect[3])), point(Some(rect[2]), Some(rect[1]))) else {
                    return;
                };
                // The rectangle as displayed, in points (it can be turned a quarter).
                let (dw, dh) = self.display_size(p);
                let (w, h) = ((ax - bx).abs() * dw, (ay - by).abs() * dh);
                if w >= 1.0 && h >= 1.0 && self.viewport_w >= 1.0 && self.viewport_h >= 1.0 {
                    self.zoom = (self.viewport_w / (w * PT)).min(self.viewport_h / (h * PT)).clamp(0.08, 64.0);
                    self.fit = Fit::None;
                    self.zoom_anchor = None;
                }
                (Some((ax + bx) / 2.0), Some((ay + by) / 2.0), vec2(self.viewport_w, self.viewport_h) / 2.0)
            }
        };
        if fx.is_some() || fy.is_some() {
            self.goto_point = Some((page, fx, fy, rel));
        }
    }

    /// Next page (`true`) or previous page. In two-page view this moves a whole spread: the other
    /// page of the spread is already on screen, so stepping to it wouldn't move the view (#70).
    pub fn step_page(&mut self, forward: bool) {
        let c = self.current;
        let last = self.page_count.saturating_sub(1);
        let (target, first) = match self.layout {
            PageLayout::TwoUp => {
                let first = self.spread_first(c);
                let target = match (forward, self.cover && c == 0) {
                    (true, true) => 1,
                    (true, false) => first.saturating_add(2),
                    (false, _) if self.cover && first <= 1 => 0,
                    (false, _) => first.saturating_sub(2),
                };
                (target, first)
            }
            PageLayout::Single if forward => (c + 1, c),
            PageLayout::Single => (c.saturating_sub(1), c),
        };
        // Past either end there is nowhere to go: stay put rather than re-align the view, which
        // would scroll the page back to its top.
        if if forward { target > last } else { first == 0 } {
            return;
        }
        self.go_to_page(target);
    }

    /// Page-editor focus and drafts belong to a document, even while another tab is active.
    pub(crate) fn page_input_id(&self) -> egui::Id {
        egui::Id::new(("page-input", self.id.0))
    }

    /// The idle page box and unchanged-input lookup share the same label fallback.
    pub(crate) fn current_page_label(&self, label: Option<&str>) -> String {
        label.filter(|l| !l.is_empty()).map(str::to_owned).unwrap_or_else(|| (self.current + 1).to_string())
    }

    /// Retain the current page when `typed` exactly matches its displayed label, even if
    /// another page has that label. Otherwise resolve a trimmed label (case-insensitive),
    /// then a physical page number. `false` when it names no page.
    pub fn go_to_typed(&mut self, typed: &str, labels: &[String]) -> bool {
        // Compare the displayed value before trimming: a label may itself contain spaces.
        // Repeated labels occur in restarted sections and prefix-only ranges.
        if self.current < self.page_count && self.current_page_label(labels.get(self.current).map(String::as_str)) == typed {
            return true;
        }
        let t = typed.trim();
        let page = labels
            .iter()
            .position(|l| !l.is_empty() && l.eq_ignore_ascii_case(t))
            .or_else(|| t.parse::<usize>().ok().filter(|n| *n >= 1).map(|n| n - 1));
        match page {
            Some(p) if p < self.page_count => {
                self.go_to_page(p);
                true
            }
            _ => false,
        }
    }

    /// View ▸ Page navigation ▸ Previous view (`false`) / Next view (`true`).
    pub fn view_history(&mut self, forward: bool) -> bool {
        let (from, to) = if forward { (&mut self.forward, &mut self.back) } else { (&mut self.back, &mut self.forward) };
        let Some(page) = from.pop() else { return false };
        to.push(self.current);
        let page = page.min(self.page_count.saturating_sub(1));
        self.goto = Some((page, 0.0));
        self.current = page;
        self.reset_coarse_state();
        true
    }

    /// Select all: every page in Organize, or every word on the current page.
    pub fn select_all(&mut self) -> bool {
        if self.organize {
            self.selected = (0..self.page_count).collect();
            // Keep the current page as the anchor for the next Shift-click.
            self.select_anchor = (self.page_count > 0).then_some(self.current);
            return !self.selected.is_empty();
        }
        // Scrolling continuously, Select All takes the whole document's text (#2639); one page at
        // a time, the page shown.
        // Like the page's, true once some text is selected: the pages whose text isn't read yet are
        // read when the selection is copied.
        if self.continuous && self.page_count > 1 {
            self.selection = None;
            self.all_text = true;
            return self.selected_text().is_some();
        }
        self.all_text = false;
        let page = self.current;
        let Some(t) = self.texts.get(&page) else { return false };
        if t.glyphs.is_empty() {
            return false;
        }
        self.selection = Some(Selection::run(page, 0, t.glyphs.len() - 1));
        true
    }

    /// Pages whose text [`DocView::whole_text`] still needs: not read yet, nor failed.
    pub(crate) fn pages_without_text(&self) -> Vec<usize> {
        (0..self.page_count).filter(|p| !self.texts.contains_key(p) && !self.text_failed.contains(p)).collect()
    }

    /// The whole document's text in page order, one page after another on new lines, from the
    /// pages' text read so far and `more` (pages read for this, keyed by page).
    pub(crate) fn whole_text(&self, more: &HashMap<usize, Arc<PageText>>) -> String {
        let mut out = String::new();
        for p in 0..self.page_count {
            let Some(t) = self.texts.get(&p).or_else(|| more.get(&p)) else { continue };
            let text = t.text_of(0..t.glyphs.len());
            if text.is_empty() {
                continue;
            }
            if !out.is_empty() {
                out.push('\n');
            }
            out.push_str(&text);
        }
        out
    }

    /// Rotate the view 90° clockwise or counter-clockwise (View ▸ Rotate View, ⇧⌘+ / ⇧⌘−).
    pub fn rotate_view(&mut self, clockwise: bool) {
        self.rotation = (self.rotation + if clockwise { 90 } else { 270 }) % 360;
        self.goto = Some((self.current, 0.0));
        self.reset_coarse_state();
    }

    /// Switch the zoom mode, keeping the current page in view.
    pub fn set_fit(&mut self, fit: Fit) {
        self.fit = fit;
        self.goto = Some((self.current, 0.0));
        self.reset_coarse_state();
    }

    /// Full screen, as Acrobat's Full Screen Mode (#379, #1459): one page at a time (or one
    /// spread, when two-page view was on: #1353), fitted to the screen, from the current page.
    /// The display it replaces is kept for [`Self::leave_presentation`]. A no-op while presenting.
    pub fn enter_presentation(&mut self) {
        if self.presentation.is_some() {
            return;
        }
        self.presentation =
            Some(SavedDisplay { layout: self.layout, continuous: self.continuous, cover: self.cover, fit: self.fit, zoom: self.zoom });
        self.continuous = false;
        self.fit = Fit::Page;
        self.selection = None;
        self.goto = Some((self.current, 0.0));
    }

    /// Leave full screen: the display from before it comes back, at the page reached meanwhile.
    pub fn leave_presentation(&mut self) {
        let Some(saved) = self.presentation.take() else { return };
        self.layout = saved.layout;
        self.continuous = saved.continuous;
        self.cover = saved.cover;
        self.fit = saved.fit;
        self.zoom = saved.zoom;
        self.goto = Some((self.current, 0.0));
    }

    /// The display shown outside full screen: the one presenting replaced, or the current one.
    pub fn normal_display(&self) -> SavedDisplay {
        self.presentation.unwrap_or(SavedDisplay {
            layout: self.layout,
            continuous: self.continuous,
            cover: self.cover,
            fit: self.fit,
            zoom: self.zoom,
        })
    }

    /// Whether the view is in full screen's one-page-at-a-time display.
    pub fn presenting(&self) -> bool {
        self.presentation.is_some()
    }

    /// Switch the base page display (Single or Two-page), leaving continuous scrolling as it is.
    /// A no-op when the layout is already current, so re-selecting it never yanks a scrolled view
    /// back to the top.
    pub fn set_layout(&mut self, layout: PageLayout) {
        if self.layout == layout {
            return;
        }
        self.layout = layout;
        self.goto = Some((self.current, 0.0));
        self.reset_coarse_state();
    }

    /// Whether the view scrolls continuously (as opposed to one page or spread at a time).
    pub fn continuous(&self) -> bool {
        self.continuous
    }

    /// Change page separation without editing the PDF or losing the reader's place.
    pub fn set_page_gaps(&mut self, show: bool) {
        if self.show_page_gaps == show {
            return;
        }
        if !self.organize && self.viewport_screen.is_positive() {
            let at = self.viewport_screen.center();
            if let Some((page, r)) = self
                .screen_rects
                .iter()
                .filter(|(_, r)| r.is_positive())
                .min_by(|(_, a), (_, b)| a.distance_sq_to_pos(at).total_cmp(&b.distance_sq_to_pos(at)))
            {
                let fraction = (at - r.min) / r.size();
                self.zoom_anchor = Some((*page, fraction.x, fraction.y, at - self.viewport_screen.min));
            }
        }
        self.show_page_gaps = show;
    }

    /// Separation between adjacent page rectangles, in logical screen points.
    fn page_gap(&self) -> f32 {
        if self.show_page_gaps { GAP } else { 0.0 }
    }

    /// Turn continuous scrolling on or off, keeping the current page in view. A no-op when it is
    /// already in that state.
    pub fn set_continuous(&mut self, continuous: bool) {
        if self.continuous != continuous {
            self.continuous = continuous;
            // Select All of the document belongs to continuous scrolling.
            self.all_text = false;
            self.goto = Some((self.current, 0.0));
            self.reset_coarse_state();
        }
    }

    /// Set the whole page display at once (the base layout and continuous scrolling together),
    /// as the control channel and settings do through a single [`display_token`].
    pub fn set_display(&mut self, layout: PageLayout, continuous: bool) {
        self.set_layout(layout);
        self.set_continuous(continuous);
    }

    /// Set the cover page in two-page view. A no-op unless something changed.
    pub fn set_cover(&mut self, cover: bool) {
        if self.cover != cover {
            self.cover = cover;
            self.goto = Some((self.current, 0.0));
            self.reset_coarse_state();
        }
    }

    /// Whether the cover page setting applies: only two-page view has a cover page.
    pub fn cover_applies(&self) -> bool {
        self.layout == PageLayout::TwoUp
    }

    /// One wheel event in a non-continuous view, where the wheel turns pages or spreads (the
    /// rules are in `wheel_pager`): `dy` is its vertical delta (negative scrolls down) and `now`
    /// is egui time. With `can_turn` false the gesture is followed but the page stays. Returns
    /// whether the page turned.
    pub fn single_page_wheel(&mut self, unit: egui::MouseWheelUnit, dy: f32, phase: egui::TouchPhase, now: f64, can_turn: bool) -> bool {
        if self.continuous {
            return false;
        }
        let Some(forward) = self.wheel.feed(unit, dy, phase, now, can_turn) else { return false };
        let before = self.current;
        self.step_page(forward);
        self.current != before
    }

    /// One wheel event in a non-continuous view whose page is taller than the window, where the
    /// wheel pans (#1924). Once the view is at the bottom (top) edge, scrolling on turns to the
    /// next (previous) page, which shows its top (bottom). A smooth gesture that panned the page
    /// has to end first (see `WheelPager::feed_edge`). `can_turn` is false when something else
    /// owns the wheel. Returns whether the page turned.
    pub fn single_page_wheel_edge(&mut self, unit: egui::MouseWheelUnit, dy: f32, phase: egui::TouchPhase, now: f64, can_turn: bool) -> bool {
        if self.continuous {
            return false;
        }
        // Only an up-to-date view counts: the page drawn is the shown one, with no jump pending.
        let edge = self.goto.is_none()
            && self.goto_point.is_none()
            && self.single_edges.is_some_and(|(page, top, bottom)| page == self.current && if dy < 0.0 { bottom } else { top });
        let Some(forward) = self.wheel.feed_edge(unit, dy, phase, now, can_turn && edge) else { return false };
        let before = self.current;
        self.step_page(forward);
        if self.current == before {
            return false;
        }
        if !forward {
            // Going back lands on the bottom of the previous page.
            self.goto = Some((self.current, 1.0));
        }
        self.single_edges = None;
        true
    }

    /// Displayed page size in points for this view rotation.
    fn display_size(&self, p: &pdfcraft_render::PageInfo) -> (f32, f32) {
        if self.rotation % 180 == 90 { (p.height, p.width) } else { (p.width, p.height) }
    }

    /// Marquee Zoom: zoom so `r` (on screen) fills the window, centred.
    pub fn zoom_to_rect(&mut self, r: Rect) {
        let Some((page, pr)) = self.screen_rects.iter().find(|(_, pr)| pr.contains(r.center())).copied() else { return };
        let k = (self.viewport_screen.width() / r.width().max(1.0)).min(self.viewport_screen.height() / r.height().max(1.0));
        let f = (r.center() - pr.min) / pr.size();
        self.zoom = (self.zoom * k).clamp(0.08, 64.0);
        self.fit = Fit::None;
        self.zoom_anchor = Some((page, f.x.clamp(0.0, 1.0), f.y.clamp(0.0, 1.0), self.viewport_screen.center() - self.viewport_screen.min));
    }

    /// View ▸ Zoom ▸ Fit Visible: zoom so the visible content of `page` (display-normalised
    /// [x0, y0, x1, y1], i.e. after view rotation) spans the window's width, its left edge at
    /// the window's left and its top at the top.
    pub fn fit_content(&mut self, page: usize, content: [f32; 4]) -> bool {
        const PAD: f32 = 16.0;
        let Some((_, pr)) = self.screen_rects.iter().find(|(p, _)| *p == page).copied() else { return false };
        let w = (content[2] - content[0]).max(0.01) * pr.width();
        let k = (self.viewport_screen.width() - 2.0 * PAD).max(50.0) / w.max(1.0);
        self.zoom = (self.zoom * k).clamp(0.08, 64.0);
        self.fit = Fit::None;
        self.zoom_anchor = Some((page, content[0].clamp(0.0, 1.0), content[1].clamp(0.0, 1.0), vec2(PAD, MARGIN)));
        true
    }

    /// How large the page grid draws its pages (1.0 = the usual thumbnails).
    pub fn grid_zoom(&self) -> f32 {
        self.grid_zoom
    }

    /// Scale of the thumbnails in the Pages inspector (1.0 is the usual size).
    pub fn thumbnail_zoom(&self) -> f32 {
        self.thumbnail_zoom
    }

    /// Resize the inspector thumbnails, keeping the current page in view after layout.
    /// Out-of-range values are clamped; non-finite values are ignored.
    pub fn set_thumbnail_zoom(&mut self, zoom: f32) {
        if zoom.is_finite() {
            let zoom = zoom.clamp(*THUMBNAIL_ZOOM_RANGE.start(), *THUMBNAIL_ZOOM_RANGE.end());
            if zoom != self.thumbnail_zoom {
                self.thumbnail_zoom = zoom;
                self.panel_followed = None;
            }
        }
    }

    /// Zoom the page grid; out-of-range values are clamped and nonsense is ignored.
    pub fn set_grid_zoom(&mut self, zoom: f32) {
        if zoom.is_finite() {
            self.grid_zoom = zoom.clamp(*GRID_ZOOM_RANGE.start(), *GRID_ZOOM_RANGE.end());
        }
    }

    /// How many device pixels wide the grid's sharp render of `page` is, if it has one.
    pub fn grid_page_pixels(&self, page: usize) -> Option<u32> {
        self.grid_pages.get(&page).map(|(w, _)| *w)
    }

    /// Zoom keeping the centre of the view still.
    pub fn set_zoom(&mut self, zoom: f32) {
        // CLI/view options can set zoom before the first layout. Rect::NOTHING has no
        // finite centre yet, but an initial zoom still needs to leave the default fit mode.
        let centre = if self.viewport_screen.is_positive() { self.viewport_screen.center() } else { Pos2::ZERO };
        self.zoom_at(zoom, centre);
    }

    /// Zoom keeping the document point under `screen_pos` still (pinch, ⌘-scroll).
    pub fn zoom_at(&mut self, zoom: f32, screen_pos: Pos2) {
        if !zoom.is_finite() || !screen_pos.is_finite() {
            return;
        }
        let zoom = zoom.clamp(0.08, 64.0);
        if zoom == self.zoom && self.fit == Fit::None {
            return;
        }
        // In gutters/gaps, anchor relative to the nearest page rather than jumping to its top.
        // Fractions outside 0..1 preserve the pointer's position outside the page as well.
        let anchor =
            self.screen_rects.iter().min_by(|(_, a), (_, b)| a.distance_sq_to_pos(screen_pos).total_cmp(&b.distance_sq_to_pos(screen_pos))).map(
                |(p, r)| {
                    let f = (screen_pos - r.min) / r.size();
                    (*p, f.x, f.y, screen_pos - self.viewport_screen.min)
                },
            );
        self.zoom = zoom;
        self.fit = Fit::None;
        match anchor {
            Some(a) => self.zoom_anchor = Some(a),
            // Nothing on screen yet (e.g. a launch option): align the current page instead.
            None => self.goto = Some((self.current, 0.0)),
        }
    }

    /// A zoom typed as a percentage ("150", "150%", " 62.5 % ") as a zoom factor; `None` unless it is a
    /// finite, positive number. `set_zoom` clamps it to the supported range.
    pub fn parse_zoom_percent(typed: &str) -> Option<f32> {
        let percent = typed.trim().trim_end_matches('%').trim_end().parse::<f32>().ok()?;
        (percent.is_finite() && percent > 0.0).then_some(percent / 100.0)
    }

    /// Standard zoom steps (as in Acrobat's zoom menu).
    pub fn zoom_step(&mut self, up: bool) {
        const STEPS: [f32; 17] = [0.1, 0.25, 0.33, 0.5, 0.66, 0.75, 1.0, 1.25, 1.5, 2.0, 3.0, 4.0, 6.0, 8.0, 12.0, 16.0, 32.0];
        let z = self.zoom;
        let next = if up { STEPS.iter().copied().find(|s| *s > z * 1.01) } else { STEPS.iter().rev().copied().find(|s| *s < z * 0.99) };
        if let Some(n) = next {
            self.set_zoom(n);
        }
    }

    /// Start collecting demand from every consumer before scheduling or uploading results.
    pub(crate) fn begin_render_frame(&mut self) {
        self.frame_geometry_changed = false;
        self.pages_panel = None;
        self.panel_gap = None;
        self.thumb_demand.clear();
        self.frame_queue.clear();
        self.frame_visible.clear();
    }

    pub(crate) fn need_thumbnail(&mut self, page: usize, visible: bool) {
        if page >= self.page_count {
            return;
        }
        if let Some(priority) = self.thumb_demand.get_mut(&page) {
            *priority |= visible;
        } else if self.thumb_demand.len() < MAX_THUMB_DEMAND {
            self.thumb_demand.insert(page, visible);
        } else if visible && let Some(prefetch) = self.thumb_demand.iter().find_map(|(&p, &v)| (!v).then_some(p)) {
            self.thumb_demand.remove(&prefetch);
            self.thumb_demand.insert(page, true);
        }
    }

    /// Only the displayed document owns raster textures. Tabs keep their document and view
    /// state, but do not each retain a separate full application texture allowance.
    pub(crate) fn suspend_rendering(&mut self, pool: &RenderPool) {
        self.begin_render_frame();
        self.pages.clear();
        self.tiles.clear();
        self.thumbs.clear();
        self.grid_pages.clear();
        self.print_pages.clear();
        self.stale_thumbs.clear();
        self.stale_tiles.clear();
        self.waiting_since.clear();
        // Signature previews own a separate renderer and image/background textures.
        self.signature_drag = Default::default();
        self.text_background = Default::default();
        self.move_background = Default::default();
        if !self.last_queue.is_empty() {
            pool.set_queue(Vec::new());
            self.last_queue.clear();
        }
        // Results already finished for this tab would otherwise sit in its pool, rasters and
        // all, until the tab is shown again. (An inline pool renders inside try_recv: skip it.)
        if !pool.is_inline() {
            while pool.try_recv().is_some() {}
        }
    }

    fn thumbnail_requests(&self, info: &DocInfo, ppp: f32) -> Vec<RenderRequest> {
        let mut pages: Vec<_> = self.thumb_demand.iter().map(|(&p, &visible)| (p, visible)).collect();
        pages.sort_unstable_by_key(|&(p, visible)| (!visible, p));
        // At high DPI or in a very large viewport every demanded cell still fits the same
        // byte allowance. Both dimensions are capped, including tall/wide page boxes.
        // The count is rounded up to a power of two: the scale is part of each request's tag,
        // so a density that followed every row scrolling in or out would re-render them all.
        let density = thumbnail_density(pages.len(), ppp);
        pages
            .into_iter()
            .filter_map(|(page, _)| {
                let p = info.pages.get(page)?;
                let scale = thumbnail_scale(p.width, p.height, density);
                Some(RenderRequest {
                    page,
                    kind: RequestKind::Pixels,
                    tile: None,
                    scale,
                    tag: THUMB_TAG | u64::from(scale.to_bits()),
                    // Thumbnails are previews: tiny images are skipped for them.
                    preview: true,
                })
            })
            .collect()
    }

    /// Admit current demand within explicit byte limits. Cached and pending surfaces share
    /// the allowance, so an evicted page can be requested again without a permanent done set.
    fn prepare_render_queue(&mut self, info: &DocInfo, ppp: f32) -> Vec<RenderRequest> {
        let requests = std::mem::take(&mut self.frame_queue);
        let mut queue = Vec::new();
        let mut pages = HashSet::new();
        let mut tiles = HashSet::new();
        let mut tile_tags = HashSet::new();
        let mut grid = Vec::new();
        let (mut page_bytes, mut tile_bytes) = (0usize, 0usize);
        for req in requests.iter().filter(|r| r.kind == RequestKind::Pixels) {
            let Some(p) = info.pages.get(req.page) else { continue };
            // Organize-grid renders: only the cells in view that need one (organize_grid).
            if req.tag & GRID_TAG != 0 {
                grid.push(*req);
                continue;
            }
            // Print-preview rasters: a few sheets at the pane's size (the dialog bounds them).
            if req.tag & PRINT_TAG != 0 {
                if !self.errors.contains_key(&req.page) {
                    queue.push(*req);
                }
                continue;
            }
            // On-screen work is always admitted (the viewport bounds it); the limits only
            // decide how much prefetch fits beside it.
            let visible = self.frame_visible.contains(&req.page);
            if let Some(tile) = req.tile {
                // Keyed by exact device-pixel origin: the tile side varies with the zoom
                // (`tile_side`), so grid indices would need the scale to decode.
                let key = (req.page, req.tag, tile.x, tile.y);
                let bytes = rgba_bytes(tile.w as usize, tile.h as usize);
                if (!visible && tile_bytes.saturating_add(bytes) > TILE_BYTES) || !tiles.insert(key) {
                    continue;
                }
                tile_bytes = tile_bytes.saturating_add(bytes);
                tile_tags.insert(req.tag);
                if !self.tiles.contains_key(&key) || self.stale_tiles.contains(&key) {
                    queue.push(*req);
                }
            } else {
                let bytes = rgba_bytes(device_pixels(p.width, req.scale) as usize, device_pixels(p.height, req.scale) as usize);
                if (!visible && (page_bytes.saturating_add(bytes) > PAGE_BYTES || pages.len() >= MAX_CACHED_PAGES)) || !pages.insert(req.page) {
                    continue;
                }
                page_bytes = page_bytes.saturating_add(bytes);
                if self.pages.get(&req.page).is_none_or(|p| p.tag != req.tag) {
                    queue.push(*req);
                }
            }
        }
        // Pages no longer demanded stay cached while the allowance has room, nearest to the
        // current page first, so turning back a page shows it at once instead of rendering.
        let mut undemanded: Vec<usize> = self.pages.keys().copied().filter(|p| !pages.contains(p)).collect();
        undemanded.sort_by_key(|p| p.abs_diff(self.current));
        let mut kept = pages.len();
        for page in undemanded {
            let bytes = self.pages.get(&page).map_or(0, |p| texture_bytes(&p.tex));
            if kept < MAX_CACHED_PAGES && page_bytes.saturating_add(bytes) <= PAGE_BYTES {
                page_bytes = page_bytes.saturating_add(bytes);
                kept += 1;
                pages.insert(page);
            }
        }
        self.pages.retain(|p, _| pages.contains(p));
        // Tiles of an earlier zoom (not a demanded scale) stay while the canvas keeps them, a
        // bounded few under a page whose current tiles are still missing (OLD_TILES). Tiles of a
        // page on screen stay too even when this frame does not demand them (#2196): the viewport
        // moves while zooming, so a tile that was sharp a moment ago falls outside the demanded
        // rectangle, and dropping it made every pan or zoom back a cold miss that re-blurred the
        // page and re-interpreted it once per tile. The per-frame cap below (OLD_TILES,
        // nearest scale first) is what bounds how much of that stays.
        self.tiles.retain(|key, _| tiles.contains(key) || !tile_tags.contains(&key.1) || self.frame_visible.contains(&key.0));
        self.stale_tiles.retain(|key| self.tiles.contains_key(key));
        queue.extend(grid);
        // Opening (or turning to) a page queues its pixels alongside thumbnails and
        // text extraction. The workers would otherwise spend the first seconds on
        // thumbnails and search text while the visible page stays blank: hold those
        // until no visible main-page raster is still missing. Errored pages never
        // block: they are never re-requested.
        let visible_mains_pending = queue.iter().any(|r| {
            r.kind == RequestKind::Pixels
                && r.tag & (GRID_TAG | PRINT_TAG | THUMB_TAG | TEXT_TAG) == 0
                && self.frame_visible.contains(&r.page)
                && !self.errors.contains_key(&r.page)
        });
        if visible_mains_pending {
            // Caches are kept; demand is re-evaluated next frame once pixels arrive.
            return queue;
        }
        let thumbs = self.thumbnail_requests(info, ppp);
        let mut thumb_bytes = 0usize;
        let mut admitted = HashSet::new();
        for req in thumbs {
            let Some(p) = info.pages.get(req.page) else { continue };
            let bytes = rgba_bytes(device_pixels(p.width, req.scale) as usize, device_pixels(p.height, req.scale) as usize);
            if thumb_bytes.saturating_add(bytes) > THUMB_BYTES || self.errors.contains_key(&req.page) {
                continue;
            }
            thumb_bytes += bytes;
            admitted.insert(req.page);
            if self.thumbs.get(&req.page).is_none_or(|t| t.tag != req.tag) || self.stale_thumbs.contains(&req.page) {
                queue.push(req);
            }
        }
        self.thumbs.retain(|p, _| admitted.contains(p));
        self.stale_thumbs.retain(|p| admitted.contains(p));
        self.retain_text_layers();
        // Visible main pages and thumbnails take precedence over background search work.
        queue.extend(requests.into_iter().filter(|r| r.kind == RequestKind::Text));
        queue
    }

    /// Drop text layers nothing can still need. Every visited page's text used to stay
    /// cached for the session. Keep visible, selected and matched pages plus the nearest
    /// prefetch window; evicted pages re-extract on revisit. (Count window only: unlike
    /// rasters there is no byte budget here.) While a search is active keep everything:
    /// demand still wants every page, so evicting would re-request evicted pages each
    /// frame and churn the workers. Failures are never evicted either: they cost nothing
    /// to remember, and evicting one would re-request and re-fail it every frame.
    fn retain_text_layers(&mut self) {
        if self.texts.len() <= MAX_CACHED_TEXT_PAGES {
            return;
        }
        if self.find.as_ref().is_some_and(|f| !f.case_query.trim().is_empty()) {
            return;
        }
        let mut keep: HashSet<usize> = self.frame_visible.clone();
        if let Some(s) = self.selection {
            keep.insert(s.page);
        }
        if let Some(f) = self.find.as_ref() {
            keep.extend(f.matches.iter().map(|(p, _)| *p));
        }
        let mut rest: Vec<usize> = self.texts.keys().filter(|p| !keep.contains(*p)).copied().collect();
        rest.sort_by_key(|p| p.abs_diff(self.current));
        keep.extend(rest.into_iter().take(MAX_CACHED_TEXT_PAGES));
        self.texts.retain(|p, _| keep.contains(p));
    }

    pub(crate) fn finish_render_frame(&mut self, ctx: &egui::Context, info: &DocInfo, pool: &RenderPool) {
        if self.frame_geometry_changed {
            // Dialogs and edits can replace pages after the canvas collected demand. Its
            // scales and indexes belong to the old pages, not to the new renderer (#3149).
            self.begin_render_frame();
            self.last_queue.clear();
            pool.set_queue(Vec::new());
            ctx.request_repaint();
            return;
        }
        let queue = self.prepare_render_queue(info, ctx.pixels_per_point());
        if queue != self.last_queue {
            pool.set_queue(queue.clone());
            self.last_queue = queue;
        }
        if !self.hold_rasters {
            self.receive(ctx, pool);
        }
        if !self.last_queue.is_empty() {
            ctx.request_repaint_after(std::time::Duration::from_millis(30));
        }
    }

    /// Pull a bounded batch of current results into textures. Demand is committed first, so
    /// offscreen results and obsolete scale/configuration tags never allocate a ColorImage.
    pub fn receive(&mut self, ctx: &egui::Context, pool: &RenderPool) {
        let budget = if pool.is_inline() { 1 } else { RESULTS_PER_FRAME };
        let mut uploaded = 0usize;
        #[cfg(not(target_arch = "wasm32"))]
        let started = std::time::Instant::now();
        for _ in 0..budget {
            let Some(r) = pool.try_recv() else { break };
            uploaded = uploaded.saturating_add(self.receive_result(ctx, r));
            // One whole-page upload may exceed the frame allowance; it must still progress.
            if uploaded >= UPLOAD_BYTES_PER_FRAME {
                break;
            }
            #[cfg(not(target_arch = "wasm32"))]
            if started.elapsed() >= std::time::Duration::from_millis(4) {
                break;
            }
        }
    }

    fn receive_result(&mut self, ctx: &egui::Context, r: pdfcraft_render::RenderedPage) -> usize {
        if !self.last_queue.contains(&r.request) {
            return 0;
        }
        ctx.request_repaint();
        if r.request.kind == RequestKind::Text {
            match r.text {
                Some(t) => {
                    self.texts.insert(r.request.page, t);
                    self.refresh_find_page(r.request.page);
                }
                None => {
                    log::warn!("page {} text: {}", r.request.page + 1, r.error.unwrap_or_default());
                    self.text_failed.insert(r.request.page);
                    if let Some(f) = self.find.as_mut() {
                        let current_key = f.current.and_then(|c| f.matches.get(c).cloned());
                        f.matches.retain(|(p, _)| *p != r.request.page);
                        f.current = current_key.and_then(|k| f.matches.iter().position(|m| *m == k));
                        if f.current.is_none() && !f.matches.is_empty() {
                            let cur = self.current;
                            f.current = Some(f.matches.iter().position(|(p, _)| *p >= cur).unwrap_or(0));
                        }
                    }
                }
            }
            return 0;
        }
        if let Some(e) = r.error {
            log::warn!("page {}: {e}", r.request.page + 1);
            self.errors.insert(r.request.page, e);
            return 0;
        }
        // Even current demand may have been sized with obsolete geometry or a different
        // device limit. Check before cache eviction, conversion and every upload path.
        let max_side = ctx.input(|i| i.max_texture_side);
        if r.width == 0 || r.height == 0 || r.width as usize > max_side || r.height as usize > max_side {
            return 0;
        }
        // Remember heavy pages: only these get the zoom/pan settle holds, so fast pages
        // never pay added latency. Stale entries die with the tiles on edits.
        if r.millis >= SLOW_MILLIS {
            self.slow_pages.insert(r.request.page);
        }
        let page = r.request.page;
        let bytes = rgba_bytes(r.width as usize, r.height as usize);
        if bytes != r.rgba.len() || !self.admit_texture(r.request, bytes) {
            return 0;
        }
        // The length was checked above, so the renderer's buffer becomes the texture data as is.
        let img = texture_image([r.width as usize, r.height as usize], r.rgba);
        if let Some(t) = r.request.tile {
            let key = (page, r.request.tag, t.x, t.y);
            let tex = ctx.load_texture(format!("tile-{:?}-{page}-{}-{}", self.id, t.x, t.y), img, TextureOptions::LINEAR);
            self.tiles.insert(key, tex);
            self.stale_tiles.remove(&key);
            if !self.stale_tiles.iter().any(|k| k.0 == page) {
                self.forms.unbaked.retain(|(_, p)| *p != page);
                self.comments.page_rendered(page);
            }
            if !self.page_out_of_date(page) {
                self.text_background.page_received(page);
            }
        } else if r.request.tag & PRINT_TAG != 0 {
            let tex = ctx.load_texture(
                format!("print-{:?}-{page}", self.id),
                if r.request.tag & GRAYSCALE_TAG != 0 { grayscale(img) } else { img },
                TextureOptions::LINEAR,
            );
            self.print_pages.insert(page, (r.request.tag, tex));
        } else if r.request.tag & GRID_TAG != 0 {
            let tex = ctx.load_texture(format!("grid-{:?}-{page}", self.id), img, TextureOptions::LINEAR);
            self.grid_pages.insert(page, (r.width, tex));
        } else if r.request.tag & THUMB_TAG != 0 {
            let tex = ctx.load_texture(format!("thumb-{:?}-{page}", self.id), img, TextureOptions::LINEAR);
            self.thumbs.insert(page, PageTex { tag: r.request.tag, tex });
            self.stale_thumbs.remove(&page);
        } else {
            let tex = ctx.load_texture(format!("page-{:?}-{page}", self.id), img, TextureOptions::LINEAR);
            self.pages.insert(page, PageTex { tag: r.request.tag, tex });
            self.signature_drag.page_received(page);
            self.move_background.page_received(page);
            if !self.page_out_of_date(page) {
                self.text_background.page_received(page);
            }
            self.comments.page_rendered(page);
            self.waiting_since.remove(&page);
            if !self.stale_tiles.iter().any(|k| k.0 == page) {
                self.forms.unbaked.retain(|(_, p)| *p != page);
            }
        }
        bytes
    }

    /// Check real dimensions before conversion, including stale textures kept during edits.
    /// Dropping an old handle releases its backend allocation after the current frame.
    fn admit_texture(&mut self, req: RenderRequest, bytes: usize) -> bool {
        // An on-screen page or tile is always taken (the viewport bounds it): it may evict
        // off-screen rasters but never another on-screen one, and is never refused.
        let visible = req.tag & THUMB_TAG == 0 && self.frame_visible.contains(&req.page);
        // Organize-grid renders are bounded by the cells in view (organize_grid). Print
        // previews are the sheets around the one on screen.
        if req.tag & (GRID_TAG | PRINT_TAG) != 0 {
            return true;
        }
        if let Some(t) = req.tile {
            let key = (req.page, req.tag, t.x, t.y);
            self.tiles.remove(&key);
            let retained = self.tiles.values().fold(0usize, |n, tex| n.saturating_add(texture_bytes(tex)));
            visible || bytes <= TILE_BYTES.saturating_sub(retained)
        } else {
            let thumb = req.tag & THUMB_TAG != 0;
            let (cache, limit) = if thumb { (&mut self.thumbs, THUMB_BYTES) } else { (&mut self.pages, PAGE_BYTES) };
            cache.remove(&req.page);
            let mut retained = cache.values().fold(0usize, |n, p| n.saturating_add(texture_bytes(&p.tex)));
            // A zoom/DPI change can temporarily retain larger stale images than the new
            // demand estimates. Evict those first, then let the current demand refill them.
            let on_screen = |p: &usize| !thumb && self.frame_visible.contains(p);
            while retained.saturating_add(bytes) > limit {
                let Some(page) = cache.keys().copied().filter(|p| !on_screen(p)).max_by_key(|p| p.abs_diff(self.current)) else { break };
                if let Some(old) = cache.remove(&page) {
                    retained = retained.saturating_sub(texture_bytes(&old.tex));
                }
            }
            visible || bytes <= limit.saturating_sub(retained)
        }
    }

    /// The first page of the two-page spread page `c` is in: spreads are [0, 1], [2, 3], … or,
    /// with a cover page, [0], [1, 2], [3, 4], ….
    fn spread_first(&self, c: usize) -> usize {
        if self.cover { c.saturating_sub((c + 1) % 2) } else { c - c % 2 }
    }

    fn fit_zoom(&mut self, info: &DocInfo) {
        let largest = |side: fn((f32, f32)) -> f32| info.pages.iter().map(|p| side(self.display_size(p))).fold(1.0, f32::max);
        let max_w = largest(|s| s.0);
        // A single page shown on its own fits that page, and full screen in two-page view fits
        // the spread it shows (review on #2070). Every other display (scrolling, or two-page
        // view in the window) fits its largest page, so the zoom holds still while pages of
        // other sizes scroll or page past.
        let (w, h) = if self.layout == PageLayout::Single && !self.continuous {
            info.pages.get(self.current).map_or_else(|| (max_w, largest(|s| s.1)), |p| self.display_size(p))
        } else if self.presentation.is_some() && self.layout == PageLayout::TwoUp && !self.continuous {
            let first = self.spread_first(self.current);
            let last = if self.cover && first == 0 { 0 } else { first.saturating_add(1) };
            let spread: Vec<(f32, f32)> = info.pages.iter().skip(first).take(last - first + 1).map(|p| self.display_size(p)).collect();
            if spread.is_empty() {
                (max_w, largest(|s| s.1))
            } else {
                (spread.iter().map(|s| s.0).fold(1.0, f32::max), spread.iter().map(|s| s.1).fold(1.0, f32::max))
            }
        } else {
            (max_w, largest(|s| s.1))
        };
        let avail_w = (self.viewport_w - 2.0 * SIDE).max(100.0);
        let per_row = if self.layout == PageLayout::TwoUp { 2.0 } else { 1.0 };
        match self.fit {
            Fit::Width => self.zoom = (avail_w - self.page_gap() * (per_row - 1.0)) / (w * PT * per_row),
            Fit::Page => {
                let zw = (avail_w - self.page_gap() * (per_row - 1.0)) / (w * PT * per_row);
                let zh = (self.viewport_h - 2.0 * MARGIN) / (h * PT);
                self.zoom = zw.min(zh);
            }
            Fit::Height => self.zoom = (self.viewport_h - 2.0 * MARGIN) / (h * PT),
            Fit::None => {}
        }
        self.zoom = self.zoom.clamp(0.08, 64.0);
    }

    /// Width of the scroll content. Single-page view sizes it to the page it shows, so a
    /// wider page elsewhere in the document can't push the shown one off-centre.
    fn content_width(&self, info: &DocInfo, avail_w: f32) -> f32 {
        let single = (self.layout == PageLayout::Single && !self.continuous).then(|| self.current.min(info.pages.len().saturating_sub(1)));
        let shown = info.pages.iter().enumerate().filter(|&(i, _)| single.is_none_or(|c| c == i));
        let max_w = shown.map(|(_, p)| self.display_size(p).0).fold(0.0, f32::max);
        let per_row = if self.layout == PageLayout::TwoUp { 2.0 } else { 1.0 };
        (max_w * self.zoom * PT * per_row + 2.0 * SIDE).max(avail_w)
    }

    /// Page rects in content coordinates (origin at the scroll content's top-left).
    fn layout(&self, info: &DocInfo, content_w: f32, ppp: f32) -> Vec<Rect> {
        let s = self.zoom * PT;
        let gap = self.page_gap();
        let page_size = |p: &pdfcraft_render::PageInfo| {
            let (w, h) = self.display_size(p);
            if self.show_page_gaps {
                vec2(w * s, h * s)
            } else {
                // Match the raster's whole physical pixels. Snapping each page's origin
                // independently with fractional sizes would leave hairline seams.
                vec2(device_pixels(w, s * ppp) as f32 / ppp, device_pixels(h, s * ppp) as f32 / ppp)
            }
        };
        let mut rects = Vec::with_capacity(info.pages.len());
        let mut y = MARGIN;
        match self.layout {
            PageLayout::Single => {
                for p in &info.pages {
                    let size = page_size(p);
                    rects.push(Rect::from_min_size(pos2(((content_w - size.x) / 2.0).max(SIDE), y), size));
                    y += size.y + gap;
                }
            }
            PageLayout::TwoUp => {
                // With a cover page, the first page sits alone on the right.
                let rows: Vec<&[pdfcraft_render::PageInfo]> = if self.cover && !info.pages.is_empty() {
                    std::iter::once(&info.pages[..1]).chain(info.pages[1..].chunks(2)).collect()
                } else {
                    info.pages.chunks(2).collect()
                };
                for (ri, pair) in rows.into_iter().enumerate() {
                    let sizes: Vec<Vec2> = pair.iter().map(page_size).collect();
                    let row_w: f32 = sizes.iter().map(|v| v.x).sum::<f32>() + gap * (sizes.len() as f32 - 1.0);
                    let row_h = sizes.iter().map(|v| v.y).fold(0.0, f32::max);
                    let mut x = if self.cover && ri == 0 { (content_w / 2.0 + gap / 2.0).max(SIDE) } else { ((content_w - row_w) / 2.0).max(SIDE) };
                    for size in sizes {
                        rects.push(Rect::from_min_size(pos2(x, y + (row_h - size.y) / 2.0), size));
                        x += size.x + gap;
                    }
                    y += row_h + gap;
                }
            }
        }
        rects
    }

    fn render_scale(&self, ppp: f32) -> f32 {
        // Exactly the device scale: a raster at any other scale is resampled on screen, which
        // blurs every line and glyph (#260).
        self.zoom * PT * ppp
    }

    /// Set whether the print preview rasters become luminance before upload. Toggling it drops
    /// the colour previews: new requests carry the grayscale tag, so stale ones are replaced.
    pub(crate) fn set_print_grayscale(&mut self, grayscale: bool) {
        if self.grayscale != grayscale {
            self.grayscale = grayscale;
            self.print_pages.clear();
        }
    }

    /// Queue sharp rasters for the print preview. Pages that leave the nearby sheets are dropped.
    /// Previews render with reading colours off (the request's tag tells the renderer): they must
    /// match the printout, which is imposed from the uncoloured document.
    pub(crate) fn queue_print_previews(&mut self, pages: &[(usize, f32)]) {
        self.print_pages.retain(|page, _| pages.iter().any(|(p, _)| p == page));
        for &(page, scale) in pages {
            if self.errors.contains_key(&page) || !scale.is_finite() || scale <= 0.0 {
                continue;
            }
            let tag = scale_tag(scale) | PRINT_TAG | if self.grayscale { GRAYSCALE_TAG } else { 0 };
            let fresh = self.print_pages.get(&page).is_some_and(|(have, _)| *have == tag);
            if !fresh {
                self.frame_queue.push(RenderRequest { page, kind: RequestKind::Pixels, tile: None, scale, tag, preview: false });
            }
        }
    }
}

/// The hover popup for an annotation. Replies and Fill & Sign marks (typed text, checks, signatures)
/// show none: they are page content being edited, not comments.
fn annotation_hover_text(a: &pdfcraft_render::Annotation) -> Option<String> {
    if a.in_reply_to.is_some() || a.fill_sign {
        return None;
    }
    let who = a.author.clone().unwrap_or_else(|| a.subtype.clone());
    let body = a.contents.clone().unwrap_or_default();
    Some(if body.is_empty() { who } else { format!("{who}\n{body}") })
}

/// The scroll offset that puts `point` (content coordinates) at `rel` in the viewport, kept within
/// the content as egui keeps it. egui clamps only after drawing, so an offset past the end showed
/// the page misplaced for a frame and then jumped (zooming out beside a page that fits).
fn anchored_scroll(point: Pos2, rel: Vec2, content: Vec2, viewport: Vec2) -> Vec2 {
    let max = vec2((content.x - viewport.x).max(0.0), (content.y - viewport.y).max(0.0));
    vec2((point.x - rel.x).clamp(0.0, max.x), (point.y - rel.y).clamp(0.0, max.y))
}

/// The request tag for a raster at `scale`: equal tags mean the same scale (to 1/65536).
fn scale_tag(scale: f32) -> u64 {
    (f64::from(scale) * 65536.0).round() as u64
}

/// The scale of a raster tagged by [`scale_tag`].
fn tag_scale(tag: u64) -> f32 {
    (tag as f64 / 65536.0) as f32
}

/// Device-pixel side of a page tile at `scale` for a page `page_max_px` device pixels on
/// its long side. Each tile costs a full page interpret, so tiles grow with the zoom and
/// with the page: a zoomed viewport, or a giant sheet shown whole, is covered by a few
/// large tiles instead of dozens of small ones. Bounded by the render backend (tile sides
/// above 4096 are clamped there) and kept granular at ordinary zooms.
fn tile_side(scale: f32, page_max_px: u32) -> u32 {
    if !scale.is_finite() || scale <= 0.0 {
        return TILE;
    }
    let by_zoom = (TILE as f32 * (scale / 6.0)).clamp(TILE as f32, 4096.0) as u32;
    // At most four tiles along the long side, so a giant sheet needs a handful of
    // interprets instead of dozens.
    by_zoom.max(page_max_px.div_ceil(4).min(4096))
}

/// Tiles of earlier zooms kept for a page while its current tiles are rendered.
const OLD_TILES: usize = 64;

/// A page whose raster took at least this long is heavy: zoom/pan holds apply to it,
/// so gestures stay on stretched surfaces instead of queueing full-page interprets.
const SLOW_MILLIS: u32 = 500;

/// `r` moved so its corner lies on a whole physical pixel, so that a raster drawn from there
/// maps texel for texel onto the screen.
fn snap_to_pixels(r: Rect, ppp: f32) -> Rect {
    let snap = |v: f32| (v * ppp).round() / ppp;
    Rect::from_min_size(pos2(snap(r.min.x), snap(r.min.y)), r.size())
}

/// Maps between a page's coordinate spaces and the screen, including view rotation.
///
/// *View space* is the page as rendered (points, y down, the document's own `/Rotate` applied);
/// normalised page coordinates `(u, v)` are view space divided by the page size. The view
/// rotation (View ▸ Rotate View) turns the page clockwise on screen by `rot` degrees.
#[derive(Clone, Copy)]
pub struct PageXform {
    /// The page's rectangle on screen (already rotated, so width/height may be swapped).
    pub rect: Rect,
    pub rot: u16,
    /// Page size in view space (unrotated by the view).
    pub pw: f32,
    pub ph: f32,
}

impl PageXform {
    /// Screen pixels per page point. The view's rotation swaps the rectangle's sides, so a page
    /// turned a quarter shows its width down the screen.
    pub(crate) fn zoom(&self) -> f32 {
        let across = if self.rot % 180 == 90 { self.rect.height() } else { self.rect.width() };
        across / self.pw.max(1.0)
    }

    /// Draw an image in the PDF user-space `rect` as a custom stamp's appearance draws it: turned
    /// back by `turn` (the page /Rotate it was placed for), so with the page's own rotation it
    /// reads upright as displayed.
    pub(crate) fn paint_user_image(
        &self,
        painter: &egui::Painter,
        tex: egui::TextureId,
        p: &pdfcraft_render::PageInfo,
        rect: [f64; 4],
        turn: i64,
        color: Color32,
    ) {
        let mut mesh = egui::Mesh::with_texture(tex);
        let (w, h) = (rect[2] - rect[0], rect[3] - rect[1]);
        let (shown_w, shown_h) = if turn % 180 == 0 { (w, h) } else { (h, w) };
        let [a, b, c, d, e, f] = pdfcraft_model::view_matrix_for(turn, rect);
        for (u, v) in [(0.0_f32, 0.0_f32), (1.0, 0.0), (1.0, 1.0), (0.0, 1.0)] {
            // The texture's (u, v), from its top-left, in the picture's upright frame, then in user space.
            let (dx, dy) = (f64::from(u) * shown_w, f64::from(1.0 - v) * shown_h);
            let (x, y) = (a * dx + c * dy + e, b * dx + d * dy + f);
            let q = p.user_to_view(x as f32, y as f32);
            mesh.vertices.push(egui::epaint::Vertex { pos: self.norm_to_screen(q[0] / self.pw, q[1] / self.ph), uv: pos2(u, v), color });
        }
        mesh.add_triangle(0, 1, 2);
        mesh.add_triangle(0, 2, 3);
        painter.add(egui::Shape::mesh(mesh));
    }
    pub fn norm_to_screen(&self, u: f32, v: f32) -> Pos2 {
        let (a, b) = match self.rot {
            90 => (1.0 - v, u),
            180 => (1.0 - u, 1.0 - v),
            270 => (v, 1.0 - u),
            _ => (u, v),
        };
        pos2(self.rect.left() + a * self.rect.width(), self.rect.top() + b * self.rect.height())
    }

    pub fn screen_to_norm(&self, p: Pos2) -> (f32, f32) {
        let (a, b) = ((p.x - self.rect.left()) / self.rect.width().max(1e-3), (p.y - self.rect.top()) / self.rect.height().max(1e-3));
        match self.rot {
            90 => (b, 1.0 - a),
            180 => (1.0 - a, 1.0 - b),
            270 => (1.0 - b, a),
            _ => (a, b),
        }
    }

    /// Screen point → view space (points).
    pub fn screen_to_view(&self, p: Pos2) -> (f32, f32) {
        let (u, v) = self.screen_to_norm(p);
        (u * self.pw, v * self.ph)
    }

    /// A view-space rect [x0, y0, x1, y1] → screen rect.
    pub fn view_rect(&self, g: [f32; 4]) -> Rect {
        Rect::from_two_pos(self.norm_to_screen(g[0] / self.pw, g[1] / self.ph), self.norm_to_screen(g[2] / self.pw, g[3] / self.ph))
    }

    /// A PDF user-space rect (crop box, document `/Rotate`) → screen rect.
    pub fn user_rect(&self, info: &DocInfo, page: usize, r: [f32; 4]) -> Rect {
        let p = &info.pages[page];
        let [cx0, cy0, cx1, cy1] = p.crop;
        let (cw, ch) = ((cx1 - cx0).max(1.0), (cy1 - cy0).max(1.0));
        let norm = |x: f32, y: f32| -> (f32, f32) {
            let (u, v) = ((x - cx0) / cw, (cy1 - y) / ch);
            match p.rotation {
                90 => (1.0 - v, u),
                180 => (1.0 - u, 1.0 - v),
                270 => (v, 1.0 - u),
                _ => (u, v),
            }
        };
        let (a, b) = (norm(r[0], r[1]), norm(r[2], r[3]));
        Rect::from_two_pos(self.norm_to_screen(a.0, a.1), self.norm_to_screen(b.0, b.1))
    }

    /// This transform resized to a raster of `px` device pixels (width, height before the view
    /// rotation), so each of its texels covers one screen pixel. The rectangle's corner must
    /// already be on a whole pixel ([`snap_to_pixels`]).
    fn texel_aligned(&self, px: [usize; 2], ppp: f32) -> PageXform {
        let (w, h) = (px[0] as f32 / ppp, px[1] as f32 / ppp);
        let size = if self.rot % 180 == 90 { vec2(h, w) } else { vec2(w, h) };
        PageXform { rect: Rect::from_min_size(self.rect.min, size), ..*self }
    }

    /// Draw a texture covering the normalised page region, rotated with the view.
    pub fn paint_image(&self, painter: &egui::Painter, tex: egui::TextureId, u0: f32, v0: f32, u1: f32, v1: f32) {
        let mut mesh = egui::Mesh::with_texture(tex);
        let corners = [(u0, v0, 0.0, 0.0), (u1, v0, 1.0, 0.0), (u1, v1, 1.0, 1.0), (u0, v1, 0.0, 1.0)];
        for (u, v, tu, tv) in corners {
            mesh.vertices.push(egui::epaint::Vertex { pos: self.norm_to_screen(u, v), uv: pos2(tu, tv), color: Color32::WHITE });
        }
        mesh.add_triangle(0, 1, 2);
        mesh.add_triangle(0, 2, 3);
        painter.add(egui::Shape::mesh(mesh));
    }
}

pub fn shortcuts(view: &mut DocView, ctx: &egui::Context, speed: crate::scroll_speed::ScrollSpeed) {
    use egui::{Key, KeyboardShortcut, Modifiers};
    if ctx.input_mut(|i| i.consume_shortcut(&KeyboardShortcut::new(Modifiers::COMMAND, Key::F))) {
        view.open_find();
    }
    let cmd = |k| KeyboardShortcut::new(Modifiers::COMMAND, k);
    let pressed = |s: KeyboardShortcut| ctx.input_mut(|i| i.consume_shortcut(&s));
    // ⌘G / ⇧⌘G step through the matches from the find field too, as in Acrobat and Preview
    // (#2669): ahead of the text-input check, but only while there is a search to step.
    if view.find.is_some() {
        if pressed(KeyboardShortcut::new(Modifiers::COMMAND | Modifiers::SHIFT, Key::G)) {
            view.find_step(false);
        } else if pressed(cmd(Key::G)) {
            view.find_step(true);
        }
    }
    // A focused check box or radio button holds egui's keyboard focus only for Space, and a
    // radio button for the arrows that move within its group: the other page keys still work.
    let form_box = view.forms.keys.filter(|(id, _)| ctx.memory(|m| m.focused()) == Some(*id));
    if ctx.egui_wants_keyboard_input() && form_box.is_none() {
        return;
    }
    let arrows = !form_box.is_some_and(|(_, radio)| radio);
    // Rotation first: egui matches ⌘+ loosely with respect to Shift.
    if pressed(KeyboardShortcut::new(Modifiers::COMMAND | Modifiers::SHIFT, Key::Plus))
        || pressed(KeyboardShortcut::new(Modifiers::COMMAND | Modifiers::SHIFT, Key::Equals))
    {
        view.rotate_view(true);
    }
    if pressed(KeyboardShortcut::new(Modifiers::COMMAND | Modifiers::SHIFT, Key::Minus)) {
        view.rotate_view(false);
    }
    // In the page grid ⌘+ / ⌘− / ⌘0 size its pages instead (looking is always allowed).
    let (zoom_in, zoom_out, zoom_reset) = (pressed(cmd(Key::Plus)) || pressed(cmd(Key::Equals)), pressed(cmd(Key::Minus)), pressed(cmd(Key::Num0)));
    if view.organize {
        if zoom_in {
            view.grid_zoom_request = Some(view.grid_zoom * GRID_ZOOM_STEP);
        }
        if zoom_out {
            view.grid_zoom_request = Some(view.grid_zoom / GRID_ZOOM_STEP);
        }
        if zoom_reset {
            view.grid_zoom_request = Some(1.0);
        }
    } else {
        if zoom_in {
            view.zoom_step(true);
        }
        if zoom_out {
            view.zoom_step(false);
        }
        if zoom_reset {
            view.fit = Fit::Page;
            view.goto = Some((view.current, 0.0));
        }
    }
    if pressed(cmd(Key::Num1)) {
        view.set_zoom(1.0);
    }
    if pressed(cmd(Key::Num2)) {
        view.fit = Fit::Width;
        view.goto = Some((view.current, 0.0));
    }
    if pressed(cmd(Key::OpenBracket)) {
        view.view_history(false);
    }
    if pressed(cmd(Key::CloseBracket)) {
        view.view_history(true);
    }
    let key = |k| (arrows || !matches!(k, Key::ArrowLeft | Key::ArrowRight | Key::ArrowUp | Key::ArrowDown)) && ctx.input(|i| i.key_pressed(k));
    // An open menu takes Escape first and closes; the selection stays for the next one (#3535).
    if key(Key::Escape) && !egui::Popup::is_any_open(ctx) {
        if view.camera.is_some() || view.camera_done.is_some() {
            view.pending_action = Some(ViewAction::CancelCamera);
        } else if view.selection.is_some() || view.all_text {
            view.selection = None;
            view.all_text = false;
        } else {
            view.find = None;
        }
    }
    if key(Key::Home) {
        view.go_to_page(0);
    }
    if key(Key::End) {
        view.go_to_page(usize::MAX);
    }
    // Full screen: Space and Enter go forward a page and Backspace back, as in Acrobat (the
    // arrows and Page Up / Page Down already turn pages where nothing scrolls, below).
    if view.presentation.is_some() {
        if key(Key::Space) || key(Key::Enter) {
            view.step_page(true);
        }
        if key(Key::Backspace) {
            view.step_page(false);
        }
    }
    // As in Acrobat: → / ← go to the next / previous page in every layout (#185), and so do
    // ⌘Page Down / ⌘Page Up, or plain Page Down / Page Up in single-page view.
    let command = ctx.input(|i| i.modifiers.command);
    let scrolls = view.continuous;
    if key(Key::ArrowRight) || (key(Key::PageDown) && (command || !scrolls)) {
        view.step_page(true);
    }
    if key(Key::ArrowLeft) || (key(Key::PageUp) && (command || !scrolls)) {
        view.step_page(false);
    }
    // ↓ / ↑ scroll a line, and Page Down / Page Up a screen where the pages scroll. In
    // single-page view, ↓ / ↑ turn the page once there is nothing left to scroll that way, as
    // the wheel does (#273): always on a page that fits.
    // The Scroll speed preference sets how far ↓ / ↑ go as well as the wheel (#985); Page Down /
    // Page Up keep a screen.
    if !command {
        let line = KEY_SCROLL_LINE * speed.factor();
        let screen = (view.viewport_h - KEY_SCROLL_LINE).max(KEY_SCROLL_LINE);
        let steps = [(Key::ArrowDown, line), (Key::ArrowUp, -line), (Key::PageDown, screen), (Key::PageUp, -screen)];
        for (k, by) in steps {
            let arrow = matches!(k, Key::ArrowDown | Key::ArrowUp);
            if !key(k) || !(scrolls || arrow) {
                continue;
            }
            let forward = k == Key::ArrowDown;
            // Only an up-to-date view counts: the page drawn is the current one, with no jump pending.
            let at_end = !scrolls
                && view.goto.is_none()
                && view.single_edges.is_some_and(|(page, top, bottom)| page == view.current && if forward { bottom } else { top });
            if at_end {
                let before = view.current;
                view.step_page(forward);
                if !forward && view.current != before {
                    // Going back up lands on the bottom of the previous page.
                    view.goto = Some((view.current, 1.0));
                }
                view.single_edges = None;
            } else {
                view.key_scroll += by;
            }
        }
    }
}

/// How far ↓ / ↑ scroll at Normal speed (Scroll speed halves or doubles it). Page Down / Page Up
/// keep this much of the previous screen in view.
const KEY_SCROLL_LINE: f32 = 40.0;
/// Suppression window for egui's eased copy of point input.
///
/// egui 0.36.2 drains `WheelState::after_events` tail deltas by 90% per 0.1 s.
const SUPPRESS_SECS: f64 = 0.25;
/// Short rolling extension while egui still has an unprocessed wheel tail.
const SUPPRESS_TAIL_SECS: f64 = 0.05;

// The viewer moves between docking and the central panel as modes change; keep its scroll
// identity tied to the document instead of either parent UI.
fn page_scroll<R>(
    ui: &mut egui::Ui,
    scroll: egui::ScrollArea,
    id: egui::Id,
    draw: impl FnOnce(&mut egui::Ui, egui::Rect) -> R,
) -> egui::scroll_area::ScrollAreaOutput<R> {
    ui.scope_builder(egui::UiBuilder::new().id(id), |ui| scroll.show_viewport(ui, draw)).inner
}

/// Draws the document tab at `index`: its notice bars, then its pages (or the Organize grid) in a
/// scroll area with this frame's zoom, scrolling, tool input, overlays and render requests.
pub fn document_area(app: &mut PdfCraftApp, index: usize, ui: &mut egui::Ui) {
    let t = Tokens::get(ui.ctx());
    let javascript_interaction = app.link_script_interaction_active();
    // Keyboard scrolling applies to this frame's page view only, never later.
    let key_scroll = std::mem::take(&mut app.views[index].key_scroll);
    // The Search panel closed: its search moves to the find bar.
    let search_open = app.right == Some(crate::RightPanel::Search);
    if let Some(f) = app.views[index].find.as_mut()
        && f.in_panel
        && !search_open
    {
        f.in_panel = false;
    }
    // The snapshot notice bar's Close was clicked: close here, where no document borrow is
    // held. The tab is looked up by id: indices may have shifted since the click, and the
    // flag rides its own view, so a stale flag can never close the wrong tab.
    let pending = app.views.get(index).filter(|v| v.close_requested).map(|v| v.id).and_then(|id| app.views.iter().position(|v| v.id == id));
    if let Some(i) = pending {
        app.views[i].close_requested = false;
        if app.has_unsaved_work(i) {
            app.close_request = Some(crate::CloseRequest::Tab(app.views[i].id));
        } else {
            app.close_tab(i);
        }
        return;
    }
    let Some(doc) = app.session.get(app.views[index].id) else { return };
    let info = &doc.info;
    let paper = app.session.reading_colors(app.views[index].id).and_then(|c| c.params).map_or(Color32::WHITE, |p| {
        let [r, g, b] = p.background;
        Color32::from_rgb(r, g, b)
    });
    if info.pages.is_empty() {
        app.views[index].auto_scroll.cancel();
        ui.centered_and_justified(|ui| ui.label(tl!("This document has no pages.")));
        return;
    }
    // The print dialog queues its own rasters later in the frame. Closing it drops them.
    if app.dialog != Some(crate::Dialog::Print) {
        app.views[index].print_pages.clear();
    }
    // The Prepare a form panel is open (or a field tool is picked): fields are edited, not filled.
    let preparing = app.is_preparing();
    // Prepare a form ▸ Calculation order: the order's steps and links on the page.
    let calc_order = preparing && app.left_open && app.left == crate::LeftPanel::CalculationOrder;
    // Edit a PDF: added text and images can be selected, moved and edited.
    let editing_content =
        (app.left_open && app.left == crate::LeftPanel::Tool("edit")) || matches!(app.quick_tool, QuickTool::AddText | QuickTool::EditText);
    let text_style = app.text_style.clone();
    let show_secured_notice = app.show_secured_notice;
    let view = &mut app.views[index];
    // Opened without the owner password and something is restricted. The notice says so unless
    // the user turned it off (#4035): the restrictions themselves, and Document Properties ▸
    // Security, stay as they are.
    let secured = show_secured_notice && doc.security_summary().is_some_and(|s| !(s.owner || (s.permissions.modify() && s.permissions.assemble())));
    let repaired = !doc.repair_log().is_empty();
    match notices(view, doc, secured, repaired, crate::sign_ui::banner(&doc.signatures), ui, &t) {
        #[cfg(not(target_arch = "wasm32"))]
        Some(Notice::Reload) => app.watch.reload_request = Some(view.id),
        #[cfg(target_arch = "wasm32")]
        Some(Notice::Reload) => {}
        Some(Notice::Repairs) => app.dialog = Some(crate::Dialog::Properties(crate::PropsTab::Advanced)),
        Some(Notice::Security) => app.dialog = Some(crate::Dialog::Properties(crate::PropsTab::Security)),
        Some(Notice::Unlock) => app.unlock_prompt = Some(crate::UnlockPrompt::new(doc, None)),
        // Flags the view: the tab closes at the top of the next frame, where no document
        // borrow is held.
        Some(Notice::Close) => {
            if let Some(view) = app.views.get_mut(index) {
                view.close_requested = true;
            }
        }
        Some(Notice::Signatures) => app.right = Some(RightPanel::Signatures),
        Some(Notice::FieldHighlights(on)) => app.view_defaults.highlight_fields = on,
        Some(Notice::Console) => app.dialog = Some(crate::Dialog::JsConsole),
        // Back on in Preferences ▸ General.
        Some(Notice::HideSecured) => app.show_secured_notice = false,
        None => {}
    }
    // No dialog, close prompt or palette over the page: only then does page input count.
    let unobstructed = !app.modal_open() && !app.palette_open;
    let organize_zoom = app.organize_zoom;
    let view = &mut app.views[index];
    if view.organize {
        // One remembered grid size for every document, kept in the settings.
        view.set_grid_zoom(organize_zoom);
        organize_grid(view, info, doc.allows_assembly(), doc.dirty, unobstructed, ui, &t);
        app.organize_zoom = view.grid_zoom();
        return;
    }

    let avail = ui.available_rect_before_wrap();
    view.viewport_w = avail.width();
    view.viewport_h = avail.height();
    let old_zoom = view.zoom;
    view.fit_zoom(info);
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
        if let Some((page, fy, rel_y)) = anchor {
            view.zoom_anchor = Some((page, 0.5, fy, vec2(avail.width() / 2.0, rel_y)));
        } else {
            view.zoom_anchor = Some((view.current, 0.5, 0.0, vec2(avail.width() / 2.0, MARGIN)));
        }
    }

    // Pinch / ctrl+scroll zoom, anchored on the current page.
    let zoom_delta = crate::motion::zoom_delta(ui.ctx());
    // Exclude the previous frame's scrollbar tracks from gesture activation.
    let navigation_rect = if view.viewport_screen.is_positive() { avail.intersect(view.viewport_screen) } else { avail };
    let navigation_enabled = app.dialog.is_none() && app.close_request.is_none() && !app.palette_open && ui.is_enabled() && ui.input(|i| i.focused);
    // Full screen: a click on the document goes forward a page and a shift-click back, as in
    // Acrobat, unless the click did something else (a link, a word selected). Decided below,
    // once the pages have had the click.
    let present_click = view.presentation.is_some()
        && navigation_enabled
        && ui.rect_contains_pointer(navigation_rect)
        && ui.input(|i| i.pointer.primary_clicked())
        && app.quick_tool == QuickTool::Select;
    // Shift as held now, or as the click's own event carried it (a synthetic click).
    let present_back = present_click
        && ui.input(|i| {
            i.modifiers.shift
                || i.events.iter().any(|e| {
                    matches!(e, egui::Event::PointerButton { button: egui::PointerButton::Primary, pressed: false, modifiers, .. } if modifiers.shift)
                })
        });
    if zoom_delta.is_finite() && zoom_delta > 0.0 && zoom_delta != 1.0 && navigation_enabled && ui.rect_contains_pointer(navigation_rect) {
        let z = view.zoom * zoom_delta;
        match ui.input(|i| i.pointer.hover_pos()) {
            Some(p) => view.zoom_at(z, p),
            None => view.set_zoom(z),
        }
    }
    view.viewport_screen = avail;
    let auto_delta = if unobstructed {
        view.auto_scroll.update(ui, avail, false)
    } else {
        view.auto_scroll.cancel();
        Vec2::ZERO
    };

    // Drawing a comment: the pointer in a band along any edge (or just past it) scrolls the view
    // that way, so a point beyond what is shown can be reached without letting go of the tool.
    let edge_delta = if unobstructed && matches!(view.comments.gesture, Some(comments::Gesture::Draw { .. })) {
        let (pointer, dt) = ui.input(|i| (if i.pointer.any_down() { i.pointer.latest_pos() } else { i.pointer.hover_pos() }, i.stable_dt));
        crate::autoscroll::edge_scroll(pointer, avail, dt)
    } else {
        Vec2::ZERO
    };
    if edge_delta != Vec2::ZERO {
        ui.ctx().request_repaint();
    }

    let content_w = view.content_width(info, avail.width());
    let rects = view.layout(info, content_w, ui.ctx().pixels_per_point());
    // When not scrolling continuously, only one row is shown at a time: the current page, or the
    // current two-page spread (with the cover rule). These indices drive paging, which pages are
    // drawn, and the content height (#752). Empty for a document with no pages.
    let shown_row = |view: &DocView| -> Vec<usize> {
        if rects.is_empty() {
            return Vec::new();
        }
        let last = rects.len() - 1;
        let cur = view.current.min(last);
        match view.layout {
            PageLayout::Single => vec![cur],
            PageLayout::TwoUp if view.cover && cur == 0 => vec![0],
            PageLayout::TwoUp => {
                let first = if view.cover { cur - (cur + 1) % 2 } else { cur - cur % 2 };
                if first < last { vec![first, first + 1] } else { vec![first] }
            }
        }
    };
    let row = shown_row(view);
    let middle_gesture = view.auto_scroll.blocks_input();
    // Not scrolling: when the whole page or spread fits, the wheel would do nothing, so it turns
    // pages instead; zoomed in far enough to pan, it pans, and at the edge it turns the page on
    // (#1924, `single_page_wheel_edge`). Touch drags are untouched: with
    // nothing to pan, touch users turn pages with the rail buttons, the page box and the arrow
    // keys. Runs before `visible_pages` so the frame that turns the page draws it.
    let mut single_paging = false;
    let mut edge_turned = false;
    if !view.continuous {
        // The row's bounding box on screen (one page, or a two-page spread).
        let row_rect = row.iter().filter_map(|&i| rects.get(i)).copied().reduce(Rect::union);
        // Within a point, so layout rounding can't stop a fitting page from turning.
        let fits = row_rect.is_some_and(|r| r.height() + 2.0 * MARGIN <= avail.height() + 1.0);
        let can_turn = !middle_gesture && unobstructed && ui.rect_contains_pointer(avail);
        single_paging = can_turn && fits;
        // Every wheel event goes to the pager, so it follows each trackpad touch to its end
        // even while the page can't turn.
        ui.input(|i| {
            for e in &i.events {
                if let egui::Event::MouseWheel { unit, delta, phase, modifiers } = e {
                    // Zooming (⌘ or Ctrl) and sideways scrolling (Shift) never turn pages.
                    let dy = if modifiers.command || modifiers.ctrl || modifiers.shift { 0.0 } else { delta.y };
                    if fits {
                        view.single_page_wheel(*unit, dy, *phase, i.time, can_turn);
                    } else {
                        edge_turned |= view.single_page_wheel_edge(*unit, dy, *phase, i.time, can_turn);
                    }
                }
            }
        });
        if single_paging {
            // Paging owns vertical wheel motion here. A diagonal gesture keeps its sideways
            // part, as `ScrollArea` itself handles each axis.
            ui.input_mut(|i| i.smooth_scroll_delta.y = 0.0);
        }
    }
    // Point input bypasses egui smoothing for 1:1 motion. Line input keeps egui's configured,
    // smoothed base and adds only the accelerated surplus. While point suppression is active,
    // reconstruct line and page input here so current coarse events survive without exposing an
    // older eased tail.
    let now = ui.input(|i| i.time);
    let input = if ui.rect_contains_pointer(avail) {
        // Match the line distance used by egui's other scroll areas.
        let live = ui.ctx().options(|o| o.input_options.line_scroll_speed);
        let line_points = if live.is_finite() && live > 0.0 { live } else { crate::scroll_speed::EGUI_LINE_POINTS };
        ui.input(|i| crate::wheel_boost::accumulate(&i.events, line_points, single_paging))
    } else {
        crate::wheel_boost::FrameInput::default()
    };
    let mut precise_scroll = input.precise;
    let mut line_base = input.line;
    let mut boost_extra = crate::wheel_boost::boost_extra(input, &mut view.wheel_boost_x, &mut view.wheel_boost_y, now);
    let has_page = input.page != Vec2::ZERO;
    if now.is_finite() && precise_scroll != Vec2::ZERO && !has_page {
        view.precise_scroll_until = now + SUPPRESS_SECS;
    }
    if edge_turned {
        // The turn leaves egui's eased copy of the notch to decay for a few frames; it must not
        // scroll the page just shown away from its top (or bottom).
        view.precise_scroll_until = now + 0.25;
    }
    // Paging owns the vertical axis in single-page view; leave it to the pager.
    // The rest of a gesture that turned the page is swallowed too, or it would pan the new page
    // away from the edge it was shown at.
    let paging_owns_vertical = single_paging || (!view.continuous && view.wheel.gesture_turned());
    if paging_owns_vertical {
        precise_scroll.y = 0.0;
        line_base.y = 0.0;
        boost_extra.y = 0.0;
    }
    if now < view.precise_scroll_until {
        if ui.input(|i| i.is_scrolling()) {
            // Keep hiding egui's unprocessed copy until it has drained. The short rolling grace
            // avoids a delayed duplicate without prolonging suppression after the tail is gone.
            view.precise_scroll_until = view.precise_scroll_until.max(now + SUPPRESS_TAIL_SECS);
        }
        ui.input_mut(|i| i.smooth_scroll_delta = Vec2::ZERO);
        // The point path suppresses every eased wheel delta. Restore the line base through the
        // same un-eased path as its surplus; paging-owned axes were cleared above.
        boost_extra = crate::wheel_boost::cap_frame(boost_extra + line_base);
        if has_page {
            // Page units normally stay on egui's viewport-sized path. During suppression only,
            // reproduce the current event directly so an older hidden tail stays hidden.
            let page_scale = ui.input(|i| i.viewport_rect().height());
            let mut page_delta =
                if page_scale.is_finite() && page_scale > 0.0 { crate::wheel_boost::cap_frame(input.page * page_scale) } else { Vec2::ZERO };
            if paging_owns_vertical {
                page_delta.y = 0.0;
            }
            boost_extra = crate::wheel_boost::cap_frame(boost_extra + page_delta);
        }
    } else {
        precise_scroll = Vec2::ZERO;
        if line_base != Vec2::ZERO || has_page {
            // Selection handling below uses zero to distinguish an inactive window.
            view.precise_scroll_until = 0.0;
        }
    }
    if has_page && paging_owns_vertical {
        // Page input belongs to egui on the free axis; the pager still owns vertical motion.
        ui.input_mut(|i| i.smooth_scroll_delta.y = 0.0);
    }
    // Touchpad momentum (#531): a swipe over the page lifted at speed glides on and slows to a
    // stop, through the same un-eased path. Never while the pager owns the wheel, a gesture or
    // dialog has the input, or animations are off; any click, key, zoom or other scrolling ends it.
    // Asked of the preference, not of egui's animation time, which test harnesses zero.
    let animates = app.animation_effects.animates(app.motion.reduced);
    let may_glide = !paging_owns_vertical && !middle_gesture && navigation_enabled && unobstructed && animates;
    let over_page = ui.rect_contains_pointer(avail);
    ui.input(|i| view.fling.read(&i.events, now, may_glide && over_page));
    let interrupted = ui.input(|i| i.pointer.any_pressed() || i.events.iter().any(|e| matches!(e, egui::Event::Key { pressed: true, .. })));
    if !may_glide || interrupted || key_scroll != 0.0 || auto_delta != Vec2::ZERO || zoom_delta != 1.0 {
        view.fling.stop();
    }
    let glide = view.fling.step(now);
    if view.fling.gliding() {
        ui.ctx().request_repaint();
    }
    // The wheel may have turned the page above: draw the row it turned to. Measured from the old
    // row, the jump to the new page scrolled both out of view, a blank frame on every turn forward.
    let row = shown_row(view);
    let visible_pages: Vec<usize> = if view.continuous { (0..rects.len()).collect() } else { row.clone() };
    let (y_shift, content_h) = if view.continuous {
        (0.0, rects.iter().map(|r| r.bottom()).reduce(f32::max).map(|bottom| bottom + MARGIN).unwrap_or(0.0))
    } else {
        // Only the current row scrolls into view; the content is its bounding box plus margins.
        match row.iter().filter_map(|&i| rects.get(i)).copied().reduce(Rect::union) {
            Some(r) => (r.top() - MARGIN, r.height() + 2.0 * MARGIN),
            None => (0.0, 0.0),
        }
    };

    let mut scroll = egui::ScrollArea::both().auto_shrink([false, false]).scroll_source(egui::scroll_area::ScrollSource {
        drag: if middle_gesture {
            egui::scroll_area::DragScroll::Never
        } else if app.quick_tool == QuickTool::Hand {
            egui::scroll_area::DragScroll::Always
        } else {
            egui::scroll_area::DragScroll::OnTouch
        },
        ..Default::default()
    });
    // Each document keeps its own scroll position when switching tabs (#189).
    scroll = scroll.id_salt(("page-view", view.id));
    // Every scroll here asks for no animation. An animated area would still queue a zero-length
    // offset target for the next frame, which overwrites the jump a page turn just set.
    scroll = scroll.animated(false);
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
    if app.quick_tool == QuickTool::Hand
        && !middle_gesture
        && let Some(id) = view.page_widget.filter(|id| ui.ctx().is_being_dragged(*id))
    {
        let moved = if ui.ctx().drag_started_id() == Some(id) {
            ui.input(|i| i.pointer.total_drag_delta()).unwrap_or(Vec2::ZERO)
        } else {
            ui.input(|i| i.pointer.delta())
        };
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
        let (left, right) = (r.left() - SIDE, r.right() + SIDE);
        let target = if right - left <= w { if left >= x && right <= x + w { x } else { r.center().x - w / 2.0 } } else { x.clamp(left, right - w) };
        if (target - x).abs() > 0.5 {
            scroll = scroll.horizontal_scroll_offset(target.clamp(0.0, (content_w - w).max(0.0)));
        }
    }
    let ppp = ui.ctx().pixels_per_point();
    let scale = view.render_scale(ppp);
    let tag = scale_tag(scale);
    // Hold sharp re-renders while the zoom is still moving: a new scale every frame would
    // cancel the in-flight full-page interpret and restart it, so on a heavy page (a CAD
    // plot with ~1000 images renders for seconds) the sharp raster never arrives and the
    // view looks stuck on a soft image. The stale raster stays stretched meanwhile; the
    // sharp request goes out once the zoom settles.
    let now_scale = ui.input(|i| i.time);
    if (scale - view.last_scale).abs() > f32::EPSILON {
        view.last_scale = scale;
        view.sharp_hold_until = now_scale + 0.35;
        ui.ctx().request_repaint_after(std::time::Duration::from_millis(380));
    }
    let sharp_held = now_scale < view.sharp_hold_until;
    view.current_scale_tag = tag;
    let hand = app.quick_tool == QuickTool::Hand;
    let tool = app.quick_tool;
    // Text selection runs for the Select tool and for the markup tools (highlight…).
    let selects_text = match tool {
        QuickTool::Comment(t) => t.markup().is_some() || t == comments::CommentTool::ReplaceText,
        QuickTool::Select => !preparing,
        QuickTool::Redact | QuickTool::ColumnSelect => true,
        QuickTool::Hand
        | QuickTool::Measure(_)
        | QuickTool::Crop
        | QuickTool::EnhanceCamera
        | QuickTool::Fill(_)
        | QuickTool::Field(_)
        | QuickTool::AddText
        | QuickTool::EditText
        | QuickTool::Stamp(_)
        | QuickTool::CustomStamp(_)
        | QuickTool::Link
        | QuickTool::SignArea { .. }
        | QuickTool::MarqueeZoom
        | QuickTool::Snapshot => false,
    };
    let prefs = &app.comment_prefs;
    let redact_mark = (app.redact_prefs.hover_fill, app.redact_prefs.opacity);
    let units = app.units;
    let allowed = doc.allows_annotation();
    let can_copy = doc.allows_copying();
    let mut copy_selected = false;
    let comments_hidden = doc.comments_hidden();
    let form = doc.form.clone();
    let generation = doc.edit_generation();
    let can_fill = doc.allows_form_filling();
    let can_crop = doc.allows_assembly();
    let mut open_boxes = false;
    let signature = app.signature.clone();
    let initials = app.initials.clone();
    let mut open_initials = false;
    let author = app.comment_prefs.author.clone();
    let today = app.session.today();
    // In Preferences ▸ Date format, or why it can't be stamped into the PDF.
    let date_text = crate::date_text_for_pdf(&app.session);
    let mut refused: Option<String> = None;
    let by_line = app.session.stamp_by_line(&author);
    let mut stamp_placed = false;
    let mut image_action: Option<crate::edit_text_ui::ImageAction> = None;
    let custom_stamp = match app.quick_tool {
        QuickTool::CustomStamp(i) => app.custom_stamps.get(i).cloned(),
        _ => None,
    };
    let mut open_signature = false;
    let mut hover_text: Option<(Pos2, String, bool)> = None;
    let mut badge_hover: Option<(Pos2, String)> = None;
    // A dialog, save prompt or palette covers the page. Hover tips are drawn above those
    // windows, so a comment under the pointer would otherwise float over the dialog.
    let page_covered = app.dialog.is_some()
        || app.close_request.is_some()
        || app.palette_open
        || app.password_prompt.is_some()
        || app.pending_link.is_some()
        || app.pending_remote_link.is_some()
        || app.updates.open;
    let mut clicked_link: Option<LinkTarget> = None;
    let mut javascript_link = None;
    let mut clicked_annotation = None;
    let mut remote_open: Option<(pdfcraft_engine::DocId, String, pdfcraft_render::RemoteDestination)> = None;
    let mut canvas_action: Option<comments::CanvasAction> = None;
    let mut field_menu: Option<FieldMenu> = None;
    let mut open_props: Option<(usize, usize)> = None;
    let mut field_props = false;
    let mut field_placed = false;
    let mut open_table: Option<(usize, [f64; 4])> = None;
    // The table being set up in its window, shown on its own document's page.
    let table_preview = app.field_table_draft.as_mut().filter(|d| d.doc == view.id).and_then(|d| Some(d.preview(info.pages.get(d.page)?, &form)));
    let can_modify = doc.allows_modification();
    if preparing {
        crate::prepare::after_refresh(view, &form);
    } else {
        view.prepare.selected = None;
    }
    view.objects.prepare(doc, tool == QuickTool::EditText && can_modify, ui.ctx());
    view.text_background.prepare(ui.ctx(), doc, view.line_editor.as_ref(), view.content.draft.as_ref(), scale);
    let added = if editing_content { doc.added().to_vec() } else { Vec::new() };
    let doc_links = if tool == QuickTool::Link { doc.links().to_vec() } else { Vec::new() };
    if editing_content {
        crate::content_ui::after_refresh(view, &added);
    } else {
        view.content.selected = None;
    }
    view.move_background.prepare(ui.ctx(), doc, &view.content, scale);

    // A destination's position (bookmarks, links) refines `goto`: its point at the window's
    // top-left, or the centre of a /FitR rectangle at the window's centre.
    if let Some((page, fx, fy, rel)) = view.goto_point.take() {
        let r = rects[page.min(rects.len() - 1)];
        // Within the content, so an unreachable point never shows a frame scrolled past the end.
        let (max_x, max_y) = ((content_w - avail.width()).max(0.0), (content_h - avail.height()).max(0.0));
        if let Some(fx) = fx {
            scroll = scroll.horizontal_scroll_offset((r.left() + fx * r.width() - rel.x).clamp(0.0, max_x));
        }
        if let Some(fy) = fy {
            scroll = scroll.vertical_scroll_offset((r.top() - y_shift + fy * r.height() - rel.y).clamp(0.0, max_y));
        }
    }
    let selection_wheel = view.selection_drag && navigation_enabled && unobstructed && !single_paging && ui.rect_contains_pointer(avail);
    let out = page_scroll(ui, scroll, egui::Id::new(("page-view-scope", view.id)), |ui, viewport| {
        // egui's drag responses accept every pointer button. Keep a wheel gesture from also
        // selecting text, drawing a mark, or panning with the Hand tool.
        if middle_gesture {
            let opacity = ui.opacity();
            ui.disable();
            // Only interactions pause; the document must keep its original colours.
            ui.set_opacity(opacity);
        }
        // Middle-button auto-scroll, the keyboard, precise touchpad input (#759) and its glide
        // (#531), accelerated wheel surplus (#1072), and a drawing's edge scroll; positive y
        // moves the content down. All scroll without easing, unlike egui's wheel default.
        // ScrollArea ignores wheel input during a widget drag. Defer the wheel contribution
        // until after the text widgets when a selection owns that drag.
        let wheel_delta = if selection_wheel { Vec2::ZERO } else { precise_scroll + boost_extra };
        let delta = auto_delta + edge_delta - vec2(0.0, key_scroll) + wheel_delta + glide;
        if delta != Vec2::ZERO && delta.is_finite() {
            ui.scroll_with_delta_animation(delta, egui::style::ScrollAnimation::none());
        }
        let (resp_rect, resp) = ui.allocate_exact_size(vec2(content_w, content_h), Sense::click_and_drag());
        view.page_widget = Some(resp.id);
        // A drag lasts only while egui reports it, so the mark can't outlive one.
        if !resp.dragged_by(egui::PointerButton::Primary) {
            view.selection_drag = false;
        }
        // The Hand tool pans by this widget's drag, before the pages are laid out (above).
        if hand {
            if resp.dragged() {
                ui.ctx().set_cursor_icon(egui::CursorIcon::Grabbing);
            } else if resp.hovered() {
                ui.ctx().set_cursor_icon(egui::CursorIcon::Grab);
            }
        }
        let origin = resp_rect.min - vec2(0.0, y_shift);
        let painter = ui.painter();
        let visible = viewport.translate(resp_rect.min.to_vec2());
        let mut wanted = Vec::new();
        let mut visible_now = Vec::new();
        view.screen_rects.clear();
        view.screen_xforms.clear();
        // Visible heights (points) closer than this are equal: one page size can meet the
        // viewport a fraction of a point differently once the scroll offset is applied.
        const TIE: f32 = 0.5;
        let mut current = view.current;
        let mut best_overlap = -1.0f32;
        let mut current_overlap = -1.0f32;
        let mut current_height = f32::INFINITY;
        let pointer = ui.input(|i| i.pointer.hover_pos());
        // A press on a page makes it the current one (as in Acrobat), so a page command with
        // no selection (Rotate, Delete) acts on the page the reader pointed at, not on a page
        // gone to earlier that still shows whole beside it.
        let pressed = unobstructed && ui.input(|i| i.pointer.primary_pressed()) && ui.rect_contains_pointer(avail);
        let mut pressed_page = None;
        if resp.secondary_clicked() {
            // Set again below when the right-click lands on a link.
            view.links.context = None;
        }
        let mut calc_hover = None;
        for &i in &visible_pages {
            badge_hover = None;
            let r = snap_to_pixels(rects[i].translate(origin.to_vec2()), ppp);
            if !r.intersects(visible.expand(400.0)) {
                continue;
            }
            if pressed && pressed_page.is_none() && pointer.is_some_and(|p| r.contains(p)) {
                pressed_page = Some(i);
            }
            if r.intersects(visible) {
                visible_now.push(i);
                view.screen_rects.push((i, r));
                view.screen_xforms
                    .push((i, PageXform { rect: r, rot: view.rotation, pw: info.pages[i].width.max(1.0), ph: info.pages[i].height.max(1.0) }));
            }
            let overlap = r.intersect(visible).height();
            if i == view.current {
                current_overlap = overlap;
                current_height = r.height();
            }
            // Pages shown equally (two rows wholly on screen) differ only by rounding: the
            // topmost one counts.
            if overlap > best_overlap + TIE {
                best_overlap = overlap;
                current = i;
            }
            let xf = PageXform { rect: r, rot: view.rotation, pw: info.pages[i].width.max(1.0), ph: info.pages[i].height.max(1.0) };
            // Shadows would darken the preceding page when pages touch.
            if view.show_page_gaps {
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
                let msg =
                    ui.fonts_mut(|f| f.layout(message, theme::regular(12.5), Color32::from_rgb(0x6A, 0x2A, 0x2A), (r.width() - 40.0).max(80.0)));
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
                    Some(p) => wanted.push((i, tag_scale(p.tag), p.tag, None)),
                    None => wanted.push((i, want_scale, want_tag, None)),
                }
                match view.pages.get(&i) {
                    Some(p) => {
                        if !tiled && p.tag == want_tag {
                            xf.texel_aligned(p.tex.size(), ppp).paint_image(painter, p.tex.id(), 0.0, 0.0, 1.0, 1.0);
                        } else {
                            // A backdrop, or a raster at an older scale until the new one arrives.
                            xf.paint_image(painter, p.tex.id(), 0.0, 0.0, 1.0, 1.0);
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
                    let mut older: Vec<(f32, u32, u32, &TextureHandle)> = view
                        .tiles
                        .iter()
                        .filter(|((p, t, _, _), _)| *p == i && *t != tag)
                        .map(|((_, t, ox, oy), tex)| (tag_scale(*t), *ox, *oy, tex))
                        .collect();
                    older.sort_by(|a, b| (b.0 / scale).ln().abs().total_cmp(&(a.0 / scale).ln().abs()));
                    for (s, ox, oy, tex) in older {
                        let (ow, oh) = (device_pixels(pw_pt, s) as f32, device_pixels(ph_pt, s) as f32);
                        let [w, h] = tex.size();
                        let (x, y) = (ox as f32, oy as f32);
                        xf.paint_image(painter, tex.id(), x / ow, y / oh, (x + w as f32) / ow, (y + h as f32) / oh);
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
                        wanted.push((i, scale, tag, Some(Tile { x, y, w, h })));
                        if let Some(tex) = view.tiles.get(&(i, tag, x, y)) {
                            let (fw, fh) = (dw as f32, dh as f32);
                            xf.texel_aligned([dw as usize, dh as usize], ppp).paint_image(
                                painter,
                                tex.id(),
                                x as f32 / fw,
                                y as f32 / fh,
                                (x + w) as f32 / fw,
                                (y + h) as f32 / fh,
                            );
                        }
                    }
                }
            }
            view.text_background.paint(painter, &xf, i, view.line_editor.as_ref(), view.content.draft.as_ref());
            if editing_content {
                let page_tex = view.pages.get(&i).map(|p| p.tex.id());
                view.move_background.paint(ui, painter, &xf, i, info, &view.content, page_tex);
            }
            if view.show_page_gaps {
                painter.rect_stroke(r, CornerRadius::ZERO, Stroke::new(0.5, t.border.gamma_multiply(0.8)), egui::StrokeKind::Outside);
            }

            // Comments: tools, selection, moving and resizing come before text selection.
            let pcx = comments::PageCx { page: i, xf: &xf, info, tool, prefs, allowed, hidden: comments_hidden, redact_mark };
            if let QuickTool::Measure(measure_tool) = tool {
                crate::measure_ui::page_input(ui, &resp, doc, view, &pcx, measure_tool, units);
            }

            // Form fields take clicks first with the Select tool (as Acrobat fills fields in
            // every viewing mode); then comments; then text selection.
            if let QuickTool::Stamp(kind) = tool
                && allowed
                && let Some(p) = ui.input(|inp| inp.pointer.hover_pos()).filter(|p| xf.rect.contains(*p))
            {
                ui.ctx().set_cursor_icon(egui::CursorIcon::Crosshair);
                if resp.clicked() {
                    // Centred on the click, upright as the page is shown.
                    let (vx, vy) = xf.screen_to_view(p);
                    let (w, h) = kind.size();
                    let corners = [(vx as f64 - w / 2.0, vy as f64 - h / 2.0), (vx as f64 + w / 2.0, vy as f64 + h / 2.0)];
                    let u: Vec<[f32; 2]> = corners.iter().map(|(x, y)| info.pages[i].view_to_user(*x as f32, *y as f32)).collect();
                    let rect = [u[0][0].min(u[1][0]) as f64, u[0][1].min(u[1][1]) as f64, u[0][0].max(u[1][0]) as f64, u[0][1].max(u[1][1]) as f64];
                    let by = (kind.group() == pdfcraft_engine::StampGroup::Dynamic).then(|| by_line.clone());
                    let shape = pdfcraft_engine::Shape::Stamp { rect, stamp: kind, by };
                    view.pending_edit = Some(pdfcraft_engine::Edit::AddAnnotation(pdfcraft_engine::NewAnnotation {
                        page: i,
                        style: pdfcraft_engine::Style::default_for(&shape),
                        shape,
                        contents: String::new(),
                        author: author.clone(),
                    }));
                    stamp_placed = true;
                }
            }
            if let Some(cs) = custom_stamp.as_ref()
                && allowed
                && let Some(p) = ui.input(|inp| inp.pointer.hover_pos()).filter(|p| xf.rect.contains(*p))
            {
                ui.ctx().set_cursor_icon(egui::CursorIcon::Crosshair);
                if resp.clicked() {
                    // Centred on the click at its natural size (the engine sizes it).
                    let (vx, vy) = xf.screen_to_view(p);
                    let u = info.pages[i].view_to_user(vx, vy);
                    let (x, y) = (u[0] as f64, u[1] as f64);
                    view.pending_edit = Some(pdfcraft_engine::Edit::AddCustomStamp {
                        page: i,
                        rect: [x, y, x, y],
                        name: cs.name.clone(),
                        file: pdfcraft_engine::MarkFile { name: cs.file.clone(), bytes: cs.data.clone(), page: cs.page },
                        author: author.clone(),
                    });
                    stamp_placed = true;
                }
            }
            // Over a mark already placed, the Fill & Sign tool picks it up (comments, below).
            if let QuickTool::Fill(ft) = tool
                && allowed
                && !comments::fill_grabs(ui, &pcx, view)
            {
                match crate::fill_sign::page_input(
                    ui,
                    &resp,
                    &xf,
                    i,
                    info,
                    ft,
                    app.fill_text_size,
                    view,
                    signature.as_ref(),
                    initials.as_ref(),
                    &mut app.signature_preview,
                    &author,
                    &date_text,
                ) {
                    Some(crate::fill_sign::FillAction::Edit(e)) => view.pending_edit = Some(*e),
                    Some(crate::fill_sign::FillAction::Signature(e)) => {
                        view.pending_edit = Some(*e);
                        view.fill_signature_page = Some(i);
                    }
                    Some(crate::fill_sign::FillAction::CreateSignature) => open_signature = true,
                    Some(crate::fill_sign::FillAction::CreateInitials) => open_initials = true,
                    Some(crate::fill_sign::FillAction::Refused(why)) => refused = Some(why),
                    None => {}
                }
            }
            if matches!(tool, QuickTool::SignArea { .. }) {
                crate::sign_ui::page_input(ui, &resp, &xf, i, info, view);
            }
            if matches!(tool, QuickTool::MarqueeZoom | QuickTool::Snapshot) {
                crate::zoom_snap::page_input(ui, &resp, &xf, i, view);
            }
            if tool == QuickTool::EnhanceCamera {
                crate::camera_ui::page_input(ui, &resp, &xf, i, view, can_modify);
            }
            if tool == QuickTool::Crop && crate::crop::page_input(ui, &resp, &xf, i, info, view, can_crop) {
                view.current = i;
                open_boxes = true;
            }
            // `over_field`: a form field is under the pointer and has set its cursor.
            let (on_field, over_field) = if preparing {
                let field_tool = match tool {
                    QuickTool::Field(f) => Some(f),
                    _ => None,
                };
                // While a table is being set up, nothing else is drawn or moved.
                let allowed = can_modify && table_preview.is_none() && open_table.is_none();
                let o = crate::prepare::page_input(ui, &resp, &xf, i, info, &form, field_tool, allowed, view);
                field_props |= o.properties;
                field_placed |= o.placed;
                open_table = open_table.or(o.table);
                (o.consumed || field_tool.is_some(), false)
            } else {
                use crate::forms_ui::FieldPointer;
                let field = if tool == QuickTool::Select
                    && !(editing_content && can_modify && crate::content_ui::select_grabs(ui, &xf, i, info, &added, view))
                    && !comments::select_grabs(ui, &pcx, view)
                {
                    crate::forms_ui::page_input(ui, &resp, &xf, i, info, &form, can_fill, view)
                } else {
                    FieldPointer::Away
                };
                (field == FieldPointer::Owned, field == FieldPointer::Over)
            };
            // Redact draws boxes off text; so does Highlight (an area highlight, as in Acrobat).
            let area_tool = tool == QuickTool::Redact && can_modify || tool == QuickTool::Comment(comments::CommentTool::Highlight) && allowed;
            let boxing = area_tool && {
                let text = view.page_text(i);
                let over_text = |p: Pos2| {
                    let (vx, vy) = xf.screen_to_view(p);
                    text.as_ref().is_some_and(|t| t.glyphs.iter().any(|g| vx >= g.rect[0] && vx <= g.rect[2] && vy >= g.rect[1] && vy <= g.rect[3]))
                };
                crate::redact_ui::page_input(ui, &resp, &xf, i, info, over_text, view)
            };
            let image_owns_pointer = tool == QuickTool::EditText && crate::edit_text_ui::image_owns_pointer(ui, &xf, i, info, view);
            let on_objects =
                !image_owns_pointer && tool == QuickTool::EditText && can_modify && crate::object_ui::page_input(ui, &resp, &xf, i, doc, view);
            let lines = if matches!(tool, QuickTool::AddText | QuickTool::EditText) && can_modify {
                let generation = doc.edit_generation();
                match view.edit_lines.get(&i) {
                    Some((g, l)) if *g == generation => l.clone(),
                    _ => {
                        let l = doc.text_blocks(i);
                        view.edit_lines.insert(i, (generation, l.clone()));
                        l
                    }
                }
            } else {
                Vec::new()
            };
            let over_source = tool == QuickTool::AddText
                && pointer.is_some_and(|p| {
                    !crate::content_ui::hits_added(&xf, info, i, &added, p)
                        && lines.iter().any(|line| xf.user_rect(info, i, line.rect.map(|v| v as f32)).expand(2.0).contains(p))
                });
            let source_blocked = over_source && view.content.hold_blocked();
            if over_source && resp.clicked() && !source_blocked {
                if let Some(edit) = view.content.finish_draft(&added) {
                    view.pending_edit = Some(edit);
                }
                view.content.selected = None;
            }
            let on_content = !on_objects && editing_content && can_modify && {
                source_blocked
                    || crate::content_ui::page_input(
                        ui,
                        &resp,
                        &xf,
                        i,
                        info,
                        &added,
                        tool == QuickTool::AddText && !over_source,
                        matches!(tool, QuickTool::AddText | QuickTool::EditText),
                        &text_style,
                        view,
                    )
                    || tool == QuickTool::AddText && !over_source
            };
            // Added content owns its gesture; its source text must not queue a second edit.
            // Edit text & images boxes every paragraph and image whenever the tool is on. Only the
            // input waits while selected objects or added content own the pointer, or while page
            // input is blocked (another tab, a dialog, the window unfocused): the boxes stay drawn
            // instead of vanishing until the pointer comes back.
            let others_own_pointer = on_objects || on_content;
            let on_edit_text = (tool == QuickTool::EditText || over_source) && can_modify && {
                let generation = doc.edit_generation();
                let images = match view.edit_images.get(&i) {
                    Some((g, l)) if *g == generation => l.clone(),
                    _ => {
                        let l = doc.page_images(i);
                        view.edit_images.insert(i, (generation, l.clone()));
                        l
                    }
                };
                // Editable text takes precedence over background/overlapping artwork. Both
                // inventories are painted every frame, even when text owns the pointer.
                let text_used = crate::edit_text_ui::page_input(ui, &resp, &xf, i, doc, &lines, view, !others_own_pointer && !image_owns_pointer);
                let image_used = crate::edit_text_ui::image_input(
                    ui,
                    &resp,
                    &xf,
                    i,
                    info,
                    &images,
                    view,
                    &mut image_action,
                    !others_own_pointer && (image_owns_pointer || !text_used),
                );
                !others_own_pointer && (text_used || image_used)
            };
            let on_link = tool == QuickTool::Link && can_modify && crate::link_ui::page_input(ui, &resp, &xf, i, info, &doc_links, view);
            if tool == QuickTool::Select
                && resp.secondary_clicked()
                && can_modify
                && let Some(p) = pointer.filter(|p| xf.rect.contains(*p))
                && let Some(l) = doc.links().iter().find(|l| l.page == i && crate::link_ui::screen_rect(&xf, info, i, l.rect).expand(2.0).contains(p))
            {
                view.links.context = Some((i, l.index));
            }
            let consumed = on_objects || on_edit_text || on_link || on_content || boxing || on_field || comments::page_input(ui, &resp, &pcx, view);

            let preview_target =
                (matches!(tool, QuickTool::Select | QuickTool::Fill(_)) && !comments_hidden).then_some(view.comments.selected).flatten();
            view.signature_drag.prepare(ui.ctx(), doc, preview_target, scale);
            view.signature_drag.paint(painter, &pcx, &view.comments, view.pending_edit.as_ref());

            // Any press on the page ends Select All of the document: text read or not, any tool,
            // on a link too.
            // A press that lands on something drawn over the page (a menu, a dialog) is not one.
            if view.all_text && pressed_page == Some(i) {
                view.all_text = false;
            }
            // Text layer: find matches, selection, I-beam and drag-to-select.
            let to_screen = |g: [f32; 4]| xf.view_rect(g);
            if let Some(text) = view.texts.get(&i).cloned() {
                if let Some(f) = &view.find {
                    for (k, (mp, range)) in f.matches.iter().enumerate() {
                        if *mp != i {
                            continue;
                        }
                        let current = f.current == Some(k);
                        for lr in text.line_rects(range.clone()) {
                            let fill = if current {
                                Color32::from_rgba_unmultiplied(255, 140, 0, 110)
                            } else {
                                Color32::from_rgba_unmultiplied(255, 214, 0, 90)
                            };
                            painter.rect_filled(to_screen(lr).expand(1.0), CornerRadius::same(2), fill);
                        }
                    }
                }
                if view.all_text {
                    for lr in text.line_rects(0..text.glyphs.len()) {
                        painter.rect_filled(to_screen(lr), CornerRadius::same(1), Color32::from_rgba_unmultiplied(0x3A, 0x7B, 0xF0, 70));
                    }
                }
                if let Some(sel) = view.selection.filter(|s| s.page == i) {
                    for lr in sel.rects(&text) {
                        painter.rect_filled(to_screen(lr), CornerRadius::same(1), Color32::from_rgba_unmultiplied(0x3A, 0x7B, 0xF0, 70));
                    }
                    // The rectangle being drawn, as Acrobat shows it while column-selecting.
                    if let Span::Column { rect } = sel.span
                        && resp.dragged()
                    {
                        let outline = Stroke::new(1.0, Color32::from_rgb(0x3A, 0x7B, 0xF0));
                        painter.rect_stroke(to_screen(rect), CornerRadius::ZERO, outline, egui::StrokeKind::Middle);
                    }
                }
                if selects_text
                    && !consumed
                    && let Some(p) = pointer.filter(|p| r.contains(*p))
                {
                    let (vx, vy) = xf.screen_to_view(p);
                    let over_text = text.glyphs.iter().any(|g| vx >= g.rect[0] && vx <= g.rect[2] && vy >= g.rect[1] && vy <= g.rect[3]);
                    let over_link = info.links.iter().any(|l| l.page == i && xf.user_rect(info, i, l.rect).contains(p));
                    // Column select (#740): the Column select tool, or Alt (Option on macOS) held
                    // while a drag starts, as in Acrobat.
                    let column = tool == QuickTool::ColumnSelect || ui.input(|inp| inp.modifiers.alt);
                    // A link or a form field under the pointer keeps its own cursor: a check box
                    // drawn with a glyph, or set on printed text, shows the hand, not the I-beam.
                    // So do the mode bar and side panels over a page scrolled under them (#2755),
                    // unless a selection is being dragged there.
                    let cursor_free = !over_link && !over_field && (resp.contains_pointer() || resp.dragged());
                    if column && cursor_free {
                        ui.ctx().set_cursor_icon(egui::CursorIcon::Crosshair);
                    } else if over_text && cursor_free {
                        ui.ctx().set_cursor_icon(egui::CursorIcon::Text);
                    }
                    let press_here = ui
                        .input(|inp| {
                            inp.pointer.press_origin().or_else(|| {
                                // A press and release can arrive in one pass; egui has already
                                // cleared press_origin, but the press still starts multi-click selection.
                                inp.events.iter().find_map(|event| match event {
                                    egui::Event::PointerButton { pos, button: egui::PointerButton::Primary, pressed: true, .. }
                                        if pos.is_finite() =>
                                    {
                                        Some(*pos)
                                    }
                                    _ => None,
                                })
                            })
                        })
                        .is_some_and(|o| r.contains(o));
                    let pressed = ui.input(|inp| inp.pointer.primary_pressed());
                    if pressed && press_here && !over_link {
                        let now = ui.input(|inp| inp.time);
                        // The same limits egui uses to count clicks on release.
                        let (max_delay, max_dist) = ui.ctx().options(|o| (o.input_options.max_double_click_delay, o.input_options.max_click_dist));
                        let is_consecutive =
                            now - view.last_press_time <= max_delay && view.last_press_pos.is_some_and(|pos| pos.distance(p) <= max_dist);
                        view.clicks = if is_consecutive { view.clicks.saturating_add(1) } else { 1 };
                        view.last_press_time = now;
                        view.last_press_pos = Some(p);

                        // Multi-click highlights immediately on pointer press instead of release (#1024).
                        let clicks = if view.clicks > 2 && tool != QuickTool::Select { 1 } else { view.clicks };
                        if clicks >= 2 && over_text {
                            let span = text.nearest(vx, vy).and_then(|a| click_unit(&text, clicks, a));
                            if let Some((first, last)) = span {
                                view.selection = Some(Selection::run(i, first, last));
                            }
                        }
                    }
                    if resp.drag_started() && press_here && !over_link {
                        // A drag replaces the selection without passing through none, so a
                        // re-drag over text whose bar Copy closed would keep the bar hidden.
                        view.actions_dismissed = None;
                        view.selection_drag = true;
                        let origin = ui.input(|inp| inp.pointer.press_origin()).unwrap_or(p);
                        let (ox, oy) = xf.screen_to_view(origin);
                        if view.clicks < 2 {
                            view.selection = if column {
                                Some(Selection { page: i, span: Span::Column { rect: [ox, oy, vx, vy] } })
                            } else {
                                text.nearest(ox, oy).map(|a| Selection::run(i, a, a))
                            };
                        }
                    }
                    let clicks = if view.clicks > 2 && tool != QuickTool::Select { 1 } else { view.clicks };
                    if resp.dragged()
                        && let Some(sel) = view.selection.as_mut().filter(|s| s.page == i)
                    {
                        match &mut sel.span {
                            // After a multi-click the drag extends by its unit (#1685), as in
                            // Acrobat and browsers: whole words or lines, always keeping the one
                            // clicked, which holds the anchor whichever way the drag went.
                            Span::Run { anchor, head } if clicks >= 2 => {
                                if let Some(h) = text.nearest(vx, vy)
                                    && let Some((first, last)) = click_unit(&text, clicks, *anchor)
                                {
                                    let (to_first, to_last) = click_unit(&text, clicks, h).unwrap_or((h, h));
                                    (*anchor, *head) = if to_first < first { (last, to_first) } else { (first, to_last.max(last)) };
                                }
                            }
                            Span::Run { head, .. } => {
                                if let Some(h) = text.nearest(vx, vy) {
                                    *head = h;
                                }
                            }
                            Span::Column { rect } => {
                                rect[2] = vx;
                                rect[3] = vy;
                            }
                        }
                    }
                    // A rectangle around no text selects nothing.
                    if resp.drag_stopped()
                        && view.selection.is_some_and(|s| s.page == i && matches!(s.span, Span::Column { .. }) && s.text(&text).is_empty())
                    {
                        view.selection = None;
                    }
                    // Quick clicks widen the selection: two a word, three the line, four the page.
                    // Release-based fallback for platforms or synthetic events that report double_clicked.
                    let release_clicks = if resp.triple_clicked() {
                        view.clicks.max(2).saturating_add(1)
                    } else if resp.double_clicked() {
                        view.clicks.max(2)
                    } else {
                        0
                    };
                    if release_clicks > 0 {
                        view.clicks = release_clicks;
                    }
                    let clicks = if view.clicks > 2 && tool != QuickTool::Select { 1 } else { view.clicks };
                    // Only on the frame egui reports the multi-click: `view.clicks` outlives it, and
                    // acting on every frame re-selected the word as soon as a selection was cleared
                    // (a highlight made by the double-click left its word selected).
                    if release_clicks > 0 && clicks >= 2 && over_text {
                        if view.selection.is_none() {
                            let span = text.nearest(vx, vy).and_then(|a| click_unit(&text, clicks, a));
                            if let Some((first, last)) = span {
                                view.selection = Some(Selection::run(i, first, last));
                            }
                        }
                    } else if resp.clicked() && view.clicks < 2 && !over_link {
                        // ⇧-click extends a selection on this page to the click, keeping its
                        // anchor (#527). Otherwise (no shift, no selection on this page, or a
                        // column selection, which has no anchor glyph; a selection cannot yet
                        // span pages) a click clears the selection.
                        let shift = ui.input(|inp| inp.modifiers.shift);
                        match (view.selection.as_mut().filter(|s| shift && s.page == i).map(|s| &mut s.span), text.nearest(vx, vy)) {
                            (Some(Span::Run { head, .. }), Some(h)) => *head = h,
                            _ => view.selection = None,
                        }
                    }
                }
            }

            if tool == QuickTool::Select && !consumed {
                crate::image_pick::page_input(&resp, &xf, i, info, doc, pointer, view);
            }
            crate::image_pick::drop_stale(doc, view);
            crate::image_pick::paint(painter, &xf, i, info, view);
            comments::page_after_text(&resp, &pcx, view);
            if tool == QuickTool::Redact && can_modify {
                crate::redact_ui::after_text(&resp, i, info, view);
            }
            if area_tool {
                crate::redact_ui::paint(ui, painter, i, view);
            }
            comments::paint_page(ui, painter, &pcx, view);
            if editing_content {
                crate::content_ui::paint_page(ui, painter, &xf, i, info, &added, view);
            }
            if tool == QuickTool::EditText {
                crate::object_ui::paint(painter, &xf, i, info, &view.objects);
            }
            if tool == QuickTool::EnhanceCamera {
                crate::camera_ui::paint(painter, &xf, i, view);
            }
            if tool == QuickTool::Link {
                crate::link_ui::paint(ui, painter, &xf, i, info, &doc_links, view);
            }
            if preparing {
                crate::prepare::paint_page(ui, painter, &xf, i, info, &form, view);
                if let Some(preview) = &table_preview {
                    crate::field_table_ui::paint_preview(painter, &xf, i, preview);
                }
                if calc_order {
                    let selected = view.prepare.selected.as_ref().map(|(name, _)| name.as_str());
                    if let Some(name) = crate::calc_order_ui::paint_page(ui, painter, &xf, i, info, &form, &view.calc_order, generation, selected) {
                        calc_hover = Some(name);
                    }
                }
            } else {
                crate::forms_ui::paint_page(ui, painter, &xf, i, info, &form, view);
            }

            // Form-field highlight (Acrobat's "Highlight existing fields"); required fields get a
            // red border. Radio buttons are round, and so is theirs (as in Acrobat). Only what can
            // be filled is marked: a read-only widget (its field's, or a radio button's own) is
            // left as it is drawn.
            if view.highlight_fields {
                for f in form.iter() {
                    let required = f.has(pdfcraft_engine::field_flags::REQUIRED);
                    let round = f.kind == pdfcraft_engine::FormFieldKind::Radio;
                    for w in f.widgets.iter().filter(|w| w.page == Some(i) && !w.hidden && !w.read_only) {
                        let r = w.rect;
                        let sr = xf.user_rect(info, i, [r[0] as f32, r[1] as f32, r[2] as f32, r[3] as f32]);
                        let tint = Color32::from_rgba_unmultiplied(0x6E, 0x8E, 0xF5, 48);
                        let (width, color) =
                            if required { (2.0, Color32::from_rgb(0xE3, 0x22, 0x22)) } else { (1.0, Color32::from_rgb(0x6E, 0x8E, 0xF5)) };
                        if round {
                            let radius = sr.width().min(sr.height()) / 2.0;
                            painter.circle_filled(sr.center(), radius, tint);
                            painter.circle_stroke(sr.center(), radius - width / 2.0, Stroke::new(width, color));
                        } else {
                            painter.rect_filled(sr, CornerRadius::same(1), tint);
                            painter.rect_stroke(sr, CornerRadius::same(1), Stroke::new(width, color), egui::StrokeKind::Inside);
                        }
                    }
                }
            }
            // Fill & Sign keeps its placement cursor clear of link/comment hover feedback.
            if !page_covered
                && !matches!(tool, QuickTool::Fill(_))
                && let Some(p) = pointer
            {
                for l in info.links.iter().filter(|l| l.page == i) {
                    let sr = xf.user_rect(info, i, l.rect);
                    if sr.contains(p) {
                        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
                        let label = match &l.target {
                            LinkTarget::Page(n, _) => {
                                crate::i18n::fmt(tl!("Go to page {p}"), &[("p", info.pages.get(*n).map(|p| p.label.as_str()).unwrap_or("?"))])
                            }
                            LinkTarget::Uri(u) => u.clone(),
                            LinkTarget::SetLayers { .. } => tl!("Set layer visibility").to_string(),
                            LinkTarget::RemoteGoTo { file, .. } => file.clone(),
                            LinkTarget::JavaScript(_) => "JavaScript".to_string(),
                            LinkTarget::Other(s) => crate::i18n::fmt(tl!("{s} action"), &[("s", s)]),
                        };
                        hover_text = Some((p, label, matches!(&l.target, LinkTarget::JavaScript(_))));
                        if resp.clicked() && tool == QuickTool::Select && !consumed {
                            clicked_link = Some(l.target.clone());
                            clicked_annotation = Some((l.page, l.index));
                        }
                    }
                }
                // Annotation hover shows the comment, as Acrobat's popups do; not while drawing
                // freehand, where it would cover the next stroke (#429).
                let freehand = matches!(tool, QuickTool::Comment(comments::CommentTool::Ink | comments::CommentTool::Eraser));
                let gesturing = view.comments.gesture.is_some() || freehand;
                // Inspection sorts annotations spatially, but the renderer paints in /Annots
                // order. Keep the shared list intact and choose the last eligible raw index.
                if hover_text.is_none()
                    && !gesturing
                    && let Some((_, text)) = info
                        .annotations
                        .iter()
                        .filter(|a| a.page == i)
                        .filter(|a| {
                            !(prefs.hide_empty_highlight_hover && a.subtype == "Highlight" && a.contents.as_deref().is_none_or(str::is_empty))
                        })
                        .filter(|a| pcx.screen_rects(a).any(|r| r.contains(p)))
                        .filter_map(|a| annotation_hover_text(a).map(|text| (a.index, text)))
                        .max_by_key(|(index, _)| *index)
                {
                    hover_text = Some((p, text, false));
                }
            }
            for (mp, mr, mc) in &view.compare_marks {
                if *mp == i {
                    let sr = xf.user_rect(info, i, *mr).expand(1.5);
                    painter.rect_filled(sr, CornerRadius::same(2), mc.gamma_multiply(0.28));
                }
            }
            if let Some((fp, fr, t0)) = view.flash
                && fp == i
            {
                let now = ui.input(|inp| inp.time);
                let t0 = if t0 == 0.0 { now } else { t0 };
                view.flash = Some((fp, fr, t0));
                let age = (now - t0) as f32;
                if age < 1.6 {
                    let a = ((1.6 - age) / 1.6 * 255.0) as u8;
                    let sr = xf.user_rect(info, i, fr).expand(4.0);
                    painter.rect_stroke(
                        sr,
                        CornerRadius::same(3),
                        Stroke::new(2.5, Color32::from_rgba_unmultiplied(0x1B, 0x63, 0xE0, a)),
                        egui::StrokeKind::Outside,
                    );
                    ui.ctx().request_repaint();
                } else {
                    view.flash = None;
                }
            }
            // Validation is viewer chrome over the signer's original artwork, which
            // may still say "Signature Not Verified". These badges never enter document
            // streams, image/text exports, vector printing or Print as Image.
            for badge in signature_badges(&doc.signatures, i) {
                let [x0, y0, x1, y1] = badge.rect;
                let sr = xf.user_rect(info, i, [x0 as f32, y0 as f32, x1 as f32, y1 as f32]);
                let (icon, color) = crate::sign_ui::signature_status_style(badge.status);
                painter.rect_stroke(sr, CornerRadius::same(2), Stroke::new(1.5, color), egui::StrokeKind::Outside);
                // A widget too small for the mark keeps its outline: the verdict still shows.
                if sr.width() < 4.0 || sr.height() < 4.0 {
                    continue;
                }
                // Big enough to read at a glance, small enough to leave the signer's own
                // text visible around it.
                let side = (sr.width().min(sr.height()) * 0.8).clamp(24.0, 192.0);
                let mark = Rect::from_center_size(sr.center(), vec2(side, side));
                icons::paint(ui, mark, icon, side * 0.64, color);
                if let Some(p) = pointer.filter(|p| mark.expand(2.0).contains(*p)) {
                    badge_hover = Some((p, badge.summary.to_string()));
                }
            }
            // Overlapping widgets paint in order, so the topmost badge owns the hover; an
            // earlier link or annotation hover still wins over any badge.
            if hover_text.is_none() {
                // A validation verdict is an ordinary tip, including during JavaScript menus.
                hover_text = badge_hover.take().map(|(pos, text)| (pos, text, false));
            }
        }
        if calc_order {
            view.calc_order.set_hover_page(calc_hover);
            let selected = view.prepare.selected.as_ref().map(|(name, _)| name.as_str());
            crate::calc_order_ui::paint_trace(ui, painter, visible, &view.calc_order, generation, selected);
        }
        // ScrollArea ignores the wheel while any widget is dragged, so a text-selection drag
        // scrolls the page itself (#721). After the page loop, so it keeps scrolling once the
        // pointer leaves the selection's page for the gap or the next page; over the whole
        // document area, like #759. Combine direct and smoothed motion in one request so mixed
        // Point and Line events retain their full contribution.
        if selection_wheel {
            let smooth =
                if view.precise_scroll_until == 0.0 { ui.input_mut(|input| std::mem::take(&mut input.smooth_scroll_delta)) } else { Vec2::ZERO };
            let delta = precise_scroll + boost_extra + smooth;
            if delta != Vec2::ZERO && delta.is_finite() {
                ui.scroll_with_delta_animation(delta, egui::style::ScrollAnimation::none());
            }
        }
        // Held at the top or bottom edge, or dragged past it, a text selection scrolls the view
        // so it can reach text off screen (#1670); the selection follows the pointer on its page.
        if view.selection_drag
            && navigation_enabled
            && unobstructed
            && !single_paging
            && let Some(p) = ui.input(|i| i.pointer.latest_pos())
        {
            let dy = crate::autoscroll::edge_delta(p.y, avail, ui.input(|i| i.stable_dt));
            if dy != 0.0 {
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
        if current_overlap >= best_overlap - TIE || current_overlap >= current_height - TIE {
            current = view.current;
        }
        if let Some(p) = pressed_page {
            current = p;
        }
        if view.continuous {
            view.current = current;
        }
        resp.context_menu(|ui| {
            // Preparing a form: the selected field's menu.
            if preparing && let Some((name, wi)) = view.prepare.selected.clone() {
                // Several fields: Align, Center, Distribute, Set Fields to Same Size.
                if !view.prepare.also.is_empty() {
                    use crate::prepare::Arrange as A;
                    let others = view.prepare.also.clone();
                    let anchor = (name.clone(), wi);
                    let mut pick = |ui: &mut egui::Ui, op: A, label: &str| {
                        if ui.add_enabled(can_modify, egui::Button::new(tl!(label))).clicked() {
                            view.pending_edit = crate::prepare::arrange(&form, &anchor, &others, op);
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
            if let Some((page, index)) = view.links.context {
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
            if view.comments.selected.is_none() {
                let selected = view.selected_text().is_some_and(|t| !t.is_empty());
                let key = crate::commands::shortcut_label(ui.ctx(), crate::commands::COPY);
                if ui.add_enabled(can_copy && selected, egui::Button::new(tl!("Copy")).shortcut_text(key)).clicked() {
                    copy_selected = true;
                    ui.close();
                }
                ui.separator();
            }
            // Over a picture (and no comment): Copy Image, as in Acrobat's page menu.
            let mut copy_image = None;
            if view.comments.selected.is_none()
                && let Some((page, at)) = view.comments.context_at
            {
                // The page's content is parsed once per right-click, not on every frame the menu is open.
                let key = egui::Id::new("copy-image-under-menu");
                let cached = ui.data(|d| d.get_temp::<(usize, [f64; 2], Option<usize>)>(key)).filter(|c| c.0 == page && c.1 == at);
                let hit = match cached {
                    Some(c) => c.2,
                    None => {
                        let hit = doc.page_image_at(page, at);
                        ui.data_mut(|d| d.insert_temp(key, (page, at, hit)));
                        hit
                    }
                };
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
            canvas_action = comments::context_menu(ui, view, info, prefs, allowed, can_modify).or(copy_image);
        });
        (wanted, visible_now)
    });
    view.single_edges = visible_pages.first().copied().filter(|_| !view.continuous).map(|page| {
        // At the top once the page's top edge shows where going to a page puts it (a gap
        // below the window's top). Within a point, as for wheel paging, so layout rounding
        // can't hide an edge.
        let max_y = (out.content_size.y - out.inner_rect.height()).max(0.0);
        (page, out.state.offset.y <= MARGIN - view.page_gap() + 1.0, out.state.offset.y >= max_y - 1.0)
    });

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
    let scroll_offset = out.state.offset;
    let now_settle = ui.input(|i| i.time);
    if (scroll_offset - view.last_scroll).length() > 2.0 {
        view.pan_hold_until = now_settle + 0.25;
        ui.ctx().request_repaint_after(std::time::Duration::from_millis(280));
    }
    if glide != Vec2::ZERO {
        view.fling.moved((scroll_offset - view.last_scroll).length() > 0.01);
    }
    view.last_scroll = scroll_offset;
    let pan_held = now_settle < view.pan_hold_until;
    let (mut wanted, visible_now) = out.inner;
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
            let side = p.width.max(p.height).max(1.0);
            wanted.push(if side * scale > TILE_THRESHOLD.min(max_side) {
                let bs = BASE_SIDE.min(max_side) / side;
                (i, bs, scale_tag(bs), None)
            } else {
                (i, scale, tag, None)
            });
        }
    }
    let cur = view.current;
    wanted.sort_by_key(|w| (!visible_now.contains(&w.0), w.0.abs_diff(cur), w.3.is_some()));
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

    // The action starts below, after painting. Suppress its tip in the activation frame too,
    // then throughout the interaction: Tooltip is above the menu's Foreground layer.
    let javascript_activating = matches!(&clicked_link, Some(LinkTarget::JavaScript(_)));
    if let Some((pos, text, javascript)) = hover_text
        && !(javascript && (javascript_interaction || javascript_activating))
    {
        egui::Area::new(egui::Id::new("canvas-hover")).order(egui::Order::Tooltip).interactable(false).fixed_pos(pos + vec2(14.0, 16.0)).show(
            ui.ctx(),
            |ui| {
                egui::Frame::popup(ui.style()).show(ui, |ui| {
                    ui.set_max_width(320.0);
                    ui.label(text);
                });
            },
        );
    }
    find_bar(view, info.pages.len(), avail, ui, &t);
    page_size_hint(view, info, units, avail, unobstructed, ui, &t);
    if tool == QuickTool::EnhanceCamera {
        crate::camera_ui::bar(ui.ctx(), view, avail, &t, can_modify);
    }
    if let Some(e) = comments::composer(ui.ctx(), view, info, prefs) {
        view.pending_edit = Some(e);
    }
    if let Some(e) = crate::edit_text_ui::overlay(ui.ctx(), view, info) {
        view.pending_edit = Some(e);
    }
    let text_box = crate::forms_ui::text_box(doc, view, &form);
    if let Some(e) = crate::forms_ui::overlay(ui.ctx(), view, info, &form, text_box.as_ref(), today) {
        view.pending_edit = Some(e);
    }
    if let Some(e) = crate::fill_sign::type_box(ui.ctx(), view, info, &author) {
        view.pending_edit = Some(e);
    }
    let (typed, text_done) = crate::content_ui::editor(ui.ctx(), view, info, &added);
    if typed.is_some() {
        view.pending_edit = typed;
    }
    if let Some(why) = view.objects.notice.take() {
        refused = Some(why);
    }
    let form_notice = view.forms.notice.take();
    // One crop, then back to selecting (as Acrobat does).
    let cropped = view.pending_edit.as_ref().is_some_and(|e| matches!(e, pdfcraft_engine::Edit::SetPageBox { .. }));
    let mut tool = app.quick_tool;
    comments::keys(ui.ctx(), view, &mut tool, allowed);
    if preparing {
        crate::prepare::keys(ui.ctx(), view);
    }
    if editing_content {
        crate::content_ui::keys(ui.ctx(), view);
    }
    if tool == QuickTool::Link {
        crate::link_ui::keys(ui.ctx(), view);
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
        Some(comments::CanvasAction::OpenComments) => {
            app.right = Some(RightPanel::Comments);
            app.comments_panel_closed = false;
        }
        Some(comments::CanvasAction::Properties(p, i)) => open_props = Some((p, i)),
        Some(comments::CanvasAction::CopyImage(p, i)) => image_action = Some(crate::edit_text_ui::ImageAction::Copy(p, i)),
        Some(comments::CanvasAction::CopyImageFile(p, i)) => image_action = Some(crate::edit_text_ui::ImageAction::CopyFile(p, i)),
        None => {}
    }
    // Full screen: the click turns the page unless it did something else first. It waits out the
    // double-click time: turning on the first click's release sent a double-click's second click,
    // meant to select a word, to the next page (review on #2070). A selection cancels it, and so
    // do a link followed meanwhile and leaving full screen; a double-click elsewhere still turns
    // once.
    let now = ui.input(|i| i.time);
    if present_click && clicked_link.is_none() && view.selection.is_none() && view.clicks < 2 {
        let delay = ui.ctx().options(|o| o.input_options.max_double_click_delay);
        view.present_turn = Some((now + delay, !present_back));
    }
    if let Some((due, forward)) = view.present_turn {
        if clicked_link.is_some() || view.selection.is_some() || view.presentation.is_none() {
            view.present_turn = None;
        } else if now >= due {
            view.present_turn = None;
            view.step_page(forward);
        } else {
            ui.ctx().request_repaint_after(std::time::Duration::from_secs_f64((due - now).max(0.0)));
        }
    }
    match clicked_link {
        Some(LinkTarget::Page(p, dest)) => view.go_to_dest(p, dest, info),
        Some(LinkTarget::Uri(u)) => app.request_document_url(&u, crate::LinkOrigin::Link),
        Some(LinkTarget::SetLayers { changes, preserve_rb }) => set_layers(app, index, &changes, preserve_rb),
        Some(LinkTarget::RemoteGoTo { file, destination, .. }) => remote_open = Some((view.id, file, destination)),
        Some(LinkTarget::JavaScript(_)) => {
            javascript_link =
                clicked_annotation.map(|(page, index)| (view.id, page, index, ui.ctx().input(|i| i.pointer.interact_pos()).unwrap_or(avail.center())))
        }
        Some(LinkTarget::Other(s)) => app.notify_fmt("This PDF uses the unsupported {s} action.", &[("s", &s)]),
        None => {}
    }
    if tool == QuickTool::Crop && cropped {
        tool = QuickTool::Select;
    }
    if let Some(done) = app.views[index].marquee_done.take() {
        app.finish_marquee(index, done);
    }
    // A signature rectangle was drawn, or an empty signature field clicked.
    let drawn = app.views[index].sign.drawn.take();
    if let (Some((page, rect)), QuickTool::SignArea { certify }) = (drawn, tool) {
        tool = QuickTool::Select;
        app.quick_tool = tool;
        app.start_signing(page, Some(rect), None, certify.then_some(2));
    }
    if let Some(field) = app.views[index].sign.field.take() {
        let signed = app.session.get(app.views[index].id).is_some_and(|d| d.signatures.iter().any(|s| s.field == field && s.signed));
        if signed {
            app.right = Some(RightPanel::Signatures);
            if !app.sig_expanded.contains(&field) {
                app.sig_expanded.push(field);
            }
        } else {
            let page = app.views[index].current;
            app.start_signing(page, None, Some(field), None);
        }
    }
    app.quick_tool = tool;
    if let Some((page, at)) = app.views[index].comments.attach_at.take() {
        app.attach_file_comment(page, at);
    }
    if let Some((p, i)) = open_props {
        app.open_comment_props(p, i);
    }
    if let Some((p, i)) = app.views[index].comments.default_request.take() {
        app.make_comment_default(p, i);
    }
    if let Some((p, i, open)) = app.views[index].comments.attachment_request.take() {
        let doc = app.views[index].id;
        app.comment_attachment_action(doc, p, i, open);
    }
    if let Some((name, w)) = field_props {
        app.open_field_props(&name, w);
    }
    if let Some(FieldMenu::Duplicate(name)) = field_menu {
        let pages = app.session.get(app.views[index].id).map_or(1, |d| d.info.pages.len());
        app.duplicate_draft = Some(crate::DuplicateDraft { name, all: true, from: 1, to: pages });
        app.dialog = Some(crate::Dialog::DuplicateField);
    }
    if let Some((page, rect)) = app.views[index].links.open_new.take() {
        app.open_link_props(page, Some(rect), None);
    }
    if let Some((page, i)) = app.views[index].links.open_existing.take() {
        app.open_link_props(page, None, Some(i));
    }
    if let Some((page, quads)) = app.views[index].pending_redaction.take() {
        let author = app.comment_prefs.author.clone();
        app.views[index].pending_edit = Some(if app.quick_tool == QuickTool::Comment(comments::CommentTool::Highlight) {
            // An area highlight: a highlight over the box.
            let style = app.comment_prefs.style(comments::CommentTool::Highlight);
            pdfcraft_engine::Edit::AddAnnotation(pdfcraft_engine::NewAnnotation {
                page,
                shape: pdfcraft_engine::Shape::TextMarkup { kind: pdfcraft_engine::Markup::Highlight, quads },
                style,
                contents: String::new(),
                author,
            })
        } else {
            app.redact_prefs.mark(page, quads, &author)
        });
    }
    match image_action {
        Some(crate::edit_text_ui::ImageAction::Replace(page, index)) => app.replace_page_image_dialog(page, index),
        Some(crate::edit_text_ui::ImageAction::Save(page, index)) => app.save_page_image(page, index),
        Some(crate::edit_text_ui::ImageAction::Copy(page, index)) => {
            app.copy_page_image(page, index);
        }
        Some(crate::edit_text_ui::ImageAction::CopyFile(page, index)) => app.copy_page_image_file(page, index),
        Some(crate::edit_text_ui::ImageAction::Cut(page, image)) => {
            app.views[index].image_selection = None;
            app.cut_page_image(page, image);
        }
        Some(crate::edit_text_ui::ImageAction::Redact(page, index)) => app.redact_page_image(page, index),
        Some(crate::edit_text_ui::ImageAction::Link(page, index)) => app.link_page_image(page, index),
        Some(crate::edit_text_ui::ImageAction::Recognize(page)) => app.recognize_text_on_page(page),
        None => {}
    }
    if let Some(why) = refused {
        app.notify_error(why);
    }
    if open_signature || open_initials {
        app.signature_draft = crate::fill_sign::SigDraft::new(open_initials, &app.comment_prefs.author);
        app.dialog = Some(crate::Dialog::Signature);
    }
    if open_boxes {
        app.boxes_draft.range = crate::pageboxes::Range::Current;
        app.boxes_draft.seeded = None;
        app.dialog = Some(crate::Dialog::PageBoxes);
    }
    if let Some((page, rect)) = open_table {
        app.field_table_draft = Some(crate::field_table_ui::FieldTableDraft::new(app.views[index].id, page, rect, &form, generation));
        app.dialog = Some(crate::Dialog::FieldTable);
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
    if let Some((name, action)) = app.views[index].forms.button.take() {
        run_button(app, index, &name, action);
    }
    if copy_selected {
        app.execute("edit.copy");
    }
    if let Some((id, page, annotation, point)) = javascript_link {
        app.run_link_script_at(id, page, annotation, point, Some(ui.ctx().cumulative_frame_nr()));
    }
    // Read mode hides the tools (#2646), and full screen is the page and nothing else (#1459);
    // the quick tools come back with the window.
    if app.mode != crate::Mode::Read && !app.full_screen {
        quick_bar(app, avail, ui);
    }
    if let Some((source, file, destination)) = remote_open {
        app.open_remote_link(source, file, destination);
    }
}

#[derive(Clone, Copy)]
enum SelectionAction {
    Note,
    Markup(comments::CommentTool),
    Replace,
    Copy,
}

/// Actions on an existing text selection, next to the selected words.
fn selection_actions(
    ui: &mut egui::Ui,
    viewport: Rect,
    view: &mut DocView,
    info: &DocInfo,
    prefs: &comments::CommentPrefs,
    can_comment: bool,
    can_copy: bool,
) -> bool {
    if ui.input(|input| input.pointer.is_decidedly_dragging()) {
        return false;
    }
    let Some(selection) = view.selection else {
        view.actions_dismissed = None;
        return false;
    };
    if view.actions_dismissed == Some(selection) {
        return false;
    }
    let Some(text) = view.texts.get(&selection.page) else { return false };
    let Some(xf) = view.page_xform(selection.page) else { return false };
    let Some(last) = selection.rects(text).last().copied() else { return false };
    let selected = selection.text(text);
    if selected.is_empty() {
        return false;
    }
    let rect = xf.view_rect(last);
    if !viewport.intersects(rect) {
        return false;
    }
    let x = rect.left().clamp(viewport.left() + 8.0, (viewport.right() - 520.0).max(viewport.left() + 8.0));
    let y = if rect.bottom() + 48.0 < viewport.bottom() { rect.bottom() + 6.0 } else { (rect.top() - 44.0).max(viewport.top() + 6.0) };
    let mut chosen = None;
    egui::Area::new(egui::Id::new(("text-selection-actions", view.id.0))).order(egui::Order::Foreground).fixed_pos(pos2(x, y)).show(ui.ctx(), |ui| {
        egui::Frame::popup(ui.style()).show(ui, |ui| {
            ui.horizontal(|ui| {
                for (label, action) in [
                    (tl!("Add comment"), SelectionAction::Note),
                    (tl!("Highlight"), SelectionAction::Markup(comments::CommentTool::Highlight)),
                    (tl!("Underline"), SelectionAction::Markup(comments::CommentTool::Underline)),
                    (tl!("Strikethrough"), SelectionAction::Markup(comments::CommentTool::StrikeOut)),
                    (tl!("Replace text"), SelectionAction::Replace),
                ] {
                    if ui.add_enabled(can_comment, egui::Button::new(label)).clicked() {
                        chosen = Some(action);
                    }
                }
                if ui.add_enabled(can_copy, egui::Button::new(tl!("Copy"))).clicked() {
                    chosen = Some(SelectionAction::Copy);
                }
            });
        });
    });
    let Some(action) = chosen else { return false };
    if let SelectionAction::Copy = action {
        // Copying closes the bar but keeps the words selected, for ⌘C or a second action.
        view.actions_dismissed = Some(selection);
        return true;
    }
    let Some((page, quads)) = view.selection_quads(info) else { return false };
    let last = quads.last().copied().unwrap_or_default();
    let at = [last[2].max(last[6]), last[1].max(last[3])];
    view.clear_selection();
    match action {
        SelectionAction::Note | SelectionAction::Replace => {
            if matches!(action, SelectionAction::Replace) {
                view.comments.replace = Some(quads);
            }
            view.comments.composer = Some(comments::Composer {
                page,
                at,
                kind: if matches!(action, SelectionAction::Replace) { comments::ComposerKind::Replace } else { comments::ComposerKind::Note },
                text: String::new(),
                focus: true,
            });
        }
        SelectionAction::Markup(tool) => {
            // Every markup button carries a markup tool; anything else adds nothing.
            let Some(kind) = tool.markup() else { return false };
            view.pending_edit = Some(Edit::AddAnnotation(pdfcraft_engine::NewAnnotation {
                page,
                shape: pdfcraft_engine::Shape::TextMarkup { kind, quads },
                style: prefs.style(tool),
                contents: String::new(),
                author: prefs.author.clone(),
            }));
        }
        // Copy is dispatched through the app after the view borrow ends.
        SelectionAction::Copy => {}
    }
    false
}

/// Run a push button's action (the ones that need no JavaScript engine).
fn run_button(app: &mut PdfCraftApp, index: usize, name: &str, action: pdfcraft_engine::form_scripts::ButtonAction) {
    use pdfcraft_engine::form_scripts::ButtonAction as B;
    let pages = app.session.get(app.views[index].id).map_or(0, |d| d.info.pages.len());
    match action {
        B::Reset { fields, exclude } => {
            let all: Vec<String> = app.session.get(app.views[index].id).map(|d| d.form.iter().map(|f| f.name.clone()).collect()).unwrap_or_default();
            let listed = |n: &String| fields.iter().any(|f| n == f || n.starts_with(&format!("{f}.")));
            let names = match (fields.is_empty(), exclude) {
                (true, _) => None,
                (false, false) => Some(all.iter().filter(|n| listed(n)).cloned().collect()),
                (false, true) => Some(all.iter().filter(|n| !listed(n)).cloned().collect()),
            };
            app.views[index].forms.focus = None;
            app.views[index].pending_edit = Some(pdfcraft_engine::Edit::ResetForm { names });
        }
        B::Named(n) => match n.as_str() {
            "Print" => app.open_print(),
            // What PdfCraft's XFA buttons use, and what a script's `execMenuItem("Save")`
            // does too: the Save As dialog, so a click never overwrites the file unasked.
            "SaveAs" => app.run_command("file.save_as"),
            "NextPage" => app.views[index].step_page(true),
            "PrevPage" => app.views[index].step_page(false),
            "FirstPage" => app.views[index].go_to_page(0),
            "LastPage" => app.views[index].go_to_page(pages.saturating_sub(1)),
            other => app.notify_fmt("{name}: the {other} action isn't supported yet", &[("name", name), ("other", other)]),
        },
        B::Uri(u) => app.request_document_url(&u, crate::LinkOrigin::Button),
        B::GoTo(p) => app.views[index].go_to_page(p.min(pages.saturating_sub(1))),
        B::ShowHide { fields, hide } => {
            let listed = |n: &String| fields.iter().any(|f| n == f || n.starts_with(&format!("{f}.")));
            // `display.hidden` 1, `display.visible` 0, as a script setting `field.display` would.
            let changes: Vec<pdfcraft_engine::FieldChange> = app
                .session
                .get(app.views[index].id)
                .map(|d| d.form.iter().filter(|f| listed(&f.name)).map(|f| f.name.clone()).collect::<Vec<_>>())
                .unwrap_or_default()
                .into_iter()
                .map(|name| pdfcraft_engine::FieldChange {
                    name,
                    value: None,
                    read_only: None,
                    required: None,
                    display: Some(if hide { 1 } else { 0 }),
                })
                .collect();
            if !changes.is_empty() {
                let label = if hide { "Hide a field" } else { "Show a field" };
                app.views[index].pending_edit =
                    Some(pdfcraft_engine::Edit::Batch { label: label.into(), edits: vec![pdfcraft_engine::Edit::ApplyScriptChanges { changes }] });
            }
        }
        B::SetLayers { changes, preserve_rb } => {
            use pdfcraft_engine::form_scripts::LayerOp as Op;
            let changes: Vec<(LayerOp, (u32, u16))> = changes
                .into_iter()
                .map(|(op, ocg)| {
                    let op = match op {
                        Op::On => LayerOp::On,
                        Op::Off => LayerOp::Off,
                        Op::Toggle => LayerOp::Toggle,
                    };
                    (op, ocg)
                })
                .collect();
            set_layers(app, index, &changes, preserve_rb);
        }
        B::Alert(m) => app.notify(m),
        B::Submit(url) => app.notify_fmt(
            "{name} submits the form to {url}; PdfCraft doesn't send form data. Save the document to keep your entries.",
            &[("name", name), ("url", &url)],
        ),
        B::ImportIcon => app.choose_field_image(name),
        B::Script(js) => {
            let id = app.views[index].id;
            app.run_button_script(id, name, &js);
        }
    }
}

/// Run a set-layer-visibility action from a button or link. It changes what is shown, as the
/// Layers panel does (which follows), not the document.
fn set_layers(app: &mut PdfCraftApp, index: usize, changes: &[(LayerOp, (u32, u16))], preserve_rb: bool) {
    let id = app.views[index].id;
    if app.session.set_layer_state(id, changes, preserve_rb) {
        app.views[index].invalidate_content();
    }
}

/// Whether `p` lies in the bottom-left quadrant of `area`, where the page size shows (#1855).
fn in_size_hint_zone(area: Rect, p: Pos2) -> bool {
    p.x >= area.min.x && p.x < area.center().x && p.y > area.center().y && p.y <= area.max.y
}

/// The current page's size, floating at the bottom-left of the document area while the pointer is
/// in that quadrant, as Acrobat does. Same formatter and unit as Document Properties.
fn page_size_hint(view: &DocView, info: &DocInfo, units: crate::units::Unit, area: Rect, unobstructed: bool, ui: &mut egui::Ui, t: &Tokens) {
    let Some(page) = info.pages.get(view.current) else { return };
    let inside = unobstructed && ui.input(|i| i.pointer.hover_pos()).is_some_and(|p| in_size_hint_zone(area, p));
    if !inside {
        return;
    }
    let text = units.size(f64::from(page.width), f64::from(page.height));
    egui::Area::new(egui::Id::new("page-size-hint"))
        .order(egui::Order::Foreground)
        .interactable(false)
        .pivot(egui::Align2::LEFT_BOTTOM)
        .fixed_pos(area.left_bottom() + vec2(14.0, -14.0))
        .show(ui.ctx(), |ui| {
            egui::Frame::NONE
                .fill(t.card)
                .stroke(Stroke::new(1.0, t.border))
                .corner_radius(CornerRadius::same(6))
                .inner_margin(egui::Margin::symmetric(8, 4))
                .show(ui, |ui| {
                    ui.add(egui::Label::new(egui::RichText::new(text).color(t.text)).selectable(false));
                });
        });
}

/// Acrobat-style floating find bar at the top-right of the document area.
fn find_bar(view: &mut DocView, pages: usize, area: Rect, ui: &mut egui::Ui, t: &Tokens) {
    let Some(find) = view.find.as_mut() else { return };
    // The Search panel shows this search.
    if find.in_panel {
        return;
    }
    let mut close = false;
    let mut step: Option<bool> = None;
    let searched = view.texts.len() + view.text_failed.len();
    egui::Area::new(egui::Id::new("find-bar"))
        .order(egui::Order::Middle)
        .pivot(Align2::RIGHT_TOP)
        .fixed_pos(area.right_top() + vec2(-18.0, 12.0))
        .show(ui.ctx(), |ui| {
            egui::Frame::NONE
                .fill(t.card)
                .stroke(Stroke::new(1.0, t.border))
                .corner_radius(CornerRadius::same(10))
                .shadow(egui::Shadow { offset: [0, 4], blur: 16, spread: 0, color: Color32::from_black_alpha(if t.dark() { 90 } else { 30 }) })
                .inner_margin(egui::Margin::symmetric(10, 6))
                .show(ui, |ui| {
                    ui.horizontal(|ui| {
                        ui.add(icons::image("search", 16.0, t.text_muted));
                        let edit = egui::TextEdit::singleline(&mut find.query)
                            .id(egui::Id::new("find-input"))
                            .hint_text(tl!("Find text"))
                            .desired_width(220.0)
                            .frame(egui::Frame::NONE);
                        let r = ui.add(edit);
                        if find.focus {
                            r.request_focus();
                            find.focus = false;
                        }
                        if r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                            step = Some(!ui.input(|i| i.modifiers.shift));
                            r.request_focus();
                        }
                        if r.has_focus() && ui.input(|i| i.key_pressed(egui::Key::Escape)) {
                            close = true;
                        }
                        let status = if find.query.trim().is_empty() {
                            String::new()
                        } else if find.matches.is_empty() {
                            if searched < pages {
                                crate::i18n::fmt(tl!("Searching… {s}/{p}"), &[("s", &searched.to_string()), ("p", &pages.to_string())])
                            } else {
                                tl!("No matches").to_string()
                            }
                        } else {
                            let more = if searched < pages { "+" } else { "" };
                            crate::i18n::fmt(
                                tl!("{c} of {n}{more}"),
                                &[
                                    ("c", &find.current.map(|c| c + 1).unwrap_or(0).to_string()),
                                    ("n", &find.matches.len().to_string()),
                                    ("more", more),
                                ],
                            )
                        };
                        ui.label(egui::RichText::new(status).font(theme::regular(12.0)).color(t.text_muted));
                        let tip = crate::commands::key_tip(ui.ctx(), tl!("Previous ({key})"), crate::commands::FIND_PREV);
                        if icons::button(ui, "chevron-up", 26.0, false, &tip).clicked() {
                            step = Some(false);
                        }
                        let tip = crate::commands::key_tip(ui.ctx(), tl!("Next ({key})"), crate::commands::FIND_NEXT);
                        if icons::button(ui, "chevron-down", 26.0, false, &tip).clicked() {
                            step = Some(true);
                        }
                        let opts = icons::button(ui, "settings-2", 26.0, find.case_sensitive || find.whole_words, tl!("Find options"));
                        egui::Popup::menu(&opts).show(|ui| {
                            let a = ui.add(widgets::checkbox(&mut find.whole_words, tl!("Whole words only"))).changed();
                            let b = ui.add(widgets::checkbox(&mut find.case_sensitive, tl!("Case-sensitive"))).changed();
                            if a || b {
                                // Search again with the new options.
                                find.case_query.clear();
                            }
                        });
                        if icons::button(ui, "x", 26.0, false, tl!("Close (Esc)")).clicked() {
                            close = true;
                        }
                    });
                });
        });
    if close {
        view.find = None;
        return;
    }
    if find.query != find.case_query {
        view.rerun_find();
    }
    if let Some(forward) = step {
        view.find_step(forward);
    }
}

/// The prepare-mode field menu's choices.
enum FieldMenu {
    Properties,
    Duplicate(String),
}

/// What the notice bar's buttons ask for.
enum Notice {
    /// Reload the file that changed on disk, dropping unsaved changes.
    Reload,
    Security,
    Signatures,
    Repairs,
    /// Highlight fields was turned on or off.
    FieldHighlights(bool),
    /// Open the JavaScript console (the form's scripts were turned off; the error is there).
    Console,
    /// Unlock… (enter the permissions password).
    Unlock,
    /// Close this historical snapshot tab (View signed version / View revision).
    Close,
    /// Stop showing the secured-document notice, in every document (#4035).
    HideSecured,
}

/// The notice bar above the pages: a file changed on disk first, then what a historical
/// snapshot is, then the signature status bar, a form's scripts having been turned
/// off, security, forms and warnings.
fn notices(
    view: &mut DocView,
    doc: &pdfcraft_engine::Document,
    secured: bool,
    repaired: bool,
    signed: Option<(&str, Color32, &str, String)>,
    ui: &mut egui::Ui,
    t: &Tokens,
) -> Option<Notice> {
    let info = &doc.info;
    let xfa = doc.xfa.as_ref();
    // The file changed on disk while this document has unsaved changes (#431): ask before
    // replacing it, ahead of every other notice.
    if view.disk_changed {
        let mut reload = false;
        egui::Frame::NONE.fill(t.accent_soft).inner_margin(egui::Margin::symmetric(14, 7)).show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.add(icons::image("rotate-cw", 16.0, t.accent_text));
                ui.label(
                    egui::RichText::new(tl!(
                        "This file changed on disk. Reload shows the new version and drops your unsaved changes; Save replaces it with this version."
                    ))
                    .color(t.text),
                );
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if icons::button(ui, "x", 22.0, false, tl!("Dismiss")).clicked() {
                        view.disk_changed = false;
                    }
                    reload = crate::widgets::pill_button(ui, tl!("Reload"), false).clicked();
                });
            });
        });
        return reload.then_some(Notice::Reload);
    }
    // A historical snapshot (View signed version / View revision): what it is, above the
    // signature and security bars, so the tab is never mistaken for the live document.
    if let Some(snap) = view.snapshot.as_ref() {
        let mut close = false;
        let text = if snap.kind == SnapshotKind::SignedVersion {
            crate::i18n::fmt(
                tl!("You are viewing the signed version of “{from}” (revision {rev}). It is a snapshot: changes here won't affect the original."),
                &[("from", &snap.from), ("rev", &snap.revision.to_string())],
            )
        } else {
            crate::i18n::fmt(
                tl!("You are viewing revision {rev} of “{from}”. It is a snapshot: changes here won't affect the original."),
                &[("rev", &snap.revision.to_string()), ("from", &snap.from)],
            )
        };
        notice_bar(ui, t, "clock", t.accent_text, text, |ui| {
            if crate::widgets::pill_button(ui, tl!("Close"), true).clicked() {
                close = true;
            }
        });
        if close {
            return Some(Notice::Close);
        }
    }
    if let Some((icon, color, template, arg)) = signed {
        let mut open = false;
        notice_bar(ui, t, icon, color, crate::i18n::fmt(tl!(template), &[("by", &arg)]), |ui| {
            if crate::widgets::pill_button(ui, tl!("Signature panel"), false).clicked() {
                open = true;
            }
        });
        return open.then_some(Notice::Signatures);
    }
    if view.notice_dismissed {
        return None;
    }
    let mut open_security = false;
    let mut unlock = false;
    let mut hide_secured = false;
    let mut open_repairs = false;
    let mut open_console = false;
    let mut toggled = None;
    // A form whose scripts were turned off no longer calculates or answers its buttons: that
    // comes before what else there is to say about the document.
    let scripts_off = doc.xfa_scripts_off();
    // What the document is as a form, if it is one: what the bar says first, secured or not.
    let form = if let Some(x) = xfa {
        let lang = crate::i18n::current();
        let pages = crate::i18n::trn(lang, x.pages as u64, "{n} page", "{n} pages");
        let fields = crate::i18n::trn(lang, x.fields as u64, "{n} field", "{n} fields");
        let mut text =
            crate::i18n::fmt(tl!("Dynamic XFA form laid out from its template: {pages}, {fields}."), &[("pages", &pages), ("fields", &fields)]);
        if !x.warnings.is_empty() {
            // The notes are clauses ("a picture is left out"); the first starts a sentence here.
            let notes = x.warnings.join("; ");
            let mut chars = notes.chars();
            text.push(' ');
            text.extend(chars.next().into_iter().flat_map(char::to_uppercase));
            text.push_str(chars.as_str());
            text.push('.');
        }
        Some(("text-cursor-input", text, true))
    } else if info.xfa == Some(pdfcraft_render::Xfa::Dynamic) {
        Some((
            "triangle-alert",
            tl!("This is a dynamic XFA form, which PdfCraft can't display yet. What you see is the file's placeholder page.").to_string(),
            false,
        ))
    } else if info.xfa == Some(pdfcraft_render::Xfa::Static) {
        Some((
            "text-cursor-input",
            tl!("XFA form: its fields and its XFA data are kept in step, so other viewers show what you fill in.").to_string(),
            true,
        ))
    } else if info.fields.iter().any(|f| f.kind != pdfcraft_render::FieldKind::PushButton) {
        // Push buttons alone (LaTeX `animate` frames, navigation buttons) leave nothing to fill in.
        Some((
            "text-cursor-input",
            crate::i18n::fmt(tl!("This document contains {n} interactive form fields."), &[("n", &info.fields.len().to_string())]),
            true,
        ))
    } else {
        None
    };
    let msg = if scripts_off {
        Some(("triangle-alert", crate::js_ui::scripts_off_notice(), true))
    } else if secured {
        // A secured form still says what it is, and keeps Highlight fields: the security
        // sentence follows the form's.
        let security = tl!("This document is secured. Some changes are restricted by its security settings.");
        Some(match form {
            Some((icon, text, fields)) => (icon, format!("{text} {security}"), fields),
            None => ("lock", security.to_string(), false),
        })
    } else if form.is_some() {
        form
    } else if repaired {
        Some(("bandage", tl!("This file was damaged and has been repaired. Saving keeps the repaired version.").to_string(), false))
    } else if !info.warnings.is_empty() {
        Some(("triangle-alert", info.warnings[0].clone(), false))
    } else {
        None
    };
    let (icon, text, fields) = msg?;
    notice_bar(ui, t, icon, t.accent_text, text, |ui| {
        if icons::button(ui, "x", 22.0, false, tl!("Dismiss")).clicked() {
            view.notice_dismissed = true;
        }
        if fields {
            let label = if view.highlight_fields { tl!("Hide field highlights") } else { tl!("Highlight fields") };
            if crate::widgets::pill_button(ui, label, view.highlight_fields).clicked() {
                view.highlight_fields = !view.highlight_fields;
                toggled = Some(Notice::FieldHighlights(view.highlight_fields));
            }
        }
        if scripts_off && crate::widgets::pill_button(ui, tl!("JavaScript Console"), false).clicked() {
            open_console = true;
        }
        if secured && crate::widgets::pill_button(ui, tl!("Security settings"), false).clicked() {
            open_security = true;
        }
        if secured && doc.can_unlock() && crate::widgets::pill_button(ui, tl!("Unlock…"), true).clicked() {
            unlock = true;
        }
        // Only when the bar shows the security message: a form's stopped scripts come first.
        if secured && !scripts_off && crate::widgets::pill_button(ui, tl!("Don't show again"), false).clicked() {
            hide_secured = true;
        }
        if repaired && !secured && info.fields.is_empty() && crate::widgets::pill_button(ui, tl!("Details"), false).clicked() {
            open_repairs = true;
        }
    });
    if open_repairs {
        return Some(Notice::Repairs);
    }
    if open_console {
        return Some(Notice::Console);
    }
    if unlock {
        return Some(Notice::Unlock);
    }
    if hide_secured {
        return Some(Notice::HideSecured);
    }
    open_security.then_some(Notice::Security).or(toggled)
}

/// One notice bar: an icon and a message on the left, `buttons` (added right to left) on the
/// right. The buttons are laid out first, so the message only gets the width they leave: it
/// wraps there (or, when very little is left, is cut short with the full text on hover) instead
/// of running under the buttons and past the document area into the side panel (#785). The
/// buttons are centred on a row of button height, and the message's first line is centred on
/// the same row; a wrapped message grows the bar downwards.
fn notice_bar(ui: &mut egui::Ui, t: &Tokens, icon: &str, icon_color: Color32, text: String, buttons: impl FnOnce(&mut egui::Ui)) {
    const ROW: f32 = 28.0;
    const ICON: f32 = 16.0;
    egui::Frame::NONE.fill(t.accent_soft).inner_margin(egui::Margin::symmetric(14, 7)).show(ui, |ui| {
        let width = ui.available_width();
        ui.allocate_ui_with_layout(vec2(width, ROW), egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.set_min_height(ROW);
            buttons(ui);
            let rest = ui.available_width().max(0.0);
            ui.allocate_ui_with_layout(vec2(rest, ROW), egui::Layout::top_down(egui::Align::Min), |ui| {
                let font = egui::TextStyle::Body.resolve(ui.style());
                let line = ui.fonts_mut(|f| f.row_height(&font)).max(ICON);
                ui.add_space(((ROW - line) / 2.0).max(0.0));
                ui.with_layout(egui::Layout::left_to_right(egui::Align::Min), |ui| {
                    let (slot, _) = ui.allocate_exact_size(vec2(ICON, line), Sense::hover());
                    icons::paint(ui, Rect::from_center_size(slot.center(), vec2(ICON, ICON)), icon, ICON, icon_color);
                    let label = egui::Label::new(egui::RichText::new(text).color(t.text));
                    // Wrapping into a sliver would stack the message a few letters per line.
                    ui.add(if ui.available_width() >= 120.0 { label.wrap() } else { label.truncate() });
                });
            });
        });
    });
}

/// The floating quick-action bar at the left edge of the document area.
fn quick_bar(app: &mut PdfCraftApp, area: Rect, ui: &mut egui::Ui) {
    let t = Tokens::get(ui.ctx());
    let pos = area.left_top() + vec2(14.0, 14.0);
    egui::Area::new(egui::Id::new("quick-bar")).order(egui::Order::Middle).fixed_pos(pos).show(ui.ctx(), |ui| {
        egui::Frame::NONE
            .fill(t.card)
            .stroke(Stroke::new(1.0, t.border))
            .corner_radius(CornerRadius::same(12))
            .shadow(egui::Shadow { offset: [0, 4], blur: 16, spread: 0, color: Color32::from_black_alpha(if t.dark() { 100 } else { 28 }) })
            .inner_margin(egui::Margin::same(6))
            .show(ui, |ui| {
                ui.spacing_mut().item_spacing.y = 2.0;
                ui.vertical(|ui| {
                    if icons::button(ui, "mouse-pointer-2", 32.0, app.quick_tool == QuickTool::Select, tl!("Select (V)")).clicked() {
                        app.quick_tool = QuickTool::Select;
                    }
                    if icons::button(ui, "hand", 32.0, app.quick_tool == QuickTool::Hand, tl!("Hand (H)")).clicked() {
                        app.quick_tool = QuickTool::Hand;
                    }
                    // Comment ▸, Highlight ▸, Draw ▸ (Acrobat's comment toolbar groups). Clicking a
                    // group selects its last-used tool; clicking it again opens the flyout.
                    for g in 0..comments::GROUPS.len() {
                        let current = app.comment_prefs.group_tool[g];
                        let active = matches!(app.quick_tool, QuickTool::Comment(t) if t.group() == g);
                        let resp = icons::button(ui, current.icon(), 32.0, active, tl!(current.label()));
                        // A small corner triangle marks the flyout.
                        let r = resp.rect;
                        let tri = [r.right_bottom() + vec2(-4.0, -4.0), r.right_bottom() + vec2(-9.0, -4.0), r.right_bottom() + vec2(-4.0, -9.0)];
                        ui.painter().add(egui::Shape::convex_polygon(tri.to_vec(), if active { t.accent } else { t.text_faint }, Stroke::NONE));
                        let open = (resp.clicked() && active) || resp.secondary_clicked();
                        if resp.clicked() && !active {
                            app.execute(current.command());
                        }
                        egui::Popup::menu(&resp)
                            .open_memory(open.then_some(egui::SetOpenCommand::Toggle))
                            .align(egui::RectAlign::RIGHT_START)
                            .gap(6.0)
                            .show(|ui| {
                                for tool in comments::GROUPS[g] {
                                    let on = app.quick_tool == QuickTool::Comment(*tool);
                                    let (row, click) = ui.allocate_exact_size(vec2(180.0, 28.0), Sense::click());
                                    let press = theme::Press::track(ui, &click);
                                    press.wash(ui, row, 4, on);
                                    let body = row.translate(press.offset());
                                    icons::paint(ui, Rect::from_min_size(body.min + vec2(8.0, 6.0), vec2(16.0, 16.0)), tool.icon(), 16.0, t.text);
                                    let reserve = if on { 28.0 } else { 8.0 };
                                    crate::widgets::paint_left(
                                        ui,
                                        body.left_center() + vec2(34.0, 0.0),
                                        tl!(tool.label()),
                                        theme::regular(13.0),
                                        t.text,
                                        (row.width() - 34.0 - reserve).max(0.0),
                                    );
                                    if on {
                                        icons::paint(
                                            ui,
                                            Rect::from_min_size(body.right_top() + vec2(-24.0, 7.0), vec2(14.0, 14.0)),
                                            "check",
                                            14.0,
                                            t.accent,
                                        );
                                    }
                                    let click = theme::hand(click);
                                    let info = tl!(tool.label()).to_string();
                                    click.widget_info(|| egui::WidgetInfo::selected(egui::WidgetType::Button, true, on, info.clone()));
                                    if click.clicked() {
                                        app.execute(tool.command());
                                        ui.close();
                                    }
                                }
                            });
                    }
                    // Edit text & images, while the Edit workspace is open (Acrobat's Edit toolbar)
                    // or the tool is in use.
                    if app.mode == crate::Mode::Edit || app.quick_tool == QuickTool::EditText {
                        let on = app.quick_tool == QuickTool::EditText;
                        if icons::button(ui, "text-cursor-input", 32.0, on, tl!("Edit text & images")).clicked() {
                            if on {
                                app.put_down_edit_text();
                            } else {
                                app.quick_tool = QuickTool::EditText;
                            }
                        }
                    }
                    // Fill & Sign ▸ (text, marks, date, signature).
                    let current_fill = match app.quick_tool {
                        QuickTool::Fill(f) => Some(f),
                        _ => None,
                    };
                    let shown = current_fill.unwrap_or(crate::fill_sign::FillTool::Text);
                    let resp = icons::button(
                        ui,
                        if current_fill.is_some() { shown.icon() } else { "pen-line" },
                        32.0,
                        current_fill.is_some(),
                        tl!("Fill & Sign"),
                    );
                    let r = resp.rect;
                    let tri = [r.right_bottom() + vec2(-4.0, -4.0), r.right_bottom() + vec2(-9.0, -4.0), r.right_bottom() + vec2(-4.0, -9.0)];
                    ui.painter().add(egui::Shape::convex_polygon(
                        tri.to_vec(),
                        if current_fill.is_some() { t.accent } else { t.text_faint },
                        Stroke::NONE,
                    ));
                    if resp.clicked() && current_fill.is_none() {
                        app.execute(shown.command());
                    }
                    let open = (resp.clicked() && current_fill.is_some()) || resp.secondary_clicked();
                    egui::Popup::menu(&resp)
                        .open_memory(open.then_some(egui::SetOpenCommand::Toggle))
                        .align(egui::RectAlign::RIGHT_START)
                        .gap(6.0)
                        .show(|ui| {
                            for tool in crate::fill_sign::FILL_TOOLS {
                                if matches!(tool, crate::fill_sign::FillTool::Signature | crate::fill_sign::FillTool::Initials) {
                                    continue;
                                }
                                let on = current_fill == Some(tool);
                                let (row, click) = ui.allocate_exact_size(vec2(180.0, 28.0), Sense::click());
                                let press = theme::Press::track(ui, &click);
                                press.wash(ui, row, 4, on);
                                let body = row.translate(press.offset());
                                icons::paint(ui, Rect::from_min_size(body.min + vec2(8.0, 6.0), vec2(16.0, 16.0)), tool.icon(), 16.0, t.text);
                                let reserve = if on { 28.0 } else { 8.0 };
                                crate::widgets::paint_left(
                                    ui,
                                    body.left_center() + vec2(34.0, 0.0),
                                    tl!(tool.label()),
                                    theme::regular(13.0),
                                    t.text,
                                    (row.width() - 34.0 - reserve).max(0.0),
                                );
                                if on {
                                    icons::paint(
                                        ui,
                                        Rect::from_min_size(body.right_top() + vec2(-24.0, 7.0), vec2(14.0, 14.0)),
                                        "check",
                                        14.0,
                                        t.accent,
                                    );
                                }
                                let click = theme::hand(click);
                                let info = tl!(tool.label()).to_string();
                                click.widget_info(|| egui::WidgetInfo::selected(egui::WidgetType::Button, true, on, info.clone()));
                                if click.clicked() {
                                    app.execute(tool.command());
                                    ui.close();
                                }
                            }
                            ui.separator();
                            if let Some(command) = crate::fill_sign::signature_entries(ui, app, &t) {
                                app.execute(command);
                                ui.close();
                            }
                        });
                    if matches!(current_fill, Some(crate::fill_sign::FillTool::Text | crate::fill_sign::FillTool::Date)) {
                        ui.label(tl!("Font size:"));
                        let size = ui.add(
                            egui::DragValue::new(&mut app.fill_text_size)
                                .range(crate::fill_sign::MIN_TEXT_SIZE..=crate::fill_sign::MAX_TEXT_SIZE)
                                .speed(0.25)
                                .suffix(" pt"),
                        );
                        // #1274: the size also applies to the Fill & Sign text that is selected,
                        // once per change (a typed value, or the end of a drag), so a drag adds
                        // one undo step and not one per frame.
                        if ((size.changed() && !size.dragged()) || size.drag_stopped())
                            && let Some((index, id)) = app.active_ids()
                            && let Some((page, annotation)) = app.views.get(index).and_then(|view| view.comments.selected)
                            && app.session.get(id).is_some_and(|doc| {
                                doc.allows_annotation()
                                    && doc
                                        .info
                                        .annotations
                                        .iter()
                                        .any(|a| a.page == page && a.index == annotation && a.fill_sign && a.subtype == "FreeText" && !a.locked)
                            })
                        {
                            app.apply_edit(Edit::FontSizeAnnotation { page, index: annotation, size: app.fill_text_size });
                        }
                    }
                    if let QuickTool::Comment(tool) = app.quick_tool {
                        let (r, _) = ui.allocate_exact_size(vec2(32.0, 9.0), Sense::hover());
                        ui.painter().hline(r.x_range().shrink(6.0), r.center().y, Stroke::new(1.0, t.divider));
                        if let Some(color) = comments::quick_bar_controls(ui, tool, &mut app.comment_prefs)
                            && let Some((index, id)) = app.active_ids()
                            && let Some((page, annotation)) = app.views.get(index).and_then(|view| view.comments.selected)
                            && app.session.get(id).is_some_and(|doc| {
                                doc.allows_annotation()
                                    && doc
                                        .info
                                        .annotations
                                        .iter()
                                        .any(|a| a.page == page && a.index == annotation && a.in_reply_to.is_none() && comments::made_by(a, tool))
                            })
                        {
                            app.apply_edit(Edit::StyleAnnotation {
                                page,
                                index: annotation,
                                color: Some(color),
                                opacity: None,
                                width: None,
                                endings: None,
                            });
                        }
                    }
                });
            });
    });
}

/// Where a page of `page` size (points, as shown: after its rotation) is drawn in the grid cell
/// `cell`: as large as the picture area allows and centred in it, so a landscape or rotated page
/// fills the cell's width in its middle, as Acrobat draws it, instead of a small strip at the top.
/// The picture area leaves room for the page number and actions below. Its sides narrow only when
/// a picture that wide would reach the selection checkbox in the cell's corner.
pub(crate) fn grid_page_rect(cell: Rect, page: Vec2) -> Rect {
    let area = Rect::from_min_max(pos2(cell.left(), cell.top() + 16.0), pos2(cell.right(), cell.bottom() - 72.0));
    // A page size from a damaged file can be zero, NaN or infinite; draw such a page square.
    let side = |v: f32| if v.is_finite() { v.max(1.0) } else { 1.0 };
    let (w, h) = (side(page.x), side(page.y));
    let fit = |margin: f32| {
        let s = ((area.width() - 2.0 * margin).max(1.0) / w).min(area.height().max(1.0) / h);
        Rect::from_center_size(area.center(), vec2(w * s, h * s))
    };
    let wide = fit(16.0);
    // The checkbox sits at (8, 8)-(28, 28) from the cell's corner; keep 4 px clear of it.
    let checkbox = Rect::from_min_size(cell.min + vec2(8.0, 8.0), Vec2::splat(20.0)).expand(4.0);
    if wide.intersects(checkbox) { fit(30.0) } else { wide }
}

/// The organize toolbar: page operations on the selection (Acrobat's Organize Pages bar).
fn organize_toolbar(view: &mut DocView, info: &DocInfo, editable: bool, dirty: bool, unobstructed: bool, ui: &mut egui::Ui, t: &Tokens) {
    let targets = view.target_pages();
    let n = info.pages.len();
    let (first, last) = (targets.first().copied().unwrap_or(0), targets.last().copied().unwrap_or(0));
    egui::Frame::NONE.fill(t.chrome).inner_margin(egui::Margin::symmetric(16, 8)).show(ui, |ui| {
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 4.0;
            let label = match view.selected.len() {
                0 => crate::i18n::fmt(tl!("Page {p} of {n}"), &[("p", &(view.current + 1).to_string()), ("n", &n.to_string())]),
                1 => tl!("1 page selected").to_string(),
                k => crate::i18n::fmt(tl!("{n} pages selected"), &[("n", &k.to_string())]),
            };
            // Fixed width so the buttons never shift as the selection text changes.
            ui.add_sized([150.0, 30.0], egui::Label::new(egui::RichText::new(label).font(theme::medium(13.0)).color(t.text_muted)).truncate());
            ui.add_enabled_ui(editable, |ui| {
                ui.add_space(8.0);
                if ui.add_enabled_ui(first > 0, |ui| icons::button(ui, "chevron-left", 30.0, false, tl!("Move earlier"))).inner.clicked() {
                    view.pending_edit = Some(Edit::MovePages { pages: targets.clone(), to: first - 1 });
                }
                if ui.add_enabled_ui(last + 1 < n, |ui| icons::button(ui, "chevron-right", 30.0, false, tl!("Move later"))).inner.clicked() {
                    view.pending_edit = Some(Edit::MovePages { pages: targets.clone(), to: first + 1 });
                }
            });
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if widgets::ghost_button(ui, "x", tl!("Close")).on_hover_text(tl!("Back to the document")).clicked() {
                    view.organize = false;
                }
                // What the grid shows is what is saved.
                let save = ui.add_enabled_ui(dirty, |ui| widgets::pill_button(ui, tl!("Save pages"), true)).inner;
                if save.on_hover_text(tl!("Save these pages as one PDF")).on_disabled_hover_text(tl!("No changes to save")).clicked() {
                    view.pending_action = Some(ViewAction::Save);
                }
                ui.add_space(8.0);
                // Page size in the grid (right to left: in, the percentage, out).
                let zoom = view.grid_zoom;
                if ui
                    .add_enabled_ui(zoom < *GRID_ZOOM_RANGE.end(), |ui| icons::button(ui, "zoom-in", 30.0, false, tl!("Larger pages")))
                    .inner
                    .clicked()
                {
                    view.grid_zoom_request = Some(zoom * GRID_ZOOM_STEP);
                }
                let percent = egui::Button::new(egui::RichText::new(format!("{:.0}%", zoom * 100.0)).font(theme::medium(12.0)).color(t.text_muted))
                    .frame(false);
                let percent = ui.add_sized([44.0, 30.0], percent);
                percent.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, tl!("Reset page size")));
                if percent.on_hover_text(tl!("Reset page size")).clicked() {
                    view.grid_zoom_request = Some(1.0);
                }
                if ui
                    .add_enabled_ui(zoom > *GRID_ZOOM_RANGE.start(), |ui| icons::button(ui, "zoom-out", 30.0, false, tl!("Smaller pages")))
                    .inner
                    .clicked()
                {
                    view.grid_zoom_request = Some(zoom / GRID_ZOOM_STEP);
                }
            });
        });
    });
    // Keys act on the selection unless a text field or a dialog has them (Delete under
    // Preferences deleted the selected pages).
    if editable && unobstructed && !ui.ctx().egui_wants_keyboard_input() {
        use egui::{Key, Modifiers};
        let (del, esc) = ui.input_mut(|i| {
            (
                i.consume_key(Modifiers::NONE, Key::Delete) || i.consume_key(Modifiers::NONE, Key::Backspace),
                i.consume_key(Modifiers::NONE, Key::Escape),
            )
        });
        // ⌘C / ⌘X / ⌘V copy, cut and paste pages (egui delivers them as clipboard events).
        let (copy, cut, paste) = ui.input(|i| {
            let any = |f: &dyn Fn(&egui::Event) -> bool| i.events.iter().any(f);
            (any(&|e| matches!(e, egui::Event::Copy)), any(&|e| matches!(e, egui::Event::Cut)), any(&|e| matches!(e, egui::Event::Paste(_))))
        });
        if copy || cut {
            view.pending_action = Some(ViewAction::CopyPages { cut });
        }
        if paste {
            view.pending_action = Some(ViewAction::PastePages);
        }
        if del && targets.len() < n {
            view.pending_edit = Some(Edit::DeletePages { pages: targets });
        }
        if esc {
            view.selected.clear();
        }
    }
}

/// Organize pages: a thumbnail grid (Acrobat's Organize Pages view). Click selects, ⌘-click
/// toggles, ⇧-click extends; double-click opens the page.
/// The gap (0 = before the first page, n = after the last) the pointer points at in the grid.
pub(crate) fn drop_gap(cells: &[(usize, Rect)], p: Pos2) -> Option<usize> {
    let (i, r) = cells.iter().min_by(|(_, a), (_, b)| a.distance_sq_to_pos(p).total_cmp(&b.distance_sq_to_pos(p)))?;
    Some(if p.x < r.center().x { *i } else { i + 1 })
}

/// Ctrl (Option on macOS) held when a page drag drops copies the dragged pages into the gap
/// instead of moving them, as Acrobat's Organize Pages does.
///
/// The OS comes from the context, not `cfg!`: the wasm build runs on macOS too, where
/// `cfg!(target_os = "macos")` is false, so Option has to copy there as well (#3092 review).
fn copy_modifier(ctx: &egui::Context, m: &egui::Modifiers) -> bool {
    if crate::commands::mac_shortcuts(ctx) { m.alt } else { m.ctrl }
}

/// A "+" on a gap of the page grid: insert files there. Returns whether it was clicked.
fn gap_button(ui: &mut egui::Ui, gap: usize, at: Pos2, height: f32, label: String, t: &Tokens) -> bool {
    let resp = ui.interact(Rect::from_center_size(at, Vec2::splat(22.0)), ui.id().with(("org-gap", gap)), Sense::click());
    resp.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, label.clone()));
    let hot = resp.hovered();
    if hot {
        ui.painter().line_segment([pos2(at.x, at.y - height / 2.0), pos2(at.x, at.y + height / 2.0)], Stroke::new(3.0, t.accent));
    }
    ui.painter().circle(at, 9.0, if hot { t.accent } else { t.chrome }, Stroke::new(1.0, if hot { t.accent } else { t.border }));
    let ink = Stroke::new(1.5, if hot { Color32::WHITE } else { t.text_muted });
    ui.painter().line_segment([at - vec2(4.0, 0.0), at + vec2(4.0, 0.0)], ink);
    ui.painter().line_segment([at - vec2(0.0, 4.0), at + vec2(0.0, 4.0)], ink);
    resp.on_hover_text(label).clicked()
}

fn organize_grid(view: &mut DocView, info: &DocInfo, editable: bool, dirty: bool, unobstructed: bool, ui: &mut egui::Ui, t: &Tokens) {
    let ppp = ui.ctx().pixels_per_point();
    // The page image scales with the zoom; the padding and the page number don't.
    let cell = vec2(146.0 * view.grid_zoom + 44.0, 194.0 * view.grid_zoom + 88.0);
    let anchor = view.grid_anchor.take();
    let reveal = view.grid_reveal.take();
    let mut open_page = None;
    organize_toolbar(view, info, editable, dirty, unobstructed, ui, t);
    let viewport = ui.available_rect_before_wrap();
    view.viewport_screen = viewport;
    let auto_delta = if unobstructed {
        view.auto_scroll.update(ui, viewport, true)
    } else {
        view.auto_scroll.cancel();
        Vec2::ZERO
    };
    let middle_gesture = view.auto_scroll.blocks_input();
    let mut cells: Vec<(usize, Rect)> = Vec::new();
    // The pages in view that are drawn larger than their thumbnails, and those of them that
    // need a sharper render than they have. The width is the size drawn, in device pixels.
    let (mut in_view, mut sharper): (Vec<(usize, f32)>, Vec<RenderRequest>) = (Vec::new(), Vec::new());
    // The source cell can scroll out of the virtualized rows during a drag.
    let mut drop = view.org_drag.is_some() && ui.input(|i| i.pointer.primary_released());
    egui::ScrollArea::vertical().auto_shrink([false, false]).show_viewport(ui, |ui, clip| {
        if middle_gesture {
            let opacity = ui.opacity();
            ui.disable();
            ui.set_opacity(opacity);
        }
        if auto_delta != Vec2::ZERO {
            ui.scroll_with_delta_animation(auto_delta, egui::style::ScrollAnimation::none());
        }
        ui.add_space(20.0);
        let cols = ((ui.available_width() - 40.0) / cell.x).floor().max(1.0) as usize;
        let rows = info.pages.len().div_ceil(cols);
        let left = (ui.available_width() - cols as f32 * cell.x) / 2.0;
        let (grid, _) = ui.allocate_exact_size(vec2(ui.available_width(), rows as f32 * cell.y), Sense::hover());
        // The page Organize opened on may lie in a row the virtualized grid has not laid out,
        // so its cell is computed rather than found.
        if let Some(page) = reveal.filter(|p| *p < info.pages.len()) {
            let row = page / cols;
            let col = page % cols;
            let c = Rect::from_min_size(pos2(grid.left() + left + col as f32 * cell.x, grid.top() + row as f32 * cell.y), cell);
            ui.scroll_to_rect_animation(c, Some(egui::Align::Center), egui::style::ScrollAnimation::none());
        }
        for row in thumbnail_rows(clip.top() - 20.0, clip.bottom() - 20.0, cell.y, rows) {
            let row_rect = Rect::from_min_size(grid.min + vec2(0.0, row as f32 * cell.y), vec2(grid.width(), cell.y));
            for col in 0..cols {
                let i = row * cols + col;
                let Some(p) = info.pages.get(i) else { break };
                let c = Rect::from_min_size(pos2(row_rect.left() + left + col as f32 * cell.x, row_rect.top()), cell);
                view.need_thumbnail(i, c.intersects(ui.clip_rect()));
                let resp = ui.interact(c, ui.id().with(("org", i)), if editable { Sense::click_and_drag() } else { Sense::click() });
                cells.push((i, c));
                if let Some((_, above)) = anchor.filter(|(page, _)| *page == i) {
                    ui.scroll_to_rect_animation(c.translate(vec2(0.0, -above)), Some(egui::Align::Min), egui::style::ScrollAnimation::none());
                }
                // Drag pages to move them (the selection, or the page grabbed).
                if resp.drag_started() {
                    if !view.selected.contains(&i) {
                        view.selected = [i].into();
                        view.select_anchor = Some(i);
                    }
                    let mut pages: Vec<usize> = view.selected.iter().copied().collect();
                    pages.sort_unstable();
                    view.org_drag = Some(pages);
                }
                if resp.drag_stopped() {
                    drop = true;
                }
                let page_label = crate::i18n::fmt(tl!("Page {label}"), &[("label", &p.label)]);
                resp.widget_info(|| egui::WidgetInfo::selected(egui::WidgetType::Checkbox, true, view.selected.contains(&i), page_label.clone()));
                let pr = grid_page_rect(c, vec2(p.width, p.height));
                let size = pr.size();
                let selected = view.selected.contains(&i) || (view.selected.is_empty() && i == view.current);
                if selected || resp.hovered() {
                    ui.painter().rect_filled(c.shrink(6.0), CornerRadius::same(8), if selected { t.accent_soft } else { t.hover });
                }
                if view.selected.contains(&i) {
                    ui.painter().rect_stroke(c.shrink(6.0), CornerRadius::same(8), Stroke::new(1.5, t.accent), egui::StrokeKind::Inside);
                }
                ui.painter().rect_filled(pr.translate(vec2(0.0, 1.5)), CornerRadius::same(1), t.page_shadow);
                ui.painter().rect_filled(pr, CornerRadius::ZERO, Color32::WHITE);
                // A page drawn larger than its thumbnail gets a render at the size drawn, while
                // it is in view; until that arrives the thumbnail stands in.
                let want = size.x * ppp;
                let thumb = view.thumbs.get(&i).map(|t| &t.tex);
                let thumb_ok = thumb.is_some_and(|t| t.size()[0] as f32 >= want * GRID_SHARP.start());
                let sharp = view.grid_pages.get(&i).filter(|_| !thumb_ok);
                if !thumb_ok && ui.is_rect_visible(c) {
                    in_view.push((i, want));
                    if !sharp.is_some_and(|(w, _)| sharp_enough(*w, want)) && !view.errors.contains_key(&i) {
                        sharper.push(RenderRequest {
                            page: i,
                            kind: RequestKind::Pixels,
                            tile: None,
                            scale: want / p.width.max(1.0),
                            tag: GRID_TAG,
                            // Grid cells are previews: tiny images are skipped for them.
                            preview: true,
                        });
                    }
                }
                if let Some(tex) = sharp.map(|(_, t)| t).or(thumb) {
                    ui.painter().image(tex.id(), pr, Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)), Color32::WHITE);
                }
                ui.painter().rect_stroke(pr, CornerRadius::ZERO, Stroke::new(1.0, t.border), egui::StrokeKind::Outside);
                let label_font = theme::medium(12.0);
                let label_w = (c.width() - 12.0).max(0.0);
                let page_label = crate::widgets::fit_line(&p.label, label_w, |s| crate::widgets::text_width(ui, s, &label_font));
                let label_pos = pos2(c.center().x, c.bottom() - 48.0);
                ui.painter().with_clip_rect(Rect::from_center_size(label_pos, vec2(label_w, 18.0))).text(
                    label_pos,
                    Align2::CENTER_CENTER,
                    page_label,
                    label_font,
                    t.text_muted,
                );
                // The checkbox toggles just this page without requiring a modifier. Keep it
                // outside the page image, including on very tall or wide pages.
                let mut checked = view.selected.contains(&i);
                let check_rect = Rect::from_min_size(c.min + vec2(8.0, 8.0), Vec2::splat(20.0));
                let check = ui.push_id(("org-check", i), |ui| ui.put(check_rect, widgets::checkbox_without_text(&mut checked))).inner;
                let label = crate::i18n::fmt(tl!("Select page {label}"), &[("label", &p.label)]);
                check.widget_info(|| egui::WidgetInfo::selected(egui::WidgetType::Checkbox, true, checked, label.clone()));
                if check.on_hover_text(label).changed() {
                    view.click_page(i, egui::Modifiers::COMMAND, false);
                    // The document shows the page picked here when the grid closes, as after a
                    // Pages-panel click, so reopening the grid keeps the pick (issue #1910).
                    view.go_to_page(i);
                }
                // Local actions always target the labelled page. The toolbar above the grid
                // remains the place for actions on the whole selection.
                if selected || resp.contains_pointer() || resp.has_focus() {
                    let actions = Rect::from_center_size(pos2(c.center().x, c.bottom() - 24.0), vec2(84.0, 26.0));
                    ui.scope_builder(
                        egui::UiBuilder::new().id_salt(("org-actions", i)).max_rect(actions).layout(egui::Layout::left_to_right(egui::Align::Center)),
                        |ui| {
                            ui.spacing_mut().item_spacing.x = 3.0;
                            ui.add_enabled_ui(editable, |ui| {
                                for (icon, degrees, key) in
                                    [("rotate-ccw", -90, "Rotate page {label} counterclockwise"), ("rotate-cw", 90, "Rotate page {label} clockwise")]
                                {
                                    let label = crate::i18n::fmt(tl!(key), &[("label", &p.label)]);
                                    if icons::button(ui, icon, 26.0, false, &label).clicked() {
                                        view.pending_edit = Some(Edit::RotatePages { pages: vec![i], degrees });
                                    }
                                }
                                let label = crate::i18n::fmt(tl!("Delete page {label}"), &[("label", &p.label)]);
                                if ui.add_enabled_ui(info.pages.len() > 1, |ui| icons::button(ui, "trash-2", 26.0, false, &label)).inner.clicked() {
                                    view.pending_edit = Some(Edit::DeletePages { pages: vec![i] });
                                }
                            });
                        },
                    );
                }
                if resp.clicked() {
                    let m = ui.input(|i| i.modifiers);
                    view.click_page(i, m, false);
                    view.go_to_page(i);
                }
                if resp.double_clicked() {
                    open_page = Some(i);
                }
                // Right-click: Extract, Replace, Cut, Copy, Paste (on the selection, or this page).
                resp.context_menu(|ui| {
                    if !view.selected.contains(&i) {
                        view.selected = [i].into();
                        view.go_to_page(i);
                    }
                    if ui.button(tl!("Extract pages")).clicked() {
                        view.pending_action = Some(ViewAction::Extract);
                        ui.close();
                    }
                    if ui.button(tl!("Split document…")).clicked() {
                        view.pending_action = Some(ViewAction::SplitBeforeSelected);
                        ui.close();
                    }
                    if ui.add_enabled(editable, egui::Button::new(tl!("Replace pages…"))).clicked() {
                        view.pending_action = Some(ViewAction::ReplacePages);
                        ui.close();
                    }
                    ui.separator();
                    if ui.add_enabled(editable, egui::Button::new(tl!("Cut"))).clicked() {
                        view.pending_action = Some(ViewAction::CopyPages { cut: true });
                        ui.close();
                    }
                    if ui.button(tl!("Copy")).clicked() {
                        view.pending_action = Some(ViewAction::CopyPages { cut: false });
                        ui.close();
                    }
                    if ui.add_enabled(editable, egui::Button::new(tl!("Paste after"))).clicked() {
                        view.pending_action = Some(ViewAction::PastePages);
                        ui.close();
                    }
                });
            }
        }
        // Between pages (and at both ends): insert files right there.
        if editable && !middle_gesture && view.org_drag.is_none() {
            let mid = |r: &Rect| r.top() + 16.0 + (cell.y - 88.0) / 2.0;
            for (i, c) in &cells {
                let label = info
                    .pages
                    .get(*i)
                    .map(|p| crate::i18n::fmt(tl!("Insert a file before page {label}"), &[("label", &p.label)]))
                    .unwrap_or_default();
                if gap_button(ui, *i, pos2(c.left(), mid(c)), cell.y - 88.0, label, t) {
                    view.pending_action = Some(ViewAction::InsertFromFileAt(*i));
                }
            }
            if let Some((i, c)) = cells.last()
                && i + 1 == info.pages.len()
                && gap_button(ui, i + 1, pos2(c.right(), mid(c)), cell.y - 88.0, tl!("Insert a file at the end").to_string(), t)
            {
                view.pending_action = Some(ViewAction::InsertFromFileAt(i + 1));
            }
        }
        // While dragging: the gap the pages would go to, drawn as a bar.
        if let (Some(pages), Some(p)) = (view.org_drag.clone(), ui.input(|i| i.pointer.hover_pos())) {
            // Ctrl (Option on macOS) held over the drop copies the pages; see #2997.
            let copying = copy_modifier(ui.ctx(), &ui.input(|i| i.modifiers));
            if let Some(gap) = drop_gap(&cells, p) {
                let x = match cells.iter().find(|(i, _)| *i == gap) {
                    Some((_, r)) => r.left() + 3.0,
                    None => cells.last().map_or(0.0, |(_, r)| r.right() - 3.0),
                };
                let row = cells.iter().find(|(i, _)| *i == gap).or(cells.last()).map(|(_, r)| r.y_range()).unwrap_or(egui::Rangef::new(0.0, 0.0));
                ui.painter().line_segment([pos2(x, row.min + 10.0), pos2(x, row.max - 10.0)], Stroke::new(3.0, t.accent));
                let mut text_at = p + vec2(14.0, 14.0);
                if copying {
                    // A "+" by the pointer: this drop copies the pages, it doesn't move them.
                    let badge = p + vec2(12.0, 12.0);
                    ui.painter().circle(badge, 9.0, t.accent, Stroke::new(1.0, t.accent));
                    let ink = Stroke::new(1.5, Color32::WHITE);
                    ui.painter().line_segment([badge - vec2(4.0, 0.0), badge + vec2(4.0, 0.0)], ink);
                    ui.painter().line_segment([badge - vec2(0.0, 4.0), badge + vec2(0.0, 4.0)], ink);
                    text_at.x += 18.0;
                }
                ui.painter().text(
                    text_at,
                    Align2::LEFT_TOP,
                    if pages.len() == 1 { tl!("1 page").to_string() } else { crate::i18n::fmt(tl!("{n} pages"), &[("n", &pages.len().to_string())]) },
                    theme::medium(12.0),
                    t.accent_text,
                );
            }
            if drop {
                view.org_drag = None;
                if let Some(gap) = drop_gap(&cells, p) {
                    if copying {
                        // The copies land at the gap as it is: nothing leaves its place.
                        view.pending_edit = Some(Edit::DuplicatePages { pages: pages.clone(), at: Some(gap) });
                        view.selected = (gap..gap + pages.len()).collect();
                    } else {
                        // `to` counts positions without the moving pages.
                        let to = gap - pages.iter().filter(|x| **x < gap).count();
                        let first = pages[0];
                        let contiguous = pages.windows(2).all(|w| w[1] == w[0] + 1);
                        if !(contiguous && to == first) {
                            view.pending_edit = Some(Edit::MovePages { pages: pages.clone(), to });
                            view.selected = (to..to + pages.len()).collect();
                        }
                    }
                }
            }
        } else if drop {
            view.org_drag = None;
        }
    });
    // Files dragged in from outside go to the gap under the pointer. Some platforms don't say
    // where the pointer is until the files are dropped; then the whole grid is the target.
    let pointer = ui.input(|i| i.pointer.hover_pos()).filter(|p| viewport.contains(*p));
    view.grid_gap = pointer.filter(|_| editable && view.org_drag.is_none()).and_then(|p| drop_gap(&cells, p));
    if editable && ui.input(|i| !i.raw.hovered_files.is_empty()) {
        let painter = ui.painter().with_clip_rect(viewport);
        painter.rect_stroke(viewport.shrink(2.0), CornerRadius::same(6), Stroke::new(2.0, t.accent), egui::StrokeKind::Inside);
        if let Some(gap) = view.grid_gap {
            let at = cells.iter().find(|(i, _)| *i == gap).map(|(_, r)| (r.left() + 3.0, r.y_range()));
            if let Some((x, row)) = at.or(cells.last().map(|(_, r)| (r.right() - 3.0, r.y_range()))) {
                painter.line_segment([pos2(x, row.min + 10.0), pos2(x, row.max - 10.0)], Stroke::new(3.0, t.accent));
            }
        }
        // A "+" that follows the pointer.
        if let Some(p) = pointer {
            let at = p + vec2(16.0, 16.0);
            painter.circle_filled(at, 9.0, t.accent);
            painter.line_segment([at - vec2(4.0, 0.0), at + vec2(4.0, 0.0)], Stroke::new(1.5, Color32::WHITE));
            painter.line_segment([at - vec2(0.0, 4.0), at + vec2(0.0, 4.0)], Stroke::new(1.5, Color32::WHITE));
        }
    }
    view.auto_scroll.paint(ui, viewport);
    // Pinch, or Ctrl/⌘ with the wheel, zooms the grid about the page under the pointer.
    let pinch = crate::motion::zoom_delta(ui.ctx());
    if (pinch - 1.0).abs() > 0.001 && pointer.is_some() && !middle_gesture {
        view.grid_zoom_request = Some(view.grid_zoom * pinch);
    }
    if let Some(zoom) = view.grid_zoom_request.take() {
        let before = view.grid_zoom;
        view.set_grid_zoom(zoom);
        if view.grid_zoom != before {
            // Keep the page under the pointer (or else the first one in view) where it is.
            let under = pointer.and_then(|p| cells.iter().find(|(_, r)| r.contains(p)));
            let anchor = under.or_else(|| cells.iter().find(|(_, r)| r.bottom() > viewport.top()));
            view.grid_anchor = anchor.map(|(i, r)| (*i, r.top() - viewport.top()));
            ui.ctx().request_repaint();
        }
    }
    // When this frame's thumbnail will cover the cell, drop the sharp render without waiting
    // for that texture to arrive.
    let density = thumbnail_density(view.thumb_demand.len(), ppp);
    in_view.retain(|&(page, want)| {
        info.pages.get(page).is_none_or(|p| (device_pixels(p.width, thumbnail_scale(p.width, p.height, density)) as f32) < want * GRID_SHARP.start())
    });
    sharper.retain(|req| in_view.iter().any(|(page, _)| *page == req.page));
    view.grid_pages.retain(|page, _| in_view.iter().any(|(kept, _)| kept == page));
    // Sharp renders of the pages in view join this frame's demand ahead of the thumbnails.
    view.frame_queue.extend(sharper);
    if let Some(p) = open_page {
        view.organize = false;
        view.go_to_page(p);
    }
}

/// A rendered raster as texture data. Its words are premultiplied RGBA bytes, exactly
/// `Color32`s, so the renderer's buffer becomes the image without a copy on the UI thread.
pub(crate) fn texture_image(size: [usize; 2], pixels: pdfcraft_render::Pixels) -> egui::ColorImage {
    match bytemuck::allocation::try_cast_vec::<u32, Color32>(pixels.into_words()) {
        Ok(px) if px.len() == size[0].saturating_mul(size[1]) => egui::ColorImage::new(size, px),
        Ok(px) => egui::ColorImage::from_rgba_premultiplied(size, bytemuck::cast_slice(&px)),
        Err((_, words)) => egui::ColorImage::from_rgba_premultiplied(size, bytemuck::cast_slice(&words)),
    }
}

/// A luminance preview for a monochrome print. `Color32` stores premultiplied pixels, so a
/// non-opaque colour's alpha is restored after multiplying the unmultiplied RGB by its
/// luminance. This uses the display gamma-space weights users expect in editors/print dialogs,
/// not the linear-light coefficients of a colour-managed render.
fn grayscale(mut img: egui::ColorImage) -> egui::ColorImage {
    for c in img.pixels.iter_mut() {
        let a = c.a();
        let luminance = f32::from(c.r()) * 0.299_f32 + f32::from(c.g()) * 0.587_f32 + f32::from(c.b()) * 0.114_f32;
        let gray = if a == 0 || a == 255 {
            luminance.round() as u8
        } else {
            // Unmultiply first, so translucent content keeps its paper-relative luminance.
            let f = f32::from(a) / 255.0;
            let un = [f32::from(c.r()) / f, f32::from(c.g()) / f, f32::from(c.b()) / f];
            let l = un[0] * 0.299_f32 + un[1] * 0.587_f32 + un[2] * 0.114_f32;
            (f * l).round() as u8
        };
        *c = Color32::from_rgba_premultiplied(gray, gray, gray, a);
    }
    img
}

#[cfg(test)]
mod tests {
    #[test]
    fn zoom_anchor_scroll_stays_within_the_content() {
        let (content, viewport) = (vec2(1000.0, 2000.0), vec2(800.0, 600.0));
        // Past the end: clamped to the maximum (200, 1400), not left for egui to snap back.
        assert_eq!(anchored_scroll(pos2(900.0, 1900.0), vec2(0.0, 0.0), content, viewport), vec2(200.0, 1400.0));
        // Before the start.
        assert_eq!(anchored_scroll(pos2(10.0, 10.0), vec2(300.0, 300.0), content, viewport), Vec2::ZERO);
        // Inside: unchanged.
        assert_eq!(anchored_scroll(pos2(150.0, 800.0), vec2(100.0, 100.0), content, viewport), vec2(50.0, 700.0));
        // Content smaller than the viewport: no scrolling.
        assert_eq!(anchored_scroll(pos2(400.0, 400.0), Vec2::ZERO, vec2(500.0, 500.0), viewport), vec2(0.0, 0.0));
    }

    #[test]
    fn page_size_hint_zone_is_the_bottom_left_quadrant() {
        let area = Rect::from_min_max(pos2(100.0, 50.0), pos2(500.0, 450.0));
        assert!(in_size_hint_zone(area, pos2(110.0, 440.0)));
        assert!(!in_size_hint_zone(area, pos2(110.0, 100.0)), "top-left");
        assert!(!in_size_hint_zone(area, pos2(400.0, 440.0)), "bottom-right");
        assert!(!in_size_hint_zone(area, pos2(50.0, 440.0)), "outside");
        assert!(!in_size_hint_zone(area, pos2(f32::NAN, 440.0)));
    }

    use super::*;

    #[test]
    fn typed_zoom_percentages_parse() {
        for (typed, zoom) in [("150", 1.5), ("150%", 1.5), (" 62.5 % ", 0.625), ("6400%", 64.0)] {
            assert_eq!(DocView::parse_zoom_percent(typed), Some(zoom), "{typed}");
        }
        for typed in ["", "%", "abc", "0", "-50", "NaN", "inf", "1e40", "150%%x", "%150"] {
            assert_eq!(DocView::parse_zoom_percent(typed), None, "{typed}");
        }
    }

    /// A rotated (landscape) page is drawn as wide as the cell allows and centred in the picture
    /// area, as Acrobat draws it, not as a small strip pinned to the top; a portrait page still
    /// fills the area's height; neither covers the checkbox corner or the label below.
    #[test]
    fn grid_pages_are_centred_and_as_large_as_the_cell_allows() {
        let cell = Rect::from_min_size(pos2(100.0, 50.0), vec2(190.0, 282.0));
        let area = Rect::from_min_max(pos2(cell.left(), cell.top() + 16.0), pos2(cell.right(), cell.bottom() - 72.0));
        let checkbox = Rect::from_min_size(cell.min + vec2(8.0, 8.0), Vec2::splat(20.0));
        let landscape = grid_page_rect(cell, vec2(842.0, 595.0));
        assert!((landscape.center().y - area.center().y).abs() < 0.01, "centred vertically: {landscape:?}");
        assert!((landscape.center().x - cell.center().x).abs() < 0.01);
        assert!((landscape.width() - (cell.width() - 32.0)).abs() < 0.01, "as wide as the cell allows: {landscape:?}");
        assert!(!landscape.intersects(checkbox));
        let portrait = grid_page_rect(cell, vec2(595.0, 842.0));
        assert!(area.contains_rect(portrait), "inside the picture area: {portrait:?}");
        assert!((portrait.center().y - area.center().y).abs() < 0.01);
        assert!(portrait.left() >= checkbox.right(), "clear of the checkbox: {portrait:?}");
        // Degenerate sizes still give a finite rectangle inside the cell.
        for page in [vec2(0.0, 0.0), vec2(1e9, 1.0), vec2(1.0, 1e9), vec2(f32::NAN, 10.0), vec2(f32::INFINITY, 10.0)] {
            let r = grid_page_rect(cell, page);
            assert!(r.is_finite() && cell.contains_rect(r), "{page:?} -> {r:?}");
        }
    }

    fn annotation(fill_sign: bool) -> pdfcraft_render::Annotation {
        pdfcraft_render::Annotation {
            page: 0,
            subtype: "FreeText".into(),
            author: None,
            contents: Some("Hello".into()),
            modified: None,
            name: None,
            in_reply_to: None,
            rect: [0.0; 4],
            color: None,
            index: 0,
            state: None,
            quads: Vec::new(),
            locked: false,
            intent: None,
            fill_sign,
            no_rotate: false,
            points: Vec::new(),
        }
    }

    #[test]
    fn hover_popup_skips_fill_and_sign_marks() {
        assert_eq!(annotation_hover_text(&annotation(false)).as_deref(), Some("FreeText\nHello"));
        assert_eq!(annotation_hover_text(&annotation(true)), None);
    }

    fn signature(signed: bool, visible: bool, page: Option<usize>, rect: Option<[f64; 4]>) -> pdfcraft_engine::SignatureInfo {
        pdfcraft_engine::SignatureInfo {
            field: "Signature1".into(),
            signed,
            page,
            rect,
            visible,
            signer: None,
            certificate: None,
            chain: Vec::new(),
            date: None,
            signing_time: None,
            reason: None,
            location: None,
            contact: None,
            sub_filter: None,
            doc_timestamp: false,
            certify: None,
            revision: 1,
            signed_len: 0,
            digest: None,
            algorithm: None,
            timestamp: false,
            timestamp_time: None,
            status: pdfcraft_engine::SignatureStatus::Valid,
            modification: pdfcraft_engine::sign::Modification::None,
            details: Vec::new(),
        }
    }

    #[test]
    fn signature_badges_keep_signed_visible_widgets_on_the_page() {
        let rect = [10.0, 10.0, 100.0, 50.0];
        let signatures =
            vec![signature(true, true, Some(0), Some(rect)), signature(true, true, Some(1), Some(rect)), signature(true, true, Some(0), Some(rect))];
        let badges = signature_badges(&signatures, 0);
        assert_eq!(badges.len(), 2);
        assert_eq!(badges[0].rect, rect);
    }

    #[test]
    fn signature_badges_skip_unsigned_invisible_and_unplaced_fields() {
        let rect = [10.0, 10.0, 100.0, 50.0];
        let signatures = vec![
            signature(false, true, Some(0), Some(rect)),
            signature(true, false, Some(0), Some(rect)),
            signature(true, true, Some(0), None),
            signature(true, true, Some(0), Some(rect)),
        ];
        let badges = signature_badges(&signatures, 0);
        assert_eq!(badges.len(), 1);
        assert_eq!(badges[0].rect, rect);
    }

    #[test]
    fn signature_badges_skip_rectangles_that_cannot_reach_the_screen() {
        let hostile = vec![
            [f64::NAN, 10.0, 100.0, 50.0],
            [10.0, f64::INFINITY, 100.0, 50.0],
            [10.0, 10.0, f64::NEG_INFINITY, 50.0],
            [10.0, 10.0, 1e300, 50.0],
            [10.0, 10.0, 100.0, 50.0],
        ];
        let signatures: Vec<_> = hostile.into_iter().map(|rect| signature(true, true, Some(0), Some(rect))).collect();
        let badges = signature_badges(&signatures, 0);
        assert_eq!(badges.len(), 1);
    }

    #[test]
    fn page_zoom_reads_the_side_the_page_width_is_on() {
        // A 200 x 100 pt page at 2 pixels per point: 400 x 200 on screen, or 200 x 400 turned.
        let xf = |rot: u16, size: Vec2| PageXform { rect: Rect::from_min_size(Pos2::ZERO, size), rot, pw: 200.0, ph: 100.0 };
        for rot in [0, 180] {
            assert_eq!(xf(rot, vec2(400.0, 200.0)).zoom(), 2.0, "{rot}");
        }
        for rot in [90, 270] {
            assert_eq!(xf(rot, vec2(200.0, 400.0)).zoom(), 2.0, "{rot}: the width runs down the screen");
        }
    }

    #[test]
    fn snapshot_tabs_read_like_their_source_plus_the_suffix() {
        let snap = Snapshot { kind: SnapshotKind::SignedVersion, revision: 1, from: "report.pdf".into() };
        assert_eq!(snapshot_tab_name("Report", "report.pdf", &snap), "Report (signed version)");
        let rev = Snapshot { kind: SnapshotKind::Revision, revision: 2, from: "report.pdf".into() };
        assert_eq!(snapshot_tab_name("Report", "report.pdf", &rev), "Report (revision 2)");
        assert_eq!(snapshot_tab_name("report.pdf", "report.pdf", &snap), "report.pdf");
        assert_eq!(snapshot_tab_name("report (revision 2).pdf", "report (revision 2).pdf", &rev), "report (revision 2).pdf");
    }

    #[test]
    fn grayscale_converts_colours_to_their_luminance() {
        let img = egui::ColorImage::new([2, 1], vec![Color32::from_rgb(255, 0, 0), Color32::from_rgba_premultiplied(0, 51, 102, 128)]);
        let gray = grayscale(img);
        let opaque = gray.pixels[0];
        assert_eq!(opaque.to_array(), [76, 76, 76, 255]);
        // Unmultiply RGB first, so a translucent colour keeps its paper-relative luminance.
        let translucent = gray.pixels[1];
        assert_eq!(translucent.to_array(), [42, 42, 42, 128]);
    }

    #[test]
    fn black_white_and_transparents_are_preserved_by_grayscale() {
        let img = egui::ColorImage::new([3, 1], vec![Color32::BLACK, Color32::WHITE, Color32::TRANSPARENT]);
        let gray = grayscale(img);
        assert_eq!(gray.pixels[0].to_array(), [0, 0, 0, 255]);
        assert_eq!(gray.pixels[1].to_array(), [255, 255, 255, 255]);
        assert_eq!(gray.pixels[2].to_array(), [0, 0, 0, 0]);
    }

    #[test]
    fn rendered_rasters_become_texture_images_without_a_copy() {
        let pdf = b"%PDF-1.4
1 0 obj << /Type /Catalog /Pages 2 0 R >> endobj
2 0 obj << /Type /Pages /Kids [3 0 R] /Count 1 >> endobj
3 0 obj << /Type /Page /Parent 2 0 R /MediaBox [0 0 100 50] /Contents 4 0 R >> endobj
4 0 obj << /Length 35 >> stream
0 0 1 rg 10 10 30 20 re f
endstream endobj
trailer << /Root 1 0 R >>
%%EOF";
        let mut r = pdfcraft_render::PageRenderer::new(Arc::new(pdf.to_vec()), Default::default());
        let out = r.render(RenderRequest { page: 0, kind: RequestKind::Pixels, tile: None, scale: 1.5, tag: 0, preview: false });
        assert!(out.error.is_none(), "{:?}", out.error);
        let size = [out.width as usize, out.height as usize];
        let copied = egui::ColorImage::from_rgba_premultiplied(size, &out.rgba);
        let at = out.rgba.as_ptr();
        let img = texture_image(size, out.rgba);
        assert_eq!(img, copied);
        assert_eq!(img.pixels.as_ptr().cast::<u8>(), at, "the renderer's own buffer");
    }

    #[test]
    fn tiles_stay_granular_up_close_and_cover_giant_sheets_in_a_handful() {
        // Ordinary zooms keep 1024 px tiles; degenerate scales fall back to it, never zero.
        assert_eq!(tile_side(1.0, 2000), TILE);
        assert_eq!(tile_side(4.0, 2000), TILE);
        assert_eq!(tile_side(f32::NAN, 2000), TILE);
        assert_eq!(tile_side(f32::INFINITY, 2000), TILE);
        assert_eq!(tile_side(0.0, 2000), TILE);
        // Zoomed viewports need a few large tiles instead of many small ones.
        assert_eq!(tile_side(12.0, 2000), 2048);
        assert_eq!(tile_side(20.0, 2000), 3413);
        assert_eq!(tile_side(100.0, 2000), 4096);
        // A giant sheet at fit width (6740 px long side) is covered four tiles a side.
        assert_eq!(tile_side(96.0 / 72.0, 6740), 1685);
        // The backend clamps tile sides at 4096.
        assert_eq!(tile_side(20.0, 100_000), 4096);
    }

    #[test]
    fn grid_zoom_is_clamped_and_sharp_renders_tolerate_a_pinch() {
        assert!(sharp_enough(900, 876.0) && sharp_enough(800, 876.0) && sharp_enough(1300, 876.0));
        assert!(!sharp_enough(264, 876.0), "a thumbnail stretched over a zoomed page");
        assert!(!sharp_enough(876, 200.0), "far larger than drawn: wasteful");
        assert!(!sharp_enough(100, 0.0) && !sharp_enough(100, f32::NAN));
        let mut v = view(3, PageLayout::Single, true);
        for odd in [f32::NAN, f32::INFINITY] {
            v.set_grid_zoom(odd);
            assert_eq!(v.grid_zoom(), 1.0);
        }
        v.set_grid_zoom(99.0);
        assert_eq!(v.grid_zoom(), 3.0);
        v.set_grid_zoom(-4.0);
        assert_eq!(v.grid_zoom(), 0.5);
    }

    fn view(pages: usize, layout: PageLayout, continuous: bool) -> DocView {
        let info = pdfcraft_render::DocInfo {
            pages: (0..pages)
                .map(|_| pdfcraft_render::PageInfo { width: 300.0, height: 400.0, label: String::new(), crop: [0.0, 0.0, 300.0, 400.0], rotation: 0 })
                .collect(),
            ..Default::default()
        };
        DocView::new(DocId(1), &info, ViewDefaults { layout, continuous, ..Default::default() })
    }

    fn history_search_view() -> DocView {
        let mut v = view(5, PageLayout::Single, true);
        v.texts.insert(
            3,
            pdfcraft_render::PageText {
                glyphs: vec![pdfcraft_render::TextGlyph {
                    text: "match".into(),
                    rect: [20.0, 260.0, 60.0, 280.0],
                    quad: [[20.0, 260.0], [60.0, 260.0], [60.0, 280.0], [20.0, 280.0]],
                    direction: [1.0, 0.0],
                }],
                line_of: vec![0],
                space_before: vec![false],
            }
            .into(),
        );
        v.find = Some(Find { matches: vec![(3, 0..1), (3, 0..1), (1, 0..1)], current: Some(2), ..Default::default() });
        v
    }

    #[test]
    fn explicit_search_jumps_record_history_and_keep_match_destination() {
        for step in [None, Some(true), Some(false)] {
            let mut v = history_search_view();
            v.page_input = "draft in progress".into();
            v.goto_point = Some((3, Some(0.8), Some(0.9), Vec2::ZERO));
            if let Some(forward) = step {
                v.find_step(forward);
            } else {
                v.go_to_match(0);
            }
            assert_eq!(v.back, vec![0], "explicit search jump must remember its origin");
            assert_eq!(v.goto, Some((3, 0.5)), "keep the match offset instead of the page top");
            assert_eq!(v.current, 3);
            assert_eq!(v.current_page_label(None), "4");
            // Chrome refreshes the idle page box; navigation must not overwrite a draft
            // that may still be focused. DocView does not own the editor's focus state.
            assert_eq!(v.page_input, "draft in progress");
            assert!(v.goto_point.is_none(), "an earlier link destination cannot override the search offset");
            v.go_to_match(1);
            assert_eq!(v.back, vec![0], "same-page matches do not duplicate history");
            assert!(v.view_history(false));
            assert_eq!(v.current, 0);
            assert!(v.view_history(true));
            assert_eq!(v.current, 3);
            assert!(v.view_history(false));
            v.go_to_match(2);
            assert!(v.forward.is_empty(), "a new explicit jump discards Forward");
            assert_eq!(v.back, vec![0]);
        }
    }

    #[test]
    fn automatic_search_updates_do_not_record_history_or_discard_forward() {
        let mut v = history_search_view();
        v.forward = vec![4];
        v.find = Some(Find { query: "match".into(), ..Default::default() });
        v.rerun_find();
        assert_eq!(v.current, 3);
        assert_eq!(v.goto, Some((3, 0.5)));
        assert!(v.back.is_empty());
        assert_eq!(v.forward, vec![4]);
        v.current = 0;
        v.find.as_mut().unwrap().current = None;
        v.refresh_find_page(3);
        assert_eq!(v.current, 3);
        assert!(v.back.is_empty());
        assert_eq!(v.forward, vec![4]);
    }

    #[test]
    fn text_layers_evict_far_pages_but_keep_visible_and_matches() {
        let info = thumbnail_info(100);
        let mut v = DocView::new(DocId(1), &info, ViewDefaults::default());
        v.current = 50;
        v.frame_visible = HashSet::from([50, 51]);
        for p in 0..100 {
            v.texts.insert(p, Arc::new(PageText::default()));
        }
        v.text_failed.insert(0);
        v.text_failed.insert(99);
        v.find = Some(Find { matches: vec![(0, 0..1)], ..Default::default() });
        let _ = v.prepare_render_queue(&info, 1.0);
        assert!(v.texts.contains_key(&50) && v.texts.contains_key(&51), "visible pages stay");
        assert!(v.texts.contains_key(&0), "matched pages stay");
        assert!(v.texts.contains_key(&49), "nearest prefetch window stays");
        assert!(!v.texts.contains_key(&99), "far pages go");
        assert_eq!(v.texts.len(), 2 + 1 + MAX_CACHED_TEXT_PAGES, "visible + matched + prefetch window");
        assert!(v.text_failed.contains(&0) && v.text_failed.contains(&99), "failures are bytes-free: never evict");
    }

    /// A PDF of one line of text on each page, `lines[i]` on page `i`.
    fn text_pages_pdf(lines: &[&str]) -> Vec<u8> {
        let n = lines.len();
        // 1 catalog, 2 pages, 3 font, then a page and its content stream for each line.
        let mut objects = vec![
            "<< /Type /Catalog /Pages 2 0 R >>".to_string(),
            format!("<< /Type /Pages /Count {n} /Kids [{}] >>", (0..n).map(|i| format!("{} 0 R", 4 + 2 * i)).collect::<Vec<_>>().join(" ")),
            "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>".to_string(),
        ];
        for (i, line) in lines.iter().enumerate() {
            let content = format!("BT /F1 18 Tf 72 700 Td ({line}) Tj ET");
            objects.push(format!(
                "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Resources << /Font << /F1 3 0 R >> >> /Contents {} 0 R >>",
                5 + 2 * i
            ));
            objects.push(format!("<< /Length {} >>\nstream\n{content}\nendstream", content.len()));
        }
        let mut out = b"%PDF-1.7\n".to_vec();
        let mut offsets = Vec::new();
        for (i, o) in objects.iter().enumerate() {
            offsets.push(out.len());
            out.extend(format!("{} 0 obj\n{o}\nendobj\n", i + 1).bytes());
        }
        let xref = out.len();
        out.extend(format!("xref\n0 {}\n0000000000 65535 f \n", objects.len() + 1).bytes());
        for o in offsets {
            out.extend(format!("{o:010} 00000 n \n").bytes());
        }
        out.extend(format!("trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n", objects.len() + 1).bytes());
        out
    }

    /// #2639: scrolling continuously, Select All takes every page's text, and Copy gets all of it
    /// in page order, reading the pages whose text wasn't read for display.
    #[test]
    fn select_all_copies_the_whole_document_while_scrolling_continuously() {
        let mut app = PdfCraftApp::new();
        app.open_bytes("three.pdf", None, text_pages_pdf(&["Alpha one", "Bravo two", "Charlie three"])).unwrap();
        let v = &mut app.views[0];
        assert!(v.continuous, "continuous scrolling is the default");
        assert!(v.texts.is_empty(), "no page's text read yet");
        // Nothing shown is selected yet (false, as for a page whose text isn't read), but the whole
        // document is: Copy reads every page.
        assert!(!v.select_all());
        assert!(v.all_text && v.selection.is_none());
        assert_eq!(v.pages_without_text(), vec![0, 1, 2]);
        // Copy reads the pages off the UI thread; the clipboard gets the text once they're read.
        let ctx = egui::Context::default();
        assert!(app.copy_selected_text(0, &ctx), "the copy starts");
        assert!(app.document_copy.is_some(), "reading the pages, not on this thread");
        let mut text = None;
        for _ in 0..500 {
            let mut out = ctx.run_ui(egui::RawInput::default(), |ui| app.finish_document_copy(ui.ctx()));
            out.textures_delta.clear();
            text = out.platform_output.commands.iter().find_map(|c| match c {
                egui::OutputCommand::CopyText(t) => Some(t.clone()),
                _ => None,
            });
            if text.is_some() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        let text = text.expect("the copied text arrives");
        assert!(app.document_copy.is_none(), "done");
        let at = |w: &str| text.find(w).unwrap_or_else(|| panic!("{w:?} missing from {text:?}"));
        assert!(at("Alpha") < at("Bravo") && at("Bravo") < at("Charlie"), "page order: {text:?}");
        assert_eq!(text.lines().count(), 3, "a line per page: {text:?}");
        // A new scope ends it: one page at a time, Select All is the page's again.
        app.views[0].set_continuous(false);
        assert!(!app.views[0].all_text, "not after switching to one page at a time");
        // So do a page selection and a change of the document's content (an edit, undo).
        app.views[0].set_continuous(true);
        app.views[0].select_all();
        app.views[0].select_text(0, 0, 0);
        assert!(!app.views[0].all_text, "a page selection replaces it");
        app.views[0].select_all();
        app.views[0].invalidate_content();
        assert!(!app.views[0].all_text, "not past an edit of the document");
    }

    /// A whole-document copy reads the pages without text, keeps those already read and skips
    /// those that failed before.
    #[test]
    fn a_document_copy_reads_only_the_pages_it_needs() {
        let mut app = PdfCraftApp::new();
        app.open_bytes("three.pdf", None, text_pages_pdf(&["Alpha one", "Bravo two", "Charlie three"])).unwrap();
        app.views[0].text_failed.insert(2);
        let job = crate::DocumentCopyJob::of(&app.views[0], app.session.get(app.views[0].id).unwrap());
        let kinds: Vec<_> = job.pages.iter().map(|p| matches!(p, crate::CopyPage::ToRead)).collect();
        assert_eq!(kinds, vec![true, true, false], "pages 1 and 2 to read, page 3 failed before");
        assert!(matches!(job.pages[2], crate::CopyPage::Skip));
    }

    /// A later copy wins: a whole-document copy still reading its pages never replaces what was
    /// copied after it started, from the page or anywhere else (a text field, a comment).
    #[test]
    fn a_later_copy_cancels_a_document_copy_still_reading() {
        let mut app = PdfCraftApp::new();
        app.open_bytes("three.pdf", None, text_pages_pdf(&["Alpha one", "Bravo two", "Charlie three"])).unwrap();
        let ctx = egui::Context::default();
        // Another copy in the same frame (here a text field's) drops it.
        app.views[0].select_all();
        assert!(app.copy_selected_text(0, &ctx) && app.document_copy.is_some());
        let mut out = ctx.run_ui(egui::RawInput::default(), |ui| {
            ui.ctx().copy_text("a word".to_owned());
            app.finish_document_copy(ui.ctx());
        });
        out.textures_delta.clear();
        assert!(app.document_copy.is_none(), "cancelled");
        let copies: Vec<_> = out.platform_output.commands.iter().filter(|c| matches!(c, egui::OutputCommand::CopyText(_))).collect();
        assert_eq!(copies, vec![&egui::OutputCommand::CopyText("a word".to_owned())], "the later copy stays");
        // So does copying a page selection.
        app.views[0].select_all();
        assert!(app.copy_selected_text(0, &ctx) && app.document_copy.is_some());
        app.views[0].select_text(0, 0, 5);
        app.copy_selected_text(0, &ctx);
        assert!(app.document_copy.is_none(), "a page selection's copy cancels it");
    }

    /// A press anywhere on a page ends Select All of the document, whatever the tool (here the
    /// Hand, which selects no text) and whether the page's text is read yet.
    #[test]
    fn a_press_on_a_page_ends_select_all_of_the_document() {
        let mut h = egui_kittest::Harness::builder().with_size(egui::vec2(1400.0, 900.0)).build_eframe(|_cc| {
            let mut app = PdfCraftApp::new();
            app.open_bytes("three.pdf", None, text_pages_pdf(&["Alpha one", "Bravo two", "Charlie three"])).unwrap();
            app
        });
        let mut on_screen = None;
        for _ in 0..600 {
            h.run_steps(2);
            on_screen = h.state().views[0].page_screen_rect(0);
            if on_screen.is_some() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        let r = on_screen.expect("the first page is on screen");
        h.state_mut().quick_tool = crate::QuickTool::Hand;
        h.state_mut().views[0].select_all();
        assert!(h.state().views[0].all_text);
        let at = r.center();
        h.event(egui::Event::PointerMoved(at));
        h.event(egui::Event::PointerButton { pos: at, button: egui::PointerButton::Primary, pressed: true, modifiers: egui::Modifiers::NONE });
        h.run_steps(1);
        h.event(egui::Event::PointerButton { pos: at, button: egui::PointerButton::Primary, pressed: false, modifiers: egui::Modifiers::NONE });
        h.run_steps(2);
        assert!(!h.state().views[0].all_text, "a press on the page ends it");

        // A press while a dialog is up lands on the dialog, not the page: the selection stays.
        h.state_mut().views[0].select_all();
        h.state_mut().dialog = Some(crate::Dialog::JsConsole);
        h.run_steps(2);
        h.event(egui::Event::PointerMoved(at));
        h.event(egui::Event::PointerButton { pos: at, button: egui::PointerButton::Primary, pressed: true, modifiers: egui::Modifiers::NONE });
        h.run_steps(1);
        h.event(egui::Event::PointerButton { pos: at, button: egui::PointerButton::Primary, pressed: false, modifiers: egui::Modifiers::NONE });
        h.run_steps(2);
        assert!(h.state().views[0].all_text, "a press beneath a dialog doesn't end it");
    }

    /// The pages of a whole-document copy come out in order whatever was read before, and a
    /// cancelled copy gives nothing (a new copy or a closed document cancels the old one).
    #[test]
    fn a_document_copy_reads_in_page_order_and_stops_when_cancelled() {
        let mut app = PdfCraftApp::new();
        app.open_bytes("three.pdf", None, text_pages_pdf(&["Alpha one", "Bravo two", "Charlie three"])).unwrap();
        // The middle page's text read for display already; the others read by the copy.
        let doc = app.session.get(app.views[0].id).unwrap();
        let mut r = pdfcraft_render::PageRenderer::new(doc.bytes.clone(), Default::default());
        let middle = r.render(RenderRequest { page: 1, kind: RequestKind::Text, ..Default::default() }).text.unwrap();
        app.views[0].texts.insert(1, middle);
        let job = || crate::DocumentCopyJob::of(&app.views[0], app.session.get(app.views[0].id).unwrap());
        let text = job().text(&std::sync::atomic::AtomicBool::new(false)).unwrap();
        assert!(text.find("Alpha").unwrap() < text.find("Bravo").unwrap() && text.find("Bravo").unwrap() < text.find("Charlie").unwrap(), "{text:?}");
        assert_eq!(job().text(&std::sync::atomic::AtomicBool::new(true)), None, "cancelled");
    }

    /// Select All keeps to the page shown one page at a time, and in a document of one page; of
    /// the document, it reads the text already read and doesn't ask again for failed pages.
    #[test]
    fn select_all_of_the_document_only_while_scrolling_through_several_pages() {
        let info = thumbnail_info(3);
        let one_line = |s: &str| PageText {
            glyphs: s
                .chars()
                .map(|c| pdfcraft_render::TextGlyph { text: c.into(), rect: [0.0, 0.0, 10.0, 10.0], quad: [[0.0, 0.0]; 4], direction: [1.0, 0.0] })
                .collect(),
            line_of: s.chars().map(|_| 0).collect(),
            space_before: s.chars().map(|_| false).collect(),
        };
        let mut v = DocView::new(DocId(1), &info, ViewDefaults::default());
        for p in 0..3 {
            v.texts.insert(p, Arc::new(one_line("abc")));
        }
        v.continuous = false;
        v.current = 1;
        assert!(v.select_all());
        assert!(!v.all_text && v.selection.is_some_and(|s| s.page == 1), "one page at a time: the page shown");
        let single = thumbnail_info(1);
        let mut w = DocView::new(DocId(2), &single, ViewDefaults::default());
        w.texts.insert(0, Arc::new(one_line("abc")));
        assert!(w.select_all());
        assert!(!w.all_text && w.selection.is_some(), "a one-page document: that page");
        // Text read for display is used; pages that failed aren't asked for again.
        let mut d = DocView::new(DocId(3), &info, ViewDefaults::default());
        d.texts.insert(1, Arc::new(one_line("abc")));
        d.text_failed.insert(2);
        assert!(d.select_all() && d.all_text);
        assert_eq!(d.pages_without_text(), vec![0]);
    }

    #[test]
    fn text_layers_keep_the_selection_page() {
        let info = thumbnail_info(100);
        let mut v = DocView::new(DocId(1), &info, ViewDefaults::default());
        let text = PageText {
            glyphs: vec![pdfcraft_render::TextGlyph { text: "x".into(), rect: [0.0, 0.0, 10.0, 10.0], quad: [[0.0, 0.0]; 4], direction: [1.0, 0.0] }],
            line_of: vec![0],
            space_before: vec![false],
        };
        for p in 0..100 {
            v.texts.insert(p, Arc::new(text.clone()));
        }
        v.select_text(0, 0, 0);
        v.current = 90;
        v.frame_visible = HashSet::from([90, 91]);
        let _ = v.prepare_render_queue(&info, 1.0);
        assert_eq!(v.selected_text().as_deref(), Some("x"), "stale selections still copy");
    }

    #[test]
    fn text_layers_stay_while_a_search_is_active() {
        let info = thumbnail_info(100);
        let mut v = DocView::new(DocId(1), &info, ViewDefaults::default());
        v.current = 50;
        v.frame_visible = HashSet::from([50]);
        for p in 0..100 {
            v.texts.insert(p, Arc::new(PageText::default()));
        }
        v.find = Some(Find { query: "dup".into(), case_query: "dup".into(), matches: vec![(0, 0..1)], ..Default::default() });
        let _ = v.prepare_render_queue(&info, 1.0);
        assert_eq!(v.texts.len(), 100, "evicting mid-search would churn the workers");
    }

    fn thumbnail_info(count: usize) -> DocInfo {
        DocInfo {
            pages: (0..count)
                .map(|i| {
                    let (width, height) = match i % 3 {
                        0 => (612.0, 792.0),
                        1 => (200.0, 4000.0),
                        _ => (4000.0, 200.0),
                    };
                    pdfcraft_render::PageInfo { width, height, label: (i + 1).to_string(), crop: [0.0, 0.0, width, height], rotation: 0 }
                })
                .collect(),
            ..Default::default()
        }
    }

    #[test]
    fn ten_thousand_pages_only_demand_viewport_and_prefetch_at_both_densities() {
        let info = thumbnail_info(10_000);
        let mut v = DocView::new(DocId(1), &info, ViewDefaults::default());
        let cols = 4;
        let rows = info.pages.len().div_ceil(cols);
        for ppp in [1.0, 2.0] {
            for row in [0, 1234, rows - 1] {
                v.begin_render_frame();
                let range = thumbnail_rows(row as f32 * 250.0, row as f32 * 250.0 + 750.0, 250.0, rows);
                for p in range.start * cols..(range.end * cols).min(info.pages.len()) {
                    v.need_thumbnail(p, p / cols == row);
                }
                let queue = v.prepare_render_queue(&info, ppp);
                assert!(queue.len() <= 5 * cols, "only three visible rows and two prefetch rows");
                assert_eq!(queue.first().unwrap().page / cols, row, "visible cells precede prefetch");
                let bytes: usize = queue
                    .iter()
                    .map(|r| {
                        let p = &info.pages[r.page];
                        let (w, h) = (device_pixels(p.width, r.scale), device_pixels(p.height, r.scale));
                        assert!(w <= (THUMB_W * ppp).ceil() as u32);
                        assert!(h <= (THUMB_H * ppp).ceil() as u32);
                        rgba_bytes(w as usize, h as usize)
                    })
                    .sum();
                assert!(bytes <= THUMB_BYTES);
                assert_eq!(queue, v.prepare_render_queue(&info, ppp), "unchanged demand reconstructs the same work");
            }
        }
    }

    #[test]
    fn oversized_thumbnail_viewport_scales_all_admitted_cells_to_one_budget() {
        let info = thumbnail_info(10_000);
        let mut v = DocView::new(DocId(1), &info, ViewDefaults::default());
        for page in 0..MAX_THUMB_DEMAND {
            v.need_thumbnail(page, true);
        }
        let queue = v.prepare_render_queue(&info, 2.0);
        assert_eq!(queue.len(), MAX_THUMB_DEMAND);
        let bytes: usize = queue
            .iter()
            .map(|r| {
                let p = &info.pages[r.page];
                rgba_bytes(device_pixels(p.width, r.scale) as usize, device_pixels(p.height, r.scale) as usize)
            })
            .sum();
        assert!(bytes <= THUMB_BYTES);
    }

    fn solid_result(request: RenderRequest, w: u32, h: u32) -> pdfcraft_render::RenderedPage {
        pdfcraft_render::RenderedPage {
            request,
            width: w,
            height: h,
            rgba: pdfcraft_render::Pixels::from(vec![u32::MAX; rgba_bytes(w as usize, h as usize) / 4]),
            text: None,
            error: None,
            warnings: Vec::new(),
            millis: 0,
        }
    }

    /// #3149: the replacement dialog commits after the canvas collected this frame's demand.
    #[test]
    fn replacing_a_page_after_layout_never_uploads_an_oversized_texture() {
        let ctx = egui::Context::default();
        ctx.input_mut(|i| i.max_texture_side = 2048);
        let mut app = PdfCraftApp::new();
        let original = app.session.create_blank(200.0, 300.0, 1).unwrap();
        let replacement = app.session.create_blank(400.0, 500.0, 1).unwrap();
        app.open_bytes("original.pdf", None, original.as_ref().clone()).unwrap();
        let request = RenderRequest { scale: 4.7, tag: scale_tag(4.7), ..Default::default() };
        app.views[0].frame_queue = vec![request];
        app.views[0].frame_visible.insert(0);
        assert!(app.apply_edit(Edit::ReplacePages { pages: vec![0], name: "replacement.pdf".into(), bytes: replacement, src_pages: vec![0] }));
        let doc = app.session.get(app.views[0].id).unwrap();
        assert_eq!((doc.info.pages[0].width, doc.info.pages[0].height), (400.0, 500.0));
        let edited_bytes = doc.bytes.clone();
        let pool = RenderPool::new_inline(doc.display.clone(), Default::default());
        app.views[0].finish_render_frame(&ctx, &doc.info, &pool);
        assert!(app.views[0].last_queue.is_empty(), "old geometry must not reach the replacement renderer");

        // The next layout can render a bounded backdrop and sharp tiles at the same zoom.
        let backdrop = RenderRequest { scale: 2048.0 / 500.0, tag: scale_tag(2048.0 / 500.0), ..request };
        let tile = RenderRequest { tile: Some(Tile { x: 0, y: 0, w: 1024, h: 1024 }), ..request };
        for _ in 0..2 {
            let view = &mut app.views[0];
            view.begin_render_frame();
            view.frame_visible.insert(0);
            view.frame_queue = vec![backdrop, tile];
            view.finish_render_frame(&ctx, &doc.info, &pool);
        }
        let view = &app.views[0];
        assert_eq!(view.pages.get(&0).unwrap().tex.size(), [1639, 2048]);
        assert_eq!(view.tiles.get(&(0, tile.tag, 0, 0)).unwrap().size(), [1024, 1024]);
        assert!(view.errors.is_empty());
        assert_eq!(doc.bytes, edited_bytes, "rendering preserves the unsaved edit");
        assert!(doc.dirty);
        assert_eq!(doc.can_undo(), Some("Replace page"));
    }

    #[test]
    fn oversized_raster_results_do_not_replace_cached_textures() {
        let ctx = egui::Context::default();
        ctx.input_mut(|i| i.max_texture_side = 64);
        for request in [
            RenderRequest::default(),
            RenderRequest { tag: THUMB_TAG, ..Default::default() },
            RenderRequest { tag: GRID_TAG, ..Default::default() },
            RenderRequest { tag: PRINT_TAG, ..Default::default() },
            RenderRequest { tile: Some(Tile { x: 0, y: 0, w: 64, h: 32 }), ..Default::default() },
        ] {
            let mut view = DocView::new(DocId(1), &thumbnail_info(1), ViewDefaults::default());
            view.last_queue = vec![request];
            let texture = |v: &DocView| {
                v.pages
                    .values()
                    .map(|p| &p.tex)
                    .chain(v.thumbs.values().map(|p| &p.tex))
                    .chain(v.grid_pages.values().map(|(_, t)| t))
                    .chain(v.print_pages.values().map(|(_, t)| t))
                    .chain(v.tiles.values())
                    .map(|t| (t.id(), t.size()))
                    .collect::<Vec<_>>()
            };
            assert_eq!(view.receive_result(&ctx, solid_result(request, 64, 32)), 64 * 32 * 4);
            let cached = texture(&view);
            assert_eq!(cached.len(), 1);
            for (w, h) in [(65, 32), (32, 65), (65, 65), (0, 32), (32, 0)] {
                assert_eq!(view.receive_result(&ctx, solid_result(request, w, h)), 0, "{request:?}: {w}x{h}");
                assert_eq!(texture(&view), cached, "an invalid result must keep the previous raster");
            }
            assert_eq!(view.receive_result(&ctx, solid_result(request, 32, 64)), 32 * 64 * 4);
            assert_eq!(texture(&view)[0].1, [32, 64], "valid results still make progress");
        }
    }

    /// A print preview carries the renderer's print bit; the bit that asks for luminance is
    /// its own, so a colour preview is not converted to monochrome and the two modes are told
    /// apart by their tags.
    #[test]
    fn print_preview_tag_distinguishes_grayscale_from_colour() {
        let mut v = view(1, PageLayout::Single, true);
        v.set_print_grayscale(false);
        v.queue_print_previews(&[(0, 1.0)]);
        let colour = v.frame_queue.last().expect("a print preview was queued").tag;
        assert_ne!(colour & PRINT_TAG, 0, "a print preview carries the renderer's print bit");
        assert_eq!(colour & GRAYSCALE_TAG, 0, "a colour print preview must not ask for luminance");

        v.set_print_grayscale(true);
        v.queue_print_previews(&[(0, 1.0)]);
        let gray = v.frame_queue.last().expect("a grayscale print preview was queued").tag;
        assert_ne!(gray & GRAYSCALE_TAG, 0, "a grayscale print preview asks for luminance");
        assert_ne!(gray, colour, "the two print preview modes must not share a tag");
    }

    #[test]
    fn thumbnails_release_on_scroll_panel_close_and_request_again_after_eviction() {
        let ctx = egui::Context::default();
        let info = thumbnail_info(10_000);
        let mut v = DocView::new(DocId(1), &info, ViewDefaults::default());
        v.need_thumbnail(0, true);
        let first = v.prepare_render_queue(&info, 1.0);
        v.last_queue = first.clone();
        assert_eq!(v.receive_result(&ctx, solid_result(first[0], 132, 171)), 132 * 171 * 4);
        assert!(v.prepare_render_queue(&info, 1.0).is_empty());
        v.begin_render_frame();
        v.need_thumbnail(9999, true);
        v.prepare_render_queue(&info, 1.0);
        assert!(v.thumbs.is_empty(), "jump releases old handles");
        v.begin_render_frame();
        v.need_thumbnail(0, true);
        assert_eq!(v.prepare_render_queue(&info, 1.0), first, "an evicted result must be requested again");
        v.begin_render_frame();
        assert!(v.prepare_render_queue(&info, 1.0).is_empty(), "closing consumers cancels thumbnail work");
    }

    #[test]
    fn stale_results_are_rejected_before_conversion_after_dpi_invalidation_and_suspend() {
        let ctx = egui::Context::default();
        let info = thumbnail_info(1);
        let mut v = DocView::new(DocId(1), &info, ViewDefaults::default());
        v.need_thumbnail(0, true);
        let first = v.prepare_render_queue(&info, 1.0)[0];
        v.last_queue = v.prepare_render_queue(&info, 2.0);
        // Valid pixels: only the request gate can keep these out (a size check can't).
        let obsolete = || solid_result(first, 132, 171);
        assert_eq!(v.receive_result(&ctx, obsolete()), 0);
        assert!(v.thumbs.is_empty(), "a result for the old DPI is not uploaded");
        // The same result is uploaded while its request is current.
        v.last_queue = vec![first];
        assert_eq!(v.receive_result(&ctx, obsolete()), 132 * 171 * 4);
        v.thumbs.clear();
        v.invalidate_content();
        assert_eq!(v.receive_result(&ctx, obsolete()), 0);
        assert!(v.thumbs.is_empty(), "nor after the content changed");
        let pool = RenderPool::new_inline(Arc::new(Vec::new()), Default::default());
        v.last_queue = vec![first];
        v.suspend_rendering(&pool);
        assert_eq!(v.receive_result(&ctx, obsolete()), 0);
        assert!(v.thumbs.is_empty(), "nor after the tab was suspended");
    }

    #[test]
    fn editing_after_layout_preserves_stale_page_until_replacement() {
        let ctx = egui::Context::default();
        let info = thumbnail_info(1);
        let mut v = DocView::new(DocId(1), &info, ViewDefaults::default());
        let req = RenderRequest { scale: 1.0, tag: 1000, ..Default::default() };
        v.last_queue = vec![req];
        v.receive_result(&ctx, solid_result(req, 10, 10));
        v.frame_queue = vec![req];
        v.page_changed(0);
        assert_eq!(v.prepare_render_queue(&info, 1.0), vec![req]);
        assert_eq!(v.pages.get(&0).unwrap().tag, STALE_TAG);
    }

    /// #1858: a released drawing stays painted from its points until a raster that has it
    /// arrives. A page raster that lands before the drawing's edit is applied (in full screen the
    /// edit waits for the next frame, and a render already under way can finish first) is of the
    /// document without it, and must not end the preview.
    #[test]
    fn a_raster_from_before_a_drawing_is_committed_keeps_it_drawn() {
        let ctx = egui::Context::default();
        let info = thumbnail_info(1);
        let mut v = DocView::new(DocId(1), &info, ViewDefaults::default());
        let req = RenderRequest { scale: 1.0, tag: 1000, ..Default::default() };
        v.comments.settle(0.0, 0, crate::comments::CommentTool::Ink, &[[10.0, 10.0], [40.0, 30.0]]);
        v.last_queue = vec![req];
        assert!(v.receive_result(&ctx, solid_result(req, 10, 10)) > 0, "the raster is taken");
        assert_eq!(v.comments.settling_len(), 1, "a raster from before the commit doesn't have the stroke");
        v.comments.drawings_committed();
        v.last_queue = vec![req];
        assert!(v.receive_result(&ctx, solid_result(req, 10, 10)) > 0);
        assert_eq!(v.comments.settling_len(), 0, "a raster after the commit draws it");
        // An edit that fails leaves nothing to wait for.
        v.comments.settle(0.0, 0, crate::comments::CommentTool::Ink, &[[10.0, 10.0], [40.0, 30.0]]);
        v.comments.drawings_refused();
        assert_eq!(v.comments.settling_len(), 0, "a refused drawing isn't left painted");
    }

    #[test]
    fn page_and_tile_admission_accounts_for_bytes_before_count_limits() {
        let mut info = thumbnail_info(40);
        for p in &mut info.pages {
            p.width = 4000.0;
            p.height = 4000.0;
        }
        let mut v = DocView::new(DocId(1), &info, ViewDefaults::default());
        v.frame_queue = (0..40).map(|page| RenderRequest { page, scale: 1.0, tag: 1000, ..Default::default() }).collect();
        let queue = v.prepare_render_queue(&info, 1.0);
        assert_eq!(queue.len(), 2, "128 MiB admits two 4000-square pages, not 24");
        v.frame_queue = (0..40)
            .map(|page| RenderRequest { page, scale: 1.0, tag: 1000, tile: Some(Tile { x: 0, y: 0, w: 1024, h: 1024 }), ..Default::default() })
            .collect();
        assert_eq!(v.prepare_render_queue(&info, 1.0).len(), 32);
        assert!(!v.admit_texture(RenderRequest::default(), PAGE_BYTES + 1));
    }

    #[test]
    fn on_screen_pages_and_tiles_are_admitted_beyond_the_prefetch_allowance() {
        let ctx = egui::Context::default();
        let mut info = thumbnail_info(40);
        for p in &mut info.pages {
            p.width = 2000.0;
            p.height = 2000.0;
        }
        let mut v = DocView::new(DocId(1), &info, ViewDefaults::default());
        // Ten 16 MB pages on screen (a zoomed-out HiDPI view) exceed the 128 MiB allowance.
        v.frame_visible = (0..10).collect();
        v.frame_queue = (0..40).map(|page| RenderRequest { page, scale: 1.0, tag: 1000, ..Default::default() }).collect();
        let queue = v.prepare_render_queue(&info, 1.0);
        let pages: Vec<usize> = queue.iter().map(|r| r.page).collect();
        assert_eq!(pages, (0..10).collect::<Vec<_>>(), "every on-screen page, and no prefetch past the allowance");
        v.last_queue = queue.clone();
        for req in &queue {
            assert!(v.receive_result(&ctx, solid_result(*req, 2000, 2000)) > 0, "on-screen page {} uploaded", req.page);
        }
        assert_eq!(v.pages.len(), 10, "none evicted another on-screen page");
        // A 5K viewport needs more 1024-pixel tiles than the tile allowance holds.
        v.frame_queue = (0..40)
            .map(|x| RenderRequest { page: 0, scale: 1.0, tag: 1000, tile: Some(Tile { x: x * TILE, y: 0, w: 1024, h: 1024 }), ..Default::default() })
            .collect();
        assert_eq!(v.prepare_render_queue(&info, 1.0).len(), 40);
    }

    #[test]
    fn a_page_turned_back_to_is_still_cached() {
        let ctx = egui::Context::default();
        let info = thumbnail_info(10);
        let mut v = DocView::new(DocId(1), &info, ViewDefaults::default());
        let req = |page| RenderRequest { page, scale: 1.0, tag: 1000, ..Default::default() };
        v.frame_visible = [0].into();
        v.frame_queue = vec![req(0)];
        v.last_queue = v.prepare_render_queue(&info, 1.0);
        v.receive_result(&ctx, solid_result(req(0), 100, 130));
        // Single-page view moves on to page 1: page 0 is no longer demanded.
        v.begin_render_frame();
        v.current = 1;
        v.frame_visible = [1].into();
        v.frame_queue = vec![req(1)];
        assert_eq!(v.prepare_render_queue(&info, 1.0), vec![req(1)]);
        assert!(v.pages.contains_key(&0), "kept while the allowance has room");
        // And back: nothing to render.
        v.begin_render_frame();
        v.current = 0;
        v.frame_visible = [0].into();
        v.frame_queue = vec![req(0)];
        assert!(v.prepare_render_queue(&info, 1.0).is_empty());
    }

    #[test]
    fn visible_pages_render_before_thumbnails_and_text() {
        let ctx = egui::Context::default();
        let info = thumbnail_info(4);
        let mut v = DocView::new(DocId(1), &info, ViewDefaults::default());
        let main = RenderRequest { page: 0, scale: 1.0, tag: 1000, ..Default::default() };
        let text = RenderRequest { page: 0, kind: RequestKind::Text, scale: 1.0, tag: TEXT_TAG, ..Default::default() };
        // Opening page 0 queues its pixels and text alongside Pages-panel thumbnails.
        v.begin_render_frame();
        v.frame_visible = [0].into();
        v.frame_queue = vec![main, text];
        for p in 1..4 {
            v.need_thumbnail(p, true);
        }
        let queue = v.prepare_render_queue(&info, 1.0);
        assert_eq!(queue, vec![main], "visible pixels first: no thumbnails or text yet");
        // Once the visible raster arrives, background work is admitted.
        v.last_queue = queue;
        assert!(v.receive_result(&ctx, solid_result(main, 100, 130)) > 0);
        v.begin_render_frame();
        v.frame_visible = [0].into();
        v.frame_queue = vec![text];
        for p in 1..4 {
            v.need_thumbnail(p, true);
        }
        let queue = v.prepare_render_queue(&info, 1.0);
        assert!(queue.iter().any(|r| r.kind == RequestKind::Text), "text admitted after pixels");
        assert!(queue.iter().any(|r| r.tag & THUMB_TAG != 0), "thumbnails admitted after pixels");
        assert!(!queue.iter().any(|r| r.kind == RequestKind::Pixels && r.tag & THUMB_TAG == 0), "no main pixels re-requested");
        // An errored visible page never holds background work back.
        v.errors.insert(0, "gone".to_string());
        v.begin_render_frame();
        v.frame_visible = [0].into();
        v.frame_queue = vec![main, text];
        for p in 1..4 {
            v.need_thumbnail(p, true);
        }
        let queue = v.prepare_render_queue(&info, 1.0);
        assert!(queue.iter().any(|r| r.kind == RequestKind::Text), "text admitted despite the error");
        assert!(queue.iter().any(|r| r.tag & THUMB_TAG != 0), "thumbnails still admitted");
    }

    #[test]
    fn thumbnail_tags_do_not_change_as_a_row_scrolls_in_or_out() {
        let info = thumbnail_info(10_000);
        let mut v = DocView::new(DocId(1), &info, ViewDefaults::default());
        let tags = |v: &mut DocView, cells: usize| {
            v.begin_render_frame();
            for page in 0..cells {
                v.need_thumbnail(page, true);
            }
            v.thumbnail_requests(&info, 2.0).into_iter().map(|r| (r.page, r.tag)).collect::<HashMap<_, _>>()
        };
        // 2x on a large screen: about 70 cells, give or take a row of six.
        let (a, b) = (tags(&mut v, 70), tags(&mut v, 76));
        assert!((0..70).all(|p| a[&p] == b[&p]), "a row more or less re-renders nothing");
    }

    #[test]
    fn many_open_documents_share_one_raster_allowance_and_close_releases_handles() {
        let ctx = egui::Context::default();
        let mut app = PdfCraftApp::new();
        let bytes = app.session.create_blank(612.0, 792.0, 1).unwrap();
        let mut expected = Vec::new();
        for index in 0..6 {
            app.open_bytes(&format!("synthetic-{index}.pdf"), None, bytes.as_ref().clone()).unwrap();
            let info = app.session.get(app.views.last().unwrap().id).unwrap().info.clone();
            let view = app.views.last_mut().unwrap();
            view.begin_render_frame();
            view.need_thumbnail(0, true);
            let request = view.thumbnail_requests(&info, ctx.pixels_per_point()).into_iter().next().unwrap();
            let page = info.pages.get(request.page).unwrap();
            let dimensions = (device_pixels(page.width, request.scale), device_pixels(page.height, request.scale));
            view.last_queue = vec![request];
            assert_eq!(
                view.receive_result(&ctx, solid_result(request, dimensions.0, dimensions.1)),
                rgba_bytes(dimensions.0 as usize, dimensions.1 as usize)
            );
            assert_eq!(view.thumbs.get(&0).unwrap().tag, request.tag);
            assert_eq!(view.thumbs.get(&0).unwrap().tex.size(), [dimensions.0 as usize, dimensions.1 as usize]);

            view.begin_render_frame();
            view.need_thumbnail(0, true);
            let queue = view.prepare_render_queue(&info, ctx.pixels_per_point());
            assert!(queue.is_empty(), "the synthetic thumbnail satisfies current demand");
            view.last_queue = queue;
            expected.push((request, dimensions));
        }
        assert_eq!(app.views.len(), 6, "all six documents were opened");
        assert_eq!(app.active_ids().map(|(index, _)| index), Some(5));
        assert!(app.views.iter().zip(&expected).all(|(view, (request, dimensions))| {
            view.thumbs.len() == 1
                && view
                    .thumbs
                    .get(&0)
                    .is_some_and(|thumb| thumb.tag == request.tag && thumb.tex.size() == [dimensions.0 as usize, dimensions.1 as usize])
        }));

        app.finish_render_frame(&ctx);
        let total: usize = app.views.iter().map(|v| v.thumbs.values().map(|p| texture_bytes(&p.tex)).sum::<usize>()).sum();
        let (active_request, active_dimensions) = expected.last().unwrap();
        let active_bytes = rgba_bytes(active_dimensions.0 as usize, active_dimensions.1 as usize);
        assert_eq!(total, active_bytes, "only the active view retains a thumbnail");
        assert!(total <= THUMB_BYTES, "retained thumbnails stay within the existing allowance");
        assert!(app.views.iter().take(5).all(|v| v.thumbs.is_empty() && v.last_queue.is_empty()));
        let active = app.views.last().unwrap();
        assert_eq!(active.thumbs.len(), 1);
        assert_eq!(active.thumbs.get(&0).unwrap().tag, active_request.tag);
        assert!(active.last_queue.is_empty(), "the current thumbnail is already cached");
        for view in &app.views {
            let doc = app.session.get(view.id).unwrap();
            assert_eq!(doc.renderer.stats().render_requests, 0, "the fixture must not launch a native render");
        }

        app.close_tab(5);
        assert_eq!(app.views.len(), 5, "closing the active tab drops its owning view");
        assert!(app.views.iter().all(|v| v.thumbs.is_empty()));
    }

    #[test]
    fn suspended_signature_preview_releases_layers_and_rebuilds_without_editing() {
        let ctx = egui::Context::default();
        let mut png = std::io::Cursor::new(Vec::new());
        image::RgbaImage::from_pixel(12, 4, image::Rgba([20, 30, 40, 255])).write_to(&mut png, image::ImageFormat::Png).unwrap();
        let image = pdfcraft_engine::SignatureImage::from_bytes(png.get_ref()).unwrap();
        let mut session = pdfcraft_engine::Session::new();
        let blank = session.create_blank(300.0, 400.0, 1).unwrap();
        let id = session.open_new("signature-preview.pdf", blank).unwrap();
        let info = session.get(id).unwrap().info.pages[0].clone();
        session.apply(id, image.edit(0, &info, [20.0, 100.0], false, "Test").unwrap()).unwrap();
        let doc = session.get(id).unwrap();
        let bytes = doc.bytes.clone();
        let generation = doc.edit_generation();
        let mut v = DocView::new(id, &doc.info, ViewDefaults::default());
        v.comments.selected = Some((0, 0));
        v.signature_drag.prepare(&ctx, doc, v.comments.selected, 1.0);
        assert!(v.signature_drag.contains(0, 0));

        v.suspend_rendering(&doc.renderer);
        assert!(!v.signature_drag.contains(0, 0), "inactive tabs release the preview pool and textures");
        assert_eq!(v.comments.selected, Some((0, 0)), "the user's selection survives suspension");
        v.signature_drag.prepare(&ctx, doc, v.comments.selected, 1.0);
        assert!(v.signature_drag.contains(0, 0), "reactivating the view rebuilds the selected image's preview");
        assert!(Arc::ptr_eq(&bytes, &doc.bytes));
        assert_eq!(doc.edit_generation(), generation);
        assert_eq!(doc.can_undo(), Some("Add signature"));
        assert!(doc.dirty);
    }

    #[test]
    fn suspending_a_view_releases_owned_raster_textures_and_queue_state() {
        let ctx = egui::Context::default();
        let mut view = view(2, PageLayout::Single, true);
        let texture = |name| ctx.load_texture(name, egui::ColorImage::filled([1, 1], Color32::WHITE), TextureOptions::LINEAR);
        let request = RenderRequest { page: 0, scale: 1.0, tag: 1, ..Default::default() };
        view.pages.insert(0, PageTex { tag: request.tag, tex: texture("suspended-page") });
        view.thumbs.insert(1, PageTex { tag: THUMB_TAG, tex: texture("suspended-thumbnail") });
        view.tiles.insert((0, request.tag, 0, 0), texture("suspended-tile"));
        view.grid_pages.insert(1, (1, texture("suspended-grid-page")));
        view.print_pages.insert(1, (PRINT_TAG, texture("suspended-print-page")));
        view.stale_thumbs.insert(1);
        view.waiting_since.insert(0, 1.0);
        view.last_queue = vec![request];
        view.frame_queue = vec![request];
        view.frame_visible.insert(0);
        view.need_thumbnail(1, true);

        let pool = RenderPool::new_inline(Arc::new(Vec::new()), Default::default());
        view.suspend_rendering(&pool);

        assert!(view.pages.is_empty(), "page handles are released");
        assert!(view.thumbs.is_empty(), "thumbnail handles are released");
        assert!(view.tiles.is_empty(), "tile handles are released");
        assert!(view.grid_pages.is_empty(), "organize-grid handles are released");
        assert!(view.print_pages.is_empty(), "print-preview handles are released");
        assert!(view.stale_thumbs.is_empty(), "stale thumbnail bookkeeping is cleared");
        assert!(view.waiting_since.is_empty(), "waiting bookkeeping is cleared");
        assert!(view.last_queue.is_empty() && !view.render_pending(), "queued demand is retired");
        assert!(view.frame_queue.is_empty() && view.frame_visible.is_empty() && view.thumb_demand.is_empty());
    }

    /// Edit text & images keeps its boxes while page input is blocked (the window unfocused, a
    /// dialog or the palette open): the page's paragraphs are still measured for the latest edit
    /// and drawn, instead of vanishing until the pointer comes back.
    #[test]
    fn edit_text_boxes_stay_drawn_while_page_input_is_blocked() {
        let mut h = egui_kittest::Harness::builder().with_size(vec2(1400.0, 900.0)).build_eframe(|_cc| {
            let mut app = PdfCraftApp::new();
            let bytes = app.session.create_blank(300.0, 400.0, 1).unwrap();
            app.open_bytes("blank.pdf", None, bytes.as_ref().clone()).unwrap();
            app.quick_tool = QuickTool::EditText;
            app
        });
        h.run_steps(3);
        // Block page input the way the window losing focus does.
        h.state_mut().palette_open = true;
        h.run_steps(2);
        assert!(h.state_mut().apply_edit(Edit::RotatePages { pages: vec![0], degrees: 90 }));
        h.run_steps(3);
        let app = h.state();
        let generation = app.session.get(app.views[0].id).unwrap().edit_generation();
        assert_eq!(app.views[0].edit_lines.get(&0).map(|(g, _)| *g), Some(generation), "the boxes follow the edit while input is blocked");
    }

    #[test]
    fn virtualized_organizer_keeps_insert_gaps_at_document_positions() {
        use egui_kittest::kittest::Queryable;

        let mut h = egui_kittest::Harness::builder().with_size(vec2(1400.0, 900.0)).build_eframe(|_cc| {
            let mut app = PdfCraftApp::new();
            let bytes = app.session.create_blank(300.0, 400.0, 100).unwrap();
            app.open_bytes("organize-gaps.pdf", None, bytes.as_ref().clone()).unwrap();
            app.views[0].organize = true;
            app
        });
        h.run_steps(4);
        assert!(h.state().views[0].thumb_demand.len() < 50);
        assert_eq!(h.query_all_by_label("Insert a file at the end").count(), 0, "the last virtualized cell is not the final page");
        let center = h.state().views[0].viewport_rect().center();
        h.hover_at(center);
        h.event(egui::Event::MouseWheel {
            unit: egui::MouseWheelUnit::Point,
            delta: vec2(0.0, -100_000.0),
            phase: egui::TouchPhase::Move,
            modifiers: egui::Modifiers::NONE,
        });
        // Well past egui's longest scroll animation (about 0.3 s).
        h.run_steps(40);
        h.get_by_label("Insert a file at the end");
        let before_last = h.get_by_label("Insert a file before page 100").rect().center();
        h.hover_at(before_last);
        h.run_steps(2);
        assert_eq!(h.state().views[0].grid_gap, Some(99), "external drops use the document index after virtualized scrolling");
        assert!(h.state().views[0].thumb_demand.len() < 50);
    }

    #[test]
    fn real_shell_pages_organizer_and_print_only_collect_current_thumbnail_consumers() {
        let mut h = egui_kittest::Harness::builder().with_size(vec2(1400.0, 900.0)).build_eframe(|_cc| {
            let mut app = PdfCraftApp::new();
            let bytes = app.session.create_blank(612.0, 792.0, 10_000).unwrap();
            app.open_bytes("synthetic-ten-thousand.pdf", None, bytes.as_ref().clone()).unwrap();
            app.right = Some(RightPanel::Pages);
            app
        });
        h.run_steps(4);
        assert!((1..20).contains(&h.state().views[0].thumb_demand.len()));
        h.state_mut().right = None;
        h.state_mut().views[0].organize = true;
        h.run_steps(2);
        assert!((1..50).contains(&h.state().views[0].thumb_demand.len()));
        h.state_mut().views[0].organize = false;
        h.state_mut().dialog = Some(crate::Dialog::Print);
        h.state_mut().print_draft.sheet = 9999;
        h.run_steps(2);
        assert_eq!(h.state().views[0].thumb_demand.keys().copied().collect::<Vec<_>>(), vec![9999]);
        h.state_mut().print_draft.handling = crate::PrintHandling::Booklet;
        h.state_mut().print_draft.sheet = 0;
        h.run_steps(2);
        assert_eq!(h.state().views[0].thumb_demand.len(), 2, "booklet only needs its two placed pages");
        h.state_mut().dialog = None;
        h.run_steps(2);
        assert!(h.state().views[0].thumb_demand.is_empty());
        assert!(h.state().views[0].thumbs.is_empty());
    }

    /// One mouse-wheel notch (a line) at `at` seconds.
    fn notch(v: &mut DocView, dy: f32, at: f64) -> bool {
        v.single_page_wheel(egui::MouseWheelUnit::Line, dy, egui::TouchPhase::Move, at, true)
    }

    #[test]
    fn thumbnails_cover_the_panel_on_a_high_dpi_screen() {
        // Pages panel slot is at most 150 logical points. At 2 px/pt a letter page's thumbnail
        // must be at least that wide, so the panel samples it down instead of stretching it.
        let info = thumbnail_info(1);
        let mut v = DocView::new(DocId(1), &info, ViewDefaults::default());
        v.need_thumbnail(0, true);
        let queue = v.prepare_render_queue(&info, 2.0);
        let scale = queue.first().expect("one thumbnail").scale;
        assert!(612.0 * scale >= 150.0 * 2.0, "{scale}");
    }

    #[test]
    fn notches_turn_pages_and_stop_at_the_ends() {
        let mut v = view(3, PageLayout::Single, false);
        assert!(notch(&mut v, -1.0, 1.0) && notch(&mut v, -1.0, 1.05));
        assert_eq!(v.current, 2);
        assert!(!notch(&mut v, -1.0, 1.1), "nothing turns past the last page");
        assert!(notch(&mut v, 1.0, 1.2));
        assert_eq!(v.current, 1);
    }

    #[test]
    fn notches_turn_two_page_spreads() {
        // Two-page view, shown one spread at a time (not continuous): a notch turns a whole
        // spread at once, not a single page (#752).
        let mut v = view(4, PageLayout::TwoUp, false);
        assert!(notch(&mut v, -1.0, 1.0));
        assert_eq!(v.current, 2, "a notch turns from the first spread to the second");
        assert!(notch(&mut v, 1.0, 1.1));
        assert_eq!(v.current, 0, "a notch back turns to the previous spread");
    }

    #[test]
    fn continuous_views_ignore_the_wheel_for_paging() {
        // Scrolling views let the ScrollArea handle the wheel; it never turns pages.
        for layout in [PageLayout::Single, PageLayout::TwoUp] {
            let mut v = view(3, layout, true);
            assert!(!notch(&mut v, -1.0, 1.0), "{layout:?} continuous ignores the wheel");
            assert_eq!(v.current, 0);
        }
    }

    #[test]
    fn navigation_drops_partial_wheel_motion() {
        use egui::{MouseWheelUnit::Point, TouchPhase::Move, TouchPhase::Start};
        let mut v = view(3, PageLayout::Single, false);
        assert!(!v.single_page_wheel(Point, 0.0, Start, 1.0, true));
        assert!(!v.single_page_wheel(Point, -30.0, Move, 1.02, true));
        v.go_to_page(1);
        // The same touch has to move a full notch's worth again before it turns.
        assert!(!v.single_page_wheel(Point, -30.0, Move, 1.04, true));
        assert!(v.single_page_wheel(Point, -30.0, Move, 1.06, true));
        assert_eq!(v.current, 2);
    }

    #[test]
    fn reselecting_the_layout_does_not_scroll_to_top() {
        let mut v = view(3, PageLayout::Single, true);
        v.goto = None;
        v.set_layout(PageLayout::Single);
        assert!(v.goto.is_none(), "re-selecting the current layout is a no-op");
        v.set_continuous(true);
        assert!(v.goto.is_none(), "re-selecting the current continuity is a no-op");
        v.set_layout(PageLayout::TwoUp);
        assert_eq!(v.goto, Some((0, 0.0)), "switching the base layout aligns the current page");
        v.goto = None;
        v.set_continuous(false);
        assert_eq!(v.goto, Some((0, 0.0)), "toggling continuous scrolling aligns the current page");
    }

    #[test]
    fn fit_page_and_height_hold_still_while_mixed_page_sizes_scroll_past() {
        let page =
            |width: f32, height: f32| pdfcraft_render::PageInfo { width, height, label: String::new(), crop: [0.0, 0.0, width, height], rotation: 0 };
        let info = DocInfo { pages: vec![page(300.0, 400.0), page(600.0, 800.0)], ..Default::default() };
        for fit in [Fit::Page, Fit::Height] {
            let mut v = DocView::new(DocId(1), &info, ViewDefaults::default());
            (v.fit, v.viewport_w, v.viewport_h) = (fit, 1000.0, 800.0);
            v.fit_zoom(&info);
            let zoom = v.zoom;
            v.current = 1;
            v.fit_zoom(&info);
            assert_eq!(v.zoom, zoom, "{fit:?}: scrolling onto a bigger page keeps the zoom");
            // A single page shown on its own (not scrolling) fits each one.
            (v.layout, v.continuous) = (PageLayout::Single, false);
            v.fit_zoom(&info);
            let big = v.zoom;
            v.current = 0;
            v.fit_zoom(&info);
            assert!(v.zoom > big, "{fit:?}: single-page view fits the page it shows");
        }
    }

    /// Review on #2070: full screen in two-page view fits the spread it shows, the cover page
    /// on its own included; two-page view in the window keeps one zoom for the document.
    #[test]
    fn full_screen_two_page_view_fits_the_spread_it_shows() {
        let page =
            |width: f32, height: f32| pdfcraft_render::PageInfo { width, height, label: String::new(), crop: [0.0, 0.0, width, height], rotation: 0 };
        // Spreads [0, 1] small, [2, 3] large; with a cover page [0], [1, 2] mixed, [3].
        let info = DocInfo { pages: vec![page(300.0, 400.0), page(300.0, 400.0), page(600.0, 800.0), page(600.0, 800.0)], ..Default::default() };
        let mut v = DocView::new(DocId(1), &info, ViewDefaults::default());
        (v.layout, v.continuous, v.viewport_w, v.viewport_h) = (PageLayout::TwoUp, false, 1000.0, 800.0);
        v.fit = Fit::Page;
        v.fit_zoom(&info);
        let windowed = v.zoom;
        v.enter_presentation();
        v.fit_zoom(&info);
        let small = v.zoom;
        assert!(small > windowed * 1.9, "the small spread fills the screen: {small} vs {windowed}");
        v.current = 3;
        v.fit_zoom(&info);
        assert!((v.zoom - windowed).abs() < 1e-4, "the large spread fits as before");
        v.current = 1;
        v.fit_zoom(&info);
        assert_eq!(v.zoom, small, "page 2 is in the first spread");
        // With a cover page: [0] alone, then [1, 2] (the large page decides), then [3].
        v.cover = true;
        v.current = 0;
        v.fit_zoom(&info);
        assert_eq!(v.zoom, small, "the cover page alone");
        v.current = 1;
        v.fit_zoom(&info);
        assert!((v.zoom - windowed).abs() < 1e-4, "page 2 shares a spread with the large page 3");
        // Scrolling while in full screen fits the document, as scrolling does in the window.
        (v.continuous, v.current) = (true, 0);
        v.fit_zoom(&info);
        assert!((v.zoom - windowed).abs() < 1e-4, "continuous full screen keeps one zoom");
        v.continuous = false;
        // The cover page is a spread of its own even when the next page is larger.
        let mixed = DocInfo { pages: vec![page(300.0, 400.0), page(600.0, 800.0), page(600.0, 800.0)], ..Default::default() };
        v.current = 0;
        v.fit_zoom(&mixed);
        assert_eq!(v.zoom, small, "the small cover alone, not with page 2");
        // Leaving full screen brings the document-wide fit back.
        v.leave_presentation();
        (v.fit, v.current) = (Fit::Page, 0);
        (v.layout, v.continuous) = (PageLayout::TwoUp, false);
        v.fit_zoom(&info);
        assert!((v.zoom - windowed).abs() < 1e-4, "in the window, one zoom for the document");
    }

    #[test]
    fn fit_width_fits_the_shown_page_in_single_page_view_only() {
        let page =
            |width: f32, height: f32| pdfcraft_render::PageInfo { width, height, label: String::new(), crop: [0.0, 0.0, width, height], rotation: 0 };
        let info = DocInfo { pages: vec![page(300.0, 400.0), page(600.0, 400.0)], ..Default::default() };
        let mut v = DocView::new(DocId(1), &info, ViewDefaults::default());
        (v.fit, v.viewport_w, v.viewport_h) = (Fit::Width, 1000.0, 800.0);
        v.layout = PageLayout::Single;
        v.continuous = false;
        v.fit_zoom(&info);
        let narrow = v.zoom;
        // The shown page fills the width between the side gutters, not centred for the wide page.
        let shown = v.layout(&info, v.content_width(&info, 1000.0), 1.0)[0];
        assert!((shown.left() - SIDE).abs() < 1e-2 && (shown.width() - 860.0).abs() < 1e-2, "{shown:?}");
        v.current = 1;
        v.fit_zoom(&info);
        assert!((narrow / v.zoom - 2.0).abs() < 1e-3, "the narrow page gets twice the zoom of the wide one");
        // Scrolling views keep one zoom for the whole document.
        v.continuous = true;
        v.fit_zoom(&info);
        let wide = v.zoom;
        v.current = 0;
        v.fit_zoom(&info);
        assert_eq!(v.zoom, wide, "continuous fit width holds still across page sizes");
    }

    #[test]
    fn layouts_match_their_commands_and_names() {
        for l in PageLayout::ORDER {
            let spec = pdfcraft_engine::commands::command(l.command()).expect("registered");
            assert_eq!((l.label(), l.icon()), (spec.label, spec.icon), "{l:?}");
            assert_eq!(PageLayout::from_command(l.command()), Some(l));
            assert_eq!(PageLayout::try_parse(l.as_str()), Some(l));
        }
        assert_eq!(PageLayout::try_parse(" Two-Up "), Some(PageLayout::TwoUp));
        assert_eq!(PageLayout::try_parse("facing"), None);
    }

    #[test]
    fn a_null_xyz_coordinate_keeps_the_readers_place_on_the_new_page() {
        // CAD exports link each cross-reference as `/XYZ null null 0`. Acrobat keeps the
        // point at the window's top-left (and the zoom) when following one to another sheet.
        let info = pdfcraft_render::DocInfo {
            pages: (0..3)
                .map(|_| pdfcraft_render::PageInfo { width: 842.0, height: 595.0, label: String::new(), crop: [0.0, 0.0, 842.0, 595.0], rotation: 0 })
                .collect(),
            ..Default::default()
        };
        // Page 1 at 400 %, scrolled so its point 200 pt across and 125 pt down is at the corner
        // where /XYZ puts its point.
        let zoomed = |rot: u16, size: Vec2| {
            let mut v = view(3, PageLayout::Single, true);
            v.rotation = rot;
            v.viewport_screen = Rect::from_min_size(Pos2::ZERO, vec2(1000.0, 600.0));
            v.screen_xforms = vec![(0, PageXform { rect: Rect::from_min_size(pos2(SIDE - 800.0, -500.0), size), rot, pw: 842.0, ph: 595.0 })];
            v
        };
        let near = |a: Option<f32>, b: f32| a.is_some_and(|a| (a - b).abs() < 1e-4);
        let mut v = zoomed(0, vec2(4.0 * 842.0, 4.0 * 595.0));
        v.go_to_dest(1, DestView::Xyz { left: None, top: None, zoom: None }, &info);
        let (page, fx, fy, rel) = v.goto_point.expect("the place is kept");
        assert_eq!((v.current, page, rel), (1, 1, vec2(SIDE, 0.0)));
        assert!(near(fx, 200.0 / 842.0) && near(fy, 125.0 / 595.0), "{fx:?} {fy:?}");
        // A coordinate the destination gives wins; only the null one is kept.
        let mut v = zoomed(0, vec2(4.0 * 842.0, 4.0 * 595.0));
        v.go_to_dest(2, DestView::Xyz { left: Some(421.0), top: None, zoom: Some(4.0) }, &info);
        let (_, fx, fy, _) = v.goto_point.expect("the place is kept");
        assert!(near(fx, 0.5) && near(fy, 125.0 / 595.0), "{fx:?} {fy:?}");
        // With the view turned a quarter, the point under the corner is still the one kept.
        let mut v = zoomed(90, vec2(4.0 * 595.0, 4.0 * 842.0));
        v.go_to_dest(1, DestView::Xyz { left: None, top: None, zoom: None }, &info);
        let (_, fx, fy, _) = v.goto_point.expect("the place is kept");
        assert!(near(fx, 800.0 / (4.0 * 595.0)) && near(fy, 500.0 / (4.0 * 842.0)), "{fx:?} {fy:?}");
        // A page that fits the window keeps the layout's placement: no point, as before.
        let mut v = zoomed(0, vec2(800.0, 565.0));
        v.go_to_dest(1, DestView::Xyz { left: None, top: None, zoom: None }, &info);
        assert_eq!((v.current, v.goto, v.goto_point), (1, Some((1, 0.0)), None));
    }

    #[test]
    fn destinations_set_the_zoom_mode_and_the_point_to_show() {
        let info = pdfcraft_render::DocInfo {
            pages: (0..3)
                .map(|_| pdfcraft_render::PageInfo { width: 300.0, height: 400.0, label: String::new(), crop: [0.0, 0.0, 300.0, 400.0], rotation: 0 })
                .collect(),
            ..Default::default()
        };
        let mut v = view(3, PageLayout::Single, true);
        v.fit = Fit::Width;
        // No position (or a malformed one): the top of the page, zoom as it was.
        v.go_to_dest(1, DestView::Top, &info);
        assert_eq!((v.current, v.goto, v.goto_point, v.fit), (1, Some((1, 0.0)), None, Fit::Width));
        // /XYZ with a null zoom keeps the zoom mode; its point is a quarter down page 3.
        v.go_to_dest(2, DestView::Xyz { left: None, top: Some(300.0), zoom: None }, &info);
        assert_eq!((v.current, v.fit, v.goto_point), (2, Fit::Width, Some((2, None, Some(0.25), vec2(SIDE, 0.0)))));
        v.go_to_dest(0, DestView::Xyz { left: Some(150.0), top: None, zoom: Some(2.5) }, &info);
        assert_eq!((v.fit, v.zoom, v.goto_point), (Fit::None, 2.5, Some((0, Some(0.5), None, vec2(SIDE, 0.0)))));
        // A zoom beyond the view's range is clamped to it.
        v.go_to_dest(0, DestView::Xyz { left: None, top: None, zoom: Some(1e30) }, &info);
        assert_eq!((v.zoom, v.goto_point), (64.0, None));
        v.go_to_dest(0, DestView::Fit, &info);
        assert_eq!((v.fit, v.goto_point), (Fit::Page, None));
        v.go_to_dest(0, DestView::FitH { top: Some(100.0) }, &info);
        assert_eq!((v.fit, v.goto_point), (Fit::Width, Some((0, None, Some(0.75), vec2(SIDE, 0.0)))));
        v.go_to_dest(0, DestView::FitV { left: Some(75.0) }, &info);
        assert_eq!((v.fit, v.goto_point), (Fit::Height, Some((0, Some(0.25), None, vec2(SIDE, 0.0)))));
        // /FitR: the zoom that fits a 100×50 pt rectangle in an 800×600 window, centred.
        (v.viewport_w, v.viewport_h) = (800.0, 600.0);
        v.go_to_dest(0, DestView::FitR { rect: [0.0, 300.0, 100.0, 350.0] }, &info);
        assert_eq!(v.fit, Fit::None);
        assert!((v.zoom - 8.0 / PT).abs() < 1e-4, "{}", v.zoom);
        let (page, fx, fy, rel) = v.goto_point.expect("a point to show");
        assert_eq!((page, rel), (0, vec2(400.0, 300.0)));
        assert!((fx.unwrap_or(-1.0) - 50.0 / 300.0).abs() < 1e-4 && (fy.unwrap_or(-1.0) - 75.0 / 400.0).abs() < 1e-4, "{fx:?} {fy:?}");
        // A page past the end goes to the last page; a page the document info lacks, to its top.
        v.go_to_dest(usize::MAX, DestView::FitH { top: Some(100.0) }, &info);
        assert_eq!((v.current, v.goto_point.map(|g| g.0)), (2, Some(2)));
        let empty = pdfcraft_render::DocInfo::default();
        v.go_to_dest(1, DestView::FitH { top: Some(100.0) }, &empty);
        assert_eq!((v.current, v.goto_point), (1, None));
    }

    #[test]
    fn page_text_failure_preserves_valid_search_match_without_jitter() {
        let mut v = view(3, PageLayout::Single, true);
        v.current = 1;
        v.find = Some(Find {
            query: "test".into(),
            case_query: "test".into(),
            matches: vec![(0, 0..4), (1, 5..9)],
            current: Some(0),
            ..Default::default()
        });
        let req = RenderRequest { page: 0, kind: RequestKind::Text, ..Default::default() };
        v.last_queue = vec![req];
        let ctx = egui::Context::default();
        v.receive_result(
            &ctx,
            pdfcraft_render::RenderedPage {
                request: req,
                width: 0,
                height: 0,
                rgba: Default::default(),
                error: Some("extraction failed".into()),
                text: None,
                warnings: Vec::new(),
                millis: 0,
            },
        );
        // Active match was on page 0 which failed: current should fall back to match on page 1 without jitter
        assert_eq!(v.find.as_ref().unwrap().current, Some(0));
        assert_eq!(v.find.as_ref().unwrap().matches, vec![(1, 5..9)]);
        assert!(v.goto.is_none());
    }

    /// A page whose picture moved can get its whole-page raster back before its last stale tile.
    /// From then on the raster has the picture where it went: it is drawn from there, not from
    /// where it was (which the raster now shows empty), and another drag starts from there too.
    #[test]
    fn a_moved_picture_is_drawn_from_the_fresh_raster_while_a_tile_is_still_stale() {
        let content = b"q 60 0 0 60 40 260 cm /Im0 Do Q";
        let objs: Vec<Vec<u8>> = vec![
            b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
            b"<< /Type /Pages /Kids [3 0 R] /Count 1 /MediaBox [0 0 300 400] >>".to_vec(),
            b"<< /Type /Page /Parent 2 0 R /Contents 4 0 R /Resources << /XObject << /Im0 5 0 R >> >> >>".to_vec(),
            [format!("<< /Length {} >>\nstream\n", content.len()).as_bytes(), content.as_slice(), b"\nendstream"].concat(),
            [
                b"<< /Type /XObject /Subtype /Image /Width 1 /Height 1 /BitsPerComponent 8 /ColorSpace /DeviceRGB /Length 3 >>\nstream\n".as_slice(),
                &[200, 30, 30],
                b"\nendstream",
            ]
            .concat(),
        ];
        let mut bytes = b"%PDF-1.7\n".to_vec();
        let mut offsets = Vec::new();
        for (i, o) in objs.iter().enumerate() {
            offsets.push(bytes.len());
            bytes.extend_from_slice(format!("{} 0 obj\n", i + 1).as_bytes());
            bytes.extend_from_slice(o);
            bytes.extend_from_slice(b"\nendobj\n");
        }
        let xref = bytes.len();
        bytes.extend_from_slice(format!("xref\n0 {}\n0000000000 65535 f \n", objs.len() + 1).as_bytes());
        for o in offsets {
            bytes.extend_from_slice(format!("{o:010} 00000 n \n").as_bytes());
        }
        bytes.extend_from_slice(format!("trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n", objs.len() + 1).as_bytes());
        let mut h = egui_kittest::Harness::builder().with_size(vec2(1400.0, 1100.0)).build_eframe(move |_cc| {
            let mut app = PdfCraftApp::new();
            app.set_option("language", "en").unwrap();
            app.open_bytes("picture.pdf", None, bytes.clone()).unwrap();
            app.set_option("zoom", "100").unwrap();
            app.quick_tool = QuickTool::EditText;
            app
        });
        let settle = |h: &mut egui_kittest::Harness<'static, PdfCraftApp>| {
            for _ in 0..400 {
                h.step();
                if !h.state().render_pending() {
                    return;
                }
                std::thread::sleep(std::time::Duration::from_millis(5));
            }
        };
        h.run_steps(4);
        settle(&mut h);
        let page = h.state().views[0].page_screen_rect(0).unwrap();
        let at = |x: f32, y: f32| egui::pos2(page.left() + x / 300.0 * page.width(), page.top() + (400.0 - y) / 400.0 * page.height());
        let red = |pixels: &image::RgbaImage, r: Rect| {
            let (w, hh) = (pixels.width(), pixels.height());
            (r.top().max(0.0) as u32..(r.bottom() as u32).min(hh))
                .flat_map(|y| (r.left().max(0.0) as u32..(r.right() as u32).min(w)).map(move |x| (x, y)))
                .filter(|(x, y)| matches!(pixels.get_pixel(*x, *y).0, [r, g, b, _] if r > 150 && g < 90 && b < 90))
                .count()
        };
        let (old, new) = (Rect::from_two_pos(at(42.0, 262.0), at(98.0, 318.0)), Rect::from_two_pos(at(182.0, 82.0), at(238.0, 138.0)));
        // Select the picture and drag it, without overlap, from (40, 260) to (180, 80).
        let drag = |h: &mut egui_kittest::Harness<'static, PdfCraftApp>, from: Pos2, to: Pos2, release: bool| {
            h.hover_at(from);
            h.run_steps(1);
            h.drag_at(from);
            h.run_steps(1);
            for k in 1..=4 {
                h.hover_at(from + (to - from) * (k as f32 / 4.0));
                h.run_steps(1);
            }
            if release {
                h.drop_at(to);
                h.step();
            }
        };
        let (grab, put) = (at(70.0, 290.0), at(210.0, 110.0));
        h.hover_at(grab);
        h.run_steps(1);
        h.drag_at(grab);
        h.run_steps(1);
        h.drop_at(grab);
        h.run_steps(3);
        assert!(h.state().views[0].image_selection.is_some(), "the picture is selected");
        h.state_mut().views[0].hold_rasters = true;
        drag(&mut h, grab, put, true);
        // One tile stays stale, as a tile result arriving after the whole-page raster would; then
        // the whole-page raster is let through.
        let tile = h.ctx.load_texture("stale-tile", egui::ColorImage::new([1, 1], vec![Color32::TRANSPARENT]), TextureOptions::LINEAR);
        h.state_mut().views[0].tiles.insert((0, 1, 0, 0), tile);
        h.state_mut().views[0].stale_tiles.insert((0, 1, 0, 0));
        h.state_mut().views[0].hold_rasters = false;
        for _ in 0..400 {
            h.step();
            if !h.state().views[0].page_raster_out_of_date(0) {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        assert!(!h.state().views[0].page_raster_out_of_date(0), "the whole-page raster is fresh");
        assert!(h.state().views[0].page_out_of_date(0), "a tile is still stale");
        h.run_steps(2);
        let pixels = h.render().unwrap();
        assert!(red(&pixels, new) > 100, "the picture shows where it went");
        assert_eq!(red(&pixels, old), 0, "and not where it was");
        // Another drag in that interval takes its pixels from where the raster has them.
        let further = at(210.0, 200.0);
        drag(&mut h, put, further, false);
        let pixels = h.render().unwrap();
        let moving = Rect::from_two_pos(at(182.0, 172.0), at(238.0, 228.0));
        assert!(red(&pixels, moving) > 100, "the dragged picture is drawn from the fresh raster");
        assert_eq!(red(&pixels, old), 0, "nothing shows where it first was");
        h.state_mut().views[0].stale_tiles.clear();
        h.state_mut().views[0].tiles.clear();
    }
}
