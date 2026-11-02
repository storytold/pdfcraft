use super::*;
use std::cell::Cell;
use std::rc::Rc;

fn wheel(unit: MouseWheelUnit, delta: Vec2, phase: TouchPhase, modifiers: Modifiers) -> Event {
    Event::MouseWheel { unit, delta, phase, modifiers, source: egui::MouseWheelSource::Unknown }
}

fn notch(dy: f32, modifiers: Modifiers) -> Event {
    wheel(MouseWheelUnit::Line, vec2(0.0, dy), TouchPhase::Move, modifiers)
}

fn read(events: &[Event]) -> WheelMotion {
    WheelReader::default().read(events, 600.0, &egui::InputOptions::default())
}

#[test]
fn a_step_is_read_whole_in_its_units() {
    let line = egui::InputOptions::default().line_scroll_speed;
    assert_eq!(read(&[notch(-1.0, Modifiers::NONE)]), WheelMotion { scroll: vec2(0.0, -line), zoom: 1.0 });
    assert_eq!(read(&[notch(-1.0, Modifiers::NONE), notch(-2.0, Modifiers::NONE)]).scroll, vec2(0.0, -3.0 * line), "steps of one pass add up");
    let points = wheel(MouseWheelUnit::Point, vec2(3.0, -250.0), TouchPhase::Move, Modifiers::NONE);
    assert_eq!(read(&[points]).scroll, vec2(3.0, -250.0));
    let page = wheel(MouseWheelUnit::Page, vec2(0.0, -1.0), TouchPhase::Move, Modifiers::NONE);
    assert_eq!(read(&[page]).scroll, vec2(0.0, -600.0));
    assert_eq!(read(&[]), WheelMotion { scroll: Vec2::ZERO, zoom: 1.0 }, "and nothing is left for a later pass");
}

#[test]
fn modifiers_turn_the_wheel_sideways_or_into_zoom() {
    let line = egui::InputOptions::default().line_scroll_speed;
    assert_eq!(read(&[notch(-1.0, Modifiers::SHIFT)]).scroll, vec2(-line, 0.0));
    let sideways = wheel(MouseWheelUnit::Point, vec2(-30.0, 0.0), TouchPhase::Move, Modifiers::ALT);
    assert_eq!(read(&[sideways]).scroll, vec2(0.0, -30.0));
    let zoomed = read(&[notch(1.0, Modifiers::COMMAND)]);
    assert_eq!(zoomed.scroll, Vec2::ZERO, "a zooming wheel doesn't scroll");
    let step = (egui::InputOptions::default().scroll_zoom_speed * line).exp();
    assert!((zoomed.zoom - step).abs() < 1e-6, "{zoomed:?}");
    assert!((read(&[notch(-1.0, Modifiers::COMMAND)]).zoom - 1.0 / step).abs() < 1e-6);
    assert!((read(&[Event::Zoom(1.5), Event::Zoom(2.0)]).zoom - 3.0).abs() < 1e-6, "pinch events multiply");
}

#[test]
fn a_touch_gesture_keeps_its_modifiers_and_ends_clean() {
    let mut reader = WheelReader::default();
    let options = egui::InputOptions::default();
    let point = |dy: f32, phase, modifiers| wheel(MouseWheelUnit::Point, vec2(0.0, dy), phase, modifiers);
    // ⌘ held as the fingers land, let go before the momentum: still zooming.
    let began = reader.read(&[point(0.0, TouchPhase::Start, Modifiers::COMMAND), point(20.0, TouchPhase::Move, Modifiers::COMMAND)], 600.0, &options);
    assert!(began.zoom > 1.0 && began.scroll == Vec2::ZERO, "{began:?}");
    let momentum = reader.read(&[point(20.0, TouchPhase::Move, Modifiers::NONE)], 600.0, &options);
    assert!(momentum.zoom > 1.0 && momentum.scroll == Vec2::ZERO, "{momentum:?}");
    // The pass it ends in moves nothing, and the next gesture starts without the modifier.
    let ended = reader.read(&[point(20.0, TouchPhase::Move, Modifiers::NONE), point(0.0, TouchPhase::End, Modifiers::NONE)], 600.0, &options);
    assert_eq!(ended, WheelMotion { scroll: Vec2::ZERO, zoom: 1.0 });
    assert_eq!(reader.read(&[point(-20.0, TouchPhase::Move, Modifiers::NONE)], 600.0, &options).scroll, vec2(0.0, -20.0));
}

#[test]
fn numbers_that_are_not_finite_move_nothing() {
    for bad in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY] {
        let scrolled = read(&[notch(bad, Modifiers::NONE), notch(-1.0, Modifiers::NONE)]);
        assert!(scrolled.scroll.x.is_finite() && scrolled.scroll.y.is_finite(), "{bad}: {scrolled:?}");
        assert_eq!(read(&[notch(bad, Modifiers::COMMAND)]).zoom, 1.0, "{bad}");
        assert_eq!(read(&[Event::Zoom(bad)]).zoom, 1.0, "{bad}");
    }
    assert_eq!(read(&[Event::Zoom(0.0)]).zoom, 1.0);
    assert_eq!(read(&[Event::Zoom(-2.0)]).zoom, 1.0);
    // Too far to be a factor at all.
    assert_eq!(read(&[notch(1.0e9, Modifiers::COMMAND)]).zoom, 1.0);
}

/// One pass of a bare egui context, a sixtieth of a second after the last.
fn pass(ctx: &egui::Context, time: &mut f64, events: Vec<Event>, mut seen: impl FnMut(&egui::Context)) {
    *time += 1.0 / 60.0;
    let raw = egui::RawInput { time: Some(*time), events, ..Default::default() };
    // A bare context's first frame uploads the font atlas; nothing paints it here.
    ctx.run_ui(raw, |ui| seen(ui.ctx())).textures_delta.clear();
}

/// The reader must stay egui's own reading minus the easing: over the passes egui takes to
/// ease a step, it hands out as much scroll (and as much zoom) as the reader does at once.
#[test]
fn reading_at_once_goes_as_far_as_egui_eases() {
    let options = egui::InputOptions::default();
    for (modifiers, unit, step) in [
        (Modifiers::NONE, MouseWheelUnit::Line, vec2(0.0, -3.0)),
        (Modifiers::NONE, MouseWheelUnit::Point, vec2(40.0, -250.0)),
        (Modifiers::SHIFT, MouseWheelUnit::Line, vec2(0.0, 2.0)),
        (Modifiers::COMMAND, MouseWheelUnit::Line, vec2(0.0, 1.0)),
        (Modifiers::COMMAND, MouseWheelUnit::Point, vec2(0.0, -90.0)),
    ] {
        let event = wheel(unit, step, TouchPhase::Move, modifiers);
        let ctx = egui::Context::default();
        let (mut time, mut scrolled, mut zoomed, mut first) = (0.0, Vec2::ZERO, 1.0_f32, None);
        let mut events = vec![event.clone()];
        for _ in 0..120 {
            pass(&ctx, &mut time, std::mem::take(&mut events), |ctx| {
                let (scroll, zoom) = ctx.input(|i| (i.smooth_scroll_delta, i.zoom_delta()));
                first.get_or_insert((scroll, zoom));
                scrolled += scroll;
                zoomed *= zoom;
            });
        }
        let page_height = ctx.input(|i| i.viewport_rect().height());
        let at_once = WheelReader::default().read(&[event], page_height, &options);
        assert!((scrolled - at_once.scroll).length() < 0.01, "{modifiers:?} {unit:?}: egui scrolled {scrolled:?}, read {:?}", at_once.scroll);
        assert!((zoomed / at_once.zoom - 1.0).abs() < 1e-3, "{modifiers:?} {unit:?}: egui zoomed {zoomed}, read {}", at_once.zoom);
        // What makes the reader necessary: egui's first pass holds part of the step back.
        let first = first.expect("passes ran");
        assert!(
            first != (at_once.scroll, at_once.zoom),
            "{modifiers:?} {unit:?}: egui no longer eases this step ({first:?}); the reader may not be needed"
        );
    }
}

#[test]
fn reduced_motion_hands_egui_the_whole_step_and_stops_its_animations() {
    let ctx = egui::Context::default();
    let mut motion = Motion { reduced: true, ..Default::default() };
    let line = egui::InputOptions::default().line_scroll_speed;
    let mut time = 0.0;
    let mut seen = Vec::new();
    let mut events = vec![notch(-1.0, Modifiers::NONE)];
    for _ in 0..30 {
        pass(&ctx, &mut time, std::mem::take(&mut events), |ctx| {
            motion.begin_pass(ctx, AnimationEffects::System);
            seen.push(ctx.input(|i| i.smooth_scroll_delta));
        });
    }
    assert_eq!(seen.first(), Some(&vec2(0.0, -line)), "the whole notch in the pass it arrives");
    assert!(seen.iter().skip(1).all(|d| *d == Vec2::ZERO), "and none of egui's easing after it: {seen:?}");
    let style = ctx.global_style();
    assert_eq!((style.animation_time, style.scroll_animation), (0.0, egui::style::ScrollAnimation::none()));
    for theme in [egui::Theme::Light, egui::Theme::Dark] {
        assert_eq!(ctx.style_of(theme).animation_time, 0.0, "{theme:?}");
    }

    // ⌘/Ctrl with the wheel: the whole zoom step at once, then nothing.
    let step = (egui::InputOptions::default().scroll_zoom_speed * line).exp();
    let mut zooms = Vec::new();
    let mut events = vec![notch(1.0, Modifiers::COMMAND)];
    for _ in 0..30 {
        pass(&ctx, &mut time, std::mem::take(&mut events), |ctx| {
            motion.begin_pass(ctx, AnimationEffects::System);
            zooms.push(zoom_delta(ctx));
        });
    }
    assert!((zooms[0] - step).abs() < 1e-6, "{zooms:?}");
    assert!(zooms.iter().skip(1).all(|z| *z == 1.0), "{zooms:?}");

    // Animations allowed again: egui's own values and its own times.
    motion.reduced = false;
    let mut eased = Vec::new();
    let mut events = vec![notch(1.0, Modifiers::COMMAND)];
    for _ in 0..3 {
        pass(&ctx, &mut time, std::mem::take(&mut events), |ctx| {
            motion.begin_pass(ctx, AnimationEffects::System);
            eased.push((zoom_delta(ctx), ctx.input(|i| i.zoom_delta())));
        });
    }
    assert!(eased.iter().all(|(ours, eguis)| ours == eguis), "{eased:?}");
    assert!(eased[0].0 > 1.0 && eased[0].0 < step, "egui eases the step: {eased:?}");
    let (style, default) = (ctx.global_style(), egui::Style::default());
    assert_eq!((style.animation_time, style.scroll_animation), (default.animation_time, default.scroll_animation));
}

#[test]
fn the_preference_overrides_the_system_for_every_animation() {
    use AnimationEffects::{Off, On, System};
    assert_eq!(AnimationEffects::default(), System, "the system's setting is followed unless the user chooses");
    for choice in AnimationEffects::ALL {
        assert_eq!(AnimationEffects::parse(choice.name()), Some(choice));
    }
    assert_eq!((AnimationEffects::parse("OFF"), AnimationEffects::parse("smooth")), (Some(Off), None));
    let (line, egui_default) = (egui::InputOptions::default().line_scroll_speed, egui::Style::default());
    let egui_times = (egui_default.animation_time, egui_default.scroll_animation);
    // The system's answer, the choice, and whether the window then animates.
    for (reduced, effects, animates) in
        [(false, System, true), (true, System, false), (false, On, true), (true, On, true), (false, Off, false), (true, Off, false)]
    {
        assert_eq!(effects.animates(reduced), animates, "{effects:?}, system reduced: {reduced}");
        let ctx = egui::Context::default();
        let mut motion = Motion { reduced, ..Default::default() };
        let (mut time, mut first) = (0.0, None);
        pass(&ctx, &mut time, vec![notch(-1.0, Modifiers::NONE)], |ctx| {
            motion.begin_pass(ctx, effects);
            first = Some((ctx.input(|i| i.smooth_scroll_delta), animated(ctx)));
        });
        // The wheel, egui's animations and the app's own fades all go the same way.
        let whole_notch = first.map(|(scroll, _)| scroll) == Some(vec2(0.0, -line));
        assert_eq!(
            (whole_notch, first.map(|(_, animated)| animated)),
            (!animates, Some(animates)),
            "{effects:?}, system reduced: {reduced}: {first:?}"
        );
        let style = ctx.global_style();
        let times = if animates { egui_times } else { (0.0, ScrollAnimation::none()) };
        assert_eq!((style.animation_time, style.scroll_animation), times, "{effects:?}, system reduced: {reduced}");
    }
    // A choice made while running applies from the next pass, and choosing back undoes it.
    let ctx = egui::Context::default();
    let mut motion = Motion::default();
    let mut time = 0.0;
    for (effects, times) in [(Off, (0.0, ScrollAnimation::none())), (System, egui_times), (Off, (0.0, ScrollAnimation::none())), (On, egui_times)] {
        pass(&ctx, &mut time, Vec::new(), |ctx| motion.begin_pass(ctx, effects));
        let style = ctx.global_style();
        assert_eq!((style.animation_time, style.scroll_animation), times, "{effects:?}");
    }
}

/// The chrome's hover wash has a duration of its own (`theme::Press`): it eases in with
/// animation effects on and is simply there with them off.
#[test]
fn the_hover_wash_eases_only_with_animation_effects_on() {
    for (effects, eased) in [(AnimationEffects::On, true), (AnimationEffects::Off, false)] {
        let ctx = egui::Context::default();
        let mut motion = Motion::default();
        let (mut time, mut hover) = (0.0, Vec::new());
        for _ in 0..12 {
            time += 1.0 / 60.0;
            let raw = egui::RawInput { time: Some(time), events: vec![Event::PointerMoved(egui::pos2(20.0, 20.0))], ..Default::default() };
            ctx.run_ui(raw, |ui| {
                motion.begin_pass(ui.ctx(), effects);
                let (_, response) = ui.allocate_exact_size(vec2(100.0, 40.0), egui::Sense::click());
                hover.push(crate::theme::Press::track(ui, &response).hover);
            })
            .textures_delta
            .clear();
        }
        // The first frame the pointer is found over the control.
        let first = hover.iter().copied().find(|amount| *amount > 0.0).expect("the pointer is over the control");
        assert_eq!(first < 1.0, eased, "{effects:?}: {hover:?}");
        assert_eq!(hover.last(), Some(&1.0), "{effects:?}: the wash is whole in the end either way: {hover:?}");
    }
}

#[test]
fn a_zoom_read_in_an_earlier_pass_is_never_used_again() {
    let ctx = egui::Context::default();
    let mut motion = Motion { reduced: true, ..Default::default() };
    let mut time = 0.0;
    pass(&ctx, &mut time, vec![notch(1.0, Modifiers::COMMAND)], |ctx| {
        motion.begin_pass(ctx, AnimationEffects::System);
        assert!(zoom_delta(ctx) > 1.0);
    });
    // A pass that didn't begin here (a host that draws without `logic`): egui's own answer.
    pass(&ctx, &mut time, Vec::new(), |ctx| {
        let eguis = ctx.input(|i| i.zoom_delta());
        assert_eq!(zoom_delta(ctx), eguis);
    });
}

/// With animation effects off, a real scroll area moves by one wheel step whether the frame
/// runs one pass or two (a `request_discard` from a sizing pass): the scroll area already
/// applied and stored the step in the first pass, so a later pass must not apply it again,
/// and must not replay the frame's raw wheel events either.
#[test]
fn two_pass_scroll_area_applies_one_wheel_step_like_a_single_pass() {
    fn run(two_passes: bool) -> (Vec<Vec2>, Vec<usize>) {
        let ctx = egui::Context::default();
        let mut motion = Motion { reduced: true, ..Default::default() };
        let mut offsets = Vec::new();
        let mut wheel_events = Vec::new();
        let raw = egui::RawInput {
            time: Some(0.1),
            screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(300.0, 300.0))),
            events: vec![egui::Event::PointerMoved(egui::pos2(50.0, 50.0)), notch(-1.0, Modifiers::NONE)],
            ..Default::default()
        };
        let output = ctx.run_ui(raw, |ui| {
            motion.begin_pass(ui.ctx(), AnimationEffects::System);
            wheel_events.push(ui.input(|i| i.events.iter().filter(|e| matches!(e, egui::Event::MouseWheel { .. })).count()));
            // Taller than the 300-point viewport, so one step stays far from the scroll
            // limit and a doubled step could not be hidden by clamping.
            let out = egui::ScrollArea::vertical().id_salt("motion-two-pass-scroll").auto_shrink([false, false]).show(ui, |ui| {
                ui.set_min_size(egui::vec2(100.0, 3000.0));
            });
            offsets.push(out.state.unclamped_offset());
            if two_passes && ui.ctx().current_pass_index() == 0 {
                ui.ctx().request_discard("motion two-pass scroll regression");
            }
        });
        output.drop_without_applying_deltas();
        (offsets, wheel_events)
    }

    let (single, single_events) = run(false);
    let (two, two_events) = run(true);

    assert_eq!(single.len(), 1, "one frame, one pass");
    assert_eq!(two.len(), 2, "the frame really ran two passes");
    assert_eq!(single_events, vec![1], "one wheel event reaches the first pass");
    assert_eq!(two_events, vec![1, 0], "raw wheel events are not replayed in the later pass");

    let step = single[0].y;
    assert!(step > 0.0, "the wheel step scrolled: {single:?}");
    assert!((two[0].y - step).abs() < 0.01, "the first pass applies the whole step: single={step}, two-pass={two:?}");
    assert!((two[1].y - step).abs() < 0.01, "the presented pass must not apply the same step again: single={step}, two-pass={two:?}");
}

/// With animation effects off, ctrl/⌘-wheel and pinch zoom reach a view's persisted zoom
/// once, whether the frame runs one pass or two (a `request_discard` from a sizing pass):
/// multiplying an accumulator by each pass's factor gives the single step either way. A
/// later pass gets a unit factor explicitly, and a frame with no wheel input resets to it.
#[test]
fn two_pass_zoom_applies_one_step_like_a_single_pass() {
    /// One frame's `zoom_delta` readings, one per pass.
    fn frame(ctx: &egui::Context, motion: &mut Motion, time: f64, events: Vec<Event>, two_passes: bool) -> Vec<f32> {
        let raw = egui::RawInput {
            time: Some(time),
            screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(300.0, 300.0))),
            events,
            ..Default::default()
        };
        let mut zooms = Vec::new();
        let output = ctx.run_ui(raw, |ui| {
            motion.begin_pass(ui.ctx(), AnimationEffects::System);
            zooms.push(zoom_delta(ui.ctx()));
            if two_passes && ui.ctx().current_pass_index() == 0 {
                ui.ctx().request_discard("motion two-pass zoom regression");
            }
        });
        output.drop_without_applying_deltas();
        zooms
    }

    let line = egui::InputOptions::default().line_scroll_speed;
    let ctrl_step = (egui::InputOptions::default().scroll_zoom_speed * line).exp();
    for (name, events, step) in [("ctrl-wheel", vec![notch(1.0, Modifiers::COMMAND)], ctrl_step), ("pinch", vec![Event::Zoom(1.5)], 1.5)] {
        assert!((step - 1.0).abs() > 0.01, "{name}: the factor is nontrivial: {step}");
        let single = {
            let ctx = egui::Context::default();
            let mut motion = Motion { reduced: true, ..Default::default() };
            frame(&ctx, &mut motion, 0.1, events.clone(), false)
        };
        let ctx = egui::Context::default();
        let mut motion = Motion { reduced: true, ..Default::default() };
        let two = frame(&ctx, &mut motion, 0.1, events, true);
        assert_eq!(single.len(), 1, "{name}: one frame, one pass: {single:?}");
        assert_eq!(two.len(), 2, "{name}: the frame really ran two passes: {two:?}");
        let product = |zooms: &[f32]| zooms.iter().product::<f32>();
        assert!((product(&single) - step).abs() < 1e-5, "{name}: one pass applies the step: {single:?}");
        assert!((product(&two) - step).abs() < 1e-5, "{name}: two passes apply it once: {two:?}");
        assert!((two[1] - 1.0).abs() < 1e-6, "{name}: a later pass zooms by nothing: {two:?}");
        // The next frame's wheel reading starts over; the two-pass frame's factor is gone.
        let after = frame(&ctx, &mut motion, 0.1 + 1.0 / 60.0, Vec::new(), false);
        assert_eq!(after, vec![1.0], "{name}: the cached factor does not survive the frame");
    }
}

#[test]
fn the_system_is_asked_now_and_then_and_no_answer_changes_nothing() {
    let ctx = egui::Context::default();
    let answer = Rc::new(Cell::new(Some(true)));
    let asked = Rc::new(Cell::new(0_u32));
    let (source_answer, source_asked) = (Rc::clone(&answer), Rc::clone(&asked));
    let mut motion = Motion {
        source: Some(Box::new(move || {
            source_asked.set(source_asked.get() + 1);
            source_answer.get()
        })),
        ..Default::default()
    };
    let mut time = 0.0;
    pass(&ctx, &mut time, Vec::new(), |ctx| motion.begin_pass(ctx, AnimationEffects::System));
    assert!(motion.reduced, "asked in the first pass");
    answer.set(Some(false));
    for _ in 0..30 {
        pass(&ctx, &mut time, Vec::new(), |ctx| motion.begin_pass(ctx, AnimationEffects::System));
    }
    assert!(motion.reduced && asked.get() == 1, "not asked again within half a second ({} times)", asked.get());
    // Back at the window later: the first pass asks.
    time += ASK_EVERY;
    pass(&ctx, &mut time, Vec::new(), |ctx| motion.begin_pass(ctx, AnimationEffects::System));
    assert!(!motion.reduced);
    answer.set(None);
    motion.reduced = true;
    time += ASK_EVERY;
    pass(&ctx, &mut time, Vec::new(), |ctx| motion.begin_pass(ctx, AnimationEffects::System));
    assert!(motion.reduced && asked.get() == 3, "no answer leaves the last one standing");
}
