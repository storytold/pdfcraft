use super::{Control, Host, Pending, Reply, Sender, json};

/// Screenshot admission limits: bounded requests, per-image bytes and aggregate retained bytes.
const MAX_PENDING_SCREENSHOTS: usize = 4;

pub(super) const MAX_IMAGE_BYTES: usize = 128 * 1024 * 1024;

const MAX_TOTAL_BYTES: usize = 256 * 1024 * 1024;

/// One image's encode working copies are bounded too: the RGBA buffer fed to the PNG encoder,
/// the encoded PNG, and the base64 string in the reply. They are made one capture at a time,
/// after the retained image budget above has been checked. Peak encoding memory is therefore at
/// most the retained aggregate plus one crop, one PNG and one base64 string; it is not hidden
/// behind the retained-image limit.
const MAX_ENCODED_BYTES: usize = MAX_IMAGE_BYTES;

const MAX_BASE64_BYTES: usize = 192 * 1024 * 1024;

/// Replace one pending capture's reservation with `bytes`, keeping the other reservations.
///
/// The drain retries captures one at a time: using a single total taken before the loop would
/// let several retries each see room that no longer exists together, and the aggregate would
/// grow past [`MAX_TOTAL_BYTES`].
pub(super) fn retried_reservation(total: usize, old: usize, bytes: usize) -> Option<usize> {
    let without_this = total.saturating_sub(old);
    let new_total = without_this.saturating_add(bytes);
    (bytes <= MAX_IMAGE_BYTES && new_total <= MAX_TOTAL_BYTES).then_some(new_total)
}

/// The window content a screenshot request was admitted and issued for.
///
/// Only the viewport's local size and scale matter: the window's position on the monitor does not
/// change a single captured pixel, and keyboard focus is not part of the captured content.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct Geometry {
    pub(super) size: egui::Vec2,
    pub(super) pixels_per_point: f32,
    /// Active document and open dialog/prompt identity, never the focused widget.
    pub(super) epoch: u64,
}

impl Geometry {
    /// Capture the current viewport size and effective pixels-per-point for the request epoch.
    pub(super) fn current(ctx: &egui::Context, epoch: u64) -> Self {
        // Viewport info is refreshed by `run_logic`; the last UI pass's `viewport_rect` can be
        // stale while the window is hidden. Fall back to it only when the host cannot say. Only
        // its size is kept: `inner_rect` is in monitor space and its origin may move.
        let (rect, native) = ctx.input(|i| {
            let viewport = i.viewport();
            (viewport.inner_rect.unwrap_or_else(|| i.viewport_rect()), viewport.native_pixels_per_point)
        });
        let native = native.or_else(|| ctx.native_pixels_per_point()).unwrap_or(1.0);
        Self { size: rect.size(), pixels_per_point: native * ctx.zoom_factor(), epoch }
    }

    /// Checked bytes an image of this geometry needs, before the capture is issued.
    pub(super) fn image_bytes(self) -> Option<usize> {
        let width = (self.size.x * self.pixels_per_point).ceil();
        let height = (self.size.y * self.pixels_per_point).ceil();
        if !(width.is_finite() && height.is_finite()) || width <= 0.0 || height <= 0.0 {
            return None;
        }
        checked_image_bytes(width as usize, height as usize)
    }

    /// Whether `region` is a non-empty rectangle inside this viewport, in logical points.
    pub(super) fn contains_region(self, region: egui::Rect) -> bool {
        region.min.x.is_finite()
            && region.min.y.is_finite()
            && region.max.x.is_finite()
            && region.max.y.is_finite()
            && region.min.x >= 0.0
            && region.min.y >= 0.0
            && region.max.x > region.min.x
            && region.max.y > region.min.y
            && region.max.x <= self.size.x
            && region.max.y <= self.size.y
    }
}

/// Return the byte size of an RGBA image of the given dimensions, or `None` when the multiplication
/// overflows.
fn checked_image_bytes(width: usize, height: usize) -> Option<usize> {
    width.checked_mul(height)?.checked_mul(core::mem::size_of::<egui::Color32>())
}

impl Control {
    /// Return the total screenshot bytes reserved by pending captures without overflowing.
    pub(super) fn reserved_bytes(&self) -> usize {
        self.pending
            .iter()
            .filter_map(|p| if let Pending::Screenshot { reserved, .. } = p { Some(*reserved) } else { None })
            .fold(0usize, usize::saturating_add)
    }

    /// Count the screenshot requests still waiting for a capture.
    fn pending_screenshots(&self) -> usize {
        self.pending.iter().filter(|p| matches!(p, Pending::Screenshot { .. })).count()
    }

    /// Admit one screenshot request within the count, per-image and aggregate limits.
    pub(super) fn admit_screenshot(
        &mut self,
        ctx: &egui::Context,
        host: &mut impl Host,
        region: Option<egui::Rect>,
        reply: Sender<Reply>,
        since: web_time::Instant,
    ) {
        if self.pending_screenshots() >= MAX_PENDING_SCREENSHOTS {
            let _ = reply.send(Err(format!("too many screenshots are already pending (limit {MAX_PENDING_SCREENSHOTS})")));
            return;
        }
        let geometry = Geometry::current(ctx, host.capture_epoch());
        let Some(bytes) = geometry.image_bytes() else {
            let _ = reply.send(Err("the window has no capturable size".into()));
            return;
        };
        if let Some(region) = region
            && !geometry.contains_region(region)
        {
            let _ = reply.send(Err("the region must lie inside the window".into()));
            return;
        }
        if bytes > MAX_IMAGE_BYTES {
            let _ = reply.send(Err(format!("the screenshot would use {bytes} bytes, over the {MAX_IMAGE_BYTES}-byte image limit")));
            return;
        }
        if self.reserved_bytes().saturating_add(bytes) > MAX_TOTAL_BYTES {
            let _ = reply.send(Err(format!("the screenshot would exceed the {MAX_TOTAL_BYTES}-byte aggregate screenshot budget")));
            return;
        }
        let (slot, callback) = craft_ui::capture::CaptureSlot::new(ctx, egui::ViewportId::ROOT, MAX_IMAGE_BYTES);
        self.pending.push(Pending::Screenshot { slot, callback: Some(callback), region, reply, since, geometry, reserved: bytes, attempts: 0 });
    }
}

/// Encode (a region of) a screenshot as PNG; the reply carries base64 data and the size.
///
/// The image scale is derived from the returned pixels and the logical viewport the capture was
/// admitted for, never from the current `Context`: egui-wgpu's surface fit may have shrunk the
/// surface (and a resize/DPI change has already been rejected before this call).
pub(super) fn screenshot_png(image: &egui::ColorImage, region: Option<egui::Rect>, geometry: Geometry) -> Reply {
    let [w, h] = image.size;
    let view = geometry.size;
    if w == 0 || h == 0 {
        return Err("the screenshot has no pixels".into());
    }
    if !(view.x.is_finite() && view.y.is_finite()) || view.x <= 0.0 || view.y <= 0.0 {
        return Err("the window has no capturable size".into());
    }
    if region.is_some_and(|region| !geometry.contains_region(region)) {
        return Err("the region must lie inside the window".into());
    }
    let scale_x = w as f32 / view.x;
    let scale_y = h as f32 / view.y;
    // A fitted surface shrinks both sides by one factor; a different x/y scale means the window
    // changed shape and this image cannot be cropped honestly.
    if (scale_x - scale_y).abs() > 0.01 * scale_x.max(1.0) {
        return Err("the screenshot's aspect ratio does not match the window".into());
    }
    let (x0, y0, x1, y1) = match region {
        Some(r) => {
            let px = |v: f32, scale: f32, max: usize| ((v * scale).round().max(0.0) as usize).min(max);
            (px(r.min.x, scale_x, w), px(r.min.y, scale_y, h), px(r.max.x, scale_x, w), px(r.max.y, scale_y, h))
        }
        None => (0, 0, w, h),
    };
    if x1 <= x0 || y1 <= y0 {
        return Err("the region is outside the window".into());
    }
    let crop_bytes = (x1 - x0)
        .checked_mul(y1 - y0)
        .and_then(|pixels| pixels.checked_mul(core::mem::size_of::<egui::Color32>()))
        .ok_or_else(|| "the screenshot region is too large".to_string())?;
    if crop_bytes > MAX_ENCODED_BYTES {
        return Err(format!("the screenshot region would need {crop_bytes} bytes to encode, over the {MAX_ENCODED_BYTES}-byte encode limit"));
    }
    let mut rgba = Vec::with_capacity(crop_bytes);
    for y in y0..y1 {
        for px in &image.pixels[y * w + x0..y * w + x1] {
            rgba.extend_from_slice(&px.to_srgba_unmultiplied());
        }
    }
    let (cw, ch) = ((x1 - x0) as u32, (y1 - y0) as u32);
    let mut out = Vec::new();
    let mut enc = png::Encoder::new(&mut out, cw, ch);
    enc.set_color(png::ColorType::Rgba);
    enc.set_depth(png::BitDepth::Eight);
    let mut writer = enc.write_header().map_err(|e| e.to_string())?;
    writer.write_image_data(&rgba).map_err(|e| e.to_string())?;
    writer.finish().map_err(|e| e.to_string())?;
    if out.len() > MAX_ENCODED_BYTES {
        return Err(format!("the encoded screenshot is {} bytes, over the {MAX_ENCODED_BYTES}-byte encode limit", out.len()));
    }
    let encoded = out.len().div_ceil(3).checked_mul(4).ok_or_else(|| "the encoded screenshot is too large".to_string())?;
    if encoded > MAX_BASE64_BYTES {
        return Err(format!("the screenshot's base64 reply would be {encoded} bytes, over the {MAX_BASE64_BYTES}-byte limit"));
    }
    use base64::Engine as _;
    Ok(json!({
        "png_base64": base64::engine::general_purpose::STANDARD.encode(out),
        "width": cw,
        "height": ch,
        "pixels_per_point": scale_x,
    }))
}

#[cfg(test)]
mod screenshot_geometry_tests {
    use super::*;

    #[test]
    fn image_bytes_are_checked_against_overflow_and_limits() {
        assert_eq!(checked_image_bytes(2, 3), Some(24));
        assert_eq!(checked_image_bytes(usize::MAX, 2), None);
        let big = Geometry { size: egui::vec2(10_000.0, 10_000.0), pixels_per_point: 8.0, epoch: 0 };
        assert!(big.image_bytes().is_some_and(|bytes| bytes > MAX_IMAGE_BYTES));
    }

    /// The crop is derived from the returned pixels and the admitted logical rect, not from a
    /// current `pixels_per_point`: a fitted surface may return a smaller image.
    #[test]
    fn region_crop_uses_the_capture_geometry() {
        let image = egui::ColorImage::filled([200, 100], egui::Color32::RED);
        let geometry = Geometry { size: egui::vec2(100.0, 50.0), pixels_per_point: 2.0, epoch: 7 };
        let reply =
            screenshot_png(&image, Some(egui::Rect::from_min_max(egui::pos2(10.0, 5.0), egui::pos2(60.0, 30.0))), geometry).expect("a valid crop");
        assert_eq!(reply["width"], 100);
        assert_eq!(reply["height"], 50);
        assert_eq!(reply["pixels_per_point"], 2.0);
    }

    /// A retry replaces one reservation in a running aggregate. Three 50 MiB captures that each
    /// retry to 100 MiB must not all be admitted against a stale 150 MiB snapshot.
    #[test]
    fn retries_keep_one_running_aggregate_budget() {
        const MIB: usize = 1024 * 1024;
        let first = retried_reservation(3 * 50 * MIB, 50 * MIB, 100 * MIB).expect("the first retry fits");
        let second = retried_reservation(first, 50 * MIB, 100 * MIB).expect("the second retry fits");
        assert_eq!(second, 250 * MIB);
        assert_eq!(retried_reservation(second, 50 * MIB, 100 * MIB), None, "the third would pass 256 MiB");
        assert_eq!(retried_reservation(0, 0, MAX_IMAGE_BYTES + 1), None, "a single image over the per-image limit never fits");
    }

    #[test]
    fn an_image_with_another_aspect_ratio_is_rejected() {
        let image = egui::ColorImage::filled([200, 100], egui::Color32::RED);
        let geometry = Geometry { size: egui::vec2(100.0, 60.0), pixels_per_point: 2.0, epoch: 0 };
        assert!(screenshot_png(&image, None, geometry).is_err());
    }

    #[test]
    fn an_outside_region_is_rejected_not_panicking() {
        let image = egui::ColorImage::filled([20, 10], egui::Color32::RED);
        let geometry = Geometry { size: egui::vec2(10.0, 5.0), pixels_per_point: 2.0, epoch: 0 };
        assert!(screenshot_png(&image, Some(egui::Rect::from_min_max(egui::pos2(11.0, 6.0), egui::pos2(15.0, 9.0))), geometry).is_err());
    }

    /// A region that a shrink now cuts off is refused, not silently clamped to a smaller crop.
    #[test]
    fn a_region_cut_by_a_shrink_is_rejected_instead_of_cropped() {
        let image = egui::ColorImage::filled([20, 10], egui::Color32::RED);
        let shrunk = Geometry { size: egui::vec2(10.0, 5.0), pixels_per_point: 2.0, epoch: 0 };
        let region = egui::Rect::from_min_max(egui::pos2(8.0, 4.0), egui::pos2(15.0, 9.0));
        assert!(!shrunk.contains_region(region));
        let error = screenshot_png(&image, Some(region), shrunk).expect_err("a cut region must not become a smaller image");
        assert!(error.contains("inside the window"), "{error}");
    }

    /// Screenshot identity is the captured content, not the window's monitor-space origin or the
    /// focused widget: only the capture epoch and the viewport-local size/scale matter.
    #[test]
    fn capture_geometry_compares_size_scale_and_capture_epoch_only() {
        let admitted = Geometry { size: egui::vec2(800.0, 600.0), pixels_per_point: 2.0, epoch: 5 };
        assert_eq!(admitted, Geometry { size: egui::vec2(800.0, 600.0), pixels_per_point: 2.0, epoch: 5 });
        assert_ne!(admitted, Geometry { size: egui::vec2(801.0, 600.0), pixels_per_point: 2.0, epoch: 5 });
        assert_ne!(admitted, Geometry { size: egui::vec2(800.0, 600.0), pixels_per_point: 1.5, epoch: 5 });
        assert_ne!(admitted, Geometry { size: egui::vec2(800.0, 600.0), pixels_per_point: 2.0, epoch: 6 });
        assert!(admitted.contains_region(egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(800.0, 600.0))));
        assert!(!admitted.contains_region(egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(801.0, 600.0))));
        assert!(!admitted.contains_region(egui::Rect::from_min_max(egui::pos2(-1.0, 0.0), egui::pos2(10.0, 10.0))));
    }
}
