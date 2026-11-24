//! The open paragraph's own paint over its page: the cover under the editor, laid out in the paragraph's
//! source faces, and the changed text over it.

use std::collections::BTreeMap;
use std::sync::Arc;

use craft::behavior::{Input, Sense};
use craft::ui::contract::{Access, Common, Component, Cx, DisabledLook, MeasureCx, NoValue, Outcome, PaintUi, Role};
use craft::ui::fonts::FontRef;
use craft::ui::primitives::FacesCx;
use craft::ui::style::{ComponentStyle, Metrics, StyleCx};
use egui::{Color32, CornerRadius, Pos2, Rect, Response, Shape, Vec2, vec2};

use super::LineEditor;
use super::layout::{color32, editor_fill, editor_galley, editor_job, source_faces};

/// The style of the paragraph's paint: it paints the document's own colours and nothing of the theme.
#[derive(Clone, Debug, PartialEq, ComponentStyle)]
#[non_exhaustive]
pub struct ParagraphPaintStyle {
    #[style(metrics)]
    metrics: Metrics,
}

/// The open paragraph laid out for this pass.
struct Laid {
    faces: BTreeMap<String, FontRef>,
    width: f32,
    overflow: bool,
    live: Rect,
}

/// What the cover reports: the paragraph's faces and layout, which the editor lays its text out with.
pub(super) struct ParagraphCoverOutput {
    response: Response,
    /// The faces of the page's embedded font programs that are ready, by the page's font resource.
    pub(super) faces: BTreeMap<String, FontRef>,
    /// The layout width, in screen pixels.
    pub(super) width: f32,
    /// The changed text no longer fits the source box.
    pub(super) overflow: bool,
    /// Where the editor's text shows: the draft's extent, with the measured extent of the paragraph's draft.
    pub(super) live: Rect,
}

impl Outcome for ParagraphCoverOutput {
    type Value = NoValue;

    fn response(&self) -> &Response {
        &self.response
    }
}

/// The open paragraph's layout in its source faces, and, when `cover` is set, the box that hides the page's own text
/// under the editor while the textless page is not ready: shown over the page, clipped to it, before the editor.
// `unregistered`: it needs a live document and an open paragraph, so the suite has no sample of it.
#[derive(craft::ui::contract::Component)]
#[component(canvas_control, content_painter, unregistered)]
pub(super) struct ParagraphCover<'a> {
    #[common]
    common: Common,
    doc: &'a pdfcraft_engine::Document,
    ed: &'a LineEditor,
    /// Screen pixels per page point.
    scale: f32,
    /// The widest a one-row draft grows, in screen pixels.
    room: f32,
    /// The measured extent of this paragraph's draft on screen, once the textless page has measured it.
    draft_rect: Option<Rect>,
    cover: bool,
    laid: Option<Laid>,
}

impl<'a> ParagraphCover<'a> {
    pub(super) fn new(doc: &'a pdfcraft_engine::Document, ed: &'a LineEditor, scale: f32, room: f32, draft_rect: Option<Rect>, cover: bool) -> Self {
        ParagraphCover { common: Common::new(), doc, ed, scale, room, draft_rect, cover, laid: None }
    }

    /// The paragraph laid out for the pass: by paint, which keeps it for finish, or by finish when no paint ran.
    fn lay_out(&self, cx: &impl FacesCx, ctx: &egui::Context) -> Laid {
        let (ed, scale) = (self.ed, self.scale);
        let faces = source_faces(cx, self.doc, ed.page);
        // The editor opens at the source size. Once text changes, show the real draft extent: single rows grow toward
        // the page edge, paragraphs rewrap at their source width, and the visible frame grows/shrinks vertically with
        // their rows. An OCR phrase (several stretched words on one scanned line) also grows rightwards.
        let single_run = ed.source.as_ref().is_some_and(|(block, _)| block.rows.len() == 1 || block.invisible);
        let width = if !ed.multiline && single_run && !ed.text.contains('\n') && ed.has_changes() {
            let job = editor_job(ctx, ed, &faces, &ed.text, scale, f32::INFINITY);
            let natural = ctx.fonts_mut(|f| f.layout_job(job)).size().x;
            natural.max(ed.size * 0.4).min(self.room)
        } else {
            ed.rect.width().max(1.0)
        };
        let measured = editor_galley(ctx, ed, &faces, &ed.text, scale, width);
        let overflow = ed.has_changes() && (measured.rect.width() > ed.rect.width() + 1.5 || measured.rect.height() > ed.rect.height() + 1.5);
        let visible = Rect::from_min_size(ed.rect.min, measured.rect.size().max(vec2(1.0, 1.0)));
        // Unsupported source programs use an interface fallback whose glyph advances can differ from the PDF's. Keep
        // every visible row inside the growing frame.
        let live = self.draft_rect.map_or(visible, |bounds| bounds.union(visible));
        Laid { faces, width, overflow, live }
    }
}

impl Component for ParagraphCover<'_> {
    const ROLE: Role = Role::GROUP;
    const DISABLED: DisabledLook = DisabledLook::Faded;
    const ALT_ROLES: &'static [Role] = &[Role::None];
    const INPUT: Input = Input::Display;
    const PARTS: bool = false;
    const STOPS: bool = false;
    const OVERLAYS: bool = false;
    const SHOWS_LABEL: bool = false;
    type Style = ParagraphPaintStyle;
    type Output = ParagraphCoverOutput;

    fn style(_: &StyleCx<'_>, _: ParagraphCoverStyleKey) -> ParagraphPaintStyle {
        ParagraphPaintStyle { metrics: Metrics::new() }
    }

    fn measure(&mut self, _: &MeasureCx<'_, Self>) -> Vec2 {
        Vec2::ZERO
    }

    fn sense(&self) -> Sense {
        Sense::empty()
    }

    fn paint(&mut self, ui: &mut PaintUi<'_>, cx: &Cx<'_, Self>) {
        let laid = self.lay_out(cx, &cx.response.ctx);
        let live = laid.live;
        self.laid = Some(laid);
        if !self.cover {
            return;
        }
        // White, unless the text is too pale to read on white (white text from a coloured banner, #913): then a dark
        // box. An unchanged paragraph is the page's own text, and a changed one has the textless page under it; until
        // that is ready, a cover over the entire source box, including lines the draft no longer occupies, so
        // shrinking the text never exposes a copy.
        let fill = editor_fill(color32(self.ed.look.color));
        ui.content_painter(cx).data_shapes([Shape::rect_filled(self.ed.rect.expand(1.0).union(live), CornerRadius::ZERO, fill)]);
    }

    fn access(&self, access: &mut Access, _: &Cx<'_, Self>) {
        access.role(Role::None);
    }

    fn finish(mut self, cx: Cx<'_, Self>) -> ParagraphCoverOutput {
        let Laid { faces, width, overflow, live } = self.laid.take().unwrap_or_else(|| self.lay_out(&cx, &cx.response.ctx));
        ParagraphCoverOutput { response: cx.response.clone(), faces, width, overflow, live }
    }
}

/// The open paragraph's changed text, in its own colour, shown over the page after the editor so it paints over the
/// selection, clipped to the page.
// `unregistered`: it paints a galley the open paragraph lays out, so the suite has no sample of it.
#[derive(craft::ui::contract::Component)]
#[component(canvas_control, content_painter, unregistered)]
pub(super) struct ParagraphText {
    #[common]
    common: Common,
    origin: Pos2,
    galley: Arc<egui::Galley>,
    color: Color32,
}

impl ParagraphText {
    pub(super) fn new(origin: Pos2, galley: Arc<egui::Galley>, color: Color32) -> Self {
        ParagraphText { common: Common::new(), origin, galley, color }
    }
}

impl Component for ParagraphText {
    const ROLE: Role = Role::GROUP;
    const DISABLED: DisabledLook = DisabledLook::Faded;
    const ALT_ROLES: &'static [Role] = &[Role::None];
    const INPUT: Input = Input::Display;
    const PARTS: bool = false;
    const STOPS: bool = false;
    const OVERLAYS: bool = false;
    const SHOWS_LABEL: bool = false;
    type Style = ParagraphPaintStyle;
    type Output = Response;

    fn style(_: &StyleCx<'_>, _: ParagraphTextStyleKey) -> ParagraphPaintStyle {
        ParagraphPaintStyle { metrics: Metrics::new() }
    }

    fn measure(&mut self, _: &MeasureCx<'_, Self>) -> Vec2 {
        Vec2::ZERO
    }

    fn sense(&self) -> Sense {
        Sense::empty()
    }

    fn paint(&mut self, ui: &mut PaintUi<'_>, cx: &Cx<'_, Self>) {
        ui.content_painter(cx).data_shapes([Shape::galley(self.origin, Arc::clone(&self.galley), self.color)]);
    }

    fn access(&self, access: &mut Access, _: &Cx<'_, Self>) {
        access.role(Role::None);
    }

    fn finish(self, cx: Cx<'_, Self>) -> Response {
        cx.response.clone()
    }
}
