//! PdfCraft patch: translate Wayland's cumulative pinch scale into winit's log deltas.

use crate::event::TouchPhase;

#[derive(Debug)]
pub(crate) struct PinchState<W> {
    window: Option<W>,
    scale: f64,
}

impl<W> Default for PinchState<W> {
    fn default() -> Self {
        Self { window: None, scale: 1.0 }
    }
}

#[derive(Debug)]
pub(crate) struct PinchEvent<W> {
    pub window: W,
    pub phase: TouchPhase,
    pub delta: f64,
}

impl<W: Copy> PinchState<W> {
    pub fn begin(&mut self, window: Option<W>, fingers: u32) -> Option<PinchEvent<W>> {
        *self = Self::default();
        let window = window.filter(|_| fingers == 2)?;
        self.window = Some(window);
        Some(PinchEvent { window, phase: TouchPhase::Started, delta: 0.0 })
    }

    pub fn update(&mut self, scale: f64) -> Option<PinchEvent<W>> {
        let window = self.window?;
        if !scale.is_finite() || scale <= 0.0 {
            return None;
        }
        // egui-winit exponentiates this delta. Use log differences, not subtraction of
        // cumulative scales, so successive events multiply to the compositor's scale.
        let delta = scale.ln() - self.scale.ln();
        // Reject impossible jumps before a consumer exponentiates them as f32.
        if !delta.is_finite() || delta.abs() > 32.0 {
            return None;
        }
        self.scale = scale;
        Some(PinchEvent { window, phase: TouchPhase::Moved, delta })
    }

    pub fn end(&mut self, cancelled: bool) -> Option<PinchEvent<W>> {
        let window = self.window.take()?;
        self.scale = 1.0;
        let phase = if cancelled { TouchPhase::Cancelled } else { TouchPhase::Ended };
        Some(PinchEvent { window, phase, delta: 0.0 })
    }
}
