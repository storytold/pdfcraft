//! Prepare a form on the page: the Add-field and Select tool (`PrepareTool`), the parts that make
//! fields pressable (`page_parts`), the press handler for those parts (`field_press`), the
//! selection's resize box (`selection_box`) and the fields' look (`PrepareOverlay`).

use craft::behavior::geometry::{BoxAllow, BoxHandles, BoxRules, BoxState, ConstrainStyle, Directions, MoveGesture, ScaleRule};
use craft::behavior::item_marquee::MarqueeSelect;
use craft::behavior::kurbo::{Point, Rect as KRect};
use craft::behavior::press::{PointerButton, PressPhase};
use craft::behavior::{CancelReason, Cursor, CursorIcon, EditEvent, Input, Metrics};
use craft::draw::ui::transform::TransformBox;
use craft::pages::ui::PlacedPage;
use craft::ui::canvas::{CanvasTool, ToolAction, ToolClaims, ToolContext, ToolKey, ToolPath, ToolPointer};
use craft::ui::contract::{Bind, Common, Component, Cx, MeasureCx, PaintUi, Props, Role, Show as _};
use craft::ui::primitives::{Draw, PaintShape, StrokePlace, StrokeWidth, TextRun};
use craft::ui::style::{ComponentStyle, StyleColor, StyleCx, TypeRole};
use egui::{CornerRadius, Pos2, Rect, Response, Ui, Vec2, vec2};
use pdfcraft_engine::{Edit, FieldProps, FormField};
use pdfcraft_render::DocInfo;

use super::{FieldTool, Gesture, PrepareView, delete_selected_from, place_tab_badge, screen_rect, table_rect, to_user, user_rect};
use crate::canvas::page_parts::{Owner, PageLayer, PagePart, PagePress};
use crate::canvas::{PageBoxDrag, PageSession, PageXform};

/// A drag under this many screen pixels either way is a click, which places the default size.
const CLICK_SIZE: f32 = 4.0;

/// The transforms of the pages placed this pass, with the view's `rotation` and the page sizes of `info`.
fn transforms(pages: &[PlacedPage], info: &DocInfo, rotation: u16) -> Vec<(usize, PageXform)> {
    pages
        .iter()
        .map(|placed| {
            let size = info.pages.get(placed.page).map_or((1.0, 1.0), |p| (p.width, p.height));
            (placed.page, PageXform::of(placed, rotation, size))
        })
        .collect()
}

/// The transform of `page` among `xforms`.
fn transform_of(xforms: &[(usize, PageXform)], page: usize) -> Option<PageXform> {
    xforms.iter().find(|(p, _)| *p == page).map(|(_, xf)| *xf)
}

/// One part per widget on the placed `pages`, for the pages' router: a press on a field selects it, a drag of it
/// moves it. The caller asks only while preparing with no field tool armed, because a field tool draws
/// anywhere and fields are then not pressable.
// The parts read nothing from the session; the page body hands it over so every owner's `page_parts` has one shape.
pub(crate) fn page_parts(
    out: &mut Vec<PagePart>,
    _session: &PageSession,
    form: &[FormField],
    info: &DocInfo,
    pages: &[PlacedPage],
    rotation: u16,
    field_tool: bool,
) {
    for (page, xf) in transforms(pages, info, rotation) {
        for (field, f) in form.iter().enumerate() {
            for (widget, w) in f.widgets.iter().enumerate().filter(|(_, w)| w.page == Some(page)) {
                let rect = screen_rect(&xf, info, page, w.rect);
                let part = PagePart::new(Owner::PrepareField { field, widget }, page, rect, PageLayer::Placed);
                // A field tool draws new fields over the existing ones; a right-click still reaches them.
                out.push(if field_tool { part.context_only() } else { part.cursor(CursorIcon::Move).passes_drag() });
            }
        }
    }
}

/// A press on a field part: a click selects it (Shift or ⌘ toggles it in the selection), a double
/// click opens Field Properties, a secondary press selects it for the field menu. A drag that follows
/// goes to [`PrepareTool`].
pub(crate) fn field_press(session: &mut PageSession, form: &[FormField], press: &PagePress) {
    let Owner::PrepareField { field, widget } = press.owner else { return };
    let Some(start) = press.phase.start() else { return };
    let Some(f) = form.get(field) else { return };
    match start.button {
        PointerButton::Secondary if matches!(press.phase, PressPhase::Down { .. } | PressPhase::Release { click: true, .. }) => {
            super::context(form, field, widget, session)
        }
        PointerButton::Primary if matches!(press.phase, PressPhase::Down { .. }) => {
            let extend = press.mods.shift || press.mods.cmd;
            session.prepare.select((f.name.clone(), widget), extend);
            // The `Down` of a second primary press: the one double press, as `PressState::double_pressed` reads it.
            if start.clicks >= 2 && !extend {
                session.prepare.outcome.properties = true;
            }
        }
        _ => {}
    }
}

/// The widget `key` names, and the page it is on.
fn widget_of<'f>(form: &'f [FormField], key: &(String, usize)) -> Option<(usize, &'f pdfcraft_engine::FormWidget)> {
    let w = form.iter().find(|f| f.name == key.0)?.widgets.get(key.1)?;
    Some((w.page?, w))
}

/// The screen rect of widget `key` and its page.
fn widget_rect(xforms: &[(usize, PageXform)], info: &DocInfo, form: &[FormField], key: &(String, usize)) -> Option<(usize, Rect)> {
    let (page, w) = widget_of(form, key)?;
    let xf = transform_of(xforms, page)?;
    Some((page, screen_rect(&xf, info, page, w.rect)))
}

fn kurbo_rect(r: Rect) -> KRect {
    KRect::new(f64::from(r.min.x), f64::from(r.min.y), f64::from(r.max.x), f64::from(r.max.y))
}

fn point(p: Pos2) -> Point {
    Point::new(f64::from(p.x), f64::from(p.y))
}

/// Prepare Form's tool: with a field tool armed it draws a field, otherwise it moves the selected
/// field and sweeps a selection rectangle. The gesture lives in [`PrepareView`], because the host is
/// handed a new tool every pass.
pub(crate) struct PrepareTool<'a> {
    tool: Option<FieldTool>,
    allowed: bool,
    form: &'a [FormField],
    prepare: &'a mut PrepareView,
    placed: &'a [PlacedPage],
    info: &'a DocInfo,
    /// The transforms of the pages placed this pass.
    xforms: &'a [(usize, PageXform)],
}

impl<'a> PrepareTool<'a> {
    /// The tool for this pass. `allowed` is false while a table window is open (nothing is drawn or
    /// moved then).
    pub(crate) fn new(
        tool: Option<FieldTool>,
        allowed: bool,
        form: &'a [FormField],
        prepare: &'a mut PrepareView,
        placed: &'a [PlacedPage],
        info: &'a DocInfo,
        xforms: &'a [(usize, PageXform)],
    ) -> Self {
        Self { tool, allowed, form, prepare, placed, info, xforms }
    }

    /// Whether a pointer gesture is currently active in Prepare.
    pub(crate) fn has_gesture(&self) -> bool {
        !matches!(self.prepare.gesture, Gesture::Idle)
    }

    /// Whether the active gesture is a marquee press that has not dragged past click distance.
    pub(crate) fn is_click_marquee(&self) -> bool {
        matches!(self.prepare.gesture, Gesture::Marquee { dragged: false, .. })
    }

    /// Every widget key and screen rect on every shown page.
    fn all_widgets(&self) -> Vec<((String, usize), Rect)> {
        let mut out = Vec::new();
        for (page, xf) in self.xforms {
            for f in self.form {
                for (wi, w) in f.widgets.iter().enumerate().filter(|(_, w)| w.page == Some(*page)) {
                    out.push(((f.name.clone(), wi), screen_rect(xf, self.info, *page, w.rect)));
                }
            }
        }
        out
    }

    fn finish_draw(&mut self, mut drag: PageBoxDrag, origin: Pos2) {
        let Some(tool) = self.tool else { return };
        // Any box, a click's included: a click places the default size below.
        let Some((page, rect)) = drag.release(0.0) else { return };
        let Some(xf) = transform_of(self.xforms, page) else { return };
        let click = rect.width() < CLICK_SIZE || rect.height() < CLICK_SIZE;
        if tool == FieldTool::TextTable {
            let area = if click { Rect::from_min_size(origin, Vec2::ZERO) } else { rect };
            self.prepare.outcome.table = Some((page, table_rect(&xf, area)));
            self.prepare.outcome.placed = true;
            return;
        }
        let user = if click {
            // A click: the default size, top-left at the pointer.
            let at = to_user(&xf, self.info, page, origin);
            let (w, h) = tool.default_size();
            [at[0], at[1] - h, at[0] + w, at[1]]
        } else {
            user_rect(&xf, self.info, page, rect)
        };
        let join = self.prepare.selected.as_ref().and_then(|(n, _)| self.form.iter().find(|f| &f.name == n));
        let kind = tool.new_field(join);
        self.prepare.select_added = Some(self.form.len());
        self.prepare.outcome.edit = Some(Edit::AddField { page, rect: user, kind, name: None });
        self.prepare.outcome.placed = true;
    }

    fn finish_move(&mut self, page: usize, offset: craft::behavior::kurbo::Vec2, widget: (String, usize)) {
        // A move under a point changes nothing.
        if offset.hypot() < 1.0 {
            return;
        }
        let Some((_, rect)) = widget_rect(self.xforms, self.info, self.form, &widget) else { return };
        let Some(xf) = transform_of(self.xforms, page) else { return };
        let moved = rect.translate(vec2(offset.x as f32, offset.y as f32));
        let user = user_rect(&xf, self.info, page, moved);
        let (name, wi) = widget;
        self.prepare.outcome.edit = Some(Edit::SetFieldProps { name, props: Box::new(FieldProps { rect: Some((wi, user)), ..Default::default() }) });
    }

    /// An arrow moves the selected field a point the way it points on screen, Shift and an arrow ten points, whatever
    /// the zoom; one press is one edit. Whether there was a field to move.
    fn nudge(&mut self, direction: Vec2, shift: bool) -> bool {
        let Some(key) = self.prepare.selected.clone() else { return false };
        let Some((page, rect)) = widget_rect(self.xforms, self.info, self.form, &key) else { return false };
        let Some(xf) = transform_of(self.xforms, page) else { return false };
        // Screen points per page point: the page's width on screen over its width as turned.
        let across = if xf.rot % 180 == 90 { xf.ph } else { xf.pw };
        let zoom = xf.rect.width() / across.max(1.0);
        let step = if shift { 10.0 } else { 1.0 };
        let user = user_rect(&xf, self.info, page, rect.translate(direction * step * zoom));
        let (name, wi) = key;
        self.prepare.outcome.edit = Some(Edit::SetFieldProps { name, props: Box::new(FieldProps { rect: Some((wi, user)), ..Default::default() }) });
        true
    }

    fn finish_marquee(&mut self, marquee: MarqueeSelect<(String, usize)>) {
        let hits = self.all_widgets().into_iter().filter(|(_, r)| r.intersects(marquee.rect())).collect::<Vec<_>>();
        let mut items = marquee.selection(hits).into_iter();
        self.prepare.selected = items.next();
        self.prepare.also = items.collect();
    }
}

impl CanvasTool for PrepareTool<'_> {
    fn press(&mut self, pointer: &ToolPointer, cx: &mut ToolContext<'_>) -> ToolAction {
        if !self.allowed || pointer.button != PointerButton::Primary {
            return ToolAction::Ignored;
        }
        let Some(placed) = self.placed.iter().find(|p| p.rect.contains(pointer.pos)).cloned() else { return ToolAction::Ignored };
        if self.tool.is_some() {
            let mut drag = PageBoxDrag::default();
            drag.press(&placed, pointer.pos);
            self.prepare.gesture = Gesture::Draw { drag, origin: pointer.pos };
            return ToolAction::Claimed;
        }
        // A drag that began on a field part moves the field the part's press selected.
        let on_selected = self.prepare.selected.clone().filter(|key| {
            widget_rect(self.xforms, self.info, self.form, key).is_some_and(|(page, r)| page == placed.page && r.contains(pointer.pos))
        });
        if let Some(widget) = on_selected {
            let slop = f64::from(cx.slop);
            let gesture = MoveGesture::press(&ConstrainStyle::new(slop, slop), point(pointer.pos), pointer.mods, Directions::Four);
            self.prepare.gesture = Gesture::Move { page: placed.page, gesture, widget };
            return ToolAction::Claimed;
        }
        let extend = pointer.mods.shift || pointer.mods.cmd;
        let base = if extend { self.prepare.selected.iter().chain(self.prepare.also.iter()).cloned().collect() } else { Vec::new() };
        self.prepare.gesture = Gesture::Marquee { marquee: MarqueeSelect::begin(pointer.pos, pointer.mods, base), dragged: false };
        ToolAction::Claimed
    }

    fn drag(&mut self, pointer: &ToolPointer, _cx: &mut ToolContext<'_>) {
        match &mut self.prepare.gesture {
            Gesture::Draw { drag, .. } => drag.drag(pointer.pos),
            Gesture::Marquee { marquee, dragged } => {
                *dragged = true;
                marquee.update(pointer.pos);
            }
            Gesture::Move { gesture, .. } => {
                gesture.follow(point(pointer.pos), pointer.mods, 1.0);
            }
            Gesture::Idle => {}
        }
    }

    fn release(&mut self, pointer: &ToolPointer, _cx: &mut ToolContext<'_>) {
        match std::mem::take(&mut self.prepare.gesture) {
            Gesture::Idle => {}
            Gesture::Draw { mut drag, origin } => {
                drag.drag(pointer.pos);
                self.finish_draw(drag, origin);
            }
            Gesture::Marquee { mut marquee, .. } => {
                marquee.update(pointer.pos);
                self.finish_marquee(marquee);
            }
            Gesture::Move { page, gesture, widget } => {
                let value = gesture.release(point(pointer.pos), pointer.mods, 1.0);
                self.finish_move(page, value.delta, widget);
            }
        }
    }

    fn cancel(&mut self, _reason: CancelReason, _cx: &mut ToolContext<'_>) {
        if let Gesture::Marquee { marquee, .. } = std::mem::take(&mut self.prepare.gesture) {
            let mut before = marquee.cancel().into_iter();
            self.prepare.selected = before.next();
            self.prepare.also = before.collect();
        }
    }

    fn claims(&self) -> ToolClaims {
        if self.prepare.selected.is_some() { ToolClaims::NONE.deletes() } else { ToolClaims::NONE }
    }

    fn key(&mut self, key: &ToolKey, _cx: &mut ToolContext<'_>) -> bool {
        match key {
            ToolKey::Delete { .. } if self.prepare.selected.is_some() => {
                self.prepare.outcome.edit = delete_selected_from(self.prepare);
                true
            }
            ToolKey::Nudge { direction, mods, .. } => self.nudge(vec2(direction.x as f32, direction.y as f32), mods.shift),
            ToolKey::Cancel if self.prepare.selected.is_some() => {
                self.prepare.selected = None;
                self.prepare.also.clear();
                true
            }
            _ => false,
        }
    }

    fn paint(&self, _draw: &Draw<'_>, cx: &ToolContext<'_>) {
        match &self.prepare.gesture {
            Gesture::Draw { drag, .. } => drag.paint(cx),
            Gesture::Marquee { marquee, .. } => {
                let r = marquee.rect();
                cx.marching_ants(&[r.left_top(), r.right_top(), r.right_bottom(), r.left_bottom()], true);
            }
            Gesture::Move { .. } | Gesture::Idle => {}
        }
    }

    fn cursor(&self, _cx: &ToolContext<'_>) -> Cursor {
        if self.tool.is_some() { Cursor::from(CursorIcon::Crosshair) } else { Cursor::from(CursorIcon::Default) }
    }

    fn command_form(&self) -> ToolPath {
        ToolPath::Command("the Field Properties dialog sets a field's position and size")
    }
}

/// The corner handles of the anchor field, as a Free Transform box that only resizes: no rotating,
/// skewing or distorting, and a press inside is the field part's. A corner drag's `Commit` and an
/// arrow nudge's `Set` each become one field edit; Escape with no drag clears the selection.
pub(crate) fn selection_box(
    ui: &mut Ui,
    body: Rect,
    view: craft::behavior::CanvasView,
    session: &mut PageSession,
    form: &[FormField],
    info: &DocInfo,
    xforms: &[(usize, PageXform)],
    page: usize,
) {
    let Some(key) = session.prepare.selected.clone() else { return };
    let Some((widget_page, rect)) = widget_rect(xforms, info, form, &key) else { return };
    if widget_page != page {
        return;
    }
    let Some(xf) = transform_of(xforms, page) else { return };
    let rules = BoxRules::default()
        .with_scale(ScaleRule::Free)
        .with_allow(BoxAllow::new(false, false, false).with_warp(false))
        .with_handles(BoxHandles::Corners);
    let start = KRect::from_points(view.to_doc(rect.min), view.to_doc(rect.max));
    let shown = TransformBox::new("Field", Bind::given(BoxState::from_rect(start)), start, 0.0)
        .rules(rules)
        .reference_point(false)
        .inside_moves(false)
        .show_at(ui, body);
    let mut edit = None;
    for event in &shown.edit.events {
        let state = match event {
            EditEvent::Commit { after, .. } => after,
            EditEvent::Set { after, .. } => after,
            _ => continue,
        };
        let corners = state.quad.map(|p| view.to_screen(p));
        let screen = Rect::from_points(&corners);
        edit = Some(user_rect(&xf, info, page, screen));
    }
    if let Some(user) = edit {
        let (name, wi) = key;
        session.pending_edit = Some(Edit::SetFieldProps { name, props: Box::new(FieldProps { rect: Some((wi, user)), ..Default::default() }) });
    } else if shown.discard {
        session.prepare.selected = None;
        session.prepare.also.clear();
    }
}

/// While preparing: the fields' look on each of the placed `pages` and the selected field's resize box, over the
/// page body at `body`.
pub(crate) fn show_overlays(
    ui: &mut Ui,
    body: Rect,
    view: craft::behavior::CanvasView,
    session: &mut PageSession,
    form: &[FormField],
    info: &DocInfo,
    pages: &[PlacedPage],
    rotation: u16,
) {
    let xforms = transforms(pages, info, rotation);
    let tab_order = if session.prepare.show_tab_numbers { crate::forms_ui::tab_order(form, info) } else { Vec::new() };
    for &(page, _) in &xforms {
        if let Some(overlay) = PrepareOverlay::new(form, &tab_order, session, info, &xforms, page) {
            overlay.show_at(ui, body);
        }
        selection_box(ui, body, view, session, form, info, &xforms, page);
    }
}

/// One field as the overlay draws it.
struct OverlayField {
    rect: Rect,
    name: String,
    selected: bool,
}

/// How much of the accent the prepare overlay's field fill keeps over the page.
const FIELD_FILL_SHARE: f32 = 89.0 / 255.0;

/// The fields of one page with their fill, outline, selection and name tags, and the tab-order
/// badges while they are on. A field being moved is drawn at the moved place.
#[derive(Component, Props)]
#[component(canvas_control)]
pub(crate) struct PrepareOverlay {
    #[common]
    common: Common,
    fields: Vec<OverlayField>,
    tabs: Vec<(Rect, usize)>,
    page: Rect,
}

impl PrepareOverlay {
    /// The overlay of `page`, from the form, the tab order and the session's selection and gesture.
    pub(crate) fn new(
        form: &[FormField],
        tab_order: &[(usize, usize)],
        session: &PageSession,
        info: &DocInfo,
        xforms: &[(usize, PageXform)],
        page: usize,
    ) -> Option<Self> {
        let xf = transform_of(xforms, page)?;
        let prep: &PrepareView = &session.prepare;
        let moving = match &prep.gesture {
            Gesture::Move { page: p, gesture, widget } if *p == page => {
                let d = gesture.value().delta;
                Some((widget.clone(), vec2(d.x as f32, d.y as f32)))
            }
            _ => None,
        };
        let shown = |f: &FormField, wi: usize, w: &pdfcraft_engine::FormWidget| {
            let mut rect = screen_rect(&xf, info, page, w.rect);
            if let Some((key, by)) = &moving
                && key.0 == f.name
                && key.1 == wi
            {
                rect = rect.translate(*by);
            }
            rect
        };
        let mut fields = Vec::new();
        for f in form {
            for (wi, w) in f.widgets.iter().enumerate().filter(|(_, w)| w.page == Some(page)) {
                let key = (f.name.clone(), wi);
                let selected = prep.selected.as_ref() == Some(&key) || prep.also.contains(&key);
                fields.push(OverlayField { rect: shown(f, wi, w), name: f.name.clone(), selected });
            }
        }
        let mut tabs = Vec::new();
        if prep.show_tab_numbers {
            for (number, (fi, wi)) in tab_order.iter().copied().enumerate() {
                let Some(f) = form.get(fi) else { continue };
                let Some(w) = f.widgets.get(wi).filter(|w| w.page == Some(page)) else { continue };
                tabs.push((shown(f, wi, w), number.saturating_add(1)));
            }
        }
        Some(Self { common: Common::new(), fields, tabs, page: xf.rect }.label(tl!("Form fields")))
    }
}

/// The colours of [`PrepareOverlay`], all from theme tokens.
#[derive(Clone, Debug, PartialEq, ComponentStyle)]
#[non_exhaustive]
pub(crate) struct PrepareOverlayStyle {
    #[style(metrics)]
    metrics: Metrics,
    fill: StyleColor,
    outline: StyleColor,
    selected: StyleColor,
    /// A field's name tag: the inverse neutral pair.
    tag: StyleColor,
    tag_text: StyleColor,
    /// A selected field's name tag: the accent pair that carries text.
    selected_tag: StyleColor,
    selected_tag_text: StyleColor,
    badge: StyleColor,
}

impl Component for PrepareOverlay {
    const SINGLE_VALUE: bool = false;
    const ROLE: Role = Role::GRAPHICS_OBJECT;
    const INPUT: Input = Input::Display;
    const PARTS: bool = false;
    const STOPS: bool = false;
    const OVERLAYS: bool = false;
    type Style = PrepareOverlayStyle;
    type Output = Response;

    fn style(cx: &StyleCx<'_>, _: PrepareOverlayStyleKey) -> PrepareOverlayStyle {
        let colors = &cx.tokens.color;
        PrepareOverlayStyle {
            metrics: Metrics::new(),
            fill: cx.color(colors.accent.solid),
            outline: cx.color(colors.line.control_edge),
            selected: cx.color(colors.accent.solid),
            tag: cx.color(colors.text.normal),
            tag_text: cx.color(colors.surface.sunken),
            selected_tag: cx.color(colors.accent.solid),
            selected_tag_text: cx.color(colors.text.on_strong),
            badge: cx.color(colors.accent.solid),
        }
    }

    fn measure(&mut self, _cx: &MeasureCx<'_, Self>) -> Vec2 {
        Vec2::ZERO
    }

    fn paint(&mut self, ui: &mut PaintUi<'_>, cx: &Cx<'_, Self>) {
        let style = &cx.style;
        let draw = ui.draw();
        for field in &self.fields {
            let r = field.rect;
            // A tint of the accent over the page, as production's (89/255), so the page shows through.
            draw.faded(FIELD_FILL_SHARE, |draw| draw.fill(&PaintShape::Rect(r, CornerRadius::ZERO), style.fill));
            if !field.selected {
                draw.stroke(&PaintShape::Rect(r, CornerRadius::ZERO), StrokeWidth::Thin, style.outline);
            }
            // The full name, as Acrobat shows it: MASTER.first, not first (#2696), clipped to the field.
            let (tag_fill, tag_text) = if field.selected { (style.selected_tag, style.selected_tag_text) } else { (style.tag, style.tag_text) };
            let run = TextRun::new(field.name.clone()).role(TypeRole::Caption).color(tag_text);
            let layout = draw.layout(&run, f32::INFINITY);
            let size = layout.galley.size() + vec2(6.0, 2.0);
            let tag = Rect::from_center_size(r.center(), size).intersect(r);
            if tag.is_positive() {
                draw.fill(&PaintShape::Rect(tag, CornerRadius::same(1)), tag_fill);
                draw.galley(Rect::from_center_size(r.center(), size).min + vec2(3.0, 1.0), &layout);
            }
            // A selected field's outline goes around it, over its tag, so a small widget's tag never hides it.
            if field.selected {
                draw.stroke_placed(&PaintShape::Rect(r, CornerRadius::ZERO), StrokePlace::Outside, StrokeWidth::Thick, style.selected);
            }
        }
        let mut placed: Vec<Rect> = Vec::new();
        for (rect, number) in &self.tabs {
            let run = TextRun::new(number.to_string()).role(TypeRole::Caption).color(style.tag_text);
            let layout = draw.layout(&run, f32::INFINITY);
            let size = layout.galley.size() + vec2(8.0, 5.0);
            let Some(badge) = place_tab_badge(*rect, self.page, size, &placed) else { continue };
            placed.push(badge);
            draw.fill(&PaintShape::Rect(badge, CornerRadius::same(3)), style.badge);
            draw.galley(badge.min + vec2(4.0, 2.0), &layout);
        }
    }

    fn finish(self, cx: Cx<'_, Self>) -> Response {
        cx.response.clone()
    }
}
