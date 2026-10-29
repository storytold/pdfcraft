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

use crate::page_cursor::Layer;
use crate::page_pointer::PagePointer;
use crate::theme::{self, Tokens};
use crate::{PdfCraftApp, QuickTool, RightPanel, comments, icons, widgets};

mod area;
pub(crate) mod autoscroll;
pub(crate) mod bars;
mod cache;
pub(super) mod crop;
mod find;
pub(super) mod fling;
pub(super) mod image_pick;
mod navigation;
mod organize;
pub(super) mod page_cursor;
mod page_input;
pub(super) mod page_pointer;
pub(super) mod page_router;
pub mod scroll_speed;
mod selection;
pub(super) mod signature_drag;
pub(super) mod wheel_boost;
pub(super) mod wheel_pager;
pub(super) mod wheel_reset;
mod xform;
mod zoom;
pub(super) mod zoom_snap;

pub(crate) use area::signature_badges;
use area::{drawing_edge_pointer, glide_interrupted, page_scroll, presentation_click, run_button, set_layers, zoom_anchor};
#[cfg(test)]
use bars::in_size_hint_zone;
use bars::{Notice, notices, page_size_hint, quick_bar};
pub use cache::RasterMemory;
pub(crate) use cache::{BASE_SIDE, TILE_THRESHOLD, texture_image, thumbnail_rows};
#[cfg(test)]
use cache::{
    GRAYSCALE_TAG, GRID_TAG, MAX_CACHED_TEXT_PAGES, MAX_THUMB_DEMAND, PAGE_BYTES, THUMB_BYTES, THUMB_H, THUMB_TAG, THUMB_W, TILE, grayscale,
    rgba_bytes,
};
use cache::{OLD_TILES, PageTex, STALE_TAG, TEXT_TAG, TILE_BYTES, scale_tag, tag_scale, texture_bytes, tile_side};
pub use find::Find;
use find::find_bar;
pub use navigation::shortcuts;
pub(crate) use organize::drop_gap;
#[cfg(test)]
pub(crate) use organize::grid_page_rect;
use organize::organize_grid;
#[cfg(test)]
use organize::sharp_enough;
pub(crate) use page_input::PageToolContext;
use page_input::{PageInput, cancel_on_escape, page_overlays, page_tool_input, text_layer};
use selection::{Selection, Span, click_unit, selection_actions};
pub use xform::PageXform;
use xform::snap_to_pixels;
use zoom::anchored_scroll;
pub use zoom::{GRID_ZOOM_RANGE, THUMBNAIL_ZOOM_RANGE};

/// Logical pixels per PDF point at 100% (96 dpi, like browsers).
pub const PT: f32 = 96.0 / 72.0;
const GAP: f32 = 14.0;
const MARGIN: f32 = 28.0;
/// Horizontal gutter that keeps pages clear of the floating quick-action bar.
const SIDE: f32 = 70.0;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Fit {
    Width,
    Page,
    Height,
    None,
}

use crate::edit_text_ui::ImageHandle;

/// Extension trait on `egui::Painter` to draw polygons.
pub trait PainterExt {
    fn polygon(&self, points: Vec<Pos2>, fill: Color32, stroke: Stroke) -> egui::layers::ShapeIdx;
}

impl PainterExt for egui::Painter {
    fn polygon(&self, points: Vec<Pos2>, fill: Color32, stroke: Stroke) -> egui::layers::ShapeIdx {
        self.add(egui::Shape::convex_polygon(points, fill, stroke))
    }
}

/// User-space selectable bounding box for an image object.
/// If the image object has an active clipping path applied to it (the crop box),
/// this is the mathematical intersection of the full CTM-derived rect and the clipping path's rect.
pub fn image_selectable_bounds(im: &pdfcraft_engine::PageImage) -> [f64; 4] {
    if let Some(clip) = im.clip_rect {
        let r = [im.rect[0].max(clip[0]), im.rect[1].max(clip[1]), im.rect[2].min(clip[2]), im.rect[3].min(clip[3])];
        if r.iter().all(|v| v.is_finite()) && r[2] > r[0] && r[3] > r[1] {
            return r;
        }
    }
    im.visible_rect.unwrap_or(im.rect)
}

/// Screen-space selectable bounding box for an image object.
/// If the image object has an active clipping path applied to it (the crop box),
/// the final selectable `egui::Rect` is the mathematical intersection of the image's
/// full CTM-derived rect and the clipping path's rect.
pub fn image_screen_rect(im: &pdfcraft_engine::PageImage, xf: &PageXform, info: &DocInfo, page: usize) -> Rect {
    let full = xf.user_rect_f64(info, page, im.rect);
    if let Some(clip) = im.clip_rect {
        let clip_r = xf.user_rect_f64(info, page, clip);
        let inter = full.intersect(clip_r);
        if inter.is_positive() {
            return inter;
        }
    }
    if let Some(vr) = im.visible_rect {
        let r = xf.user_rect_f64(info, page, vr);
        if r.is_positive() {
            return r;
        }
    }
    full
}

/// An Oriented Bounding Box (OBB) representing a rotated rectangle in screen space,
/// with 8 resize handles and rotated local axes.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ImageObb {
    pub s00: Pos2,
    pub s10: Pos2,
    pub s11: Pos2,
    pub s01: Pos2,
    pub s_bc: Pos2,
    pub s_rc: Pos2,
    pub s_tc: Pos2,
    pub s_lc: Pos2,
    pub s_rot: Pos2,
    pub screen_center: Pos2,
    pub u_axis: (f64, f64),
    pub v_axis: (f64, f64),
    pub is_rotated: bool,
}

impl ImageObb {
    pub(crate) fn handle_cursor(&self, handle: ImageHandle) -> egui::CursorIcon {
        let direction = self.handle_pos(handle) - self.screen_center;
        match handle {
            ImageHandle::TopLeft | ImageHandle::TopRight | ImageHandle::BottomLeft | ImageHandle::BottomRight => {
                if direction.x * direction.y > 0.0 {
                    egui::CursorIcon::ResizeNwSe
                } else {
                    egui::CursorIcon::ResizeNeSw
                }
            }
            _ if direction.x.abs() > direction.y.abs() => egui::CursorIcon::ResizeHorizontal,
            _ => egui::CursorIcon::ResizeVertical,
        }
    }

    pub(crate) fn contains(&self, visible: Rect, p: Pos2) -> bool {
        (visible.is_positive() && visible.contains(p)) || crate::edit_text_ui::quad_contains(self.corners(), p)
    }

    pub fn from_matrix(matrix: [f64; 6], xf: &PageXform, info: &DocInfo, page: usize) -> Self {
        let m = crate::edit_text_ui::Matrix(matrix);
        let p00 = m.apply(0.0, 0.0);
        let p10 = m.apply(1.0, 0.0);
        let p11 = m.apply(1.0, 1.0);
        let p01 = m.apply(0.0, 1.0);

        let s00 = crate::edit_text_ui::user_to_screen(xf, info, page, p00.0, p00.1);
        let s10 = crate::edit_text_ui::user_to_screen(xf, info, page, p10.0, p10.1);
        let s11 = crate::edit_text_ui::user_to_screen(xf, info, page, p11.0, p11.1);
        let s01 = crate::edit_text_ui::user_to_screen(xf, info, page, p01.0, p01.1);

        let s_bc = Pos2::new((s00.x + s10.x) * 0.5, (s00.y + s10.y) * 0.5);
        let s_rc = Pos2::new((s10.x + s11.x) * 0.5, (s10.y + s11.y) * 0.5);
        let s_tc = Pos2::new((s01.x + s11.x) * 0.5, (s01.y + s11.y) * 0.5);
        let s_lc = Pos2::new((s00.x + s01.x) * 0.5, (s00.y + s01.y) * 0.5);

        let screen_center = Pos2::new((s00.x + s10.x + s11.x + s01.x) * 0.25, (s00.y + s10.y + s11.y + s01.y) * 0.25);
        let out_dir = s_tc - screen_center;
        let out_len = out_dir.length().max(1e-4);
        let out_norm = out_dir / out_len;
        let s_rot = s_tc + out_norm * 24.0;

        let [a, b, c, d, ..] = matrix;
        let w_u = (a * a + b * b).sqrt().max(1e-6);
        let h_v = (c * c + d * d).sqrt().max(1e-6);
        let u_axis = (a / w_u, b / w_u);
        let v_axis = (c / h_v, d / h_v);
        // The fast rectangle path interprets labels in screen space. Use matrix
        // resizing whenever the image's user-space axes are turned or reflected
        // on screen, including page /Rotate and View > Rotate View.
        let u = s10 - s00;
        let v = s01 - s00;
        let is_rotated = u.y.abs() > 1e-4 || v.x.abs() > 1e-4 || u.x < 0.0 || v.y > 0.0;

        Self { s00, s10, s11, s01, s_bc, s_rc, s_tc, s_lc, s_rot, screen_center, u_axis, v_axis, is_rotated }
    }

    pub fn from_image(im: &pdfcraft_engine::PageImage, xf: &PageXform, info: &DocInfo, page: usize) -> Self {
        let [a, b, c, d, ..] = im.matrix;
        let is_rot = b.abs() > 1e-4 || c.abs() > 1e-4 || a < 0.0 || d < 0.0;
        let bounds = image_selectable_bounds(im);
        let matrix = if !is_rot && bounds[2] > bounds[0] && bounds[3] > bounds[1] {
            [bounds[2] - bounds[0], 0.0, 0.0, bounds[3] - bounds[1], bounds[0], bounds[1]]
        } else {
            im.matrix
        };
        Self::from_matrix(matrix, xf, info, page)
    }

    pub fn handles(&self) -> [(ImageHandle, Pos2); 8] {
        [
            ImageHandle::TopLeft,
            ImageHandle::TopCenter,
            ImageHandle::TopRight,
            ImageHandle::RightCenter,
            ImageHandle::BottomRight,
            ImageHandle::BottomCenter,
            ImageHandle::BottomLeft,
            ImageHandle::LeftCenter,
        ]
        .map(|h| (h, self.handle_pos(h)))
    }

    pub fn handle_pos(&self, handle: ImageHandle) -> Pos2 {
        match handle {
            ImageHandle::TopLeft => self.s01,
            ImageHandle::TopCenter => self.s_tc,
            ImageHandle::TopRight => self.s11,
            ImageHandle::RightCenter => self.s_rc,
            ImageHandle::BottomRight => self.s10,
            ImageHandle::BottomCenter => self.s_bc,
            ImageHandle::BottomLeft => self.s00,
            ImageHandle::LeftCenter => self.s_lc,
        }
    }

    pub fn corners(&self) -> [Pos2; 4] {
        [self.s00, self.s10, self.s11, self.s01]
    }
}

/// Whether pointer `p` is on an image being cropped, its crop box or its handles (#5835).
pub(crate) fn crop_contains_point(
    img: &pdfcraft_engine::PageImage,
    crop: &crate::edit_text_ui::CropMode,
    xf: &PageXform,
    info: &DocInfo,
    page: usize,
    p: Pos2,
) -> bool {
    let obb = ImageObb::from_image(img, xf, info, page);
    let full_obb = ImageObb::from_matrix(img.matrix, xf, info, page);
    crate::edit_text_ui::quad_contains(obb.corners(), p)
        || crate::edit_text_ui::quad_contains(full_obb.corners(), p)
        || crate::edit_text_ui::crop_controls_contain(img, crop, xf, info, page, p)
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
    /// Pages whose whole-page raster this frame asks for, on screen or prefetched. An arriving
    /// raster over the allowance evicts the other cached pages first.
    frame_demanded: HashSet<usize>,
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
    /// Pixels per point last frame, for telling a rounding sliver of a page from a visible one.
    screen_ppp: f32,
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
    /// The document owns the full coarse-wheel distance, including its acceleration.
    wheel_motion: crate::wheel_boost::Motion,
    /// Raw input and retained motion advance once per frame, including discarded UI passes.
    wheel_frame: Option<u64>,
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
    /// Transient keyboard owner for this document's Pages sidebar.
    pub(crate) pages_focus_id: Option<egui::Id>,
    /// Marquee Zoom / Snapshot: the rectangle being dragged (page, start), and a finished one.
    pub marquee: Option<(usize, Pos2)>,
    pub marquee_done: Option<crate::zoom_snap::Marquee>,
    /// Fill & Sign text being typed.
    pub fill_text: Option<crate::fill_sign::TypeBox>,
    /// A queued Fill & Sign signature: select it after its edit succeeds, then leave placement.
    pub(crate) fill_signature_page: Option<usize>,
    pub(crate) signature_drag: crate::signature_drag::SignatureDrag,
    /// What the page modules claim for the cursor this frame; `document_area` sets it once.
    pub(crate) cursor: crate::page_cursor::PageCursor,
    /// Who the press the pages hold belongs to.
    pub(crate) router: crate::page_router::PageRouter,
    /// Escape cancelled the press the pages hold: the page ignores it until the button is up, so
    /// its release neither clicks nor ends a drag.
    pub(crate) press_cancelled: bool,
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
    /// The way the view last moved vertically: 1 down, -1 up, 0 before any move and after the
    /// content's size changes (a zoom or a layout change). A jump to a page or a search result
    /// counts as a move. It stays set while the view is still, so the pages prefetched ahead stay
    /// in the render queue until they arrive.
    scroll_dir: i8,
    /// The content's size when `scroll_dir` was last updated.
    scroll_content: Vec2,
    /// Side-by-side compare: scroll to here this frame, following the other half.
    pub(crate) sync_scroll: Option<Vec2>,
    /// The page widget's id, by which the Hand tool finds its drag before the pages are laid out.
    page_widget: Option<egui::Id>,
    pan_hold_until: f64,
    /// Pages whose raster proved slow (see `SLOW_MILLIS`): only these get the zoom/pan
    /// settle holds. Fast pages render straight through, with no added latency.
    slow_pages: HashSet<usize>,
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

/// Default page right-click menu actions.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum PageMenuAction {
    RotateCCW,
    RotateCW,
    Print,
    SelectTool,
    HandTool,
    MarqueeZoom,
    Copy,
    Paste(usize, [f64; 2]),
    AddComment(usize, [f64; 2]),
    AddBookmark,
    TakeSnapshot,
    EditPdf,
    AddText,
    AddImage,
    DeletePage(usize),
    ExportPdf,
    Find,
    DocProperties,
    FlattenComments,
}

pub(crate) struct PageMenuPerms {
    pub can_modify: bool,
    pub can_assemble: bool,
    pub can_annotate: bool,
    pub can_print: bool,
    pub can_copy: bool,
    pub can_copy_items: bool,
    pub can_paste: bool,
    pub has_annotations: bool,
}

pub(crate) fn default_page_context_menu(ui: &mut egui::Ui, page: usize, at: [f64; 2], perms: PageMenuPerms, action: &mut Option<PageMenuAction>) {
    use PageMenuAction as A;
    // Takes `choice` when `entry` was clicked, and closes the menu.
    let mut pick = |entry: egui::Response, ui: &mut egui::Ui, choice: PageMenuAction| {
        if entry.clicked() {
            *action = Some(choice);
            ui.close();
        }
    };
    let icon = |ui: &mut egui::Ui, name: &str, tip: &str| crate::icons::button(ui, name, 24.0, false, tip);
    // Top horizontal icon bar: Rotate CCW, Print, Select tool, Hand tool, Marquee Zoom
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 4.0;
        pick(icon(ui, "rotate-ccw", tl!("Rotate view counterclockwise")), ui, A::RotateCCW);
        ui.add_enabled_ui(perms.can_print, |ui| pick(icon(ui, "printer", tl!("Print")), ui, A::Print));
        pick(icon(ui, "mouse-pointer-2", tl!("Select Tool")), ui, A::SelectTool);
        pick(icon(ui, "hand", tl!("Hand Tool")), ui, A::HandTool);
        pick(icon(ui, "zoom-in", tl!("Marquee Zoom")), ui, A::MarqueeZoom);
    });

    ui.separator();

    pick(
        ui.add_enabled(
            perms.can_copy_items,
            egui::Button::new(tl!("Copy")).shortcut_text(crate::commands::shortcut_label(ui.ctx(), crate::commands::COPY)),
        ),
        ui,
        A::Copy,
    );
    pick(
        ui.add_enabled(
            perms.can_paste,
            egui::Button::new(tl!("Paste")).shortcut_text(crate::commands::shortcut_label(ui.ctx(), pdfcraft_engine::commands::Shortcut::cmd("V"))),
        ),
        ui,
        A::Paste(page, at),
    );

    ui.separator();

    pick(ui.add_enabled(perms.can_annotate, egui::Button::new(tl!("Add comment"))), ui, A::AddComment(page, at));
    if perms.has_annotations {
        pick(ui.add_enabled(perms.can_modify, egui::Button::new(tl!("Flatten comments"))), ui, A::FlattenComments);
    }
    pick(ui.add_enabled(perms.can_assemble, egui::Button::new(tl!("Add bookmark"))), ui, A::AddBookmark);
    pick(ui.add_enabled(perms.can_copy, egui::Button::new(tl!("Take a snapshot"))), ui, A::TakeSnapshot);

    ui.separator();

    pick(ui.add_enabled(perms.can_modify, egui::Button::new(egui::RichText::new(tl!("Edit this PDF")).strong())), ui, A::EditPdf);
    pick(ui.add_enabled(perms.can_modify, egui::Button::new(tl!("Add text"))), ui, A::AddText);
    pick(ui.add_enabled(perms.can_modify, egui::Button::new(tl!("Add image"))), ui, A::AddImage);
    pick(ui.add_enabled(perms.can_assemble, egui::Button::new(tl!("Delete page"))), ui, A::DeletePage(page));
    pick(ui.add_enabled(perms.can_copy, egui::Button::new(tl!("Export this PDF"))), ui, A::ExportPdf);

    ui.separator();

    pick(ui.button(tl!("Find text or tools")), ui, A::Find);
    pick(ui.button(tl!("Rotate view clockwise")), ui, A::RotateCW);
    pick(ui.button(tl!("Document properties…")), ui, A::DocProperties);
}

impl DocView {
    /// Whether a middle-button scrolling gesture is active (tests and automation).
    pub fn auto_scrolling(&self) -> bool {
        self.auto_scroll.active()
    }

    /// Whether a held middle-button pan is active, outside Linux (tests and automation).
    pub fn middle_panning(&self) -> bool {
        self.auto_scroll.panning()
    }

    /// Abandon only pointer previews: clearing egui's held button must not commit an app-owned
    /// grab through a tool's `!primary_down` fallback. Keep selections, text drafts and edits
    /// that were already completed before cancellation.
    pub(crate) fn cancel_pointer_gestures(&mut self) {
        self.marquee = None;
        self.marquee_done = None;
        self.comments.gesture = None;
        // What snapping showed for the move goes with it: the point named on the page, the
        // marker, and Tab's choice of point.
        self.comments.anchor_shown = None;
        self.comments.snap_hover = None;
        self.comments.move_anchor = None;
        self.links.drag = None;
        self.sign.drag = None;
        self.redact_drag = None;
        self.crop_drag = None;
        if let Some(q) = &mut self.camera {
            q.grab = None;
        }
        self.block_drag = None;
        self.org_drag = None;
        self.panel_drag = None;
        self.selection_drag = false;
        self.router.cancel();
        self.auto_scroll.cancel();
        self.prepare.cancel_pointer_gesture();
        self.content.cancel_pointer_gesture();
        self.objects.cancel_pointer_gesture();
        if let Some(image) = &mut self.image_selection {
            image.cancel_pointer_gesture();
        }
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
            frame_demanded: HashSet::new(),
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
            screen_ppp: 1.0,
            viewport_screen: Rect::NOTHING,
            zoom_anchor: None,
            shown: false,
            auto_ocr_tried: Default::default(),
            auto_ocr_skipped: Default::default(),
            wheel: Default::default(),
            wheel_motion: Default::default(),
            wheel_frame: None,
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
            scroll_dir: 0,
            scroll_content: Vec2::ZERO,
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
            pages_focus_id: None,
            marquee: None,
            marquee_done: None,
            fill_text: None,
            fill_signature_page: None,
            signature_drag: Default::default(),
            cursor: Default::default(),
            router: Default::default(),
            press_cancelled: false,
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

    /// Select pages in the organize grid (tests, automation). Empty clears the selection.
    pub(crate) fn page_count(&self) -> usize {
        self.page_count
    }

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
    can_crop: bool,
    can_fill: bool,
    can_modify: bool,
    comments_hidden: bool,
    content_h: f32,
    content_w: f32,
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
    initials: &'a Option<crate::fill_sign::SavedSig>,
    key_scroll: f32,
    middle_gesture: bool,
    modal: bool,
    navigation_enabled: bool,
    paper: Color32,
    show_page_edge_shadows: bool,
    ppp: f32,
    precise_scroll: Vec2,
    prefs: &'a comments::CommentPrefs,
    preparing: bool,
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
    tool: QuickTool,
    units: crate::units::Unit,
    unobstructed: bool,
    visible_pages: &'a Vec<usize>,
    y_shift: f32,
    fill_text_size: f64,
    fill_text_color: [f64; 3],
    today: (i64, u32, u32),
    can_paste_image: bool,
}

/// What the page-scroll closure produces for the rest of the frame.
#[derive(Default)]
struct PageOut {
    canvas_action: Option<comments::CanvasAction>,
    clicked_link: Option<LinkTarget>,
    comment_notice: Option<&'static str>,
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
    primary_down: bool,
    refused: Option<String>,
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
    /// Snap to drawing (M5): the paths of the page being drawn, when its tool snaps.
    snap: Option<std::sync::Arc<pdfcraft_engine::measure::snap::Geometry>>,
    /// The comments already drawn on that page, as snap targets.
    snap_comments: Option<std::sync::Arc<pdfcraft_engine::measure::snap::Geometry>>,
    /// A page took this frame's right-click.
    right_clicked: bool,
}

impl PageFrame<'_> {
    /// Where this frame's right-click is, on the page at `rect`, for the first page it is on: pages
    /// that touch (the gaps hidden) can share a row of pixels, and a right-click there is one
    /// page's, as a press is.
    fn right_click_on(&mut self, rect: Rect) -> Option<Pos2> {
        let p = self.pointer.filter(|p| !self.right_clicked && self.resp.secondary_clicked() && rect.contains(*p))?;
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
    // Shadows would darken the preceding page when pages touch.
    if view.show_page_gaps && frame.env.show_page_edge_shadows {
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
    let PageEnv { allowed, comments_hidden, doc, editing_content, info, ppp, prefs, rects, redact_mark, scale, t, tool, .. } = frame.env;
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
        view.move_background.paint(painter, &xf, i, info, &view.content, view.content.drag_pointer(pointer));
    }
    if view.show_page_gaps {
        painter.rect_stroke(r, CornerRadius::ZERO, Stroke::new(0.5, t.border.gamma_multiply(0.8)), egui::StrokeKind::Outside);
    }

    // Snap to drawing (M5): the page's paths, for the drawing tools that set points and the
    // tools that place text or a Fill & Sign mark, on the page the pointer is over or a gesture
    // runs on (extracting a plan's paths takes up to a second). `resp` covers every page, so it
    // can't tell which one that is.
    let gesture_page = match &view.comments.gesture {
        Some(
            comments::Gesture::Draw { page, .. }
            | comments::Gesture::Move { page, .. }
            | comments::Gesture::Resize { page, .. }
            | comments::Gesture::Reshape { page, .. },
        ) => Some(*page),
        _ => None,
    };
    // Text added in Edit text & images, and a picture, snap by their corners while they are
    // moved; a picture not to itself where it was.
    let moving_text = editing_content && view.content.moving().is_some_and(|(page, _)| page == i);
    let moving_picture = view.image_selection.as_ref().and_then(crate::edit_text_ui::ImageSelection::moving).filter(|(page, _)| *page == i);
    let moving = moving_text || moving_picture.is_some();
    let here = pointer.is_some_and(|p| r.contains(p)) || gesture_page == Some(i) || moving;
    // The Select tool too: a comment moved or resized snaps by its corners; only while that runs
    // on this page (or an item is moved there), as hovering over a page with Select shows no snap
    // and shouldn't stall for its paths.
    let snaps = match tool {
        QuickTool::Comment(t) => t.snaps(),
        QuickTool::Fill(t) => t.snaps(),
        QuickTool::Select => gesture_page == Some(i) || moving,
        _ => moving,
    };
    let snapping = prefs.snap && snaps && here;
    frame.snap = if snapping {
        let mut notice = None;
        let geometry = match moving_picture {
            Some((page, index)) => view.comments.snap_cache.geometry_without_image(doc, page, index, &mut notice),
            None => view.comments.snap_cache.geometry(doc, i, &mut notice),
        };
        if notice.is_some() {
            view.comments.notice = notice;
        }
        geometry
    } else {
        None
    };
    // The comments already drawn snap too (lines join at their ends), but not the one being
    // moved or reshaped, which would hold on to where it was.
    frame.snap_comments = if snapping && !comments_hidden {
        let except: Vec<usize> = match &view.comments.gesture {
            Some(
                comments::Gesture::Move { page, index, .. }
                | comments::Gesture::Resize { page, index, .. }
                | comments::Gesture::Reshape { page, index, .. },
            ) if *page == i => {
                vec![*index]
            }
            _ => Vec::new(),
        };
        view.comments.snap_cache.comments(doc, i, &except)
    } else {
        None
    };
    let slot = PageSlot { i, r, xf };
    let pcx = comments::PageCx {
        page: i,
        xf: &xf,
        ptr,
        info,
        tool,
        prefs,
        allowed,
        hidden: comments_hidden,
        redact_mark,
        snap: frame.snap.clone(),
        snap_comments: frame.snap_comments.clone(),
        snap_cycle: view.comments.snap_cycle,
    };
    // A cancelled press takes nothing until the button is up, though the page's menu still opens
    // where a right-click is.
    let input = if view.press_cancelled {
        if let Some(p) = frame.right_click_on(xf.rect) {
            comments::anchor(&pcx, p, view);
        }
        PageInput { consumed: true, area_tool: false, hover: None, released_over: false }
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
        middle_gesture,
        modal,
        navigation_enabled,
        precise_scroll,
        prefs,
        preparing,
        selection_wheel,
        ppp,
        rects,
        single_paging,
        tool,
        unobstructed,
        visible_pages,
        y_shift,
        ..
    } = *env;
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
        // comment too (below), as one on a page's empty part does. A selected Prepare field keeps
        // its menu, off the pages as on them.
        view.comments.context_at = None;
        let mut over_image = false;
        if tool == QuickTool::EditText {
            let pointer_pos = ptr.pos;
            if let Some(p) = pointer_pos {
                for &page_idx in visible_pages {
                    let r = snap_to_pixels(rects[page_idx].translate(origin.to_vec2()), ppp);
                    if !r.intersects(visible) {
                        continue;
                    }
                    let xf =
                        PageXform { rect: r, rot: view.rotation, pw: info.pages[page_idx].width.max(1.0), ph: info.pages[page_idx].height.max(1.0) };
                    let images = cached_edit_images(view, doc, page_idx).to_vec();
                    // Crop controls own the click even when another image overlaps them.
                    if view.image_selection.as_ref().is_some_and(|s| {
                        s.page == page_idx
                            && s.crop
                                .as_ref()
                                .zip(images.get(s.index))
                                .is_some_and(|(crop, img)| crop_contains_point(img, crop, &xf, info, page_idx, p))
                    }) {
                        over_image = true;
                        break;
                    }
                    for (idx, img) in images.iter().enumerate().rev() {
                        let obb = ImageObb::from_image(img, &xf, info, page_idx);
                        let b = image_screen_rect(img, &xf, info, page_idx);
                        let selected_crop = view.image_selection.as_ref().is_some_and(|s| s.page == page_idx && s.index == idx && s.crop.is_some());
                        let hit = if selected_crop { crate::edit_text_ui::quad_contains(obb.corners(), p) } else { obb.contains(b, p) };
                        if hit {
                            over_image = true;
                            if view.image_selection.as_ref().is_none_or(|s| s.page != page_idx || s.index != idx) {
                                view.image_selection = Some(crate::edit_text_ui::ImageSelection::at(page_idx, idx));
                            }
                            break;
                        }
                    }
                    if over_image {
                        break;
                    }
                }
            }
        }
        if !over_image && tool == QuickTool::EditText {
            view.image_selection = None;
        }
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
        right_clicked: false,
        snap: None,
        snap_comments: None,
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
        out: page_out,
        right_clicked,
        ..
    } = frame;
    // A right-click no page took deselects the comment, so the page's menu opens, not the comment's.
    if resp.secondary_clicked() && !right_clicked {
        view.comments.selected = None;
    }
    let PageOut {
        mut canvas_action,
        clicked_link,
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
        refused,
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
    // The pages' one cursor, from the claim on the highest layer the tool reaches, and the tip.
    let hover_text = pointer.filter(|_| unobstructed).zip(view.cursor.resolve(ui.ctx(), tool));
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
    // so it can reach text off screen (#1670); the selection follows the pointer on its page.
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
    resp.context_menu(|ui| {
        let is_crop_over_image = tool == QuickTool::EditText
            && view.image_selection.clone().is_some_and(|sel| {
                let Some(ref crop) = sel.crop else { return false };
                let pointer_pos = ptr.pos;
                let Some(p) = pointer_pos else { return false };
                let Some((_, xf)) = view.screen_xforms.iter().find(|(page_idx, _)| *page_idx == sel.page).cloned() else { return false };
                let images = cached_edit_images(view, doc, sel.page).to_vec();
                let Some(img) = images.get(sel.index) else { return false };
                crop_contains_point(img, crop, &xf, info, sel.page, p)
            });
        if is_crop_over_image {
            ui.ctx().data_mut(|d| d.insert_temp(crate::edit_text_ui::image_menu_id(), false));
            egui::Popup::close_all(ui.ctx());
            ui.close();
            return;
        }
        // Preparing a form: the selected field's menu.
        if preparing && let Some((name, wi)) = view.prepare.selected.clone() {
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
        let can_paste_image = env.can_paste_image;
        let is_image_menu = tool == QuickTool::EditText
            && (ui.ctx().data_mut(|d| d.get_temp::<bool>(crate::edit_text_ui::image_menu_id())).unwrap_or(false)
                || view.image_selection.clone().is_some_and(|sel| {
                    let pointer_pos = ptr.pos;
                    let Some(p) = pointer_pos else { return false };
                    let Some((_, xf)) = view.screen_xforms.iter().find(|(page_idx, _)| *page_idx == sel.page).cloned() else { return false };
                    let images = cached_edit_images(view, doc, sel.page).to_vec();
                    let Some(img) = images.get(sel.index) else { return false };
                    let obb = ImageObb::from_image(img, &xf, info, sel.page);
                    let b = image_screen_rect(img, &xf, info, sel.page);
                    obb.contains(b, p)
                }));
        if is_image_menu && let Some(sel) = view.image_selection.clone() {
            let images = cached_edit_images(view, doc, sel.page).to_vec();
            // Paste lands where the menu opened only on the picture's page: where pages touch, the
            // first page takes the right-click, and its point means nothing on the second.
            let at = view.comments.context_at.filter(|(page, _)| *page == sel.page).map(|(_, at)| at);
            let rotation = (view.rotation % 360 + info.pages.get(sel.page).map_or(0, |p| p.rotation % 360)) % 360;
            crate::edit_text_ui::image_context_menu(ui, view, &images, sel.page, sel.index, can_paste_image, &mut image_action, at, rotation);
            return;
        }
        if view.comments.selected.is_some() {
            canvas_action = comments::context_menu(ui, view, info, &|page, index| doc.annotation_deletable(page, index), prefs, allowed, can_modify);
            return;
        }
        // Over a picture (and no comment): Copy Image, as in Acrobat's page menu.
        let mut copy_image = None;
        if let Some((page, at)) = view.comments.context_at {
            // The page's content is parsed once per right-click, not on every frame the menu is open.
            let key = egui::Id::unique("copy-image-under-menu");
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
        let has_copy_image = copy_image.is_some();
        if copy_image.is_some() {
            canvas_action = copy_image;
        }
        let (page, at) = view.comments.context_at.unwrap_or((view.current, [0.0, 0.0]));
        let can_copy_items = can_copy && (view.image_selection.is_some() || view.selected_text().is_some() || has_copy_image);
        let can_paste = can_paste_image && can_modify;
        let perms = PageMenuPerms {
            can_modify,
            can_assemble: doc.allows_assembly(),
            can_annotate: allowed,
            can_print: doc.allows_printing(),
            can_copy,
            can_copy_items,
            can_paste,
            has_annotations: !info.annotations.is_empty(),
        };
        default_page_context_menu(ui, page, at, perms, &mut page_action);
    });
    if !resp.context_menu_opened() {
        ui.ctx().data_mut(|d| d.insert_temp(crate::edit_text_ui::image_menu_id(), false));
    }
    let comment_notice = view.comments.notice.take();
    *out = PageOut {
        canvas_action,
        clicked_link,
        comment_notice,
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
        primary_down: ptr.primary_down,
        refused,
        stamp_placed,
    };
    (wanted, visible_now)
}

/// A zoom the fit mode changed keeps the page at the top of the view in place.
fn anchor_zoom_change(view: &mut DocView, avail_width: f32, old_zoom: f32) {
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
            view.zoom_anchor = Some((page, 0.5, fy, vec2(avail_width / 2.0, rel_y)));
        } else {
            view.zoom_anchor = Some((view.current, 0.5, 0.0, vec2(avail_width / 2.0, MARGIN)));
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
        // Within a point, so layout rounding can't stop a fitting page from turning.
        let fits = row_rect.is_some_and(|r| r.height() + 2.0 * MARGIN <= avail.height() + 1.0);
        let can_turn = !middle_gesture && unobstructed && ui.rect_contains_pointer(avail);
        single_paging = can_turn && fits;
        // Every wheel event goes to the pager, so it follows each trackpad touch to its end
        // even while the page can't turn.
        ui.input(|i| {
            for e in &i.events {
                if let egui::Event::MouseWheel { unit, delta, phase, modifiers, .. } = e {
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
    let consumed = ui.input(|i| {
        i.events.iter().any(|event| {
            matches!(event, egui::Event::MouseWheel { delta, modifiers, .. } if *delta != Vec2::ZERO && delta.is_finite() && !modifiers.command && !modifiers.ctrl)
        })
    });
    if consumed {
        wheel_reset::consumed(ui.ctx());
    }
    let input = {
        // Match the line distance used by egui's other scroll areas.
        let live = ui.ctx().options(|o| o.input_options.line_scroll_speed);
        let line_points = if live.is_finite() && live > 0.0 { live } else { crate::scroll_speed::EGUI_LINE_POINTS };
        ui.input(|i| crate::wheel_boost::accumulate(&i.events, line_points, single_paging))
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
    match (view.scroll_dir, visible_now.iter().min(), visible_now.iter().max()) {
        (1, _, Some(&last)) => (last.saturating_add(1)..=last.saturating_add(n)).collect(),
        (-1, Some(&first), _) => (first.saturating_sub(n)..first).rev().collect(),
        _ => Vec::new(),
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
}

/// Acts on the notice bar button that was clicked.
fn apply_notice(app: &mut PdfCraftApp, index: usize, notice: Option<Notice>) {
    match notice {
        #[cfg(not(target_arch = "wasm32"))]
        Some(Notice::Reload) => app.watch.reload_request = app.views.get(index).map(|view| view.id),
        #[cfg(target_arch = "wasm32")]
        Some(Notice::Reload) => {}
        Some(Notice::Repairs) => app.dialog = Some(crate::Dialog::Properties(crate::PropsTab::Advanced)),
        Some(Notice::Security) => app.dialog = Some(crate::Dialog::Properties(crate::PropsTab::Security)),
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
        Some(Notice::Signatures) => app.right = Some(RightPanel::Signatures),
        Some(Notice::FieldHighlights(on)) => app.view_defaults.highlight_fields = on,
        Some(Notice::Console) => app.dialog = Some(crate::Dialog::JsConsole),
        // Back on in Preferences ▸ General.
        Some(Notice::HideSecured) => app.show_secured_notice = false,
        None => {}
    }
}

/// The snapshot notice bar's Close was clicked: close here, where no document borrow is held. The
/// tab is looked up by id: indices may have shifted since the click, and the flag rides its own
/// view, so a stale flag can never close the wrong tab. Whether a tab was closed (or asked about).
fn close_requested_tab(app: &mut PdfCraftApp, index: usize) -> bool {
    let pending = app.views.get(index).filter(|v| v.close_requested).map(|v| v.id).and_then(|id| app.views.iter().position(|v| v.id == id));
    let Some(i) = pending else { return false };
    app.views[i].close_requested = false;
    if app.has_unsaved_work(i) {
        app.close_request = Some(crate::CloseRequest::Tab(app.views[i].id));
    } else {
        app.close_tab(i);
    }
    true
}

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
    let PageEnv { added, author, avail, can_modify, doc, form, info, prefs, t, today, tool, units, unobstructed, .. } = *env;
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
    find_bar(view, info.pages.len(), avail, ui, t);
    page_size_hint(view, info, units, avail, unobstructed, ui, t);
    if tool == QuickTool::EnhanceCamera {
        crate::camera_ui::bar(ui.ctx(), view, avail, t, can_modify);
    }
    if let Some(e) = comments::composer(ui.ctx(), view, info, prefs) {
        view.pending_edit = Some(e);
    }
    if let Some(e) = crate::edit_text_ui::overlay(ui.ctx(), view, info) {
        view.pending_edit = Some(e);
    }
    let text_box = crate::forms_ui::text_box(doc, view, form);
    if let Some(e) = crate::forms_ui::overlay(ui.ctx(), view, info, form, primary_down, text_box.as_ref(), today) {
        view.pending_edit = Some(e);
    }
    if let Some(e) = crate::fill_sign::type_box(ui.ctx(), view, info, author) {
        view.pending_edit = Some(e);
    }
    let (typed, text_done) = crate::content_ui::editor(ui.ctx(), view, info, added);
    if typed.is_some() {
        view.pending_edit = typed;
    }
    text_done
}

/// Full screen: the click turns the page unless it did something else first. It waits out the
/// double-click time: turning on the first click's release sent a double-click's second click,
/// meant to select a word, to the next page (review on #2070). A selection cancels it, and so do a
/// link followed meanwhile and leaving full screen; a double-click elsewhere still turns once.
/// `click` is `Some(back)` on the frame of a click, `back` when it was a shift-click.
fn schedule_presentation_turn(ui: &egui::Ui, view: &mut DocView, click: Option<bool>, link_followed: bool) {
    let now = ui.input(|i| i.time);
    if let Some(back) = click
        && !link_followed
        && view.selection.is_none()
        && view.clicks < 2
    {
        let delay = ui.ctx().options(|o| o.input_options.max_double_click_delay);
        view.present_turn = Some((now + delay, !back));
    }
    if let Some((due, forward)) = view.present_turn {
        if link_followed || view.selection.is_some() || view.presentation.is_none() {
            view.present_turn = None;
        } else if now >= due {
            view.present_turn = None;
            view.step_page(forward);
        } else {
            ui.ctx().request_repaint_after(std::time::Duration::from_secs_f64((due - now).max(0.0)));
        }
    }
}

/// Follows the link that was clicked. A link to another file comes back, to be opened once the
/// document is no longer borrowed.
fn follow_link(
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
        LinkTarget::Other(s) => app.notify_fmt("This PDF uses the unsupported {s} action.", &[("s", &s)]),
    }
    None
}

/// A finished marquee zoom, a signature rectangle drawn, or an empty signature field clicked.
/// Returns the tool to go on with.
fn finish_sign_requests(app: &mut PdfCraftApp, index: usize, mut tool: QuickTool) -> QuickTool {
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
    tool
}

/// When not scrolling continuously, only one row is shown at a time: the current page, or the
/// current two-page spread (with the cover rule). These indices drive paging, which pages are
/// drawn, and the content height (#752). Empty for a document with no pages.
fn shown_row(view: &DocView, rects: &[Rect]) -> Vec<usize> {
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
}

/// Full screen: a click on the document goes forward a page and a shift-click back, as in
/// Acrobat, unless the click did something else (a link, a word selected). Decided once the pages
/// have had the click. `Some(back)` on the frame of a click, `back` when it was a shift-click.
fn presentation_click_state(ui: &egui::Ui, view: &DocView, navigation_rect: Rect, enabled: bool) -> Option<bool> {
    let clicked = view.presentation.is_some() && enabled && presentation_click(ui, navigation_rect);
    // Shift as held now, or as the click's own event carried it (a synthetic click).
    clicked.then(|| {
        ui.input(|i| {
            i.modifiers.shift
                || i.events.iter().any(|e| {
                    matches!(e, egui::Event::PointerButton { button: egui::PointerButton::Primary, pressed: false, modifiers, .. } if modifiers.shift)
                })
        })
    })
}

/// Pinch / ctrl+scroll zoom, anchored on the pointer or the current page.
fn pinch_zoom(ui: &egui::Ui, view: &mut DocView, zoom_delta: f32, navigation_rect: Rect, enabled: bool) {
    if zoom_delta.is_finite() && zoom_delta > 0.0 && zoom_delta != 1.0 && enabled && ui.rect_contains_pointer(navigation_rect) {
        let z = view.zoom * zoom_delta;
        match zoom_anchor(ui) {
            Some(p) => view.zoom_at(z, p),
            None => view.set_zoom(z),
        }
    }
}

/// The middle-button auto-scroll's delta, and the edge scroll a drawing in progress asks for: the
/// pointer in a band along any edge (or just past it) scrolls the view that way, so a point
/// beyond what is shown can be reached without letting go of the tool.
fn auto_scroll_deltas(ui: &egui::Ui, view: &mut DocView, avail: Rect, unobstructed: bool, invert: bool) -> (Vec2, Vec2) {
    let auto_delta = if unobstructed {
        view.auto_scroll.update(ui, avail, false, invert)
    } else {
        view.auto_scroll.cancel();
        Vec2::ZERO
    };

    // Drawing a comment: the pointer in a band along any edge (or just past it) scrolls the view
    // that way, so a point beyond what is shown can be reached without letting go of the tool.
    let edge_delta = if unobstructed && matches!(view.comments.gesture, Some(comments::Gesture::Draw { .. })) {
        let (pointer, dt) = drawing_edge_pointer(ui);
        crate::autoscroll::edge_scroll(pointer, avail, dt)
    } else {
        Vec2::ZERO
    };
    if edge_delta != Vec2::ZERO {
        ui.ctx().request_repaint();
    }
    (auto_delta, edge_delta)
}

/// What decides whether touchpad momentum goes on.
#[derive(Clone, Copy)]
struct GlideCx {
    now: f64,
    may_glide: bool,
    key_scroll: f32,
    auto_delta: Vec2,
    zoom_delta: f32,
    edge_delta: Vec2,
    wheel_scrolled: bool,
}

/// Touchpad momentum (#531): a swipe over the page lifted at speed glides on and slows to a stop,
/// through the same un-eased path. Never while the pager owns the wheel, a gesture or dialog has
/// the input, or animations are off; any click, key, zoom or other scrolling ends it. Any
/// reader-owned motion ends automatic search following, even when it stays on the same page; the
/// automatic flash uses `goto`, not these deltas, so its programmatic scroll is never mistaken for
/// a reader action. Returns this frame's glide.
fn step_glide(ui: &egui::Ui, view: &mut DocView, avail: Rect, cx: &GlideCx) -> Vec2 {
    let GlideCx { now, may_glide, key_scroll, auto_delta, zoom_delta, edge_delta, wheel_scrolled } = *cx;
    let over_page = ui.rect_contains_pointer(avail);
    ui.input(|i| view.fling.read(&i.events, now, may_glide && over_page));
    let interrupted = glide_interrupted(ui);
    if !may_glide || interrupted || key_scroll != 0.0 || auto_delta != Vec2::ZERO || zoom_delta != 1.0 {
        view.fling.stop();
    }
    let glide = view.fling.step(now);
    if view.fling.gliding() {
        ui.ctx().request_repaint();
    }
    if wheel_scrolled || auto_delta != Vec2::ZERO || edge_delta != Vec2::ZERO || glide != Vec2::ZERO {
        view.end_find_following();
    }
    glide
}

/// Text selection runs for the Select tool and for the markup tools (highlight…).
fn tool_selects_text(tool: QuickTool, preparing: bool) -> bool {
    match tool {
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
    }
}

/// Holds sharp re-renders while the zoom is still moving: a new scale every frame would cancel the
/// in-flight full-page interpret and restart it, so on a heavy page (a CAD plot with ~1000 images
/// renders for seconds) the sharp raster never arrives and the view looks stuck on a soft image.
/// The stale raster stays stretched meanwhile; the sharp request goes out once the zoom settles.
fn hold_sharp_renders(ui: &egui::Ui, view: &mut DocView, scale: f32) -> bool {
    let now_scale = ui.input(|i| i.time);
    if (scale - view.last_scale).abs() > f32::EPSILON {
        view.last_scale = scale;
        view.sharp_hold_until = now_scale + 0.35;
        ui.ctx().request_repaint_after(std::time::Duration::from_millis(380));
    }
    now_scale < view.sharp_hold_until
}

/// A destination's position (bookmarks, links) refines `goto`: its point at the window's top-left,
/// or the centre of a /FitR rectangle at the window's centre.
fn refine_goto_point(mut scroll: egui::ScrollArea, view: &mut DocView, geometry: &ScrollGeometry<'_>) -> egui::ScrollArea {
    let ScrollGeometry { rects, y_shift, content_w, content_h, avail, .. } = *geometry;
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
    scroll
}

/// Draws the document tab at `index`: its notice bars, then its pages (or the Organize grid) in a
/// scroll area with this frame's zoom, scrolling, tool input, overlays and render requests.
pub fn document_area(app: &mut PdfCraftApp, index: usize, ui: &mut egui::Ui) {
    let t = Tokens::get(ui.ctx());
    let can_paste_key = egui::Id::unique("can-paste-image");
    let can_paste_image = if ui.input(|i| i.pointer.secondary_pressed()) {
        let can = app.can_paste_image();
        ui.data_mut(|d| d.insert_temp(can_paste_key, can));
        can
    } else {
        ui.data(|d| d.get_temp(can_paste_key)).unwrap_or(false)
    };
    // Keyboard scrolling applies to this frame's page view only, never later.
    let key_scroll = std::mem::take(&mut app.views[index].key_scroll);
    if key_scroll != 0.0 {
        app.views[index].end_find_following();
    }
    // The Search panel closed: its search moves to the find bar.
    let search_open = app.right == Some(crate::RightPanel::Search);
    if let Some(f) = app.views[index].find.as_mut()
        && f.in_panel
        && !search_open
    {
        f.in_panel = false;
    }
    if close_requested_tab(app, index) {
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
    let notice = notices(view, doc, secured, repaired, crate::sign_ui::banner(&doc.signatures), ui, &t);
    apply_notice(app, index, notice);
    let Some(doc) = app.session.get(app.views[index].id) else { return };
    let info = &doc.info;
    // No dialog, close prompt or palette over the page: only then does page input count.
    let modal = app.modal_open();
    let unobstructed = !modal && !app.palette_open;
    let organize_zoom = app.organize_zoom;
    let invert_middle = app.invert_middle_drag;
    let view = &mut app.views[index];
    if view.organize {
        // One remembered grid size for every document, kept in the settings.
        view.set_grid_zoom(organize_zoom);
        organize_grid(view, doc, unobstructed, app.show_page_edge_shadows, invert_middle, ui, &t);
        app.organize_zoom = view.grid_zoom();
        return;
    }

    let avail = ui.available_rect_before_wrap();
    view.viewport_w = avail.width();
    view.viewport_h = avail.height();
    let old_zoom = view.zoom;
    view.fit_zoom(info);
    anchor_zoom_change(view, avail.width(), old_zoom);

    // Pinch / ctrl+scroll zoom, anchored on the current page.
    let zoom_delta = crate::motion::zoom_delta(ui.ctx());
    // Exclude the previous frame's scrollbar tracks from gesture activation.
    let navigation_rect = if view.viewport_screen.is_positive() { avail.intersect(view.viewport_screen) } else { avail };
    let navigation_enabled = app.dialog.is_none() && app.close_request.is_none() && !app.palette_open && ui.is_enabled() && ui.input(|i| i.focused);
    let present = presentation_click_state(ui, view, navigation_rect, navigation_enabled && app.quick_tool == QuickTool::Select);
    pinch_zoom(ui, view, zoom_delta, navigation_rect, navigation_enabled);
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
    let motion = GlideCx { now, may_glide, key_scroll, auto_delta, zoom_delta, edge_delta, wheel_scrolled };
    let glide = step_glide(ui, view, avail, &motion);
    // The wheel may have turned the page above: draw the row it turned to. Measured from the old
    // row, the jump to the new page scrolled both out of view, a blank frame on every turn forward.
    let row = shown_row(view, &rects);
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
    let tool = app.quick_tool;
    // Text selection runs for the Select tool and for the markup tools (highlight…).
    let selects_text = tool_selects_text(tool, preparing);
    let prefs = &app.comment_prefs;
    let redact_mark = (app.redact_prefs.hover_fill, app.redact_prefs.opacity);
    let units = app.units;
    let allowed = doc.allows_annotation();
    let can_copy = doc.allows_copying();
    let comments_hidden = doc.comments_hidden();
    let form = doc.form.clone();
    let generation = doc.edit_generation();
    let can_fill = doc.allows_form_filling();
    let can_crop = doc.allows_assembly();
    let signature = app.signature.clone();
    let initials = app.initials.clone();
    let author = app.comment_prefs.author.clone();
    let today = app.session.today();
    // In Preferences ▸ Date format, or why it can't be stamped into the PDF.
    let date_text = crate::date_text_for_pdf(&app.session);
    let by_line = app.session.stamp_by_line(&author);
    let custom_stamp = match app.quick_tool {
        QuickTool::CustomStamp(i) => app.custom_stamps.get(i).cloned(),
        _ => None,
    };
    let mut open_props: Option<(usize, usize)> = None;
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

    scroll = refine_goto_point(scroll, view, &ScrollGeometry { rects: &rects, y_shift, content_w, content_h, avail, middle_gesture, hand });
    let selection_wheel = view.selection_drag && navigation_enabled && unobstructed && !single_paging && ui.rect_contains_pointer(avail);
    let env = PageEnv {
        added: &added,
        allowed,
        author: &author,
        auto_delta,
        avail,
        coarse_scroll,
        by_line: &by_line,
        calc_order,
        can_copy,
        can_crop,
        can_fill,
        can_modify,
        comments_hidden,
        content_h,
        content_w,
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
        middle_gesture,
        modal,
        navigation_enabled,
        paper,
        show_page_edge_shadows: app.show_page_edge_shadows,
        ppp,
        precise_scroll,
        prefs,
        preparing,
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
        tool,
        units,
        unobstructed,
        visible_pages: &visible_pages,
        y_shift,
        fill_text_size: app.fill_text_size,
        fill_text_color: app.fill_text_color,
        today,
        can_paste_image,
    };
    let mut page_out = PageOut::default();
    let out = page_scroll(ui, scroll, egui::Id::unique(("page-view-scope", view.id)), app.show_page_edge_shadows, |ui, viewport| {
        draw_pages(ui, viewport, view, &env, &mut page_out, &mut app.signature_preview)
    });
    let PageOut {
        canvas_action,
        clicked_link,
        comment_notice,
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
        primary_down,
        mut refused,
        stamp_placed,
    } = page_out;
    view.single_edges = visible_pages.first().copied().filter(|_| !view.continuous).map(|page| {
        // At the top once the page's top edge shows where going to a page puts it (a gap
        // below the window's top). Within a point, as for wheel paging, so layout rounding
        // can't hide an edge.
        let max_y = (out.content_size.y - out.inner_rect.height()).max(0.0);
        (page, out.state.clamped_offset().y <= MARGIN - view.page_gap() + 1.0, out.state.clamped_offset().y >= max_y - 1.0)
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
    comments::keys(ui.ctx(), view, &mut tool, allowed);
    if preparing {
        crate::prepare::keys(ui.ctx(), view);
    }
    if tool != QuickTool::EditText {
        view.image_selection = None;
        crate::content_ui::release_image_selection(view, &added);
    }
    if editing_content {
        crate::content_ui::keys(ui.ctx(), view, tool == QuickTool::EditText);
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
    schedule_presentation_turn(ui, view, present, clicked_link.is_some());
    let remote_open = follow_link(app, index, clicked_link);
    if tool == QuickTool::Crop && cropped {
        tool = QuickTool::Select;
    }
    let tool = finish_sign_requests(app, index, tool);
    app.quick_tool = tool;
    let requests = FrameRequests {
        open_props,
        field_props,
        field_menu,
        image_action,
        page_action,
        refused,
        open_signature,
        open_initials,
        open_boxes,
        open_table,
        form_notice,
        copy_selected,
        comment_notice,
        remote_open,
        form,
        generation,
        avail,
    };
    apply_frame_requests(app, index, ui, requests);
}

/// What the frame's input asked the app to do, once the document is no longer borrowed.
struct FrameRequests {
    open_props: Option<(usize, usize)>,
    field_props: Option<(String, usize)>,
    field_menu: Option<FieldMenu>,
    image_action: Option<crate::edit_text_ui::ImageAction>,
    page_action: Option<PageMenuAction>,
    refused: Option<String>,
    open_signature: bool,
    open_initials: bool,
    open_boxes: bool,
    open_table: Option<(usize, [f64; 4])>,
    form_notice: Option<crate::forms_ui::FormNotice>,
    copy_selected: bool,
    /// What the comment tools have to say (Snap to drawing's limits).
    comment_notice: Option<&'static str>,
    remote_open: Option<(pdfcraft_engine::DocId, String, pdfcraft_render::RemoteDestination)>,
    form: Arc<Vec<pdfcraft_engine::FormField>>,
    generation: u64,
    avail: Rect,
}

/// Opens the dialogs and runs the app commands the frame asked for.
#[allow(clippy::cognitive_complexity)]
fn apply_frame_requests(app: &mut PdfCraftApp, index: usize, ui: &mut egui::Ui, requests: FrameRequests) {
    let FrameRequests {
        open_props,
        field_props,
        field_menu,
        image_action,
        page_action,
        refused,
        open_signature,
        open_initials,
        open_boxes,
        open_table,
        form_notice,
        copy_selected,
        comment_notice,
        remote_open,
        form,
        generation,
        avail,
    } = requests;
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
                    app.views[index].comments.context_at = Some((page, center));
                    app.views[index].comments.composer = Some(crate::comments::Composer {
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
            if let Some(sel) = app.views[index].image_selection.clone() {
                app.copy_page_image(sel.page, sel.index);
            } else if app.views[index].selected_text().is_some() {
                app.execute("edit.copy");
            } else if let Some((page, at)) = app.views[index].comments.context_at
                && let Some(doc) = app.session.get(app.views[index].id)
                && let Some(image) = doc.page_image_at(page, at)
            {
                app.copy_page_image(page, image);
            }
        }
        Some(PageMenuAction::Paste(page, at)) => {
            app.paste_image_at(page, at);
        }
        Some(PageMenuAction::RotateCCW) => {
            app.views[index].rotate_view(false);
        }
        Some(PageMenuAction::RotateCW) => {
            app.views[index].rotate_view(true);
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
            app.views[index].comments.composer =
                Some(crate::comments::Composer { page, at, kind: crate::comments::ComposerKind::Note, text: String::new(), focus: true });
        }
        Some(PageMenuAction::FlattenComments) => {
            app.views[index].comments.selected = None;
            app.views[index].pending_edit = Some(pdfcraft_engine::Edit::Flatten { comments: true, fields: false });
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
            app.views[index].pending_edit = Some(pdfcraft_engine::Edit::DeletePages { pages: vec![page] });
        }
        Some(PageMenuAction::ExportPdf) => {
            app.select_tool("export");
        }
        Some(PageMenuAction::Find) => {
            app.views[index].open_find();
        }
        Some(PageMenuAction::DocProperties) => {
            app.dialog = Some(crate::Dialog::Properties(crate::PropsTab::Description));
        }
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
    if let Some(notice) = comment_notice {
        app.notify_tr(notice);
    }
    // Read mode hides the tools (#2646), and full screen is the page and nothing else (#1459);
    // the quick tools come back with the window.
    if app.mode != crate::Mode::Read && !app.full_screen {
        // The page nearest the left edge, whose margin the bar sits in.
        let page_left = app.views[index].screen_rects.iter().map(|(_, r)| r.left()).reduce(f32::min);
        quick_bar(app, avail, page_left, ui);
    }
    if let Some((source, file, destination)) = remote_open {
        app.open_remote_link(source, file, destination);
    }
}

/// The prepare-mode field menu's choices.
enum FieldMenu {
    Properties,
    Duplicate(String),
}

#[cfg(test)]
mod tests;
