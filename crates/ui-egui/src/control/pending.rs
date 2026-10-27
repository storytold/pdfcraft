use super::screenshot::{Geometry, MAX_IMAGE_BYTES, retried_reservation, screenshot_png};
use super::*;

/// A screenshot pending in the `finish_pending` pass, with ownership of its reply.
struct PendingCapture {
    slot: craft_ui::capture::CaptureSlot,
    callback: Option<egui::ScreenshotCallback>,
    region: Option<egui::Rect>,
    reply: Sender<Reply>,
    since: web_time::Instant,
    geometry: Geometry,
    reserved: usize,
    attempts: u8,
}

impl PendingCapture {
    /// Convert a screenshot request into the pending form that later frames finish.
    fn into_pending(self) -> Pending {
        Pending::Screenshot {
            slot: self.slot,
            callback: self.callback,
            region: self.region,
            reply: self.reply,
            since: self.since,
            geometry: self.geometry,
            reserved: self.reserved,
            attempts: self.attempts,
        }
    }
}

/// The state one `finish_pending` pass needs while resolving pending replies.
struct PendingFinisher {
    now: web_time::Instant,
    passes: u64,
    current: Geometry,
    in_pass: bool,
    dropped: Vec<u64>,
    expired: Vec<u64>,
    reserved_total: usize,
}

impl PendingFinisher {
    /// Answer or keep one queued-input request, preserving its one arrival deadline.
    fn resolve_frames(&self, until_pass: u64, batches: Vec<u64>, since: web_time::Instant, reply: Sender<Reply>, value: Value) -> Option<Pending> {
        if batches.iter().any(|id| self.expired.contains(id)) {
            let _ = reply.send(Err(not_drawn("handling the input")));
            return None;
        }
        if batches.iter().any(|id| self.dropped.contains(id)) {
            let _ = reply.send(Err("the document, dialog or focus changed before the input could be applied; nothing was done".into()));
            return None;
        }
        if self.passes >= until_pass {
            let _ = reply.send(Ok(value));
            return None;
        }
        if !self.in_pass && self.now.duration_since(since) > timeout() {
            let _ = reply.send(Err(not_drawn("handling the input")));
            return None;
        }
        Some(Pending::Frames { until_pass, batches, since, reply, value })
    }

    /// Give one reservation back to the running aggregate budget.
    fn release_reservation(&mut self, reserved: usize) {
        self.reserved_total = self.reserved_total.saturating_sub(reserved);
    }

    /// Resolve one screenshot request, keeping it only while its capture is still pending.
    fn finish_capture(&mut self, ctx: &egui::Context, capture: PendingCapture) -> Option<Pending> {
        // Completion cannot restart or bypass the admission deadline, even when the callback
        // arrived before this delayed tick or a resize would retry it.
        if self.now.duration_since(capture.since) > timeout() {
            capture.slot.cancel();
            self.release_reservation(capture.reserved);
            let _ = capture.reply.send(Err(not_drawn("the screenshot")));
            return None;
        }
        match capture.slot.take() {
            Some(Ok(image)) => self.finish_captured_image(ctx, capture, image),
            Some(Err(error)) => {
                self.release_reservation(capture.reserved);
                let _ = capture.reply.send(Err(format!("the screenshot could not be captured: {error}")));
                None
            }
            None => Some(capture.into_pending()),
        }
    }

    /// Finish a screenshot whose callback produced an image.
    fn finish_captured_image(&mut self, ctx: &egui::Context, capture: PendingCapture, image: Arc<egui::ColorImage>) -> Option<Pending> {
        if self.current.epoch != capture.geometry.epoch {
            // The document/dialog changed: never save a capture of the previous context, and do
            // not retry it into the new one.
            self.release_reservation(capture.reserved);
            let _ = capture.reply.send(Err("the document or dialog changed while the screenshot was being captured; it was not saved".into()));
            return None;
        }
        if self.current.size != capture.geometry.size || self.current.pixels_per_point != capture.geometry.pixels_per_point {
            // A late completion after a resize/DPI change is never encoded: retry once with a
            // fresh slot, else report a defined error.
            return self.retry_captured_image(ctx, capture);
        }
        self.release_reservation(capture.reserved);
        let _ = capture.reply.send(screenshot_png(&image, capture.region, capture.geometry));
        None
    }

    /// Retry one resize/DPI-stale capture with the current geometry.
    fn retry_captured_image(&mut self, ctx: &egui::Context, capture: PendingCapture) -> Option<Pending> {
        if capture.attempts >= 1 {
            self.release_reservation(capture.reserved);
            let _ = capture.reply.send(Err("the window geometry changed while the screenshot was being captured; it was not saved".into()));
            return None;
        }
        if capture.region.as_ref().is_some_and(|region| !self.current.contains_region(*region)) {
            self.release_reservation(capture.reserved);
            let _ = capture.reply.send(Err("the region must lie inside the window".into()));
            return None;
        }
        let Some(bytes) = self.current.image_bytes() else {
            self.release_reservation(capture.reserved);
            let _ = capture.reply.send(Err("the window has no capturable size".into()));
            return None;
        };
        let Some(new_total) = retried_reservation(self.reserved_total, capture.reserved, bytes) else {
            self.release_reservation(capture.reserved);
            let _ = capture.reply.send(Err("the screenshot no longer fits the screenshot budget".into()));
            return None;
        };
        self.reserved_total = new_total;
        let (slot, callback) = craft_ui::capture::CaptureSlot::new(ctx, egui::ViewportId::ROOT, MAX_IMAGE_BYTES);
        Some(Pending::Screenshot {
            slot,
            callback: Some(callback),
            region: capture.region,
            reply: capture.reply,
            since: capture.since,
            geometry: self.current,
            reserved: bytes,
            attempts: capture.attempts + 1,
        })
    }
}

impl Control {
    /// Resolve completed frame and screenshot requests while retaining those that still need a pass or a
    /// retry.
    pub(super) fn finish_pending(&mut self, ctx: &egui::Context, host: &mut impl Host) {
        let now = web_time::Instant::now();
        let (dropped, expired, in_pass) = self.take_finished_shared(ctx, host, now);
        let mut finisher = PendingFinisher {
            now,
            passes: self.passes(),
            current: Geometry::current(ctx, host.capture_epoch()),
            in_pass,
            dropped,
            expired,
            reserved_total: self.reserved_bytes(),
        };
        let mut keep = Vec::new();
        for pending in std::mem::take(&mut self.pending) {
            let kept = match pending {
                Pending::Frames { until_pass, batches, since, reply, value } => finisher.resolve_frames(until_pass, batches, since, reply, value),
                Pending::Screenshot { slot, callback, region, reply, since, geometry, reserved, attempts } => {
                    finisher.finish_capture(ctx, PendingCapture { slot, callback, region, reply, since, geometry, reserved, attempts })
                }
            };
            if let Some(pending) = kept {
                keep.push(pending);
            }
        }
        self.pending = keep;
    }
}
