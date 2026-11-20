//! What the drawing device reported after start-up, and what the app does about it (#606).
//!
//! wgpu sends every error it cannot hand back as a `Result` to one handler installed on the
//! device. With no handler installed, wgpu's default panics "as fatal"; a panic that starts
//! inside a driver's C code cannot unwind, so the process aborts. `catch_errors` (#519)
//! covers only the window's surface and the renderer's first creation, so every later upload
//! — zooming, opening the editor, a font atlas rebuild — died that way (#606, #7111, #6027).
//! This is the handler for the rest of the app's life: out of memory lowers page quality and
//! says so once, on the next frame, instead of closing the app.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use eframe::wgpu;

/// How far quality may be lowered at most: three halvings, so a whole page at 100 % zoom is
/// still several hundred pixels wide — a recognizable picture, not a smudge.
const MAX_LEVEL: usize = 3;

/// What the wgpu error handler writes and the page painter reads, shared through one [`Arc`].
/// The handler runs inside wgpu's own call stack, so it may only remember what happened;
/// `PdfCraftApp::logic` reads the memory one frame later.
#[derive(Default)]
pub struct GpuHealth {
    /// Quality halvings the device's out-of-memory errors have asked for, 0 to [`MAX_LEVEL`].
    level: AtomicUsize,
    /// "The device couldn't allocate" is waiting to be said: once per quality step.
    low_memory_due: AtomicBool,
    /// A non-memory device error is waiting to be said, and one has already been said, so a
    /// driver that complains every frame doesn't complain at the user every frame.
    device_error_due: AtomicBool,
    device_error_told: AtomicBool,
    /// Whether the handler has gone on a device yet. PdfCraft gets one drawing device from
    /// eframe, so the device of the first frame is the device of the whole session; a device
    /// recreated mid-session would keep wgpu's default handler.
    installed: AtomicBool,
}

impl GpuHealth {
    /// Puts the error handler on `device`, the first time it is asked. PdfCraft calls this
    /// from `PdfCraftApp::logic` on the first frame, before that frame uploads any page.
    pub fn observe(self: &Arc<Self>, device: &wgpu::Device) {
        if self.installed.swap(true, Ordering::Relaxed) {
            return;
        }
        let health = Arc::clone(self);
        device.on_uncaptured_error(Arc::new(move |error| note(&health, &error)));
    }

    /// Drawing quality as a factor of the scale the zoom asks for: 1.0 is normal, then 1/2,
    /// 1/4 and 1/8. The page painter multiplies its raster requests by this
    /// ([`canvas::DocView::set_gpu_quality`]).
    pub fn quality(&self) -> f32 {
        match self.level.load(Ordering::Relaxed) {
            0 => 1.0,
            1 => 0.5,
            2 => 0.25,
            _ => 0.125,
        }
    }

    /// Takes the one message waiting, if any. The app shows it in the toast; wgpu's error
    /// text itself goes to the log, which is where reporters of #519 found the last one.
    pub fn take_notice(&self) -> Option<GpuNotice> {
        if self.low_memory_due.swap(false, Ordering::Relaxed) {
            Some(GpuNotice::LowMemory)
        } else if self.device_error_due.swap(false, Ordering::Relaxed) {
            Some(GpuNotice::DeviceError)
        } else {
            None
        }
    }

    /// Lowers quality one step after the device couldn't allocate; true when the step is new.
    /// The step check is all the arbitration there is: two errors at once is one step, so
    /// the same failure repeating every frame doesn't spam the message. The exchange is
    /// strong because a spurious weak failure would silently drop this error's step until
    /// another out-of-memory error came.
    fn raise_after_oom(&self) -> bool {
        let old = self.level.load(Ordering::Relaxed);
        let new = old.saturating_add(1).min(MAX_LEVEL);
        new != old && self.level.compare_exchange(old, new, Ordering::Relaxed, Ordering::Relaxed).is_ok()
    }
}

/// The message a device error left for the app to show.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GpuNotice {
    /// The device couldn't allocate; drawing now asks for less, and said so.
    LowMemory,
    /// The device reported a non-memory error; it is only reported, once.
    DeviceError,
}

impl GpuNotice {
    /// English text; `PdfCraftApp::notify_tr` translates it for the toast.
    pub fn message(self) -> &'static str {
        match self {
            Self::LowMemory => "PdfCraft ran out of drawing memory, so pages are shown at a lower quality to keep the app running.",
            Self::DeviceError => "The graphics processor reported an error. Drawing may look wrong until PdfCraft is restarted.",
        }
    }
}

/// What one wgpu error asks for. Readable apart from the handler so a test can check every
/// kind without a graphics card.
fn note(health: &GpuHealth, error: &wgpu::Error) {
    log::error!("wgpu: {error}");
    match error {
        wgpu::Error::OutOfMemory { .. } => {
            if health.raise_after_oom() {
                health.low_memory_due.store(true, Ordering::Relaxed);
            }
        }
        // Validation and internal errors are not a shortage of memory, and the app cannot
        // recreate the device eframe owns, so lowering quality would only make the screen
        // soft for nothing: these are reported, once. A device that is lost at all goes to
        // wgpu's own device-lost callback, not here (wgpu 30), and that is left as it is.
        _ => {
            if !health.device_error_told.swap(true, Ordering::Relaxed) {
                health.device_error_due.store(true, Ordering::Relaxed);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io;

    /// Builds an error of each kind wgpu raises, so [`note`]'s decisions can be checked
    /// without a graphics card. `wgpu::Error` is not `Clone`, so each kind needs its own.
    fn error(kind: u8) -> wgpu::Error {
        let source = io::Error::other("out of memory on a test machine").into();
        match kind {
            0 => wgpu::Error::OutOfMemory { source },
            1 => wgpu::Error::Validation { source, description: "a test".into() },
            _ => wgpu::Error::Internal { source, description: "a test".into() },
        }
    }

    #[test]
    fn out_of_memory_lowers_quality_once_per_step() {
        let health = GpuHealth::default();
        assert_eq!(health.quality(), 1.0);
        for (wanted, step) in [(0.5, 1), (0.25, 2), (0.125, 3)] {
            note(&health, &error(0));
            assert_eq!(
                (health.quality(), health.take_notice(), step),
                (wanted, Some(GpuNotice::LowMemory), step),
                "the {step} halving must come with its own message"
            );
        }
        // At the floor, and once the message has been said, the same failure repeating every
        // frame stays quiet: nothing to lower further, nothing new to say.
        note(&health, &error(0));
        assert_eq!(health.quality(), 0.125, "one-eighth is the floor");
        assert_eq!(health.take_notice(), None);
    }

    #[test]
    fn other_errors_are_said_once_and_do_not_lower_quality() {
        let health = GpuHealth::default();
        note(&health, &error(1));
        assert_eq!(health.quality(), 1.0, "a validation error is not a shortage of memory");
        assert_eq!(health.take_notice(), Some(GpuNotice::DeviceError));
        // An internal error later says nothing: the user has already heard about the device.
        note(&health, &error(2));
        assert_eq!(health.take_notice(), None);
        // Out of memory is its own notice: taking one does not swallow the other.
        note(&health, &error(0));
        assert_eq!(health.take_notice(), Some(GpuNotice::LowMemory));
    }
}
