//! The SOURCE PAGE pane: the rendered page with one box per recognized word. A click selects the
//! word under the pointer; a drag draws the re-OCR region (in view-space points, clamped to the
//! page). Words are coloured by their confidence band; removed words are struck out and dimmed,
//! corrected words outlined. The context menu re-reads a region or the page, and accepts or
//! rejects the page.

use craft::behavior::kurbo::{Point, Size};
use craft::behavior::{CancelReason, CanvasView, PixelSize};
use craft::theme::variant::{Emphasis, Tone};
use craft::theme::{TypeRole, Variant};
use craft::ui::canvas::{
    AppPaint, Canvas, CanvasTool, RasterDisplay, ToolAction, ToolContext, ToolHost, ToolPath, ToolPointer, ViewTarget, tool_pass,
};
use craft::ui::contract::{Bind, Props as _, Show as _, ShowContainer as _};
use craft::ui::display::{Badge, Divider, Shade, Text};
use craft::ui::layout_components::Row;
use craft::ui::menu::{Menu, MenuItem};
use craft::ui::primitives::textures::{
    Priority, StateLook, TextureState, TextureStoreOptions, ThumbPaint, ThumbRequest, ThumbnailOptions, Thumbnails, texture_side,
};
use craft::ui::primitives::{Draw, PaintShape, StrokeWidth};
use craft::ui::runtime::memory::Remember as _;
use egui::{Pos2, Rect, Ui};
use pdfcraft_engine::DocId;
use pdfcraft_engine::ocr::{ConfidenceBand, band_for};

use super::Phase;
use super::screen::Intents;
use crate::PdfCraftApp;
use crate::canvas::pictures::DocPages;

/// Where the pane publishes, each pass, the longer side of the page as it is drawn. The zoom crop
/// reads it in the same pass, so both ask the store for one entry.
fn side_id() -> egui::Id {
    egui::Id::unique("ocr-verify-page-side")
}

/// The longer side, in points, the source pane drew the page at in this pass; `None` before the
/// pane has drawn one.
pub(super) fn published_side(ctx: &egui::Context) -> Option<f32> {
    ctx.data(|d| d.get_temp::<f32>(side_id()))
}

/// The store of the screen's page pictures. The zoom crop reads the same store and request, so
/// the page is rendered once.
fn thumbnails(ctx: &egui::Context) -> Thumbnails<usize> {
    Thumbnails::in_context(ctx, "ocr-verify-page", ThumbnailOptions::new().max_side(texture_side(ctx)).store(TextureStoreOptions::pages()))
}

/// The picture of `page` of `doc` as the store holds it, asked at `side` points on its longer
/// side: ready, loading or failed.
pub(super) fn page_state(app: &PdfCraftApp, ctx: &egui::Context, doc: DocId, page: usize, side: f32) -> TextureState {
    let source = DocPages::new(&app.session, doc);
    let request = ThumbRequest::for_cell(side, ctx.pixels_per_point());
    thumbnails(ctx).state(ctx, &source, &page, &request, Priority::VISIBLE)
}

/// One recognized word as the pane draws and hits it, in view-space points.
struct WordShape {
    key: (usize, usize),
    /// The corners in order.
    quad: [Pos2; 4],
    /// The confidence band; `None` for a word the engine gave no confidence.
    band: Option<ConfidenceBand>,
    removed: bool,
    selected: bool,
    corrected: bool,
}

/// The words of `page` as shapes, through the page's rotation into view space.
fn word_shapes(app: &PdfCraftApp, info: &pdfcraft_render::PageInfo, page: usize) -> Vec<WordShape> {
    let v = &app.ocr_verify;
    let Some(pg) = v.page(page) else { return Vec::new() };
    let to_view = |p: [f64; 2]| {
        let at = info.user_to_view(p[0] as f32, p[1] as f32);
        egui::pos2(at[0], at[1])
    };
    pg.words
        .iter()
        .enumerate()
        .map(|(wi, w)| {
            let key = (pg.page, wi);
            let (o, a, u) = (w.origin, w.across, w.up);
            WordShape {
                key,
                quad: [
                    to_view(o),
                    to_view([o[0] + a[0], o[1] + a[1]]),
                    to_view([o[0] + a[0] + u[0], o[1] + a[1] + u[1]]),
                    to_view([o[0] + u[0], o[1] + u[1]]),
                ],
                band: w.confidence.map(band_for),
                removed: v.is_removed(key),
                selected: v.selected == Some(key),
                corrected: v.is_corrected(key),
            }
        })
        .collect()
}

/// The tone a confidence band reads in: High green, Medium yellow, Low red. The legend names the
/// bands as text, so meaning never rests on colour alone.
fn band_tone(band: ConfidenceBand) -> Tone {
    match band {
        ConfidenceBand::High => Tone::Good,
        ConfidenceBand::Medium => Tone::Warn,
        ConfidenceBand::Low => Tone::Bad,
    }
}

/// A drag in progress on the page.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Drag {
    /// Where the pointer went down, on screen: a release within the slop of it is a click.
    press: Pos2,
    /// The page point the drag began at, clamped to the page.
    start: Point,
    /// The page point the pointer is at, clamped to the page.
    at: Point,
}

/// The drag the tool carries from one pass to the next; the canvas body keeps it, since the tool
/// itself is rebuilt each pass from the review state.
#[derive(Clone, Debug, Default, PartialEq)]
struct RegionMemory {
    drag: Option<Drag>,
}

/// What a gesture on the page ended in.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Finished {
    /// A region, in view-space points on the page.
    Region([f32; 4]),
    /// A click, at a view-space point.
    Click(Pos2),
}

/// The tool of the source page: a drag draws the re-OCR region, a click picks a word. It also
/// paints the word boxes and the chosen region over the page, since the canvas paints a tool
/// after its controls. The document units of the canvas are the page's view-space points, so a
/// region needs no further mapping.
pub(super) struct RegionTool {
    words: Vec<WordShape>,
    region: Option<[f32; 4]>,
    page: Size,
    drag: Option<Drag>,
    finished: Option<Finished>,
}

impl RegionTool {
    /// A tool over a page of `page` view-space points showing `words` and the chosen `region`.
    fn new(words: Vec<WordShape>, region: Option<[f32; 4]>, page: Size) -> Self {
        RegionTool { words, region, page, drag: None, finished: None }
    }

    /// `at` kept on the page.
    fn clamped(&self, at: Point) -> Point {
        Point::new(at.x.clamp(0.0, self.page.width), at.y.clamp(0.0, self.page.height))
    }

    /// The topmost word that has the view-space point `at` inside it; a removed word is not hit.
    fn hit(&self, at: Pos2) -> Option<(usize, usize)> {
        self.words.iter().find(|w| !w.removed && point_in_quad(at, &w.quad)).map(|w| w.key)
    }

    /// The page on screen.
    fn page_on_screen(&self, view: &CanvasView) -> Rect {
        Rect::from_two_pos(view.to_screen(Point::ZERO), view.to_screen(Point::new(self.page.width, self.page.height)))
    }

    /// The chosen region on screen, kept on the page, or `None` when none lies on it. The region is
    /// already in view space, so it is only scaled: mapping it through the page's user space again
    /// would mirror it vertically (and misplace it on rotated or cropped pages).
    fn outline(&self, view: &CanvasView) -> Option<Rect> {
        let [x0, y0, x1, y1] = self.region?;
        let a = view.to_screen(Point::new(f64::from(x0), f64::from(y0)));
        let b = view.to_screen(Point::new(f64::from(x1), f64::from(y1)));
        let on = Rect::from_two_pos(a, b).intersect(self.page_on_screen(view));
        (on.area() > 1.0).then_some(on)
    }
}

impl CanvasTool for RegionTool {
    fn press(&mut self, pointer: &ToolPointer, cx: &mut ToolContext<'_>) -> ToolAction {
        let Some(space) = cx.space() else { return ToolAction::Ignored };
        let at = self.clamped(space.view.to_doc(pointer.pos));
        self.drag = Some(Drag { press: pointer.pos, start: at, at });
        ToolAction::Claimed
    }

    fn drag(&mut self, pointer: &ToolPointer, cx: &mut ToolContext<'_>) {
        let Some(space) = cx.space() else { return };
        let at = self.clamped(space.view.to_doc(pointer.pos));
        if let Some(drag) = self.drag.as_mut() {
            drag.at = at;
        }
    }

    fn release(&mut self, pointer: &ToolPointer, cx: &mut ToolContext<'_>) {
        let (Some(drag), Some(space)) = (self.drag.take(), cx.space()) else { return };
        let raw = space.view.to_doc(pointer.pos);
        // A click with no drag changes no region; it only picks the word under the pointer.
        if pointer.pos.distance(drag.press) <= cx.slop {
            self.finished = Some(Finished::Click(egui::pos2(raw.x as f32, raw.y as f32)));
            return;
        }
        let to = self.clamped(raw);
        let (lo, hi) = (Point::new(drag.start.x.min(to.x), drag.start.y.min(to.y)), Point::new(drag.start.x.max(to.x), drag.start.y.max(to.y)));
        // A drag along one edge of the page (or clamped flat against it) encloses nothing to re-read.
        if hi.x > lo.x && hi.y > lo.y {
            self.finished = Some(Finished::Region([lo.x as f32, lo.y as f32, hi.x as f32, hi.y as f32]));
        }
    }

    fn cancel(&mut self, _reason: CancelReason, _cx: &mut ToolContext<'_>) {
        self.drag = None;
    }

    fn paint(&self, draw: &Draw<'_>, cx: &ToolContext<'_>) {
        let (Some(space), Some(style)) = (cx.space(), cx.style()) else { return };
        let on_screen = |quad: &[Pos2; 4]| quad.map(|p| space.view.to_screen(Point::new(f64::from(p.x), f64::from(p.y)))).to_vec();
        for word in &self.words {
            let quad = on_screen(&word.quad);
            if word.removed {
                // Removed: dimmed, struck through — still there, still restorable.
                draw.fill(&PaintShape::Polygon(quad.clone()), style.tone_fill(Tone::Neutral));
                draw.stroke(&PaintShape::Path(vec![quad[0], quad[2]]), StrokeWidth::Thin, style.tone(Tone::Neutral));
                continue;
            }
            if let Some(band) = word.band {
                draw.fill(&PaintShape::Polygon(quad.clone()), style.tone_fill(band_tone(band)));
            }
            let outline = if word.selected {
                (StrokeWidth::Medium, style.color(style.tokens.color.accent.solid))
            } else if word.corrected {
                (StrokeWidth::Thin, style.color(style.tokens.color.text.normal))
            } else {
                // The band's outline; a word without a confidence reads as unmeasured.
                (StrokeWidth::Hairline, style.tone(word.band.map_or(Tone::Neutral, band_tone)))
            };
            draw.stroke(&PaintShape::Polygon(quad), outline.0, outline.1);
        }
        // The chosen region and the one being drawn right now.
        let drawn = self.drag.map(|d| Rect::from_two_pos(space.view.to_screen(d.start), space.view.to_screen(d.at)));
        for rect in [self.outline(&space.view), drawn].into_iter().flatten() {
            draw.stroke(&PaintShape::Rect(rect, egui::CornerRadius::ZERO), StrokeWidth::Medium, style.color(style.tokens.color.accent.solid));
        }
    }

    fn command_form(&self) -> ToolPath {
        ToolPath::Freehand("the region is dragged on the page; no command takes its numbers")
    }
}

/// The source pane: the page picture, the word boxes and the region over it, and the context menu.
pub(super) fn show(app: &mut PdfCraftApp, ui: &mut Ui, n: &mut Intents) {
    let (page, review, rereading, has_region) = {
        let v = &app.ocr_verify;
        (v.current, v.phase == Phase::ReviewReady, v.reread.is_some(), v.region.is_some())
    };
    let Some(doc) = app.ocr_verify.doc else { return };
    let Some(info) = app.session.get(doc).and_then(|d| d.info.pages.get(page)).cloned() else { return };
    let ctx = ui.ctx().clone();
    let thumbs = thumbnails(&ctx);
    let look = ThumbPaint::default().states(StateLook::Captioned);
    let size = Size::new(f64::from(info.width.max(1.0)), f64::from(info.height.max(1.0)));
    let pixels = PixelSize::new(info.width.ceil().max(1.0) as u32, info.height.ceil().max(1.0) as u32);
    let mut tool = RegionTool::new(word_shapes(app, &info, page), app.ocr_verify.region, size);

    // PLAN: assumed a kept `Bind<CanvasView>` lets the canvas own its view, fitted to the page when first shown (no `ToolHostResponse::view` copy).
    let shown = Canvas::new(tl!("Source page")).view(Bind::kept()).document(ViewTarget::new(size)).show(ui, |ui, body| {
        tool.drag = body.keep::<RegionMemory>().drag;
        let paint = AppPaint::new(|cx, view: CanvasView| {
            let page_rect = Rect::from_two_pos(view.to_screen(Point::ZERO), view.to_screen(Point::new(size.width, size.height)));
            let side = page_rect.size().max_elem();
            ctx.data_mut(|d| d.insert_temp(side_id(), side));
            let state = page_state(app, &ctx, doc, page, side);
            thumbs.paint(cx, page_rect, &state, &look);
        });
        RasterDisplay::painted(tl!("Source page"), pixels, paint).show_at(ui, body.rect);
        if let Some(pass) = tool_pass(&body.env) {
            // The region tool moves no view: the drag it draws is read back from the tool below.
            let _gesture = ToolHost::new().run(ui, &pass, &mut tool);
        }
        body.keep::<RegionMemory>().drag = tool.drag;
    });

    match tool.finished.take() {
        Some(Finished::Region(region)) => app.ocr_verify.region = Some(region),
        Some(Finished::Click(at)) => n.select_word = tool.hit(at),
        None => {}
    }

    Menu::context(tl!("Source page"), &shown.output.response).show(ui, |ui, _| {
        if MenuItem::new(tl!("Re-OCR this region")).enabled(review && !rereading && has_region).show(ui).ran {
            n.reocr_region = true;
        }
        if MenuItem::new(tl!("Re-OCR entire page")).enabled(review && !rereading).show(ui).ran {
            n.reocr_page = true;
        }
        Divider::horizontal().show(ui);
        if MenuItem::new(tl!("Accept entire page")).enabled(review).show(ui).ran {
            n.accept_page = true;
        }
        if MenuItem::new(tl!("Reject entire page")).enabled(review).show(ui).ran {
            n.reject_page = true;
        }
        Divider::horizontal().show(ui);
        Text::new(tl!("Click a word to inspect it; drag a rectangle to re-read part of the page"))
            .role(TypeRole::Caption)
            .shade(Shade::Faint)
            .show(ui);
    });
}

/// The overlay legend: the classifier's bands, with their thresholds spelled out.
pub(super) fn legend(ui: &mut Ui) {
    Row::new().wrap(true).show(ui, |ui, _| {
        Text::new(tl!("CONFIDENCE")).role(TypeRole::Caption).show(ui);
        for (band, text) in legend_rows() {
            Badge::new(text).variant(Variant::look(Emphasis::Soft, band_tone(band))).show(ui);
        }
    });
}

/// The legend's rows: the same [`ConfidenceBand`] values the overlay, the counts and the
/// navigation use, with the classifier's thresholds as text (a test holds the drawing to
/// these numbers).
pub fn legend_rows() -> [(ConfidenceBand, String); 3] {
    [
        (ConfidenceBand::High, tl!("HIGH (>= 90%)").to_string()),
        (ConfidenceBand::Medium, tl!("MEDIUM (70-89%)").to_string()),
        (ConfidenceBand::Low, tl!("LOW (< 70%)").to_string()),
    ]
}

/// Point in a convex quad (corners in order): the pointer is on the same side of all four
/// edges.
fn point_in_quad(p: Pos2, quad: &[Pos2; 4]) -> bool {
    (0..4).all(|i| {
        let (a, b) = (quad[i], quad[(i + 1) % 4]);
        (b.x - a.x) * (p.y - a.y) - (b.y - a.y) * (p.x - a.x) <= 0.0
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The numbers a legend row's text states.
    fn numbers(text: &str) -> Vec<f32> {
        text.split(|c: char| !c.is_ascii_digit()).filter(|part| !part.is_empty()).filter_map(|part| part.parse().ok()).collect()
    }

    /// The legend's text is the classifier's thresholds, spelled out — a drift between the
    /// two would put the wrong words in front of the reviewer.
    #[test]
    fn legend_rows_carry_the_classifier_thresholds() {
        let rows = legend_rows();
        assert_eq!(rows.iter().map(|row| row.0).collect::<Vec<_>>(), [ConfidenceBand::High, ConfidenceBand::Medium, ConfidenceBand::Low]);
        let (high, medium, low) = (numbers(&rows[0].1), numbers(&rows[1].1), numbers(&rows[2].1));
        // Each row names the confidence where its band begins: that value is in the band and one below it is in the next lower one.
        assert_eq!(band_for(high[0]), ConfidenceBand::High);
        assert_eq!(band_for(high[0] - 1.0), ConfidenceBand::Medium);
        assert_eq!(band_for(medium[0]), ConfidenceBand::Medium);
        assert_eq!(band_for(medium[0] - 1.0), ConfidenceBand::Low);
        // The Medium row ends where High begins, and the Low row ends where Medium begins.
        assert_eq!(medium[1], high[0] - 1.0);
        assert_eq!(low[0], medium[0]);
        // The same classifier drives the overlay: the bands clamp out-of-range values.
        assert_eq!(band_for(-1.0), ConfidenceBand::Low);
        assert_eq!(band_for(140.0), ConfidenceBand::High);
        assert_eq!(band_for(f32::NAN), ConfidenceBand::Low);
    }

    /// The drawn region is stored in view space, so its outline is that rectangle scaled onto the
    /// page: not mirrored on an upright page, and in place on a page whose view is turned and
    /// cropped (a different view size).
    #[test]
    fn the_region_outline_is_where_it_was_drawn() {
        let region = [100.0_f32, 50.0, 200.0, 150.0];
        for (width, height) in [(612.0, 792.0), (792.0, 612.0)] {
            let page = Size::new(width, height);
            let zoom = 0.5;
            let view =
                CanvasView::new(Rect::from_min_size(egui::pos2(10.0, 20.0), egui::vec2(500.0, 400.0)), Point::new(width / 2.0, height / 2.0), zoom);
            let tool = RegionTool::new(Vec::new(), Some(region), page);
            let outline = tool.outline(&view).expect("the region lies on the page");
            let on_page = tool.page_on_screen(&view);
            assert!(on_page.contains_rect(outline), "the outline stays on the page");
            // Scaled uniformly: both sides shrink by the zoom.
            assert!((outline.width() - 100.0 * zoom as f32).abs() < 0.01);
            assert!((outline.height() - 100.0 * zoom as f32).abs() < 0.01);
            // Not mirrored: the region's top is the same distance below the page's top as it was drawn.
            assert!((outline.top() - on_page.top() - 50.0 * zoom as f32).abs() < 0.01);
            assert!((outline.left() - on_page.left() - 100.0 * zoom as f32).abs() < 0.01);
        }
        // A region off the page has no outline.
        let page = Size::new(612.0, 792.0);
        let view = CanvasView::new(Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(500.0, 400.0)), Point::new(306.0, 396.0), 0.5);
        assert!(RegionTool::new(Vec::new(), Some([700.0, 900.0, 800.0, 1000.0]), page).outline(&view).is_none());
    }
}
