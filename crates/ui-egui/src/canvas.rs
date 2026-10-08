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
use pdfcraft_render::{DocInfo, LayerOp, LinkTarget, PageText, RenderPool, RenderRequest, RequestKind, Tile, device_pixels};

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
/// The tag of a raster that is out of date (shown until its replacement arrives).
const STALE_TAG: u64 = u64::MAX;
/// Pages whose raster would exceed this many device pixels on a side are drawn in tiles.
const TILE_THRESHOLD: f32 = 4096.0;
const TILE: u32 = 1024;
/// Longest side of the low-resolution backdrop drawn under tiles.
const BASE_SIDE: f32 = 2048.0;
const THUMB_W: f32 = 132.0;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Fit {
    Width,
    Page,
    Height,
    None,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PageLayout {
    Continuous,
    TwoUp,
    Single,
}

impl PageLayout {
    /// Every page display, in the order the View menu, the rail's Page display button and
    /// Preferences list them.
    pub const ORDER: [Self; 3] = [Self::Continuous, Self::Single, Self::TwoUp];

    /// The name settings and view options use: `continuous`, `single` or `two-up`.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Continuous => "continuous",
            Self::Single => "single",
            Self::TwoUp => "two-up",
        }
    }

    /// The layout an [`as_str`](Self::as_str) name stands for, in any case. `None` for
    /// anything else, so callers can reject a typo rather than switch layouts silently.
    pub fn try_parse(value: &str) -> Option<Self> {
        Self::ORDER.into_iter().find(|l| l.as_str().eq_ignore_ascii_case(value.trim()))
    }

    /// Rail icon for this layout.
    pub fn icon(self) -> &'static str {
        match self {
            Self::Continuous => "arrow-up-down",
            Self::Single => "file-text",
            Self::TwoUp => "columns-2",
        }
    }

    /// Registry command that switches to this layout (`PdfCraftApp::execute`).
    pub fn command(self) -> &'static str {
        match self {
            Self::Continuous => "view.layout.continuous",
            Self::Single => "view.layout.single",
            Self::TwoUp => "view.layout.two_up",
        }
    }

    /// The layout a [`command`](Self::command) id switches to.
    pub fn from_command(id: &str) -> Option<Self> {
        Self::ORDER.into_iter().find(|l| l.command() == id)
    }

    /// Menu label: the label of [`command`](Self::command), which the catalogs translate.
    pub fn label(self) -> &'static str {
        match self {
            Self::Continuous => "Continuous scrolling",
            Self::Single => "Single page",
            Self::TwoUp => "Two-page view",
        }
    }
}

/// How a newly opened document is shown (Preferences ▸ Documents and view). A PDF that asks
/// for its own layout or zoom gets that instead.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ViewDefaults {
    pub layout: PageLayout,
    /// [`Fit::Width`] or [`Fit::Page`], or [`Fit::None`] for a fixed `zoom`.
    pub fit: Fit,
    /// The zoom with [`Fit::None`] (1.0 = 100%).
    pub zoom: f32,
}

impl Default for ViewDefaults {
    fn default() -> Self {
        Self { layout: PageLayout::Continuous, fit: Fit::Width, zoom: 1.0 }
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

/// A text selection on one page, in reading-order glyph indices.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Selection {
    page: usize,
    anchor: usize,
    head: usize,
}

impl Selection {
    fn range(&self) -> Range<usize> {
        self.anchor.min(self.head)..self.anchor.max(self.head) + 1
    }
}

struct PageTex {
    tag: u64,
    tex: TextureHandle,
}

pub struct DocView {
    pub id: DocId,
    pub zoom: f32,
    pub fit: Fit,
    pub layout: PageLayout,
    /// View rotation in degrees clockwise (0, 90, 180, 270); display only, never saved.
    pub rotation: u16,
    pub current: usize,
    pub organize: bool,
    pub highlight_fields: bool,
    pub page_input: String,
    pub notice_dismissed: bool,
    /// Two-page view: show the first page alone, as a cover (View ▸ Page display).
    pub cover: bool,
    /// Previous view / Next view: pages visited before (and after, once going back).
    pub back: Vec<usize>,
    pub forward: Vec<usize>,
    /// Pending navigation: page and fraction down the page to align with the viewport top.
    pub goto: Option<(usize, f32)>,
    /// Pending keyboard scrolling, in points down (negative: up): ↓ / ↑, and Page Down /
    /// Page Up where the pages scroll.
    pub key_scroll: f32,
    /// Briefly outline an annotation after navigating to it from a panel.
    pub flash: Option<(usize, [f32; 4], f64)>,
    /// Compare files: differences shaded on this document's pages (page, user-space box, colour).
    pub compare_marks: Vec<(usize, [f32; 4], Color32)>,
    pages: HashMap<usize, PageTex>,
    /// Pages the renderer could not draw, with the reason (never re-requested).
    errors: HashMap<usize, String>,
    /// When each still-missing page was first shown, to flag unusually slow renders.
    waiting_since: HashMap<usize, f64>,
    thumbs: HashMap<usize, TextureHandle>,
    /// Thumbnails that are out of date (still shown until their replacement arrives).
    stale_thumbs: HashSet<usize>,
    /// Sharp tiles of large pages: (page, tile x, tile y) → (scale tag, texture).
    tiles: HashMap<(usize, u32, u32), (u64, TextureHandle)>,
    /// Text layers, extracted in the background on demand (selection, find, copy).
    pub(crate) texts: HashMap<usize, Arc<PageText>>,
    pub(crate) text_failed: HashSet<usize>,
    pub find: Option<Find>,
    selection: Option<Selection>,
    last_queue: Vec<RenderRequest>,
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
    /// Pending zoom anchor: page, position within it (0..1), and offset from the viewport corner.
    zoom_anchor: Option<(usize, f32, f32, Vec2)>,
    /// Turns wheel input into page turns in single-page view.
    wheel: crate::wheel_pager::WheelPager,
    pub(crate) auto_scroll: crate::autoscroll::AutoScroll,
    /// Pages selected in the organize grid or the Pages panel (0-based). Empty means "the current page".
    pub selected: BTreeSet<usize>,
    /// Anchor for ⇧-click range selection in the organize grid.
    select_anchor: Option<usize>,
    /// An edit requested by the view (organize toolbar, keys), applied by the app this frame.
    pub pending_edit: Option<Edit>,
    /// Edit text: the lines per page (with the document generation they were read at), and the
    /// line being edited.
    pub(crate) edit_lines: HashMap<usize, (u64, Vec<pdfcraft_engine::TextBlock>)>,
    pub line_editor: Option<crate::edit_text_ui::LineEditor>,
    /// Edit text & images: the images per page (by document generation), and the selected one.
    pub(crate) edit_images: HashMap<usize, (u64, Vec<pdfcraft_engine::PageImage>)>,
    pub image_selection: Option<crate::edit_text_ui::ImageSelection>,
    /// A paragraph box being dragged (moved, or resized from its right edge) in Edit text.
    pub block_drag: Option<crate::edit_text_ui::BlockDrag>,
    /// Commenting state: selected comment, gestures, composer.
    pub comments: crate::comments::CommentView,
    pub measure: crate::measure_ui::MeasureView,
    /// Form filling state: the focused field.
    pub forms: crate::forms_ui::FormView,
    /// Prepare a form: the selected field and the gesture in progress.
    pub prepare: crate::prepare::PrepareView,
    /// Edit a PDF: the selected added item, the text being typed.
    pub content: crate::content_ui::ContentView,
    /// Edit a PDF ▸ Link tool state.
    pub links: crate::link_ui::LinkView,
    /// Redact tool: the box being drawn, and a mark to add (page, quads) for the app to style.
    pub redact_drag: crate::redact_ui::AreaDrag,
    pub pending_redaction: Option<(usize, Vec<[f64; 8]>)>,
    /// A crop rectangle being dragged (Crop tool).
    pub crop_drag: crate::crop::CropDrag,
    /// Use a certificate: a signature rectangle being drawn, or an empty signature field clicked.
    pub sign: crate::sign_ui::SignView,
    /// Organize: the pages being dragged to a new place.
    pub org_drag: Option<Vec<usize>>,
    /// Marquee Zoom / Snapshot: the rectangle being dragged (page, start), and a finished one.
    pub marquee: Option<(usize, Pos2)>,
    pub marquee_done: Option<crate::zoom_snap::Marquee>,
    /// Fill & Sign text being typed.
    pub fill_text: Option<crate::fill_sign::TypeBox>,
    /// A non-edit action requested by the organize toolbar, handled by the app.
    pub pending_action: Option<ViewAction>,
}

/// Organize-toolbar actions that need the app (file pickers, new tabs, dialogs).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ViewAction {
    InsertFromFile,
    Extract,
    Split,
    /// Copy the selected pages (Cut also deletes them).
    CopyPages {
        cut: bool,
    },
    /// Paste copied pages after the selection.
    PastePages,
}

impl DocView {
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
            rotation: 0,
            current: 0,
            organize: false,
            highlight_fields: false,
            page_input: "1".into(),
            notice_dismissed: false,
            cover: false,
            back: Vec::new(),
            forward: Vec::new(),
            goto: None,
            key_scroll: 0.0,
            flash: None,
            compare_marks: Vec::new(),
            pages: HashMap::new(),
            errors: HashMap::new(),
            waiting_since: HashMap::new(),
            thumbs: HashMap::new(),
            stale_thumbs: HashSet::new(),
            tiles: HashMap::new(),
            texts: HashMap::new(),
            text_failed: HashSet::new(),
            find: None,
            selection: None,
            last_queue: Vec::new(),
            viewport_w: 800.0,
            viewport_h: 600.0,
            page_count: info.pages.len(),
            page_heights: info.pages.iter().map(|p| p.height).collect(),
            screen_rects: Vec::new(),
            screen_xforms: Vec::new(),
            viewport_screen: Rect::NOTHING,
            zoom_anchor: None,
            wheel: Default::default(),
            auto_scroll: Default::default(),
            selected: BTreeSet::new(),
            select_anchor: None,
            pending_edit: None,
            edit_lines: HashMap::new(),
            line_editor: None,
            edit_images: HashMap::new(),
            image_selection: None,
            block_drag: None,
            pending_action: None,
            comments: Default::default(),
            measure: Default::default(),
            forms: Default::default(),
            prepare: Default::default(),
            content: Default::default(),
            links: Default::default(),
            redact_drag: None,
            pending_redaction: None,
            crop_drag: None,
            sign: Default::default(),
            org_drag: None,
            marquee: None,
            marquee_done: None,
            fill_text: None,
        }
    }

    /// The document's content or page list changed (an edit, undo or redo): drop caches and
    /// adopt the new page geometry, keeping the reader's place where possible.
    pub fn document_changed(&mut self, info: &DocInfo) {
        self.invalidate_content();
        self.page_count = info.pages.len();
        self.page_heights = info.pages.iter().map(|p| p.height).collect();
        let last = self.page_count.saturating_sub(1);
        self.current = self.current.min(last);
        self.page_input = (self.current + 1).to_string();
        self.selected.retain(|p| *p <= last);
        if self.select_anchor.is_some_and(|a| a > last) {
            self.select_anchor = None;
        }
        self.goto = None;
        self.zoom_anchor = None;
        self.flash = None;
    }

    /// Pages an organize command acts on: the selection, or the current page.
    pub fn target_pages(&self) -> Vec<usize> {
        if self.selected.is_empty() { vec![self.current] } else { self.selected.iter().copied().collect() }
    }

    /// Select pages in the organize grid (tests, automation). Empty clears the selection.
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
        !self.last_queue.is_empty()
    }

    /// Re-render every page and text layer (the document's appearance changed, e.g. a layer
    /// was toggled). Out-of-date rasters stay on screen until their replacements arrive, so the
    /// view never flashes blank.
    pub fn invalidate_content(&mut self) {
        for p in self.pages.values_mut() {
            p.tag = STALE_TAG;
        }
        self.stale_thumbs.extend(self.thumbs.keys().copied());
        self.tiles.clear();
        self.texts.clear();
        self.text_failed.clear();
        self.errors.clear();
        self.waiting_since.clear();
        self.last_queue.clear();
        self.selection = None;
        if let Some(f) = self.find.as_mut() {
            f.matches.clear();
            f.current = None;
        }
    }

    /// Only `page` changed (a comment was added, edited or removed): re-render that page and
    /// re-read its text, keep everything else.
    pub fn page_changed(&mut self, page: usize) {
        if let Some(p) = self.pages.get_mut(&page) {
            p.tag = STALE_TAG;
        }
        if self.thumbs.contains_key(&page) {
            self.stale_thumbs.insert(page);
        }
        self.tiles.retain(|(p, _, _), _| *p != page);
        self.texts.remove(&page);
        self.text_failed.remove(&page);
        self.errors.remove(&page);
        self.last_queue.clear();
        if self.selection.is_some_and(|s| s.page == page) {
            self.selection = None;
        }
        if let Some(f) = self.find.as_mut() {
            f.matches.retain(|(p, _)| *p != page);
            f.current = None;
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
        let quads: Vec<[f64; 8]> = text.line_rects(s.range()).into_iter().map(|r| page.view_rect_to_quad(r)).collect();
        (!quads.is_empty()).then_some((s.page, quads))
    }

    /// A page's thumbnail texture, when rendered (the print preview uses them).
    pub(crate) fn thumb_id(&self, page: usize) -> Option<egui::TextureId> {
        self.thumbs.get(&page).map(|t| t.id())
    }

    pub(crate) fn page_text(&self, page: usize) -> Option<Arc<PageText>> {
        self.texts.get(&page).cloned()
    }

    pub(crate) fn clear_selection(&mut self) {
        self.selection = None;
    }

    /// Select text on `page` from glyph `from` to glyph `to` (tests and automation).
    pub fn select_text(&mut self, page: usize, from: usize, to: usize) {
        self.selection = Some(Selection { page, anchor: from, head: to });
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
        let pages: Vec<usize> = self.texts.keys().copied().collect();
        for p in pages {
            self.refresh_find_page(p);
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
        f.current = match current_key {
            Some(k) => f.matches.iter().position(|m| *m == k),
            None => None,
        };
        if f.current.is_none() && !f.matches.is_empty() {
            // First match at or after the page being viewed.
            let cur = self.current;
            let i = f.matches.iter().position(|(p, _)| *p >= cur).unwrap_or(0);
            f.current = Some(i);
            let (p, _) = f.matches[i];
            self.flash_match(p);
        }
    }

    /// Go to match `i` (the Search panel's results).
    pub fn go_to_match(&mut self, i: usize) {
        let Some(f) = self.find.as_mut() else { return };
        let Some(&(p, _)) = f.matches.get(i) else { return };
        f.current = Some(i);
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
    }

    /// Screen position of the centre of glyph `glyph` on `page`, if that page is on screen and its
    /// text layer is loaded (used by UI tests and automation to aim pointer input).
    pub fn glyph_screen_pos(&self, page: usize, glyph: usize) -> Option<Pos2> {
        let (_, xf) = self.screen_xforms.iter().find(|(p, _)| *p == page)?;
        let g = self.texts.get(&page)?.glyphs.get(glyph)?;
        Some(xf.view_rect(g.rect).center())
    }

    /// Selected text, if any (⌘C).
    pub fn selected_text(&self) -> Option<String> {
        let s = self.selection?;
        let t = self.texts.get(&s.page)?;
        Some(t.text_of(s.range())).filter(|x| !x.is_empty())
    }

    pub fn thumb(&self, page: usize) -> Option<&TextureHandle> {
        self.thumbs.get(&page)
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
        self.wheel.clear_motion();
        self.goto = Some((page, 0.0));
        self.current = page;
        self.page_input = (page + 1).to_string();
    }

    /// Next page (`true`) or previous page. In two-page view this moves a whole spread: the other
    /// page of the spread is already on screen, so stepping to it wouldn't move the view (#70).
    pub fn step_page(&mut self, forward: bool) {
        let c = self.current;
        let target = match self.layout {
            PageLayout::TwoUp => {
                // Spreads are [0, 1], [2, 3], … or, with a cover page, [0], [1, 2], [3, 4], ….
                let first = if self.cover { c.saturating_sub((c + 1) % 2) } else { c - c % 2 };
                match (forward, self.cover && c == 0) {
                    (true, true) => 1,
                    (true, false) => first.saturating_add(2),
                    (false, _) if self.cover && first <= 1 => 0,
                    (false, _) => first.saturating_sub(2),
                }
            }
            PageLayout::Continuous | PageLayout::Single if forward => c + 1,
            PageLayout::Continuous | PageLayout::Single => c.saturating_sub(1),
        };
        self.go_to_page(target);
    }

    /// Go to the page typed in the page box: a page label (logical page numbers, as Acrobat
    /// does), else a page number. `false` when it names no page.
    pub fn go_to_typed(&mut self, typed: &str, labels: &[String]) -> bool {
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
        self.page_input = (page + 1).to_string();
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
        let page = self.current;
        let Some(t) = self.texts.get(&page) else { return false };
        if t.glyphs.is_empty() {
            return false;
        }
        self.selection = Some(Selection { page, anchor: 0, head: t.glyphs.len() - 1 });
        true
    }

    /// Rotate the view 90° clockwise or counter-clockwise (View ▸ Rotate View, ⇧⌘+ / ⇧⌘−).
    pub fn rotate_view(&mut self, clockwise: bool) {
        self.rotation = (self.rotation + if clockwise { 90 } else { 270 }) % 360;
        self.goto = Some((self.current, 0.0));
    }

    /// Switch the zoom mode, keeping the current page in view.
    pub fn set_fit(&mut self, fit: Fit) {
        self.fit = fit;
        self.goto = Some((self.current, 0.0));
    }

    /// Switch the page display. A no-op when the layout is already current, so re-selecting
    /// it never yanks a scrolled view back to the top.
    pub fn set_layout(&mut self, layout: PageLayout) {
        if self.layout == layout {
            return;
        }
        self.layout = layout;
        self.goto = Some((self.current, 0.0));
    }

    /// Set the cover page in two-page view. A no-op unless something changed.
    pub fn set_cover(&mut self, cover: bool) {
        if self.cover != cover {
            self.cover = cover;
            self.goto = Some((self.current, 0.0));
        }
    }

    /// Whether the cover page setting applies: only two-page view has a cover page.
    pub fn cover_applies(&self) -> bool {
        self.layout == PageLayout::TwoUp
    }

    /// One wheel event in single-page view (the rules are in `wheel_pager`): `dy` is its
    /// vertical delta (negative scrolls down) and `now` is egui time. With `can_turn` false
    /// the gesture is followed but the page stays. Returns whether the page turned.
    pub fn single_page_wheel(&mut self, unit: egui::MouseWheelUnit, dy: f32, phase: egui::TouchPhase, now: f64, can_turn: bool) -> bool {
        if self.layout != PageLayout::Single {
            return false;
        }
        let Some(forward) = self.wheel.feed(unit, dy, phase, now, can_turn) else { return false };
        let before = self.current;
        self.step_page(forward);
        self.current != before
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

    /// Zoom keeping the centre of the view still.
    pub fn set_zoom(&mut self, zoom: f32) {
        let centre = self.viewport_screen.center();
        self.zoom_at(zoom, centre);
    }

    /// Zoom keeping the document point under `screen_pos` still (pinch, ⌘-scroll).
    pub fn zoom_at(&mut self, zoom: f32, screen_pos: Pos2) {
        let anchor = self.screen_rects.iter().find(|(_, r)| r.expand(GAP).contains(screen_pos)).map(|(p, r)| {
            let f = (screen_pos - r.min) / r.size();
            (*p, f.x.clamp(0.0, 1.0), f.y.clamp(0.0, 1.0), screen_pos - self.viewport_screen.min)
        });
        self.zoom = zoom.clamp(0.08, 64.0);
        self.fit = Fit::None;
        match anchor {
            Some(a) => self.zoom_anchor = Some(a),
            // Nothing on screen yet (e.g. a launch option): align the current page instead.
            None => self.goto = Some((self.current, 0.0)),
        }
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

    /// Pull finished renders into textures.
    pub fn receive(&mut self, ctx: &egui::Context, pool: &RenderPool) {
        let mut got = false;
        // Inline (single-threaded, e.g. web) rendering happens inside try_recv: one page per frame.
        let budget = if pool.is_inline() { 1 } else { usize::MAX };
        let mut n = 0;
        while n < budget
            && let Some(r) = pool.try_recv()
        {
            n += 1;
            got = true;
            if r.request.kind == RequestKind::Text {
                match r.text {
                    Some(t) => {
                        self.texts.insert(r.request.page, t);
                        self.refresh_find_page(r.request.page);
                    }
                    None => {
                        log::warn!("page {} text: {}", r.request.page + 1, r.error.unwrap_or_default());
                        self.text_failed.insert(r.request.page);
                    }
                }
                continue;
            }
            if let Some(e) = r.error {
                log::warn!("page {}: {e}", r.request.page + 1);
                self.errors.insert(r.request.page, e);
                continue;
            }
            let img = egui::ColorImage::from_rgba_premultiplied([r.width as usize, r.height as usize], &r.rgba);
            let page = r.request.page;
            if let Some(t) = r.request.tile {
                let tex = ctx.load_texture(format!("tile-{:?}-{page}-{}-{}", self.id, t.x, t.y), img, TextureOptions::LINEAR);
                self.tiles.insert((page, t.x / TILE, t.y / TILE), (r.request.tag, tex));
                continue;
            }
            if r.request.tag & THUMB_TAG != 0 {
                let tex = ctx.load_texture(format!("thumb-{:?}-{page}", self.id), img, TextureOptions::LINEAR);
                self.thumbs.insert(page, tex);
                self.stale_thumbs.remove(&page);
            } else {
                let tex = ctx.load_texture(format!("page-{:?}-{page}", self.id), img, TextureOptions::LINEAR);
                self.pages.insert(page, PageTex { tag: r.request.tag, tex });
                self.waiting_since.remove(&page);
            }
        }
        if got {
            ctx.request_repaint();
        }
    }

    fn fit_zoom(&mut self, info: &DocInfo) {
        let largest = |side: fn((f32, f32)) -> f32| info.pages.iter().map(|p| side(self.display_size(p))).fold(1.0, f32::max);
        let max_w = largest(|s| s.0);
        // Single-page view fits the page it shows. The scrolling views fit their largest page,
        // so the zoom holds still while pages of other sizes scroll past.
        let (w, h) = match self.layout {
            PageLayout::Single => info.pages.get(self.current).map_or_else(|| (max_w, largest(|s| s.1)), |p| self.display_size(p)),
            PageLayout::Continuous | PageLayout::TwoUp => (max_w, largest(|s| s.1)),
        };
        let avail_w = (self.viewport_w - 2.0 * SIDE).max(100.0);
        let per_row = if self.layout == PageLayout::TwoUp { 2.0 } else { 1.0 };
        match self.fit {
            Fit::Width => self.zoom = (avail_w - GAP * (per_row - 1.0)) / (max_w * PT * per_row),
            Fit::Page => {
                let zw = (avail_w - GAP * (per_row - 1.0)) / (w * PT * per_row);
                let zh = (self.viewport_h - 2.0 * MARGIN) / (h * PT);
                self.zoom = zw.min(zh);
            }
            Fit::Height => self.zoom = (self.viewport_h - 2.0 * MARGIN) / (h * PT),
            Fit::None => {}
        }
        self.zoom = self.zoom.clamp(0.08, 64.0);
    }

    /// Page rects in content coordinates (origin at the scroll content's top-left).
    fn layout(&self, info: &DocInfo, content_w: f32) -> Vec<Rect> {
        let s = self.zoom * PT;
        let mut rects = Vec::with_capacity(info.pages.len());
        let mut y = MARGIN;
        match self.layout {
            PageLayout::Continuous | PageLayout::Single => {
                for p in &info.pages {
                    let (w, h) = self.display_size(p);
                    let size = vec2(w * s, h * s);
                    rects.push(Rect::from_min_size(pos2(((content_w - size.x) / 2.0).max(SIDE), y), size));
                    y += size.y + GAP;
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
                    let sizes: Vec<Vec2> = pair.iter().map(|p| self.display_size(p)).map(|(w, h)| vec2(w * s, h * s)).collect();
                    let row_w: f32 = sizes.iter().map(|v| v.x).sum::<f32>() + GAP * (sizes.len() as f32 - 1.0);
                    let row_h = sizes.iter().map(|v| v.y).fold(0.0, f32::max);
                    let mut x = if self.cover && ri == 0 { (content_w / 2.0 + GAP / 2.0).max(SIDE) } else { ((content_w - row_w) / 2.0).max(SIDE) };
                    for size in sizes {
                        rects.push(Rect::from_min_size(pos2(x, y + (row_h - size.y) / 2.0), size));
                        x += size.x + GAP;
                    }
                    y += row_h + GAP;
                }
            }
        }
        rects
    }

    fn render_scale(&self, ppp: f32) -> f32 {
        // Quantize so tiny fit-width changes don't trigger re-renders.
        ((self.zoom * PT * ppp) * 64.0).round() / 64.0
    }
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

pub fn shortcuts(view: &mut DocView, ctx: &egui::Context) {
    use egui::{Key, KeyboardShortcut, Modifiers};
    if ctx.input_mut(|i| i.consume_shortcut(&KeyboardShortcut::new(Modifiers::COMMAND, Key::F))) {
        view.open_find();
    }
    if ctx.egui_wants_keyboard_input() {
        return;
    }
    let cmd = |k| KeyboardShortcut::new(Modifiers::COMMAND, k);
    let pressed = |s: KeyboardShortcut| ctx.input_mut(|i| i.consume_shortcut(&s));
    // Rotation first: egui matches ⌘+ loosely with respect to Shift.
    if pressed(KeyboardShortcut::new(Modifiers::COMMAND | Modifiers::SHIFT, Key::Plus))
        || pressed(KeyboardShortcut::new(Modifiers::COMMAND | Modifiers::SHIFT, Key::Equals))
    {
        view.rotate_view(true);
    }
    if pressed(KeyboardShortcut::new(Modifiers::COMMAND | Modifiers::SHIFT, Key::Minus)) {
        view.rotate_view(false);
    }
    if pressed(cmd(Key::Plus)) || pressed(cmd(Key::Equals)) {
        view.zoom_step(true);
    }
    if pressed(cmd(Key::Minus)) {
        view.zoom_step(false);
    }
    if pressed(cmd(Key::Num0)) {
        view.fit = Fit::Page;
        view.goto = Some((view.current, 0.0));
    }
    if pressed(cmd(Key::Num1)) {
        view.set_zoom(1.0);
    }
    if pressed(cmd(Key::Num2)) {
        view.fit = Fit::Width;
        view.goto = Some((view.current, 0.0));
    }
    if pressed(cmd(Key::G)) {
        view.find_step(true);
    }
    if pressed(cmd(Key::OpenBracket)) {
        view.view_history(false);
    }
    if pressed(cmd(Key::CloseBracket)) {
        view.view_history(true);
    }
    if pressed(KeyboardShortcut::new(Modifiers::COMMAND | Modifiers::SHIFT, Key::G)) {
        view.find_step(false);
    }
    // ⌘C arrives as a Copy event on most platforms.
    let copy = ctx.input(|i| i.events.iter().any(|e| matches!(e, egui::Event::Copy)));
    if copy && let Some(text) = view.selected_text() {
        ctx.copy_text(text);
    }
    let key = |k| ctx.input(|i| i.key_pressed(k));
    if key(Key::Escape) {
        if view.selection.is_some() {
            view.selection = None;
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
    // As in Acrobat: → / ← go to the next / previous page in every layout (#185), and so do
    // ⌘Page Down / ⌘Page Up, or plain Page Down / Page Up in single-page view.
    let command = ctx.input(|i| i.modifiers.command);
    let scrolls = view.layout != PageLayout::Single;
    if key(Key::ArrowRight) || (key(Key::PageDown) && (command || !scrolls)) {
        view.step_page(true);
    }
    if key(Key::ArrowLeft) || (key(Key::PageUp) && (command || !scrolls)) {
        view.step_page(false);
    }
    // ↓ / ↑ scroll a line, and Page Down / Page Up a screen where the pages scroll.
    if !command {
        let screen = (view.viewport_h - KEY_SCROLL_LINE).max(KEY_SCROLL_LINE);
        let steps = [(Key::ArrowDown, KEY_SCROLL_LINE), (Key::ArrowUp, -KEY_SCROLL_LINE), (Key::PageDown, screen), (Key::PageUp, -screen)];
        for (k, by) in steps {
            if key(k) && (scrolls || matches!(k, Key::ArrowDown | Key::ArrowUp)) {
                view.key_scroll += by;
            }
        }
    }
}

/// How far ↓ / ↑ scroll: a mouse-wheel line on the desktop. Page Down / Page Up keep this much
/// of the previous screen in view.
const KEY_SCROLL_LINE: f32 = 40.0;

pub fn document_area(app: &mut PdfCraftApp, index: usize, ui: &mut egui::Ui) {
    let t = Tokens::get(ui.ctx());
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
    let Some(doc) = app.session.get(app.views[index].id) else { return };
    let info = &doc.info;
    if info.pages.is_empty() {
        app.views[index].auto_scroll.cancel();
        ui.centered_and_justified(|ui| ui.label(tl!("This document has no pages.")));
        return;
    }
    let want_thumbs = app.right == Some(RightPanel::Pages) || app.views[index].organize || app.dialog == Some(crate::Dialog::Print);
    // The Prepare a form panel is open (or a field tool is picked): fields are edited, not filled.
    let preparing = app.is_preparing();
    // Edit a PDF: added text and images can be selected, moved and edited.
    let editing_content = (app.left_open && app.left == crate::LeftPanel::Tool("edit")) || app.quick_tool == QuickTool::AddText;
    let text_style = app.text_style.clone();
    let view = &mut app.views[index];
    // Opened without the owner password and something is restricted.
    let secured = doc.security_summary().is_some_and(|s| !(s.owner || (s.permissions.modify() && s.permissions.assemble())));
    let repaired = !doc.repair_log().is_empty();
    match notices(view, doc, secured, repaired, crate::sign_ui::banner(&doc.signatures), ui, &t) {
        Some(Notice::Repairs) => app.dialog = Some(crate::Dialog::Properties(crate::PropsTab::Advanced)),
        Some(Notice::Security) => app.dialog = Some(crate::Dialog::Properties(crate::PropsTab::Security)),
        Some(Notice::Signatures) => app.right = Some(RightPanel::Signatures),
        None => {}
    }
    // No dialog, close prompt or palette over the page: only then does page input count.
    let unobstructed = app.dialog.is_none() && app.close_request.is_none() && !app.palette_open;
    if view.organize {
        organize_grid(view, info, &doc.renderer, doc.allows_assembly(), unobstructed, ui, &t);
        return;
    }

    let avail = ui.available_rect_before_wrap();
    view.viewport_w = avail.width();
    view.viewport_h = avail.height();
    view.fit_zoom(info);

    // Pinch / ctrl+scroll zoom, anchored on the current page.
    let zoom_delta = ui.input(|i| i.zoom_delta());
    if (zoom_delta - 1.0).abs() > 0.001 && ui.rect_contains_pointer(avail) {
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

    let max_w = info.pages.iter().map(|p| view.display_size(p).0).fold(0.0, f32::max)
        * view.zoom
        * PT
        * if view.layout == PageLayout::TwoUp { 2.0 } else { 1.0 };
    let content_w = (max_w + 2.0 * SIDE).max(avail.width());
    let rects = view.layout(info, content_w);
    let middle_gesture = view.auto_scroll.blocks_input();
    // Single page: when the whole page fits, the wheel would do nothing, so it turns pages
    // instead; zoomed in far enough to pan, it pans. Touch drags are untouched: with nothing
    // to pan, touch users turn pages with the rail buttons, the page box and the arrow keys.
    // Runs before `visible_pages` so the frame that turns the page draws it.
    if view.layout == PageLayout::Single {
        // Within a point, so layout rounding can't stop a fitting page from turning.
        let fits = rects.get(view.current.min(rects.len().saturating_sub(1))).is_some_and(|r| r.height() + 2.0 * MARGIN <= avail.height() + 1.0);
        let can_turn = fits && !middle_gesture && unobstructed && ui.rect_contains_pointer(avail);
        // Every wheel event goes to the pager, so it follows each trackpad touch to its end
        // even while the page can't turn.
        ui.input(|i| {
            for e in &i.events {
                if let egui::Event::MouseWheel { unit, delta, phase, modifiers } = e {
                    // Zooming (⌘ or Ctrl) and sideways scrolling (Shift) never turn pages.
                    let dy = if modifiers.command || modifiers.ctrl || modifiers.shift { 0.0 } else { delta.y };
                    view.single_page_wheel(*unit, dy, *phase, i.time, can_turn);
                }
            }
        });
        if can_turn {
            // Paging owns vertical wheel motion here. A diagonal gesture keeps its sideways
            // part, as `ScrollArea` itself handles each axis.
            ui.input_mut(|i| i.smooth_scroll_delta.y = 0.0);
        }
    }
    let visible_pages: Vec<usize> = match view.layout {
        PageLayout::Single => vec![view.current.min(rects.len() - 1)],
        _ => (0..rects.len()).collect(),
    };
    let (y_shift, content_h) = match view.layout {
        PageLayout::Single => {
            let r = rects[visible_pages[0]];
            (r.top() - MARGIN, r.height() + 2.0 * MARGIN)
        }
        _ => (0.0, rects.last().map(|r| r.bottom() + MARGIN).unwrap_or(0.0)),
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
    if let Some((page, fx, fy, rel)) = view.zoom_anchor.take() {
        let r = rects[page.min(rects.len() - 1)];
        let point = pos2(r.left() + fx * r.width(), r.top() - y_shift + fy * r.height());
        scroll = scroll.scroll_offset(vec2((point.x - rel.x).max(0.0), (point.y - rel.y).max(0.0)));
    } else if let Some((page, frac)) = view.goto.take() {
        let page = page.min(rects.len() - 1);
        let r = rects[page];
        scroll = scroll.vertical_scroll_offset((r.top() - y_shift - GAP + frac * r.height()).max(0.0));
    }
    let ppp = ui.ctx().pixels_per_point();
    let scale = view.render_scale(ppp);
    let tag = (scale * 1000.0) as u64;
    let hand = app.quick_tool == QuickTool::Hand;
    let tool = app.quick_tool;
    // Text selection runs for the Select tool and for the markup tools (highlight…).
    let selects_text = match tool {
        QuickTool::Comment(t) => t.markup().is_some() || t == comments::CommentTool::ReplaceText,
        QuickTool::Select => !preparing,
        QuickTool::Redact => true,
        QuickTool::Hand
        | QuickTool::Measure(_)
        | QuickTool::Crop
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
    let allowed = doc.allows_annotation();
    let comments_hidden = doc.comments_hidden();
    let form = doc.form.clone();
    let can_fill = doc.allows_form_filling();
    let can_crop = doc.allows_assembly();
    let mut open_boxes = false;
    let signature = app.signature.clone();
    let initials = app.initials.clone();
    let mut open_initials = false;
    let author = app.comment_prefs.author.clone();
    let today = app.session.today();
    let by_line = app.session.stamp_by_line(&author);
    let mut stamp_placed = false;
    let mut image_action: Option<crate::edit_text_ui::ImageAction> = None;
    let custom_stamp = match app.quick_tool {
        QuickTool::CustomStamp(i) => app.custom_stamps.get(i).cloned(),
        _ => None,
    };
    let mut open_signature = false;
    let mut hover_text: Option<(Pos2, String)> = None;
    let mut clicked_link: Option<LinkTarget> = None;
    let mut canvas_action: Option<comments::CanvasAction> = None;
    let mut field_menu: Option<FieldMenu> = None;
    let mut open_props: Option<(usize, usize)> = None;
    let mut field_props = false;
    let mut field_placed = false;
    let can_modify = doc.allows_modification();
    if preparing {
        crate::prepare::after_refresh(view, &form);
    } else {
        view.prepare.selected = None;
    }
    let added = doc.added.clone();
    let doc_links = if tool == QuickTool::Link { doc.links.clone() } else { Vec::new() };
    if editing_content {
        crate::content_ui::after_refresh(view, &added);
    } else {
        view.content.selected = None;
    }

    let out = scroll.show_viewport(ui, |ui, viewport| {
        // egui's drag responses accept every pointer button. Keep a wheel gesture from also
        // selecting text, drawing a mark, or panning with the Hand tool.
        if middle_gesture {
            let opacity = ui.opacity();
            ui.disable();
            // Only interactions pause; the document must keep its original colours.
            ui.set_opacity(opacity);
        }
        // Middle-button auto-scroll and the keyboard (positive y moves the content down).
        let delta = auto_delta - vec2(0.0, key_scroll);
        if delta != Vec2::ZERO {
            ui.scroll_with_delta_animation(delta, egui::style::ScrollAnimation::none());
        }
        let (resp_rect, resp) = ui.allocate_exact_size(vec2(content_w, content_h), Sense::click_and_drag());
        // The Hand tool pans: the content widget takes every drag, so scroll by its delta.
        if hand {
            if resp.dragged() {
                ui.scroll_with_delta(resp.drag_delta());
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
        let pointer = ui.input(|i| i.pointer.hover_pos());
        for &i in &visible_pages {
            let r = rects[i].translate(origin.to_vec2());
            if !r.intersects(visible.expand(400.0)) {
                continue;
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
            }
            // Pages shown equally (two rows wholly on screen) differ only by rounding: the
            // topmost one counts.
            if overlap > best_overlap + TIE {
                best_overlap = overlap;
                current = i;
            }
            let xf = PageXform { rect: r, rot: view.rotation, pw: info.pages[i].width.max(1.0), ph: info.pages[i].height.max(1.0) };
            // Soft shadow + paper.
            painter.add(egui::epaint::Shadow { offset: [0, 3], blur: 14, spread: 0, color: t.page_shadow }.as_shape(r, CornerRadius::ZERO));
            painter.rect_filled(r, CornerRadius::ZERO, Color32::WHITE);
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
                let tiled = pw_pt.max(ph_pt) * scale > TILE_THRESHOLD;
                // Whole-page raster: sharp when small, a low-res backdrop when tiled.
                let (want_scale, want_tag) = if tiled {
                    let bs = BASE_SIDE / pw_pt.max(ph_pt);
                    (bs, (bs * 1000.0) as u64)
                } else {
                    (scale, tag)
                };
                match view.pages.get(&i) {
                    Some(p) => {
                        xf.paint_image(painter, p.tex.id(), 0.0, 0.0, 1.0, 1.0);
                        if p.tag != want_tag {
                            wanted.push((i, want_scale, want_tag, None));
                        }
                    }
                    None => {
                        wanted.push((i, want_scale, want_tag, None));
                        let now = ui.input(|inp| inp.time);
                        let since = *view.waiting_since.entry(i).or_insert(now);
                        let msg = if now - since > 6.0 { "Still rendering — this page is unusually complex…" } else { "Rendering…" };
                        painter.text(r.center(), Align2::CENTER_CENTER, msg, theme::regular(12.0), t.text_faint);
                    }
                }
                if tiled && r.intersects(visible) {
                    // Device-pixel geometry of the scaled page, and the visible part of it
                    // (found by mapping the visible screen corners back into the page). Sides round
                    // up as whole-page rasters do, so the last partial row and column get tiles.
                    let (dw, dh) = (device_pixels(pw_pt, scale), device_pixels(ph_pt, scale));
                    let vis = r.intersect(visible);
                    let corners = [vis.left_top(), vis.right_top(), vis.right_bottom(), vis.left_bottom()].map(|c| xf.screen_to_norm(c));
                    let (u0, u1) = corners.iter().fold((1.0f32, 0.0f32), |(a, b), c| (a.min(c.0), b.max(c.0)));
                    let (v0, v1) = corners.iter().fold((1.0f32, 0.0f32), |(a, b), c| (a.min(c.1), b.max(c.1)));
                    let (vx0, vy0) = ((u0.max(0.0) * dw as f32) as u32, (v0.max(0.0) * dh as f32) as u32);
                    let (vx1, vy1) = (((u1.min(1.0) * dw as f32).ceil() as u32).min(dw), ((v1.min(1.0) * dh as f32).ceil() as u32).min(dh));
                    for ty in vy0 / TILE..=(vy1.saturating_sub(1)) / TILE {
                        for tx in vx0 / TILE..=(vx1.saturating_sub(1)) / TILE {
                            let (x, y) = (tx * TILE, ty * TILE);
                            let (w, h) = (TILE.min(dw.saturating_sub(x)), TILE.min(dh.saturating_sub(y)));
                            if w == 0 || h == 0 {
                                continue;
                            }
                            match view.tiles.get(&(i, tx, ty)) {
                                Some((ttag, tex)) if *ttag == tag => {
                                    let (fw, fh) = (dw as f32, dh as f32);
                                    xf.paint_image(painter, tex.id(), x as f32 / fw, y as f32 / fh, (x + w) as f32 / fw, (y + h) as f32 / fh);
                                }
                                _ => wanted.push((i, scale, tag, Some(Tile { x, y, w, h }))),
                            }
                        }
                    }
                }
            }
            painter.rect_stroke(r, CornerRadius::ZERO, Stroke::new(0.5, t.border.gamma_multiply(0.8)), egui::StrokeKind::Outside);

            // Comments: tools, selection, moving and resizing come before text selection.
            let pcx = comments::PageCx { page: i, xf: &xf, info, tool, prefs, allowed, hidden: comments_hidden };
            if let QuickTool::Measure(measure_tool) = tool {
                crate::measure_ui::page_input(ui, &resp, doc, view, &pcx, measure_tool);
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
            if let QuickTool::Fill(ft) = tool
                && allowed
            {
                match crate::fill_sign::page_input(ui, &resp, &xf, i, info, ft, view, signature.as_ref(), initials.as_ref(), &author, today) {
                    Some(crate::fill_sign::FillAction::Edit(e)) => view.pending_edit = Some(*e),
                    Some(crate::fill_sign::FillAction::CreateSignature) => open_signature = true,
                    Some(crate::fill_sign::FillAction::CreateInitials) => open_initials = true,
                    None => {}
                }
            }
            if matches!(tool, QuickTool::SignArea { .. }) {
                crate::sign_ui::page_input(ui, &resp, &xf, i, info, view);
            }
            if matches!(tool, QuickTool::MarqueeZoom | QuickTool::Snapshot) {
                crate::zoom_snap::page_input(ui, &resp, &xf, i, view);
            }
            if tool == QuickTool::Crop && crate::crop::page_input(ui, &resp, &xf, i, info, view, can_crop) {
                view.current = i;
                open_boxes = true;
            }
            let on_field = if preparing {
                let field_tool = match tool {
                    QuickTool::Field(f) => Some(f),
                    _ => None,
                };
                let o = crate::prepare::page_input(ui, &resp, &xf, i, info, &form, field_tool, can_modify, view);
                field_props |= o.properties;
                field_placed |= o.placed;
                o.consumed || field_tool.is_some()
            } else {
                tool == QuickTool::Select && crate::forms_ui::page_input(ui, &resp, &xf, i, info, &form, can_fill, view)
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
            let on_content = editing_content && can_modify && {
                crate::content_ui::page_input(ui, &resp, &xf, i, info, &added, tool == QuickTool::AddText, &text_style, view)
                    || tool == QuickTool::AddText
            };
            let on_edit_text = tool == QuickTool::EditText && can_modify && {
                let generation = doc.edit_generation();
                let lines = match view.edit_lines.get(&i) {
                    Some((g, l)) if *g == generation => l.clone(),
                    _ => {
                        let l = doc.text_blocks(i);
                        view.edit_lines.insert(i, (generation, l.clone()));
                        l
                    }
                };
                let images = match view.edit_images.get(&i) {
                    Some((g, l)) if *g == generation => l.clone(),
                    _ => {
                        let l = doc.page_images(i);
                        view.edit_images.insert(i, (generation, l.clone()));
                        l
                    }
                };
                // Images first (they can sit under text boxes' corners); then paragraphs.
                crate::edit_text_ui::image_input(ui, &resp, &xf, i, info, &images, view, &mut image_action)
                    || crate::edit_text_ui::page_input(ui, &resp, &xf, i, info, &lines, view)
            };
            let on_link = tool == QuickTool::Link && can_modify && crate::link_ui::page_input(ui, &resp, &xf, i, info, &doc_links, view);
            let consumed = on_edit_text || on_link || on_content || boxing || on_field || comments::page_input(ui, &resp, &pcx, view);

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
                if let Some(sel) = view.selection.filter(|s| s.page == i) {
                    for lr in text.line_rects(sel.range()) {
                        painter.rect_filled(to_screen(lr), CornerRadius::same(1), Color32::from_rgba_unmultiplied(0x3A, 0x7B, 0xF0, 70));
                    }
                }
                if selects_text
                    && !consumed
                    && let Some(p) = pointer.filter(|p| r.contains(*p))
                {
                    let (vx, vy) = xf.screen_to_view(p);
                    let over_text = text.glyphs.iter().any(|g| vx >= g.rect[0] && vx <= g.rect[2] && vy >= g.rect[1] && vy <= g.rect[3]);
                    let over_link = info.links.iter().any(|l| l.page == i && xf.user_rect(info, i, l.rect).contains(p));
                    if over_text && !over_link {
                        ui.ctx().set_cursor_icon(egui::CursorIcon::Text);
                    }
                    let press_here = ui.input(|inp| inp.pointer.press_origin()).is_some_and(|o| r.contains(o));
                    if resp.drag_started() && press_here && !over_link {
                        let origin = ui.input(|inp| inp.pointer.press_origin()).unwrap_or(p);
                        let (ox, oy) = xf.screen_to_view(origin);
                        view.selection = text.nearest(ox, oy).map(|a| Selection { page: i, anchor: a, head: a });
                    }
                    if resp.dragged()
                        && let Some(sel) = view.selection.as_mut().filter(|s| s.page == i)
                        && let Some(h) = text.nearest(vx, vy)
                    {
                        sel.head = h;
                    }
                    if resp.double_clicked() && over_text {
                        if let Some(a) = text.nearest(vx, vy) {
                            // Expand to the word: stop at inferred spaces, explicit spaces and line ends.
                            let is_break = |k: usize| text.glyphs[k].text.trim().is_empty();
                            let mut s0 = a;
                            while s0 > 0 && !text.space_before[s0] && text.line_of[s0 - 1] == text.line_of[s0] && !is_break(s0 - 1) {
                                s0 -= 1;
                            }
                            let mut e = a;
                            while e + 1 < text.glyphs.len() && !text.space_before[e + 1] && text.line_of[e + 1] == text.line_of[e] && !is_break(e + 1)
                            {
                                e += 1;
                            }
                            view.selection = Some(Selection { page: i, anchor: s0, head: e });
                        }
                    } else if resp.clicked() && !over_link {
                        view.selection = None;
                    }
                }
            }

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
            if tool == QuickTool::Link {
                crate::link_ui::paint(ui, painter, &xf, i, info, &doc_links, view);
            }
            if preparing {
                crate::prepare::paint_page(ui, painter, &xf, i, info, &form, view);
            } else {
                crate::forms_ui::paint_page(ui, painter, &xf, i, info, &form, view);
            }

            // Form-field highlight (Acrobat's "Highlight existing fields"); required fields get a
            // red border.
            if view.highlight_fields {
                for f in form.iter() {
                    let required = f.has(pdfcraft_engine::field_flags::REQUIRED);
                    for w in f.widgets.iter().filter(|w| w.page == Some(i) && !w.hidden) {
                        let r = w.rect;
                        let sr = xf.user_rect(info, i, [r[0] as f32, r[1] as f32, r[2] as f32, r[3] as f32]);
                        painter.rect_filled(sr, CornerRadius::same(1), Color32::from_rgba_unmultiplied(0x6E, 0x8E, 0xF5, 48));
                        let (width, color) =
                            if required { (2.0, Color32::from_rgb(0xE3, 0x22, 0x22)) } else { (1.0, Color32::from_rgb(0x6E, 0x8E, 0xF5)) };
                        painter.rect_stroke(sr, CornerRadius::same(1), Stroke::new(width, color), egui::StrokeKind::Inside);
                    }
                }
            }
            // Link hover + click.
            if let Some(p) = pointer {
                for l in info.links.iter().filter(|l| l.page == i) {
                    let sr = xf.user_rect(info, i, l.rect);
                    if sr.contains(p) {
                        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
                        let label = match &l.target {
                            LinkTarget::Page(n) => {
                                crate::i18n::fmt(tl!("Go to page {p}"), &[("p", info.pages.get(*n).map(|p| p.label.as_str()).unwrap_or("?"))])
                            }
                            LinkTarget::Uri(u) => u.clone(),
                            LinkTarget::SetLayers { .. } => tl!("Set layer visibility").to_string(),
                            LinkTarget::Other(s) => crate::i18n::fmt(tl!("{s} action"), &[("s", s)]),
                        };
                        hover_text = Some((p, label));
                        if resp.clicked() && tool == QuickTool::Select && !consumed {
                            clicked_link = Some(l.target.clone());
                        }
                    }
                }
                // Annotation hover shows the comment, as Acrobat's popups do.
                let gesturing = view.comments.gesture.is_some();
                for a in info.annotations.iter().filter(|a| a.page == i && a.in_reply_to.is_none() && !gesturing) {
                    let sr = xf.user_rect(info, i, a.rect);
                    if sr.contains(p) && hover_text.is_none() {
                        let who = a.author.clone().unwrap_or_else(|| a.subtype.clone());
                        let body = a.contents.clone().unwrap_or_default();
                        hover_text = Some((p, if body.is_empty() { who } else { format!("{who}\n{body}") }));
                    }
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
        }
        // The page navigated to stays current while no page shows more of itself, so the next
        // page step starts from it rather than from a page further down the screen (#188).
        if current_overlap >= best_overlap - TIE {
            current = view.current;
        }
        if view.layout != PageLayout::Single {
            view.current = current;
            if !ui.memory(|m| m.has_focus(egui::Id::new("page-input"))) {
                view.page_input = (current + 1).to_string();
            }
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
            canvas_action = comments::context_menu(ui, view, info, prefs, allowed);
        });
        (wanted, visible_now)
    });

    view.auto_scroll.paint(ui, avail);

    // Bound texture memory: keep sharp rasters only near the current page.
    if view.pages.len() > 24 {
        let cur = view.current;
        view.pages.retain(|&p, _| p.abs_diff(cur) <= 8);
    }
    // Schedule renders: visible pages first (nearest the current page), then thumbnails.
    let (mut wanted, visible_now) = out.inner;
    let cur = view.current;
    // Nearest pages first; for each page, the backdrop before its tiles.
    wanted.sort_by_key(|w| ((w.0 as isize - cur as isize).unsigned_abs(), w.3.is_some()));
    // Tiles for other zoom levels or far-away pages are useless: free them.
    view.tiles.retain(|(p, _, _), (t, _)| *t == tag && p.abs_diff(cur) <= 2);
    let mut queue: Vec<RenderRequest> =
        wanted.iter().map(|&(page, scale, tag, tile)| RenderRequest { page, kind: RequestKind::Pixels, tile, scale, tag }).collect();
    // Text layers: visible pages for selection, every page while a search is active.
    let need_text = |p: &usize| !view.texts.contains_key(p) && !view.text_failed.contains(p);
    let mut text_pages: Vec<usize> = if hand { Vec::new() } else { visible_now.iter().copied().filter(need_text).collect() };
    if view.find.as_ref().is_some_and(|f| !f.case_query.trim().is_empty()) {
        let n = info.pages.len();
        let rest: Vec<usize> = (0..n).map(|k| (cur + k) % n).filter(need_text).filter(|p| !text_pages.contains(p)).collect();
        text_pages.extend(rest);
    }
    queue.extend(text_pages.into_iter().map(|page| RenderRequest { page, kind: RequestKind::Text, tile: None, scale: 1.0, tag: TEXT_TAG }));
    if want_thumbs {
        let s = THUMB_W * ppp / info.pages.iter().map(|p| p.width).fold(1.0, f32::max);
        for page in 0..info.pages.len() {
            if (!view.thumbs.contains_key(&page) || view.stale_thumbs.contains(&page)) && !view.errors.contains_key(&page) {
                queue.push(RenderRequest { page, kind: RequestKind::Pixels, tile: None, scale: s, tag: THUMB_TAG });
            }
        }
    }
    if queue != view.last_queue {
        doc.renderer.set_queue(queue.clone());
        view.last_queue = queue;
    }
    if !view.last_queue.is_empty() {
        ui.ctx().request_repaint_after(std::time::Duration::from_millis(30));
    }

    if let Some((pos, text)) = hover_text {
        egui::Area::new(egui::Id::new("canvas-hover")).order(egui::Order::Tooltip).fixed_pos(pos + vec2(14.0, 16.0)).show(ui.ctx(), |ui| {
            egui::Frame::popup(ui.style()).show(ui, |ui| {
                ui.set_max_width(320.0);
                ui.label(text);
            });
        });
    }
    find_bar(view, info.pages.len(), avail, ui, &t);
    if let Some(e) = comments::composer(ui.ctx(), view, info, prefs) {
        view.pending_edit = Some(e);
    }
    if let Some(e) = crate::edit_text_ui::overlay(ui.ctx(), view, info) {
        view.pending_edit = Some(e);
    }
    if let Some(e) = crate::forms_ui::overlay(ui.ctx(), view, info, &form, today) {
        view.pending_edit = Some(e);
    }
    if let Some(e) = crate::fill_sign::type_box(ui.ctx(), view, info, &author) {
        view.pending_edit = Some(e);
    }
    let (typed, text_done) = crate::content_ui::editor(ui.ctx(), view, info, &added);
    if typed.is_some() {
        view.pending_edit = typed;
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
        Some(comments::CanvasAction::OpenComments) => app.right = Some(RightPanel::Comments),
        Some(comments::CanvasAction::Properties(p, i)) => open_props = Some((p, i)),
        None => {}
    }
    match clicked_link {
        Some(LinkTarget::Page(p)) => view.go_to_page(p),
        Some(LinkTarget::Uri(u)) => app.request_document_url(&u, crate::LinkOrigin::Link),
        Some(LinkTarget::SetLayers { changes, preserve_rb }) => set_layers(app, index, &changes, preserve_rb),
        Some(LinkTarget::Other(s)) => app.notify_fmt("{s} actions run in the JavaScript engine (M6)", &[("s", &s)]),
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
        None => {}
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
    quick_bar(app, avail, ui);
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
                        if icons::button(ui, "chevron-up", 26.0, false, tl!("Previous (⇧⌘G)")).clicked() {
                            step = Some(false);
                        }
                        if icons::button(ui, "chevron-down", 26.0, false, tl!("Next (⌘G)")).clicked() {
                            step = Some(true);
                        }
                        let opts = icons::button(ui, "settings-2", 26.0, find.case_sensitive || find.whole_words, tl!("Find options"));
                        egui::Popup::menu(&opts).show(|ui| {
                            let a = ui.checkbox(&mut find.whole_words, tl!("Whole words only")).changed();
                            let b = ui.checkbox(&mut find.case_sensitive, tl!("Case-sensitive")).changed();
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
    Security,
    Signatures,
    Repairs,
}

/// The notice bar above the pages: the signature status first (Acrobat's signature bar), then
/// security, forms and warnings.
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
    if let Some((icon, color, template, arg)) = signed {
        let mut open = false;
        egui::Frame::NONE.fill(t.accent_soft).inner_margin(egui::Margin::symmetric(14, 7)).show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.add(icons::image(icon, 16.0, color));
                ui.label(egui::RichText::new(crate::i18n::fmt(tl!(template), &[("by", &arg)])).color(t.text));
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if crate::widgets::pill_button(ui, tl!("Signature panel"), false).clicked() {
                        open = true;
                    }
                });
            });
        });
        return open.then_some(Notice::Signatures);
    }
    if view.notice_dismissed {
        return None;
    }
    let mut open_security = false;
    let mut open_repairs = false;
    let msg = if secured {
        Some(("lock", tl!("This document is secured. Some changes are restricted by its security settings.").to_string(), false))
    } else if let Some(x) = xfa {
        let lang = crate::i18n::current();
        let pages = crate::i18n::trn(lang, x.pages as u64, "{n} page", "{n} pages");
        let fields = crate::i18n::trn(lang, x.fields as u64, "{n} field", "{n} fields");
        let mut text =
            crate::i18n::fmt(tl!("Dynamic XFA form laid out from its template: {pages}, {fields}."), &[("pages", &pages), ("fields", &fields)]);
        if !x.warnings.is_empty() {
            text.push(' ');
            text.push_str(&x.warnings.join("; "));
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
    } else if !info.fields.is_empty() {
        Some((
            "text-cursor-input",
            crate::i18n::fmt(tl!("This document contains {n} interactive form fields."), &[("n", &info.fields.len().to_string())]),
            true,
        ))
    } else if repaired {
        Some(("bandage", tl!("This file was damaged and has been repaired. Saving keeps the repaired version.").to_string(), false))
    } else if !info.warnings.is_empty() {
        Some(("triangle-alert", info.warnings[0].clone(), false))
    } else {
        None
    };
    let (icon, text, fields) = msg?;
    egui::Frame::NONE.fill(t.accent_soft).inner_margin(egui::Margin::symmetric(14, 7)).show(ui, |ui| {
        ui.horizontal(|ui| {
            ui.add(icons::image(icon, 16.0, t.accent_text));
            ui.label(egui::RichText::new(text).color(t.text));
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if icons::button(ui, "x", 22.0, false, tl!("Dismiss")).clicked() {
                    view.notice_dismissed = true;
                }
                if fields {
                    let label = if view.highlight_fields { tl!("Hide field highlights") } else { tl!("Highlight fields") };
                    if crate::widgets::pill_button(ui, label, view.highlight_fields).clicked() {
                        view.highlight_fields = !view.highlight_fields;
                    }
                }
                if secured && crate::widgets::pill_button(ui, tl!("Security settings"), false).clicked() {
                    open_security = true;
                }
                if repaired && !secured && info.fields.is_empty() && crate::widgets::pill_button(ui, tl!("Details"), false).clicked() {
                    open_repairs = true;
                }
            });
        });
    });
    if open_repairs {
        return Some(Notice::Repairs);
    }
    open_security.then_some(Notice::Security)
}

/// The floating quick-action bar at the left edge of the document area.
fn quick_bar(app: &mut PdfCraftApp, area: Rect, ui: &mut egui::Ui) {
    let t = Tokens::get(ui.ctx());
    let pos = area.left_top() + vec2(14.0, 14.0);
    egui::Area::new(egui::Id::new("quick-bar")).order(egui::Order::Middle).fixed_pos(pos).show(ui.ctx(), |ui| {
        egui::Frame::NONE
            .fill(t.card)
            .stroke(Stroke::new(1.0, t.border))
            .corner_radius(CornerRadius::same(10))
            .shadow(egui::Shadow { offset: [0, 2], blur: 10, spread: 0, color: Color32::from_black_alpha(if t.dark() { 80 } else { 22 }) })
            .inner_margin(egui::Margin::same(4))
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
                        ui.painter().add(egui::Shape::convex_polygon(tri.to_vec(), if active { Color32::WHITE } else { t.text_muted }, Stroke::NONE));
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
                                    if click.hovered() {
                                        ui.painter().rect_filled(row, CornerRadius::same(4), t.hover);
                                    }
                                    icons::paint(ui, Rect::from_min_size(row.min + vec2(8.0, 6.0), vec2(16.0, 16.0)), tool.icon(), 16.0, t.text);
                                    ui.painter().text(
                                        row.left_center() + vec2(34.0, 0.0),
                                        Align2::LEFT_CENTER,
                                        tl!(tool.label()),
                                        theme::regular(13.0),
                                        t.text,
                                    );
                                    if on {
                                        icons::paint(
                                            ui,
                                            Rect::from_min_size(row.right_top() + vec2(-24.0, 7.0), vec2(14.0, 14.0)),
                                            "check",
                                            14.0,
                                            t.accent,
                                        );
                                    }
                                    let click = click.on_hover_cursor(egui::CursorIcon::PointingHand);
                                    let info = tl!(tool.label()).to_string();
                                    click.widget_info(|| egui::WidgetInfo::selected(egui::WidgetType::Button, true, on, info.clone()));
                                    if click.clicked() {
                                        app.execute(tool.command());
                                        ui.close();
                                    }
                                }
                            });
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
                        if current_fill.is_some() { Color32::WHITE } else { t.text_muted },
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
                                if click.hovered() {
                                    ui.painter().rect_filled(row, CornerRadius::same(4), t.hover);
                                }
                                icons::paint(ui, Rect::from_min_size(row.min + vec2(8.0, 6.0), vec2(16.0, 16.0)), tool.icon(), 16.0, t.text);
                                ui.painter().text(
                                    row.left_center() + vec2(34.0, 0.0),
                                    Align2::LEFT_CENTER,
                                    tl!(tool.label()),
                                    theme::regular(13.0),
                                    t.text,
                                );
                                if on {
                                    icons::paint(
                                        ui,
                                        Rect::from_min_size(row.right_top() + vec2(-24.0, 7.0), vec2(14.0, 14.0)),
                                        "check",
                                        14.0,
                                        t.accent,
                                    );
                                }
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
                    if let QuickTool::Comment(tool) = app.quick_tool {
                        let (r, _) = ui.allocate_exact_size(vec2(32.0, 9.0), Sense::hover());
                        ui.painter().hline(r.x_range().shrink(6.0), r.center().y, Stroke::new(1.0, t.divider));
                        comments::quick_bar_controls(ui, tool, &mut app.comment_prefs);
                    }
                });
            });
    });
}

/// The organize toolbar: page operations on the selection (Acrobat's Organize Pages bar).
fn organize_toolbar(view: &mut DocView, info: &DocInfo, editable: bool, ui: &mut egui::Ui, t: &Tokens) {
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
                if icons::button(ui, "rotate-ccw", 30.0, false, "Rotate counterclockwise").clicked() {
                    view.pending_edit = Some(Edit::RotatePages { pages: targets.clone(), degrees: -90 });
                }
                if icons::button(ui, "rotate-cw", 30.0, false, "Rotate clockwise").clicked() {
                    view.pending_edit = Some(Edit::RotatePages { pages: targets.clone(), degrees: 90 });
                }
                let can_delete = targets.len() < n;
                if ui.add_enabled_ui(can_delete, |ui| icons::button(ui, "trash-2", 30.0, false, tl!("Delete pages (Delete)"))).inner.clicked() {
                    view.pending_edit = Some(Edit::DeletePages { pages: targets.clone() });
                }
                if icons::button(ui, "file-plus", 30.0, false, tl!("Insert a blank page after the selection")).clicked() {
                    let c = info.pages[last].crop;
                    let (w, h) = ((c[2] - c[0]).abs().max(1.0) as f64, (c[3] - c[1]).abs().max(1.0) as f64);
                    view.pending_edit = Some(Edit::InsertBlankPage { at: last + 1, width: w, height: h });
                }
                if icons::button(ui, "file-input", 30.0, false, tl!("Insert pages from a file…")).clicked() {
                    view.pending_action = Some(ViewAction::InsertFromFile);
                }
                if icons::button(ui, "file-output", 30.0, false, tl!("Extract pages to a new document")).clicked() {
                    view.pending_action = Some(ViewAction::Extract);
                }
                if icons::button(ui, "scissors", 30.0, false, tl!("Split into files…")).clicked() {
                    view.pending_action = Some(ViewAction::Split);
                }
                // Select ▸ all, odd, even, landscape, portrait pages (Acrobat's page range
                // selection in Organize Pages).
                let sel = icons::button(ui, "list", 30.0, false, tl!("Select pages"));
                egui::Popup::menu(&sel).show(|ui| {
                    use pdfcraft_engine::{PageOrientation as O, PageParity as P, filter_pages};
                    let all: Vec<usize> = (0..n).collect();
                    for (label, parity, orient) in [
                        ("All pages", P::Both, O::Both),
                        ("Odd pages", P::Odd, O::Both),
                        ("Even pages", P::Even, O::Both),
                        ("Landscape pages", P::Both, O::Landscape),
                        ("Portrait pages", P::Both, O::Portrait),
                    ] {
                        if ui.button(tl!(label)).clicked() {
                            view.select_pages(&filter_pages(info, &all, parity, orient));
                            ui.close();
                        }
                    }
                    if ui.button(tl!("None")).clicked() {
                        view.select_pages(&[]);
                        ui.close();
                    }
                });
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
            });
        });
    });
    // Keys act on the selection unless a text field has focus.
    if editable && !ui.ctx().egui_wants_keyboard_input() {
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
fn drop_gap(cells: &[(usize, Rect)], p: Pos2) -> Option<usize> {
    let (i, r) = cells.iter().min_by(|(_, a), (_, b)| a.distance_sq_to_pos(p).total_cmp(&b.distance_sq_to_pos(p)))?;
    Some(if p.x < r.center().x { *i } else { i + 1 })
}

fn organize_grid(view: &mut DocView, info: &DocInfo, pool: &RenderPool, editable: bool, auto_scroll_enabled: bool, ui: &mut egui::Ui, t: &Tokens) {
    let ppp = ui.ctx().pixels_per_point();
    let cell = vec2(190.0, 250.0);
    let mut open_page = None;
    organize_toolbar(view, info, editable, ui, t);
    let viewport = ui.available_rect_before_wrap();
    view.viewport_screen = viewport;
    let auto_delta = if auto_scroll_enabled {
        view.auto_scroll.update(ui, viewport, true)
    } else {
        view.auto_scroll.cancel();
        Vec2::ZERO
    };
    let middle_gesture = view.auto_scroll.blocks_input();
    let mut cells: Vec<(usize, Rect)> = Vec::with_capacity(info.pages.len());
    let mut drop = false;
    egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
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
        for row in 0..rows {
            let (row_rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), cell.y), Sense::hover());
            for col in 0..cols {
                let i = row * cols + col;
                let Some(p) = info.pages.get(i) else { break };
                let c = Rect::from_min_size(pos2(row_rect.left() + left + col as f32 * cell.x, row_rect.top()), cell);
                let resp = ui.interact(c, ui.id().with(("org", i)), if editable { Sense::click_and_drag() } else { Sense::click() });
                cells.push((i, c));
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
                let info = crate::i18n::fmt(tl!("Page {label}"), &[("label", &p.label)]);
                resp.widget_info(|| egui::WidgetInfo::selected(egui::WidgetType::Checkbox, true, view.selected.contains(&i), info.clone()));
                let s = (cell.x - 44.0) / p.width.max(1.0);
                let size = vec2(p.width * s, p.height * s).min(vec2(cell.x - 44.0, cell.y - 56.0));
                let pr = Rect::from_center_size(pos2(c.center().x, c.top() + 16.0 + size.y / 2.0), size);
                let selected = view.selected.contains(&i) || (view.selected.is_empty() && i == view.current);
                if selected || resp.hovered() {
                    ui.painter().rect_filled(c.shrink(6.0), CornerRadius::same(8), if selected { t.accent_soft } else { t.hover });
                }
                if view.selected.contains(&i) {
                    ui.painter().rect_stroke(c.shrink(6.0), CornerRadius::same(8), Stroke::new(1.5, t.accent), egui::StrokeKind::Inside);
                }
                ui.painter().rect_filled(pr.translate(vec2(0.0, 1.5)), CornerRadius::same(1), t.page_shadow);
                ui.painter().rect_filled(pr, CornerRadius::ZERO, Color32::WHITE);
                if let Some(tex) = view.thumbs.get(&i) {
                    ui.painter().image(tex.id(), pr, Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)), Color32::WHITE);
                }
                ui.painter().rect_stroke(pr, CornerRadius::ZERO, Stroke::new(1.0, t.border), egui::StrokeKind::Outside);
                ui.painter().text(pos2(c.center().x, pr.bottom() + 16.0), Align2::CENTER_CENTER, &p.label, theme::medium(12.0), t.text_muted);
                if resp.clicked() {
                    let m = ui.input(|i| i.modifiers);
                    view.click_page(i, m, false);
                    view.current = i;
                }
                if resp.double_clicked() {
                    open_page = Some(i);
                }
                // Right-click: Cut, Copy, Paste (on the selection, or this page).
                resp.context_menu(|ui| {
                    if !view.selected.contains(&i) {
                        view.selected = [i].into();
                        view.current = i;
                    }
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
        // While dragging: the gap the pages would go to, drawn as a bar.
        if let (Some(pages), Some(p)) = (view.org_drag.clone(), ui.input(|i| i.pointer.hover_pos())) {
            if let Some(gap) = drop_gap(&cells, p) {
                let x = match cells.iter().find(|(i, _)| *i == gap) {
                    Some((_, r)) => r.left() + 3.0,
                    None => cells.last().map_or(0.0, |(_, r)| r.right() - 3.0),
                };
                let row = cells.iter().find(|(i, _)| *i == gap).or(cells.last()).map(|(_, r)| r.y_range()).unwrap_or(egui::Rangef::new(0.0, 0.0));
                ui.painter().line_segment([pos2(x, row.min + 10.0), pos2(x, row.max - 10.0)], Stroke::new(3.0, t.accent));
                ui.painter().text(
                    p + vec2(14.0, 14.0),
                    Align2::LEFT_TOP,
                    if pages.len() == 1 { tl!("1 page").to_string() } else { crate::i18n::fmt(tl!("{n} pages"), &[("n", &pages.len().to_string())]) },
                    theme::medium(12.0),
                    t.accent_text,
                );
            }
            if drop {
                view.org_drag = None;
                if let Some(gap) = drop_gap(&cells, p) {
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
        } else if drop {
            view.org_drag = None;
        }
    });
    view.auto_scroll.paint(ui, viewport);
    let s = THUMB_W * ppp / info.pages.iter().map(|p| p.width).fold(1.0, f32::max);
    let queue: Vec<RenderRequest> = (0..info.pages.len())
        .filter(|p| (!view.thumbs.contains_key(p) || view.stale_thumbs.contains(p)) && !view.errors.contains_key(p))
        .map(|page| RenderRequest { page, kind: RequestKind::Pixels, tile: None, scale: s, tag: THUMB_TAG })
        .collect();
    if queue != view.last_queue {
        pool.set_queue(queue.clone());
        view.last_queue = queue;
    }
    if let Some(p) = open_page {
        view.organize = false;
        view.go_to_page(p);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn view(pages: usize, layout: PageLayout) -> DocView {
        let info = pdfcraft_render::DocInfo {
            pages: (0..pages)
                .map(|_| pdfcraft_render::PageInfo { width: 300.0, height: 400.0, label: String::new(), crop: [0.0, 0.0, 300.0, 400.0], rotation: 0 })
                .collect(),
            ..Default::default()
        };
        DocView::new(DocId(1), &info, ViewDefaults { layout, ..Default::default() })
    }

    /// One mouse-wheel notch (a line) at `at` seconds.
    fn notch(v: &mut DocView, dy: f32, at: f64) -> bool {
        v.single_page_wheel(egui::MouseWheelUnit::Line, dy, egui::TouchPhase::Move, at, true)
    }

    #[test]
    fn notches_turn_pages_and_stop_at_the_ends() {
        let mut v = view(3, PageLayout::Single);
        assert!(notch(&mut v, -1.0, 1.0) && notch(&mut v, -1.0, 1.05));
        assert_eq!(v.current, 2);
        assert!(!notch(&mut v, -1.0, 1.1), "nothing turns past the last page");
        assert!(notch(&mut v, 1.0, 1.2));
        assert_eq!(v.current, 1);
    }

    #[test]
    fn wheel_is_single_page_only() {
        for layout in [PageLayout::Continuous, PageLayout::TwoUp] {
            let mut v = view(3, layout);
            assert!(!notch(&mut v, -1.0, 1.0), "{layout:?} ignores the wheel");
            assert_eq!(v.current, 0);
        }
    }

    #[test]
    fn navigation_drops_partial_wheel_motion() {
        use egui::{MouseWheelUnit::Point, TouchPhase::Move, TouchPhase::Start};
        let mut v = view(3, PageLayout::Single);
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
        let mut v = view(3, PageLayout::Continuous);
        v.goto = None;
        v.set_layout(PageLayout::Continuous);
        assert!(v.goto.is_none(), "re-selecting the current layout is a no-op");
        v.set_layout(PageLayout::Single);
        assert_eq!(v.goto, Some((0, 0.0)));
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
            // Single-page view shows one page at a time, so it fits each one.
            v.layout = PageLayout::Single;
            v.fit_zoom(&info);
            let big = v.zoom;
            v.current = 0;
            v.fit_zoom(&info);
            assert!(v.zoom > big, "{fit:?}: single-page view fits the page it shows");
        }
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
}
