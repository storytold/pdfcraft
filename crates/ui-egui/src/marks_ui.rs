//! Add Header and Footer, Add Watermark and Add Background dialogs (Acrobat: Edit a PDF ▸ …;
//! dialog snapshots d13, d14, d12d). Each has the shared Page Range Options and a preview of a
//! page the range selects, with the marks laid out on it. "Update" opens the same dialog in
//! replace mode.

use std::sync::Arc;

use egui::emath::Rot2;
use egui::{Align, Color32, CornerRadius, Layout, Pos2, Rect, Stroke, pos2, vec2};
use pdfcraft_engine::{Background, Edit, FontFamily, HeaderFooter, MarkKind, Watermark};

use crate::theme::{self, Tokens};
use crate::{PdfCraftApp, widgets};

/// How far a watermark may be moved from the page centre, in points.
const MAX_OFFSET: f64 = 20_000.0;
/// Pixels along the longer side of Source ▸ File's picture as rendered for the preview, whose page
/// is at most 190 points across: sharp on a screen of more than twice that density.
const PICTURE_SIDE: u32 = 512;
/// A page with a picture behind it is drawn as a grid of cells this many screen points (logical
/// pixels) across, each corner coloured by the picture.
const CELL: f32 = 2.0;

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
            egui::ComboBox::from_id_salt("mark-subset").selected_text(label).show_ui(ui, |ui| {
                ui.selectable_value(&mut self.subset, Subset::All, tl!("All pages in range"));
                ui.selectable_value(&mut self.subset, Subset::Even, tl!("Even pages only"));
                ui.selectable_value(&mut self.subset, Subset::Odd, tl!("Odd pages only"));
            });
        });
    }
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
    /// The file's picture for the preview.
    pub picture: FilePicture,
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
            picture: FilePicture::default(),
        }
    }
}

/// Source ▸ File's picture as the engine rendered it, checked, with its summed-area table: made
/// on the worker thread, uploaded on the UI thread.
struct Rendered {
    picture: pdfcraft_engine::MarkPicture,
    pixels: [usize; 2],
    sums: Vec<[u32; 4]>,
}

impl Rendered {
    /// A raster that doesn't add up, or is larger than asked for, is refused, never uploaded.
    fn new(picture: pdfcraft_engine::MarkPicture) -> Result<Self, String> {
        let [w, h] = [picture.width, picture.height].map(|v| v as usize);
        // A pixel over for rounding.
        let side = PICTURE_SIDE as usize + 1;
        if w == 0 || h == 0 || w > side || h > side {
            return Err(format!("the picture came back {w}×{h} pixels"));
        }
        if w.checked_mul(h).and_then(|n| n.checked_mul(4)) != Some(picture.rgba.len()) {
            return Err("the picture came back incomplete".into());
        }
        let sums = summed(&picture.rgba, w);
        Ok(Self { picture, pixels: [w, h], sums })
    }
}

/// The summed-area table of premultiplied RGBA pixels `w` wide (at least 1): entry
/// `y × (w + 1) + x` holds the channel sums of the pixels above and left of (x, y), so the mean
/// of any box takes four look-ups whatever its size.
fn summed(rgba: &[u8], w: usize) -> Vec<[u32; 4]> {
    let stride = w.saturating_mul(4).max(4);
    let mut table = Vec::with_capacity((w + 1).saturating_mul(rgba.len() / stride + 1));
    table.resize(w + 1, [0u32; 4]);
    for (y, row) in rgba.chunks_exact(stride).enumerate() {
        table.push([0; 4]);
        let mut run = [0u32; 4];
        for (x, px) in row.as_chunks::<4>().0.iter().enumerate() {
            for (r, &c) in run.iter_mut().zip(px) {
                *r = r.saturating_add(u32::from(c));
            }
            let above = table.get(y * (w + 1) + x + 1).copied().unwrap_or_default();
            table.push(std::array::from_fn(|k| above[k].saturating_add(run[k])));
        }
    }
    table
}

/// Source ▸ File's picture, uploaded: a texture, and its sums for drawing it behind the page.
#[derive(Clone)]
struct Picture {
    tex: egui::TextureHandle,
    /// Its size in pixels, and its summed-area table ([`summed`]).
    pixels: [usize; 2],
    sums: Arc<Vec<[u32; 4]>>,
    /// Its size in points, which OK fits to the page.
    size: (f64, f64),
}

impl Picture {
    fn upload(ctx: &egui::Context, r: Rendered) -> Self {
        let image = crate::canvas::texture_image(r.pixels, r.picture.rgba);
        let tex = ctx.load_texture("mark-file-picture", image, egui::TextureOptions::LINEAR);
        Self { tex, pixels: r.pixels, sums: Arc::new(r.sums), size: r.picture.size }
    }

    /// Its mean colour over a box ([`box_mean`]).
    fn mean(&self, u: [f32; 2], v: [f32; 2]) -> Color32 {
        box_mean(&self.sums, self.pixels, u, v)
    }
}

/// The mean colour (premultiplied) of a picture `pixels` in size, from its summed-area table
/// `sums` ([`summed`]), over the box `u` × `v` (from and to, 0–1 across and down). What of the
/// box lies outside the picture is transparent.
fn box_mean(sums: &[[u32; 4]], pixels: [usize; 2], u: [f32; 2], v: [f32; 2]) -> Color32 {
    let [w, h] = pixels;
    // The box's edges in whole pixels, at least one pixel across.
    let edges = |f: [f32; 2], n: usize| {
        let from = (f[0] * n as f32).round() as i64;
        (from, ((f[1] * n as f32).round() as i64).max(from.saturating_add(1)))
    };
    let ((x0, x1), (y0, y1)) = (edges(u, w), edges(v, h));
    let area = x1.saturating_sub(x0).saturating_mul(y1.saturating_sub(y0)).max(1).unsigned_abs();
    let clamp = |e: i64, n: usize| e.clamp(0, n as i64) as usize;
    let at = |x: i64, y: i64| sums.get(clamp(y, h) * (w + 1) + clamp(x, w)).copied().unwrap_or_default();
    let (a, b, c, d) = (at(x0, y0), at(x1, y0), at(x0, y1), at(x1, y1));
    let channel = |k: usize| {
        let sum = (u64::from(d[k]) + u64::from(a[k])).saturating_sub(u64::from(b[k]) + u64::from(c[k]));
        ((sum + area / 2) / area).min(255) as u8
    };
    Color32::from_rgba_premultiplied(channel(0), channel(1), channel(2), channel(3))
}

/// The file and page (0-based) a picture is rendered for.
#[derive(Clone)]
struct PictureKey {
    bytes: Arc<Vec<u8>>,
    page: usize,
}

impl PictureKey {
    fn is(&self, other: &PictureKey) -> bool {
        Arc::ptr_eq(&self.bytes, &other.bytes) && self.page == other.page
    }
}

type PictureAnswer = Result<Rendered, String>;

/// Source ▸ File's picture for the preview, kept for the file and page it shows. It is rendered
/// off the UI thread, one at a time, as OK brings it in ([`pdfcraft_engine::mark_picture`]);
/// until it arrives, or when the file can't be drawn, the preview shows a frame with its name.
#[derive(Default)]
pub struct FilePicture {
    /// The picture of a file and page, or why there is none.
    shown: Option<(PictureKey, Result<Picture, String>)>,
    /// The render under way.
    running: Option<(PictureKey, std::sync::mpsc::Receiver<PictureAnswer>)>,
}

impl FilePicture {
    /// The picture of `page` (0-based) of `file`, once rendered. Asking for another file or page
    /// starts its render, once the one under way is done.
    fn get(&mut self, ctx: &egui::Context, file: Option<&(String, Arc<Vec<u8>>)>, page: usize) -> Option<&Picture> {
        let (name, bytes) = file?;
        let want = PictureKey { bytes: bytes.clone(), page };
        if let Some((key, rx)) = &self.running {
            let answer = match rx.try_recv() {
                Ok(answer) => Some(answer),
                Err(std::sync::mpsc::TryRecvError::Empty) => None,
                Err(std::sync::mpsc::TryRecvError::Disconnected) => Some(Err("the picture couldn't be drawn".into())),
            };
            if let Some(answer) = answer {
                let key = key.clone();
                self.running = None;
                self.shown = Some((key, answer.map(|r| Picture::upload(ctx, r))));
            }
        }
        if self.running.is_none() && !self.shown.as_ref().is_some_and(|(k, _)| k.is(&want)) {
            self.start(ctx, name, want.clone());
        }
        match &self.shown {
            Some((k, Ok(p))) if k.is(&want) => Some(p),
            _ => None,
        }
    }

    /// Render `key` on a worker thread, or here where there are none (the web).
    fn start(&mut self, ctx: &egui::Context, name: &str, key: PictureKey) {
        let file = pdfcraft_engine::MarkFile { name: name.to_owned(), bytes: key.bytes.clone(), page: key.page };
        if !cfg!(target_arch = "wasm32") {
            let (tx, rx) = std::sync::mpsc::channel();
            let (job, repaint) = (file.clone(), ctx.clone());
            let spawned = std::thread::Builder::new().name("mark-picture".into()).spawn(move || {
                // The receiver may be gone (the draft was replaced): nothing to report.
                let _ = tx.send(pdfcraft_engine::mark_picture(&job, PICTURE_SIDE).and_then(Rendered::new));
                repaint.request_repaint();
            });
            if spawned.is_ok() {
                self.running = Some((key, rx));
                return;
            }
        }
        let answer = pdfcraft_engine::mark_picture(&file, PICTURE_SIDE).and_then(Rendered::new).map(|r| Picture::upload(ctx, r));
        self.shown = Some((key, answer));
    }
}

/// A cache: a copy shares the picture shown, not the render under way.
impl Clone for FilePicture {
    fn clone(&self) -> Self {
        Self { shown: self.shown.clone(), running: None }
    }
}

impl std::fmt::Debug for FilePicture {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FilePicture").field("shown", &self.shown.is_some()).field("running", &self.running.is_some()).finish()
    }
}

/// A cache: drafts that say the same are equal whatever they have rendered.
impl PartialEq for FilePicture {
    fn eq(&self, _: &Self) -> bool {
        true
    }
}

/// A picture mark as OK draws it: its scale relative to the page, its turn (degrees
/// counter-clockwise), its offset from the page centre (points, right and up) and its opacity.
#[derive(Clone, Copy)]
struct PictureMark<'a> {
    picture: &'a Picture,
    scale: f64,
    rotation: f64,
    offset: [f64; 2],
    opacity: f64,
}

impl<'a> PictureMark<'a> {
    /// Laid out on the page drawn as `page`, at `s` pixels a point, where `picture` in
    /// `pdfcraft-edit` puts it: fitted to the page at its scale, centred plus its offset, turned.
    fn place(self, page: Rect, s: f32) -> Placed<'a> {
        let finite = |v: f64| if v.is_finite() { v } else { 0.0 };
        let (w, h) = (f64::from(page.width() / s), f64::from(page.height() / s));
        let (sw, sh) = (self.picture.size.0.max(0.01), self.picture.size.1.max(0.01));
        let k = (w / sw).min(h / sh) * if self.scale.is_finite() && self.scale > 0.0 { self.scale } else { 1.0 };
        let centre = page.center() + vec2(finite(self.offset[0]) as f32 * s, -finite(self.offset[1]) as f32 * s);
        Placed {
            picture: self.picture,
            rect: Rect::from_center_size(centre, vec2((sw * k) as f32 * s, (sh * k) as f32 * s)),
            // Counter-clockwise on the page is clockwise-negative on screen.
            turn: Rot2::from_angle(-(finite(self.rotation) as f32).to_radians()),
            opacity: if self.opacity.is_finite() { self.opacity.clamp(0.0, 1.0) as f32 } else { 1.0 },
        }
    }
}

/// A picture laid out on the preview's page: its rectangle before the turn, turned about its centre.
struct Placed<'a> {
    picture: &'a Picture,
    rect: Rect,
    turn: Rot2,
    opacity: f32,
}

impl Placed<'_> {
    /// The picture drawn over the page.
    fn mesh(&self) -> egui::Mesh {
        let mut mesh = egui::Mesh::with_texture(self.picture.tex.id());
        mesh.add_rect_with_uv(self.rect, Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)), Color32::WHITE.gamma_multiply(self.opacity));
        mesh.rotate(self.turn, self.rect.center());
        mesh
    }

    /// What shows at `pos`, a corner of the page's grid, where the page is white with the
    /// picture behind it: the picture over white paper, or the paper where the picture isn't.
    /// The picture is averaged over the grid cell around `pos`, so detail finer than a cell is
    /// blended, not picked at random.
    fn behind(&self, pos: Pos2) -> Color32 {
        let local = self.turn.inverse() * (pos - self.rect.center());
        let (u, v) = (local.x / self.rect.width() + 0.5, local.y / self.rect.height() + 0.5);
        let (du, dv) = (CELL / 2.0 / self.rect.width(), CELL / 2.0 / self.rect.height());
        if ![u, v, du, dv].iter().all(|f| f.is_finite()) || u + du < 0.0 || u - du > 1.0 || v + dv < 0.0 || v - dv > 1.0 {
            return Color32::WHITE;
        }
        let c = self.picture.mean([u - du, u + du], [v - dv, v + dv]).gamma_multiply(self.opacity);
        let paper = 255 - c.a();
        Color32::from_rgb(c.r().saturating_add(paper), c.g().saturating_add(paper), c.b().saturating_add(paper))
    }
}

/// What shows where the page is white, behind its content: a colour (white for nothing), or a
/// picture.
enum Behind<'a> {
    Colour(Color32),
    Picture(PictureMark<'a>),
}

/// The page drawn as `page` from its picture `tex` (a white sheet without one) as a grid of
/// cells, each corner tinted by `tint`: a mesh's colours multiply its texture, which is how what
/// is behind the page's content shows where the page is white.
fn tinted_page(page: Rect, tex: Option<egui::TextureId>, tint: impl Fn(Pos2) -> Color32) -> egui::Mesh {
    let cells = |len: f32| (len / CELL).ceil().clamp(1.0, 512.0) as u32;
    let (cols, rows) = (cells(page.width()), cells(page.height()));
    let mut mesh = egui::Mesh::with_texture(tex.unwrap_or_default());
    for j in 0..=rows {
        for i in 0..=cols {
            let f = vec2(i as f32 / cols as f32, j as f32 / rows as f32);
            let pos = page.min + f * page.size();
            let uv = if tex.is_some() { pos2(f.x, f.y) } else { egui::epaint::WHITE_UV };
            mesh.vertices.push(egui::epaint::Vertex { pos, uv, color: tint(pos) });
        }
    }
    for j in 0..rows {
        for i in 0..cols {
            let a = j * (cols + 1) + i;
            let c = a + cols + 1;
            mesh.add_triangle(a, a + 1, c);
            mesh.add_triangle(a + 1, c + 1, c);
        }
    }
    mesh
}

const PAGE_FORMATS: [&str; 5] = ["<<1>>", "<<1 of n>>", "<<1/n>>", "<<Page 1>>", "<<Page 1 of n>>"];
// New formats go last: `date_format` is an index into this list.
const DATE_FORMATS: [&str; 8] =
    ["<<m/d/yyyy>>", "<<mm/dd/yyyy>>", "<<d/m/yyyy>>", "<<yyyy-mm-dd>>", "<<m/d/yy>>", "<<mmmm d, yyyy>>", "<<yyyy/mm/dd>>", "<<yyyy/m/d>>"];
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
/// What is `behind` the page multiplies it, which is how it looks behind the page's content.
fn preview(
    ui: &mut egui::Ui,
    t: &Tokens,
    size: (f64, f64),
    picture: Option<egui::TextureId>,
    behind: Behind<'_>,
    draw: impl FnOnce(&egui::Painter, Rect, f32),
) {
    let (w, h) = (size.0.max(1.0), size.1.max(1.0));
    let s = (190.0 / w.max(h)) as f32;
    let (r, _) = ui.allocate_exact_size(vec2(210.0, 210.0), egui::Sense::hover());
    let page = Rect::from_center_size(r.center(), vec2(w as f32 * s, h as f32 * s));
    let painter = ui.painter_at(r);
    match (behind, picture) {
        (Behind::Colour(tint), Some(tex)) => {
            painter.image(tex, page, Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)), tint);
        }
        (Behind::Colour(tint), None) => {
            painter.rect_filled(page, CornerRadius::ZERO, tint);
        }
        (Behind::Picture(mark), picture) => {
            let placed = mark.place(page, s);
            painter.add(tinted_page(page, picture, |pos| placed.behind(pos)));
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

/// The page (0-based) the preview shows: the current one when the range selects it, otherwise the
/// first page the range selects (the current one again when it selects none).
fn preview_page(range: &PageRange, current: usize, count: usize) -> usize {
    let pages = range.pages(count);
    if pages.contains(&current) { current } else { pages.first().copied().unwrap_or(current) }
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
                egui::ComboBox::from_id_salt("hf-font").selected_text(d.hf.font.label()).width(100.0).show_ui(ui, |ui| {
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
                egui::ComboBox::from_id_salt("hf-page-format").selected_text(PAGE_FORMATS[d.page_format].trim_matches(['<', '>'])).show_ui(
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
                egui::ComboBox::from_id_salt("hf-date-format").selected_text(DATE_FORMATS[d.date_format].trim_matches(['<', '>'])).show_ui(
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
        }
    }
    ui.add_space(8.0);
    ui.label(egui::RichText::new(tl!("Page Range Options")).font(theme::semibold(12.5)));
    d.range.ui(ui, count);
    ui.add_space(8.0);
    // The preview: the current page, or the first one the range selects if it leaves that out,
    // drawn from its thumbnail at its displayed size (after /Rotate), the frame the engine lays
    // the marks out in.
    let target = preview_page(&d.range, current, count);
    let page = doc.info.pages.get(target).map(|p| (p.width as f64, p.height as f64)).unwrap_or((612.0, 792.0));
    let picture = app.views.get_mut(i).and_then(|view| {
        view.need_thumbnail(target, true);
        view.thumb_id(target)
    });
    let tint = if kind == MarkKind::Background && !d.use_file { behind(d.bg.color, d.bg.opacity) } else { Color32::WHITE };
    let hf = d.hf.clone();
    let size_h = page.1.max(1.0);
    let (wm, fit) = (d.wm.clone(), d.fit);
    let file_name = d.use_file.then(|| d.file.as_ref().map(|f| f.0.clone())).flatten();
    // Source ▸ File: the file's picture, as OK lays it out. A background, or a watermark that
    // appears behind the page, shows where the page is white; a watermark on top covers it.
    let rendered = match kind {
        MarkKind::Watermark | MarkKind::Background if d.use_file => d.picture.get(ui.ctx(), d.file.as_ref(), d.file_page.saturating_sub(1)),
        _ => None,
    };
    let mark = rendered.map(|picture| match kind {
        MarkKind::Background => PictureMark { picture, scale: d.bg.scale, rotation: 0.0, offset: [0.0; 2], opacity: d.bg.opacity },
        _ => PictureMark { picture, scale: wm.scale, rotation: wm.rotation, offset: wm.offset, opacity: wm.opacity },
    });
    let under = kind == MarkKind::Background || wm.behind;
    let page_behind = match mark {
        Some(mark) if under => Behind::Picture(mark),
        _ => Behind::Colour(tint),
    };
    ui.horizontal(|ui| {
        preview(ui, t, page, picture, page_behind, |p, r, s| match kind {
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
            // The picture behind the page is drawn with the page.
            MarkKind::Watermark | MarkKind::Background if mark.is_some() => {
                if let Some(mark) = mark.filter(|_| !under) {
                    p.add(mark.place(r, s).mesh());
                }
            }
            // Until the picture arrives, or when the file can't be drawn: a frame with its name.
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
            ui.label(
                egui::RichText::new(crate::i18n::fmt(
                    tl!("Preview: page {p} of {n}"),
                    &[("p", &(target + 1).to_string()), ("n", &count.to_string())],
                ))
                .small()
                .color(t.text_faint),
            );
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
    fn a_background_tints_the_page_it_is_behind() {
        assert_eq!(behind([1.0, 0.0, 0.0], 1.0), Color32::from_rgb(255, 0, 0));
        assert_eq!(behind([1.0, 0.0, 0.0], 0.0), Color32::WHITE);
        assert_eq!(behind([0.0, 0.0, 0.0], 0.5), Color32::from_rgb(128, 128, 128));
        assert_eq!(behind([0.2, 0.4, 0.6], f64::NAN), behind([0.2, 0.4, 0.6], 1.0), "opaque, as the engine has it");
    }

    #[test]
    fn a_picture_behind_the_page_is_averaged_over_each_cell() {
        // 2 × 2: opaque red, opaque blue on top; transparent, opaque white below.
        let rgba = [255, 0, 0, 255, 0, 0, 255, 255, 0, 0, 0, 0, 255, 255, 255, 255];
        let sums = summed(&rgba, 2);
        assert_eq!(sums.len(), 9);
        let mean = |u: [f32; 2], v: [f32; 2]| box_mean(&sums, [2, 2], u, v);
        assert_eq!(mean([0.0, 0.5], [0.0, 0.5]), Color32::from_rgb(255, 0, 0), "one pixel");
        assert_eq!(mean([0.5, 1.0], [0.5, 1.0]), Color32::WHITE);
        assert_eq!(mean([0.0, 1.0], [0.0, 0.5]), Color32::from_rgba_premultiplied(128, 0, 128, 255), "red and blue");
        assert_eq!(mean([0.0, 1.0], [0.0, 1.0]), Color32::from_rgba_premultiplied(128, 64, 128, 191), "all four");
        assert_eq!(mean([-1.0, 0.0], [0.0, 1.0]), Color32::TRANSPARENT, "outside the picture");
        assert_eq!(mean([-0.5, 0.5], [0.0, 0.5]), Color32::from_rgba_premultiplied(128, 0, 0, 128), "half outside");
        // Edges that aren't numbers, or are far off, neither panic nor overflow.
        mean([f32::NAN, f32::INFINITY], [f32::NEG_INFINITY, 1e30]);
        assert_eq!(box_mean(&[], [0, 0], [0.0, 1.0], [0.0, 1.0]), Color32::TRANSPARENT, "no pixels");
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
