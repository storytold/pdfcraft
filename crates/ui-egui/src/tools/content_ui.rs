//! Edit a PDF ▸ Add content: the Text tool (click on the page and type), Image (pick a file;
//! it lands in the middle of the page), and editing what was added: select, move, resize
//! (images keep their proportions), double-click text to retype it, format it from the panel,
//! Delete to remove it. Each change is one undoable engine edit.

use std::ops::Range;
use std::sync::Arc;

use craft::behavior::geometry::{BoxAllow, BoxHandles, BoxRules, BoxState, ScaleRule};
use craft::behavior::hit::HitShape;
use craft::behavior::kurbo::{Point, Rect as DocRect};
use craft::behavior::press::PressPhase;
use craft::behavior::{CancelReason, Cursor, CursorIcon, IconId};
use craft::draw::ui::TransformBox;
use craft::pages::ui::{PageBody, PlacedPage};
use craft::platform::clipboard::{BITMAP, ClipboardImage, TEXT};
use craft::text::ui::editor::{CaretLine, EditKind, EditorEnd, HostLine, HostRun, TextEditor, TextEditorHost, TextSelection};
use craft::theme::variant::Tone;
use craft::ui::buttons::IconButton;
use craft::ui::canvas::{CanvasTool, ToolAction, ToolContext, ToolPath, ToolPointer};
use craft::ui::canvas_controls::{HoverOutline, HoverShape, TaskBar};
use craft::ui::contract::{Bind, Props as _, Show as _, ShowContainer as _};
use craft::ui::display::{Shade, Text};
use craft::ui::form::Field;
use craft::ui::layout_components::Row;
use craft::ui::primitives::textures::{CachedTexture, Revision, TextureStore, TextureStoreOptions};
use craft::ui::primitives::{ContentPainterCx, Draw};
use craft::ui::runtime::env::UnitKind;
use craft::ui::runtime::focus::FocusRequest;
use craft::ui::sections::Section;
use craft::ui::text::NumberField;
use egui::{Color32, CornerRadius, Pos2, Rect};
use pdfcraft_engine::{Added, AddedContent, AddedText, Document, Edit};
use pdfcraft_render::DocInfo;

use crate::canvas::PageSession;
use crate::canvas::page_parts::{Owner, PageLayer, PagePart, PagePress};
use crate::edit_text_ui::{Typed, TypingHistory};

/// The colour of the warning under text the standard fonts can't draw.
pub(crate) const UNDRAWABLE: Color32 = Color32::from_rgb(0xD7, 0x37, 0x3F);

/// The first character of a draft that can't be drawn, as the engine decides it (it refuses
/// such text, #125): outside the standard fonts and not shaped with the Arabic face.
pub(crate) fn undrawable(d: &TextDraft, memo: &UndrawableMemo) -> Option<char> {
    // Checked as `finish` will commit it: without trailing whitespace.
    let t = AddedText { text: d.text.trim_end().to_string(), rect: d.rect, ..d.style.clone() };
    if let Some((key, c)) = memo.borrow().as_ref()
        && *key == t
    {
        return *c;
    }
    let c = pdfcraft_engine::first_undrawable(&t);
    *memo.borrow_mut() = Some((t, c));
    c
}

/// The last draft [`undrawable`] checked and its answer: the check lays the text out (and shapes
/// Arabic), too slow to repeat every frame for long text, so it reruns only when the text or
/// anything that changes its layout does.
pub(crate) type UndrawableMemo = std::cell::RefCell<Option<(AddedText, Option<char>)>>;

/// The warning shown for text with `c` in it.
pub(crate) fn undrawable_message(c: char) -> String {
    crate::i18n::fmt(
        tl!("The PDF text fonts can't draw “{c}” ({code}). Choose supported text."),
        &[("c", &c.to_string()), ("code", &format!("U+{:04X}", u32::from(c)))],
    )
}

/// The warning after an edit that saved text its font can't draw: PdfCraft drew `?` for it (a form
/// field, which other viewers redraw from its value, or a comment, where the `?` shows everywhere).
pub(crate) fn undrawn_message(n: &pdfcraft_engine::TextNotice) -> String {
    let template = if n.redrawn_elsewhere {
        tl!(
            "This field's font can't draw “{c}” ({code}), so PdfCraft shows ? in its place. The value is saved, and other PDF viewers redraw the field from it."
        )
    } else {
        tl!("The fonts PdfCraft writes with can't draw “{c}” ({code}), so it shows as ? in every PDF viewer. The text is saved.")
    };
    crate::i18n::fmt(template, &[("c", &n.ch.to_string()), ("code", &format!("U+{:04X}", u32::from(n.ch)))])
}

/// The text being typed or retyped in place.
#[derive(Clone, Debug)]
pub struct TextDraft {
    pub page: usize,
    /// `None` for new text; the item index for retyping.
    pub index: Option<usize>,
    /// Display space.
    pub rect: [f64; 4],
    pub text: String,
    /// The box's formatting; its `text` and `rect` are what the page has (empty for new text), and
    /// its `aligns` follow `text` as it is typed.
    pub style: AddedText,
    pub focus: bool,
    /// The selection in chars with its caret end, the value the box's `TextEditor` binds; it also
    /// says which lines an alignment touches. `None` until the editor reports one.
    pub selection: Option<TextSelection>,
    /// The box's own Undo history, from the text the box opened with.
    pub(crate) history: TypingHistory,
}

/// Two drafts are equal when everything the user sees and commits is: the history is how the box got
/// there, not what it holds.
impl PartialEq for TextDraft {
    fn eq(&self, other: &Self) -> bool {
        self.page == other.page
            && self.index == other.index
            && self.rect == other.rect
            && self.text == other.text
            && self.style == other.style
            && self.focus == other.focus
            && self.selection == other.selection
    }
}

impl TextDraft {
    /// A box on `page` for the item `index` (`None`: new text) over `rect` in display space, with
    /// `text` in `style`: nothing selected, no focus requested, and an Undo history that starts at `text`.
    pub fn new(page: usize, index: Option<usize>, rect: [f64; 4], text: String, style: AddedText) -> TextDraft {
        let history = TypingHistory::new(Typed::of(text.clone(), None, style.aligns.clone()));
        TextDraft { page, index, rect, text, style, focus: false, selection: None, history }
    }

    /// Typing the page doesn't have yet: Undo takes it back in the box before touching the document.
    pub(crate) fn has_unapplied_typing(&self) -> bool {
        self.text != self.style.text
    }

    /// The box as the history records it.
    fn typed(&self) -> Typed {
        Typed::of(self.text.clone(), self.selection, self.style.aligns.clone())
    }
}

/// Where a page is on screen as the parts last registered it: what a press, which arrives in screen
/// points, needs to say where it was in the page's display space (origin bottom left, y up).
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct PageFrame {
    page: usize,
    /// The page's top left on screen.
    origin: Pos2,
    /// Screen points per page point.
    scale: f64,
    /// The page's height in page points.
    height: f64,
}

impl PageFrame {
    fn of(placed: &PlacedPage, info: &DocInfo) -> Option<PageFrame> {
        let height = f64::from(info.pages.get(placed.page)?.height);
        Some(PageFrame { page: placed.page, origin: placed.rect.min, scale: placed.scale, height })
    }

    fn screen(&self, display: [f64; 2]) -> Pos2 {
        Pos2::new(self.origin.x + (display[0] * self.scale) as f32, self.origin.y + ((self.height - display[1]) * self.scale) as f32)
    }

    fn screen_rect(&self, r: [f64; 4]) -> Rect {
        Rect::from_two_pos(self.screen([r[0], r[1]]), self.screen([r[2], r[3]]))
    }

    fn display(&self, p: Pos2) -> [f64; 2] {
        let k = if self.scale > 0.0 { self.scale } else { 1.0 };
        [f64::from(p.x - self.origin.x) / k, self.height - f64::from(p.y - self.origin.y) / k]
    }

    /// `r` moved by what the pointer travelled from `from` to `to`.
    fn moved(&self, r: [f64; 4], from: Pos2, to: Pos2) -> [f64; 4] {
        let (a, b) = (self.display(from), self.display(to));
        let (dx, dy) = (b[0] - a[0], b[1] - a[1]);
        [r[0] + dx, r[1] + dy, r[2] + dx, r[3] + dy]
    }
}

/// An added item shown at a box other than the applied one while the pointer drags it, between the
/// press and the release.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Preview {
    page: usize,
    index: usize,
    /// The box in display space.
    rect: [f64; 4],
    /// The item is resized by a corner, not moved.
    resizing: bool,
}

/// The open box as the page body paints it: the white paper and the laid-out text. The editor draws
/// its caret and selection over it.
#[derive(Clone, Debug)]
struct DraftPaint {
    /// The editor's box.
    paper: Rect,
    /// The page's own copy of the text, to be covered until its replacement is rendered.
    cover: Option<Rect>,
    /// Whether the paper is filled: not once the page without the item is shown behind it.
    fill: bool,
    galley: Arc<egui::Galley>,
    origin: Pos2,
}

impl PartialEq for DraftPaint {
    fn eq(&self, other: &Self) -> bool {
        self.paper == other.paper
            && self.cover == other.cover
            && self.fill == other.fill
            && self.origin == other.origin
            && Arc::ptr_eq(&self.galley, &other.galley)
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct ContentView {
    /// The selected added item: (page, index among the page's items).
    pub selected: Option<(usize, usize)>,
    pub draft: Option<TextDraft>,
    /// Select the item an add creates (the page's item count before it).
    pub select_added: Option<(usize, usize)>,
    /// Done was pressed on the open box: the page body returns to Select ([`ContentView::take_done`]).
    done: bool,
    preview: Option<Preview>,
    drawn: Option<DraftPaint>,
    undrawable: UndrawableMemo,
    layout: Option<(AddedText, f32, f64, [f64; 4])>,
}

impl ContentView {
    /// Whether Done ended the box since the last call; the page body goes back to Select then.
    pub(crate) fn take_done(&mut self) -> bool {
        std::mem::take(&mut self.done)
    }

    /// Paints the open box's paper and text, under the editor's caret: called by the page control's
    /// content painter after the page, the document's own pixels being content.
    pub(crate) fn paint_draft(&self, content: &ContentPainterCx<'_>) {
        let Some(drawn) = &self.drawn else { return };
        if let Some(cover) = drawn.cover {
            content.painter.rect_filled(cover, CornerRadius::ZERO, Color32::WHITE);
        }
        if drawn.fill {
            content.painter.rect_filled(drawn.paper, CornerRadius::ZERO, Color32::WHITE);
        }
        content.painter.galley(drawn.origin, Arc::clone(&drawn.galley), Color32::BLACK);
    }

    pub(crate) fn finish_draft(&mut self, added: &[Added]) -> Option<Edit> {
        self.draft.take().and_then(|d| finish(self, d, added))
    }
    /// The draft's first character the standard fonts can't draw. Such a draft is kept open, and
    /// counts as unsaved work, until it is corrected or discarded.
    pub(crate) fn blocked(&self) -> Option<char> {
        self.draft.as_ref().and_then(|d| undrawable(d, &self.undrawable))
    }

    /// Keeps a blocked draft open and focused instead of finishing or replacing it.
    pub(crate) fn hold_blocked(&mut self) -> bool {
        let blocked = self.blocked().is_some();
        if let Some(d) = self.draft.as_mut().filter(|_| blocked) {
            d.focus = true;
        }
        blocked
    }
}

/// While an added item is moved, or a picture resized by a corner, the page without it, rendered
/// once per gesture: the item's own pixels follow the pointer over it instead of staying put until
/// release (#1488).
#[derive(Default)]
pub(crate) struct MoveBackground {
    key: Option<(u64, usize, usize)>,
    renderer: Option<pdfcraft_render::RenderPool>,
    /// The two rasters live in this store, which keeps what the painter shows from being evicted.
    store: Option<TextureStore<Raster>>,
    texture: Option<CachedTexture>,
    /// The item alone on a transparent page: the pixels that follow the pointer. The page's own
    /// raster has whatever the item covers in its box too, which must stay where it is (#4464).
    item_renderer: Option<pdfcraft_render::RenderPool>,
    item: Option<CachedTexture>,
    tag: Option<u64>,
    /// The item's display-space box when the gesture started.
    from: Option<[f64; 4]>,
    /// The item is a picture: a corner stretches its pixels, and it keeps its whole applied box.
    picture: bool,
    /// The applied move's box. The page raster on screen still has the item where it was until
    /// its replacement arrives, so the preview stays up until then.
    settling: Option<[f64; 4]>,
}

impl MoveBackground {
    pub fn pending(&self) -> bool {
        self.renderer.is_some() && self.item_renderer.is_some() && (self.texture.is_none() || self.item.is_none())
    }

    /// Both rasters are in: the page without the item, and the item alone.
    fn ready(&self) -> bool {
        self.texture.is_some() && self.item.is_some()
    }

    pub fn prepare(&mut self, ctx: &egui::Context, doc: &pdfcraft_engine::Document, cv: &ContentView, scale: f32) {
        let picture = |page, index| on_page(doc.added(), page).into_iter().any(|(i, a)| i == index && matches!(a.content, AddedContent::Image(_)));
        // Text resized by a corner rewraps, so only a picture's pixels can follow the corner.
        let key = cv.preview.filter(|p| !p.resizing || picture(p.page, p.index)).map(|p| (doc.edit_generation(), p.page, p.index));
        if !(key.is_none() && self.settling.is_some()) && self.key != key {
            *self = Self { key, ..Default::default() };
            if let Some((_, page, index)) = key {
                self.renderer = doc.added_content_preview(page, index).ok().map(|p| p.renderer());
                self.item_renderer = doc.added_content_alone(page, index).ok().map(|p| p.renderer());
                self.from = on_page(doc.added(), page).into_iter().find(|(i, _)| *i == index).map(|(_, a)| a.content.rect());
                self.picture = picture(page, index);
            }
        }
        if self.settling.is_some() {
            return;
        }
        let (Some((_, page, _)), Some(renderer), Some(item_renderer)) = (self.key, self.renderer.as_ref(), self.item_renderer.as_ref()) else {
            return;
        };
        let Some(p) = doc.info.pages.get(page) else { return };
        let max_side = ctx.input(|i| i.max_texture_side) as f32;
        let scale = scale.min(crate::canvas::BASE_SIDE.min(max_side) / p.width.max(p.height).max(1.0));
        let tag = (scale * 1000.0) as u64;
        if self.tag != Some(tag) {
            for r in [renderer, item_renderer] {
                r.set_queue(vec![pdfcraft_render::RenderRequest { page, scale, tag, ..Default::default() }]);
            }
            self.tag = Some(tag);
        }
        if self.receive(ctx, tag) && self.pending() {
            ctx.request_repaint_after(std::time::Duration::from_millis(16));
        }
    }

    /// Takes in the rasters rendered at `tag`. `false` when either failed: the preview is then
    /// dropped whole, including rasters in from an earlier scale.
    fn receive(&mut self, ctx: &egui::Context, tag: u64) -> bool {
        let store = self.store.get_or_insert_with(|| TextureStore::in_context(ctx, "added-content-move", TextureStoreOptions::pages())).clone();
        let (Some(renderer), Some(item_renderer)) = (self.renderer.as_ref(), self.item_renderer.as_ref()) else {
            return false;
        };
        let mut failed = false;
        for (r, slot, raster) in [(renderer, &mut self.texture, Raster::Page), (item_renderer, &mut self.item, Raster::Item)] {
            let Some(out) = r.try_recv() else { continue };
            if out.error.is_some() {
                failed = true;
            } else if out.request.tag == tag {
                let image = crate::canvas::texture_image([out.width as usize, out.height as usize], out.rgba);
                *slot = Some(store.put(ctx, raster, Revision::new(tag), image));
            }
        }
        if failed {
            // Without both rasters the item stays put while dragged, as before.
            self.renderer = None;
            self.item_renderer = None;
            self.texture = None;
            self.item = None;
        }
        !failed
    }

    /// The move was applied: keep the preview, now at the new generation, until the page is
    /// rendered again. Any other edit ends it.
    pub fn committed(&mut self, edit: &Edit, generation: u64) {
        match edit {
            Edit::UpdateContent { page, index, content } if self.ready() && self.key.is_some_and(|(_, p, i)| (p, i) == (*page, *index)) => {
                self.key = Some((generation, *page, *index));
                self.settling = Some(content.rect());
            }
            _ => *self = Self::default(),
        }
    }

    pub fn page_received(&mut self, page: usize) {
        if self.settling.is_some() && self.key.is_some_and(|(_, p, _)| p == page) {
            *self = Self::default();
        }
    }

    /// The page without the moving item, then the item's own pixels (from its raster alone, so
    /// nothing it covered comes along) at the dragged (`cv`'s preview) or applied position. `draw`
    /// puts the rasters up; `content` is the page control's painter, which the shared draft paint
    /// and the pixels of the document go through.
    pub fn paint(&self, draw: &Draw<'_>, content: &ContentPainterCx<'_>, placed: &PlacedPage, info: &DocInfo, cv: &ContentView) {
        let (Some((_, kp, ki)), Some(background), Some(item), Some(from)) = (self.key, self.texture.as_ref(), self.item.as_ref(), self.from) else {
            return;
        };
        let Some(frame) = PageFrame::of(placed, info).filter(|f| f.page == kp) else { return };
        let r = frame.screen_rect(from);
        let to = if let Some(p) = cv.preview.filter(|p| (p.page, p.index) == (kp, ki)) {
            frame.screen_rect(p.rect)
        } else if let Some(applied) = self.settling {
            let base = frame.screen_rect(applied);
            // A moved text box keeps its top edge (its height follows its lines).
            if self.picture { base } else { r.translate(base.left_top() - r.left_top()) }
        } else {
            return;
        };
        // Painting marks both rasters as seen, so the store keeps them for the whole gesture.
        if let Some(store) = &self.store {
            let ctx = content.painter.ctx();
            let revision = Revision::new(self.tag.unwrap_or_default());
            for raster in [Raster::Page, Raster::Item] {
                let _ = store.state(ctx, &raster, revision);
            }
        }
        let page = placed.rect;
        let whole = Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0));
        draw.texture(page, background.id, whole);
        // A pixel around the box, so its edges come along. Clip the source to the page raster so
        // an item extending past the page border never samples clamped edge texels (#3938, #4477).
        let (r, to) = (r.expand(1.0), to.expand(1.0));
        let clipped = r.intersect(page);
        if !clipped.is_positive() || r.width() <= 0.0 || r.height() <= 0.0 || page.width() <= 0.0 || page.height() <= 0.0 {
            return;
        }
        let map = |p: Pos2| Pos2::new(to.min.x + (p.x - r.min.x) / r.width() * to.width(), to.min.y + (p.y - r.min.y) / r.height() * to.height());
        let dest = Rect::from_two_pos(map(clipped.min), map(clipped.max));
        // What shows is what lies on the page: the part of the destination inside it, with the
        // source cut down the same way.
        let shown = dest.intersect(page);
        if !shown.is_positive() || dest.width() <= 0.0 || dest.height() <= 0.0 {
            return;
        }
        let norm = |p: Pos2| Pos2::new((p.x - page.min.x) / page.width(), (p.y - page.min.y) / page.height());
        let (a, b) = (norm(clipped.min), norm(clipped.max));
        let at = |p: Pos2| Pos2::new((p.x - dest.min.x) / dest.width(), (p.y - dest.min.y) / dest.height());
        let (t0, t1) = (at(shown.min), at(shown.max));
        let lerp = |from: f32, to: f32, t: f32| from + (to - from) * t;
        let uv = Rect::from_min_max(Pos2::new(lerp(a.x, b.x, t0.x), lerp(a.y, b.y, t0.y)), Pos2::new(lerp(a.x, b.x, t1.x), lerp(a.y, b.y, t1.y)));
        draw.texture(shown, item.id, uv);
    }
}

/// The two rasters of a move preview.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
enum Raster {
    /// The page without the moving item.
    Page,
    /// The item alone.
    Item,
}

/// Text style for new text (the panel's Format controls set it).
pub fn default_style() -> AddedText {
    AddedText { size: 12.0, ..AddedText::default() }
}

fn on_page(added: &[Added], page: usize) -> Vec<(usize, &Added)> {
    added.iter().filter(|a| a.page == page).enumerate().collect()
}

/// The parts of the added text on `page`, one per item in paint order: the page body registers
/// them with the rest of the page's parts, over a form field under them, and the topmost item wins
/// where boxes overlap. Pictures are `picture_parts`' (pdf-edit-w9). A box open for typing has none:
/// its editor takes the presses.
// PLAN: A-2 gives `page_parts(view, page, placed)`; the items live in the document, not in the session, so the
// document is a parameter (caller: pdf-canvas-w9's page body).
pub(crate) fn page_parts(session: &PageSession, doc: &Document, page: usize, placed: &PlacedPage) -> Vec<PagePart> {
    let Some(frame) = PageFrame::of(placed, &doc.info) else { return Vec::new() };
    let draft = session.content.draft.as_ref().filter(|d| d.page == page).and_then(|d| d.index);
    on_page(doc.added(), page)
        .into_iter()
        .filter(|(i, a)| matches!(a.content, AddedContent::Text(_)) && draft != Some(*i))
        .map(|(index, a)| {
            let shape = HitShape::Rect(frame.screen_rect(a.content.rect()).expand(2.0));
            PagePart::new(Owner::Added { index }, page, shape, PageLayer::Placed).cursor(CursorIcon::Move)
        })
        .collect()
}

/// A press on an added item (`press.owner` is its part), or, on any other owner, the page's
/// picture selection following the picture controller. A drag shows the item at the dragged box
/// and a release that moved it at least a point applies the move; a release that didn't changes
/// nothing but the selection. A click selects, a double click (or with `edit_on_click`, which Add
/// text and Edit text set, a click) opens the text for typing. `pages` are the pages as this pass's
/// body placed them, which the press's screen points are read against. The edit the press ends in,
/// if any.
// PLAN: A-2 gives `added_press(view, doc, press)`; the placement is a parameter so no page geometry is
// carried between passes (caller: pdf-canvas-w9's `page_tools.rs`, with `body.pages`).
pub(crate) fn added_press(session: &mut PageSession, doc: &Document, pages: &[PlacedPage], press: &PagePress, edit_on_click: bool) -> Option<Edit> {
    let page = press.page;
    let added = doc.added();
    let Owner::Added { index } = press.owner else {
        sync_image_selection(session, page, &doc.page_images(page), added);
        return None;
    };
    let (_, item) = on_page(added, page).into_iter().find(|(i, _)| *i == index)?;
    let AddedContent::Text(text) = &item.content else { return None };
    let frame = PageFrame::of_page(pages, &doc.info, page)?;
    let cv = &mut session.content;
    match press.phase {
        PressPhase::DragStart { start, pos, .. } | PressPhase::Drag { start, pos, .. } => {
            cv.selected = Some((page, index));
            let rect = frame.moved(text.rect, start.origin, pos);
            cv.preview = Some(Preview { page, index, rect, resizing: false });
            None
        }
        PressPhase::Release { start, pos, click: false, .. } => {
            cv.preview = None;
            if (pos - start.origin).length() < 1.0 {
                return None;
            }
            // A moved text box keeps its size, so its top edge moves with it (its height follows its lines).
            let rect = frame.moved(text.rect, start.origin, pos);
            Some(Edit::UpdateContent { page, index, content: item.content.with_rect(rect) })
        }
        PressPhase::Release { start, click: true, .. } => {
            cv.preview = None;
            cv.selected = Some((page, index));
            if start.clicks < 2 && !edit_on_click {
                return None;
            }
            // Text that can't be drawn stays where it is being typed (#125).
            if cv.hold_blocked() || cv.draft.as_ref().is_some_and(|d| d.page == page && d.index == Some(index)) {
                return None;
            }
            let typed = cv.finish_draft(added);
            let mut draft = TextDraft::new(page, Some(index), text.rect, text.text.clone(), text.clone());
            draft.focus = true;
            cv.draft = Some(draft);
            cv.selected = Some((page, index));
            typed
        }
        PressPhase::Cancel { .. } => {
            cv.preview = None;
            None
        }
        _ => None,
    }
}

/// Add text: a press on a page that nothing else holds starts a new box there, keeping the box being
/// typed. It is the tool's, not a part's: the press is the canvas's background (M-8). The tool value
/// is built again each pass, so the box starts on the press itself and no gesture state is kept.
pub(crate) struct AddTextTool<'a> {
    session: &'a mut PageSession,
    doc: &'a Document,
    pages: &'a [PlacedPage],
    style: &'a AddedText,
}

impl<'a> AddTextTool<'a> {
    /// The tool over the session's content, writing the text kept by a click to `session.pending_edit`, with
    /// `pages` as the body placed them and `style` for the new text.
    pub(crate) fn new(session: &'a mut PageSession, doc: &'a Document, pages: &'a [PlacedPage], style: &'a AddedText) -> Self {
        AddTextTool { session, doc, pages, style }
    }

    /// The box a click at `at` (screen) on `page` starts.
    fn start_box(&mut self, page: usize, at: Pos2) {
        let Some(frame) = PageFrame::of_page(self.pages, &self.doc.info, page) else { return };
        let added = self.doc.added();
        let cv = &mut self.session.content;
        // Text that can't be drawn stays where it is being typed (#125).
        if cv.hold_blocked() {
            return;
        }
        // A click elsewhere keeps the text being typed and starts a new box (#74). The editor ends its
        // session on the same background press, so whichever runs first finds the other done.
        let typed = cv.finish_draft(added);
        // The Format panel styles the new text, not the box just kept.
        (cv.selected, cv.select_added) = (None, None);
        let at = frame.display(at);
        let mut style = self.style.clone();
        style.text.clear();
        let page_width = self.doc.info.pages.get(page).map_or(f64::INFINITY, |p| p.width as f64);
        let rect = [at[0], at[1] - style.size * 1.2, (at[0] + 200.0).min(page_width), at[1]];
        style.rect = rect;
        let mut draft = TextDraft::new(page, None, rect, String::new(), style);
        draft.focus = true;
        cv.draft = Some(draft);
        if typed.is_some() {
            self.session.pending_edit = typed;
        }
    }
}

impl PageFrame {
    /// The frame of `page` among the pages the body placed.
    fn of_page(pages: &[PlacedPage], info: &DocInfo, page: usize) -> Option<PageFrame> {
        pages.iter().find(|p| p.page == page).and_then(|p| PageFrame::of(p, info))
    }
}

impl CanvasTool for AddTextTool<'_> {
    fn press(&mut self, pointer: &ToolPointer, _cx: &mut ToolContext<'_>) -> ToolAction {
        let Some(placed) = self.pages.iter().find(|p| p.rect.contains(pointer.pos)) else { return ToolAction::Ignored };
        self.start_box(placed.page, pointer.pos);
        ToolAction::Done
    }

    fn drag(&mut self, _pointer: &ToolPointer, _cx: &mut ToolContext<'_>) {}

    fn release(&mut self, _pointer: &ToolPointer, _cx: &mut ToolContext<'_>) {}

    fn cancel(&mut self, _reason: CancelReason, _cx: &mut ToolContext<'_>) {}

    /// The text tool draws nothing over the page: the box being typed is the page body's.
    fn paint(&self, _draw: &Draw<'_>, _cx: &ToolContext<'_>) {}

    fn cursor(&self, _cx: &ToolContext<'_>) -> Cursor {
        Cursor::System(CursorIcon::Text)
    }

    fn command_form(&self) -> ToolPath {
        ToolPath::Command("the Add Text command (Edit ▸ Add text)")
    }
}

/// Delete removes the selected item (not while a box is open): one undoable edit. `None` when
/// nothing is selected.
pub(crate) fn delete_selected(session: &mut PageSession) -> Option<Edit> {
    if session.content.draft.is_some() {
        return None;
    }
    let (page, index) = session.content.selected.take()?;
    session.content.preview = None;
    Some(Edit::DeleteContent { page, index })
}

/// Escape clears the selection; whether there was one to clear.
pub(crate) fn deselect(session: &mut PageSession) -> bool {
    session.content.preview = None;
    session.content.selected.take().is_some()
}

/// The image controller owns image selection; the added-content panel follows the same XObject.
pub(crate) fn sync_image_selection(session: &mut PageSession, page: usize, images: &[pdfcraft_engine::PageImage], added: &[Added]) {
    let Some(selection) = session.image_selection.as_ref().filter(|selection| selection.page == page) else { return };
    if session.content.draft.is_some() {
        return;
    }
    let object = images
        .get(selection.index)
        .and_then(|image| image.object)
        .filter(|object| images.iter().filter(|image| image.object == Some(*object)).count() == 1);
    session.content.selected = object.and_then(|object| {
        let mut matches = added
            .iter()
            .filter(|item| item.page == page)
            .enumerate()
            .filter(|(_, item)| matches!(&item.content, AddedContent::Image(image) if image.image == object));
        let selected = matches.next().map(|(index, _)| (page, index));
        if matches.next().is_none() { selected } else { None }
    });
    session.content.select_added = None;
    session.content.preview = None;
}

/// Leaving the image controller cannot leave a hidden added image owning keyboard actions.
pub(crate) fn release_image_selection(session: &mut PageSession, added: &[Added]) {
    if session.content.draft.is_some() {
        return;
    }
    let image_selected = session.content.selected.is_some_and(|(page, index)| {
        added.iter().filter(|item| item.page == page).nth(index).is_some_and(|item| matches!(item.content, AddedContent::Image(_)))
    });
    if image_selected {
        session.content.selected = None;
        session.content.select_added = None;
        session.content.preview = None;
    }
}

/// After an add: select the new item.
pub(crate) fn after_refresh(session: &mut PageSession, added: &[Added]) {
    if let Some((page, before)) = session.content.select_added.take()
        && on_page(added, page).len() > before
    {
        session.content.selected = Some((page, before));
    }
    if let Some((p, i)) = session.content.selected
        && on_page(added, p).len() <= i
    {
        session.content.selected = None;
    }
}

/// The page's added content as controls over the pages: the selected text's frame (resized by its
/// corners), the outline of the text under the pointer, and the open box's `TextEditor` session
/// with Done and Discard hung below it. Returns the edit a resize or the end of the session makes.
/// The box's paper and text are painted by [`ContentView::paint_draft`].
///
/// A pass applies only what its own pass receives: the frame's and the editor's events come from the
/// pass that shows them, and the preview they leave is held fact, never replayed. `hovered` is the
/// (page, item) of the added-text part under the pointer, as the page body reads it from the parts.
/// `view_key` is the view's ([`DocView::view_key`](crate::canvas::DocView::view_key)), which keys the editor.
pub(crate) fn content_controls(
    ui: &mut egui::Ui,
    body: &PageBody<'_>,
    session: &mut PageSession,
    doc: &Document,
    hovered: Option<(usize, usize)>,
    view_key: u64,
) -> Option<Edit> {
    if session.content.draft.is_none() {
        session.content.drawn = None;
    }
    let mut edit = None;
    for placed in body.pages {
        let Some(frame) = PageFrame::of(placed, &doc.info) else { continue };
        let hover = hovered.filter(|(page, _)| *page == placed.page).map(|(_, index)| index);
        let resized = item_controls(ui, body, session, doc, &frame, hover);
        let typed = draft_session(ui, body, session, doc, placed, &frame, view_key);
        edit = edit.or(resized).or(typed);
    }
    edit
}

/// `r` (display space) as a box in the canvas's document units.
fn doc_rect(view: &craft::behavior::CanvasView, frame: &PageFrame, r: [f64; 4]) -> DocRect {
    let (a, b) = (view.to_doc(frame.screen([r[0], r[1]])), view.to_doc(frame.screen([r[2], r[3]])));
    DocRect::from_points(a, b)
}

/// The box `state` in display space: its bounds.
fn display_rect(view: &craft::behavior::CanvasView, frame: &PageFrame, state: &BoxState) -> [f64; 4] {
    let corners = state.quad.map(|p| frame.display(view.to_screen(p)));
    let fold = |pick: fn(f64, f64) -> f64, i: usize, init: f64| corners.iter().fold(init, |m, c| pick(m, c[i]));
    [fold(f64::min, 0, f64::INFINITY), fold(f64::min, 1, f64::INFINITY), fold(f64::max, 0, f64::NEG_INFINITY), fold(f64::max, 1, f64::NEG_INFINITY)]
}

/// The selected text's frame and the hover outline on one page.
fn item_controls(
    ui: &mut egui::Ui,
    body: &PageBody<'_>,
    session: &mut PageSession,
    doc: &Document,
    frame: &PageFrame,
    hovered: Option<usize>,
) -> Option<Edit> {
    let page = frame.page;
    let items = on_page(doc.added(), page);
    let draft = session.content.draft.as_ref().filter(|d| d.page == page).and_then(|d| d.index);
    let text_at = |index: usize| items.iter().find(|(i, a)| *i == index && matches!(a.content, AddedContent::Text(_))).map(|(_, a)| *a);
    let selected = session.content.selected.filter(|s| s.0 == page && draft != Some(s.1)).and_then(|(_, i)| text_at(i).map(|a| (i, a)));
    if let Some(i) = hovered.filter(|i| draft != Some(*i) && selected.map(|s| s.0) != Some(*i))
        && let Some(a) = text_at(i)
    {
        let outline = HoverShape::Rect(doc_rect(&body.view, frame, a.content.rect()));
        let _ = HoverOutline::new(tl!("Added text"), Some(outline)).show(ui);
    }
    let (index, item) = selected?;
    let applied = item.content.rect();
    let shown = session.content.preview.filter(|p| (p.page, p.index) == (page, index)).map_or(applied, |p| p.rect);
    // Text resizes freely and rewraps; a picture's own frame is the picture controller's.
    let rules = BoxRules::default()
        .with_scale(ScaleRule::Free)
        .with_aspect_linked(false)
        .with_allow(BoxAllow::new(false, false, false).with_warp(false))
        .with_min_side(4.0)
        .with_handles(BoxHandles::Corners);
    let state = BoxState::from_rect(doc_rect(&body.view, frame, shown));
    let out = TransformBox::text_frame(tl!("Added text frame"), Bind::given(state), doc_rect(&body.view, frame, applied))
        .rules(rules)
        .session_arrows(false)
        .show_at(ui, body.rect);
    if out.discard {
        session.content.selected = None;
        session.content.preview = None;
    }
    let events = &out.edit.events;
    if let Some(done) = events.committed() {
        session.content.preview = None;
        let rect = display_rect(&body.view, frame, done);
        // A click with no drag leaves the box as it was: nothing is applied.
        let moved = rect.iter().zip(applied).any(|(a, b)| (a - b).abs() > 1.0e-3);
        return moved.then(|| Edit::UpdateContent { page, index, content: item.content.with_rect(rect) });
    }
    if events.cancel_reason().is_some() {
        session.content.preview = None;
    } else if let Some(live) = events.value() {
        session.content.preview = Some(Preview { page, index, rect: display_rect(&body.view, frame, live), resizing: true });
    }
    None
}

/// The least width of the open box on screen, in points, so a short box can still be clicked into.
const LEAST_WIDTH: f32 = 60.0;

/// The editor's inset around its text in screen points: across and down.
const INSET: egui::Vec2 = egui::vec2(4.0, 2.0);

/// Where the draft's box and text lie on screen, and what they are laid out with.
struct BoxLayout {
    font: egui::FontId,
    color: Color32,
    letter_spacing: f32,
    /// The wrap width in screen points.
    wrap: f32,
    /// The galley's top left on screen.
    origin: Pos2,
}

/// The chars of each row of `galley` (without its line break) with its box, in the galley's space.
fn rows_of(galley: &egui::Galley) -> Vec<(Range<usize>, Rect)> {
    let mut start = 0;
    galley
        .rows
        .iter()
        .map(|placed| {
            let chars = placed.row.glyphs.len();
            let range = start..start + chars;
            start += chars + usize::from(placed.ends_with_newline);
            (range, Rect::from_min_size(placed.pos, placed.row.size))
        })
        .collect()
}

/// The byte where char `at` of `text` starts, or the text's end.
fn byte_at(text: &str, at: usize) -> usize {
    text.char_indices().nth(at).map_or(text.len(), |(byte, _)| byte)
}

/// The draft's text as the `TextEditor` sees it: laid out as the page draws it (each paragraph with
/// its own alignment, character spacing scaled like the font), its geometry mapped to the canvas's
/// document units, and every edit recorded in the box's own Undo history.
struct AddedTextHost<'a> {
    draft: &'a mut TextDraft,
    ctx: egui::Context,
    view: craft::behavior::CanvasView,
    layout: BoxLayout,
    laid_out: Option<(String, Vec<Option<pdfcraft_engine::TextAlign>>, Arc<egui::Galley>)>,
}

impl AddedTextHost<'_> {
    /// The galley of the draft's text now, laid out again only when the text or its alignments changed.
    fn galley(&mut self) -> Arc<egui::Galley> {
        if let Some((text, aligns, galley)) = &self.laid_out
            && *text == self.draft.text
            && *aligns == self.draft.style.aligns
        {
            return Arc::clone(galley);
        }
        let BoxLayout { font, color, letter_spacing, wrap, .. } = &self.layout;
        let galley = aligned_galley(&self.ctx, &self.draft.text, &self.draft.style, font, *color, *letter_spacing, *wrap);
        self.laid_out = Some((self.draft.text.clone(), self.draft.style.aligns.clone(), Arc::clone(&galley)));
        galley
    }

    /// A point of the galley's space in document units.
    fn doc(&self, local: egui::Vec2) -> Point {
        self.view.to_doc(self.layout.origin + local)
    }

    /// A point in document units in the galley's space.
    fn local(&self, p: Point) -> egui::Vec2 {
        self.view.to_screen(p) - self.layout.origin
    }

    /// Screen points per document unit, never zero.
    fn zoom(&self) -> f32 {
        let zoom = self.view.zoom as f32;
        if zoom > 0.0 { zoom } else { 1.0 }
    }

    /// The row holding char `at`: a soft wrap belongs to the row it ends.
    fn row_of(rows: &[(Range<usize>, Rect)], at: usize) -> usize {
        rows.iter().position(|(r, _)| r.start <= at && at < r.end).or_else(|| rows.iter().rposition(|(r, _)| r.start <= at)).unwrap_or(0)
    }
}

impl TextEditorHost for AddedTextHost<'_> {
    fn text(&self) -> &str {
        &self.draft.text
    }

    fn replace(&mut self, range: Range<usize>, text: &str, kind: EditKind) -> Option<usize> {
        let before = self.draft.text.clone();
        let start = byte_at(&before, range.start);
        let end = byte_at(&before, range.end).max(start);
        self.draft.text.replace_range(start..end, text);
        // Each paragraph keeps its own alignment through the typing, the edit's place telling which of
        // two equal paragraphs changed.
        let caret = start + text.len();
        self.draft.style.aligns = pdfcraft_engine::paragraph_align::remap_at(&before, &self.draft.text, &self.draft.style.aligns, Some(caret));
        self.draft.selection = Some(TextSelection::caret_at(range.start + text.chars().count()));
        let now = self.draft.typed();
        self.draft.history.record(kind, now);
        Some(text.chars().count())
    }

    fn hit(&mut self, at: Point) -> (usize, usize) {
        let galley = self.galley();
        let cursor = galley.cursor_from_pos(self.local(at));
        (cursor.index.0, galley.layout_from_cursor(cursor).row)
    }

    fn caret(&mut self, offset: usize, line: Option<usize>) -> Option<CaretLine> {
        let galley = self.galley();
        let mut cursor = egui::text::CCursor::new(offset);
        if line.is_some_and(|line| galley.layout_from_cursor(cursor).row != line) {
            cursor.prefer_next_row = !cursor.prefer_next_row;
        }
        let r = galley.pos_from_cursor(cursor);
        Some(CaretLine::new(self.doc(egui::vec2(r.min.x, r.min.y)), self.doc(egui::vec2(r.min.x, r.max.y))))
    }

    fn selection_quads(&mut self, range: Range<usize>) -> Vec<[Point; 4]> {
        let galley = self.galley();
        let x = |at: usize| galley.pos_from_cursor(egui::text::CCursor::new(at)).min.x;
        rows_of(&galley)
            .into_iter()
            .filter_map(|(chars, rect)| {
                let (from, to) = (range.start.max(chars.start), range.end.min(chars.end));
                (from < to).then(|| {
                    let (x0, x1) = (x(from), x(to));
                    [
                        self.doc(egui::vec2(x0, rect.min.y)),
                        self.doc(egui::vec2(x1, rect.min.y)),
                        self.doc(egui::vec2(x1, rect.max.y)),
                        self.doc(egui::vec2(x0, rect.max.y)),
                    ]
                })
            })
            .collect()
    }

    fn underline_segments(&mut self, range: Range<usize>) -> Vec<[Point; 2]> {
        let galley = self.galley();
        let x = |at: usize| galley.pos_from_cursor(egui::text::CCursor::new(at)).min.x;
        rows_of(&galley)
            .into_iter()
            .filter_map(|(chars, rect)| {
                let (from, to) = (range.start.max(chars.start), range.end.min(chars.end));
                (from < to).then(|| [self.doc(egui::vec2(x(from), rect.max.y)), self.doc(egui::vec2(x(to), rect.max.y))])
            })
            .collect()
    }

    fn lines(&mut self) -> Vec<HostLine> {
        let galley = self.galley();
        let chars: Vec<char> = self.draft.text.chars().collect();
        let x = |at: usize| galley.pos_from_cursor(egui::text::CCursor::new(at)).min.x;
        let zoom = self.zoom();
        rows_of(&galley)
            .into_iter()
            .map(|(range, rect)| {
                let text: String = chars.get(range.clone()).map(|c| c.iter().collect()).unwrap_or_default();
                let xs: Vec<f32> = range.clone().map(x).chain(std::iter::once(x(range.end))).collect();
                let origin = xs.first().copied().unwrap_or(rect.min.x);
                let positions = xs.iter().take(range.len()).map(|v| (v - origin) / zoom).collect();
                let widths = xs.windows(2).map(|w| (w[1] - w[0]) / zoom).collect();
                let (top, bottom) = (self.doc(rect.min.to_vec2()), self.doc(rect.max.to_vec2()));
                let rect = Rect::from_two_pos(Pos2::new(top.x as f32, top.y as f32), Pos2::new(bottom.x as f32, bottom.y as f32));
                // epaint keeps no baseline: the row's bottom stands for it.
                let baseline = rect.max.y;
                let runs = vec![HostRun::new(0..text.len(), false, positions, widths)];
                HostLine::new(text, range, rect, baseline, runs)
            })
            .collect()
    }

    fn line_range(&mut self, line: usize) -> Range<usize> {
        let galley = self.galley();
        rows_of(&galley).into_iter().nth(line).map_or(0..0, |(range, _)| range)
    }

    fn line_step(&mut self, offset: usize, lines: i32) -> usize {
        let galley = self.galley();
        let rows = rows_of(&galley);
        let current = Self::row_of(&rows, offset);
        let last = rows.len().saturating_sub(1);
        let target = (current as i64 + i64::from(lines)).clamp(0, last as i64) as usize;
        if target == current {
            // Up from the first row goes to the text's start, down from the last to its end.
            return if lines < 0 { 0 } else { self.draft.text.chars().count() };
        }
        let x = galley.pos_from_cursor(egui::text::CCursor::new(offset)).min.x;
        let y = rows.get(target).map_or(0.0, |(_, rect)| rect.center().y);
        galley.cursor_from_pos(egui::vec2(x, y)).index.0
    }

    fn line_edge(&mut self, offset: usize, end: bool) -> usize {
        let galley = self.galley();
        let rows = rows_of(&galley);
        rows.get(Self::row_of(&rows, offset)).map_or(0, |(range, _)| if end { range.end } else { range.start })
    }
}

/// The open box's session on `placed`'s page: layout, the editor, Done and Discard and the line
/// for text the fonts can't draw. The edit the session ends in, if any.
fn draft_session(
    ui: &mut egui::Ui,
    body: &PageBody<'_>,
    session: &mut PageSession,
    doc: &Document,
    placed: &PlacedPage,
    frame: &PageFrame,
    view_key: u64,
) -> Option<Edit> {
    let page = placed.page;
    let ctx = ui.ctx().clone();
    let zoom = placed.scale as f32;
    let page_width = f64::from(doc.info.pages.get(page)?.width);
    let editor_id = egui::Id::new(("added-text-editor", view_key));
    let background_ready = session.content.draft.as_ref().and_then(|d| d.index).is_some_and(|index| session.text_background.ready_added(page, index));
    let cv = &mut session.content;
    let draft = cv.draft.as_mut().filter(|d| d.page == page)?;
    let font = crate::edit_text_ui::editor_font(&draft.style, (draft.style.size as f32 * zoom).clamp(1.0, 300.0));
    grow(&ctx, draft, &mut cv.layout, &font, zoom, page_width);
    let r = frame.screen_rect(draft.rect);
    let source_rect = draft.index.map(|_| frame.screen_rect(draft.style.rect));
    // Where the least width is wider than the box, the editor reaches past it away from the edge its
    // text keeps (the right edge for right-aligned text, both halves for centred), so the text stays
    // where the page draws it. Lines aligned differently from each other in a box that narrow keep
    // the editor's left edge.
    let width = r.width().max(LEAST_WIDTH);
    // The alignment every line shares, its own or the box's.
    let first = draft.style.paragraph_align(0);
    let shared = (1..pdfcraft_engine::paragraph_align::count(&draft.text)).all(|i| draft.style.paragraph_align(i) == first).then_some(first);
    let anchor = match shared {
        Some(pdfcraft_engine::TextAlign::Center) => 0.5,
        Some(pdfcraft_engine::TextAlign::Right) => 1.0,
        _ => 0.0,
    };
    let at = r.min - egui::vec2((width - r.width()) * anchor, 0.0);
<<<<<<< HEAD
    let [cr, cg, cb] = draft.style.color.map(|v| (v.clamp(0.0, 1.0) * 255.0) as u8);
    let layout = BoxLayout {
        font,
        color: Color32::from_rgb(cr, cg, cb),
        // Character spacing shows as the page will draw it, scaled like the font.
        letter_spacing: (draft.style.char_spacing * f64::from(zoom)) as f32,
        wrap: (width - INSET.x * 2.0).max(1.0),
        origin: at + INSET,
    };
    let origin = layout.origin;
    // A box still waiting for its focus was started in this pass by the press the editor also sees as a press on the
    // canvas background: that press is the one that opened it, not a click away from it.
    let fresh = if body.pass.is_some() { std::mem::replace(&mut draft.focus, false) } else { draft.focus };
    let mut selection = draft.selection.unwrap_or_else(|| TextSelection::caret_at(draft.text.chars().count()));
    let mut host = AddedTextHost { draft, ctx: ctx.clone(), view: body.view, layout, laid_out: None };
    let paper = Rect::from_min_size(at, egui::vec2(width, host.galley().rect.height() + INSET.y * 2.0));
    let editor = TextEditor::new(tl!("Added text"), &mut host, Bind::bound(&mut selection)).esc_commits(false).id(editor_id);
    let output = if fresh { editor.request_focus(FocusRequest::Take) } else { editor }.show_at(ui, paper);
    let ended = output.ended.filter(|end| !(fresh && *end == EditorEnd::Commit));
    let galley = host.galley();
    let paper = Rect::from_min_size(at, egui::vec2(width, galley.rect.height() + INSET.y * 2.0));
    cv.drawn = Some(DraftPaint {
        paper,
        cover: source_rect.filter(|_| !background_ready).map(|source| source.union(r).expand(1.0)),
        fill: !background_ready,
        galley,
        origin,
    });
    if let Some(d) = cv.draft.as_mut() {
        d.selection = Some(selection);
    }
    // Page text is drawn with the standard fonts; anything else would be saved as `?` (#125).
    let blocked = cv.blocked();
    let (mut done, mut discard) = (false, false);
    // Hung below the box (above when there is no room), in place of production's bar beside it; the
    // warning for text the fonts can't draw is its first member.
    let corners = [paper.left_top(), paper.right_top(), paper.right_bottom(), paper.left_bottom()].map(|p| body.view.to_doc(p));
    let _ = TaskBar::fixed(tl!("Text box actions"), &corners).show(ui, body.rect, |ui, _| {
        if let Some(c) = blocked {
            let _ = Text::new(undrawable_message(c)).tone(Tone::Bad).show(ui);
        }
        done = IconButton::new(IconId::new("check")).label(tl!("Done adding text")).show(ui).clicks > 0;
        discard = IconButton::new(IconId::new("x")).label(tl!("Discard this text (Esc)")).show(ui).clicks > 0;
    });
    if discard || ended == Some(EditorEnd::Cancel) {
        cv.draft = None;
        cv.drawn = None;
        return None;
    }
    if !done && ended != Some(EditorEnd::Commit) {
        return None;
=======
    let background_ready = draft.index.is_some_and(|index| view.text_background.ready_added(page, index));
    let tokens = Tokens::get(ctx);
    let (mut commit, mut cancel, mut done) = (false, false, false);
    let mut undrawable = None;
    let memo = &view.content.undrawable;
    // Clicks outside the document (the Format text panel) keep the text open, as they keep a
    // paragraph being edited open.
    // Where the click that took the focus was: the pointer itself may be gone by then (a touch, a
    // pen lifted away). The keyboard moving there (Tab, a screen reader) keeps it open too.
    let away = |p: egui::Pos2| !view.viewport_rect().contains(p);
    let outside = ctx.input(|i| i.pointer.interact_pos()).is_some_and(away) || crate::edit_text_ui::focus_pos(ctx).is_some_and(away);
    // Shown at once, like the paragraph editor: a fade-in shows the page's copy through it. Over
    // the page, so only where the document shows: text scrolled under the mode bar or a side
    // panel must leave them their pointer (#2755).
    egui::Area::new(egui::Id::unique(("added-text", view.view_key())))
        .fade_in(false)
        .order(egui::Order::Foreground)
        .fixed_pos(at)
        .constrain_to(view.viewport_rect())
        .constrain(false)
        .show(ctx, |ui| {
            let Some(t) = view.content.draft.as_mut() else { return };
            ui.set_width(r.width() + 64.0);
            if let Some(source_rect) = source_rect.filter(|_| !background_ready) {
                ui.painter().rect_filled(source_rect.union(r).expand(1.0), CornerRadius::ZERO, Color32::WHITE);
            }
            // The box is white in every theme, so the caret must not follow the theme (pale in dark mode) (#655).
            ui.visuals_mut().text_cursor.stroke.color = crate::edit_text_ui::caret_color_on(Color32::WHITE);
            let [cr, cg, cb] = t.style.color.map(|v| (v.clamp(0.0, 1.0) * 255.0) as u8);
            let color = Color32::from_rgb(cr, cg, cb);
            // Character spacing shows as the page will draw it, scaled like the font.
            let letter_spacing = (t.style.char_spacing * f64::from(zoom)) as f32;
            // Laid out as the page draws it, each paragraph with its own alignment, so opening the
            // box doesn't show it realigned.
            let style = t.style.clone();
            let line_font = font.clone();
            let mut layouter = |ui: &egui::Ui, text: &dyn egui::TextBuffer, wrap: f32| {
                aligned_galley(ui, text.as_str(), &style, &line_font, color, letter_spacing, wrap)
            };
            let before = t.text.clone();
            let input = input_id();
            t.claim_typing(ui.ctx(), doc_id);
            ui.horizontal_top(|ui| {
                ui.spacing_mut().item_spacing.x = 4.0;
                let output = egui::TextEdit::multiline(&mut t.text)
                    .font(font)
                    .desired_width(width)
                    .desired_rows(1)
                    .layouter(&mut layouter)
                    // Keep the page editor's original inset; the theme's larger button padding
                    // otherwise shifts narrow right-aligned boxes away from their text edge.
                    .margin(egui::Margin::symmetric(4, 2))
                    .background_color(if background_ready { Color32::TRANSPARENT } else { Color32::WHITE })
                    .text_color(color)
                    .hint_text(tl!("Type text"))
                    .id(input)
                    .show(ui);
                crate::edit_text_ui::narrow_double_click(ui, input, &t.text, &output);
                let resp = output.response;
                if let Some(cursor) = output.cursor_range {
                    let chars = cursor.as_sorted_char_range();
                    let byte = |index: usize| t.text.char_indices().nth(index).map_or(t.text.len(), |(at, _)| at);
                    t.selection = Some(byte(chars.start.into())..byte(chars.end.into()));
                }
                // Each paragraph keeps its own alignment through the typing.
                if t.text != before {
                    crate::edit_text_ui::follow_typing(ui.ctx(), input, &before, &t.text, &mut [&mut t.style.aligns]);
                }
                // The editor has no visible label; name it for assistive tech.
                resp.response.clone().accessible_name(tl!("Added text"));
                if t.focus {
                    resp.request_focus();
                    if let Some(range) = &t.selection {
                        crate::edit_text_ui::reselect(ui.ctx(), input, &t.text, range);
                    }
                    t.focus = false;
                }
                let buttons = egui::Frame::NONE
                    .fill(tokens.card)
                    .stroke(Stroke::new(1.0, tokens.border))
                    .corner_radius(CornerRadius::same(6))
                    .inner_margin(2)
                    .show(ui, |ui| {
                        ui.spacing_mut().item_spacing = egui::vec2(2.0, 2.0);
                        ui.horizontal(|ui| {
                            done = crate::icons::button(ui, "check", 24.0, false, tl!("Done adding text")).clicked();
                            cancel = crate::icons::button(ui, "x", 24.0, false, tl!("Discard this text (Esc)")).clicked();
                        });
                    })
                    .response
                    .rect;
                // Pressing a button, or moving the keyboard to one, takes the focus from the text:
                // that is not a click away.
                let on_buttons = ui.input(|i| i.pointer.interact_pos()).is_some_and(|p| buttons.contains(p))
                    || crate::edit_text_ui::focus_pos(ui.ctx()).is_some_and(|p| buttons.contains(p));
                cancel |= keys.pressed(ui.ctx(), egui::Key::Escape);
                commit = done || resp.lost_focus() && !on_buttons && !outside && !view.objects.hold_editor;
            });
            // Page text is drawn with the standard fonts; anything else would be saved as `?` (#125).
            undrawable = self::undrawable(t, memo);
            if let Some(c) = undrawable {
                ui.label(egui::RichText::new(undrawable_message(c)).small().color(UNDRAWABLE));
            }
        });
    if cancel {
        view.content.draft = None;
        return (None, false);
>>>>>>> upstream/main
    }
    // Text that can't be drawn stays in the editor, with the warning, rather than being lost.
    if blocked.is_some() {
        if let Some(d) = cv.draft.as_mut() {
            d.focus = true;
        }
        return None;
    }
    cv.done |= done;
    cv.drawn = None;
    let draft = cv.draft.take()?;
    finish(cv, draft, doc.added())
}

/// Grows the draft's box to hold its text where the page would wrap it, remembering the answer for
/// the same text, zoom and page width.
fn grow(
    ctx: &egui::Context,
    draft: &mut TextDraft,
    cache: &mut Option<(AddedText, f32, f64, [f64; 4])>,
    font: &egui::FontId,
    zoom: f32,
    page_width: f64,
) {
    let text = AddedText { text: draft.text.clone(), ..draft.style.clone() };
    if let Some((key, z, width, rect)) = cache.as_ref()
        && *key == text
        && *z == zoom
        && *width == page_width
    {
        draft.rect = *rect;
        return;
    }
    let source = draft.style.rect;
    if draft.index.is_some() && draft.text == draft.style.text {
        draft.rect = source;
    } else {
        let natural = ctx.fonts_mut(|f| f.layout_no_wrap(draft.text.clone(), font.clone(), Color32::BLACK).size().x) as f64 / zoom as f64;
        let width = natural.max(pdfcraft_engine::added_text_width(&text)) + 2.0;
        let available = page_width - source[0];
        let width = width.max(source[2] - source[0]).min(available.max(1.0));
        draft.rect = [source[0], source[1], source[0] + width, source[3]];
        draft.rect = pdfcraft_engine::added_text_rect(&AddedText { rect: draft.rect, ..text.clone() });
    }
    *cache = Some((text, zoom, page_width, draft.rect));
}

/// A finished draft as an edit: new text is added (and selected once it exists), retyped text
/// updated, and text emptied by retyping removed. Empty new text adds nothing.
fn finish(cv: &mut ContentView, d: TextDraft, added: &[Added]) -> Option<Edit> {
    // Unchanged is no edit, even for text saved with trailing spaces the trim would drop.
    if let Some(i) = d.index
        && let Some((_, a)) = on_page(added, d.page).into_iter().find(|(k, _)| *k == i)
        && let AddedContent::Text(old) = &a.content
        && *old == (AddedText { text: d.text.clone(), rect: d.rect, ..d.style.clone() })
    {
        return None;
    }
    let text = d.text.trim_end().to_string();
    let aligns = pdfcraft_engine::paragraph_align::remap(&d.text, &text, &d.style.aligns);
    let d = TextDraft { style: AddedText { aligns, ..d.style }, ..d };
    match d.index {
        None if text.trim().is_empty() => None,
        None => {
            cv.select_added = Some((d.page, on_page(added, d.page).len()));
            Some(Edit::AddText { page: d.page, text: AddedText { rect: d.rect, text, ..d.style } })
        }
        Some(i) if text.trim().is_empty() => Some(Edit::DeleteContent { page: d.page, index: i }),
        Some(i) => {
            let (_, a) = on_page(added, d.page).into_iter().find(|(k, _)| *k == i)?;
            let AddedContent::Text(old) = &a.content else { return None };
            // Unchanged is no edit, even for text saved with trailing spaces the trim would drop.
            if *old == (AddedText { text: d.text.clone(), rect: d.rect, ..d.style.clone() }) {
                return None;
            }
            let updated = AddedText { text, rect: d.rect, ..d.style };
            (*old != updated).then_some(Edit::UpdateContent { page: d.page, index: i, content: AddedContent::Text(updated) })
        }
    }
}

/// `text` laid out as added text with `style` is drawn: each paragraph (line typed with Enter) wrapped
/// to `wrap` and placed by its own alignment, a justified one with every row but its last
/// stretched.
fn aligned_galley(
    ctx: &egui::Context,
    text: &str,
    style: &AddedText,
    font: &egui::FontId,
    color: Color32,
    letter_spacing: f32,
    wrap: f32,
) -> std::sync::Arc<egui::Galley> {
    use pdfcraft_engine::TextAlign;
    let job = |text: &str, align: TextAlign| {
        let mut job = spaced_job(text, font, color, letter_spacing, wrap, egui::Align::Min);
        job.justify = align == TextAlign::Justify;
        job
    };
    let uniform = text.split('\n').enumerate().all(|(i, _)| style.paragraph_align(i) == style.align);
    if uniform && style.align != TextAlign::Center && style.align != TextAlign::Right {
        return ctx.fonts_mut(|f| f.layout_job(job(text, style.align)));
    }
    // Each paragraph on its own (egui aligns a whole job one way), its rows moved within the
    // box's width, then joined in order: the caret and selection see one text.
    let mut galleys = Vec::new();
    let mut start = 0;
    for (i, paragraph) in text.split('\n').enumerate() {
        let end = start + paragraph.len();
        // The last paragraph keeps a trailing break, as egui's own split does.
        let last = end + 1 == text.len();
        let piece = if last { text.get(start..).unwrap_or_default() } else { paragraph };
        let mut galley = ctx.fonts_mut(|f| f.layout_job(job(piece, style.paragraph_align(i))));
        let g = std::sync::Arc::make_mut(&mut galley);
        for placed in &mut g.rows {
            let free = (wrap - placed.row.size.x).max(0.0);
            placed.pos.x = match style.paragraph_align(i) {
                TextAlign::Center => free / 2.0,
                TextAlign::Right => free,
                TextAlign::Left | TextAlign::Justify => 0.0,
            };
        }
        g.rect = Rect::from_min_max(Pos2::ZERO, Pos2::new(g.rect.width().max(wrap), g.rect.height()));
        galleys.push(galley);
        if last {
            break;
        }
        start = end + 1;
    }
    let whole = std::sync::Arc::new(spaced_job(text, font, color, letter_spacing, wrap, egui::Align::Min));
    std::sync::Arc::new(egui::Galley::concat(whole, &galleys, ctx.pixels_per_point()))
}

/// How to use the tools, under the Add content list.
pub(crate) fn hint(ui: &mut egui::Ui) {
    let _ = Text::new(tl!(
        "Click on the page to add text; each click starts a new box, and ✓ finishes. Drag items to move them, drag a corner to resize, double-click text to edit it."
    ))
    .shade(Shade::Faint)
    .show(ui);
}

impl crate::PdfCraftApp {
    /// Pick an image file for the active document, read it, and call `then` with its name and
    /// bytes on the frame the answer arrives, and only if that document is still the active one.
    #[cfg(not(target_arch = "wasm32"))]
    fn pick_image(&mut self, title: &str, then: impl FnOnce(&mut Self, &egui::Context, String, Vec<u8>) + 'static) {
        let request = craft::host::PickRequest::open("image")
            .title(title)
            .filter(tl!("Images"), &["png", "jpg", "jpeg", "tif", "tiff", "gif", "bmp", "jp2", "j2k", "jpx"]);
        let target = self.pick_target(self.active_ids().map(|(_, id)| id));
        self.pick_first(request, target, move |app: &mut Self, ctx: &egui::Context, path: std::path::PathBuf| match std::fs::read(&path) {
            Ok(bytes) => {
                let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                then(app, ctx, name, bytes);
            }
            Err(e) => app.notify_fmt("Couldn't read {name}: {e}", &[("name", &path.display().to_string()), ("e", &e.to_string())]),
        });
    }

    /// Edit a PDF ▸ Add content ▸ Image: pick a file and place it in the middle of the current page.
    pub fn add_image_dialog(&mut self) {
        #[cfg(not(target_arch = "wasm32"))]
        self.pick_image(tl!("Choose an image"), |app, ctx, name, bytes| app.add_image(ctx, name, bytes));
        #[cfg(target_arch = "wasm32")]
        self.notify_tr("Adding images arrives on the web with file pickers for images");
    }

    /// Edit text & images ▸ Replace Image: pick a file to draw in a page image's place.
    pub(crate) fn replace_page_image_dialog(&mut self, page: usize, index: usize) {
        #[cfg(not(target_arch = "wasm32"))]
        self.pick_image(tl!("Replace image"), move |app, ctx, name, bytes| {
            app.apply_edit(
                ctx,
                Edit::EditPageImage { page, index, change: pdfcraft_engine::ImageEdit::Replace { name, bytes: std::sync::Arc::new(bytes) } },
            );
        });
        #[cfg(target_arch = "wasm32")]
        self.notify_fmt(
            "Replacing images on page {p} arrives on the web with image pickers ({i})",
            &[("p", &(page + 1).to_string()), ("i", &index.to_string())],
        );
    }

    /// Edit text & images ▸ Save Image As.
    pub(crate) fn save_page_image(&mut self, page: usize, index: usize) {
        let Some((_, id)) = self.active_ids() else { return };
        if !self.may_copy(id) {
            return;
        }
        let file = self.session.get(id).map(|d| d.page_image_file(page, index));
        match file {
            Some(Ok((ext, bytes))) => {
                self.write_files(&[(format!("Image page {} #{}.{ext}", page + 1, index + 1), std::sync::Arc::new(bytes))], "Save image");
            }
            Some(Err(e)) => self.notify_fmt("Couldn't save the image: {e}", &[("e", &e.to_string())]),
            None => {}
        }
    }

    /// Copy Image: image `index` on `page`, at its own resolution, to the clipboard.
    pub fn copy_page_image(&mut self, page: usize, index: usize) -> bool {
        // PLAN: a newer copy wins over a whole-document copy still reading its pages; `text_copies` has no
        // cancel, so its drain (pdf-shell-w17, pdf-hub-w1) drops a finished copy older than this write.
        let Some((_, id)) = self.active_ids() else { return false };
        if !self.may_copy(id) {
            return false;
        }
        let Some(pixels) = self.session.get(id).map(|d| d.page_image_rgba(page, index)) else { return false };
        let copied = pixels.and_then(|(w, h, rgba)| {
            let image = ClipboardImage { width: w, height: h, rgba: rgba.clone() };
            // PLAN: assumed `PdfCraftApp.clipboard: craft::platform::clipboard::Clipboard` (pdf-shell-w17); the app has none yet.
            self.clipboard.write_image(&image).map_err(|e| e.to_string())?;
            self.last_copied_file = None;
            self.last_copied_image = Some((w, h, rgba));
            Ok((w, h))
        });
        match copied {
            Ok((w, h)) => {
                self.notify_fmt("The image has been copied ({w} × {h} pixels)", &[("w", &w.to_string()), ("h", &h.to_string())]);
                true
            }
            Err(e) => {
                self.notify_fmt("Couldn't copy the image: {e}", &[("e", &e)]);
                false
            }
        }
    }

    /// The picture's visible box on its page, in user space.
    pub(crate) fn page_image_box(&self, page: usize, index: usize) -> Option<[f64; 4]> {
        let (_, id) = self.active_ids()?;
        let image = self.session.get(id)?.page_images(page).into_iter().nth(index)?;
        Some(image.visible_rect.unwrap_or(image.rect))
    }

    /// Cut: copy the picture, and only when that worked take it off the page (one undo step).
    pub(crate) fn cut_page_image(&mut self, ctx: &egui::Context, page: usize, index: usize) -> bool {
        let Some((view, id)) = self.active_ids() else { return false };
        if self.session.get(id).is_some_and(|doc| !doc.allows_modification()) {
            self.notify_error(tl!("The document's security settings don't allow this change"));
            return false;
        }
        if !self.copy_page_image(page, index)
            || !self.apply_edit(ctx, Edit::EditPageImage { page, index, change: pdfcraft_engine::ImageEdit::Delete })
        {
            return false;
        }
        if let Some(view) = self.views.get_mut(view) {
            view.session.image_selection = None;
        }
        true
    }

    /// Redact image: a redaction mark over the whole picture. It is applied, like any mark, from
    /// Redact a PDF.
    pub(crate) fn redact_page_image(&mut self, ctx: &egui::Context, page: usize, index: usize) {
        let Some(rect) = self.page_image_box(page, index) else { return };
        let author = self.comment_prefs.author.clone();
        let mark = self.redact_prefs.mark(page, vec![pdfcraft_engine::rect_quad(rect)], &author);
        if self.apply_edit(ctx, mark) {
            self.notify_tr("The image is marked for redaction. Apply it with Redact a PDF");
        }
    }

    /// Create link: the link properties for a new link over the picture.
    pub(crate) fn link_page_image(&mut self, page: usize, index: usize) {
        if let Some(rect) = self.page_image_box(page, index) {
            self.open_link_props(page, Some(rect), None);
        }
    }

    /// Recognize text: the Recognize Text dialog, set to this page.
    // PLAN: the caller `canvas.rs` (`ImageAction::Recognize`, pdf-canvas-w8) passes its `ctx` (L-208).
    pub fn recognize_text_on_page(&mut self, ctx: &egui::Context, page: usize) {
        self.ocr_draft.pages = crate::ocr_ui::OcrPages::Range;
        self.ocr_draft.from = page + 1;
        self.ocr_draft.to = page + 1;
        self.open_recognize_text(ctx);
    }

    /// Click an image field: pick a picture for it.
    pub fn choose_field_image(&mut self, name: &str) {
        let Some((_, id)) = self.active_ids() else { return };
        self.choose_field_image_target(id, name, None);
    }
    /// Keep the clicked document and field identity through the asynchronous image picker.
    pub fn choose_field_image_target(&mut self, id: pdfcraft_engine::DocId, name: &str, identity: Option<u64>) {
        if identity.is_some_and(|key| key >= 1u64 << 48) {
            self.notify("The selected image field identity is invalid");
            return;
        }
        #[cfg(not(target_arch = "wasm32"))]
        {
            let name = name.to_string();
            let Some(generation) = self.session.get(id).map(|d| d.edit_generation()) else { return };
            self.pick_image(tl!("Select Icon"), move |app, ctx, _, bytes| {
                if app.session.get(id).is_none_or(|d| d.edit_generation() != generation) {
                    app.notify("The document changed while choosing the image; the picture was not applied");
                    return;
                }
                let image = std::sync::Arc::new(bytes);
                let edit = match identity {
                    Some(key) => {
                        Edit::SetFieldImageTarget { name, object: pdfcraft_cos::ObjRef { num: (key >> 16) as u32, generation: key as u16 }, image }
                    }
                    None => Edit::SetFieldImage { name, image },
                };
                match app.session.apply(id, edit) {
                    Ok(_) => {
                        if let Some(view) = app.views.iter_mut().find(|v| v.id == id)
                            && let Some(doc) = app.session.get(id)
                        {
                            view.document_changed(ctx, &doc.info);
                        }
                    }
                    Err(e) => app.notify(e.to_string()),
                }
            });
        }
        #[cfg(target_arch = "wasm32")]
        let _ = (id, identity);
        #[cfg(target_arch = "wasm32")]
        self.notify_fmt("{name}: choosing images arrives on the web with file pickers for images", &[("name", &name)]);
    }

    /// Edit image ▸ Replace: pick a file for the selected image.
    pub fn replace_image_dialog(&mut self, page: usize, index: usize) {
        #[cfg(not(target_arch = "wasm32"))]
        self.pick_image(tl!("Replace image"), move |app, ctx, name, bytes| {
            app.apply_edit(ctx, Edit::ReplaceImage { page, index, name, bytes: std::sync::Arc::new(bytes) });
        });
        #[cfg(target_arch = "wasm32")]
        {
            let _ = (page, index);
            self.notify_tr("Replacing images arrives on the web with image file pickers");
        }
    }

    /// Place image `bytes` in the middle of the current page and select it.
    pub fn add_image(&mut self, ctx: &egui::Context, name: String, bytes: Vec<u8>) {
        let Some((i, id)) = self.active_ids() else { return };
        let page = self.views[i].current();
        let before_img = self.session.get(id).map_or(0, |d| d.page_images(page).len());
        let before_added = self.session.get(id).map_or(0, |d| d.added().iter().filter(|a| a.page == page).count());
        if self.apply_edit(ctx, Edit::AddImage { page, rect: None, name, bytes: std::sync::Arc::new(bytes) }) {
            self.views[i].session.content.selected = Some((page, before_added));
            self.views[i].session.content.select_added = Some((page, before_added));
            self.views[i].session.image_selection = Some(crate::edit_text_ui::ImageSelection::at(page, before_img));
            self.quick_tool = crate::QuickTool::EditText;
            self.left = crate::LeftPanel::Tool("edit");
            self.set_left_open(true);
        }
    }

    /// Whether there is an image on the clipboard (or, on the web, in the in-app copy) ready to
    /// paste. Text on the clipboard wins over an image copied before it.
    // PLAN: reading the clipboard takes `&mut` (`Clipboard::has`, `read_image`), so this takes `&mut self`;
    // its callers `canvas.rs:3222` (pdf-canvas-w8) and `tests/it/editing.rs:8715` (pdf-edit-w12, `state_mut()`) follow.
    pub fn can_paste_image(&mut self) -> bool {
        if self.clipboard.has(&[TEXT]) {
            return false;
        }
        #[cfg(not(target_arch = "wasm32"))]
        {
            self.clipboard.has(&[BITMAP])
        }
        #[cfg(target_arch = "wasm32")]
        {
            self.last_copied_image.is_some() || self.last_copied_file.is_some()
        }
    }

    /// Paste the integration's delivered pixels without rereading a clipboard that may have changed.
    pub(crate) fn paste_received_image(&mut self, ctx: &egui::Context, page: usize, image: &egui::ColorImage) -> bool {
        self.paste_received_image_at(ctx, page, image, None)
    }

    pub(crate) fn paste_received_image_at(&mut self, ctx: &egui::Context, page: usize, image: &egui::ColorImage, at: Option<[f64; 2]>) -> bool {
        let [width, height] = image.size;
        let pixels = width.checked_mul(height);
        let bytes = pixels.and_then(|n| n.checked_mul(4));
        if width == 0
            || height == 0
            || width > crate::create_ui::MAX_IMAGE_SIDE
            || height > crate::create_ui::MAX_IMAGE_SIDE
            || pixels != Some(image.pixels.len())
            || bytes.is_none_or(|n| n > crate::create_ui::MAX_IMAGE_BYTES)
        {
            self.notify_error("the clipboard image is empty, malformed or exceeds size limits");
            return false;
        }
        let rgba = image.pixels.iter().flat_map(egui::Color32::to_srgba_unmultiplied).collect();
        let delivered = crate::create_ui::Clip::Image { width, height, rgba };
        self.paste_image_with_system(ctx, page, None, at, None, false, Some(delivered))
    }

    /// Paste an image from the clipboard onto `page`.
    pub fn paste_image(&mut self, ctx: &egui::Context, page: usize, rect: Option<[f64; 4]>) -> bool {
        self.paste_image_internal(ctx, page, rect, None)
    }

    /// Paste an image centered at context menu point `at: [f64; 2]` on `page`.
    pub fn paste_image_at(&mut self, ctx: &egui::Context, page: usize, at: [f64; 2]) -> bool {
        self.paste_image_internal(ctx, page, None, Some(at))
    }

    fn paste_image_internal(&mut self, ctx: &egui::Context, page: usize, rect: Option<[f64; 4]>, at: Option<[f64; 2]>) -> bool {
        // Text on the clipboard beats an image copied in the app before it.
        let system = if self.clipboard.has(&[TEXT]) {
            Some(crate::create_ui::Clip::Text(String::new()))
        } else {
            self.clipboard.read_image().ok().flatten().map(|image| crate::create_ui::Clip::Image {
                width: image.width as usize,
                height: image.height as usize,
                rgba: image.rgba,
            })
        };
        self.paste_image_with_system(ctx, page, rect, at, system, cfg!(not(target_arch = "wasm32")), None)
    }

    fn paste_image_with_system(
        &mut self,
        ctx: &egui::Context,
        page: usize,
        rect: Option<[f64; 4]>,
        at: Option<[f64; 2]>,
        system: Option<crate::create_ui::Clip>,
        system_authoritative: bool,
        clipboard: Option<crate::create_ui::Clip>,
    ) -> bool {
        let Some((i, id)) = self.active_ids() else {
            return false;
        };
        if let Some(doc) = self.session.get(id)
            && !doc.allows_modification()
        {
            self.notify_error(tl!("The document's security settings don't allow this change"));
            return false;
        }

        let image_data: Option<(String, Vec<u8>)> = match clipboard {
            Some(crate::create_ui::Clip::Image { width, height, rgba }) => {
                let is_same = self
                    .last_copied_image
                    .as_ref()
                    .is_some_and(|(w, h, bytes)| *w as usize == width && *h as usize == height && bytes.as_slice() == rgba.as_slice());
                if !is_same {
                    self.last_copied_file = None;
                    self.last_copied_image = Some((width as u32, height as u32, rgba.clone()));
                }
                crate::create_ui::png_from_rgba(width, height, &rgba).ok().map(|b| ("pasted_image.png".to_string(), b))
            }
            Some(crate::create_ui::Clip::Text(_)) => None,
            None => {
                if matches!(system, Some(crate::create_ui::Clip::Text(_))) || (system_authoritative && system.is_none()) {
                    self.last_copied_image = None;
                    self.last_copied_file = None;
                    self.notify_tr("No image on clipboard to paste");
                    return false;
                }
                let sys_result = match system {
                    Some(crate::create_ui::Clip::Image { width, height, rgba }) => Some((width, height, rgba)),
                    _ => None,
                };

                if let Some((img_width, img_height, img_bytes)) = sys_result {
                    let is_same_as_last = self
                        .last_copied_image
                        .as_ref()
                        .is_some_and(|(w, h, rgba)| *w as usize == img_width && *h as usize == img_height && rgba.as_slice() == img_bytes.as_slice());
                    if is_same_as_last && let Some((name, bytes)) = self.last_copied_file.clone() {
                        Some((name, bytes))
                    } else if is_same_as_last && let Some((w, h, rgba)) = self.last_copied_image.clone() {
                        crate::create_ui::png_from_rgba(w as usize, h as usize, &rgba).ok().map(|b| ("pasted_image.png".to_string(), b))
                    } else {
                        // System clipboard has a newer / external image!
                        self.last_copied_file = None;
                        self.last_copied_image = Some((img_width as u32, img_height as u32, img_bytes.clone()));
                        crate::create_ui::png_from_rgba(img_width, img_height, &img_bytes).ok().map(|b| ("clipboard_image.png".to_string(), b))
                    }
                } else if let Some((name, bytes)) = self.last_copied_file.clone() {
                    Some((name, bytes))
                } else if let Some((w, h, rgba)) = self.last_copied_image.clone() {
                    crate::create_ui::png_from_rgba(w as usize, h as usize, &rgba).ok().map(|b| ("pasted_image.png".to_string(), b))
                } else {
                    None
                }
            }
        };

        let Some((name, bytes)) = image_data else {
            self.notify_tr("No image on clipboard to paste");
            return false;
        };

        let target_rect = if let Some(r) = rect {
            Some(r)
        } else if let Some(at) = at {
            let (raw_w, raw_h) = self
                .last_copied_image
                .as_ref()
                .map(|(w, h, _)| (*w as f64, *h as f64))
                .or_else(|| {
                    (bytes.len() <= 64_000_000 * 4).then(|| pdfcraft_engine::image_dimensions(&bytes)).flatten().map(|(w, h)| (w as f64, h as f64))
                })
                .unwrap_or((100.0, 100.0));
            let (pw, ph) =
                self.session.get(id).and_then(|d| d.info.pages.get(page)).map(|p| (p.width as f64, p.height as f64)).unwrap_or((612.0, 792.0));
            let k = ((pw * 0.8) / raw_w.max(1.0)).min((ph * 0.8) / raw_h.max(1.0)).min(1.0);
            let (w, h) = (raw_w * k, raw_h * k);
            let Some(info) = self.session.get(id).and_then(|doc| doc.info.pages.get(page)) else { return false };
            let view = info.user_to_view(at[0] as f32, at[1] as f32).map(f64::from);
            // Added content uses the page's bottom-left display origin; rendered view space is y-down.
            let center = [view[0], f64::from(info.height) - view[1]];
            Some([center[0] - w * 0.5, center[1] - h * 0.5, center[0] + w * 0.5, center[1] + h * 0.5])
        } else {
            None
        };

        let added_before: std::collections::HashSet<_> =
            self.session.get(id).into_iter().flat_map(|doc| doc.added()).filter(|item| item.page == page).map(|item| item.obj).collect();
        if self.apply_edit(ctx, pdfcraft_engine::Edit::AddImage { page, rect: target_rect, name, bytes: std::sync::Arc::new(bytes) }) {
            let selection = self.session.get(id).and_then(|doc| {
                let mut inserted =
                    doc.added().iter().filter(|item| item.page == page).enumerate().filter(|(_, item)| !added_before.contains(&item.obj));
                let (added_idx, item) = inserted.next()?;
                if inserted.next().is_some() {
                    return None;
                }
                let AddedContent::Image(image) = &item.content else { return None };
                let pasted_idx = doc.page_images(page).iter().rposition(|placed| placed.object == Some(image.image))?;
                Some((added_idx, pasted_idx))
            });
            if let Some(v) = self.views.get_mut(i) {
                v.go_to_page(page);
                v.session.content.selected = selection.map(|(added, _)| (page, added));
                v.session.content.select_added = v.session.content.selected;
                v.session.image_selection = selection.map(|(_, image)| crate::edit_text_ui::ImageSelection::at(page, image));
            }
            self.quick_tool = crate::QuickTool::EditText;
            self.notify_tr("Image pasted");
            true
        } else {
            false
        }
    }
}

/// What the image tools ask for.
pub(crate) enum ImageAction {
    Update(pdfcraft_engine::AddedContent),
    Replace,
}

// Lucide names a flip icon after the axis it draws, Acrobat names the flip after the direction the
// picture moves: Flip horizontal (left and right swap) shows Lucide's vertical axis.
const FLIP_HORIZONTAL_ICON: &str = "flip-vertical-2";
const FLIP_VERTICAL_ICON: &str = "flip-horizontal-2";

/// Edit image: rotate, flip, crop, replace (shown while an added image is selected).
pub(crate) fn image_panel(ui: &mut egui::Ui, img: &pdfcraft_engine::AddedImage) -> Option<ImageAction> {
    let mut out = None;
    let _ = Section::fixed(tl!("Edit image")).show(ui, |ui, _| {
        let _ = Row::new().show(ui, |ui, _| {
            let mut i = img.clone();
            let r = i.rect;
            let turn = |i: &mut pdfcraft_engine::AddedImage, k: u8| {
                i.rotation = (i.rotation + k) % 4;
                // The box turns with the picture, around its centre.
                let (cx, cy, w, h) = ((r[0] + r[2]) / 2.0, (r[1] + r[3]) / 2.0, r[2] - r[0], r[3] - r[1]);
                i.rect = [cx - h / 2.0, cy - w / 2.0, cx + h / 2.0, cy + w / 2.0];
            };
            let button =
                |ui: &mut egui::Ui, icon: &'static str, label: &str| IconButton::new(IconId::new(icon)).label(label.to_string()).show(ui).clicks > 0;
            if button(ui, "rotate-ccw", tl!("Rotate counterclockwise")) {
                turn(&mut i, 1);
                out = Some(ImageAction::Update(AddedContent::Image(i.clone())));
            }
            if button(ui, "rotate-cw", tl!("Rotate clockwise")) {
                turn(&mut i, 3);
                out = Some(ImageAction::Update(AddedContent::Image(i.clone())));
            }
            if button(ui, FLIP_HORIZONTAL_ICON, tl!("Flip horizontal")) {
                i.flip_h = !i.flip_h;
                out = Some(ImageAction::Update(AddedContent::Image(i.clone())));
            }
            if button(ui, FLIP_VERTICAL_ICON, tl!("Flip vertical")) {
                i.flip_v = !i.flip_v;
                out = Some(ImageAction::Update(AddedContent::Image(i.clone())));
            }
            if button(ui, "replace", tl!("Replace image")) {
                out = Some(ImageAction::Replace);
            }
        });
        let _ = Text::new(tl!("Crop (% trimmed from each side)")).shade(Shade::Muted).show(ui);
        let most = (1.0 - pdfcraft_engine::AddedImage::MIN_CROP_KEEP) * 100.0;
        // Unrounded: a side the user doesn't touch keeps its stored value, also one read from a file.
        let shown = img.crop.map(|v| v * 100.0);
        let mut changed = None;
        let _ = Row::new().show(ui, |ui, _| {
            for (k, label) in ["L", "B", "R", "T"].into_iter().enumerate() {
                // Up to what the opposite side leaves.
                let room = (most - shown.get((k + 2) % 4).copied().unwrap_or(0.0)).max(0.0);
                let Some(side) = shown.get(k).copied() else { continue };
                let shown_field = Field::new(label).show(ui, |ui, _| {
                    NumberField::new(label, Bind::given(Some(side))).unit_kind(UnitKind::Percent).unit("%").range(0.0..=room).step(0.5).show(ui).edit
                });
                // A typing run or a scrub is one gesture: applied when it commits, one undo step.
                if let Some(Some(value)) = shown_field.inner.as_ref().and_then(|edit| edit.events.committed()).copied() {
                    changed = Some((k, value));
                }
            }
        });
        if let Some((k, value)) = changed {
            let mut i = img.clone();
            if let Some(stored) = i.crop.get_mut(k) {
                *stored = value.clamp(0.0, most) / 100.0;
            }
            if i.crop != img.crop {
                out = Some(ImageAction::Update(AddedContent::Image(i)));
            }
        }
    });
    out
}

/// The editor's layout of added text with character spacing. The page puts the spacing after every
/// character, the last of a line included (`latin_width`), while epaint puts it only between
/// characters: the wrap width gives up one spacing so both break lines at the same place.
fn spaced_job(text: &str, font: &egui::FontId, color: Color32, letter_spacing: f32, wrap_width: f32, halign: egui::Align) -> egui::text::LayoutJob {
    let format = egui::TextFormat { font_id: font.clone(), color, extra_letter_spacing: letter_spacing, ..Default::default() };
    let mut job = egui::text::LayoutJob::single_section(text.to_owned(), format);
    job.wrap.max_width = (wrap_width - letter_spacing).max(0.0);
    job.halign = halign;
    job
}

#[cfg(test)]
mod tests {
    use egui_kittest::kittest::Queryable;

    use super::*;

    // PLAN: these fixtures still build `PdfCraftApp::for_unit_tests()`; moving them onto `with_host` with an
    // `EventClipboard` needs the test host recipe (`AppServices::new` arguments), which no owner set provides yet (pdf-hub-w1).
    #[test]
    fn delivered_pixels_are_bounded_before_cache_or_document_changes() {
        let mut app = crate::PdfCraftApp::for_unit_tests();
        app.create_blank();
        app.last_copied_image = Some((1, 1, vec![0, 255, 0, 255]));
        let before = app.last_copied_image.clone();
        for size in [[0, 1], [usize::MAX, 2], [crate::create_ui::MAX_IMAGE_SIDE + 1, 1], [2, 2]] {
            let mut image = egui::ColorImage::filled([1, 1], egui::Color32::RED);
            image.size = size;
            assert!(!app.paste_received_image(&egui::Context::default(), 0, &image));
            assert_eq!(app.last_copied_image, before);
        }
        assert!(app.session.get(app.views[0].id).unwrap().page_images(0).is_empty());
    }

    #[test]
    fn delivered_pixels_keep_unpremultiplied_alpha() {
        let mut app = crate::PdfCraftApp::for_unit_tests();
        app.create_blank();
        let image = egui::ColorImage::from_rgba_unmultiplied([1, 1], &[255, 0, 0, 128]);
        assert!(app.paste_received_image(&egui::Context::default(), 0, &image));
        assert_eq!(app.last_copied_image.unwrap().2, vec![255, 0, 0, 128]);
    }

    #[test]
    fn review_web_can_paste_an_internal_image_without_a_native_clipboard() {
        let mut app = crate::PdfCraftApp::for_unit_tests();
        app.create_blank();
        app.last_copied_image = Some((1, 1, vec![255, 0, 0, 255]));
        assert!(app.paste_image_with_system(&egui::Context::default(), 0, None, None, None, false, None));
        let id = app.active_ids().unwrap().1;
        assert_eq!(app.session.get(id).unwrap().page_images(0).len(), 1);
    }

    #[test]
    fn review_system_without_supported_formats_does_not_paste_a_cached_image() {
        let mut app = crate::PdfCraftApp::for_unit_tests();
        app.create_blank();
        app.last_copied_image = Some((1, 1, vec![255, 0, 0, 255]));
        assert!(!app.paste_image_with_system(&egui::Context::default(), 0, None, None, None, true, None));
    }

    #[test]
    fn review_cut_checks_modify_permission_before_writing_the_clipboard() {
        let mut app = crate::PdfCraftApp::for_unit_tests();
        app.create_blank();
        let png = crate::create_ui::png_from_rgba(1, 1, &[255, 0, 0, 255]).unwrap();
        app.add_image(&egui::Context::default(), "test.png".into(), png);
        app.apply_edit(
            &egui::Context::default(),
            Edit::Protect(pdfcraft_engine::Protection {
                permissions_password: Some("owner".into()),
                changes: pdfcraft_engine::Changes::None,
                copy: true,
                ..Default::default()
            }),
        );
        let id = app.active_ids().unwrap().1;
        let bytes = app.session.save_bytes(id).unwrap();
        let mut locked = crate::PdfCraftApp::for_unit_tests();
        locked.open_bytes(&egui::Context::default(), "locked.pdf", None, bytes.as_ref().clone()).unwrap();
        assert!(!locked.session.get(locked.active_ids().unwrap().1).unwrap().allows_modification());
        locked.last_copied_image = Some((1, 1, vec![0, 255, 0, 255]));
        let before = locked.last_copied_image.clone();
        assert!(!locked.cut_page_image(&egui::Context::default(), 0, 0));
        assert_eq!(locked.last_copied_image, before);
    }

    #[test]
    fn system_text_prevents_context_menu_paste_of_a_cached_image() {
        let mut app = crate::PdfCraftApp::for_unit_tests();
        app.create_blank();
        let id = app.active_ids().unwrap().1;
        app.last_copied_image = Some((1, 1, vec![255, 0, 0, 255]));
        let undo = app.session.get(id).unwrap().can_undo().map(str::to_owned);
        assert!(!app.paste_image_with_system(
            &egui::Context::default(),
            0,
            None,
            Some([20.0, 20.0]),
            Some(crate::create_ui::Clip::Text("new text".into())),
            true,
            None
        ));
        assert_eq!(app.session.get(id).unwrap().can_undo().map(str::to_owned), undo);
        assert!(app.session.get(id).unwrap().page_images(0).is_empty());
        assert!(app.last_copied_image.is_none());
    }

    /// The image a flip button leaves behind, as the panel returns it.
    fn flipped(tooltip: &str) -> pdfcraft_engine::AddedImage {
        struct PanelApp {
            img: pdfcraft_engine::AddedImage,
            out: Option<pdfcraft_engine::AddedImage>,
        }
        impl eframe::App for PanelApp {
            fn raw_input_hook(&mut self, ctx: &egui::Context, _raw: &mut egui::RawInput) {
                craft::ui::shell::embed::before_input(ctx, |_| {
                    craft::ui::runtime::frame::FrameInput::new(craft::ui::runtime::env::RootEnv::default())
                });
            }
            fn logic(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
                craft::ui::shell::embed::logic(ctx, || craft::ui::shell::embed::CloseDecision::Close);
            }
            fn ui(&mut self, ui: &mut egui::Ui, frame: &mut eframe::Frame) {
                craft::ui::shell::embed::before_ui(ui.ctx(), frame);
                egui::CentralPanel::default().show(ui, |ui| {
                    if let Some(ImageAction::Update(AddedContent::Image(i))) = image_panel(ui, &self.img) {
                        self.out = Some(i);
                    }
                });
            }
        }
        let img = pdfcraft_engine::AddedImage::new([0.0, 0.0, 100.0, 50.0], pdfcraft_cos::ObjRef::new(7, 0));
        let mut h = egui_kittest::Harness::builder().with_size(egui::vec2(400.0, 400.0)).build_eframe(move |cc| {
            crate::PdfCraftApp::install_runtime(&cc.egui_ctx);
            PanelApp { img, out: None }
        });
        h.run_steps(3);
        h.get_by_label(tooltip).click();
        h.run_steps(3);
        h.state().out.clone().expect("the button updates the image")
    }

    #[test]
    fn flip_buttons_show_the_mirror_axis_of_the_flip_they_make() {
        // #1834: Acrobat's Flip horizontal swaps left and right across a vertical axis. Lucide names
        // its flip icons after the axis drawn, so that is its flip-vertical-2.
        let svg = |name: &str| {
            let (_, bytes) = crate::icons::PDFCRAFT.icons.iter().find(|(n, _)| *n == name).expect("bundled icon");
            String::from_utf8_lossy(bytes).into_owned()
        };
        let (h, v) = (svg(FLIP_HORIZONTAL_ICON), svg(FLIP_VERTICAL_ICON));
        assert!(h.contains("M12 2v2") && !h.contains("M4 12H2"), "Flip horizontal draws a vertical axis:\n{h}");
        assert!(v.contains("M4 12H2") && !v.contains("M12 2v2"), "Flip vertical draws a horizontal axis:\n{v}");

        let i = flipped("Flip horizontal");
        assert!(i.flip_h && !i.flip_v, "Flip horizontal mirrors left and right");
        let i = flipped("Flip vertical");
        assert!(i.flip_v && !i.flip_h, "Flip vertical mirrors top and bottom");
    }

    /// #4464: both rasters were in at one scale and a render at a new scale failed. The rasters
    /// from the old scale must go too, or the item keeps following the pointer although the
    /// preview gave up.
    #[test]
    fn failed_rerender_drops_the_rasters_from_the_earlier_scale() {
        let ctx = egui::Context::default();
        let store = TextureStore::in_context(&ctx, "added-content-move-test", TextureStoreOptions::default());
        let texture = |raster| Some(store.put(&ctx, raster, Revision::new(2000), egui::ColorImage::filled([1, 1], Color32::WHITE)));
        // Bytes no parser accepts: every render answers with an error.
        let pool = || {
            let pool = pdfcraft_render::RenderPool::new_inline(std::sync::Arc::new(b"not a pdf".to_vec()), Default::default());
            pool.set_queue(vec![pdfcraft_render::RenderRequest { page: 0, scale: 2.0, tag: 2000, ..Default::default() }]);
            pool
        };
        let mut mb = MoveBackground {
            key: Some((0, 0, 0)),
            renderer: Some(pool()),
            texture: texture(Raster::Page),
            item_renderer: Some(pool()),
            item: texture(Raster::Item),
            tag: Some(2000),
            from: Some([0.0, 0.0, 10.0, 10.0]),
            ..Default::default()
        };
        assert!(mb.ready(), "both rasters from the earlier scale are in");
        assert!(!mb.receive(&ctx, 2000), "the new scale's renders fail");
        assert!(mb.texture.is_none() && mb.item.is_none(), "no raster is left to draw the moving item from");
        assert!(!mb.ready() && !mb.pending());
    }

    /// #4245: the page counts the spacing after a line's last character too, so a line that fits
    /// only without it wraps in the saved text and must wrap in the editor.
    #[test]
    fn editor_wraps_where_the_saved_text_does_with_trailing_spacing() {
        let rows = |wrap: f32| {
            let mut n = 0;
            let mut h = egui_kittest::Harness::new_ui_state(
                move |ui, n: &mut usize| {
                    let font = egui::FontId::proportional(14.0);
                    let job = spaced_job("AAAA BBBB", &font, Color32::BLACK, 10.0, wrap, egui::Align::Min);
                    *n = ui.fonts_mut(|f| f.layout_job(job)).rows.len();
                },
                n,
            );
            h.run();
            n = *h.state();
            n
        };
        // The text's width without the trailing spacing, as epaint measures it.
        let one_line = rows(f32::INFINITY);
        assert_eq!(one_line, 1);
        let width = {
            let mut w = 0.0;
            let mut h = egui_kittest::Harness::new_ui_state(
                |ui, w: &mut f32| {
                    let font = egui::FontId::proportional(14.0);
                    let job = spaced_job("AAAA BBBB", &font, Color32::BLACK, 10.0, f32::INFINITY, egui::Align::Min);
                    *w = ui.fonts_mut(|f| f.layout_job(job)).rect.width();
                },
                w,
            );
            h.run();
            w = *h.state();
            w
        };
        // Page width = width + 10 (spacing after the last character) > width + 5: the page wraps.
        assert_eq!(rows(width + 5.0), 2, "the saved text wraps here, so the editor must too");
        assert_eq!(rows(width + 10.0), 1, "and keeps the line when the spacing fits");
    }
}
