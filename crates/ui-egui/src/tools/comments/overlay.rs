use craft::behavior::kurbo::{BezPath, Point, Rect as DocRect, Shape as _};
use craft::behavior::{Input, Sense};
use craft::pages::ui::PageBody;
use craft::review::annotate::pick_at;
use craft::review::ui::annotate::{AnnotationOverlay, BodyPaint};
use craft::review::{Annotation as Marked, AnnotationEdit, AnnotationId, AnnotationKind, HandleRule};
use craft::ui::canvas::ToolContext;
use craft::ui::canvas_controls::{HoverOutline, HoverShape};
use craft::ui::contract::{Access, Common, Component, Cx, MeasureCx, PaintUi, Role, Show as _};
use craft::ui::primitives::{Draw, PaintShape, StrokeWidth, TextRun};
use craft::ui::style::{ComponentStyle, DataColor, Metrics, StyleColor, StyleCx};
use egui::{Color32, Pos2, Rect, Response, Vec2, vec2};
use pdfcraft_engine::CommentOutline as O;

use super::bridge::{self, PageMap};
use super::state::Settling;
use super::tool::PastePlacement;
use super::{CommentPrefs, DocInfo, QuickTool, color32};
use crate::canvas::PageSession;

/// What the overlay needs besides the document: what the pointer is over.
pub(crate) struct Hover {
    /// The pointer on screen, when it is over the pages with no button down.
    pub pointer: Option<Pos2>,
    /// The user's Redaction Tool Properties fill and opacity.
    pub redact: ([f64; 3], f64),
    /// The document-unit `(tolerance, icon side)` of the hit test.
    pub pick: (f64, f64),
    /// The document allows commenting.
    pub allowed: bool,
}

/// Paints the page's comments: the annotation overlay over the pages `body` places, the outline of the comment a
/// press under the pointer would take, and a redaction mark's fill while the pointer is over it, as Acrobat's does,
/// so the marked area reads before anything is applied.
#[allow(clippy::too_many_arguments)]
pub(crate) fn show(
    ui: &mut egui::Ui,
    body: &PageBody<'_>,
    session: &PageSession,
    rotation: u16,
    info: &DocInfo,
    prefs: &CommentPrefs,
    tool: QuickTool,
    hover: Hover,
) {
    let map = PageMap::new(info, body.pages, rotation, body.view);
    let all = bridge::annotations(info, &map, QuickTool::Select, hover.allowed);
    let reached = bridge::annotations(info, &map, tool, hover.allowed);
    let markers = bridge::markers(info, &map, hover.pick.1);
    let in_flight = session.comments.tool.in_flight();
    let selected: Vec<AnnotationId> = session
        .comments
        .tool
        .selected
        .iter()
        .copied()
        .filter(|id| {
            let (page, index) = bridge::key_of(*id);
            !(session.signature_drag.contains(page, index) && matches!(&in_flight, Some(AnnotationEdit::Move { ids, .. }) if ids.contains(id)))
        })
        .collect();
    let idle = in_flight.is_none() && session.comments.tool.drawing().is_empty() && !body.pass.is_some_and(|p| p.press.phase.is_press());
    for page in body.pages {
        let mut on_page: Vec<Marked> = all.iter().filter(|a| a.page == page.page).cloned().collect();
        // The settling strokes are drawn by the overlay until their page is drawn again; their ids sit at the top of
        // the id space, which `id_of(page, index)` never reaches.
        let mut settling_ids = Vec::new();
        for (n, s) in session.comments.settling.iter().filter(|s| s.page == page.page).enumerate() {
            let id = bridge::id_of(page.page, u32::MAX as usize - n);
            if let Some(a) = settling_annotation(s, prefs, id, &map) {
                settling_ids.push(id);
                on_page.push(a);
            }
        }
        let mut overlay = AnnotationOverlay::new(tl!("Comments"), &on_page, &selected).bodies(BodyPaint::Only(&settling_ids));
        if let Some(edit) = &in_flight {
            overlay = overlay.live(edit);
        }
        overlay.show_at(ui, body.rect);
        let Some(at) = hover.pointer.filter(|p| idle && page.rect.contains(*p)) else { continue };
        let at = body.view.to_doc(at);
        let (tolerance, icon_side) = hover.pick;
        let under = pick_at(&on_page, at, tolerance, icon_side, &markers).and_then(|id| on_page.iter().find(|a| a.id == id));
        if let Some(a) = under
            && let (page_of, index) = bridge::key_of(a.id)
            && info.annotations.iter().any(|e| e.page == page_of && e.index == index && e.subtype == "Redact")
        {
            RedactionHover::new(redact_areas(&map, info, page_of, index), redaction_fill(hover.redact)).show_at(ui, body.rect);
        }
        let shape = under
            .filter(|a| !selected.contains(&a.id) && matches!(tool, QuickTool::Select | QuickTool::Fill(_)) && reached.iter().any(|r| r.id == a.id))
            .map(|a| hover_shape(a, &markers, icon_side));
        HoverOutline::new(tl!("Comment under the pointer"), shape).show(ui);
    }
}

/// The outline of `a`: the lines of a text markup, else its extent, with the marker of a commented markup.
// PLAN: the same shape `bridge::parts` gives its page part, which it does not expose; pdf-comments-w7 to share one.
fn hover_shape(a: &Marked, markers: &[(AnnotationId, DocRect)], icon_side: f64) -> HoverShape {
    if !a.kind.is_markup() {
        return HoverShape::Rect(a.extent(icon_side));
    }
    let mut path = BezPath::new();
    for quad in a.points.chunks(4) {
        let (min, max) = quad.iter().fold((Point::new(f64::MAX, f64::MAX), Point::new(f64::MIN, f64::MIN)), |(lo, hi), p| {
            (Point::new(lo.x.min(p.x), lo.y.min(p.y)), Point::new(hi.x.max(p.x), hi.y.max(p.y)))
        });
        path.extend(DocRect::new(min.x, min.y, max.x, max.y).path_elements(0.1));
    }
    for (_, r) in markers.iter().filter(|(m, _)| *m == a.id) {
        path.extend(r.path_elements(0.1));
    }
    HoverShape::Path(path)
}

/// A released drawing still waiting for its page raster, as an annotation under `id`.
fn settling_annotation(s: &Settling, prefs: &CommentPrefs, id: AnnotationId, map: &PageMap<'_>) -> Option<Marked> {
    let kind = s.tool.kind()?;
    let style = prefs.style(s.tool);
    let points: Vec<Point> = s.points.iter().map(|p| map.to_doc(s.page, *p)).collect();
    let first = points.first()?;
    let rect = points.iter().fold(DocRect::new(first.x, first.y, first.x, first.y), |r, p| r.union_pt(*p));
    let c = color32(style.color);
    let (strokes, points) = match kind {
        AnnotationKind::Ink | AnnotationKind::FreehandHighlight => (vec![points], Vec::new()),
        AnnotationKind::Rectangle | AnnotationKind::Oval => (Vec::new(), Vec::new()),
        _ => (Vec::new(), points),
    };
    Some(Marked {
        id,
        page: s.page,
        kind,
        rect,
        points,
        strokes,
        color: [c.r(), c.g(), c.b()],
        opacity: style.opacity as f32,
        stroke_width: map.frame(s.page).map_or(style.width, |f| f.doc_len(style.width)),
        text: String::new(),
        locked: true,
        aspect: None,
        handle_rule: HandleRule::Free,
    })
}

/// A redaction mark's fill in the user's colour and opacity, a data colour and not a theme value.
fn redaction_fill((rgb, opacity): ([f64; 3], f64)) -> DataColor {
    let [r, g, b] = rgb.map(|c| (c.clamp(0.0, 1.0) * 255.0) as u8);
    DataColor::new(Color32::from_rgba_unmultiplied(r, g, b, (opacity.clamp(0.0, 1.0) * 255.0) as u8))
}

/// The style of the redaction hover: it paints the user's own colour and nothing of the theme.
#[derive(Clone, Debug, PartialEq, ComponentStyle)]
#[non_exhaustive]
pub struct RedactionHoverStyle {
    #[style(metrics)]
    metrics: Metrics,
}

/// A redaction mark's areas filled while the pointer is over the mark, so the marked area reads before anything is
/// applied. Paint only.
// `unregistered`: it needs a live document with a redaction mark, so the suite has no sample of it.
#[derive(craft::ui::contract::Component)]
#[component(canvas_control, unregistered)]
struct RedactionHover {
    #[common]
    common: Common,
    areas: Vec<Rect>,
    fill: DataColor,
}

impl RedactionHover {
    fn new(areas: Vec<Rect>, fill: DataColor) -> Self {
        RedactionHover { common: Common::new(), areas, fill }
    }
}

impl Component for RedactionHover {
    const ROLE: Role = Role::GROUP;
    const ALT_ROLES: &'static [Role] = &[Role::None];
    const INPUT: Input = Input::Display;
    const PARTS: bool = false;
    const STOPS: bool = false;
    const OVERLAYS: bool = false;
    const SHOWS_LABEL: bool = false;
    type Style = RedactionHoverStyle;
    type Output = Response;

    fn style(_: &StyleCx<'_>, _: RedactionHoverStyleKey) -> RedactionHoverStyle {
        RedactionHoverStyle { metrics: Metrics::new() }
    }

    fn measure(&mut self, _: &MeasureCx<'_, Self>) -> Vec2 {
        Vec2::ZERO
    }

    fn sense(&self) -> Sense {
        Sense::empty()
    }

    fn paint(&mut self, ui: &mut PaintUi<'_>, _: &Cx<'_, Self>) {
        let draw = ui.draw();
        for area in &self.areas {
            draw.data_fill(&PaintShape::Polygon(rect_corners(*area)), self.fill);
        }
    }

    fn access(&self, access: &mut Access, _: &Cx<'_, Self>) {
        access.role(Role::None);
    }

    fn finish(self, cx: Cx<'_, Self>) -> Response {
        cx.response.clone()
    }
}

/// A redaction mark's areas on screen: one per marked quad, or its rectangle.
fn redact_areas(map: &PageMap<'_>, info: &DocInfo, page: usize, index: usize) -> Vec<Rect> {
    let Some(a) = info.annotations.iter().find(|a| a.page == page && a.index == index) else { return Vec::new() };
    let Some(p) = info.pages.get(page) else { return Vec::new() };
    let boxes: Vec<[f32; 4]> = if a.quads.is_empty() {
        vec![a.shown_rect(p.rotation)]
    } else {
        a.quads
            .iter()
            .map(|q| {
                let xs = [q[0], q[2], q[4], q[6]];
                let ys = [q[1], q[3], q[5], q[7]];
                let fold = |v: [f32; 4], f: fn(f32, f32) -> f32, seed: f32| v.into_iter().fold(seed, f);
                [fold(xs, f32::min, f32::MAX), fold(ys, f32::min, f32::MAX), fold(xs, f32::max, f32::MIN), fold(ys, f32::max, f32::MIN)]
            })
            .collect()
    };
    boxes
        .into_iter()
        .filter_map(|b| {
            let corners = [[b[0], b[1]], [b[2], b[1]], [b[2], b[3]], [b[0], b[3]]];
            let screen: Option<Vec<Pos2>> =
                corners.iter().map(|c| map.to_screen(page, map.to_doc(page, [f64::from(c[0]), f64::from(c[1])]))).collect();
            screen.map(|points| Rect::from_points(&points))
        })
        .collect()
}

/// The copied bundle's box and each comment's outline at the pointer, centred on it, in `overlay.line`, labelled with
/// how many comments it holds. `page` is the page under `at`.
pub(crate) fn paste_outline(draw: &Draw<'_>, cx: &ToolContext<'_>, paste: &PastePlacement, map: &PageMap<'_>, page: usize, at: Pos2) {
    let Some(style) = cx.style() else { return };
    let line = style.color(style.tokens.color.overlay.line);
    let Some(under) = map.to_page(page, Point::new(f64::from(at.x), f64::from(at.y))) else { return };
    let [x, y] = map.to_user(page, under);
    let [bx0, by0, bx1, by1] = paste.bounds;
    let (dx, dy) = (x - (bx0 + bx1) / 2.0, y - (by0 + by1) / 2.0);
    let on_screen = |q: [f64; 2]| map.to_screen(page, map.to_doc(page, [q[0] + dx, q[1] + dy])).unwrap_or(at);
    let gap = style.tokens.space.gap.tight;
    let frame = Rect::from_two_pos(on_screen([bx0, by0]), on_screen([bx1, by1])).expand(gap);
    draw.stroke(&PaintShape::Polygon(rect_corners(frame)), StrokeWidth::Thin, line);
    for outline in &paste.outlines {
        paint_outline(draw, outline, &on_screen, line);
    }
    let label = crate::i18n::fmt(tl!("Comments: {n}"), &[("n", &paste.count.to_string())]);
    let mut run = TextRun::new(label);
    run.color = Some(line);
    let height = style.tokens.size.icon.md;
    draw.text(Rect::from_min_size(frame.left_top() - vec2(0.0, height + gap), vec2(cx.rect.width(), height)), &run);
}

fn rect_corners(r: Rect) -> Vec<Pos2> {
    vec![r.left_top(), r.right_top(), r.right_bottom(), r.left_bottom()]
}

/// One copied comment's outline on screen (`at` maps a user-space point, moved to where the bundle hangs), in the
/// comment's colour, or `fallback` when it has none.
fn paint_outline(draw: &Draw<'_>, outline: &O, at: &dyn Fn([f64; 2]) -> Pos2, fallback: StyleColor) {
    let width = StrokeWidth::Medium;
    let own = |c: &Option<[f64; 3]>| {
        c.map(|[r, g, b]| {
            DataColor::new(Color32::from_rgb((r.clamp(0.0, 1.0) * 255.0) as u8, (g.clamp(0.0, 1.0) * 255.0) as u8, (b.clamp(0.0, 1.0) * 255.0) as u8))
        })
    };
    let stroke = |shape: PaintShape, c: &Option<[f64; 3]>| match own(c) {
        Some(color) => draw.stroke(&shape, width, color),
        None => draw.stroke(&shape, width, fallback),
    };
    match outline {
        O::Rect { rect, round, color } => {
            let r = Rect::from_two_pos(at([rect[0], rect[1]]), at([rect[2], rect[3]]));
            if *round {
                let (c, rx, ry) = (r.center(), r.width() / 2.0, r.height() / 2.0);
                let points =
                    (0..48).map(|i| i as f32 / 48.0 * std::f32::consts::TAU).map(|t| Pos2::new(c.x + rx * t.cos(), c.y + ry * t.sin())).collect();
                stroke(PaintShape::Polygon(points), color);
            } else {
                stroke(PaintShape::Polygon(rect_corners(r)), color);
            }
        }
        O::Line { points, color } => stroke(PaintShape::Path(points.iter().map(|p| at(*p)).collect()), color),
        O::Poly { points, closed, color } => {
            let points = points.iter().map(|p| at(*p)).collect();
            stroke(if *closed { PaintShape::Polygon(points) } else { PaintShape::Path(points) }, color);
        }
        O::Ink { strokes, color } => {
            for s in strokes {
                stroke(PaintShape::Path(s.iter().map(|p| at(*p)).collect()), color);
            }
        }
        O::Quads { quads, color } => {
            for q in quads {
                // Upper left, upper right, lower right, lower left: a closed shape in that order.
                stroke(PaintShape::Polygon(vec![at([q[0], q[1]]), at([q[2], q[3]]), at([q[6], q[7]]), at([q[4], q[5]])]), color);
            }
        }
        O::Text { rect, color, .. } => {
            // The text itself is not drawn on the pointer's outline; its box says where it lands.
            let r = Rect::from_two_pos(at([rect[0], rect[1]]), at([rect[2], rect[3]]));
            stroke(PaintShape::Polygon(rect_corners(r)), color);
        }
    }
}
