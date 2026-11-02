//! Add Header and Footer, Add Watermark and Add Background dialogs (Acrobat: Edit a PDF ▸ …;
//! dialog snapshots d13, d14, d12d). Each has the shared Page Range Options and a preview of a
//! page the range selects, with the marks laid out on it. "Update" opens the same dialog in
//! replace mode.

use std::collections::{HashMap, HashSet};

use egui::{Align, Color32, CornerRadius, Layout, Rect, Stroke, pos2, vec2};
use pdfcraft_engine::{Backdrop, Background, DocId, Edit, FontFamily, HeaderFooter, MarkKind, Watermark};

use crate::theme::{self, Tokens};
use crate::{PdfCraftApp, icons, widgets};

/// How far a watermark may be moved from the page centre, in points.
const MAX_OFFSET: f64 = 20_000.0;

/// The preview page's longer side, in points.
const PREVIEW_SIDE: f32 = 190.0;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Subset {
    All,
    Even,
    Odd,
}

/// Page Range Options (shared by the three dialogs). Pages are 1-based.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PageRange {
    pub all: bool,
    pub from: usize,
    pub to: usize,
    pub subset: Subset,
}

impl Default for PageRange {
    /// From the first page to the last: `to` starts past any page count, and the range field
    /// shows (and clamps it to) the document's last page.
    fn default() -> Self {
        Self { all: true, from: 1, to: usize::MAX, subset: Subset::All }
    }
}

impl PageRange {
    /// The pages (0-based) the range selects in a document of `count` pages.
    pub fn pages(&self, count: usize) -> Vec<usize> {
        let (a, b) = if self.all { (1, count) } else { (self.from.max(1), self.to.min(count)) };
        (a..=b)
            .filter(|p| match self.subset {
                Subset::All => true,
                Subset::Even => p % 2 == 0,
                Subset::Odd => p % 2 == 1,
            })
            .map(|p| p - 1)
            .collect()
    }

    pub(crate) fn ui(&mut self, ui: &mut egui::Ui, count: usize) {
        ui.horizontal(|ui| {
            widgets::radio_value(ui, &mut self.all, true, tl!("All pages"));
            widgets::radio_value(ui, &mut self.all, false, tl!("Pages from"));
            ui.add_enabled(!self.all, egui::DragValue::new(&mut self.from).range(1..=count.max(1)));
            ui.label(tl!("to"));
            ui.add_enabled(!self.all, egui::DragValue::new(&mut self.to).range(1..=count.max(1)));
            ui.label(tl!("Subset"));
            let label = match self.subset {
                Subset::All => tl!("All pages in range"),
                Subset::Even => tl!("Even pages only"),
                Subset::Odd => tl!("Odd pages only"),
            };
            craft_ui::ComboBox::from_id_salt("mark-subset").selected_text(label).show_ui(ui, |ui| {
                ui.selectable_value(&mut self.subset, Subset::All, tl!("All pages in range"));
                ui.selectable_value(&mut self.subset, Subset::Even, tl!("Even pages only"));
                ui.selectable_value(&mut self.subset, Subset::Odd, tl!("Odd pages only"));
            });
        });
    }
}

/// How many pages a frame the Background dialog checks for a backdrop of their own, so a long
/// document never stalls the dialog.
const BACKDROP_PAGES_PER_FRAME: usize = 24;

/// Which pages paint their own backdrop (it hides a background), for the document and edit
/// generation they were checked in. Filled a few pages a frame.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct BackdropCheck {
    key: Option<(DocId, u64)>,
    /// Per page: `None` until checked.
    pages: Vec<Option<Option<Backdrop>>>,
}

/// What the check found among the pages a background goes on.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct BackdropCount {
    /// Pages (0-based, ascending) with a white backdrop that can be replaced.
    pub fills: Vec<usize>,
    /// Pages (0-based, ascending) covered by a picture (scans): the background stays hidden there.
    pub images: Vec<usize>,
    /// Pages not checked yet.
    pub pending: usize,
}

impl BackdropCheck {
    /// Check up to [`BACKDROP_PAGES_PER_FRAME`] more of `pages` (0-based) and sort them.
    pub fn update(&mut self, doc: &pdfcraft_engine::Document, pages: &[usize]) -> BackdropCount {
        let key = (doc.id, doc.edit_generation());
        let count = doc.info.pages.len();
        if self.key != Some(key) || self.pages.len() != count {
            self.key = Some(key);
            self.pages = vec![None; count];
        }
        // One batch a frame: finding a page walks the page tree, so the batch shares one walk.
        let mut batch = Vec::with_capacity(BACKDROP_PAGES_PER_FRAME);
        for &p in pages {
            if batch.len() >= BACKDROP_PAGES_PER_FRAME {
                break;
            }
            if matches!(self.pages.get(p), Some(None)) && !batch.contains(&p) {
                batch.push(p);
            }
        }
        if !batch.is_empty() {
            for (p, found) in batch.iter().zip(doc.page_backdrops(&batch)) {
                if let Some(slot) = self.pages.get_mut(*p) {
                    *slot = Some(found);
                }
            }
        }
        let mut n = BackdropCount::default();
        for &p in pages {
            let Some(slot) = self.pages.get(p) else { continue };
            match slot {
                None => n.pending += 1,
                Some(Some(Backdrop::Fill)) => n.fills.push(p),
                Some(Some(Backdrop::Image)) => n.images.push(p),
                Some(None) => {}
            }
        }
        for list in [&mut n.fills, &mut n.images] {
            list.sort_unstable();
            list.dedup();
        }
        n
    }

    /// What the check found on `page` (0-based) of this version of `doc`: `None` until it has
    /// looked, `Some(None)` for a page with no backdrop of its own.
    pub fn found(&self, doc: &pdfcraft_engine::Document, page: usize) -> Option<Option<Backdrop>> {
        if self.key != Some((doc.id, doc.edit_generation())) {
            return None;
        }
        self.pages.get(page).copied().flatten()
    }
}

impl MarksDraft {
    /// The dialog for `kind` is opening. Other settings carry over from last time, but replacing
    /// a page's own backdrop starts on: Update restores backdrops before it re-applies, so a
    /// page whose backdrop was replaced shows none and the checkbox stays hidden, and a choice
    /// left off from an earlier dialog would hide the background again unseen.
    pub fn opened(&mut self, kind: MarkKind) {
        if kind == MarkKind::Background {
            self.bg.replace_backdrop = true;
        }
    }
}

/// How many runs of pages a message lists before it stops with "…".
const LISTED_PAGE_RUNS: usize = 8;

/// After Remove background: the pages (0-based, ascending) whose own white backdrop stays off,
/// because they changed after the background was added (`pdfcraft_engine::EditNotice`).
pub(crate) fn backdrops_left_off_message(pages: &[usize]) -> String {
    pages_phrase(
        pages,
        tl!("Page {pages} changed after the background was added, so its own white backdrop stays off. It looks the same on white paper."),
        tl!("Pages {pages} changed after the background was added, so their own white backdrops stay off. They look the same on white paper."),
    )
}

/// `one` or `many` with `{pages}` replaced by `pages` (0-based, ascending) as 1-based runs
/// ("1-3, 6"), so the message says which pages it means.
fn pages_phrase(pages: &[usize], one: &str, many: &str) -> String {
    // `count` 0 never matches a list that isn't empty, so the list is always written out.
    let runs = crate::combine_ui::range_of(pages, 0);
    let mut parts: Vec<&str> = runs.split(", ").take(LISTED_PAGE_RUNS + 1).collect();
    if parts.len() > LISTED_PAGE_RUNS {
        parts.truncate(LISTED_PAGE_RUNS);
        parts.push("…");
    }
    let template = if pages.len() == 1 { one } else { many };
    crate::i18n::fmt(template, &[("pages", &parts.join(", "))])
}

#[derive(Clone, Debug, PartialEq)]
pub struct MarksDraft {
    pub hf: HeaderFooter,
    pub wm: Watermark,
    pub bg: Background,
    pub range: PageRange,
    /// Replace existing marks of this kind (Update).
    pub replace: bool,
    /// Which header/footer box Insert Page Number / Insert Date type into.
    pub focused_box: usize,
    pub page_format: usize,
    pub date_format: usize,
    /// Watermark: size the text to the page.
    pub fit: bool,
    /// Source ▸ File (watermark or background): the file picked, and the page of a PDF (1-based).
    pub use_file: bool,
    pub file: Option<(String, std::sync::Arc<Vec<u8>>)>,
    pub file_page: usize,
    /// Background: pages that paint their own backdrop.
    pub backdrops: BackdropCheck,
    /// The page (0-based) the preview was stepped to; `None` previews the current page. Reset
    /// each time the dialog opens.
    pub preview: Option<usize>,
}

impl Default for MarksDraft {
    fn default() -> Self {
        Self {
            hf: HeaderFooter::default(),
            wm: Watermark::default(),
            bg: Background::default(),
            range: PageRange::default(),
            replace: false,
            focused_box: 1,
            page_format: 0,
            date_format: 0,
            fit: true,
            use_file: false,
            file: None,
            file_page: 1,
            backdrops: BackdropCheck::default(),
            preview: None,
        }
    }
}

const PAGE_FORMATS: [&str; 5] = ["<<1>>", "<<1 of n>>", "<<1/n>>", "<<Page 1>>", "<<Page 1 of n>>"];
// New formats go last: `date_format` is an index into this list.
const DATE_FORMATS: [&str; 11] = [
    "<<m/d/yyyy>>",
    "<<mm/dd/yyyy>>",
    "<<d/m/yyyy>>",
    "<<yyyy-mm-dd>>",
    "<<m/d/yy>>",
    "<<mmmm d, yyyy>>",
    "<<yyyy/mm/dd>>",
    "<<yyyy/m/d>>",
    "<<dd.mm.yyyy>>",
    "<<d.m.yyyy>>",
    "<<dd.mm.yy>>",
];
const BOX_NAMES: [&str; 6] =
    ["Left Header Text", "Center Header Text", "Right Header Text", "Left Footer Text", "Center Footer Text", "Right Footer Text"];

fn color_button(ui: &mut egui::Ui, c: &mut [f64; 3]) {
    let mut f = c.map(|v| v as f32);
    if egui::color_picker::color_edit_button_rgb(ui, &mut f).changed() {
        *c = f.map(f64::from);
    }
}

/// The page (displayed size `size`) with the marks laid out on it. `picture` is its thumbnail,
/// rendered off the UI thread like the pages panel's; until it arrives the page is a blank sheet.
/// `tint` multiplies the page, which is how a background behind its content looks.
fn preview(
    ui: &mut egui::Ui,
    t: &Tokens,
    size: (f64, f64),
    picture: Option<egui::TextureId>,
    tint: Color32,
    draw: impl FnOnce(&egui::Painter, Rect, f32),
) {
    let (w, h) = (size.0.max(1.0), size.1.max(1.0));
    let s = (f64::from(PREVIEW_SIDE) / w.max(h)) as f32;
    let (r, _) = ui.allocate_exact_size(vec2(210.0, 210.0), egui::Sense::hover());
    let page = Rect::from_center_size(r.center(), vec2(w as f32 * s, h as f32 * s));
    let painter = ui.painter_at(r);
    match picture {
        Some(tex) => {
            painter.image(tex, page, Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)), tint);
        }
        None => {
            painter.rect_filled(page, CornerRadius::ZERO, tint);
        }
    }
    painter.rect_stroke(page, CornerRadius::ZERO, Stroke::new(1.0, t.border), egui::StrokeKind::Outside);
    draw(&painter, page, s);
}

fn rgb32(c: [f64; 3]) -> Color32 {
    let b = |v: f64| (v.clamp(0.0, 1.0) * 255.0).round() as u8;
    Color32::from_rgb(b(c[0]), b(c[1]), b(c[2]))
}

/// The tint of a page over a background of `color` at `opacity`: the colour shows where the page
/// is white and the content stays on top. A non-finite opacity is opaque, as the engine has it.
fn behind(color: [f64; 3], opacity: f64) -> Color32 {
    let a = if opacity.is_finite() { opacity.clamp(0.0, 1.0) } else { 1.0 };
    rgb32(color.map(|c| 1.0 - a * (1.0 - c)))
}

/// The preview's tint for a background colour on a page whose own backdrop is `found` (`None`
/// when it has none, or the check hasn't reached it): no colour where the page will still hide
/// the background after OK, because a picture covers it (a scan) or it keeps its white backdrop
/// with Replace unticked.
fn background_tint(d: &MarksDraft, found: Option<Backdrop>) -> Color32 {
    let hidden = match found {
        Some(Backdrop::Image) => true,
        Some(Backdrop::Fill) => !d.bg.replace_backdrop,
        None => false,
    };
    if hidden { Color32::WHITE } else { behind(d.bg.color, d.bg.opacity) }
}

/// In Update, the pages previewed without the marks the dialog replaces. Their thumbnails still
/// have them, so the new marks would be drawn over the old ones, and a new background colour
/// would tint the old one. The document without them is made once per dialog (it costs about a
/// save). The page shown is drawn from it straight away, at the preview's small size, so neither
/// a blank sheet nor the old marks show while it renders; the pages a step goes to next render
/// ahead, off the UI thread (not on the web, where the pool renders on it), so stepping rarely
/// waits.
#[derive(Default)]
pub(crate) struct UnmarkedPage {
    /// The document, its edit generation and the kind of mark taken away.
    key: Option<(pdfcraft_engine::DocId, u64, MarkKind)>,
    pages: Option<pdfcraft_engine::UnmarkedPages>,
    /// Renders the pages next to the one shown, ahead. None on the web, where the pool renders on
    /// the UI thread inside `try_recv`: it would draw them in the same frame too.
    renderer: Option<pdfcraft_render::RenderPool>,
    /// The pages rendered, with the scale (×1000) they were rendered at: the page shown and the
    /// pages next to it.
    textures: HashMap<usize, (u64, egui::TextureHandle)>,
    /// Pages that failed to render when shown, so they aren't tried every frame: their
    /// thumbnails show instead, the other pages still don't.
    failed: HashSet<usize>,
    /// The pages asked of the renderer ahead, and not back yet. Nothing waits for them: a step
    /// is a frame, which takes what has come back first and draws the rest itself.
    ahead: Vec<(usize, u64)>,
    /// Pages (at a scale) that came back from rendering ahead with an error, not asked ahead
    /// again: the error needn't be the page's, so drawing it when shown still decides.
    not_ahead: HashSet<(usize, u64)>,
}

impl UnmarkedPage {
    /// The picture to preview `page` on in place of its thumbnail, or `None` for the thumbnail:
    /// not Update, no marks of `kind` in the document, the pages can't be prepared, or `page`
    /// fails to render and was never drawn (a picture of it at another scale is shown rather than
    /// the old marks). `near` are the pages a step goes to from it, rendered ahead.
    fn picture(
        &mut self,
        ctx: &egui::Context,
        (doc, id): (&pdfcraft_engine::Document, pdfcraft_engine::DocId),
        page: usize,
        near: &[usize],
        kind: MarkKind,
        replace: bool,
    ) -> Option<egui::TextureId> {
        let key = replace.then(|| (id, doc.edit_generation(), kind));
        if self.key != key {
            *self = Self { key, ..Default::default() };
            if replace {
                self.pages = doc.without_marks(kind).ok().flatten();
                self.renderer = self.pages.as_ref().filter(|_| !cfg!(target_arch = "wasm32")).map(|p| p.renderer());
            }
        }
        self.pages.as_ref()?;
        let near = if self.renderer.is_some() { near } else { &[][..] };
        // At the preview's size in pixels.
        let scale = |page: usize| {
            let p = doc.info.pages.get(page)?;
            let s = PREVIEW_SIDE * ctx.pixels_per_point() / p.width.max(p.height).max(1.0);
            Some((s, (s * 1000.0) as u64))
        };
        let load = |ctx: &egui::Context, out: pdfcraft_render::RenderedPage| {
            let image = crate::canvas::texture_image([out.width as usize, out.height as usize], out.rgba);
            ctx.load_texture("marks-preview-unmarked", image, egui::TextureOptions::LINEAR)
        };
        while let Some(out) = self.renderer.as_ref().and_then(|r| r.try_recv()) {
            let at = (out.request.page, out.request.tag);
            self.ahead.retain(|a| *a != at);
            // An error rendering ahead needn't be the page's (the pool retires its parser, a
            // request is cancelled): the page is drawn here when it is shown, and that decides.
            // It isn't asked ahead again, so a page that always fails isn't rendered every frame.
            if out.error.is_none() {
                self.textures.insert(at.0, (at.1, load(ctx, out)));
            } else {
                self.not_ahead.insert(at);
            }
        }
        let (s, tag) = scale(page)?;
        if !self.failed.contains(&page)
            && self.textures.get(&page).is_none_or(|(t, _)| *t != tag)
            && let Some(pages) = self.pages.as_mut()
        {
            match pages.render_page(page, s) {
                Ok(out) => {
                    self.textures.insert(page, (tag, load(ctx, out)));
                }
                Err(_) => {
                    self.failed.insert(page);
                }
            }
        }
        // Ahead: the pages a step goes to, not rendered yet at their scale.
        let wanted: Vec<(usize, u64)> = near
            .iter()
            .filter(|p| !self.failed.contains(p))
            .filter_map(|&p| scale(p).map(|(_, t)| (p, t)))
            .filter(|at| !self.not_ahead.contains(at))
            .filter(|(p, t)| self.textures.get(p).is_none_or(|(have, _)| have != t))
            .collect();
        if let Some(renderer) = self.renderer.as_ref()
            && !wanted.is_empty()
            && wanted != self.ahead
        {
            let requests = wanted
                .iter()
                .filter_map(|&(p, _)| scale(p).map(|(s, tag)| pdfcraft_render::RenderRequest { page: p, scale: s, tag, ..Default::default() }));
            renderer.set_queue(requests.collect());
            self.ahead = wanted;
        }
        self.textures.retain(|p, _| *p == page || near.contains(p));
        // A picture rendered at another scale beats the thumbnail with the old marks.
        self.textures.get(&page).map(|(_, t)| t.id())
    }
}

/// The page (0-based) the preview shows: the current one when the range selects it, otherwise the
/// first page the range selects (the current one again when it selects none).
fn preview_page(range: &PageRange, current: usize, count: usize) -> usize {
    let pages = range.pages(count);
    if pages.contains(&current) { current } else { pages.first().copied().unwrap_or(current) }
}

/// [`preview_page`] for the page stepped to (`stepped`), or the current page if none. A page
/// stepped to that the range leaves out is replaced by the page shown, so widening the range again
/// keeps that page rather than jumping back.
fn shown_preview(range: &PageRange, stepped: &mut Option<usize>, current: usize, count: usize) -> usize {
    let shown = preview_page(range, stepped.unwrap_or(current), count);
    if stepped.is_some() {
        *stepped = Some(shown);
    }
    shown
}

/// The page the preview steps to from `shown`: the range's next page after it (`forward`) or the
/// last one before it, or `None` at either end.
fn step_preview(range: &PageRange, shown: usize, count: usize, forward: bool) -> Option<usize> {
    let pages = range.pages(count);
    if forward { pages.into_iter().find(|&p| p > shown) } else { pages.into_iter().rev().find(|&p| p < shown) }
}

/// The dialog for `kind`; returns (apply, cancel).
pub(crate) fn body(ui: &mut egui::Ui, app: &mut PdfCraftApp, t: &Tokens, kind: MarkKind) -> (bool, bool) {
    let Some((i, id)) = app.active_ids() else { return (false, true) };
    let current = app.views[i].current;
    let Some(doc) = app.session.get(id) else { return (false, true) };
    let count = doc.info.pages.len();
    let units = app.units;
    let d = &mut app.marks_draft;
    let mut browse = false;
    let verb = if d.replace { tl!("Update") } else { tl!("Add") };
    let title = match kind {
        MarkKind::HeaderFooter => crate::i18n::fmt(tl!("{verb} Header and Footer"), &[("verb", verb)]),
        MarkKind::Watermark => crate::i18n::fmt(tl!("{verb} Watermark"), &[("verb", verb)]),
        MarkKind::Background => crate::i18n::fmt(tl!("{verb} Background"), &[("verb", verb)]),
    };
    ui.label(egui::RichText::new(title).font(theme::semibold(18.0)));
    ui.add_space(8.0);
    match kind {
        MarkKind::HeaderFooter => {
            ui.horizontal(|ui| {
                // The standard faces a header can be drawn in (#3754).
                ui.label(tl!("Font:"));
                craft_ui::ComboBox::from_id_salt("hf-font").selected_text(d.hf.font.label()).width(100.0).show_ui(ui, |ui| {
                    for f in [FontFamily::Helvetica, FontFamily::Times, FontFamily::Courier] {
                        ui.selectable_value(&mut d.hf.font, f, f.label());
                    }
                });
                ui.label(tl!("Size"));
                ui.add(egui::DragValue::new(&mut d.hf.font_size).range(4.0..=72.0).speed(0.5));
                ui.add(widgets::checkbox(&mut d.hf.underline, tl!("Underline")));
                color_button(ui, &mut d.hf.color);
                ui.add_space(16.0);
                ui.label(format!("{} ({})", tl!("Margins"), units.label()));
                let per = units.per_point();
                for (label, k) in [(tl!("Top"), 0), (tl!("Bottom"), 1), (tl!("Left"), 2), (tl!("Right"), 3)] {
                    ui.label(label);
                    let mut v = d.hf.margins[k] * per;
                    // Up to 10 inches, whatever the unit.
                    let max = 720.0 * per;
                    if ui.add(egui::DragValue::new(&mut v).range(0.0..=max).speed(units.speed()).max_decimals(units.decimals())).changed() {
                        d.hf.margins[k] = v / per;
                    }
                }
            });
            ui.add_space(8.0);
            egui::Grid::new("hf-boxes").num_columns(3).spacing([10.0, 4.0]).show(ui, |ui| {
                for row in 0..2 {
                    for col in 0..3 {
                        ui.label(egui::RichText::new(tl!(BOX_NAMES[row * 3 + col])).small());
                    }
                    ui.end_row();
                    for col in 0..3 {
                        let k = row * 3 + col;
                        let r = ui.add_sized([220.0, 40.0], crate::widgets::block(&mut d.hf.text[k]).desired_rows(2).id_salt(("hf-box", k)));
                        if r.gained_focus() || r.has_focus() {
                            d.focused_box = k;
                        }
                    }
                    ui.end_row();
                }
            });
            ui.horizontal(|ui| {
                craft_ui::ComboBox::from_id_salt("hf-page-format").selected_text(PAGE_FORMATS[d.page_format].trim_matches(['<', '>'])).show_ui(
                    ui,
                    |ui| {
                        for (k, f) in PAGE_FORMATS.iter().enumerate() {
                            ui.selectable_value(&mut d.page_format, k, f.trim_matches(['<', '>']));
                        }
                    },
                );
                if ui.button(tl!("Insert Page Number")).clicked() {
                    d.hf.text[d.focused_box].push_str(PAGE_FORMATS[d.page_format]);
                }
                ui.add_space(12.0);
                craft_ui::ComboBox::from_id_salt("hf-date-format").selected_text(DATE_FORMATS[d.date_format].trim_matches(['<', '>'])).show_ui(
                    ui,
                    |ui| {
                        for (k, f) in DATE_FORMATS.iter().enumerate() {
                            ui.selectable_value(&mut d.date_format, k, f.trim_matches(['<', '>']));
                        }
                    },
                );
                if ui.button(tl!("Insert Date")).clicked() {
                    d.hf.text[d.focused_box].push_str(DATE_FORMATS[d.date_format]);
                }
                ui.add_space(12.0);
                ui.label(tl!("Start page number"));
                ui.add(egui::DragValue::new(&mut d.hf.start_number).range(1..=999_999));
            });
            ui.add(widgets::checkbox(&mut d.hf.shrink, tl!("Shrink the page content to make room for the header and footer")));
        }
        MarkKind::Watermark => {
            ui.horizontal(|ui| {
                ui.label(tl!("Source"));
                widgets::radio_value(ui, &mut d.use_file, false, tl!("Text"));
                widgets::radio_value(ui, &mut d.use_file, true, tl!("File"));
            });
            if d.use_file {
                browse |= file_source(ui, d, MarkKind::Watermark);
            }
            ui.horizontal_top(|ui| {
                ui.vertical(|ui| {
                    ui.add_enabled_ui(!d.use_file, |ui| {
                        ui.label(tl!("Text"));
                        ui.add(
                            crate::widgets::block(&mut d.wm.text).desired_rows(2).desired_width(320.0).hint_text("CONFIDENTIAL").id_salt("wm-text"),
                        );
                    });
                    ui.horizontal(|ui| {
                        ui.add_enabled_ui(!d.use_file, |ui| ui.add(widgets::checkbox(&mut d.fit, tl!("Fit to page"))));
                        ui.add_enabled_ui(!d.fit, |ui| {
                            ui.label(tl!("Size"));
                            if d.wm.font_size == 0.0 {
                                d.wm.font_size = 72.0;
                            }
                            ui.add(egui::DragValue::new(&mut d.wm.font_size).range(6.0..=300.0));
                        });
                        color_button(ui, &mut d.wm.color);
                    });
                    ui.horizontal(|ui| {
                        ui.label(tl!("Rotation"));
                        for r in [-45.0, 0.0, 45.0] {
                            widgets::radio_value(ui, &mut d.wm.rotation, r, if r == 0.0 { tl!("None").to_string() } else { format!("{r}°") });
                        }
                        ui.add(egui::DragValue::new(&mut d.wm.rotation).range(-180.0..=180.0).suffix("°"));
                    });
                    ui.horizontal(|ui| {
                        ui.label(tl!("Opacity"));
                        let mut pct = d.wm.opacity * 100.0;
                        if ui.add(egui::Slider::new(&mut pct, 0.0..=100.0).suffix("%")).changed() {
                            d.wm.opacity = pct / 100.0;
                        }
                    });
                    ui.horizontal(|ui| {
                        ui.label(tl!("Location"));
                        widgets::radio_value(ui, &mut d.wm.behind, true, tl!("Appear behind page"));
                        widgets::radio_value(ui, &mut d.wm.behind, false, tl!("Appear on top of page"));
                    });
                    ui.horizontal(|ui| {
                        ui.label(tl!("Offset from centre"));
                        ui.label(tl!("Horizontal"));
                        ui.add(egui::DragValue::new(&mut d.wm.offset[0]).range(-MAX_OFFSET..=MAX_OFFSET).suffix(" pt"));
                        ui.label(tl!("Vertical"));
                        ui.add(egui::DragValue::new(&mut d.wm.offset[1]).range(-MAX_OFFSET..=MAX_OFFSET).suffix(" pt"));
                    });
                });
            });
        }
        MarkKind::Background => {
            ui.horizontal(|ui| {
                widgets::radio_value(ui, &mut d.use_file, false, tl!("From colour"));
                ui.add_enabled_ui(!d.use_file, |ui| color_button(ui, &mut d.bg.color));
                ui.add_space(12.0);
                widgets::radio_value(ui, &mut d.use_file, true, tl!("File"));
            });
            if d.use_file {
                browse |= file_source(ui, d, MarkKind::Background);
            }
            ui.horizontal(|ui| {
                ui.label(tl!("Opacity"));
                let mut pct = d.bg.opacity * 100.0;
                if ui.add(egui::Slider::new(&mut pct, 0.0..=100.0).suffix("%")).changed() {
                    d.bg.opacity = pct / 100.0;
                }
            });
            if let Some(doc) = app.session.get(id) {
                backdrop_options(ui, t, d, doc, count);
            }
        }
    }
    ui.add_space(8.0);
    ui.label(egui::RichText::new(tl!("Page Range Options")).font(theme::semibold(12.5)));
    d.range.ui(ui, count);
    ui.add_space(8.0);
    // The preview: the current page (or the page stepped to), or the first one the range selects
    // if it leaves that out, drawn from its thumbnail at its displayed size (after /Rotate), the
    // frame the engine lays the marks out in. Update draws it without the marks it replaces.
    let target = shown_preview(&d.range, &mut d.preview, current, count);
    let page = doc.info.pages.get(target).map(|p| (p.width as f64, p.height as f64)).unwrap_or((612.0, 792.0));
    let near: Vec<usize> = [false, true].into_iter().filter_map(|forward| step_preview(&d.range, target, count, forward)).collect();
    let unmarked = app.unmarked_page.picture(ui.ctx(), (doc, id), target, &near, kind, d.replace);
    let picture = unmarked.or_else(|| {
        app.views.get_mut(i).and_then(|view| {
            view.need_thumbnail(target, true);
            view.thumb_id(target)
        })
    });
    let tint =
        if kind == MarkKind::Background && !d.use_file { background_tint(d, d.backdrops.found(doc, target).flatten()) } else { Color32::WHITE };
    let hf = d.hf.clone();
    let size_h = page.1.max(1.0);
    let (wm, fit) = (d.wm.clone(), d.fit);
    let file_name = d.use_file.then(|| d.file.as_ref().map(|f| f.0.clone())).flatten();
    ui.horizontal(|ui| {
        preview(ui, t, page, picture, tint, |p, r, s| match kind {
            MarkKind::HeaderFooter => {
                if hf.shrink {
                    // The band the page content shrinks into, centred across.
                    let (top, bottom) = hf.bands();
                    let k = ((size_h - top - bottom) / size_h).clamp(0.0, 1.0) as f32;
                    let band = Rect::from_min_max(pos2(r.left(), r.top() + top as f32 * s), pos2(r.right(), r.bottom() - bottom as f32 * s));
                    let content = Rect::from_center_size(band.center(), vec2(r.width() * k, band.height().max(0.0)));
                    p.rect_filled(content, CornerRadius::ZERO, Color32::from_gray(232));
                }
                for (k, text) in hf.text.iter().enumerate() {
                    if text.trim().is_empty() {
                        continue;
                    }
                    let shown = text.replace("<<", "").replace(">>", "");
                    let y = if k < 3 { r.top() + hf.margins[0] as f32 * s } else { r.bottom() - hf.margins[1] as f32 * s };
                    let (x, align) = match k % 3 {
                        0 => (r.left() + hf.margins[2] as f32 * s, egui::Align2::LEFT_CENTER),
                        1 => (r.center().x, egui::Align2::CENTER_CENTER),
                        _ => (r.right() - hf.margins[3] as f32 * s, egui::Align2::RIGHT_CENTER),
                    };
                    // Courier previews in the monospaced UI face, the others proportional.
                    let size = (hf.font_size as f32 * s).max(5.0);
                    let font = if hf.font == FontFamily::Courier { egui::FontId::monospace(size) } else { egui::FontId::proportional(size) };
                    p.text(pos2(x, y), align, shown, font, rgb32(hf.color));
                }
            }
            MarkKind::Watermark if file_name.is_some() => {
                let side = r.width().min(r.height()) * wm.scale as f32;
                let centre = r.center() + vec2(wm.offset[0] as f32 * s, -wm.offset[1] as f32 * s);
                p.rect_stroke(
                    Rect::from_center_size(centre, vec2(side, side)),
                    CornerRadius::ZERO,
                    egui::Stroke::new(1.0, Color32::GRAY),
                    egui::StrokeKind::Inside,
                );
                p.text(centre, egui::Align2::CENTER_CENTER, file_name.clone().unwrap_or_default(), egui::FontId::proportional(10.0), Color32::GRAY);
            }
            MarkKind::Watermark => {
                let text = if wm.text.trim().is_empty() { "CONFIDENTIAL" } else { wm.text.trim() };
                let size = if fit {
                    (r.width().hypot(r.height()) * 0.5 / (text.len().max(1) as f32 * 0.55)).clamp(6.0, 80.0)
                } else {
                    (wm.font_size as f32 * s).max(5.0)
                };
                let galley = p.layout_no_wrap(text.to_string(), egui::FontId::proportional(size), rgb32(wm.color).gamma_multiply(wm.opacity as f32));
                let angle = -(wm.rotation as f32).to_radians();
                let half = galley.size() / 2.0;
                let (sn, cs) = angle.sin_cos();
                let offset = vec2(half.x * cs - half.y * sn, half.x * sn + half.y * cs);
                let centre = r.center() + vec2(wm.offset[0] as f32 * s, -wm.offset[1] as f32 * s);
                p.add(egui::epaint::TextShape::new(centre - offset, galley, Color32::BLACK).with_angle(angle));
            }
            MarkKind::Background if file_name.is_some() => {
                p.rect_stroke(r.shrink(6.0), CornerRadius::ZERO, egui::Stroke::new(1.0, Color32::GRAY), egui::StrokeKind::Inside);
                p.text(
                    r.center(),
                    egui::Align2::CENTER_CENTER,
                    file_name.clone().unwrap_or_default(),
                    egui::FontId::proportional(10.0),
                    Color32::GRAY,
                );
            }
            // A colour is the page's tint.
            MarkKind::Background => {}
        });
        ui.vertical(|ui| {
            ui.horizontal(|ui| {
                // Steps through the pages the range selects.
                let (back, next) = (step_preview(&d.range, target, count, false), step_preview(&d.range, target, count, true));
                if ui.add_enabled_ui(back.is_some(), |ui| icons::button(ui, "chevron-left", 22.0, false, tl!("Previous page"))).inner.clicked() {
                    d.preview = back.or(d.preview);
                }
                ui.label(
                    egui::RichText::new(crate::i18n::fmt(
                        tl!("Preview: page {p} of {n}"),
                        &[("p", &(target + 1).to_string()), ("n", &count.to_string())],
                    ))
                    .small()
                    .color(t.text_faint),
                );
                if ui.add_enabled_ui(next.is_some(), |ui| icons::button(ui, "chevron-right", 22.0, false, tl!("Next page"))).inner.clicked() {
                    d.preview = next.or(d.preview);
                }
            });
            let n = d.range.pages(count).len();
            let applies =
                if n == 1 { tl!("Applies to 1 page").to_string() } else { crate::i18n::fmt(tl!("Applies to {n} pages"), &[("n", &n.to_string())]) };
            ui.label(egui::RichText::new(applies).small().color(t.text_faint));
            if d.replace {
                ui.label(egui::RichText::new(tl!("Replaces the existing one on those pages.")).small().color(t.text_faint));
            }
        });
    });
    // Text the standard fonts can't draw is refused on apply (#125), so it's refused here, with the
    // preview (drawn in the interface font, which can draw it) no longer the only answer (#2013).
    let undrawable = undrawable(kind, d);
    if let Some(c) = undrawable {
        ui.add_space(6.0);
        ui.label(egui::RichText::new(crate::content_ui::undrawable_message(c)).small().color(crate::content_ui::UNDRAWABLE));
    }
    ui.add_space(10.0);
    let (mut apply, mut cancel) = (false, false);
    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
        let ready = undrawable.is_none()
            && match kind {
                MarkKind::HeaderFooter => d.hf.text.iter().any(|x| !x.trim().is_empty()),
                MarkKind::Watermark => {
                    if d.use_file {
                        d.file.is_some()
                    } else {
                        !d.wm.text.trim().is_empty()
                    }
                }
                MarkKind::Background => !d.use_file || d.file.is_some(),
            }
            && !d.range.pages(count).is_empty();
        if ui.add_enabled_ui(ready, |ui| widgets::pill_button(ui, tl!("OK"), true)).inner.clicked() {
            apply = true;
        }
        if widgets::pill_button(ui, tl!("Cancel"), false).clicked() {
            cancel = true;
        }
    });
    // Browse…: the picker answers on a later frame, into the draft.
    #[cfg(not(target_arch = "wasm32"))]
    if browse {
        let dialog = rfd::AsyncFileDialog::new()
            .add_filter(tl!("PDF or image"), &["pdf", "png", "jpg", "jpeg", "tif", "tiff", "gif", "bmp", "jp2", "j2k", "jpx"]);
        let epoch = app.dialog_epoch();
        app.ask_one(crate::pickers::Ask::File(dialog), None, move |app, p| {
            // Only into the dialog that asked: not one closed meanwhile, or opened again since.
            if app.dialog_epoch() != epoch {
                return;
            }
            app.marks_draft.file =
                std::fs::read(&p).ok().map(|b| (p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default(), std::sync::Arc::new(b)));
        });
    }
    #[cfg(target_arch = "wasm32")]
    let _ = browse;
    (apply, cancel)
}

/// The first character of the dialog's text that applying would refuse (the standard fonts can't
/// draw it), as the engine decides it. Pictures draw no text.
fn undrawable(kind: MarkKind, d: &MarksDraft) -> Option<char> {
    match kind {
        MarkKind::HeaderFooter => d.hf.first_undrawable(),
        MarkKind::Watermark if !d.use_file => d.wm.first_undrawable(),
        MarkKind::Watermark | MarkKind::Background => None,
    }
}

fn mark_file(d: &MarksDraft) -> Option<pdfcraft_engine::MarkFile> {
    let (name, bytes) = d.file.clone().filter(|_| d.use_file)?;
    Some(pdfcraft_engine::MarkFile { name, bytes, page: d.file_page.max(1) - 1 })
}

/// Source ▸ File: Browse…, the page of a PDF, and the size relative to the page. Returns true
/// when Browse… was clicked (the caller shows the picker once it no longer borrows the draft).
fn file_source(ui: &mut egui::Ui, d: &mut MarksDraft, kind: MarkKind) -> bool {
    // Browse… is desktop-only.
    #[cfg_attr(target_arch = "wasm32", allow(unused_mut))]
    let mut browse = false;
    ui.horizontal(|ui| {
        #[cfg(not(target_arch = "wasm32"))]
        if ui.button(tl!("Browse…")).clicked() {
            browse = true;
        }
        ui.label(d.file.as_ref().map(|f| f.0.clone()).unwrap_or_else(|| tl!("No file chosen").to_string()));
        if d.file.as_ref().is_some_and(|f| f.1.starts_with(b"%PDF")) {
            ui.label(tl!("Page number"));
            ui.add(egui::DragValue::new(&mut d.file_page).range(1..=9999));
        }
    });
    ui.horizontal(|ui| {
        ui.label(tl!("Scale relative to target page"));
        let scale = if kind == MarkKind::Background { &mut d.bg.scale } else { &mut d.wm.scale };
        let mut pct = *scale * 100.0;
        if ui.add(egui::Slider::new(&mut pct, 5.0..=100.0).suffix("%")).changed() {
            *scale = pct / 100.0;
        }
    });
    browse
}

/// Pages that paint their own backdrop would hide the background: offer to replace a white one,
/// and say where a picture (a scan) will still hide it.
fn backdrop_options(ui: &mut egui::Ui, t: &Tokens, d: &mut MarksDraft, doc: &pdfcraft_engine::Document, count: usize) {
    let n = d.backdrops.update(doc, &d.range.pages(count));
    if n.pending > 0 {
        ui.label(egui::RichText::new(tl!("Checking pages…")).small().color(t.text_faint));
        ui.ctx().request_repaint();
    }
    if !n.fills.is_empty() {
        ui.add_space(4.0);
        let label = pages_phrase(
            &n.fills,
            tl!("Page {pages} paints its own white backdrop, which would hide the background. Replace it"),
            tl!("Pages {pages} paint their own white backdrop, which would hide the background. Replace it"),
        );
        ui.add(widgets::checkbox(&mut d.bg.replace_backdrop, &label));
        ui.label(egui::RichText::new(tl!("Removing the background puts the page's own backdrop back.")).small().color(t.text_faint));
    }
    if !n.images.is_empty() {
        ui.add_space(4.0);
        let text = pages_phrase(
            &n.images,
            tl!("A picture covers page {pages} (a scan?), so the background won't show there."),
            tl!("Pictures cover pages {pages} (scans?), so the background won't show there."),
        );
        ui.label(egui::RichText::new(text).small().color(crate::content_ui::UNDRAWABLE));
    }
}

/// The edit the dialog's OK makes.
pub fn edit(d: &MarksDraft, kind: MarkKind, count: usize) -> Edit {
    let pages = d.range.pages(count);
    match kind {
        MarkKind::HeaderFooter => Edit::AddHeaderFooter { pages, settings: d.hf.clone(), replace: d.replace },
        MarkKind::Watermark => {
            let mut wm = d.wm.clone();
            if d.fit {
                wm.font_size = 0.0;
            }
            Edit::AddWatermark { pages, settings: wm, replace: d.replace, file: mark_file(d) }
        }
        MarkKind::Background => Edit::AddBackground { pages, settings: d.bg.clone(), replace: d.replace, file: mark_file(d) },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn text_applying_would_refuse_is_caught_in_the_dialog() {
        let mut d = MarksDraft::default();
        d.hf.text[5] = "Страница <<1>> из <<n>> / Page <<1>> of <<n>>".into();
        assert_eq!(undrawable(MarkKind::HeaderFooter, &d), Some('С'), "#2013: refused before OK, not after");
        d.hf.text[5] = "Page <<1>> of <<n>> — café".into();
        assert_eq!(undrawable(MarkKind::HeaderFooter, &d), None);
        d.wm.text = "ΑΠΟΡΡΗΤΟ".into();
        assert_eq!(undrawable(MarkKind::Watermark, &d), Some('Α'));
        d.use_file = true;
        assert_eq!(undrawable(MarkKind::Watermark, &d), None, "a picture watermark draws no text");
        assert_eq!(undrawable(MarkKind::Background, &d), None);
    }

    #[test]
    fn backdrop_messages_name_their_pages() {
        let phrase = |pages: &[usize]| pages_phrase(pages, "Page {pages}", "Pages {pages}");
        assert_eq!(phrase(&[2]), "Page 3");
        assert_eq!(phrase(&[0, 1, 2, 5]), "Pages 1-3, 6");
        assert_eq!(phrase(&[0, 1]), "Pages 1-2", "a whole document is still listed");
        let every_other: Vec<usize> = (0..40).step_by(2).collect();
        assert_eq!(phrase(&every_other), "Pages 1, 3, 5, 7, 9, 11, 13, 15, …", "a long list stops");
        let runs: Vec<usize> = (0..LISTED_PAGE_RUNS * 2).step_by(2).collect();
        assert_eq!(phrase(&runs), "Pages 1, 3, 5, 7, 9, 11, 13, 15", "exactly the limit: no …");
    }

    #[test]
    fn page_ranges_with_subsets() {
        let r = PageRange::default();
        assert_eq!(r.pages(4), [0, 1, 2, 3]);
        let r = PageRange { all: false, from: 2, to: 9, subset: Subset::Even };
        assert_eq!(r.pages(6), [1, 3, 5]);
        let r = PageRange { all: true, subset: Subset::Odd, ..PageRange::default() };
        assert_eq!(r.pages(5), [0, 2, 4]);
        // Not all pages: by default up to the last one; a single page is a range too.
        let r = PageRange { all: false, ..PageRange::default() };
        assert_eq!(r.pages(4), [0, 1, 2, 3]);
        let r = PageRange { all: false, from: 1, to: 1, subset: Subset::All };
        assert_eq!(r.pages(38), [0]);
    }

    #[test]
    fn the_preview_shows_a_page_the_range_selects() {
        let all = PageRange::default();
        assert_eq!(preview_page(&all, 2, 5), 2, "the current page when the range selects it");
        let even = PageRange { all: true, subset: Subset::Even, ..PageRange::default() };
        assert_eq!(preview_page(&even, 0, 5), 1, "otherwise the first page the range selects");
        let later = PageRange { all: false, from: 3, to: 4, subset: Subset::All };
        assert_eq!(preview_page(&later, 0, 5), 2);
        let none = PageRange { all: false, from: 4, to: 2, subset: Subset::All };
        assert_eq!(preview_page(&none, 1, 5), 1, "the current page when the range selects none");
        assert_eq!(preview_page(&all, 0, 0), 0, "no pages at all");
    }

    #[test]
    fn the_preview_steps_through_the_pages_the_range_selects() {
        let odd = PageRange { all: true, subset: Subset::Odd, ..PageRange::default() };
        // Pages 1, 3 and 5 (0-based 0, 2, 4) of 5.
        assert_eq!(step_preview(&odd, 0, 5, true), Some(2));
        assert_eq!(step_preview(&odd, 2, 5, true), Some(4));
        assert_eq!(step_preview(&odd, 4, 5, true), None, "the last page has no next");
        assert_eq!(step_preview(&odd, 4, 5, false), Some(2));
        assert_eq!(step_preview(&odd, 0, 5, false), None, "the first page has no previous");
        // From a page the range leaves out, to the nearest one it selects on that side.
        assert_eq!(step_preview(&odd, 1, 5, true), Some(2));
        assert_eq!(step_preview(&odd, 1, 5, false), Some(0));
        assert_eq!(step_preview(&odd, 0, 0, true), None, "no pages at all");
        // A page stepped to is previewed while the range selects it, and the range's first page
        // once a new range leaves it out.
        assert_eq!(preview_page(&odd, 2, 5), 2);
        let later = PageRange { all: false, from: 4, to: 5, subset: Subset::All };
        assert_eq!(preview_page(&later, 2, 5), 3);
        // Narrowing the range past the page stepped to keeps the page shown instead, also once
        // the range is widened again.
        let mut stepped = Some(4);
        let first_two = PageRange { all: false, from: 1, to: 2, subset: Subset::All };
        assert_eq!(shown_preview(&first_two, &mut stepped, 1, 5), 0);
        assert_eq!(stepped, Some(0));
        assert_eq!(shown_preview(&PageRange::default(), &mut stepped, 1, 5), 0);
        // A page stepped to inside the range stays; with none stepped to, the current page shows.
        let mut stepped = Some(2);
        assert_eq!(shown_preview(&odd, &mut stepped, 0, 5), 2);
        assert_eq!(stepped, Some(2));
        let mut stepped = None;
        assert_eq!(shown_preview(&odd, &mut stepped, 4, 5), 4);
        assert_eq!(stepped, None);
    }

    #[test]
    fn a_background_tints_the_page_it_is_behind() {
        assert_eq!(behind([1.0, 0.0, 0.0], 1.0), Color32::from_rgb(255, 0, 0));
        assert_eq!(behind([1.0, 0.0, 0.0], 0.0), Color32::WHITE);
        assert_eq!(behind([0.0, 0.0, 0.0], 0.5), Color32::from_rgb(128, 128, 128));
        assert_eq!(behind([0.2, 0.4, 0.6], f64::NAN), behind([0.2, 0.4, 0.6], 1.0), "opaque, as the engine has it");
    }

    #[test]
    fn the_preview_shows_a_background_only_where_ok_lets_it_show() {
        let mut d = MarksDraft::default();
        let colour = behind(d.bg.color, d.bg.opacity);
        assert_ne!(colour, Color32::WHITE);
        assert_eq!(background_tint(&d, None), colour, "no backdrop of its own");
        assert_eq!(background_tint(&d, Some(Backdrop::Fill)), colour, "its white backdrop is replaced");
        assert_eq!(background_tint(&d, Some(Backdrop::Image)), Color32::WHITE, "a picture covers it");
        d.bg.replace_backdrop = false;
        assert_eq!(background_tint(&d, Some(Backdrop::Fill)), Color32::WHITE, "its white backdrop stays and hides it");
        assert_eq!(background_tint(&d, None), colour, "unticked changes nothing without a backdrop");
    }

    #[test]
    fn the_watermark_edit_carries_the_offset() {
        let mut d = MarksDraft::default();
        d.wm.text = "DRAFT".into();
        d.wm.offset = [-72.0, 36.0];
        match edit(&d, MarkKind::Watermark, 3) {
            Edit::AddWatermark { settings, .. } => assert_eq!(settings.offset, [-72.0, 36.0]),
            other => panic!("expected AddWatermark, got {other:?}"),
        }
    }
}
