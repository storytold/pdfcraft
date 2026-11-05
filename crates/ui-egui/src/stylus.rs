//! The stylus, where the pages ask for it: which tool the fresh press on them is.
//!
//! winit's W28 tablet translation reproduces a mouse on purpose: presses arrive as plain
//! `PointerButton` primaries, indistinguishable from a mouse's, so the pointer pipeline draws
//! unchanged. The tool kind cannot ride those events — eframe's web (egui-winit) translates a
//! closed set of winit events — so winit keeps the small channel `winit::tablet::contact_tool`
//! instead, and the pages ask here beside the press they are deciding on: an eraser end's
//! contact erases with whatever comment tool is selected (Acrobat's eraser end), everything
//! else keeps the selected tool.
//!
//! On wasm there is no winit (eframe's web app runs without it), and a browser pen is a
//! pointer like a mouse's: every press keeps the selected tool there.
//!
//! The channel is tool metadata, not a button: presses are never re-mapped, and only a fresh
//! press (each contact starts its own gesture) is staged, so a gesture begun with one tool
//! runs to its end on the tool it began with — a contact that lifted or a switch between the
//! pen's ends mid-stroke does not flip the tool under an open gesture.
//!
//! The pressure side lives here too (`PressureTrack` and the mapping it runs): the raw
//! reports the channel carries (`winit::tablet::sole_contact`, one fixed-capacity window per
//! contact) stay raw — the track folds the reports not yet consumed into a smoothed force and
//! maps it into the width band the `PenPressure` configuration defines, and the drawing code
//! sees nothing but width factors. A tool (or driver) reporting no pressure, pressure off in
//! the preferences and every wasm build see width factor 1.0: the configured fixed width.

use crate::page_pointer::PagePointer;
use crate::state::QuickTool;
#[cfg(target_arch = "wasm32")]
use crate::tools::comments::PenPressure;
#[cfg(not(target_arch = "wasm32"))]
use crate::tools::comments::{CommentTool, PenPressure};

/// The tool the pages' fresh press acts with. A press the channel attributes to an eraser
/// end erases; every other press keeps `tool`.
#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn press_tool(tool: QuickTool, ptr: &PagePointer) -> QuickTool {
    press_tool_with(ptr.fresh_press(), tool, winit::tablet::contact_tool())
}

/// No winit on the web (eframe's web app runs without it): no channel, and nothing the
/// pages could attribute — a browser pen is a pointer, so every press keeps the tool.
#[cfg(target_arch = "wasm32")]
pub(crate) fn press_tool(tool: QuickTool, _ptr: &PagePointer) -> QuickTool {
    tool
}

/// The channel-free core, so the tests drive every freedom without touching the channel's own
/// process state. `None`/`Mixed` contacts make no eraser claim and keep the selected tool.
#[cfg(not(target_arch = "wasm32"))]
fn press_tool_with(fresh_press: bool, tool: QuickTool, contact: winit::tablet::ContactTool) -> QuickTool {
    if fresh_press && matches!(contact, winit::tablet::ContactTool::Eraser) { QuickTool::Comment(CommentTool::Eraser) } else { tool }
}

/// One drawing gesture's track through the pressure stream.
///
/// It counts which reports it has already folded (the marker `seq`), so a frame's read adds
/// only the newer ones, and carries the smoothed force between frames. The track owns no
/// reports — the channel's per-contact window does — so a contact that lifts or a tool that
/// is removed cannot leave reports behind: the next gesture starts from a fresh track, and a
/// vanished contact is simply not consulted again.
///
/// A frame without a nameable contact keeps the folded value: the width holds where the last
/// report put it instead of guessing (the channel hides the tool kind there the same way).
///
/// The type rides the public `Gesture::Draw` variant inside the gesture it advances; its
/// state and half of its behavior are crate internals.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct PressureTrack {
    /// The first report sequence the track has not folded yet.
    seq: u64,
    /// The smoothed force so far, 0–1; `None` until a report has arrived.
    smoothed: Option<f64>,
}

impl PressureTrack {
    /// The width a point captured now draws with: `1.0` (the tool's configured fixed width)
    /// whenever pressure is off in the preferences; otherwise the smoothed force of the new
    /// reports the channel holds, mapped into the configured band.
    pub(crate) fn point_width(&mut self, pressure: &PenPressure) -> f64 {
        if !pressure.on {
            return 1.0;
        }
        #[cfg(not(target_arch = "wasm32"))]
        {
            self.see(pressure, sole_reports().as_deref())
        }
        #[cfg(target_arch = "wasm32")]
        {
            self.see(pressure, None)
        }
    }

    /// The pure half: fold a channel read — no contact (not one, or not nameable, `None`) or
    /// the contact's kept reports as `(seq, force)` pairs, force normalized to 0–1 — smooth,
    /// and map into the configured band. Tests drive every case without process state.
    pub(crate) fn see(&mut self, pressure: &PenPressure, read: Option<&[(u64, f64)]>) -> f64 {
        let Some(read) = read else { return self.take(pressure) };
        for (seq, force) in read {
            if *seq < self.seq {
                continue;
            }
            self.smoothed = Some(match self.smoothed {
                Some(previous) => pressure.smoothing * force.clamp(0.0, 1.0) + (1.0 - pressure.smoothing) * previous,
                None => force.clamp(0.0, 1.0),
            });
            self.seq = seq.saturating_add(1);
        }
        self.take(pressure)
    }

    /// What the folded state says now, shared by the no-report answers: absent reports keep
    /// what the width last stood at.
    fn take(&self, pressure: &PenPressure) -> f64 {
        self.smoothed.map(|p| pressure.width_scale(p)).unwrap_or(1.0)
    }
}

/// One channel read, in the terms the track folds — a pressure-less tool arrives with no
/// reports, the empty case, which keeps the configured fixed width.
#[cfg(not(target_arch = "wasm32"))]
fn sole_reports() -> Option<Vec<(u64, f64)>> {
    winit::tablet::sole_contact().map(|contact| {
        contact
            .samples
            .iter()
            .map(|sample| {
                let force = match sample.force {
                    winit::event::Force::Normalized(force) => force.clamp(0.0, 1.0),
                    // Only our Wayland backend writes the channel and normalizes itself; a
                    // calibrated force (another backend arriving later) still maps honestly.
                    winit::event::Force::Calibrated { force, max_possible_force, .. } => force / max_possible_force.max(f64::EPSILON),
                };
                (sample.seq, force.clamp(0.0, 1.0))
            })
            .collect()
    })
}

#[cfg(all(not(target_arch = "wasm32"), test))]
mod tests {
    use super::press_tool_with;
    use crate::state::QuickTool;
    use crate::tools::comments::CommentTool;
    use winit::tablet::ContactTool;

    /// Without the channel's eraser, a press is exactly the selected tool — the pen draws with
    /// what it drew before this fix.
    #[test]
    fn other_contacts_keep_the_selected_tool() {
        let tool = QuickTool::Comment(CommentTool::Ink);
        for contact in [ContactTool::None, ContactTool::Pen, ContactTool::Mixed] {
            assert_eq!(press_tool_with(true, tool, contact), tool, "{contact:?} keeps the selected tool");
        }
        // Nothing fresh to attribute: a hover or a continue-does-not-re-decide frame keeps it too.
        for contact in [ContactTool::None, ContactTool::Pen, ContactTool::Mixed, ContactTool::Eraser] {
            assert_eq!(press_tool_with(false, tool, contact), tool, "no fresh press keeps the selected tool ({contact:?})");
        }
    }

    /// The eraser end erases with every tool selected, and stages nothing without a fresh
    /// press: each contact begins its erase as its own gesture.
    #[test]
    fn an_eraser_contact_presses_with_the_eraser() {
        for tool in [
            QuickTool::Select,
            QuickTool::Comment(CommentTool::Ink),
            QuickTool::Comment(CommentTool::FreehandHighlight),
            QuickTool::Comment(CommentTool::Line),
            QuickTool::Comment(CommentTool::Rectangle),
        ] {
            assert_eq!(
                press_tool_with(true, tool, ContactTool::Eraser),
                QuickTool::Comment(CommentTool::Eraser),
                "{tool:?} erases from the eraser end"
            );
        }
        assert_eq!(
            press_tool_with(false, QuickTool::Comment(CommentTool::Ink), ContactTool::Eraser),
            QuickTool::Comment(CommentTool::Ink),
            "the gesture keeps the tool it began with"
        );
    }

    /// The pressure side's contract, driven purely through [`PressureTrack::see`] so no test
    /// touches the channel's process state: `on: true` and reports as `(seq, force)` pairs.
    mod track {
        use crate::stylus::PressureTrack;
        use crate::tools::comments::PenPressure;

        fn pen() -> PenPressure {
            PenPressure { on: true, ..PenPressure::default() }
        }

        #[test]
        fn pressure_off_maps_to_the_fixed_width() {
            let mut track = PressureTrack::default();
            assert_eq!(track.point_width(&PenPressure::default()), 1.0, "`on: false` never reads the channel");
        }

        #[test]
        fn the_first_report_sets_the_width_its_force_names() {
            let mut track = PressureTrack::default();
            assert_eq!(track.see(&pen(), Some(&[(0, 0.0)])), PenPressure::default().min);
            track = PressureTrack::default();
            assert_eq!(track.see(&pen(), Some(&[(0, 1.0)])), PenPressure::default().max);
        }

        #[test]
        fn the_band_orders_itself_however_the_knobs_arrive() {
            let pen = PenPressure::clamped(true, 1.2, 0.5, 1.0);
            let mut track = PressureTrack::default();
            assert_eq!(track.see(&pen, Some(&[(0, 0.0)])), 1.2);
            assert_eq!(track.see(&pen, Some(&[(1, 1.0)])), 0.5);
        }

        #[test]
        fn smoothing_blends_the_newest_report_into_the_fold() {
            // smoothing 0.35: the first report is taken whole, the next blends into it.
            let pen = PenPressure { smoothing: 0.35, ..pen() };
            let mut track = PressureTrack::default();
            track.see(&pen, Some(&[(0, 1.0)]));
            let alpha = pen.smoothing;
            assert_eq!(track.see(&pen, Some(&[(1, 0.0)])), pen.width_scale(alpha * 0.0 + (1.0 - alpha) * 1.0));
        }

        #[test]
        fn a_read_older_than_the_marker_changes_nothing() {
            let pen = pen();
            let mut track = PressureTrack::default();
            track.see(&pen, Some(&[(0, 0.2), (1, 0.8)]));
            let at_08 = track.see(&pen, None);
            assert_eq!(track.see(&pen, Some(&[(0, 0.1)])), at_08, "the touch-down report replays through evicted windows");
            // Only the genuinely new report folds: the blend of 1.0 into what 0.2 then 0.8 left.
            let alpha = pen.smoothing;
            let folded = alpha * 0.8 + (1.0 - alpha) * 0.2;
            assert_eq!(
                track.see(&pen, Some(&[(0, 0.1), (2, 1.0)])),
                pen.width_scale(alpha * 1.0 + (1.0 - alpha) * folded),
                "only the genuinely new report folds"
            );
        }

        #[test]
        fn no_report_yet_is_the_fixed_width() {
            let mut track = PressureTrack::default();
            assert_eq!(track.see(&pen(), Some(&[])), 1.0, "a pressure-less tool's empty window");
            assert_eq!(track.see(&pen(), None), 1.0, "no nameable contact");
        }

        #[test]
        fn a_missing_contact_holds_the_last_width() {
            let pen = pen();
            let mut track = PressureTrack::default();
            track.see(&pen, Some(&[(0, 1.0)]));
            assert_eq!(track.see(&pen, None), pen.max, "the frame after the channel went quiet");
        }
    }
}
