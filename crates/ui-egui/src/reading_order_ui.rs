//! Accessibility ▸ Fix reading order: the Reading Order tool. The page's tagged content shows as
//! numbered regions in reading (tag) order and untagged content in another colour. Click a
//! region or content, or drag a rectangle over it, then tag the selection as a paragraph,
//! heading, figure, table or list, or mark it as an artifact. The order list reorders regions:
//! drag a row, or Move up / Move down.

use std::collections::BTreeSet;
use std::sync::Arc;

use egui::{Color32, CornerRadius, FontId, Pos2, Rect, Stroke, Ui};
use pdfcraft_cos::ObjRef;
use pdfcraft_engine::a11y::{Destination, PageTagging, Selection, UnitKind, UnitMark};
use pdfcraft_engine::{DocId, Document, Edit};
use pdfcraft_render::DocInfo;

use crate::canvas::PageXform;
use crate::theme::Tokens;
use crate::widgets;

const REGION: Color32 = Color32::from_rgb(0x1B, 0x63, 0xE0);
const UNTAGGED: Color32 = Color32::from_rgb(0xD9, 0x48, 0x2B);
const SELECTED: Color32 = Color32::from_rgb(0x0E, 0x9F, 0x6E);

/// The tags the tool's buttons apply, as Acrobat's Reading Order dialog offers them.
pub(crate) const KINDS: [&str; 10] = ["P", "H1", "H2", "H3", "H4", "H5", "H6", "Figure", "Table", "L"];

/// What the panel asks the app to do.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Action {
    Edit(Box<Edit>),
    /// Select this tag in the Tags panel too.
    SelectTag(ObjRef),
}

#[derive(Clone, Debug, Default)]
pub(crate) struct State {
    /// Pages as the tool sees them, per (document, edit generation, page).
    cache: Vec<(DocId, u64, usize, Arc<PageTagging>)>,
    /// What the selection belongs to: (document, edit generation). An edit renumbers the units.
    owner: Option<(DocId, u64)>,
    /// The selected units and their page.
    pub page: Option<usize>,
    pub units: BTreeSet<usize>,
    /// A selected annotation.
    pub annotation: Option<ObjRef>,
    /// A rectangle being drawn: (page, start).
    drag: Option<(usize, Pos2)>,
    /// Alternate text for a new Figure.
    pub alt: String,
    /// A region clicked on the page: its tag is selected in the Tags panel.
    pub picked: Option<ObjRef>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct RegionDrag(ObjRef);

impl State {
    /// The page as the tool sees it, read again only after an edit.
    pub(crate) fn page(&mut self, doc: &Document, page: usize) -> Option<Arc<PageTagging>> {
        let generation = doc.edit_generation();
        if let Some((_, _, _, t)) = self.cache.iter().find(|(d, g, p, _)| *d == doc.id && *g == generation && *p == page) {
            return Some(t.clone());
        }
        let t = Arc::new(doc.page_tagging(page).ok()?);
        self.cache.retain(|(d, g, _, _)| *d == doc.id && *g == generation);
        if self.cache.len() >= 8 {
            self.cache.remove(0);
        }
        self.cache.push((doc.id, generation, page, t.clone()));
        Some(t)
    }

    /// Forget a selection made on another document or before an edit.
    fn sync(&mut self, doc: &Document) {
        let now = Some((doc.id, doc.edit_generation()));
        if self.owner != now {
            // An edit keeps the page; another document's pages are not the one selected.
            if self.owner.map(|(id, _)| id) != Some(doc.id) {
                self.page = None;
            }
            self.owner = now;
            self.units.clear();
            self.annotation = None;
            self.drag = None;
        }
    }

    fn select(&mut self, page: usize, add: bool) {
        if !add || self.page != Some(page) {
            self.units.clear();
            self.annotation = None;
        }
        self.page = Some(page);
    }
}

fn to_user(xf: &PageXform, info: &DocInfo, page: usize, p: Pos2) -> [f64; 2] {
    let (vx, vy) = xf.screen_to_view(p);
    let u = info.pages.get(page).map_or([0.0, 0.0], |pg| pg.view_to_user(vx, vy));
    [f64::from(u[0]), f64::from(u[1])]
}

fn screen(xf: &PageXform, info: &DocInfo, page: usize, b: [f64; 4]) -> Rect {
    xf.user_rect(info, page, [b[0] as f32, b[1] as f32, b[2] as f32, b[3] as f32])
}

/// Is a box mostly inside a rectangle (its centre in it, or half its area)?
fn covered(r: [f64; 4], b: [f64; 4]) -> bool {
    let (cx, cy) = ((b[0] + b[2]) / 2.0, (b[1] + b[3]) / 2.0);
    let w = (b[2].min(r[2]) - b[0].max(r[0])).max(0.0);
    let h = (b[3].min(r[3]) - b[1].max(r[1])).max(0.0);
    let area = (b[2] - b[0]) * (b[3] - b[1]);
    (r[0] <= cx && cx <= r[2] && r[1] <= cy && cy <= r[3]) || (area > 0.0 && w * h >= area * 0.5)
}

fn inside(b: [f64; 4], p: [f64; 2]) -> bool {
    b[0] <= p[0] && p[0] <= b[2] && b[1] <= p[1] && p[1] <= b[3]
}

fn area(b: [f64; 4]) -> f64 {
    (b[2] - b[0]).abs() * (b[3] - b[1]).abs()
}

/// Reading Order tool input on one page. Returns `true` when the gesture belongs to the tool.
pub(crate) fn page_input(
    ui: &egui::Ui,
    resp: &egui::Response,
    xf: &PageXform,
    page: usize,
    info: &DocInfo,
    doc: &Document,
    state: &mut State,
) -> bool {
    state.sync(doc);
    let pointer = ui.input(|i| i.pointer.hover_pos().or(i.pointer.interact_pos()));
    let add = ui.input(|i| i.modifiers.shift || i.modifiers.command);
    if let Some((dp, start)) = state.drag
        && dp == page
    {
        if resp.drag_stopped() || !ui.input(|i| i.pointer.primary_down()) {
            state.drag = None;
            let r = Rect::from_two_pos(start, pointer.unwrap_or(start)).intersect(xf.rect);
            if r.width() >= 3.0
                && r.height() >= 3.0
                && let Some(listing) = state.page(doc, page)
            {
                let (a, b) = (to_user(xf, info, page, r.min), to_user(xf, info, page, r.max));
                let rect = [a[0].min(b[0]), a[1].min(b[1]), a[0].max(b[0]), a[1].max(b[1])];
                state.select(page, add);
                state.units.extend(listing.units.iter().filter(|u| u.bbox.is_some_and(|b| covered(rect, b))).map(|u| u.index));
                state.annotation =
                    listing.annotations.iter().find(|n| n.rect.is_some_and(|b| covered(rect, b))).map(|n| n.object).or(state.annotation);
            }
        }
        return true;
    }
    let Some(p) = pointer.filter(|p| xf.rect.contains(*p)) else { return false };
    ui.ctx().set_cursor_icon(egui::CursorIcon::Crosshair);
    if resp.clicked() {
        let Some(listing) = state.page(doc, page) else { return true };
        let at = to_user(xf, info, page, p);
        state.select(page, add);
        // The smallest unit under the pointer; a tagged one selects its whole region.
        let unit = listing
            .units
            .iter()
            .filter(|u| u.bbox.is_some_and(|b| inside(b, at)))
            .min_by(|a, b| area(a.bbox.unwrap_or_default()).total_cmp(&area(b.bbox.unwrap_or_default())));
        match unit.map(|u| (u.index, u.mark)) {
            Some((_, UnitMark::Tagged { tag, .. })) => {
                if let Some(region) = listing.regions.iter().find(|g| g.tag == tag) {
                    state.units.extend(region.units.iter().copied());
                }
                state.picked = Some(tag);
            }
            Some((index, _)) => {
                state.units.insert(index);
            }
            None => {
                let hit = listing.annotations.iter().find(|n| n.rect.is_some_and(|b| inside(b, at)));
                state.annotation = hit.map(|n| n.object);
                state.picked = hit.and_then(|n| n.tag);
            }
        }
        return true;
    }
    if resp.drag_started() {
        let origin = ui.input(|i| i.pointer.press_origin()).unwrap_or(p);
        state.drag = Some((page, origin));
        return true;
    }
    resp.is_pointer_button_down_on()
}

/// Numbered regions, untagged content, the selection and the rectangle being drawn.
pub(crate) fn paint(ui: &egui::Ui, painter: &egui::Painter, xf: &PageXform, page: usize, info: &DocInfo, doc: &Document, state: &mut State) {
    let Some(listing) = state.page(doc, page) else { return };
    for u in listing.units.iter().filter(|u| u.is_untagged()) {
        if let Some(b) = u.bbox {
            let r = screen(xf, info, page, b);
            painter.rect_filled(r, CornerRadius::ZERO, UNTAGGED.gamma_multiply(0.10));
            painter.rect_stroke(r, CornerRadius::ZERO, Stroke::new(1.0, UNTAGGED.gamma_multiply(0.8)), egui::StrokeKind::Outside);
        }
    }
    for n in listing.annotations.iter().filter(|n| n.tag.is_none()) {
        if let Some(b) = n.rect {
            painter.rect_stroke(screen(xf, info, page, b), CornerRadius::ZERO, Stroke::new(1.0, UNTAGGED), egui::StrokeKind::Outside);
        }
    }
    for g in &listing.regions {
        let Some(b) = g.bbox else { continue };
        let r = screen(xf, info, page, b).expand(1.5);
        painter.rect_filled(r, CornerRadius::ZERO, REGION.gamma_multiply(0.08));
        painter.rect_stroke(r, CornerRadius::ZERO, Stroke::new(1.0, REGION.gamma_multiply(0.8)), egui::StrokeKind::Outside);
        // The number sits just left of the region, so it never covers what the region shows;
        // inside it only when the page has no room on that side.
        let galley = painter.layout_no_wrap(g.order.to_string(), FontId::proportional(10.0), Color32::WHITE);
        let size = galley.size() + egui::vec2(6.0, 2.0);
        let outside = r.left() - size.x - 2.0;
        let left = if outside >= xf.rect.left() { outside } else { r.left() };
        let badge = Rect::from_min_size(egui::pos2(left, r.top()), size);
        painter.rect_filled(badge, CornerRadius::same(2), REGION);
        painter.galley(badge.min + egui::vec2(3.0, 1.0), galley, Color32::WHITE);
    }
    if state.page == Some(page) {
        for u in listing.units.iter().filter(|u| state.units.contains(&u.index)) {
            if let Some(b) = u.bbox {
                painter.rect_stroke(
                    screen(xf, info, page, b).expand(2.5),
                    CornerRadius::same(2),
                    Stroke::new(2.0, SELECTED),
                    egui::StrokeKind::Outside,
                );
            }
        }
        if let Some(n) = state.annotation.and_then(|a| listing.annotations.iter().find(|n| n.object == a))
            && let Some(b) = n.rect
        {
            painter.rect_stroke(screen(xf, info, page, b).expand(2.5), CornerRadius::same(2), Stroke::new(2.0, SELECTED), egui::StrokeKind::Outside);
        }
    }
    if let (Some((dp, start)), Some(p)) = (state.drag, ui.input(|i| i.pointer.hover_pos()))
        && dp == page
    {
        let r = Rect::from_two_pos(start, p);
        painter.rect_filled(r, CornerRadius::ZERO, SELECTED.gamma_multiply(0.08));
        painter.rect_stroke(r, CornerRadius::ZERO, Stroke::new(1.0, SELECTED), egui::StrokeKind::Inside);
    }
}

/// An edit the panel asks for.
fn edit(e: Edit) -> Option<Action> {
    Some(Action::Edit(Box::new(e)))
}

fn kind_label(kind: UnitKind) -> &'static str {
    match kind {
        UnitKind::Text => tl!("Text"),
        UnitKind::Image => tl!("Image"),
        UnitKind::Graphic => tl!("Graphic"),
    }
}

fn short(text: &str) -> String {
    let t: String = text.chars().take(48).collect();
    if text.chars().count() > 48 { format!("{t}…") } else { t }
}

/// The tool's panel: tag buttons, the order list and the untagged content of the page.
pub(crate) fn panel(ui: &mut Ui, t: &Tokens, doc: &Document, current: usize, state: &mut State, can_edit: bool) -> Option<Action> {
    state.sync(doc);
    let page = state.page.unwrap_or(current);
    let Some(listing) = state.page(doc, page) else {
        ui.label(tl!("This document has no editable structure tags."));
        return None;
    };
    let mut action = None;
    ui.label(egui::RichText::new(tl!("Draw a rectangle on the page or click content, then choose its tag.")).small().color(t.text_muted));
    let chosen: Vec<usize> = state.units.iter().copied().collect();
    if !chosen.is_empty() {
        ui.label(egui::RichText::new(crate::i18n::fmt(tl!("{n} selected"), &[("n", &chosen.len().to_string())])).small().color(t.accent_text));
    }
    if let Some(why) = &listing.read_only {
        ui.label(egui::RichText::new(why.as_str()).small().color(t.text_faint));
    }
    let usable = can_edit && listing.read_only.is_none() && !chosen.is_empty();
    ui.add_space(4.0);
    ui.horizontal_wrapped(|ui| {
        for kind in KINDS {
            if ui.add_enabled(usable, egui::Button::new(format!("<{kind}>"))).clicked() {
                let alt = (kind == "Figure").then(|| state.alt.trim().to_owned()).filter(|a| !a.is_empty());
                action = edit(Edit::TagContent { page, selection: Selection::Units(chosen.clone()), kind: kind.to_owned(), alt, destination: None });
            }
        }
        if ui.add_enabled(usable, egui::Button::new(tl!("Mark as artifact"))).clicked() {
            action = edit(Edit::ContentToArtifact { page, selection: Selection::Units(chosen.clone()) });
        }
    });
    let alt_label = ui.label(tl!("Alternate text"));
    ui.add(widgets::block(&mut state.alt).desired_rows(2).desired_width(f32::INFINITY)).labelled_by(alt_label.id);
    let untagged_annotation = state.annotation.filter(|a| listing.annotations.iter().any(|n| n.object == *a && n.tag.is_none()));
    if let Some(annotation) = untagged_annotation
        && ui.add_enabled(can_edit, egui::Button::new(tl!("Tag annotation"))).clicked()
    {
        action = edit(Edit::TagAnnotation { page, annotation });
    }
    ui.separator();
    // The order list: regions in reading order. Drag a row onto another to move it there.
    let selected_region = listing
        .regions
        .iter()
        .position(|g| !g.units.is_empty() && g.units.iter().all(|u| state.units.contains(u)) && g.units.len() == state.units.len());
    egui::ScrollArea::vertical().id_salt("reading-order-list").max_height(220.0).show_rows(ui, 24.0, listing.regions.len(), |ui, range| {
        for i in range {
            let Some(g) = listing.regions.get(i) else { continue };
            let label =
                if g.text.is_empty() { format!("{}  <{}>", g.order, g.kind) } else { format!("{}  <{}>  {}", g.order, g.kind, short(&g.text)) };
            let resp = ui.add(egui::Button::selectable(selected_region == Some(i), label).truncate().sense(egui::Sense::click_and_drag()));
            if resp.clicked() {
                state.page = Some(page);
                state.units = g.units.iter().copied().collect();
                state.annotation = None;
                action = Some(Action::SelectTag(g.tag));
            }
            if resp.drag_started() {
                egui::DragAndDrop::set_payload(ui.ctx(), RegionDrag(g.tag));
            }
            let below = |r: &egui::Response| r.ctx.pointer_latest_pos().is_some_and(|p| p.y > r.rect.center().y);
            if let Some(dragged) = resp.dnd_hover_payload::<RegionDrag>()
                && dragged.0 != g.tag
            {
                let y = if below(&resp) { resp.rect.bottom() } else { resp.rect.top() };
                ui.painter().hline(resp.rect.x_range(), y, Stroke::new(2.0, t.accent));
            }
            if let Some(dragged) = resp.dnd_release_payload::<RegionDrag>()
                && dragged.0 != g.tag
                && can_edit
            {
                let destination = if below(&resp) { Destination::After(g.tag) } else { Destination::Before(g.tag) };
                action = edit(Edit::MoveTagTo { object: dragged.0, destination });
            }
            resp.on_hover_text(tl!("Drag to reorder"));
        }
    });
    ui.horizontal(|ui| {
        let at = selected_region;
        let previous = at.and_then(|i| i.checked_sub(1)).and_then(|i| listing.regions.get(i));
        let next = at.and_then(|i| listing.regions.get(i.saturating_add(1)));
        let this = at.and_then(|i| listing.regions.get(i));
        if ui.add_enabled(can_edit && previous.is_some(), egui::Button::new(tl!("Move up"))).clicked()
            && let (Some(this), Some(previous)) = (this, previous)
        {
            action = edit(Edit::MoveTagTo { object: this.tag, destination: Destination::Before(previous.tag) });
        }
        if ui.add_enabled(can_edit && next.is_some(), egui::Button::new(tl!("Move down"))).clicked()
            && let (Some(this), Some(next)) = (this, next)
        {
            action = edit(Edit::MoveTagTo { object: this.tag, destination: Destination::After(next.tag) });
        }
    });
    let untagged: Vec<_> = listing.units.iter().filter(|u| u.is_untagged()).collect();
    let loose: Vec<_> = listing.annotations.iter().filter(|n| n.tag.is_none()).collect();
    if !untagged.is_empty() || !loose.is_empty() {
        ui.separator();
        ui.label(egui::RichText::new(tl!("Untagged content")).strong());
        egui::ScrollArea::vertical().id_salt("reading-order-untagged").max_height(160.0).show(ui, |ui| {
            for u in untagged {
                let text = if u.text.is_empty() { kind_label(u.kind).to_owned() } else { format!("{} · {}", kind_label(u.kind), short(&u.text)) };
                if ui.add(egui::Button::selectable(state.units.contains(&u.index), text)).clicked() {
                    let add = ui.input(|i| i.modifiers.shift || i.modifiers.command);
                    state.select(page, add);
                    state.units.insert(u.index);
                }
            }
            for n in loose {
                let text = format!("{} · {}", tl!("Annotation"), n.subtype);
                if ui.add(egui::Button::selectable(state.annotation == Some(n.object), text)).clicked() {
                    state.select(page, false);
                    state.annotation = Some(n.object);
                }
            }
        });
    }
    action
}
