use super::{
    Color32, CornerRadius, DEFAULT_SENSITIVITY, PREVIEW_MAX_SIDE, PhotoRect, Pos2, SCAN_MAX_SIDE, SavedSig, ScanError, Sense, SigDraft,
    SignatureImage, SignaturePhoto, Stroke, Tokens, Turn, image_preview, pos2, vec2,
};

/// The Scan tab's photo, the frame around the signature and how strongly faint ink is picked up.
#[derive(Clone, Debug, PartialEq)]
pub struct ScanDraft {
    pub photo: SignaturePhoto,
    pub frame: PhotoRect,
    pub sensitivity: u8,
    /// Changes whenever the photo's pixels do (a new photo or a turn), to refresh its texture.
    version: u64,
    pub(super) drag: Option<FrameDrag>,
    /// The draft's image is the full-resolution result, not a quick preview.
    pub(super) full: bool,
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
pub(super) enum FrameDrag {
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

/// The Scan tab above the pad buttons. Returns whether a photo should be chosen.
pub(super) fn scan_tab(
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
        resp.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Image, true, tl!("Photo")));
        return browse;
    };
    resp.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Image, true, tl!("Photo")));
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
