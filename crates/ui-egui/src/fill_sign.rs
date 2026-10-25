//! Fill & Sign (Acrobat's Fill & Sign tool, execution plan M5.7): type text onto the page, place
//! ✓ ✕ ● ─ marks and today's date (in Preferences ▸ Date format), and sign with a drawn
//! signature. Everything is an annotation (typewriter text, PdfCraft-drawn stamps, ink), so it
//! can be moved, deleted and undone like any comment.

use egui::{Color32, CornerRadius, Pos2, Sense, Stroke, pos2, vec2};
use pdfcraft_engine::signature_scan::{DEFAULT_SENSITIVITY, PREVIEW_MAX_SIDE, SCAN_MAX_SIDE, ScanError};
use pdfcraft_engine::{Edit, FillMark, NewAnnotation, PhotoRect, Shape, SignatureImage, SignaturePhoto, Style, Turn};
use pdfcraft_render::{DocInfo, PageInfo};

use crate::canvas::{DocView, PageXform};
use crate::page_pointer::PagePointer;
use crate::theme::Tokens;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum FillTool {
    Text,
    Check,
    Cross,
    Dot,
    Line,
    Date,
    Signature,
    Initials,
}

pub const FILL_TOOLS: [FillTool; 8] =
    [FillTool::Text, FillTool::Cross, FillTool::Check, FillTool::Dot, FillTool::Line, FillTool::Date, FillTool::Signature, FillTool::Initials];

/// A saved signature or initials: drawn strokes (normalised to the pad width, y up) or typed
/// text (drawn in the script font), or a locally imported image.
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum SavedSig {
    Drawn(Vec<Vec<[f32; 2]>>),
    Typed(String),
    Image(#[serde(with = "image_data")] SignatureImage),
}

impl SavedSig {
    /// This signature as the graphic of a visible digital signature. `None` for typed text the
    /// script font has nothing to draw for.
    pub fn graphic(&self) -> Option<pdfcraft_engine::sign::Graphic> {
        match self {
            SavedSig::Drawn(strokes) => Some(pdfcraft_engine::sign::Graphic::Strokes(
                strokes.iter().map(|s| s.iter().map(|p| [f64::from(p[0]), f64::from(p[1])]).collect()).collect(),
            )),
            SavedSig::Typed(text) => pdfcraft_engine::typed_signature_graphic(text),
            SavedSig::Image(image) => Some(image.graphic()),
        }
    }
}

pub(crate) mod image_data {
    use base64::Engine as _;
    use pdfcraft_engine::{SignatureImage, signature_image::MAX_SIGNATURE_IMAGE_BYTES};
    use serde::{Deserialize, Deserializer, Serializer};

    pub fn serialize<S: Serializer>(image: &SignatureImage, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&base64::engine::general_purpose::STANDARD.encode(image.bytes().as_slice()))
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<SignatureImage, D::Error> {
        let encoded = String::deserialize(d)?;
        if encoded.len() > MAX_SIGNATURE_IMAGE_BYTES.div_ceil(3) * 4 {
            return Err(serde::de::Error::custom("signature image exceeds 4 MiB"));
        }
        let bytes = base64::engine::general_purpose::STANDARD.decode(encoded).map_err(serde::de::Error::custom)?;
        SignatureImage::from_bytes(&bytes).map_err(serde::de::Error::custom)
    }
}

/// The Create signature / initials dialog.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct SigDraft {
    pub strokes: Vec<Vec<[f32; 2]>>,
    pub text: String,
    /// The Draw tab (otherwise Type).
    pub drawing: bool,
    /// The Image tab. The file is held in the draft until Apply.
    pub image_mode: bool,
    /// The Image or Scan tab's result.
    pub image: Option<SignatureImage>,
    /// The Scan tab: a photo of a signature on paper, cleaned into `image`.
    pub scan_mode: bool,
    pub scan: Option<ScanDraft>,
    /// Creating initials (otherwise the signature).
    pub initials: bool,
    /// Replacing an existing saved value.
    pub editing: bool,
}

impl SigDraft {
    pub fn new(initials: bool, name: &str) -> Self {
        let text = if initials { name.split_whitespace().filter_map(|w| w.chars().next()).collect() } else { name.trim().to_string() };
        Self { text, initials, ..Default::default() }
    }

    /// Edit a copy of the saved value; Cancel must leave the original untouched.
    pub fn from_saved(initials: bool, saved: &SavedSig) -> Self {
        match saved {
            SavedSig::Drawn(strokes) => Self { strokes: strokes.clone(), drawing: true, initials, editing: true, ..Default::default() },
            SavedSig::Typed(text) => Self { text: text.clone(), initials, editing: true, ..Default::default() },
            SavedSig::Image(image) => Self { image: Some(image.clone()), image_mode: true, initials, editing: true, ..Default::default() },
        }
    }

    /// Take back the last drawn stroke (and any dot-less empty one a bare click left behind it).
    /// Returns whether a visible stroke was removed.
    pub fn undo_stroke(&mut self) -> bool {
        while let Some(s) = self.strokes.pop() {
            if s.len() > 1 {
                return true;
            }
        }
        false
    }

    /// The Draw tab is showing (not Type, Image or Scan).
    fn draw_tab(&self) -> bool {
        self.drawing && !self.image_mode && !self.scan_mode
    }

    fn ready(&self) -> bool {
        if self.scan_mode {
            // A preview made while the frame or slider is still moving is not the result.
            self.scan.as_ref().is_some_and(|s| s.drag.is_none() && s.full && !s.scanning()) && self.image.is_some()
        } else if self.image_mode {
            self.image.is_some()
        } else if self.drawing {
            self.strokes.iter().any(|s| s.len() > 1)
        } else {
            !self.text.trim().is_empty()
                && self.text.chars().take(pdfcraft_engine::MAX_SIGNATURE_CHARS + 1).count() <= pdfcraft_engine::MAX_SIGNATURE_CHARS
        }
    }

    pub fn saved(&self) -> Option<SavedSig> {
        if self.scan_mode {
            self.ready().then(|| self.image.clone().map(SavedSig::Image)).flatten()
        } else if self.image_mode {
            self.image.clone().map(SavedSig::Image)
        } else if self.drawing {
            Some(SavedSig::Drawn(self.strokes.clone()))
        } else {
            Some(SavedSig::Typed(self.text.trim().to_string()))
        }
    }
}

/// The Scan tab's photo, the frame around the signature and how strongly faint ink is picked up.
#[derive(Clone, Debug, PartialEq)]
pub struct ScanDraft {
    pub photo: SignaturePhoto,
    pub frame: PhotoRect,
    pub sensitivity: u8,
    /// Changes whenever the photo's pixels do (a new photo or a turn), to refresh its texture.
    version: u64,
    drag: Option<FrameDrag>,
    /// The draft's image is the full-resolution result, not a quick preview.
    full: bool,
    /// Why the frame gives no signature, shown in place of the preview.
    pub problem: Option<String>,
    /// Scans run here while the frame or slider moves, so a frame never waits for one.
    worker: ScanWorker,
}

/// What one scan cleans: the frame and sensitivity at the time it was asked for.
#[derive(Clone, Copy, Debug, PartialEq)]
struct ScanRequest {
    frame: PhotoRect,
    sensitivity: u8,
    full: bool,
}

type ScanAnswer = Result<Result<SignatureImage, ScanError>, String>;

/// The scan running on a worker thread (desktop) and the newest request made meanwhile, which
/// starts when it answers; requests in between are skipped. It is not part of the draft's value:
/// a clone has nothing running and comparisons ignore it.
#[derive(Default)]
struct ScanWorker {
    running: Option<(ScanRequest, std::sync::mpsc::Receiver<ScanAnswer>)>,
    queued: Option<ScanRequest>,
}

impl Clone for ScanWorker {
    fn clone(&self) -> Self {
        Self::default()
    }
}

impl PartialEq for ScanWorker {
    fn eq(&self, _: &Self) -> bool {
        true
    }
}

impl std::fmt::Debug for ScanWorker {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ScanWorker").field("running", &self.running.as_ref().map(|(r, _)| r)).field("queued", &self.queued).finish()
    }
}

/// A drag on the photo, in photo pixels.
#[derive(Clone, Copy, Debug, PartialEq)]
enum FrameDrag {
    /// Moving the frame, grabbed at this offset from its top-left corner.
    Move([f32; 2]),
    /// Drawing a new frame or resizing from a corner, with the opposite corner fixed here.
    Span([f32; 2]),
}

fn next_photo_version() -> u64 {
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
    NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
}

impl ScanDraft {
    /// Frame the signature (turning a sideways one upright) with the default sensitivity.
    pub fn new(mut photo: SignaturePhoto) -> Self {
        let frame = photo.auto_frame();
        Self {
            photo,
            frame,
            sensitivity: DEFAULT_SENSITIVITY,
            version: next_photo_version(),
            drag: None,
            full: false,
            problem: None,
            worker: ScanWorker::default(),
        }
    }

    fn request(&self, full: bool) -> ScanRequest {
        ScanRequest { frame: self.frame, sensitivity: self.sensitivity, full }
    }

    fn run(photo: &SignaturePhoto, req: ScanRequest) -> ScanAnswer {
        let max_side = if req.full { SCAN_MAX_SIDE } else { PREVIEW_MAX_SIDE };
        pdfcraft_engine::guard(|| photo.scan(req.frame, req.sensitivity, max_side))
    }

    /// Clean the framed area into `image` now: quickly (`full` false) while something is being
    /// dragged, at full resolution otherwise. A scan still running in the background is dropped.
    pub fn rescan(&mut self, image: &mut Option<SignatureImage>, full: bool) {
        self.worker = ScanWorker::default();
        let req = self.request(full);
        let answer = Self::run(&self.photo, req);
        self.apply(image, req, answer);
    }

    /// Like [`Self::rescan`], but on a worker thread on desktop, so the frame that asked doesn't
    /// wait (a scan of a large frame takes a noticeable fraction of a second). The draft isn't
    /// ready until the answer for the current frame and sensitivity arrives; [`Self::poll`]
    /// picks it up. Where threads are unavailable (the web) it scans at once.
    pub fn rescan_in_background(&mut self, image: &mut Option<SignatureImage>, full: bool, ctx: Option<&egui::Context>) {
        if cfg!(target_arch = "wasm32") {
            return self.rescan(image, full);
        }
        let req = self.request(full);
        self.full = false;
        if self.worker.running.is_some() {
            self.worker.queued = Some(req);
        } else if !self.start(req, ctx) {
            self.rescan(image, full);
        }
    }

    /// Start `req` on a worker thread; false if no thread could be started.
    fn start(&mut self, req: ScanRequest, ctx: Option<&egui::Context>) -> bool {
        let photo = self.photo.clone();
        let ctx = ctx.cloned();
        let (tx, rx) = std::sync::mpsc::channel();
        let spawned = std::thread::Builder::new().name("signature-scan".into()).spawn(move || {
            // The receiver may be gone (the dialog closed or a newer photo): nothing to report.
            let _ = tx.send(Self::run(&photo, req));
            if let Some(ctx) = ctx {
                ctx.request_repaint();
            }
        });
        if spawned.is_err() {
            return false;
        }
        self.worker.running = Some((req, rx));
        true
    }

    /// Whether a background scan is running or waiting to run.
    pub fn scanning(&self) -> bool {
        self.worker.running.is_some() || self.worker.queued.is_some()
    }

    /// Pick up a finished background scan (each frame), and start the newest waiting request.
    pub fn poll(&mut self, image: &mut Option<SignatureImage>, ctx: Option<&egui::Context>) {
        let Some((req, rx)) = &self.worker.running else { return };
        let answer = match rx.try_recv() {
            Ok(answer) => answer,
            Err(std::sync::mpsc::TryRecvError::Empty) => return,
            Err(std::sync::mpsc::TryRecvError::Disconnected) => Err("the scan stopped unexpectedly".into()),
        };
        let req = *req;
        self.worker.running = None;
        match self.worker.queued.take() {
            Some(next) => {
                // Shown as a preview while the newer request runs.
                self.apply(image, ScanRequest { full: false, ..req }, answer);
                if !self.start(next, ctx) {
                    self.rescan(image, next.full);
                }
            }
            None => self.apply(image, req, answer),
        }
    }

    /// Show a scan's answer. It is the result only if it was full and for the current frame and
    /// sensitivity.
    fn apply(&mut self, image: &mut Option<SignatureImage>, req: ScanRequest, result: ScanAnswer) {
        self.full = req.full && req.frame == self.frame && req.sensitivity == self.sensitivity;
        match result {
            Ok(Ok(scanned)) => {
                *image = Some(scanned);
                self.problem = None;
            }
            Ok(Err(ScanError::NoInk)) => {
                *image = None;
                self.problem =
                    Some(tl!("No signature was found in the frame. Move the frame over the signature or raise the ink sensitivity.").to_string());
            }
            Ok(Err(e)) => {
                *image = None;
                self.problem = Some(e.to_string());
            }
            Err(e) => {
                *image = None;
                self.problem = Some(e);
            }
        }
    }

    pub fn turn(&mut self, turn: Turn, image: &mut Option<SignatureImage>, ctx: Option<&egui::Context>) {
        self.frame = self.photo.turn(turn, self.frame);
        self.version = next_photo_version();
        self.rescan_in_background(image, true, ctx);
    }
}

impl FillTool {
    pub fn command(self) -> &'static str {
        match self {
            FillTool::Text => "sign.fill.text",
            FillTool::Check => "sign.fill.check",
            FillTool::Cross => "sign.fill.cross",
            FillTool::Dot => "sign.fill.dot",
            FillTool::Line => "sign.fill.line",
            FillTool::Date => "sign.fill.date",
            FillTool::Signature => "sign.fill.signature",
            FillTool::Initials => "sign.fill.initials",
        }
    }

    pub fn from_command(id: &str) -> Option<Self> {
        FILL_TOOLS.into_iter().find(|t| t.command() == id)
    }

    pub fn label(self) -> &'static str {
        match self {
            FillTool::Text => "Add text",
            FillTool::Check => "Checkmark",
            FillTool::Cross => "Cross",
            FillTool::Dot => "Dot",
            FillTool::Line => "Line",
            FillTool::Date => "Date",
            FillTool::Signature => "Sign",
            FillTool::Initials => "Initials",
        }
    }

    pub fn icon(self) -> &'static str {
        match self {
            FillTool::Text => "type",
            FillTool::Check => "check",
            FillTool::Cross => "x",
            FillTool::Dot => "circle-dot",
            FillTool::Line => "minus",
            FillTool::Date => "clock-3",
            FillTool::Signature | FillTool::Initials => "signature",
        }
    }

    fn mark(self) -> Option<FillMark> {
        match self {
            FillTool::Check => Some(FillMark::Check),
            FillTool::Cross => Some(FillMark::Cross),
            FillTool::Dot => Some(FillMark::Dot),
            FillTool::Line => Some(FillMark::Line),
            _ => None,
        }
    }
}

/// Text being typed onto a page: (page, top-left in user space, text, focus requested).
#[derive(Clone, Debug, PartialEq)]
pub struct TypeBox {
    pub page: usize,
    pub at: [f64; 2],
    pub text: String,
    pub size: f64,
    pub focus: bool,
}

/// The default size Fill & Sign uses for typed text.
pub const TEXT_SIZE: f64 = 10.0;
pub const MIN_TEXT_SIZE: f64 = 4.0;
pub const MAX_TEXT_SIZE: f64 = 300.0;

pub fn valid_text_size(size: f64) -> Option<f64> {
    (size.is_finite() && (MIN_TEXT_SIZE..=MAX_TEXT_SIZE).contains(&size)).then_some(size)
}

fn to_user(xf: &PageXform, info: &DocInfo, page: usize, p: Pos2) -> [f64; 2] {
    let (vx, vy) = xf.screen_to_view(p);
    let u = info.pages[page].view_to_user(vx, vy);
    [u[0] as f64, u[1] as f64]
}

fn new(page: usize, shape: Shape, contents: String, author: &str) -> Edit {
    let style = Style::default_for(&shape);
    Edit::AddAnnotation(NewAnnotation { page, shape, style, contents, author: author.to_string() })
}

/// A typewriter annotation sized to its text, its displayed top-left at `at` on a page turned by
/// `rotation` (its `/Rotate`).
pub fn typed(page: usize, at: [f64; 2], text: &str, size: f64, rotation: i64, author: &str) -> Edit {
    let rect = crate::comments::text_box_rect(at, text, size, rotation);
    new(page, Shape::Typewriter { rect, font_size: size }, text.to_string(), author)
}

/// Place a saved signature (strokes normalised to a 0–1 box, y up) with its left edge at `at`,
/// 150 pt wide. Left, centred and upright are as displayed on a page turned by `rotation` (its
/// `/Rotate`), so the strokes are turned back into user space.
pub fn signature_at(page: usize, at: [f64; 2], strokes: &[Vec<[f32; 2]>], rotation: i64, author: &str) -> Option<Edit> {
    let w = 150.0;
    let (min_y, max_y) = strokes.iter().flatten().fold((f32::MAX, f32::MIN), |(a, b), p| (a.min(p[1]), b.max(p[1])));
    if !min_y.is_finite() {
        return None;
    }
    let h = f64::from(max_y - min_y).max(0.05) * w;
    // Displayed right and up as user-space unit vectors (the identity on an unturned page).
    let [a, b, c, d, ..] = pdfcraft_model::view_matrix_for(rotation, [0.0; 4]);
    let strokes: Vec<Vec<[f64; 2]>> = strokes
        .iter()
        .filter(|s| !s.is_empty())
        .map(|s| {
            s.iter()
                .map(|p| {
                    let (dx, dy) = (f64::from(p[0]) * w, f64::from(p[1] - min_y) * w - h / 2.0);
                    [at[0] + a * dx + c * dy, at[1] + b * dx + d * dy]
                })
                .collect()
        })
        .collect();
    (!strokes.is_empty()).then(|| new(page, Shape::Signature { strokes }, String::new(), author))
}

/// Place typed text in the script font with its left edge at `at`, `height` points tall, upright
/// as displayed on a page turned by `rotation`.
pub fn typed_signature_at(page: usize, at: [f64; 2], text: &str, height: f64, rotation: i64, author: &str) -> Option<Edit> {
    pdfcraft_engine::typed_signature_shape(at, text, height, rotation).map(|shape| new(page, shape, String::new(), author))
}

/// Place a saved signature or initials, upright as `info`'s page is displayed.
pub fn place(page: usize, info: &PageInfo, at: [f64; 2], sig: &SavedSig, initials: bool, author: &str) -> Option<Edit> {
    let rotation = i64::from(info.rotation);
    match sig {
        SavedSig::Drawn(strokes) => signature_at(page, at, strokes, rotation, author),
        SavedSig::Image(image) => image.edit(page, info, at, initials, author),
        SavedSig::Typed(text) => {
            let [left, bottom, right, top] = pdfcraft_engine::script_outline(text).bounds();
            let height = if initials { 24.0_f64 } else { 32.0_f64 };
            // Keep long names within the same placement width as drawn signatures.
            let height = height.min(150.0 * (top - bottom).max(0.1) / (right - left).max(0.01));
            typed_signature_at(page, at, text, height, rotation, author)
        }
    }
}

/// The text in the script font as a picture (`w`×`h` px, black on transparent), for previews.
pub(crate) fn script_preview(text: &str, w: usize, h: usize) -> egui::ColorImage {
    let o = pdfcraft_engine::script_outline(text);
    let mut img = egui::ColorImage::filled([w, h], Color32::TRANSPARENT);
    let [left, bottom, right, top] = o.bounds();
    let span = (top - bottom).max(0.1);
    let width = (right - left).max(0.01);
    if o.contours.is_empty() {
        return img;
    }
    let k = ((h as f64 * 0.9) / span).min((w as f64 * 0.95) / width);
    let x0 = (w as f64 - width * k) / 2.0;
    // Device points (y down), then a non-zero winding scanline fill so overlapping same-direction
    // TrueType contours (such as Inter's composite Ђ and ћ) stay filled where strokes cross.
    let polys: Vec<Vec<(f64, f64)>> = o
        .contours
        .iter()
        .map(|c| c.iter().map(|p| (x0 + (p[0] - left) * k, h as f64 * 0.5 + (top + bottom) / 2.0 * k - p[1] * k)).collect())
        .collect();
    for y in 0..h {
        let sy = y as f64 + 0.5;
        let mut xs: Vec<(f64, i32)> = Vec::new();
        for poly in &polys {
            for i in 0..poly.len() {
                let (a, b) = (poly[i], poly[(i + 1) % poly.len()]);
                if (a.1 <= sy) != (b.1 <= sy) {
                    let dir = if a.1 < b.1 { 1 } else { -1 };
                    xs.push((a.0 + (sy - a.1) / (b.1 - a.1) * (b.0 - a.0), dir));
                }
            }
        }
        xs.sort_by(|a, b| a.0.total_cmp(&b.0));
        let mut winding = 0i32;
        let mut start = 0.0;
        for (x, dir) in xs {
            if winding == 0 {
                start = x;
            }
            winding += dir;
            if winding == 0 {
                let (from, to) = (start.round().clamp(0.0, w as f64) as usize, x.round().clamp(0.0, w as f64) as usize);
                for px in from..to {
                    img[(px, y)] = Color32::BLACK;
                }
            }
        }
    }
    img
}

/// Saved previews and add/remove controls, shared by the left panel and quick-tool picker.
pub(crate) fn signature_entries(ui: &mut egui::Ui, app: &mut crate::PdfCraftApp, t: &Tokens) -> Option<&'static str> {
    ui.set_width(ui.available_width().clamp(240.0, 248.0));
    let mut command = None;
    for (i, (saved, what, use_id, change_id, remove_id)) in [
        (app.signature.as_ref(), "signature", "sign.fill.signature", "sign.fill.signature.change", "sign.fill.signature.remove"),
        (app.initials.as_ref(), "initials", "sign.fill.initials", "sign.fill.initials.change", "sign.fill.initials.remove"),
    ]
    .into_iter()
    .enumerate()
    {
        let label = |template: &str| crate::i18n::fmt(tl!(template), &[("what", tl!(what))]);
        let Some(saved) = saved else {
            if crate::widgets::ghost_button(ui, "plus", &label("Add {what}")).clicked() {
                command = Some(use_id);
            }
            continue;
        };
        ui.horizontal(|ui| {
            let (rect, response) = ui.allocate_exact_size(vec2((ui.available_width() - 68.0).max(140.0), 52.0), Sense::click());
            let use_label = label("Use {what}");
            response.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, use_label.clone()));
            let painter = ui.painter_at(rect);
            painter.rect_filled(rect, CornerRadius::same(4), Color32::WHITE);
            painter.rect_stroke(
                rect,
                CornerRadius::same(4),
                Stroke::new(1.0, if response.hovered() { t.accent } else { t.border }),
                egui::StrokeKind::Inside,
            );
            paint_saved(ui, rect.shrink(8.0), saved, &mut app.saved_signature_previews[i], &format!("saved-{what}"));
            if response.on_hover_text(label("Place saved {what}")).clicked() {
                command = Some(use_id);
            }
            if crate::icons::button(ui, "pencil", 26.0, false, &label("Change {what}")).clicked() {
                command = Some(change_id);
            }
            if crate::icons::button(ui, "x", 26.0, false, &label("Remove saved {what}")).clicked() {
                command = Some(remove_id);
            }
        });
        ui.add_space(4.0);
    }
    command
}

/// Draw a saved signature as large as fits in `rect`, centred: typed text and images through
/// `cache` (a texture named `key`), strokes as lines.
pub(crate) fn paint_saved(ui: &egui::Ui, rect: egui::Rect, saved: &SavedSig, cache: &mut Option<(SavedSig, egui::TextureHandle)>, key: &str) {
    let painter = ui.painter_at(rect);
    match saved {
        SavedSig::Typed(text) => {
            if cache.as_ref().is_none_or(|(s, _)| s != saved) {
                *cache = Some((saved.clone(), ui.ctx().load_texture(key, script_preview(text, 480, 104), egui::TextureOptions::LINEAR)));
            }
            if let Some((_, tex)) = cache {
                let size = vec2(480.0, 104.0) * (rect.width() / 480.0).min(rect.height() / 104.0);
                painter.image(
                    tex.id(),
                    egui::Rect::from_center_size(rect.center(), size),
                    egui::Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)),
                    Color32::WHITE,
                );
            }
        }
        SavedSig::Image(image) => image_preview(ui, rect, image, cache),
        SavedSig::Drawn(strokes) => {
            let bounds = strokes
                .iter()
                .flatten()
                .filter(|p| p.iter().all(|v| v.is_finite()))
                .fold(egui::Rect::NOTHING, |r, p| r.union(egui::Rect::from_min_max(pos2(p[0], p[1]), pos2(p[0], p[1]))));
            if bounds.is_finite() {
                let scale = (rect.width() / bounds.width().max(0.01)).min(rect.height() / bounds.height().max(0.01));
                for stroke in strokes {
                    let pts = stroke
                        .iter()
                        .filter(|p| p.iter().all(|v| v.is_finite()))
                        .map(|p| rect.center() + vec2(p[0] - bounds.center().x, bounds.center().y - p[1]) * scale)
                        .collect();
                    painter.add(egui::Shape::line(pts, Stroke::new(1.5, Color32::BLACK)));
                }
            }
        }
    }
}

/// Preview and clicks with a Fill & Sign tool on one page.
#[allow(clippy::too_many_arguments)]
pub(crate) fn page_input(
    ui: &egui::Ui,
    resp: &egui::Response,
    xf: &PageXform,
    ptr: &PagePointer,
    page: usize,
    info: &DocInfo,
    tool: FillTool,
    text_size: f64,
    view: &mut DocView,
    signature: Option<&SavedSig>,
    initials: Option<&SavedSig>,
    preview: &mut Option<(SavedSig, egui::TextureHandle)>,
    author: &str,
    date_text: &Result<String, String>,
) -> Option<FillAction> {
    let pointer = ptr.within(xf.rect)?;
    let p = info.pages.get(page)?;
    let at = to_user(xf, info, page, pointer);
    let saved = match tool {
        FillTool::Signature => signature,
        FillTool::Initials => initials,
        _ => None,
    };
    if let Some(SavedSig::Image(image)) = saved {
        if let Some(rect) = image.rect(p, at, tool == FillTool::Initials) {
            let tex = image_texture(ui, image, preview);
            xf.paint_user_image(ui.painter(), tex, p, rect, i64::from(p.rotation), Color32::WHITE);
            ui.ctx().set_cursor_icon(egui::CursorIcon::None);
        }
    } else {
        ui.ctx().set_cursor_icon(if tool == FillTool::Text { egui::CursorIcon::Text } else { egui::CursorIcon::Crosshair });
    }
    if !resp.clicked() {
        return None;
    }
    // Typed text starts a little above the click, as displayed.
    let rotation = i64::from(p.rotation);
    let top = pdfcraft_engine::displayed_offset(rotation, at, 0.0, text_size * 0.6);
    match tool {
        FillTool::Text => {
            // Clicking away from an open box commits it: the page sees this click before the box
            // sees its lost focus, so replacing the box first would drop what was typed.
            let open = view.fill_text.replace(TypeBox { page, at: top, text: String::new(), size: text_size, focus: true });
            open.filter(|tb| !tb.text.trim().is_empty()).map(|tb| {
                FillAction::Edit(Box::new(typed(tb.page, tb.at, tb.text.trim(), tb.size, crate::comments::page_rotation(info, tb.page), author)))
            })
        }
        FillTool::Date => Some(match date_text {
            Ok(text) => FillAction::Edit(Box::new(typed(page, top, text, text_size, rotation, author))),
            Err(why) => FillAction::Refused(why.clone()),
        }),
        FillTool::Signature => match signature {
            Some(s) => place(page, p, at, s, false, author).map(|e| FillAction::Signature(Box::new(e))),
            None => Some(FillAction::CreateSignature),
        },
        FillTool::Initials => match initials {
            Some(s) => place(page, p, at, s, true, author).map(|e| FillAction::Signature(Box::new(e))),
            None => Some(FillAction::CreateInitials),
        },
        mark => {
            let mark = mark.mark()?;
            let rect = mark.rect_at(at, rotation);
            Some(FillAction::Edit(Box::new(new(page, Shape::Mark { rect, mark }, String::new(), author))))
        }
    }
}

/// What a Fill & Sign click asks the app to do.
#[derive(Clone, Debug, PartialEq)]
pub enum FillAction {
    Edit(Box<Edit>),
    /// Place once, then select the new signature or initials for adjustment.
    Signature(Box<Edit>),
    /// No signature yet: open the signature pad.
    CreateSignature,
    /// No initials yet.
    CreateInitials,
    /// Nothing placed, and why (today's date can't be written into the PDF yet).
    Refused(String),
}

/// The in-place editor for typed text. Returns the edit once committed.
pub(crate) fn type_box(ctx: &egui::Context, view: &mut DocView, info: &DocInfo, author: &str) -> Option<Edit> {
    let tb = view.fill_text.clone()?;
    let xf = view.page_xform(tb.page)?;
    let v = info.pages.get(tb.page)?.user_to_view(tb.at[0] as f32, tb.at[1] as f32);
    let pos = xf.norm_to_screen(v[0] / xf.pw, v[1] / xf.ph);
    let zoom = xf.zoom();
    let mut commit = false;
    let mut cancel = false;
    // Shown at once, like the paragraph editor: a fade-in shows the page's copy through it.
    // Over the page, so only where the document shows: a box scrolled under the mode bar or a
    // side panel must leave them their pointer (#2755). It stays where it was placed, cut off as
    // the page is, rather than pushed back into the window.
    egui::Area::new(egui::Id::new(("fill-text", view.id.0)))
        .fade_in(false)
        .order(egui::Order::Foreground)
        .fixed_pos(pos)
        .constrain_to(view.viewport_rect())
        .constrain(false)
        .show(ctx, |ui| {
            let Some(t) = view.fill_text.as_mut() else { return };
            // The box is white in every theme, so the caret must not follow the theme (pale in dark mode) (#655).
            ui.visuals_mut().text_cursor.stroke.color = crate::edit_text_ui::caret_color_on(Color32::WHITE);
            let width = ((t.text.chars().count().max(8) as f32) * t.size as f32 * 0.6 * zoom).clamp(60.0, 600.0);
            // The area's max rect is last frame's size, which caps the edit's width; widen it so the
            // box grows as text is typed.
            ui.set_min_width(width);
            let r = ui.add(
                egui::TextEdit::singleline(&mut t.text)
                    .font(egui::FontId::proportional((t.size as f32 * zoom).max(8.0)))
                    .desired_width(width)
                    .background_color(Color32::from_rgba_unmultiplied(255, 255, 255, 230))
                    .text_color(Color32::BLACK)
                    .hint_text(tl!("Type text"))
                    .id_salt("fill-text-edit"),
            );
            if t.focus {
                r.request_focus();
                t.focus = false;
            }
            if ui.input(|i| i.key_pressed(egui::Key::Escape)) {
                cancel = true;
            } else if r.lost_focus() {
                commit = true;
            }
        });
    if cancel {
        view.fill_text = None;
        return None;
    }
    if commit {
        let tb = view.fill_text.take()?;
        if !tb.text.trim().is_empty() {
            return Some(typed(tb.page, tb.at, tb.text.trim(), tb.size, crate::comments::page_rotation(info, tb.page), author));
        }
    }
    None
}

/// The signature pad: type, draw, import an image or scan a photo. Returns Apply, Cancel and
/// Browse (for an image or, on the Scan tab, a photo).
pub(crate) fn signature_pad(
    ui: &mut egui::Ui,
    t: &Tokens,
    d: &mut SigDraft,
    preview: &mut Option<(SavedSig, egui::TextureHandle)>,
    photo_texture: &mut Option<(u64, egui::TextureHandle)>,
) -> (bool, bool, bool) {
    let what = if d.initials { tl!("initials") } else { tl!("signature") };
    let title = if d.editing { tl!("Change {what}") } else { tl!("Create {what}") };
    ui.label(egui::RichText::new(crate::i18n::fmt(title, &[("what", what)])).font(crate::theme::semibold(18.0)));
    ui.horizontal(|ui| {
        if crate::widgets::pill_button(ui, tl_ctx!("signature pad", "Type"), !d.drawing && !d.image_mode && !d.scan_mode).clicked() {
            d.drawing = false;
            d.image_mode = false;
            d.scan_mode = false;
        }
        if crate::widgets::pill_button(ui, tl!("Draw"), d.drawing && !d.image_mode && !d.scan_mode).clicked() {
            d.drawing = true;
            d.image_mode = false;
            d.scan_mode = false;
        }
        if crate::widgets::pill_button(ui, tl!("Image"), d.image_mode).clicked() {
            d.image_mode = true;
            d.scan_mode = false;
        }
        if crate::widgets::pill_button(ui, tl!("Scan"), d.scan_mode).clicked() && !d.scan_mode {
            d.scan_mode = true;
            d.image_mode = false;
            // The Image tab shares the result; show this tab's own again.
            if let Some(scan) = d.scan.as_mut() {
                scan.rescan_in_background(&mut d.image, true, Some(ui.ctx()));
            }
        }
    });
    ui.add_space(6.0);
    if d.scan_mode {
        let browse = scan_tab(ui, t, d, preview, photo_texture);
        let (apply, cancel) = pad_buttons(ui, d);
        return (apply, cancel, browse);
    }
    if d.image_mode {
        ui.label(egui::RichText::new(tl!("Choose a PNG or JPEG image (up to 4 MiB). Transparency is preserved.")).color(t.text_muted));
        let browse = crate::widgets::ghost_button(ui, "folder", tl!("Browse…")).clicked();
        let (rect, _) = ui.allocate_exact_size(vec2(460.0, 150.0), Sense::hover());
        ui.painter().rect_filled(rect, CornerRadius::same(6), Color32::WHITE);
        ui.painter().rect_stroke(rect, CornerRadius::same(6), Stroke::new(1.0, t.border), egui::StrokeKind::Inside);
        if let Some(image) = &d.image {
            image_preview(ui, rect.shrink(8.0), image, preview);
        }
        let (apply, cancel) = pad_buttons(ui, d);
        return (apply, cancel, browse);
    }
    if !d.drawing {
        let l = ui.label(egui::RichText::new(crate::i18n::fmt(tl!("Type your {what}."), &[("what", what)])).color(t.text_muted));
        ui.add(
            egui::TextEdit::singleline(&mut d.text)
                .char_limit(pdfcraft_engine::MAX_SIGNATURE_CHARS)
                .desired_width(460.0)
                .hint_text(tl!(if d.initials { "Initials" } else { "Your name" })),
        )
        .labelled_by(l.id);
        let (rect, _) = ui.allocate_exact_size(vec2(460.0, 150.0), Sense::hover());
        let painter = ui.painter_at(rect);
        painter.rect_filled(rect, CornerRadius::same(6), Color32::WHITE);
        painter.rect_stroke(rect, CornerRadius::same(6), Stroke::new(1.0, t.border), egui::StrokeKind::Inside);
        let key = SavedSig::Typed(d.text.clone());
        if preview.as_ref().is_none_or(|(s, _)| *s != key) {
            let img = script_preview(&d.text, 920, 300);
            *preview = Some((key, ui.ctx().load_texture("typed-signature", img, egui::TextureOptions::LINEAR)));
        }
        if let Some((_, tex)) = preview {
            painter.image(tex.id(), rect.shrink(4.0), egui::Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)), Color32::WHITE);
        }
        let (apply, cancel) = pad_buttons(ui, d);
        return (apply, cancel, false);
    }
    ui.label(egui::RichText::new(crate::i18n::fmt(tl!("Draw your {what} below."), &[("what", what)])).color(t.text_muted));
    // The dialog is modal, so ⌘Z / Ctrl+Z here can only mean the last stroke (the document's own
    // undo is blocked while it is open).
    if ui.input_mut(|i| i.consume_key(egui::Modifiers::COMMAND, egui::Key::Z)) {
        d.undo_stroke();
    }
    let strokes = &mut d.strokes;
    let (rect, resp) = ui.allocate_exact_size(vec2(460.0, 150.0), Sense::drag());
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, CornerRadius::same(6), Color32::WHITE);
    painter.rect_stroke(rect, CornerRadius::same(6), Stroke::new(1.0, t.border), egui::StrokeKind::Inside);
    painter.hline(rect.x_range().shrink(24.0), rect.bottom() - 34.0, Stroke::new(1.0, t.divider));
    // Normalised: x 0–1 across the pad, y up, in units of the pad's width.
    let norm = |p: Pos2| [(p.x - rect.left()) / rect.width(), (rect.bottom() - p.y) / rect.width()];
    if resp.drag_started() {
        strokes.push(Vec::new());
    }
    if resp.dragged()
        && let (Some(p), Some(s)) = (resp.interact_pointer_pos(), strokes.last_mut())
    {
        let n = norm(p.clamp(rect.min, rect.max));
        if s.last().is_none_or(|l| (l[0] - n[0]).abs() + (l[1] - n[1]).abs() > 0.002) {
            s.push(n);
        }
    }
    for s in strokes.iter() {
        let pts: Vec<Pos2> = s.iter().map(|p| pos2(rect.left() + p[0] * rect.width(), rect.bottom() - p[1] * rect.width())).collect();
        painter.add(egui::Shape::line(pts, Stroke::new(2.0, Color32::BLACK)));
    }
    let (apply, cancel) = pad_buttons(ui, d);
    (apply, cancel, false)
}

/// The Scan tab above the pad buttons. Returns whether a photo should be chosen.
fn scan_tab(
    ui: &mut egui::Ui,
    t: &Tokens,
    d: &mut SigDraft,
    preview: &mut Option<(SavedSig, egui::TextureHandle)>,
    photo_texture: &mut Option<(u64, egui::TextureHandle)>,
) -> bool {
    ui.label(
        egui::RichText::new(tl!(
            "Sign a sheet of white paper and take a photo of it in bright, even light (use the flash). Choose the photo or drop it here: PdfCraft finds the signature and removes the paper."
        ))
        .color(t.text_muted),
    );
    let mut browse = false;
    ui.horizontal(|ui| {
        browse = crate::widgets::ghost_button(ui, "camera", tl!("Choose photo…")).clicked();
        if let Some(scan) = d.scan.as_mut() {
            ui.add_space(8.0);
            if crate::icons::button(ui, "rotate-ccw", 28.0, false, tl!("Rotate counterclockwise")).clicked() {
                scan.turn(Turn::CounterClockwise, &mut d.image, Some(ui.ctx()));
            }
            if crate::icons::button(ui, "rotate-cw", 28.0, false, tl!("Rotate clockwise")).clicked() {
                scan.turn(Turn::Clockwise, &mut d.image, Some(ui.ctx()));
            }
        }
    });
    let (area, resp) = ui.allocate_exact_size(vec2(460.0, 230.0), Sense::drag());
    let painter = ui.painter_at(area);
    painter.rect_filled(area, CornerRadius::same(6), t.pasteboard);
    painter.rect_stroke(area, CornerRadius::same(6), Stroke::new(1.0, t.border), egui::StrokeKind::Inside);
    if let Some(scan) = d.scan.as_mut() {
        scan.poll(&mut d.image, Some(ui.ctx()));
    }
    let Some(scan) = d.scan.as_mut() else {
        painter.text(area.center(), egui::Align2::CENTER_CENTER, tl!("No photo chosen"), egui::FontId::proportional(13.0), t.text_muted);
        resp.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Other, true, tl!("Photo")));
        return browse;
    };
    resp.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Other, true, tl!("Photo")));
    // The photo, fitted into the area; `to_screen`/`to_photo` map between its pixels and points.
    let [pw, ph] = scan.photo.size();
    let (pw, ph) = (pw.max(1) as f32, ph.max(1) as f32);
    let fit = area.shrink(8.0);
    let scale = (fit.width() / pw).min(fit.height() / ph);
    let shown = egui::Rect::from_center_size(fit.center(), vec2(pw * scale, ph * scale));
    if photo_texture.as_ref().is_none_or(|(v, _)| *v != scan.version) {
        let thumb = scan.photo.thumbnail(1024);
        let size = [thumb.width() as usize, thumb.height() as usize];
        let pixels = egui::ColorImage::from_rgba_unmultiplied(size, thumb.as_raw());
        *photo_texture = Some((scan.version, ui.ctx().load_texture("signature-photo", pixels, egui::TextureOptions::LINEAR)));
    }
    if let Some((_, tex)) = photo_texture {
        painter.image(tex.id(), shown, egui::Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)), Color32::WHITE);
    }
    let to_screen = |x: f32, y: f32| pos2(shown.left() + x * scale, shown.top() + y * scale);
    let to_photo = |p: Pos2| [((p.x - shown.left()) / scale).clamp(0.0, pw), ((p.y - shown.top()) / scale).clamp(0.0, ph)];
    let frame_screen = |f: PhotoRect| {
        egui::Rect::from_min_max(
            to_screen(f.x as f32, f.y as f32),
            to_screen(f.x.saturating_add(f.width) as f32, f.y.saturating_add(f.height) as f32),
        )
    };

    // Move the frame from inside, resize it from a corner, or draw a new one elsewhere.
    let handle = 10.0;
    if resp.drag_started()
        && let Some(p) = resp.interact_pointer_pos()
    {
        let fr = frame_screen(scan.frame);
        let corners = [fr.left_top(), fr.right_top(), fr.left_bottom(), fr.right_bottom()];
        let opposite = [fr.right_bottom(), fr.left_bottom(), fr.right_top(), fr.left_top()];
        let at = to_photo(p);
        scan.drag = Some(if let Some(i) = corners.iter().position(|c| c.distance(p) <= handle) {
            FrameDrag::Span(opposite.get(i).map_or(at, |o| to_photo(*o)))
        } else if fr.contains(p) {
            FrameDrag::Move([at[0] - scan.frame.x as f32, at[1] - scan.frame.y as f32])
        } else {
            FrameDrag::Span(at)
        });
    }
    if resp.dragged()
        && let (Some(drag), Some(p)) = (scan.drag, resp.interact_pointer_pos())
    {
        let at = to_photo(p);
        let (x0, y0, x1, y1) = match drag {
            FrameDrag::Move(grab) => {
                let (w, h) = (scan.frame.width as f32, scan.frame.height as f32);
                let x = (at[0] - grab[0]).clamp(0.0, (pw - w).max(0.0));
                let y = (at[1] - grab[1]).clamp(0.0, (ph - h).max(0.0));
                (x, y, x + w, y + h)
            }
            FrameDrag::Span(anchor) => (anchor[0].min(at[0]), anchor[1].min(at[1]), anchor[0].max(at[0]), anchor[1].max(at[1])),
        };
        // At least a few screen points, so a frame can always be grabbed again.
        let min = (12.0 / scale).max(1.0);
        let frame = PhotoRect {
            x: x0.round() as u32,
            y: y0.round() as u32,
            width: (x1 - x0).max(min).round() as u32,
            height: (y1 - y0).max(min).round() as u32,
        }
        .clamped(pw as u32, ph as u32);
        if frame != scan.frame {
            scan.frame = frame;
            scan.rescan_in_background(&mut d.image, false, Some(ui.ctx()));
        }
    }
    if resp.drag_stopped() && scan.drag.take().is_some() {
        scan.rescan_in_background(&mut d.image, true, Some(ui.ctx()));
    }

    // Dim the photo outside the frame, then outline it with corner handles.
    let fr = frame_screen(scan.frame).intersect(shown);
    let dim = Color32::from_black_alpha(120);
    for r in [
        egui::Rect::from_min_max(shown.left_top(), pos2(shown.right(), fr.top())),
        egui::Rect::from_min_max(pos2(shown.left(), fr.bottom()), shown.right_bottom()),
        egui::Rect::from_min_max(pos2(shown.left(), fr.top()), pos2(fr.left(), fr.bottom())),
        egui::Rect::from_min_max(pos2(fr.right(), fr.top()), pos2(shown.right(), fr.bottom())),
    ] {
        if r.is_positive() {
            painter.rect_filled(r, CornerRadius::ZERO, dim);
        }
    }
    painter.rect_stroke(fr, CornerRadius::ZERO, Stroke::new(2.0, t.accent), egui::StrokeKind::Middle);
    for c in [fr.left_top(), fr.right_top(), fr.left_bottom(), fr.right_bottom()] {
        let r = egui::Rect::from_center_size(c, vec2(9.0, 9.0));
        painter.rect_filled(r, CornerRadius::same(2), Color32::WHITE);
        painter.rect_stroke(r, CornerRadius::same(2), Stroke::new(1.5, t.accent), egui::StrokeKind::Inside);
    }
    if resp.hovered() && scan.drag.is_none() {
        let hover = ui.input(|i| i.pointer.hover_pos());
        let fr = frame_screen(scan.frame);
        let icon = match hover {
            Some(p) if [fr.left_top(), fr.right_bottom()].iter().any(|c| c.distance(p) <= handle) => egui::CursorIcon::ResizeNwSe,
            Some(p) if [fr.right_top(), fr.left_bottom()].iter().any(|c| c.distance(p) <= handle) => egui::CursorIcon::ResizeNeSw,
            Some(p) if fr.contains(p) => egui::CursorIcon::Grab,
            _ => egui::CursorIcon::Crosshair,
        };
        ui.ctx().set_cursor_icon(icon);
    }

    ui.add_space(6.0);
    ui.horizontal(|ui| {
        let l = ui.label(tl!("Ink sensitivity"));
        let slider = ui.add(egui::Slider::new(&mut scan.sensitivity, 0..=100)).labelled_by(l.id);
        if slider.changed() {
            // Quick previews while dragging; keyboard steps get the full result at once.
            scan.rescan_in_background(&mut d.image, !slider.dragged(), Some(ui.ctx()));
        }
        if slider.drag_stopped() {
            scan.rescan_in_background(&mut d.image, true, Some(ui.ctx()));
        }
    });
    ui.add_space(4.0);
    let (rect, _) = ui.allocate_exact_size(vec2(460.0, 110.0), Sense::hover());
    ui.painter().rect_filled(rect, CornerRadius::same(6), Color32::WHITE);
    ui.painter().rect_stroke(rect, CornerRadius::same(6), Stroke::new(1.0, t.border), egui::StrokeKind::Inside);
    if let Some(image) = &d.image {
        image_preview(ui, rect.shrink(8.0), image, preview);
    } else if let Some(problem) = &scan.problem {
        ui.put(rect.shrink(12.0), egui::Label::new(egui::RichText::new(problem.as_str()).color(Color32::from_rgb(0x8A, 0x5A, 0x00))).wrap());
    }
    browse
}

fn image_texture(ui: &egui::Ui, image: &SignatureImage, cache: &mut Option<(SavedSig, egui::TextureHandle)>) -> egui::TextureId {
    let key = SavedSig::Image(image.clone());
    if cache.as_ref().is_some_and(|(s, _)| *s != key) {
        *cache = None;
    }
    let (_, tex) = cache.get_or_insert_with(|| {
        let pixels = egui::ColorImage::from_rgba_unmultiplied(image.size(), image.rgba());
        (key, ui.ctx().load_texture("signature-image", pixels, egui::TextureOptions::LINEAR))
    });
    tex.id()
}

fn image_preview(ui: &egui::Ui, rect: egui::Rect, image: &SignatureImage, cache: &mut Option<(SavedSig, egui::TextureHandle)>) {
    let tex = image_texture(ui, image, cache);
    let [w, h] = image.size();
    let size = vec2(w as f32, h as f32);
    let size = size * (rect.width() / size.x).min(rect.height() / size.y);
    ui.painter().image(
        tex,
        egui::Rect::from_center_size(rect.center(), size),
        egui::Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)),
        Color32::WHITE,
    );
}

fn pad_buttons(ui: &mut egui::Ui, d: &mut SigDraft) -> (bool, bool) {
    ui.add_space(10.0);
    let (mut apply, mut cancel) = (false, false);
    ui.horizontal(|ui| {
        if d.draw_tab() && ui.add_enabled(!d.strokes.is_empty(), egui::Button::new(tl!("Undo"))).clicked() {
            d.undo_stroke();
        }
        if ui.button(tl!("Clear")).clicked() {
            d.strokes.clear();
            d.text.clear();
            d.image = None;
            if d.scan_mode {
                d.scan = None;
            }
        }
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            let ready = d.ready();
            if ui.add_enabled_ui(ready, |ui| crate::widgets::pill_button(ui, tl!("Apply"), true)).inner.clicked() {
                apply = true;
            }
            if crate::widgets::pill_button(ui, tl!("Cancel"), false).clicked() {
                cancel = true;
            }
        });
    });
    (apply, cancel)
}

#[cfg(target_arch = "wasm32")]
#[derive(Clone, Default)]
pub(crate) struct ImageInbox {
    busy: std::sync::Arc<std::sync::atomic::AtomicBool>,
    picked: std::sync::Arc<std::sync::Mutex<Option<PickedImage>>>,
}

#[cfg(target_arch = "wasm32")]
struct PickedImage {
    epoch: u64,
    initials: bool,
    /// A photo for the Scan tab (otherwise an image for the Image tab).
    scan: bool,
    bytes: Option<Result<Vec<u8>, String>>,
}

impl crate::PdfCraftApp {
    /// Browse asynchronously, answering only into the dialog (and tab) that asked for the image
    /// or, on the Scan tab, the photo.
    pub(crate) fn pick_signature_image(&mut self) {
        let epoch = self.dialog_epoch();
        let initials = self.signature_draft.initials;
        let scan = self.signature_draft.scan_mode;
        let dialog = rfd::AsyncFileDialog::new().add_filter(tl!(if scan { "Photo" } else { "Image" }), &["png", "jpg", "jpeg"]);
        #[cfg(not(target_arch = "wasm32"))]
        self.ask_one(crate::pickers::Ask::File(dialog), None, move |app, path| {
            if app.dialog_epoch() != epoch
                || app.dialog != Some(crate::Dialog::Signature)
                || app.signature_draft.initials != initials
                || app.signature_draft.scan_mode != scan
            {
                return;
            }
            if scan {
                let result = std::fs::File::open(path).map_err(ScanError::from).and_then(SignaturePhoto::read);
                app.use_signature_photo(result.map_err(|e| e.to_string()));
                return;
            }
            let result =
                std::fs::File::open(path).map_err(pdfcraft_engine::signature_image::SignatureImageError::from).and_then(SignatureImage::read);
            app.use_signature_image(result.map_err(|e| e.to_string()));
        });
        #[cfg(target_arch = "wasm32")]
        {
            use std::sync::atomic::Ordering;
            if self.signature_images.busy.swap(true, Ordering::SeqCst) {
                self.notify_tr("Another file dialog is still open. Finish with it first.");
                return;
            }
            let inbox = self.signature_images.clone();
            let ctx = self.ctx.clone();
            let (limit, too_large) = if scan {
                (pdfcraft_engine::signature_scan::MAX_PHOTO_BYTES, ScanError::Size.to_string())
            } else {
                (pdfcraft_engine::signature_image::MAX_SIGNATURE_IMAGE_BYTES, pdfcraft_engine::signature_image::SignatureImageError::Size.to_string())
            };
            wasm_bindgen_futures::spawn_local(async move {
                let bytes = if let Some(file) = dialog.pick_file().await {
                    if file.inner().size() > limit as f64 {
                        Some(Err(too_large))
                    } else {
                        // `pick_file` returns a readable handle. Read its Blob directly so a
                        // rejected browser read is an error (rfd's read helper unwraps it).
                        Some(match wasm_bindgen_futures::JsFuture::from(file.inner().array_buffer()).await {
                            Ok(buffer) => {
                                let bytes = js_sys::Uint8Array::new(&buffer);
                                if bytes.length() as usize > limit { Err(too_large) } else { Ok(bytes.to_vec()) }
                            }
                            Err(error) => Err(format!("{error:?}")),
                        })
                    }
                } else {
                    None
                };
                *inbox.picked.lock().unwrap_or_else(std::sync::PoisonError::into_inner) = Some(PickedImage { epoch, initials, scan, bytes });
                if let Some(ctx) = ctx {
                    ctx.request_repaint();
                }
            });
        }
    }

    #[cfg(target_arch = "wasm32")]
    pub(crate) fn process_signature_images(&mut self) {
        let picked = self.signature_images.picked.lock().unwrap_or_else(std::sync::PoisonError::into_inner).take();
        let Some(picked) = picked else { return };
        self.signature_images.busy.store(false, std::sync::atomic::Ordering::SeqCst);
        if self.dialog_epoch() != picked.epoch
            || self.dialog != Some(crate::Dialog::Signature)
            || self.signature_draft.initials != picked.initials
            || self.signature_draft.scan_mode != picked.scan
        {
            return;
        }
        let Some(bytes) = picked.bytes else { return };
        if picked.scan {
            let result = bytes.and_then(|bytes| {
                pdfcraft_engine::guard(|| SignaturePhoto::from_bytes(&bytes)).map_err(|e| e.to_string())?.map_err(|e| e.to_string())
            });
            self.use_signature_photo(result);
            return;
        }
        let result = bytes
            .and_then(|bytes| pdfcraft_engine::guard(|| SignatureImage::from_bytes(&bytes)).map_err(|e| e.to_string())?.map_err(|e| e.to_string()));
        self.use_signature_image(result);
    }

    fn use_signature_image(&mut self, result: Result<SignatureImage, String>) {
        let cleaned = result.and_then(|image| image.remove_white_background(245, 35).map_err(|e| e.to_string()));
        match cleaned {
            Ok(image) => {
                self.signature_draft.image = Some(image);
                self.toast = None;
            }
            Err(e) => self.notify_fmt("Couldn't import the signature image: {e}", &[("e", &e)]),
        }
    }

    /// Files dropped while the signature pad shows the Image or Scan tab are imported there (the
    /// first one) instead of opening behind the dialog.
    #[cfg(not(target_arch = "wasm32"))]
    pub(crate) fn drop_on_signature_pad(&mut self, dropped: Vec<egui::DroppedFileHandle>) -> Vec<egui::DroppedFileHandle> {
        let d = &self.signature_draft;
        if dropped.is_empty() || self.dialog != Some(crate::Dialog::Signature) || !(d.scan_mode || d.image_mode) {
            return dropped;
        }
        let scan = d.scan_mode;
        let Some(file) = dropped.into_iter().next() else { return Vec::new() };
        let limit = pdfcraft_engine::signature_image::MAX_SIGNATURE_IMAGE_BYTES;
        let path = file.path().to_path_buf();
        let bytes: Result<Vec<u8>, String> = if path.is_absolute() {
            use std::io::Read as _;
            let mut bytes = Vec::new();
            std::fs::File::open(&path).and_then(|f| f.take(limit as u64 + 1).read_to_end(&mut bytes)).map(|_| bytes).map_err(|e| e.to_string())
        } else {
            file.bytes().map_err(|e| e.to_string())
        };
        if scan {
            let photo =
                bytes.and_then(|b| pdfcraft_engine::guard(|| SignaturePhoto::from_bytes(&b)).map_err(|e| e.to_string())?.map_err(|e| e.to_string()));
            self.use_signature_photo(photo);
        } else {
            let image =
                bytes.and_then(|b| pdfcraft_engine::guard(|| SignatureImage::from_bytes(&b)).map_err(|e| e.to_string())?.map_err(|e| e.to_string()));
            self.use_signature_image(image);
        }
        Vec::new()
    }

    /// Frame the signature in a new photo and clean it up. A previous photo stays on failure.
    fn use_signature_photo(&mut self, result: Result<SignaturePhoto, String>) {
        match result.and_then(|photo| pdfcraft_engine::guard(|| ScanDraft::new(photo))) {
            Ok(mut scan) => {
                scan.rescan_in_background(&mut self.signature_draft.image, true, self.ctx.as_ref());
                self.signature_draft.scan = Some(scan);
                self.toast = None;
            }
            Err(e) => self.notify_fmt("Couldn't use the photo: {e}", &[("e", &e)]),
        }
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn undo_removes_the_last_stroke_and_skips_empty_ones() {
        let mut d = super::SigDraft { drawing: true, ..Default::default() };
        assert!(!d.undo_stroke());
        d.strokes = vec![vec![[0.1, 0.1], [0.2, 0.2]], vec![[0.3, 0.3], [0.4, 0.4]], vec![[0.5, 0.5]], vec![]];
        assert!(d.undo_stroke());
        assert_eq!(d.strokes.len(), 1);
        assert!(d.undo_stroke());
        assert!(d.strokes.is_empty());
        assert!(!d.undo_stroke());
    }

    #[test]
    fn long_signature_previews_have_clear_margins() {
        for text in ["Alexandria Catherine Elizabeth Montgomery-Wellington", "Jg Jg Jg Jg Jg Jg Jg Jg Jg Jg Jg"] {
            for [w, h] in [[920, 300], [480, 104]] {
                let image = super::script_preview(text, w, h);
                assert!(image.pixels.iter().any(|p| p.a() > 0));
                for x in 0..w {
                    assert_eq!(image[(x, 0)].a(), 0);
                    assert_eq!(image[(x, h - 1)].a(), 0);
                }
                for y in 0..h {
                    assert_eq!(image[(0, y)].a(), 0);
                    assert_eq!(image[(w - 1, y)].a(), 0);
                }
            }
        }
    }

    #[test]
    fn overlapping_glyph_contours_fill_without_holes_while_keeping_counters() {
        let runs =
            |img: &egui::ColorImage, y: usize| (0..img.width()).filter(|&x| img[(x, y)].a() > 0 && (x == 0 || img[(x - 1, y)].a() == 0)).count();
        // #1786: Inter's Ђ (T + right hook) and ћ (h + crossbar) have two same-direction contours
        // that overlap on the stem; non-zero winding fills the join instead of punching a hole.
        let dje = super::script_preview("Ђ", 200, 200);
        assert_eq!(dje[(97, 108)], egui::Color32::BLACK, "stem-hook overlap of Ђ is filled");
        assert_eq!(runs(&dje, 108), 2, "stem and right loop of Ђ are two solid strokes");
        let tshe = super::script_preview("ћ", 200, 200);
        assert_eq!(tshe[(93, 53)], egui::Color32::BLACK, "stem-crossbar overlap of ћ is filled");
        assert_eq!(runs(&tshe, 53), 1, "crossbar of ћ is one unbroken stroke across the stem");
        // Counter-clockwise inner contours remain open holes.
        let omicron = super::script_preview("О", 200, 200);
        assert_eq!(omicron[(109, 94)].a(), 0, "counter of О stays transparent");
        assert_eq!(runs(&omicron, 94), 2);
        assert!(super::script_preview("", 200, 200).pixels.iter().all(|p| p.a() == 0));
        assert!(super::script_preview("Ђћ", 0, 0).pixels.is_empty());
    }
}
