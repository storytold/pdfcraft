//! Edit a PDF ▸ Add content: the Text tool (click on the page and type), Image (pick a file;
//! it lands in the middle of the page), and editing what was added: select, move, resize
//! (images keep their proportions), double-click text to retype it, format it from the panel,
//! Delete to remove it. Each change is one undoable engine edit.

use egui::{Color32, CornerRadius, Pos2, Rect, Stroke};
use pdfcraft_engine::{Added, AddedContent, AddedText, Edit};
use pdfcraft_render::DocInfo;

use crate::canvas::{DocView, PageXform};
use crate::theme::Tokens;
use crate::widgets;

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

const SELECT_BLUE: Color32 = Color32::from_rgb(0x14, 0x73, 0xE6);

/// The text being typed or retyped in place.
#[derive(Clone, Debug, PartialEq)]
pub struct TextDraft {
    pub page: usize,
    /// `None` for new text; the item index for retyping.
    pub index: Option<usize>,
    /// Display space.
    pub rect: [f64; 4],
    pub text: String,
    pub style: AddedText,
    pub focus: bool,
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum Grab {
    Move,
    /// Which corner: (left, top) in screen terms.
    Corner(bool, bool),
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct ContentView {
    /// The selected added item: (page, index among the page's items).
    pub selected: Option<(usize, usize)>,
    pub draft: Option<TextDraft>,
    /// Select the item an add creates (the page's item count before it).
    pub select_added: Option<(usize, usize)>,
    grab: Option<(usize, Grab, Pos2)>,
    undrawable: UndrawableMemo,
    layout: Option<(AddedText, f32, f64, [f64; 4])>,
}

impl ContentView {
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
    texture: Option<egui::TextureHandle>,
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
        self.renderer.is_some() && self.texture.is_none()
    }

    pub fn prepare(&mut self, ctx: &egui::Context, doc: &pdfcraft_engine::Document, cv: &ContentView, scale: f32) {
        let picture = |page, index| on_page(doc.added(), page).into_iter().any(|(i, a)| i == index && matches!(a.content, AddedContent::Image(_)));
        let key = match (cv.grab, cv.selected) {
            (Some((gp, Grab::Move, _)), Some((sp, si))) if gp == sp => Some((doc.edit_generation(), sp, si)),
            // Text resized by a corner rewraps, so only a picture's pixels can follow the corner.
            (Some((gp, Grab::Corner(..), _)), Some((sp, si))) if gp == sp && picture(sp, si) => Some((doc.edit_generation(), sp, si)),
            _ => None,
        };
        if !(key.is_none() && self.settling.is_some()) && self.key != key {
            *self = Self { key, ..Default::default() };
            if let Some((_, page, index)) = key {
                self.renderer = doc.added_content_preview(page, index).ok().map(|p| p.renderer());
                self.from = on_page(doc.added(), page).into_iter().find(|(i, _)| *i == index).map(|(_, a)| a.content.rect());
                self.picture = picture(page, index);
            }
        }
        if self.settling.is_some() {
            return;
        }
        let (Some((_, page, _)), Some(renderer)) = (self.key, self.renderer.as_ref()) else { return };
        let Some(p) = doc.info.pages.get(page) else { return };
        let max_side = ctx.input(|i| i.max_texture_side) as f32;
        let scale = scale.min(crate::canvas::BASE_SIDE.min(max_side) / p.width.max(p.height).max(1.0));
        let tag = (scale * 1000.0) as u64;
        if self.tag != Some(tag) {
            renderer.set_queue(vec![pdfcraft_render::RenderRequest { page, scale, tag, ..Default::default() }]);
            self.tag = Some(tag);
        }
        if let Some(out) = renderer.try_recv() {
            if out.error.is_some() {
                // Without a background the item stays put while dragged, as before.
                self.renderer = None;
                return;
            }
            if out.request.tag == tag {
                self.texture = Some(ctx.load_texture(
                    "added-content-move-background",
                    crate::canvas::texture_image([out.width as usize, out.height as usize], out.rgba),
                    egui::TextureOptions::LINEAR,
                ));
            }
        }
        if self.pending() {
            ctx.request_repaint_after(std::time::Duration::from_millis(16));
        }
    }

    /// The move was applied: keep the preview, now at the new generation, until the page is
    /// rendered again. Any other edit ends it.
    pub fn committed(&mut self, edit: &Edit, generation: u64) {
        match edit {
            Edit::UpdateContent { page, index, content } if self.texture.is_some() && self.key.is_some_and(|(_, p, i)| (p, i) == (*page, *index)) => {
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

    /// The page without the moving item, then the item's pixels taken from `page_tex` (the page
    /// as rendered, item included where it was) at the dragged or applied position.
    #[allow(clippy::too_many_arguments)]
    pub fn paint(
        &self,
        ui: &egui::Ui,
        painter: &egui::Painter,
        xf: &PageXform,
        page: usize,
        info: &DocInfo,
        cv: &ContentView,
        page_tex: Option<egui::TextureId>,
    ) {
        let (Some((_, kp, _)), Some(background), Some(page_tex), Some(from)) = (self.key, &self.texture, page_tex, self.from) else { return };
        if kp != page {
            return;
        }
        let r = screen_rect(xf, info, page, from);
        let to = if let Some((_, grab, start)) = cv.grab {
            let Some(p) = ui.input(|i| i.pointer.hover_pos()) else { return };
            dragged(r, grab, p - start, self.picture)
        } else if let Some(to) = self.settling {
            let to = screen_rect(xf, info, page, to);
            // A moved text box keeps its top edge (its height follows its lines).
            if self.picture { to } else { r.translate(to.left_top() - r.left_top()) }
        } else {
            return;
        };
        let painter = painter.with_clip_rect(painter.clip_rect().intersect(xf.rect));
        xf.paint_image(&painter, background.id(), 0.0, 0.0, 1.0, 1.0);
        // A pixel around the box, so its edges come along.
        let (r, to) = (r.expand(1.0), to.expand(1.0));
        let mut mesh = egui::Mesh::with_texture(page_tex);
        for (a, b) in [
            (r.left_top(), to.left_top()),
            (r.right_top(), to.right_top()),
            (r.right_bottom(), to.right_bottom()),
            (r.left_bottom(), to.left_bottom()),
        ] {
            let (u, v) = xf.screen_to_norm(a);
            mesh.vertices.push(egui::epaint::Vertex { pos: b, uv: Pos2::new(u, v), color: Color32::WHITE });
        }
        mesh.add_triangle(0, 1, 2);
        mesh.add_triangle(0, 2, 3);
        painter.add(egui::Shape::mesh(mesh));
    }
}

/// Text style for new text (the panel's Format controls set it).
pub fn default_style() -> AddedText {
    AddedText { size: 12.0, ..AddedText::default() }
}

/// Display-space rect → screen.
fn screen_rect(xf: &PageXform, info: &DocInfo, page: usize, r: [f64; 4]) -> Rect {
    let ph = info.pages[page].height as f64;
    xf.view_rect([r[0] as f32, (ph - r[3]) as f32, r[2] as f32, (ph - r[1]) as f32])
}

/// Screen point → display space.
fn to_display(xf: &PageXform, info: &DocInfo, page: usize, p: Pos2) -> [f64; 2] {
    let (vx, vy) = xf.screen_to_view(p);
    [vx as f64, info.pages[page].height as f64 - vy as f64]
}

fn on_page(added: &[Added], page: usize) -> Vec<(usize, &Added)> {
    added.iter().filter(|a| a.page == page).enumerate().collect()
}

pub(crate) fn hits_added(xf: &PageXform, info: &DocInfo, page: usize, added: &[Added], point: Pos2) -> bool {
    on_page(added, page).iter().any(|(_, a)| screen_rect(xf, info, page, a.content.rect()).expand(2.0).contains(point))
}

fn handles(r: Rect) -> [(Pos2, bool, bool); 4] {
    [(r.left_top(), true, true), (r.right_top(), false, true), (r.left_bottom(), true, false), (r.right_bottom(), false, false)]
}

/// The rect while moving or resizing (images keep their proportions).
fn dragged(r: Rect, grab: Grab, d: egui::Vec2, keep_aspect: bool) -> Rect {
    match grab {
        Grab::Move => r.translate(d),
        Grab::Corner(left, top) => {
            let (mut dx, mut dy) = (if left { -d.x } else { d.x }, if top { -d.y } else { d.y });
            if keep_aspect && r.width() > 0.0 && r.height() > 0.0 {
                let k = ((r.width() + dx) / r.width()).max((r.height() + dy) / r.height()).max(0.05);
                dx = r.width() * k - r.width();
                dy = r.height() * k - r.height();
            }
            let mut out = r;
            if left {
                out.min.x -= dx;
            } else {
                out.max.x += dx;
            }
            if top {
                out.min.y -= dy;
            } else {
                out.max.y += dy;
            }
            Rect::from_two_pos(out.min, out.max)
        }
    }
}

/// Added text and images keep pointer ownership over a form field while editing content.
pub(crate) fn select_grabs(ui: &egui::Ui, xf: &PageXform, page: usize, info: &DocInfo, added: &[Added], view: &DocView) -> bool {
    let cv = &view.content;
    if cv.grab.is_some_and(|(grab_page, _, _)| grab_page == page) {
        return true;
    }
    let Some(p) = ui.input(|i| i.pointer.hover_pos().or(i.pointer.interact_pos())).filter(|p| xf.rect.contains(*p)) else {
        return false;
    };
    on_page(added, page).into_iter().any(|(index, item)| {
        let rect = screen_rect(xf, info, page, item.content.rect());
        rect.expand(2.0).contains(p)
            || cv.selected == Some((page, index)) && handles(rect).into_iter().any(|(corner, _, _)| corner.distance(p) <= 6.0)
    })
}

/// Pointer input on one page in Edit mode (`adding_text` with the Text tool). `true` when it
/// used the input.
#[allow(clippy::too_many_arguments)]
pub(crate) fn page_input(
    ui: &egui::Ui,
    resp: &egui::Response,
    xf: &PageXform,
    page: usize,
    info: &DocInfo,
    added: &[Added],
    adding_text: bool,
    edit_on_click: bool,
    style: &AddedText,
    view: &mut DocView,
) -> bool {
    let mut consumed = false;
    let pointer = ui.input(|i| i.pointer.hover_pos().or(i.pointer.interact_pos()));
    let cv = &mut view.content;
    if let Some((gp, grab, start)) = cv.grab
        && gp == page
    {
        consumed = true;
        if resp.drag_stopped() || !ui.input(|i| i.pointer.primary_down()) {
            cv.grab = None;
            let end = pointer.unwrap_or(start);
            if let Some((sp, si)) = cv.selected
                && sp == page
                && let Some((_, a)) = on_page(added, page).into_iter().find(|(i, _)| *i == si)
                && (end - start).length() >= 1.0
            {
                let keep = matches!(a.content, AddedContent::Image(_));
                let r = dragged(screen_rect(xf, info, page, a.content.rect()), grab, end - start, keep);
                let (p0, p1) = (to_display(xf, info, page, r.left_bottom()), to_display(xf, info, page, r.right_top()));
                let mut rect = [p0[0], p0[1], p1[0], p1[1]];
                if let AddedContent::Text(t) = &a.content
                    && grab == Grab::Move
                {
                    // Text height follows its lines: keep the top edge's offset only.
                    rect = [rect[0], rect[3] - (t.rect[3] - t.rect[1]), rect[2], rect[3]];
                }
                view.pending_edit = Some(Edit::UpdateContent { page, index: si, content: a.content.with_rect(rect) });
            }
        }
        return consumed;
    }
    let Some(p) = pointer.filter(|p| xf.rect.contains(*p)) else { return consumed };
    let items = on_page(added, page);
    let hit = items.iter().rev().find(|(_, a)| screen_rect(xf, info, page, a.content.rect()).expand(2.0).contains(p)).map(|(i, a)| (*i, *a));
    let selected_rect = cv
        .selected
        .filter(|s| s.0 == page)
        .and_then(|(_, si)| items.iter().find(|(i, _)| *i == si))
        .map(|(_, a)| screen_rect(xf, info, page, a.content.rect()));
    let corner = selected_rect.and_then(|r| handles(r).into_iter().find(|(c, ..)| c.distance(p) <= 6.0));
    // Cursors only where the page shows: the mode bar and side panels over a page scrolled under
    // them keep their own (#2755).
    let shows = resp.contains_pointer();
    if adding_text && hit.is_none() {
        if shows {
            ui.ctx().set_cursor_icon(egui::CursorIcon::Text);
        }
        if resp.clicked() {
            // Text that can't be drawn stays where it is being typed (#125).
            if cv.hold_blocked() {
                return true;
            }
            // A click elsewhere keeps the text being typed and starts a new box (#74). The click
            // can reach the page before the editor sees it lose focus, so finish it here.
            let typed = cv.draft.take().and_then(|d| finish(cv, d, added));
            // The Format panel styles the new text, not the box just kept.
            (cv.selected, cv.select_added) = (None, None);
            let at = to_display(xf, info, page, p);
            let mut style = style.clone();
            style.text.clear();
            let page_width = info.pages.get(page).map_or(f64::INFINITY, |p| p.width as f64);
            let rect = [at[0], at[1] - style.size * 1.2, (at[0] + 200.0).min(page_width), at[1]];
            style.rect = rect;
            cv.draft = Some(TextDraft { page, index: None, rect, text: String::new(), style, focus: true });
            consumed = true;
            if typed.is_some() {
                view.pending_edit = typed;
            }
        }
        return consumed;
    }
    if let Some((_, left, top)) = corner.filter(|_| shows) {
        ui.ctx().set_cursor_icon(if left == top { egui::CursorIcon::ResizeNwSe } else { egui::CursorIcon::ResizeNeSw });
    } else if hit.is_some() && shows {
        ui.ctx().set_cursor_icon(egui::CursorIcon::Move);
    }
    if resp.drag_started() {
        let origin = ui.input(|i| i.pointer.press_origin()).unwrap_or(p);
        if let Some((_, left, top)) = selected_rect.and_then(|r| handles(r).into_iter().find(|(c, ..)| c.distance(origin) <= 6.0)) {
            cv.grab = Some((page, Grab::Corner(left, top), origin));
            consumed = true;
        } else if let Some((i, _)) = items.iter().rev().find(|(_, a)| screen_rect(xf, info, page, a.content.rect()).expand(2.0).contains(origin)) {
            cv.selected = Some((page, *i));
            cv.grab = Some((page, Grab::Move, origin));
            consumed = true;
        }
        return consumed;
    }
    if (resp.double_clicked() || edit_on_click && resp.clicked() && corner.is_none())
        && let Some((i, a)) = hit
        && let AddedContent::Text(t) = &a.content
    {
        if cv.hold_blocked() {
            return true;
        }
        if cv.draft.as_ref().is_some_and(|d| d.page == page && d.index == Some(i)) {
            return true;
        }
        if let Some(edit) = cv.finish_draft(added) {
            view.pending_edit = Some(edit);
        }
        cv.draft = Some(TextDraft { page, index: Some(i), rect: t.rect, text: t.text.clone(), style: t.clone(), focus: true });
        cv.selected = Some((page, i));
        consumed = true;
        return consumed;
    }
    if resp.clicked() {
        match hit {
            Some((i, _)) => {
                cv.selected = Some((page, i));
                consumed = true;
            }
            None if corner.is_none() => cv.selected = None,
            None => consumed = true,
        }
    }
    consumed || hit.is_some() && resp.is_pointer_button_down_on()
}

/// Outlines of added items (hover and selection with handles) and the box being dragged.
pub(crate) fn paint_page(ui: &egui::Ui, painter: &egui::Painter, xf: &PageXform, page: usize, info: &DocInfo, added: &[Added], view: &DocView) {
    let cv = &view.content;
    let pointer = ui.input(|i| i.pointer.hover_pos());
    for (i, a) in on_page(added, page) {
        if cv.draft.as_ref().is_some_and(|d| d.page == page && d.index == Some(i)) {
            continue;
        }
        let mut r = screen_rect(xf, info, page, a.content.rect());
        let selected = cv.selected == Some((page, i));
        if selected
            && let (Some((gp, g, start)), Some(p)) = (cv.grab, pointer)
            && gp == page
        {
            r = dragged(r, g, p - start, matches!(a.content, AddedContent::Image(_)));
        }
        if selected {
            painter.rect_stroke(r.expand(1.0), CornerRadius::ZERO, Stroke::new(1.5, SELECT_BLUE), egui::StrokeKind::Outside);
            for (c, ..) in handles(r.expand(1.0)) {
                painter.circle(c, 3.5, Color32::WHITE, Stroke::new(1.5, SELECT_BLUE));
            }
        } else if pointer.is_some_and(|p| r.expand(2.0).contains(p)) {
            painter.rect_stroke(r.expand(1.0), CornerRadius::ZERO, Stroke::new(1.0, SELECT_BLUE.gamma_multiply(0.7)), egui::StrokeKind::Outside);
        }
    }
}

/// Delete removes the selected item; Escape clears the selection.
pub(crate) fn keys(ctx: &egui::Context, view: &mut DocView) {
    if view.content.selected.is_none() || view.content.draft.is_some() || ctx.egui_wants_keyboard_input() {
        return;
    }
    let (del, esc) = ctx.input(|i| (i.key_pressed(egui::Key::Delete) || i.key_pressed(egui::Key::Backspace), i.key_pressed(egui::Key::Escape)));
    if del && let Some((page, index)) = view.content.selected.take() {
        view.pending_edit = Some(Edit::DeleteContent { page, index });
    } else if esc {
        view.content.selected = None;
    }
}

/// After an add: select the new item.
pub(crate) fn after_refresh(view: &mut DocView, added: &[Added]) {
    if let Some((page, before)) = view.content.select_added.take()
        && on_page(added, page).len() > before
    {
        view.content.selected = Some((page, before));
    }
    if let Some((p, i)) = view.content.selected
        && on_page(added, p).len() <= i
    {
        view.content.selected = None;
    }
}

/// The in-place text editor, with Done and Discard beside it. Returns the edit once the text is
/// committed (a click away, or Done), and whether Done finished adding text. Discard or Escape
/// throws the draft away.
pub(crate) fn editor(ctx: &egui::Context, view: &mut DocView, info: &DocInfo, added: &[Added]) -> (Option<Edit>, bool) {
    let Some(page) = view.content.draft.as_ref().map(|d| d.page) else { return (None, false) };
    let Some(xf) = view.page_xform(page) else { return (None, false) };
    let zoom = xf.rect.width() / xf.pw.max(1.0);
    let Some(draft) = view.content.draft.as_mut() else { return (None, false) };
    let font = crate::edit_text_ui::editor_font(&draft.style, (draft.style.size as f32 * zoom).clamp(1.0, 300.0));
    let text = AddedText { text: draft.text.clone(), ..draft.style.clone() };
    let Some(page_width) = info.pages.get(page).map(|p| p.width as f64) else { return (None, false) };
    if let Some((key, z, width, rect)) = &view.content.layout
        && *key == text
        && *z == zoom
        && *width == page_width
    {
        draft.rect = *rect;
    } else {
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
        view.content.layout = Some((text, zoom, page_width, draft.rect));
    }
    let r = screen_rect(&xf, info, page, draft.rect);
    let source_rect = draft.index.map(|_| screen_rect(&xf, info, page, draft.style.rect));
    let background_ready = draft.index.is_some_and(|index| view.text_background.ready_added(page, index));
    let tokens = Tokens::get(ctx);
    let (mut commit, mut cancel, mut done) = (false, false, false);
    let mut undrawable = None;
    let memo = &view.content.undrawable;
    // Shown at once, like the paragraph editor: a fade-in shows the page's copy through it. Over
    // the page, so only where the document shows: text scrolled under the mode bar or a side
    // panel must leave them their pointer (#2755).
    egui::Area::new(egui::Id::new(("added-text", view.id.0)))
        .fade_in(false)
        .order(egui::Order::Foreground)
        .fixed_pos(r.min)
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
            ui.horizontal_top(|ui| {
                ui.spacing_mut().item_spacing.x = 4.0;
                let resp = ui.add(
                    egui::TextEdit::multiline(&mut t.text)
                        .font(font)
                        .desired_width(r.width().max(60.0))
                        .desired_rows(1)
                        .background_color(if background_ready { Color32::TRANSPARENT } else { Color32::WHITE })
                        .text_color(Color32::from_rgb(cr, cg, cb))
                        .hint_text(tl!("Type text"))
                        .id_salt("added-text-edit"),
                );
                if t.focus {
                    resp.request_focus();
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
                // Pressing a button takes the focus from the text: that is not a click away.
                let on_buttons = ui.input(|i| i.pointer.interact_pos()).is_some_and(|p| buttons.contains(p));
                cancel |= ui.input(|i| i.key_pressed(egui::Key::Escape));
                commit = done || resp.lost_focus() && !on_buttons && !view.objects.hold_editor;
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
    }
    // Text that can't be drawn stays in the editor, with the warning, rather than being lost.
    if !commit || undrawable.is_some() {
        return (None, false);
    }
    let edit = view.content.draft.take().and_then(|d| finish(&mut view.content, d, added));
    (edit, done)
}

/// A finished draft as an edit: new text is added (and selected once it exists), retyped text
/// updated, and text emptied by retyping removed. Empty new text adds nothing.
fn finish(cv: &mut ContentView, d: TextDraft, added: &[Added]) -> Option<Edit> {
    let text = d.text.trim_end().to_string();
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
            let updated = AddedText { text, rect: d.rect, ..d.style };
            (*old != updated).then_some(Edit::UpdateContent { page: d.page, index: i, content: AddedContent::Text(updated) })
        }
    }
}

/// How to use the tools, under the Add content list.
pub(crate) fn hint(ui: &mut egui::Ui, t: &Tokens) {
    ui.label(
        egui::RichText::new(
            tl!("Click on the page to add text; each click starts a new box, and ✓ finishes. Drag items to move them, drag a corner to resize, double-click text to edit it."),
        )
            .small()
            .color(t.text_faint),
    );
    ui.add_space(4.0);
}

impl crate::PdfCraftApp {
    /// Pick an image file for the active document, read it, and call `then` with its name and
    /// bytes: now when `save_override` answers (tests and automation never see a native dialog),
    /// otherwise on a later frame, and only if that document is still the active one.
    #[cfg(not(target_arch = "wasm32"))]
    fn pick_image(&mut self, title: &str, then: impl FnOnce(&mut Self, String, Vec<u8>) + Send + 'static) {
        let read = |app: &mut Self, path: std::path::PathBuf| match std::fs::read(&path) {
            Ok(bytes) => {
                let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                then(app, name, bytes);
            }
            Err(e) => app.notify_fmt("Couldn't read {name}: {e}", &[("name", &path.display().to_string()), ("e", &e.to_string())]),
        };
        match self.save_override.clone() {
            Some(p) if p.ends_with(".png") || p.ends_with(".jpg") => read(self, p.into()),
            Some(_) => {}
            None => {
                let dialog = rfd::AsyncFileDialog::new().add_filter(tl!("Images"), &IMAGE_EXTENSIONS).set_title(title);
                let target = self.active_ids().map(|(_, id)| id);
                self.ask_one(crate::pickers::Ask::File(dialog), target, read);
            }
        }
    }

    /// Edit a PDF ▸ Add content ▸ Image: pick a file and place it in the middle of the current page.
    pub fn add_image_dialog(&mut self) {
        #[cfg(not(target_arch = "wasm32"))]
        self.pick_image(tl!("Choose an image"), |app, name, bytes| {
            app.add_image(name, bytes);
        });
        #[cfg(target_arch = "wasm32")]
        self.notify_tr("Adding images arrives on the web with file pickers for images");
    }

    /// Edit text & images ▸ Replace Image: pick a file to draw in a page image's place.
    pub(crate) fn replace_page_image_dialog(&mut self, page: usize, index: usize) {
        #[cfg(not(target_arch = "wasm32"))]
        self.pick_image(tl!("Replace image"), move |app, name, bytes| {
            app.apply_edit(Edit::EditPageImage {
                page,
                index,
                change: pdfcraft_engine::ImageEdit::Replace { name, bytes: std::sync::Arc::new(bytes) },
            });
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
    pub(crate) fn copy_page_image(&mut self, page: usize, index: usize) -> bool {
        let Some((_, id)) = self.active_ids() else { return false };
        if !self.may_copy(id) {
            return false;
        }
        let Some(pixels) = self.session.get(id).map(|d| d.page_image_rgba(page, index)) else { return false };
        let copied = pixels.and_then(|(w, h, rgba)| {
            #[cfg(not(target_arch = "wasm32"))]
            if self.system_clipboard {
                let img = arboard::ImageData { width: w as usize, height: h as usize, bytes: std::borrow::Cow::Borrowed(&rgba) };
                arboard::Clipboard::new().and_then(|mut c| c.set_image(img)).map_err(|e| e.to_string())?;
            }
            #[cfg(target_arch = "wasm32")]
            if self.system_clipboard {
                return Err("the web version can't put images on the clipboard yet; use Save Image As".to_string());
            }
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
    fn page_image_box(&self, page: usize, index: usize) -> Option<[f64; 4]> {
        let (_, id) = self.active_ids()?;
        let image = self.session.get(id)?.page_images(page).into_iter().nth(index)?;
        Some(image.visible_rect.unwrap_or(image.rect))
    }

    /// Cut: copy the picture, and only when that worked take it off the page (one undo step).
    pub(crate) fn cut_page_image(&mut self, page: usize, index: usize) {
        if self.copy_page_image(page, index) {
            self.apply_edit(Edit::EditPageImage { page, index, change: pdfcraft_engine::ImageEdit::Delete });
        }
    }

    /// Redact image: a redaction mark over the whole picture. It is applied, like any mark, from
    /// Redact a PDF.
    pub(crate) fn redact_page_image(&mut self, page: usize, index: usize) {
        let Some(rect) = self.page_image_box(page, index) else { return };
        let author = self.comment_prefs.author.clone();
        let mark = self.redact_prefs.mark(page, vec![pdfcraft_engine::rect_quad(rect)], &author);
        if self.apply_edit(mark) {
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
    pub fn recognize_text_on_page(&mut self, page: usize) {
        self.ocr_draft.pages = crate::ocr_ui::OcrPages::Range;
        self.ocr_draft.from = page + 1;
        self.ocr_draft.to = page + 1;
        self.dialog = Some(crate::Dialog::RecognizeText);
    }

    /// Click an image field: pick a picture for it.
    pub fn choose_field_image(&mut self, name: &str) {
        #[cfg(not(target_arch = "wasm32"))]
        {
            let name = name.to_string();
            self.pick_image(tl!("Select Icon"), move |app, _, bytes| {
                app.apply_edit(Edit::SetFieldImage { name, image: std::sync::Arc::new(bytes) });
            });
        }
        #[cfg(target_arch = "wasm32")]
        self.notify_fmt("{name}: choosing images arrives on the web with file pickers for images", &[("name", &name)]);
    }

    /// Edit image ▸ Replace: pick a file for the selected image.
    pub fn replace_image_dialog(&mut self, page: usize, index: usize) {
        #[cfg(not(target_arch = "wasm32"))]
        self.pick_image(tl!("Replace image"), move |app, name, bytes| {
            app.apply_edit(Edit::ReplaceImage { page, index, name, bytes: std::sync::Arc::new(bytes) });
        });
        #[cfg(target_arch = "wasm32")]
        {
            let _ = (page, index);
            self.notify_tr("Replacing images arrives on the web with image file pickers");
        }
    }

    /// Place image `bytes` in the middle of the current page and select it. Returns whether it
    /// was placed (a failure has been said).
    pub fn add_image(&mut self, name: String, bytes: Vec<u8>) -> bool {
        let Some((i, _)) = self.active_ids() else { return false };
        let page = self.views[i].current;
        self.add_image_on(page, name, bytes)
    }

    /// [`Self::add_image`] on `page` of the active document.
    fn add_image_on(&mut self, page: usize, name: String, bytes: Vec<u8>) -> bool {
        let Some((i, id)) = self.active_ids() else { return false };
        let before = self.session.get(id).map_or(0, |d| d.added().iter().filter(|a| a.page == page).count());
        let placed = self.apply_edit(Edit::AddImage { page, rect: None, name, bytes: std::sync::Arc::new(bytes) });
        if placed {
            self.views[i].content.select_added = Some((page, before));
            self.left = crate::LeftPanel::Tool("edit");
            self.left_open = true;
        }
        placed
    }

    /// Whether ⌘V / Ctrl+V was let go this frame. egui-winit swallows that press (with a Paste
    /// event only if the clipboard holds text), while a plain V press always comes through: a
    /// release of V with no press of V since the last one is the shortcut, even when ⌘ or Ctrl
    /// was let go first. Read every frame, dialogs or not, so a press is never missed.
    pub(crate) fn paste_key_released(&mut self, ctx: &egui::Context) -> bool {
        let mut released = false;
        ctx.input(|inp| {
            for e in &inp.events {
                if let egui::Event::Key { key: egui::Key::V, pressed, .. } = e {
                    released |= !*pressed && !self.v_pressed;
                    self.v_pressed = *pressed;
                }
            }
        });
        released
    }

    /// ⌘V / Ctrl+V with a picture on the system clipboard (a screenshot, a picture copied in a
    /// browser) places it on the current page, as Add Image does. So do image files copied in
    /// Explorer, Finder or a file manager (#1289), each in its own format; the clipboard's file
    /// list comes first, because Finder puts the file's icon beside it as a picture.
    /// Organize Pages keeps ⌘V for pages, Prepare Form for fields, and text fields for text.
    pub(crate) fn paste_image_key(&mut self, i: usize, ctx: &egui::Context, pasted: bool) {
        let free = self.dialog.is_none() && !self.palette_open && !ctx.egui_wants_keyboard_input();
        let Some(view) = self.views.get(i) else { return };
        if !pasted || !free || view.organize || view.forms.focus.is_some() || self.is_preparing() {
            return;
        }
        let clip = match self.clipboard_override.clone() {
            Some(c) => Some(c),
            #[cfg(not(target_arch = "wasm32"))]
            None if self.system_clipboard => clipboard_picture(crate::create_ui::read_clipboard_files(), crate::create_ui::read_clipboard),
            None => None,
        };
        match clip {
            Some(crate::create_ui::Clip::Image { width, height, rgba }) => match crate::create_ui::png_from_rgba(width, height, &rgba) {
                Ok(png) => {
                    self.add_image("Pasted image.png".into(), png);
                }
                Err(e) => self.notify_fmt("Couldn't read {name}: {e}", &[("name", tl!("Clipboard")), ("e", &e)]),
            },
            #[cfg(not(target_arch = "wasm32"))]
            Some(crate::create_ui::Clip::Files(paths)) => self.paste_image_files(&paths),
            _ => {}
        }
    }

    /// Place the copied image files on the current page, each as Add Image places one (so
    /// several lie one over the other, the last one selected). The files are read on a worker
    /// thread, as a slow or network drive must not hold up the frame, and placed on a later
    /// frame by [`Self::finish_paste_reads`]. Copied files that aren't pictures (PDFs,
    /// documents, folders) are left alone; when nothing could be placed and no file said why,
    /// a notice says so.
    #[cfg(not(target_arch = "wasm32"))]
    fn paste_image_files(&mut self, paths: &[std::path::PathBuf]) {
        let files: Vec<_> = paths.iter().filter(|p| p.is_absolute()).collect();
        if files.len() > MAX_PASTED_IMAGES {
            self.notify_fmt("Only the first {n} copied files are pasted", &[("n", &MAX_PASTED_IMAGES.to_string())]);
        }
        let pictures: Vec<_> = files.into_iter().take(MAX_PASTED_IMAGES).filter(|p| is_image_file_name(p)).cloned().collect();
        if pictures.is_empty() {
            return self.notify_tr(NO_PICTURE);
        }
        let Some((i, doc)) = self.active_ids() else { return };
        let target = PasteTarget { doc, page: self.views[i].current, generation: self.session.get(doc).map(|d| d.edit_generation()) };
        let done = self.paste_reads.done.clone();
        let ctx = self.ctx.clone();
        self.paste_reads.reading += 1;
        let started = std::thread::Builder::new().name("paste-images".into()).spawn(move || {
            // Folders called like pictures are skipped, as other files that aren't pictures are.
            let pictures = pictures.into_iter().filter(|p| !p.is_dir());
            let files = read_within(pictures, MAX_PASTED_IMAGE_BYTES, MAX_PASTE_BYTES, read_capped);
            done.lock().unwrap_or_else(std::sync::PoisonError::into_inner).push(PasteRead { target, files });
            if let Some(ctx) = ctx {
                ctx.request_repaint();
            }
        });
        if let Err(e) = started {
            self.paste_reads.reading = self.paste_reads.reading.saturating_sub(1);
            self.notify_fmt("Couldn't read {name}: {e}", &[("name", tl!("Clipboard")), ("e", &e.to_string())]);
        }
    }

    /// Whether copied image files are still being read for ⌘V (tests wait for it).
    #[cfg(not(target_arch = "wasm32"))]
    pub fn paste_reading(&self) -> bool {
        self.paste_reads.reading > 0
    }

    /// Place the copied image files read since the last frame, on the page they were pasted on,
    /// if their document is still the active one and unedited. Called every frame.
    #[cfg(not(target_arch = "wasm32"))]
    pub(crate) fn finish_paste_reads(&mut self) {
        let finished = std::mem::take(&mut *self.paste_reads.done.lock().unwrap_or_else(std::sync::PoisonError::into_inner));
        for PasteRead { target, files } in finished {
            self.paste_reads.reading = self.paste_reads.reading.saturating_sub(1);
            let active = self.active_ids();
            let generation = self.session.get(target.doc).map(|d| d.edit_generation());
            if active.is_none_or(|(_, id)| id != target.doc) || generation != target.generation {
                self.notify_tr("The document changed while the copied files were read, so nothing was pasted.");
                continue;
            }
            // Only folders called like pictures were copied: nothing else would say why nothing
            // was placed. Each file read is placed, or its own notice (or the document's
            // refusal) says why not.
            if files.is_empty() {
                self.notify_tr(NO_PICTURE);
                continue;
            }
            for (path, read) in files {
                let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                match read {
                    FileRead::Bytes(bytes) => {
                        // A document that can't take it says so once, not once per file.
                        if !self.add_image_on(target.page, name, bytes) {
                            break;
                        }
                    }
                    FileRead::TooLarge => self.notify_fmt(
                        "{name} is too large to paste (at most {n} MB)",
                        &[("name", &name), ("n", &(MAX_PASTED_IMAGE_BYTES >> 20).to_string())],
                    ),
                    FileRead::OverTotal => self.notify_fmt(
                        "{name} wasn't pasted: the copied files together are over {n} MB",
                        &[("name", &name), ("n", &(MAX_PASTE_BYTES >> 20).to_string())],
                    ),
                    FileRead::Failed(e) => {
                        self.notify_fmt("Couldn't read {name}: {e}", &[("name", &path.display().to_string()), ("e", &e)]);
                    }
                }
            }
        }
    }
}

/// What a paste with no picture to place says.
#[cfg(not(target_arch = "wasm32"))]
const NO_PICTURE: &str = "None of the copied files is a picture to place (PNG, JPEG, TIFF, GIF, BMP or JPEG 2000)";

/// Copied image files being read for ⌘V, and those read, waiting for the next frame.
#[cfg(not(target_arch = "wasm32"))]
#[derive(Debug, Default)]
pub struct PasteReads {
    /// Reads started and not yet placed.
    reading: usize,
    done: std::sync::Arc<std::sync::Mutex<Vec<PasteRead>>>,
}

/// The document and page a paste was for, and the document's edit generation then.
#[cfg(not(target_arch = "wasm32"))]
#[derive(Clone, Copy, Debug)]
struct PasteTarget {
    doc: pdfcraft_engine::DocId,
    page: usize,
    generation: Option<u64>,
}

/// One copied file as read.
#[cfg(not(target_arch = "wasm32"))]
#[derive(Debug, PartialEq)]
enum FileRead {
    Bytes(Vec<u8>),
    /// Over the cap for one file.
    TooLarge,
    /// Over what the files read before it left of the cap for the whole paste.
    OverTotal,
    /// Why it couldn't be read.
    Failed(String),
}

/// Read each of `paths` with `read`, at most `per_file` bytes each and `total` bytes for all
/// of them, so a paste never holds more than `total` bytes of files waiting to be placed.
#[cfg(not(target_arch = "wasm32"))]
fn read_within(
    paths: impl IntoIterator<Item = std::path::PathBuf>,
    per_file: u64,
    total: u64,
    mut read: impl FnMut(&std::path::Path, u64) -> std::io::Result<Option<Vec<u8>>>,
) -> Vec<(std::path::PathBuf, FileRead)> {
    let mut left = total;
    paths
        .into_iter()
        .map(|path| {
            let cap = per_file.min(left);
            let result = match read(&path, cap) {
                Ok(Some(bytes)) => {
                    left = left.saturating_sub(bytes.len() as u64);
                    FileRead::Bytes(bytes)
                }
                // Over what the paste had left: that is the cap it ran into.
                Ok(None) if cap < per_file => FileRead::OverTotal,
                Ok(None) => FileRead::TooLarge,
                Err(e) => FileRead::Failed(e.to_string()),
            };
            (path, result)
        })
        .collect()
}

/// Copied image files read for one paste.
#[cfg(not(target_arch = "wasm32"))]
#[derive(Debug)]
struct PasteRead {
    target: PasteTarget,
    files: Vec<(std::path::PathBuf, FileRead)>,
}

/// At most this many copied files are pasted at once.
#[cfg(not(target_arch = "wasm32"))]
const MAX_PASTED_IMAGES: usize = 50;

/// A copied image file larger than this is not pasted.
#[cfg(not(target_arch = "wasm32"))]
const MAX_PASTED_IMAGE_BYTES: u64 = 256 << 20;

/// The copied image files of one paste together are not read past this.
#[cfg(not(target_arch = "wasm32"))]
const MAX_PASTE_BYTES: u64 = 512 << 20;

/// The regular file at `path`, or `None` when it holds more than `cap` bytes. Anything else
/// (a named pipe or a device, which a file list can name too) is refused before it is read,
/// because reading one waits until something writes to it.
#[cfg(not(target_arch = "wasm32"))]
fn read_capped(path: &std::path::Path, cap: u64) -> std::io::Result<Option<Vec<u8>>> {
    use std::io::Read;
    if !std::fs::metadata(path)?.is_file() {
        return Err(not_a_regular_file());
    }
    let mut bytes = Vec::new();
    open_regular(path)?.take(cap.saturating_add(1)).read_to_end(&mut bytes)?;
    Ok((bytes.len() as u64 <= cap).then_some(bytes))
}

/// Open `path` for reading if it is a regular file. On Unix the open doesn't wait: a path
/// swapped for a named pipe after the caller looked would otherwise wait in the open itself for
/// a writer. The flag changes nothing for a regular file.
#[cfg(not(target_arch = "wasm32"))]
fn open_regular(path: &std::path::Path) -> std::io::Result<std::fs::File> {
    let mut open = std::fs::OpenOptions::new();
    open.read(true);
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::custom_flags(&mut open, libc::O_NONBLOCK);
    let file = open.open(path)?;
    if !file.metadata()?.is_file() {
        return Err(not_a_regular_file());
    }
    Ok(file)
}

#[cfg(not(target_arch = "wasm32"))]
fn not_a_regular_file() -> std::io::Error {
    std::io::Error::new(std::io::ErrorKind::InvalidInput, "it isn't a regular file")
}

/// What ⌘V places from the system clipboard: its copied files when it has any, else its picture.
/// Finder puts a copied file's icon beside it as a picture, so the files come first, and the
/// picture isn't read at all then.
#[cfg(not(target_arch = "wasm32"))]
fn clipboard_picture(files: Vec<std::path::PathBuf>, picture: impl FnOnce() -> Option<crate::create_ui::Clip>) -> Option<crate::create_ui::Clip> {
    if files.is_empty() { picture() } else { Some(crate::create_ui::Clip::Files(files)) }
}

/// The image files Add Image offers, by extension.
#[cfg(not(target_arch = "wasm32"))]
const IMAGE_EXTENSIONS: [&str; 10] = ["png", "jpg", "jpeg", "tif", "tiff", "gif", "bmp", "jp2", "j2k", "jpx"];

/// Whether `path` names an image file Add Image takes, by its extension (any case).
#[cfg(not(target_arch = "wasm32"))]
fn is_image_file_name(path: &std::path::Path) -> bool {
    path.extension().and_then(|e| e.to_str()).is_some_and(|e| IMAGE_EXTENSIONS.iter().any(|x| e.eq_ignore_ascii_case(x)))
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
pub(crate) fn image_panel(ui: &mut egui::Ui, t: &Tokens, img: &pdfcraft_engine::AddedImage) -> Option<ImageAction> {
    let mut out = None;
    widgets::section_title(ui, tl!("Edit image"));
    ui.horizontal(|ui| {
        let mut i = img.clone();
        let r = i.rect;
        let turn = |i: &mut pdfcraft_engine::AddedImage, k: u8| {
            i.rotation = (i.rotation + k) % 4;
            // The box turns with the picture, around its centre.
            let (cx, cy, w, h) = ((r[0] + r[2]) / 2.0, (r[1] + r[3]) / 2.0, r[2] - r[0], r[3] - r[1]);
            i.rect = [cx - h / 2.0, cy - w / 2.0, cx + h / 2.0, cy + w / 2.0];
        };
        if crate::icons::button(ui, "rotate-ccw", 28.0, false, tl!("Rotate counterclockwise")).clicked() {
            turn(&mut i, 1);
            out = Some(ImageAction::Update(AddedContent::Image(i.clone())));
        }
        if crate::icons::button(ui, "rotate-cw", 28.0, false, tl!("Rotate clockwise")).clicked() {
            turn(&mut i, 3);
            out = Some(ImageAction::Update(AddedContent::Image(i.clone())));
        }
        if crate::icons::button(ui, FLIP_HORIZONTAL_ICON, 28.0, false, tl!("Flip horizontal")).clicked() {
            i.flip_h = !i.flip_h;
            out = Some(ImageAction::Update(AddedContent::Image(i.clone())));
        }
        if crate::icons::button(ui, FLIP_VERTICAL_ICON, 28.0, false, tl!("Flip vertical")).clicked() {
            i.flip_v = !i.flip_v;
            out = Some(ImageAction::Update(AddedContent::Image(i.clone())));
        }
        if crate::icons::button(ui, "replace", 28.0, false, tl!("Replace image")).clicked() {
            out = Some(ImageAction::Replace);
        }
    });
    ui.label(egui::RichText::new(tl!("Crop (% trimmed from each side)")).small().color(t.text_muted));
    let mut crop = img.crop.map(|v| (v * 100.0).round());
    let mut changed = false;
    ui.horizontal(|ui| {
        for (k, label) in ["L", "B", "R", "T"].into_iter().enumerate() {
            ui.label(label);
            // Applied when the drag ends, so a drag is one undo step.
            let r = ui.add(egui::DragValue::new(&mut crop[k]).range(0.0..=45.0).speed(0.5).suffix("%"));
            changed |= r.drag_stopped() || (r.changed() && !r.dragged());
        }
    });
    if changed {
        let mut i = img.clone();
        i.crop = crop.map(|v| v / 100.0);
        if i.crop != img.crop {
            out = Some(ImageAction::Update(AddedContent::Image(i)));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use egui_kittest::kittest::Queryable;

    use super::*;

    /// Copied files win over the picture beside them (Finder's icon of the file), which then
    /// isn't read at all.
    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn copied_files_come_before_the_clipboard_picture() {
        use crate::create_ui::Clip;
        let files = vec![std::path::PathBuf::from("/tmp/a.png")];
        let unread = || -> Option<Clip> { panic!("the picture is read only without files") };
        assert_eq!(clipboard_picture(files.clone(), unread), Some(Clip::Files(files)));
        let icon = Clip::Image { width: 1, height: 1, rgba: vec![0; 4] };
        assert_eq!(clipboard_picture(Vec::new(), || Some(icon.clone())), Some(icon));
    }

    /// The files of one paste share a cap: once it is spent, the rest aren't read whole, and say
    /// which cap they were over.
    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn a_paste_reads_its_files_within_one_total() {
        let sizes = [("b.png", 20usize), ("a.png", 6), ("c.png", 3), ("d.png", 5), ("e.png", 1)];
        let read = |p: &std::path::Path, cap: u64| -> std::io::Result<Option<Vec<u8>>> {
            let name = p.to_string_lossy();
            let size = sizes.iter().find(|(n, _)| name.ends_with(n)).map_or(0, |(_, s)| *s);
            Ok((size as u64 <= cap).then(|| vec![0; size]))
        };
        let paths = sizes.iter().map(|(n, _)| std::path::PathBuf::from(format!("/tmp/{n}")));
        let got: Vec<_> = read_within(paths, 10, 12, read).into_iter().map(|(_, r)| r).collect();
        assert_eq!(
            got,
            [FileRead::TooLarge, FileRead::Bytes(vec![0; 6]), FileRead::Bytes(vec![0; 3]), FileRead::OverTotal, FileRead::Bytes(vec![0; 1])]
        );
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn a_pasted_file_over_the_cap_is_not_read_whole() {
        let path = std::env::temp_dir().join(format!("pdfcraft-read-capped-{}", std::process::id()));
        std::fs::write(&path, [7u8; 10]).unwrap();
        assert_eq!(read_capped(&path, 10).unwrap(), Some(vec![7u8; 10]));
        assert_eq!(read_capped(&path, 9).unwrap(), None);
        std::fs::remove_file(&path).unwrap();
    }

    /// A named pipe called like a picture is refused without opening it, which would wait for a
    /// writer forever.
    #[cfg(unix)]
    #[test]
    fn a_pasted_named_pipe_is_refused_without_waiting() {
        let dir = std::env::temp_dir().join(format!("pdfcraft-read-fifo-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let fifo = dir.join("photo.png");
        assert!(std::process::Command::new("mkfifo").arg(&fifo).status().unwrap().success());
        let (tx, rx) = std::sync::mpsc::channel();
        let path = fifo.clone();
        std::thread::spawn(move || {
            let _ = tx.send(read_capped(&path, 10).map_err(|e| e.kind()));
        });
        let result = rx.recv_timeout(std::time::Duration::from_secs(5));
        // Let a reader that did block go: open the pipe for writing once.
        if result.is_err() {
            let _ = std::fs::OpenOptions::new().write(true).open(&fifo);
        }
        std::fs::remove_dir_all(&dir).unwrap();
        assert_eq!(result.expect("the read returns at once"), Err(std::io::ErrorKind::InvalidInput));
    }

    /// A path that became a named pipe after it was looked at: the open itself doesn't wait,
    /// and the pipe is refused.
    #[cfg(unix)]
    #[test]
    fn opening_a_named_pipe_does_not_wait() {
        let dir = std::env::temp_dir().join(format!("pdfcraft-open-fifo-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let fifo = dir.join("photo.png");
        assert!(std::process::Command::new("mkfifo").arg(&fifo).status().unwrap().success());
        let (tx, rx) = std::sync::mpsc::channel();
        let path = fifo.clone();
        std::thread::spawn(move || {
            let _ = tx.send(open_regular(&path).map(|_| ()).map_err(|e| e.kind()));
        });
        let result = rx.recv_timeout(std::time::Duration::from_secs(5));
        if result.is_err() {
            let _ = std::fs::OpenOptions::new().write(true).open(&fifo);
        }
        std::fs::remove_dir_all(&dir).unwrap();
        assert_eq!(result.expect("the open returns at once"), Err(std::io::ErrorKind::InvalidInput));
    }

    /// The image a flip button leaves behind, as the panel returns it.
    fn flipped(tooltip: &str) -> pdfcraft_engine::AddedImage {
        let img = pdfcraft_engine::AddedImage::new([0.0, 0.0, 100.0, 50.0], pdfcraft_cos::ObjRef::new(7, 0));
        let mut fonts = false;
        let mut h = egui_kittest::Harness::new_ui_state(
            move |ui, out: &mut Option<pdfcraft_engine::AddedImage>| {
                // The app's fonts (the semibold section title) take effect from the next frame.
                if !fonts {
                    ui.ctx().set_fonts(crate::theme::font_definitions());
                    fonts = true;
                    return;
                }
                let t = Tokens::get(ui.ctx());
                if let Some(ImageAction::Update(AddedContent::Image(i))) = image_panel(ui, &t, &img) {
                    *out = Some(i);
                }
            },
            None,
        );
        h.run();
        h.get_by_label(tooltip).click();
        h.run();
        h.state().clone().expect("the button updates the image")
    }

    #[test]
    fn flip_buttons_show_the_mirror_axis_of_the_flip_they_make() {
        // #1834: Acrobat's Flip horizontal swaps left and right across a vertical axis. Lucide names
        // its flip icons after the axis drawn, so that is its flip-vertical-2.
        let svg = |name: &str| {
            let (_, bytes) = crate::icon_data::ICONS.iter().find(|(n, _)| *n == name).expect("bundled icon");
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
}
