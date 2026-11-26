use std::sync::Arc;

use craft::behavior::geometry::CropBox;
use craft::behavior::kurbo::{Point, Rect as DocRect, Size};
use craft::behavior::{CanvasView, EditEvent, PixelSize};
use craft::host::queue::{JobQueue, Priority, QueueOptions};
use craft::host::{Host, PickId, PickRequest, Picked, Wake};
use craft::review::ui::signature::DrawPad;
use craft::review::{SignatureDraft, SignatureImage as PadImage};
use craft::theme::variant::Tone;
use craft::ui::buttons::{Button, IconButton};
use craft::ui::canvas::{Canvas, CropOutside, CropOverlay, PixelRect, RasterDisplay, RasterSource};
use craft::ui::contract::{Bind, Props as _, Show as _, ShowContainer as _, clicked};
use craft::ui::display::{Shade, Text};
use craft::ui::form::SliderField;
use craft::ui::layout_components::Row;
use craft::ui::notices::{Notice, NoticeLayout};
use craft::ui::primitives::Revision;
use craft::ui::runtime::drag::OsFile;
use pdfcraft_engine::signature_scan::{DEFAULT_SENSITIVITY, MAX_PHOTO_BYTES, PREVIEW_MAX_SIDE, SCAN_MAX_SIDE, ScanError};
use pdfcraft_engine::{PhotoRect, SignatureImage, SignaturePhoto, Turn};

/// The Scan tab's photo, the frame around the signature and how strongly faint ink is picked up.
#[derive(Clone, Debug, PartialEq)]
pub struct ScanDraft {
    pub photo: SignaturePhoto,
    pub frame: PhotoRect,
    pub sensitivity: u8,
    /// Changes whenever the photo's pixels do (a new photo or a turn), as the photo raster's revision.
    version: u64,
    /// The draft's image is the full-resolution result, not a quick preview.
    pub(super) full: bool,
    /// Why the frame gives no signature, shown in place of the preview.
    pub problem: Option<String>,
}

/// What one scan cleans: the frame and sensitivity at the time it was asked for.
#[derive(Clone, Copy, Debug, PartialEq)]
struct ScanRequest {
    frame: PhotoRect,
    sensitivity: u8,
    full: bool,
}

type ScanAnswer = Result<Result<SignatureImage, ScanError>, String>;

/// A finished scan with the photo version and request it answers.
type ScanDone = (u64, u64, ScanRequest, ScanAnswer);

fn next_photo_version() -> u64 {
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
    NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
}

impl ScanDraft {
    /// Frame the signature (turning a sideways one upright) with the default sensitivity.
    pub fn new(mut photo: SignaturePhoto) -> Self {
        let frame = photo.auto_frame();
        Self { photo, frame, sensitivity: DEFAULT_SENSITIVITY, version: next_photo_version(), full: false, problem: None }
    }

    fn request(&self, full: bool) -> ScanRequest {
        ScanRequest { frame: self.frame, sensitivity: self.sensitivity, full }
    }

    fn run(photo: &SignaturePhoto, req: ScanRequest) -> ScanAnswer {
        let max_side = if req.full { SCAN_MAX_SIDE } else { PREVIEW_MAX_SIDE };
        pdfcraft_engine::guard(|| photo.scan(req.frame, req.sensitivity, max_side))
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

    /// Turn the photo and the frame with it.
    pub fn turn(&mut self, turn: Turn) {
        self.frame = self.photo.turn(turn, self.frame);
        self.version = next_photo_version();
        self.full = false;
    }
}

/// The photo as the canvas's raster. The photo is opaque, so its pixels are the premultiplied texels.
struct PhotoSource<'a> {
    photo: &'a SignaturePhoto,
    version: u64,
}

impl RasterSource for PhotoSource<'_> {
    fn size(&self) -> PixelSize {
        let [w, h] = self.photo.size();
        PixelSize::new(w, h)
    }

    fn revision(&self) -> Revision {
        Revision::new(self.version)
    }

    fn damage(&self, _: Revision) -> Option<PixelRect> {
        None
    }

    fn read(&self, region: PixelRect, factor: u32, out: &mut egui::ColorImage) {
        let rgb = self.photo.rgb();
        let [w, h] = self.photo.size();
        let width = out.size[0].max(1);
        let step = i64::from(factor.max(1));
        let (last_x, last_y) = (i64::from(w.max(1)) - 1, i64::from(h.max(1)) - 1);
        for (i, texel) in out.pixels.iter_mut().enumerate() {
            let (column, row) = ((i % width) as i64, (i / width) as i64);
            let x = (i64::from(region.x0) + column * step + step / 2).clamp(0, last_x) as u32;
            let y = (i64::from(region.y0) + row * step + step / 2).clamp(0, last_y) as u32;
            if let Some(pixel) = rgb.get_pixel_checked(x, y) {
                *texel = egui::Color32::from_rgb(pixel.0[0], pixel.0[1], pixel.0[2]);
            }
        }
    }
}

/// The cleaned scan as the pad's image; none when its size does not fit the pad's.
fn pad_image(image: &SignatureImage) -> Option<PadImage> {
    let [w, h] = image.size();
    Some(PadImage::new(u32::try_from(w).ok()?, u32::try_from(h).ok()?, image.rgba().to_vec()))
}

/// The frame as the overlay's box, in photo pixels.
fn crop_of(frame: PhotoRect) -> CropBox {
    let (x, y) = (f64::from(frame.x), f64::from(frame.y));
    CropBox::upright(DocRect::new(x, y, x + f64::from(frame.width), y + f64::from(frame.height)))
}

/// The frame an overlay box stands for: whole pixels, at least one, inside the photo.
fn frame_of(rect: DocRect, size: [u32; 2]) -> PhotoRect {
    let whole = |v: f64| v.round().max(0.0) as u32;
    PhotoRect { x: whole(rect.x0), y: whole(rect.y0), width: whole(rect.width()).max(1), height: whole(rect.height()).max(1) }
        .clamped(size[0], size[1])
}

/// What the Scan tab keeps between passes.
struct ScanTabState {
    host: Arc<Host>,
    scan: Option<ScanDraft>,
    /// The latest cleaned image, quick or full; the draft's own image is set only from a full one.
    preview: Option<SignatureImage>,
    /// `preview` as the pad's image type, converted once per answer.
    shown: Option<PadImage>,
    pick: Option<PickId>,
    /// One key: a newer request replaces a waiting one.
    queue: JobQueue<u8, ScanDone>,
    request: u64,
    view: CanvasView,
    /// Why the last photo was not used.
    error: Option<String>,
}

impl Drop for ScanTabState {
    fn drop(&mut self) {
        if let Some(id) = self.pick.take() {
            self.host.files().forget(id);
        }
    }
}

/// A view that fits the photo on its first pass with room.
fn unfitted() -> CanvasView {
    CanvasView::new(egui::Rect::NOTHING, Point::ORIGIN, 1.0)
}

impl ScanTabState {
    /// Load a photo from `bytes`, keeping the previous photo and draft when they are no photo.
    fn load(&mut self, draft: &mut SignatureDraft, bytes: Result<Vec<u8>, String>) {
        let photo = bytes.and_then(|bytes| {
            if bytes.len() > MAX_PHOTO_BYTES {
                return Err(ScanError::Size.to_string());
            }
            SignaturePhoto::from_bytes(&bytes).map_err(|e| e.to_string())
        });
        match photo {
            Ok(photo) => {
                self.error = None;
                self.scan = Some(ScanDraft::new(photo));
                self.preview = None;
                self.shown = None;
                self.view = unfitted();
                self.submit(draft, true);
            }
            Err(e) => self.error = Some(crate::i18n::fmt(tl!("Couldn't use the photo: {e}"), &[("e", &e)])),
        }
    }

    /// The bytes of a picked or dropped file: a path on desktop, the content where the platform gives no path.
    fn file_bytes(&self, file: OsFile) -> Result<Vec<u8>, String> {
        if let Some(path) = &file.path {
            return self.host.files().read(path).map_err(|e| e.to_string());
        }
        match file.bytes {
            Some(bytes) => Ok(bytes.to_vec()),
            None => Err(tl!("The photo could not be read.").to_string()),
        }
    }

    /// Takes the answer of the pick, once it has come. Cancelled and unavailable pickers choose nothing.
    fn take_pick(&mut self, draft: &mut SignatureDraft) {
        let Some(id) = self.pick else { return };
        let Some(picked) = self.host.files().take_answer(id) else { return };
        self.pick = None;
        match picked {
            Picked::Paths(paths) => {
                if let Some(path) = paths.into_iter().next() {
                    let bytes = self.host.files().read(&path).map_err(|e| e.to_string());
                    self.load(draft, bytes);
                }
            }
            Picked::Contents(files) => {
                if let Some((_, bytes)) = files.into_iter().next() {
                    self.load(draft, Ok(bytes));
                }
            }
            // Save answers (a failed write, a download) never come to an open pick.
            Picked::Cancelled | Picked::Unavailable(_) | Picked::Failed(_) | Picked::Downloaded(_) => {}
        }
    }

    /// Queue a scan of the current frame and sensitivity; the draft is not ready until a full one answers.
    fn submit(&mut self, draft: &mut SignatureDraft, full: bool) {
        let Some(frame) = self.scan.as_ref().map(|scan| scan.frame) else { return };
        self.submit_frame(draft, frame, full);
    }

    fn submit_frame(&mut self, draft: &mut SignatureDraft, frame: PhotoRect, full: bool) {
        let Some(scan) = self.scan.as_mut() else { return };
        scan.full = false;
        draft.image = None;
        let (photo, mut req, version) = (scan.photo.clone(), scan.request(full), scan.version);
        req.frame = frame;
        self.request = self.request.wrapping_add(1);
        let request = self.request;
        self.queue.submit(0, Priority::VISIBLE, move |_| (version, request, req, ScanDraft::run(&photo, req)));
    }

    /// Takes only the newest scan for the current photo.
    fn collect(&mut self, draft: &mut SignatureDraft) {
        while let Some(done) = self.queue.try_recv() {
            let (version, request, req, answer) = done.result;
            let Some(scan) = self.scan.as_mut() else { continue };
            if version != scan.version || request != self.request {
                continue;
            }
            scan.apply(&mut self.preview, req, answer);
            self.shown = self.preview.as_ref().and_then(pad_image);
            draft.image = if scan.full { self.shown.clone() } else { None };
        }
    }

    fn show(&mut self, ui: &mut egui::Ui, draft: &mut SignatureDraft, dropped: Option<OsFile>) {
        self.take_pick(draft);
        if let Some(file) = dropped {
            let bytes = self.file_bytes(file);
            self.load(draft, bytes);
        }
        self.collect(draft);

        Text::new(tl!(
            "Sign a sheet of white paper and take a photo of it in bright, even light (use the flash). Choose the photo or drop it here: PdfCraft finds the signature and removes the paper."
        ))
        .shade(Shade::Muted)
        .show(ui);
        let (mut choose, mut turn) = (false, None);
        Row::new().show(ui, |ui, _| {
            choose = clicked(&Button::new(tl!("Choose photo…")).icon(craft::behavior::IconId::new("camera")).show(ui));
            if self.scan.is_some() {
                if clicked(&IconButton::new(craft::behavior::IconId::new("rotate-ccw")).label(tl!("Rotate counterclockwise")).show(ui)) {
                    turn = Some(Turn::CounterClockwise);
                }
                if clicked(&IconButton::new(craft::behavior::IconId::new("rotate-cw")).label(tl!("Rotate clockwise")).show(ui)) {
                    turn = Some(Turn::Clockwise);
                }
            }
        });
        if choose {
            if let Some(old) = self.pick.take() {
                self.host.files().forget(old);
            }
            let request = PickRequest::open("signature-photo").title(tl!("Choose a photo")).filter(tl!("Image"), &["png", "jpg", "jpeg"]);
            self.pick = Some(self.host.files().pick(request));
        }
        if let Some(turn) = turn
            && let Some(scan) = self.scan.as_mut()
        {
            scan.turn(turn);
            self.view = unfitted();
            self.submit(draft, true);
        }
        if let Some(error) = &self.error {
            Notice::new(Tone::Warn, error.clone()).layout(NoticeLayout::Inline).show(ui);
        }

        if self.scan.is_none() {
            Text::new(tl!("No photo chosen")).shade(Shade::Muted).show(ui);
            return;
        }
        self.show_photo(ui, draft);
        self.show_sensitivity(ui, draft);
        if let Some(problem) = self.scan.as_ref().and_then(|s| s.problem.clone()) {
            Notice::new(Tone::Warn, problem).layout(NoticeLayout::Inline).show(ui);
        } else {
            DrawPad::image(tl!("Preview"), self.shown.as_ref()).show(ui);
        }
    }

    /// The photo on a canvas with the frame over it; dragging the frame scans quickly, releasing it scans in full.
    fn show_photo(&mut self, ui: &mut egui::Ui, draft: &mut SignatureDraft) {
        let Some(scan) = self.scan.as_ref() else { return };
        let size = scan.photo.size();
        let (pw, ph) = (f64::from(size[0].max(1)), f64::from(size[1].max(1)));
        let width = ui.available_width();
        let room = ui.allocate_space(egui::vec2(width, width * (ph / pw).min(0.6) as f32)).1;
        let source = PhotoSource { photo: &scan.photo, version: scan.version };
        let crop = crop_of(scan.frame);
        let bounds = DocRect::new(0.0, 0.0, pw, ph);
        let corners = [Point::new(0.0, 0.0), Point::new(pw, 0.0), Point::new(pw, ph), Point::new(0.0, ph)];
        let shown = Canvas::new(tl!("Photo")).view(Bind::bound(&mut self.view)).document(Size::new(pw, ph)).show_at(ui, room, |ui, body| {
            RasterDisplay::new(tl!("Signature photo"), &source).show_at(ui, body.rect);
            CropOverlay::new(tl!("Frame"), Bind::given(crop)).outside(CropOutside::NewFrame).area(corners).bounds(bounds).show_at(ui, body.rect)
        });
        let Some(overlay) = shown.inner else { return };
        let mut commit = false;
        for event in overlay.edit.events.iter() {
            match event {
                EditEvent::Update { value, .. } => {
                    self.submit_frame(draft, frame_of(value.rect, size), false);
                }
                EditEvent::Commit { after, .. } | EditEvent::Set { after, .. } => {
                    if let Some(scan) = self.scan.as_mut() {
                        scan.frame = frame_of(after.rect, size);
                        commit = true;
                    }
                }
                EditEvent::Cancel { .. } => {
                    commit = true;
                }
                _ => {}
            }
        }
        if commit {
            self.submit(draft, true);
        }
    }

    /// The ink sensitivity: quick scans while the slider moves, a full one on its commit.
    fn show_sensitivity(&mut self, ui: &mut egui::Ui, draft: &mut SignatureDraft) {
        let Some(scan) = self.scan.as_ref() else { return };
        let mut value = f64::from(scan.sensitivity);
        let out = SliderField::new(tl!("Ink sensitivity"), Bind::bound(&mut value), 0.0..=100.0).step(1.0).show(ui);
        for event in out.edit.events.iter() {
            let (proposed, full) = match event {
                EditEvent::Update { value, .. } => (*value, false),
                EditEvent::Commit { after, .. } | EditEvent::Set { after, .. } => (*after, true),
                _ => continue,
            };
            if let Some(scan) = self.scan.as_mut() {
                scan.sensitivity = proposed.round().clamp(0.0, 100.0) as u8;
            }
            self.submit(draft, full);
        }
    }
}

/// The Scan tab of the signature pad: choose or drop a photo, frame the signature, adjust the ink. Cleaning runs on
/// the host's jobs, so a pass never waits for a scan. `draft.image` is set only by a full scan of the current frame
/// and sensitivity, and cleared on any change, so the pad's Apply waits for the result.
pub(crate) fn tab(host: Arc<Host>, wake: Wake) -> impl FnMut(&mut egui::Ui, &mut SignatureDraft, craft::review::ui::ScanEvent) + Send + 'static {
    let queue = JobQueue::new(host.jobs_shared().clone(), wake, QueueOptions::default());
    let mut state = ScanTabState { host, scan: None, preview: None, shown: None, pick: None, queue, request: 0, view: unfitted(), error: None };
    move |ui, draft, event| match event {
        craft::review::ui::ScanEvent::Show(dropped) => state.show(ui, draft, dropped),
        craft::review::ui::ScanEvent::Clear => {
            if let Some(id) = state.pick.take() {
                state.host.files().forget(id);
            }
            state.queue.reprioritize(|_, _| None);
            state.request = state.request.wrapping_add(1);
            state.scan = None;
            state.preview = None;
            state.shown = None;
            state.view = unfitted();
            state.error = None;
        }
    }
}
