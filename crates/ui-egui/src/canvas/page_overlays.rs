//! What the pages show over their content that is the viewer's own: link and text-link parts, the sign
//! tabs on empty signature fields, the form-field highlights, the compare and flash marks and the
//! validation badges. None of it enters the file, exports or printing.
//!
//! The page body shows these in the order its plan gives: the other families' overlays (comments, edit
//! controls, form-field paint), then [`PageOverlays`]. Its parts are built by [`PageLinks::parts`] and
//! [`overlay_parts`] and registered by `PageParts`, never here.
// PLAN: `mod page_overlays;` goes in `canvas.rs` (pdf-canvas-w8); the callee overlays (`comments` page overlay,
// `content_controls`, `forms_ui::paint_page`, ..) are shown by w8's page body before `PageOverlays`, because each is
// its own component and their signatures are their owners'.

use craft::behavior::{CursorIcon, HitShape, IconId, Input, PointerButton, PressPhase, Sense};
use craft::pages::ui::PlacedPage;
use craft::theme::variant::Tone;
use craft::ui::contract::{Access, Animate as _, Common, Component, Cx, MeasureCx, NoValue, Outcome, PaintUi, Props, Role, Show as _};
use craft::ui::primitives::{Draw, IconSpec, PaintShape, StrokeWidth, TextRun};
use craft::ui::style::{ComponentStyle, DataColor, Metrics, StyleColor, StyleCx, TypeRole};
use egui::{Color32, CornerRadius, Rect, Response, Vec2, pos2};
use pdfcraft_engine::{Document, FormFieldKind, LinkItem};
use pdfcraft_render::{DocInfo, Link, LinkTarget};

use super::area::{MARK_ICON_SHARE, SignTab, mark_anchor, sign_tab_on_page, sign_tabs, signature_badges, verdict_mark_rect};
use super::page_parts::{Owner, PageLayer, PagePart, PagePress};
use super::{DocView, PageSession, PageXform};
use crate::link_ui::{LINK_TOOL_MARGIN, target_label};
use crate::sign_ui::status_tone;

/// How long the flash around a jumped-to rectangle takes to fade out, in seconds.
const FLASH_SECONDS: f64 = 1.6;

/// How much of its colour a compare mark paints with, as an opacity beside [`FLASH_SECONDS`]: the mark's colour is
/// data, so its fade is a documented constant of the overlay, and it tints the page without hiding what it marks.
const COMPARE_SHARE: f32 = 0.28;

/// What a link family does with a click.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum LinkMode {
    /// Clicking follows the link (the Select tool).
    Follow,
    /// Clicking selects it, and the Link tool edits it.
    Edit,
}

/// What a press on a link part came to.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum LinkEvent {
    /// A click followed the link, or the web address written as page text. `javascript` is the page and
    /// index of the link when its action is a script, which the caller runs with the link's identity.
    Follow {
        /// Where the link goes.
        target: LinkTarget,
        /// The page and index of a link whose action is JavaScript.
        javascript: Option<(usize, usize)>,
    },
    /// A primary click selected the link; `clicks` of 2 or more opens it.
    Select {
        /// The link's page.
        page: usize,
        /// The link's index in its page's `/Annots`.
        index: usize,
        /// 1 for a single click, 2 for a double click.
        clicks: u32,
    },
    /// A secondary click set the link the context menu is about.
    Context {
        /// The link's page.
        page: usize,
        /// The link's index in its page's `/Annots`.
        index: usize,
    },
}

/// The link family of one page: the link annotations and, in `Follow`, the web and email addresses written
/// as page text. One list builds the parts and the same list reads their presses, so what the hand promises
/// is what the click does.
pub(crate) struct PageLinks<'a> {
    mode: LinkMode,
    /// The session, whose text layer the written addresses come from. The caller refreshes them
    /// (`DocView::refresh_text_links`) before the parts are built.
    session: &'a PageSession,
    info: &'a DocInfo,
    /// In `Edit`, every link the document has (the engine's listing, which holds links the renderer cannot
    /// follow too); empty in `Follow`.
    engine: &'a [LinkItem],
    pages: &'a [PlacedPage],
    rotation: u16,
    sizes: &'a [(f32, f32)],
    text_links: bool,
    unobstructed: bool,
}

/// One page's link family: what [`PageLinks`] builds the parts and reads the presses of a page with.
struct LinksOnPage<'a> {
    mode: LinkMode,
    session: &'a PageSession,
    info: &'a DocInfo,
    engine: &'a [LinkItem],
    text_links: bool,
    unobstructed: bool,
    /// The page's place on screen.
    xf: PageXform,
    page: usize,
}

impl<'a> PageLinks<'a> {
    /// The links of `pages`, placed as `rotation` turns the view and `sizes` gives each page's size in view space.
    /// `Follow` is the Select tool's mode, `Edit` the Link tool's; addresses written as page text are off and tips
    /// are off until [`text_links`](Self::text_links) and [`tips`](Self::tips) say otherwise.
    pub(crate) fn new(
        mode: LinkMode,
        session: &'a PageSession,
        info: &'a DocInfo,
        engine: &'a [LinkItem],
        pages: &'a [PlacedPage],
        rotation: u16,
        sizes: &'a [(f32, f32)],
    ) -> Self {
        Self { mode, session, info, engine, pages, rotation, sizes, text_links: false, unobstructed: false }
    }

    /// Whether addresses written as page text open too (Preferences).
    pub(crate) fn text_links(mut self, on: bool) -> Self {
        self.text_links = on;
        self
    }

    /// Nothing is drawn over the pages (no palette, no menu), so tips show and a click counts.
    pub(crate) fn tips(mut self, on: bool) -> Self {
        self.unobstructed = on;
        self
    }

    fn on_page(&self, placed: &PlacedPage) -> LinksOnPage<'a> {
        let size = self.sizes.get(placed.page).copied().unwrap_or_default();
        LinksOnPage {
            mode: self.mode,
            session: self.session,
            info: self.info,
            engine: self.engine,
            text_links: self.text_links,
            unobstructed: self.unobstructed,
            xf: PageXform::of(placed, self.rotation, size),
            page: placed.page,
        }
    }

    /// Appends the pages' link parts, later links over earlier ones as the renderer draws them. Links are
    /// click-only parts: a drag over one reaches what lies under it (a text selection, a new link).
    pub(crate) fn parts(&self, out: &mut Vec<PagePart>) {
        for placed in self.pages {
            self.on_page(placed).parts(out);
        }
    }

    /// What `press` did to a link of its page, or `None` when it is no click of the pages' links.
    pub(crate) fn read(&self, press: &PagePress) -> Option<LinkEvent> {
        let placed = self.pages.iter().find(|placed| placed.page == press.page)?;
        self.on_page(placed).read(press)
    }
}

impl LinksOnPage<'_> {
    fn parts(&self, out: &mut Vec<PagePart>) {
        match self.mode {
            LinkMode::Follow => self.follow_parts(out),
            LinkMode::Edit => self.edit_parts(out),
        }
    }

    fn follow_parts(&self, out: &mut Vec<PagePart>) {
        for link in self.info.links.iter().filter(|l| l.page == self.page) {
            let tip = self.unobstructed.then(|| target_label(self.info, &link.target));
            for shape in self.link_shapes(link) {
                out.push(link_part(Owner::Link { index: link.index }, self.page, shape, tip.clone()));
            }
        }
        if !self.text_links {
            return;
        }
        let (Some((_, found)), Some(page_info)) = (self.session.text_links.get(&self.page), self.info.pages.get(self.page)) else { return };
        for (index, link) in found.iter().enumerate() {
            // The annotation over an address is what the document asks for there.
            if self.info.links.iter().any(|l| l.page == self.page && link.covered_by(page_info, l.rect)) {
                continue;
            }
            let tip = self.unobstructed.then(|| link.uri.clone());
            for r in &link.rects {
                let rect = Rect::from_two_pos(
                    self.xf.norm_to_screen(r[0] / self.xf.pw, r[1] / self.xf.ph),
                    self.xf.norm_to_screen(r[2] / self.xf.pw, r[3] / self.xf.ph),
                );
                if rect.is_finite() {
                    out.push(link_part(Owner::TextLink { index }, self.page, HitShape::Rect(rect), tip.clone()));
                }
            }
        }
    }

    fn edit_parts(&self, out: &mut Vec<PagePart>) {
        for item in self.engine.iter().filter(|l| l.page == self.page) {
            // A margin round each, so a thin link can be picked; a press off the page is no page's.
            let rect = self.xf.user_rect_f64(self.info, self.page, item.rect).expand(LINK_TOOL_MARGIN).intersect(self.xf.rect);
            if !rect.is_finite() || !rect.is_positive() {
                continue;
            }
            let tip = self
                .unobstructed
                .then(|| self.info.links.iter().find(|l| l.page == self.page && l.index == item.index))
                .flatten()
                .map(|l| target_label(self.info, &l.target));
            out.push(link_part(Owner::Link { index: item.index }, self.page, HitShape::Rect(rect), tip));
        }
    }

    /// Where `link` is active on screen: each of its quadrilaterals, or its rectangle when it has none
    /// (ISO 32000-2 Table 176).
    fn link_shapes(&self, link: &Link) -> Vec<HitShape> {
        if link.quads.is_empty() {
            let rect = self.xf.user_rect(self.info, self.page, link.rect);
            return if rect.is_finite() { vec![HitShape::Rect(rect)] } else { Vec::new() };
        }
        link.quads
            .iter()
            .filter_map(|q| {
                let mut corners: Vec<egui::Pos2> =
                    q.chunks_exact(2).map(|c| self.xf.user_rect(self.info, self.page, [c[0], c[1], c[0], c[1]]).min).collect();
                if !corners.iter().all(|p| p.is_finite()) {
                    return None;
                }
                // The four points come in any order (writers differ): taken round their centre they
                // outline the quadrilateral the renderer's own test (`Link::contains`) covers.
                let centre = corners.iter().fold(egui::Vec2::ZERO, |sum, p| sum + p.to_vec2()) / corners.len().max(1) as f32;
                corners.sort_by(|a, b| {
                    let angle = |p: &egui::Pos2| (p.y - centre.y).atan2(p.x - centre.x);
                    angle(a).total_cmp(&angle(b))
                });
                Some(HitShape::Polygon(corners))
            })
            .collect()
    }

    /// What `press` did to a link of this family, or `None` when it is no click of this page's links.
    /// A press that was not released over the part it began on is no click (`PressPhase::Release`). It
    /// reads only the press it is given, so a pass that reports none follows nothing.
    fn read(&self, press: &PagePress) -> Option<LinkEvent> {
        let PressPhase::Release { start, click: true, .. } = &press.phase else { return None };
        if press.page != self.page {
            return None;
        }
        let primary = start.button == PointerButton::Primary;
        match (self.mode, press.owner) {
            (LinkMode::Follow, Owner::Link { index }) if primary && self.follows() => {
                let link = self.info.links.iter().find(|l| l.page == self.page && l.index == index)?;
                let javascript = matches!(link.target, LinkTarget::JavaScript(_)).then_some((link.page, link.index));
                Some(LinkEvent::Follow { target: link.target.clone(), javascript })
            }
            // A web or email address written as page text (#4207): the same click, the same check and
            // confirmation on opening.
            (LinkMode::Follow, Owner::TextLink { index }) if primary && self.follows() => {
                let (_, found) = self.session.text_links.get(&self.page)?;
                let uri = found.get(index)?.uri.clone();
                Some(LinkEvent::Follow { target: LinkTarget::Uri(uri), javascript: None })
            }
            (LinkMode::Edit, Owner::Link { index }) if primary => Some(LinkEvent::Select { page: self.page, index, clicks: start.clicks }),
            (LinkMode::Follow | LinkMode::Edit, Owner::Link { index }) if start.button == PointerButton::Secondary => {
                Some(LinkEvent::Context { page: self.page, index })
            }
            _ => None,
        }
    }

    /// Whether a click follows a link: `Follow` is the Select tool's mode, and nothing is drawn over the pages.
    fn follows(&self) -> bool {
        self.unobstructed && self.mode == LinkMode::Follow
    }
}

/// The link or address `press` followed: where it goes and, for a script link, which one runs. `None` for
/// a press that follows nothing (another owner's, not released over its part, or a click in `Edit`).
pub(crate) fn follow_link(links: &PageLinks<'_>, press: &PagePress) -> Option<(LinkTarget, Option<(usize, usize)>)> {
    match links.read(press)? {
        LinkEvent::Follow { target, javascript } => Some((target, javascript)),
        LinkEvent::Select { .. } | LinkEvent::Context { .. } => None,
    }
}

/// A link part: click-only, with the pointing hand and the link's target as its tip, shown at once as a comment's is.
fn link_part(owner: Owner, page: usize, shape: HitShape, tip: Option<String>) -> PagePart {
    let part = PagePart::new(owner, page, shape, PageLayer::Link).cursor(CursorIcon::PointingHand).passes_drag();
    match tip {
        Some(tip) => part.tip_now(tip),
        None => part,
    }
}

/// Full screen is turning to the current page, or waiting for it to render: the page under the pointer isn't
/// the one on screen, so its links, fields, text and comments take no input. Turning the page (a click, a
/// key) is handled apart and still works.
pub(super) fn veiled(view: &DocView) -> bool {
    view.presentation.is_some() && !view.session.presenter.settled_on(view.view.current)
}

/// The sign tabs of one page: their parts, and the layouts those parts and the paint share.
pub(crate) struct OverlayParts {
    /// One part per tab, over the field parts: the tab wins where a field's widget lies under it (the stacking
    /// order is the order of the slice the page body registers, so the body appends these after the fields').
    pub parts: Vec<PagePart>,
    /// The tab layouts, in the order of `parts`, for [`PageOverlays`].
    pub tabs: Vec<SignTab>,
}

/// The sign tabs on `page`'s empty signature fields that a click signs, laid out once: the label's width is
/// measured in this pass by [`SignTabMetrics`], so the part a press hits and the chip that is painted are
/// one rectangle. Nothing unless `shown` (the Select tool, no Prepare Form, no full-screen veil).
pub(crate) fn overlay_parts(
    ui: &mut egui::Ui,
    xf: &PageXform,
    doc: &Document,
    page: usize,
    can_fill: bool,
    shown: bool,
    badge_tips: bool,
) -> OverlayParts {
    let mut out = OverlayParts { parts: Vec::new(), tabs: Vec::new() };
    if badge_tips {
        for (index, badge) in signature_badges(&doc.signatures, page).into_iter().enumerate() {
            let [x0, y0, x1, y1] = badge.rect;
            let rect = xf.user_rect(&doc.info, page, [x0 as f32, y0 as f32, x1 as f32, y1 as f32]).expand(2.0).intersect(xf.rect);
            if rect.is_finite() && rect.is_positive() {
                out.parts.push(
                    PagePart::new(Owner::Badge { index }, page, HitShape::Rect(rect), PageLayer::Text)
                        .hover_only()
                        .tip_now(crate::i18n::t(badge.summary).to_string()),
                );
            }
        }
    }
    if !shown {
        return out;
    }
    let (info, form) = (&doc.info, &doc.form);
    let empty = sign_tabs(&doc.signatures, form, page, can_fill);
    if empty.is_empty() {
        return out;
    }
    let width = SignTabMetrics::new(tl!("Sign")).show_at(ui, Rect::from_min_size(xf.rect.min, Vec2::ZERO)).width;
    for (name, [x0, y0, x1, y1]) in empty {
        let Some(field) = form.iter().position(|f| f.name == name) else { continue };
        let on_screen = xf.user_rect(info, page, [x0 as f32, y0 as f32, x1 as f32, y1 as f32]);
        let Some(tab) = sign_tab_on_page(on_screen, xf.rect, width) else { continue };
        out.parts.push(
            PagePart::new(Owner::SignTab { field }, page, HitShape::Rect(tab.rect), PageLayer::Field).cursor(CursorIcon::PointingHand).passes_drag(),
        );
        out.tabs.push(tab);
    }
    out
}

/// The style of the sign tab's measuring leaf: the width of its label.
#[derive(Clone, Debug, PartialEq, ComponentStyle)]
#[non_exhaustive]
pub struct SignTabMetricsStyle {
    #[style(metrics)]
    metrics: Metrics,
    /// The label's width in the theme's small text role.
    label_width: f32,
}

/// What the measuring leaf reports.
#[derive(Debug)]
pub(crate) struct SignTabMetricsOutput {
    response: Response,
    /// The width of the label.
    pub width: f32,
}

impl Outcome for SignTabMetricsOutput {
    type Value = NoValue;

    fn response(&self) -> &Response {
        &self.response
    }
}

/// A leaf of no size that gives the sign tab's layout the width of its label from the resolved style, in the
/// pass that registers the tab's part (`StyleCx::text_width`), so no layout reads the previous pass.
#[derive(craft::ui::contract::Component)]
#[component(sample = SignTabMetrics::sample)]
pub(crate) struct SignTabMetrics {
    #[common]
    common: Common,
    /// The label, whose language the style follows.
    #[style_key]
    label: &'static str,
}

impl SignTabMetrics {
    /// A leaf measuring `label`.
    pub(crate) fn new(label: &'static str) -> Self {
        SignTabMetrics { common: Common::new(), label }
    }

    /// A leaf for the suite.
    fn sample() -> Self {
        SignTabMetrics::new("Sign")
    }
}

impl Component for SignTabMetrics {
    const ROLE: Role = Role::None;
    const PARTS: bool = false;
    const STOPS: bool = false;
    const INPUT: Input = Input::Display;
    const SHOWS_LABEL: bool = false;
    const OVERLAYS: bool = false;
    type Style = SignTabMetricsStyle;
    type Output = SignTabMetricsOutput;

    fn style(cx: &StyleCx<'_>, key: SignTabMetricsStyleKey) -> SignTabMetricsStyle {
        SignTabMetricsStyle { metrics: Metrics::new(), label_width: cx.text_width(TypeRole::Caption, &key.label) }
    }

    fn measure(&mut self, _: &MeasureCx<'_, Self>) -> Vec2 {
        Vec2::ZERO
    }

    fn sense(&self) -> Sense {
        Sense::empty()
    }

    fn finish(self, cx: Cx<'_, Self>) -> SignTabMetricsOutput {
        SignTabMetricsOutput { response: cx.response.clone(), width: cx.style.label_width }
    }
}

/// The style of the page overlays: every colour and size is a theme role or token.
#[derive(Clone, Debug, PartialEq, ComponentStyle)]
#[non_exhaustive]
pub struct PageOverlaysStyle {
    #[style(metrics)]
    metrics: Metrics,
    /// The tint of a fillable field.
    field_fill: StyleColor,
    /// The outline of a fillable field.
    field_edge: StyleColor,
    /// The outline of a required field.
    required_edge: StyleColor,
    /// The flash round a jumped-to rectangle.
    flash: StyleColor,
    /// The sign tab's chip.
    tab_fill: StyleColor,
    /// The sign tab's icon and label.
    tab_ink: StyleColor,
    /// The colour of every verdict tone.
    tones: Vec<(Tone, StyleColor)>,
    /// The badge outline's width, and how far it stands off the widget; also a required field's outline width.
    outline: f32,
    /// The outline width of a field that isn't required.
    field_line: f32,
    /// The corners of the badge outline and the flash.
    corners: CornerRadius,
    /// How far the flash and the compare marks stand off what they mark.
    grow: f32,
}

impl PageOverlaysStyle {
    /// The colour of `tone`.
    fn tone(&self, tone: Tone) -> Option<StyleColor> {
        self.tones.iter().find(|(t, _)| *t == tone).map(|(_, color)| *color)
    }
}

/// What a page shows over its content, painted in this order: the form-field highlights, the compare marks,
/// the flash, the sign tabs and the validation badges. Paint only: the parts that take presses are
/// registered by `PageParts` (a signed field's own part carries its verdict as its tip).
// `unregistered`: it needs a live document, so the suite has no sample of it.
#[derive(craft::ui::contract::Component, Props)]
#[component(canvas_control, unregistered)]
pub(crate) struct PageOverlays<'a> {
    #[common]
    common: Common,
    page: usize,
    xf: PageXform,
    doc: &'a Document,
    tabs: &'a [SignTab],
    compare: &'a [(usize, [f32; 4], Color32)],
    flash: &'a mut Option<(usize, [f32; 4], f64)>,
    /// Acrobat's "Highlight existing fields".
    #[prop]
    highlight_fields: bool,
    /// Snapshot's kept area on this page (view space): outlined until another tool or drag replaces it.
    #[prop]
    kept_area: Option<[f32; 4]>,
    /// The selected picture on this page (user space): tinted like production's selection.
    #[prop]
    picked: Option<[f64; 4]>,
}

impl<'a> PageOverlays<'a> {
    /// The overlays of `page` of `doc`, placed by `xf`: `tabs` are the layouts [`overlay_parts`] returned,
    /// `compare` the view's compare marks (each with its own colour) and `flash` its flash (its start time is
    /// set on first paint, 0 meaning "not started").
    pub(crate) fn new(
        page: usize,
        xf: PageXform,
        doc: &'a Document,
        tabs: &'a [SignTab],
        compare: &'a [(usize, [f32; 4], Color32)],
        flash: &'a mut Option<(usize, [f32; 4], f64)>,
    ) -> Self {
        PageOverlays { common: Common::new(), page, xf, doc, tabs, compare, flash, highlight_fields: false, kept_area: None, picked: None }
    }
}

impl Component for PageOverlays<'_> {
    const ROLE: Role = Role::GROUP;
    const ALT_ROLES: &'static [Role] = &[Role::None];
    const INPUT: Input = Input::Display;
    const PARTS: bool = false;
    const STOPS: bool = false;
    const OVERLAYS: bool = false;
    const SHOWS_LABEL: bool = false;
    type Style = PageOverlaysStyle;
    type Output = Response;

    fn style(cx: &StyleCx<'_>, _: PageOverlaysStyleKey) -> PageOverlaysStyle {
        let tokens = cx.tokens;
        let colors = &tokens.color;
        let tones =
            [Tone::Neutral, Tone::Accent, Tone::Info, Tone::Good, Tone::Warn, Tone::Bad].into_iter().map(|tone| (tone, cx.tone(tone))).collect();
        PageOverlaysStyle {
            metrics: Metrics::new(),
            field_fill: cx.color(colors.status.info.solid),
            field_edge: cx.color(colors.status.info.solid),
            required_edge: cx.color(colors.status.bad.solid),
            flash: cx.color(colors.accent.solid),
            tab_fill: cx.color(colors.accent.solid),
            tab_ink: cx.color(colors.text.on_accent),
            tones,
            outline: tokens.stroke.thick,
            field_line: tokens.stroke.thin,
            corners: CornerRadius::same(tokens.shape.radius.sm.round().clamp(0.0, 255.0) as u8),
            grow: tokens.space.gap.inline,
        }
    }

    fn measure(&mut self, _: &MeasureCx<'_, Self>) -> Vec2 {
        Vec2::ZERO
    }

    fn sense(&self) -> Sense {
        Sense::empty()
    }

    fn paint(&mut self, ui: &mut PaintUi<'_>, cx: &Cx<'_, Self>) {
        let style = cx.style;
        let draw = ui.draw();
        if self.highlight_fields {
            self.paint_field_highlights(&draw, style);
        }
        self.paint_compare_marks(&draw, style);
        self.paint_flash(&draw, cx);
        if let Some(rect) = self.picked {
            let rect = self.xf.user_rect_f64(&self.doc.info, self.page, rect);
            if rect.is_finite() {
                draw.faded(50.0 / 255.0, |draw| draw.fill(&PaintShape::Rect(rect, CornerRadius::ZERO), style.flash));
            }
        }
        if let Some(area) = self.kept_area {
            let rect = self.xf.view_rect(area);
            if rect.is_finite() {
                draw.stroke(&PaintShape::Rect(rect, CornerRadius::ZERO), StrokeWidth::Thick, style.flash);
            }
        }
        for tab in self.tabs {
            paint_sign_tab(&draw, tab, style);
        }
        self.paint_badges(&draw, style);
    }

    fn access(&self, access: &mut Access, _: &Cx<'_, Self>) {
        access.role(Role::None);
    }

    fn finish(self, cx: Cx<'_, Self>) -> Response {
        cx.response.clone()
    }
}

impl PageOverlays<'_> {
    /// Form-field highlight (Acrobat's "Highlight existing fields"); a required field gets the stronger red
    /// outline. Radio buttons and round XFA check boxes are round, and so is theirs. Only what can be filled
    /// is marked: a push button holds no value (the frames and controls of a LaTeX `animate` animation are
    /// push buttons), and a read-only widget is left as it is drawn.
    fn paint_field_highlights(&self, draw: &Draw<'_>, style: &PageOverlaysStyle) {
        for field in self.doc.form.iter().filter(|f| f.kind != FormFieldKind::PushButton) {
            let required = field.has(pdfcraft_engine::field_flags::REQUIRED);
            let (width, line, edge) = if required {
                (StrokeWidth::Thick, style.outline, style.required_edge)
            } else {
                (StrokeWidth::Thin, style.field_line, style.field_edge)
            };
            for widget in field.widgets.iter().filter(|w| w.page == Some(self.page) && !w.hidden && !w.read_only) {
                let rect = self.xf.user_rect_f64(&self.doc.info, self.page, widget.rect);
                // The page transform can turn a finite widget into one the screen cannot place.
                if !rect.is_finite() {
                    continue;
                }
                // The outline lies inside the widget: a rectangle strokes inside its edge, a circle on its radius.
                let radius = rect.width().min(rect.height()) / 2.0;
                let (shape, outline) = if widget.round {
                    (PaintShape::Circle(rect.center(), radius), PaintShape::Circle(rect.center(), radius - line / 2.0))
                } else {
                    (PaintShape::Rect(rect, CornerRadius::ZERO), PaintShape::Rect(rect, CornerRadius::ZERO))
                };
                draw.faded(48.0 / 255.0, |draw| draw.fill(&shape, style.field_fill));
                draw.stroke(&outline, width, edge);
            }
        }
    }

    /// The compare marks on this page, each in its own colour (data, not a theme role).
    fn paint_compare_marks(&self, draw: &Draw<'_>, style: &PageOverlaysStyle) {
        for (page, rect, color) in self.compare {
            if *page != self.page {
                continue;
            }
            let rect = self.xf.user_rect(&self.doc.info, self.page, *rect).expand(style.grow);
            if rect.is_finite() {
                draw.faded(COMPARE_SHARE, |draw| draw.data_fill(&PaintShape::Rect(rect, CornerRadius::ZERO), DataColor::new(*color)));
            }
        }
    }

    /// The flash round a jumped-to rectangle: it fades out over [`FLASH_SECONDS`], then the view forgets it.
    fn paint_flash(&mut self, draw: &Draw<'_>, cx: &Cx<'_, Self>) {
        let Some((page, rect, started)) = *self.flash else { return };
        if page != self.page {
            return;
        }
        let now = cx.now();
        // 0 is "not started": the first pass that paints it starts the clock. A second pass of the frame
        // finds the start already written.
        let started = if started == 0.0 { now } else { started };
        *self.flash = Some((page, rect, started));
        let age = now - started;
        if age >= FLASH_SECONDS {
            *self.flash = None;
            return;
        }
        let shape = PaintShape::Rect(self.xf.user_rect(&self.doc.info, self.page, rect).expand(cx.style.grow), cx.style.corners);
        let share = ((FLASH_SECONDS - age) / FLASH_SECONDS) as f32;
        draw.faded(share, |draw| draw.stroke(&shape, StrokeWidth::Thick, cx.style.flash));
        cx.wake_at(started + FLASH_SECONDS);
    }

    /// The validation badges: an outline round each signed signature widget and, where there is room, the
    /// verdict's mark in its corner. The verdict's tip is the field part's (`forms_ui::field_parts`).
    fn paint_badges(&self, draw: &Draw<'_>, style: &PageOverlaysStyle) {
        let info = &self.doc.info;
        for badge in signature_badges(&self.doc.signatures, self.page) {
            let [x0, y0, x1, y1] = badge.rect;
            let rect = self.xf.user_rect(info, self.page, [x0 as f32, y0 as f32, x1 as f32, y1 as f32]);
            // Stroking a rectangle the screen cannot place would put NaN into the mesh.
            if !rect.is_finite() {
                continue;
            }
            let (icon, tone) = status_tone(badge.status);
            let Some(color) = style.tone(tone) else { continue };
            // The outline stands outside the widget: the stroke is inside its rectangle, so grow it.
            draw.stroke(&PaintShape::Rect(rect.expand(style.outline), style.corners), StrokeWidth::Thick, color);
            if let Some(mark) = verdict_mark_rect(rect, self.xf.rect, mark_anchor(&self.xf, info, self.page, badge.rect)) {
                let side = mark.width() * MARK_ICON_SHARE;
                draw.icon(Rect::from_center_size(mark.center(), Vec2::splat(side)), &IconSpec::svg(icon).color(color));
            }
        }
    }
}

/// Paints the textless page background (for an open paragraph or added-text draft) and the moving-item
/// background/preview (`MoveBackground`) over the placed pages before the overlays and controls.
#[derive(craft::ui::contract::Component)]
#[component(canvas_control, content_painter, unregistered)]
pub(crate) struct PageBackdrop<'a> {
    #[common]
    common: Common,
    pages: &'a [PlacedPage],
    sizes: &'a [(f32, f32)],
    rotation: u16,
    info: &'a DocInfo,
    session: &'a PageSession,
    /// The document's fields and the part under the pointer: the unbaked widgets, the hover frame and the drop-down arrows.
    form: &'a [pdfcraft_engine::FormField],
    hover: Option<&'a Owner>,
}

impl<'a> PageBackdrop<'a> {
    pub(crate) fn new(
        pages: &'a [PlacedPage],
        sizes: &'a [(f32, f32)],
        rotation: u16,
        info: &'a DocInfo,
        session: &'a PageSession,
        form: &'a [pdfcraft_engine::FormField],
        hover: Option<&'a Owner>,
    ) -> Self {
        Self { common: Common::new(), pages, sizes, rotation, info, session, form, hover }
    }
}

#[derive(Clone, Debug, PartialEq, ComponentStyle)]
#[non_exhaustive]
pub struct PageBackdropStyle {
    #[style(metrics)]
    metrics: Metrics,
}

impl Component for PageBackdrop<'_> {
    const ROLE: Role = Role::GROUP;
    const ALT_ROLES: &'static [Role] = &[Role::None];
    const INPUT: Input = Input::Display;
    const PARTS: bool = false;
    const STOPS: bool = false;
    const OVERLAYS: bool = false;
    const SHOWS_LABEL: bool = false;
    type Style = PageBackdropStyle;
    type Output = Response;

    fn style(_: &StyleCx<'_>, _: PageBackdropStyleKey) -> PageBackdropStyle {
        PageBackdropStyle { metrics: Metrics::new() }
    }

    fn measure(&mut self, _: &MeasureCx<'_, Self>) -> Vec2 {
        Vec2::ZERO
    }

    fn sense(&self) -> Sense {
        Sense::empty()
    }

    fn paint(&mut self, ui: &mut PaintUi<'_>, cx: &Cx<'_, Self>) {
        let draw = ui.draw();
        let content = ui.content_painter(cx);
        for placed in self.pages {
            let size = self.sizes.get(placed.page).copied().unwrap_or_default();
            let xf = PageXform::of(placed, self.rotation, size);
            self.session.text_background.paint(
                &draw,
                &content,
                &xf,
                placed.page,
                self.session.line_editor.as_ref(),
                self.session.content.draft.as_ref(),
            );
            self.session.move_background.paint(&draw, &content, placed, self.info, &self.session.content);
            crate::forms_ui::paint_page(&draw, &content.painter, (&xf, placed.page), self.info, self.form, self.session, self.hover);
        }
    }

    fn access(&self, access: &mut Access, _: &Cx<'_, Self>) {
        access.role(Role::None);
    }

    fn finish(self, cx: Cx<'_, Self>) -> Response {
        cx.response.clone()
    }
}

/// Paints the open added-text draft's paper and text under the editor's caret.
#[derive(craft::ui::contract::Component)]
#[component(canvas_control, content_painter, unregistered)]
pub(crate) struct PageDraft<'a> {
    #[common]
    common: Common,
    content: &'a crate::content_ui::ContentView,
}

impl<'a> PageDraft<'a> {
    pub(crate) fn new(content: &'a crate::content_ui::ContentView) -> Self {
        Self { common: Common::new(), content }
    }
}

impl Component for PageDraft<'_> {
    const ROLE: Role = Role::GROUP;
    const ALT_ROLES: &'static [Role] = &[Role::None];
    const INPUT: Input = Input::Display;
    const PARTS: bool = false;
    const STOPS: bool = false;
    const OVERLAYS: bool = false;
    const SHOWS_LABEL: bool = false;
    type Style = PageBackdropStyle;
    type Output = Response;

    fn style(_: &StyleCx<'_>, _: PageDraftStyleKey) -> PageBackdropStyle {
        PageBackdropStyle { metrics: Metrics::new() }
    }

    fn measure(&mut self, _: &MeasureCx<'_, Self>) -> Vec2 {
        Vec2::ZERO
    }

    fn sense(&self) -> Sense {
        Sense::empty()
    }

    fn paint(&mut self, ui: &mut PaintUi<'_>, cx: &Cx<'_, Self>) {
        let content = ui.content_painter(cx);
        self.content.paint_draft(&content);
    }

    fn access(&self, access: &mut Access, _: &Cx<'_, Self>) {
        access.role(Role::None);
    }

    fn finish(self, cx: Cx<'_, Self>) -> Response {
        cx.response.clone()
    }
}

/// Paints a sign tab laid out by [`overlay_parts`]: an accent chip with the signature icon and "Sign" on it,
/// as the app's primary buttons are. Screen-only chrome, like the validation badges.
fn paint_sign_tab(draw: &Draw<'_>, tab: &SignTab, style: &PageOverlaysStyle) {
    let radius = CornerRadius::same(tab.radius.round().clamp(0.0, 255.0) as u8);
    draw.fill(&PaintShape::Rect(tab.rect, radius), style.tab_fill);
    draw.icon(tab.icon, &IconSpec::svg(IconId::new("signature")).color(style.tab_ink));
    if let Some(at) = tab.label {
        let run = TextRun::new(tl!("Sign").to_string()).role(TypeRole::Caption).color(style.tab_ink);
        draw.text(Rect::from_min_max(pos2(at.x, tab.rect.top()), tab.rect.right_bottom()), &run);
    }
}
