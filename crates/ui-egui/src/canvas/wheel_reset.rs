//! Clear egui's retained copy of wheel input already owned by the document canvas.
//!
//! The reset precedes fresh events and all panels. Its synthetic event is hidden from app
//! motion routing; genuine input stays in its original order. No input is deferred or replayed.

use egui::{Context, Event, Id, Modifiers, MouseWheelSource, MouseWheelUnit, Plugin, RawInput, TouchPhase, Ui, Vec2, ViewportId};

/// Schedule a reset of this viewport's hidden egui wheel state before its next frame.
pub(crate) fn consumed(ctx: &Context) {
    let _ = ctx.plugin_or_default::<WheelReset>();
    let key = pending(ctx.viewport_id());
    let frame = ctx.cumulative_frame_nr();
    ctx.data_mut(|data| data.insert_temp(key, frame));
}

/// Whether this viewport's old egui wheel state was reset at the start of the current pass.
pub(crate) fn reset_this_pass(ctx: &Context) -> bool {
    let viewport = ctx.viewport_id();
    let pass = ctx.cumulative_pass_nr_for(viewport);
    ctx.data(|data| data.get_temp::<u64>(applied(viewport)) == Some(pass))
}

/// Separates reset requests belonging to different native viewports.
fn pending(viewport: ViewportId) -> Id {
    Id::unique(("pdfcraft-wheel-reset-pending", viewport))
}

/// Records whether this pass contains an internal reset that must be hidden from app routing.
fn injected(viewport: ViewportId) -> Id {
    Id::unique(("pdfcraft-wheel-reset-injected", viewport))
}

/// Shares reset ownership with app wheel readers only during the pass that applied it.
fn applied(viewport: ViewportId) -> Id {
    Id::unique(("pdfcraft-wheel-reset-applied", viewport))
}

/// A zero-distance cancellation clears egui's private wheel queue before fresh input arrives.
fn reset_event() -> Event {
    Event::MouseWheel {
        unit: MouseWheelUnit::Point,
        delta: Vec2::ZERO,
        phase: TouchPhase::Cancel,
        modifiers: Modifiers::NONE,
        source: MouseWheelSource::Unknown,
    }
}

/// Stateless plugin: per-viewport flags live in the context without retaining the context.
#[derive(Default)]
struct WheelReset;

impl Plugin for WheelReset {
    /// Identifies the input ownership hook in egui diagnostics.
    fn debug_name(&self) -> &'static str {
        "pdfcraft-wheel-reset"
    }

    /// Cancel only the old wheel copy before egui processes this pass's fresh input.
    fn input_hook(&mut self, ctx: &Context, raw: &mut RawInput) {
        ctx.data_mut(|data| data.remove::<u64>(applied(raw.viewport_id)));
        let key = pending(raw.viewport_id);
        let Some(consumed_frame) = ctx.data(|data| data.get_temp::<u64>(key)) else {
            return;
        };
        // A flag can outlive a closed viewport. Create its replacement before querying the
        // counter, and discard stale ownership if this viewport has not received input yet.
        let initialized = ctx.viewport_for(raw.viewport_id, |viewport| viewport.input.raw.viewport_id == raw.viewport_id);
        if !initialized {
            ctx.data_mut(|data| data.remove::<u64>(key));
            return;
        }
        // run_pass snapshots begin-pass plugins before the frame, but refreshes input hooks
        // on each discard pass. A lazily registered plugin must wait for the next frame.
        if ctx.cumulative_frame_nr_for(raw.viewport_id) == consumed_frame {
            return;
        }
        if raw.events.try_reserve(1).is_err() {
            // Keep the pending flag so a later pass can retry without dropping fresh input.
            return;
        }
        // Index zero is always valid; the fallible reservation above admits the extra event.
        raw.events.insert(0, reset_event());
        ctx.data_mut(|data| {
            data.remove::<u64>(key);
            data.insert_temp(injected(raw.viewport_id), true);
        });
    }

    /// Hide exactly our prepended reset before app motion or any panel inspects input events.
    fn on_begin_pass(&mut self, ui: &mut Ui) {
        let ctx = ui.ctx();
        let key = injected(ctx.viewport_id());
        let ours = ctx.data_mut(|data| data.remove_temp::<bool>(key).unwrap_or(false));
        if ours {
            let removed = ctx.input_mut(|input| {
                if input.events.first() == Some(&reset_event()) {
                    // The first-event check proves index zero exists and belongs to this reset.
                    input.events.remove(0);
                    true
                } else {
                    false
                }
            });
            if removed {
                let key = applied(ctx.viewport_id());
                let pass = ctx.cumulative_pass_nr();
                ctx.data_mut(|data| data.insert_temp(key, pass));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui::{Pos2, Rect, vec2};

    #[derive(Clone)]
    struct Observed {
        scroll: Vec2,
        zoom: f32,
        events: Vec<Event>,
        reset: bool,
    }

    fn context() -> Context {
        let ctx = Context::default();
        ctx.options_mut(|options| {
            options.max_passes = 3.try_into().unwrap();
            options.input_options.line_scroll_speed = 100.0;
        });
        ctx
    }

    fn wheel(unit: MouseWheelUnit, amount: f32, modifiers: Modifiers) -> Event {
        Event::MouseWheel { unit, delta: vec2(0.0, amount), phase: TouchPhase::Move, source: MouseWheelSource::Unknown, modifiers }
    }

    /// Run the actual hooks and input pipeline without an app raw-input hook.
    fn frame(ctx: &Context, viewport: ViewportId, time: f64, events: Vec<Event>, consume: bool, passes: usize) -> Vec<Observed> {
        let mut raw = RawInput {
            viewport_id: viewport,
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(600.0, 800.0))),
            time: Some(time),
            predicted_dt: 1.0 / 60.0,
            events,
            ..Default::default()
        };
        raw.viewports.entry(viewport).or_default();
        let mut observed = Vec::new();
        let output = ctx.run_ui(raw, |ui| {
            let reset = reset_this_pass(ui.ctx());
            observed.push(ui.input(|input| Observed {
                scroll: input.smooth_scroll_delta,
                zoom: input.zoom_delta(),
                events: input.events.clone(),
                reset,
            }));
            if consume {
                consumed(ui.ctx());
                // The canvas drains the public value after taking ownership of the raw wheel.
                ui.input_mut(|input| input.smooth_scroll_delta = Vec2::ZERO);
            }
            if observed.len() < passes {
                ui.ctx().request_discard("wheel reset multipass regression");
            }
        });
        output.drop_without_applying_deltas();
        assert_eq!(observed.len(), passes);
        observed
    }

    /// Draining the public delta alone leaves a hidden tail; the plugin clears it before UI.
    #[test]
    fn consumed_wheel_does_not_leak_into_the_next_frame_elsewhere() {
        let baseline = context();
        frame(&baseline, ViewportId::ROOT, 0.0, vec![wheel(MouseWheelUnit::Point, 200.0, Modifiers::NONE)], false, 1);
        baseline.input_mut(|input| input.smooth_scroll_delta = Vec2::ZERO);
        let elsewhere = vec![Event::PointerMoved(Pos2::new(10.0, 10.0))];
        assert!(frame(&baseline, ViewportId::ROOT, 1.0 / 60.0, elsewhere.clone(), false, 1)[0].scroll.y > 0.0);

        let reset = context();
        frame(&reset, ViewportId::ROOT, 0.0, vec![wheel(MouseWheelUnit::Point, 200.0, Modifiers::NONE)], true, 1);
        let next = frame(&reset, ViewportId::ROOT, 1.0 / 60.0, elsewhere.clone(), false, 1);
        assert_eq!(next[0].scroll, Vec2::ZERO);
        assert_eq!(next[0].events, elsewhere);
    }

    /// Resetting old motion preserves the complete distance of fresh Point and Line events.
    #[test]
    fn fresh_wheel_distance_is_preserved_after_the_reset() {
        for (unit, amount, expected) in [(MouseWheelUnit::Point, 60.0, 60.0), (MouseWheelUnit::Line, 1.0, 100.0)] {
            let ctx = context();
            frame(&ctx, ViewportId::ROOT, 0.0, vec![wheel(MouseWheelUnit::Point, 200.0, Modifiers::NONE)], true, 1);
            let fresh = wheel(unit, amount, Modifiers::NONE);
            let first = frame(&ctx, ViewportId::ROOT, 1.0 / 60.0, vec![fresh.clone()], false, 1);
            assert_eq!(first[0].events, vec![fresh]);
            let mut distance = first[0].scroll.y;
            for index in 2..120 {
                distance += frame(&ctx, ViewportId::ROOT, f64::from(index) / 60.0, Vec::new(), false, 1)[0].scroll.y;
            }
            assert!((distance - expected).abs() < 0.005, "{unit:?}: {distance}, expected {expected}");
        }
    }

    /// A fresh Ctrl-wheel remains a zoom input after old non-zoom wheel state is cleared.
    #[test]
    fn fresh_ctrl_wheel_zoom_is_preserved() {
        let ctx = context();
        frame(&ctx, ViewportId::ROOT, 0.0, vec![wheel(MouseWheelUnit::Point, 200.0, Modifiers::NONE)], true, 1);
        let zoom = wheel(MouseWheelUnit::Point, 4.0, Modifiers::CTRL);
        let next = frame(&ctx, ViewportId::ROOT, 1.0 / 60.0, vec![zoom.clone()], false, 1);
        assert!(next[0].zoom > 1.0);
        assert_eq!(next[0].scroll, Vec2::ZERO);
        assert_eq!(next[0].events, vec![zoom]);
    }

    /// A genuine zero-delta Cancel is retained, both with and without our preceding marker.
    #[test]
    fn genuine_cancel_events_are_not_removed() {
        for schedule in [false, true] {
            let ctx = context();
            let _ = ctx.plugin_or_default::<WheelReset>();
            if schedule {
                frame(&ctx, ViewportId::ROOT, 0.0, vec![wheel(MouseWheelUnit::Point, 200.0, Modifiers::NONE)], true, 1);
            }
            let events = vec![reset_event(), wheel(MouseWheelUnit::Point, 3.0, Modifiers::NONE)];
            let observed = frame(&ctx, ViewportId::ROOT, 1.0 / 60.0, events.clone(), false, 1);
            assert_eq!(observed[0].events, events);
            assert_eq!(observed[0].scroll.y, 3.0);
            assert_eq!(observed[0].reset, schedule);
        }
    }

    /// Discard passes see no replay of wheel events or synthetic cancellation markers.
    #[test]
    fn discard_passes_do_not_replay_events_or_old_wheel_motion() {
        let ctx = context();
        frame(&ctx, ViewportId::ROOT, 0.0, vec![wheel(MouseWheelUnit::Point, 200.0, Modifiers::NONE)], true, 1);
        let fresh = wheel(MouseWheelUnit::Point, 4.0, Modifiers::NONE);
        let passes = frame(&ctx, ViewportId::ROOT, 1.0 / 60.0, vec![fresh.clone()], false, 3);
        assert_eq!(passes[0].events, vec![fresh]);
        assert_eq!(passes[0].scroll.y, 4.0);
        for pass in passes.iter().skip(1) {
            assert!(pass.events.is_empty());
            assert_eq!(pass.scroll, Vec2::ZERO);
        }
    }

    /// Reset flags belong to a viewport, even when another viewport processes input first.
    #[test]
    fn another_viewport_does_not_consume_the_pending_reset() {
        let ctx = context();
        let other = ViewportId::from_hash_of("wheel-reset-other");
        frame(&ctx, ViewportId::ROOT, 0.0, vec![wheel(MouseWheelUnit::Point, 200.0, Modifiers::NONE)], true, 1);
        let other_events = vec![wheel(MouseWheelUnit::Point, 4.0, Modifiers::NONE)];
        let observed = frame(&ctx, other, 1.0 / 60.0, other_events.clone(), false, 1);
        assert_eq!(observed[0].events, other_events);
        assert_eq!(observed[0].scroll.y, 4.0);
        assert!(ctx.data(|data| data.get_temp::<u64>(pending(ViewportId::ROOT)).is_some()));
        let root = frame(&ctx, ViewportId::ROOT, 2.0 / 60.0, Vec::new(), false, 1);
        assert_eq!(root[0].scroll, Vec2::ZERO);
        assert!(root[0].events.is_empty());
    }

    /// First-use registration during a discarded pass must not expose the reset to later passes.
    #[test]
    fn first_consumed_frame_can_discard_without_exposing_the_reset() {
        let ctx = context();
        let fresh = wheel(MouseWheelUnit::Point, 200.0, Modifiers::NONE);
        let passes = frame(&ctx, ViewportId::ROOT, 0.0, vec![fresh.clone()], true, 3);
        assert_eq!(passes[0].events, vec![fresh]);
        for pass in passes.iter().skip(1) {
            assert!(pass.events.is_empty(), "the newly registered input hook must not expose its reset before the begin-pass hook is active");
        }
        let next = frame(&ctx, ViewportId::ROOT, 1.0 / 60.0, Vec::new(), false, 1);
        assert_eq!(next[0].scroll, Vec2::ZERO);
        assert!(next[0].events.is_empty());
    }

    /// App readers observe reset ownership only in the viewport and pass that applied it.
    #[test]
    fn reset_notification_is_scoped_to_the_pass_and_viewport() {
        let ctx = context();
        let first = frame(&ctx, ViewportId::ROOT, 0.0, vec![wheel(MouseWheelUnit::Point, 200.0, Modifiers::NONE)], true, 1);
        assert!(!first[0].reset);
        let other = ViewportId::from_hash_of("wheel-reset-notification-other");
        assert!(!frame(&ctx, other, 1.0 / 60.0, Vec::new(), false, 1)[0].reset);
        let passes = frame(&ctx, ViewportId::ROOT, 2.0 / 60.0, Vec::new(), false, 3);
        assert!(passes[0].reset);
        assert!(passes.iter().skip(1).all(|pass| !pass.reset));
        assert!(!frame(&ctx, ViewportId::ROOT, 3.0 / 60.0, Vec::new(), false, 1)[0].reset);
    }

    /// An old flag for a removed viewport must neither panic nor cancel a replacement's input.
    #[test]
    fn stale_reset_for_a_recreated_viewport_does_not_cancel_fresh_motion() {
        let ctx = context();
        let other = ViewportId::from_hash_of("wheel-reset-recreated");
        let _ = ctx.plugin_or_default::<WheelReset>();
        // Model the temporary flag surviving egui's removal of the old ViewportState.
        ctx.data_mut(|data| data.insert_temp(pending(other), 0u64));
        let first = frame(&ctx, other, 0.0, vec![wheel(MouseWheelUnit::Point, 200.0, Modifiers::NONE)], false, 1);
        assert!(!first[0].reset);
        assert!(ctx.data(|data| data.get_temp::<u64>(pending(other)).is_none()));
        let next = frame(&ctx, other, 1.0 / 60.0, Vec::new(), false, 1);
        assert!(next[0].scroll.y > 0.0, "fresh unconsumed wheel motion still belongs to the replacement viewport");
        assert!(!next[0].reset);
    }
}
