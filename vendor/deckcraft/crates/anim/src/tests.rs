#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::sync::Arc;

use deckcraft_model::anim::{ANIMATIONS, TRANSITIONS, TextBuild};
use deckcraft_model::{
    AnimClass, AnimStart, Animation, ColorRef, CustomShow, Presentation, Rgba, Shape, ShapeId, Slide, SlideId, TextBody, Transition, Xfrm,
};

use crate::{
    AnimState, Layer, LinkJump, MotionPath, ShowAction, ShowState, Source, Timeline, TriggerPlay, default_path, ease, morph_crop, morph_frame,
    morph_pairs, morph_xfrm, show_order, transition_frame, transition_layers,
};

const W: f64 = 960.0;
const H: f64 = 540.0;

fn shape(id: u32, name: &str) -> Shape {
    Shape { id: ShapeId(id), name: name.into(), xfrm: Some(Xfrm::new(100.0, 100.0, 200.0, 100.0)), ..Default::default() }
}

fn anim(shape: u32, class: AnimClass, effect: &str, start: AnimStart) -> Animation {
    Animation { shape: ShapeId(shape), class, effect: effect.into(), start, duration_ms: 1000, ..Default::default() }
}

fn slide_with(anims: Vec<Animation>) -> Slide {
    Slide { shapes: (1..=4).map(|i| shape(i, &format!("S{i}"))).collect(), animations: anims, ..Default::default() }
}

fn finite(s: &AnimState) -> bool {
    s.opacity.is_finite()
        && s.offset_x.is_finite()
        && s.offset_y.is_finite()
        && s.scale_x.is_finite()
        && s.scale_y.is_finite()
        && s.rotate.is_finite()
        && s.clip.is_none_or(|c| c.iter().all(|v| v.is_finite()))
}

#[test]
fn default_state_is_identity() {
    let s = AnimState::default();
    assert!(s.visible && s.opacity == 1.0 && s.scale_x == 1.0 && s.scale_y == 1.0);
    assert!(s.is_identity());
}

#[test]
fn on_click_starts_steps() {
    let tl = Timeline::new(
        &slide_with(vec![
            anim(1, AnimClass::Entrance, "fade", AnimStart::OnClick),
            anim(2, AnimClass::Entrance, "fade", AnimStart::OnClick),
            anim(3, AnimClass::Entrance, "fade", AnimStart::OnClick),
        ]),
        W,
        H,
    );
    assert_eq!(tl.steps(), 3);
    assert!(!tl.step_is_auto(0));
    assert!((tl.step_duration(1) - 1.0).abs() < 1e-9);
}

#[test]
fn with_previous_joins_step() {
    let tl = Timeline::new(
        &slide_with(vec![anim(1, AnimClass::Entrance, "fade", AnimStart::OnClick), anim(2, AnimClass::Entrance, "fade", AnimStart::WithPrevious)]),
        W,
        H,
    );
    assert_eq!(tl.steps(), 1);
    let e = tl.effects();
    assert_eq!(e[1].step, 0);
    assert_eq!(e[1].start, 0.0);
    assert!((tl.step_duration(0) - 1.0).abs() < 1e-9);
}

#[test]
fn after_previous_chains() {
    let tl = Timeline::new(
        &slide_with(vec![
            anim(1, AnimClass::Entrance, "fade", AnimStart::OnClick),
            anim(2, AnimClass::Entrance, "fade", AnimStart::AfterPrevious),
            anim(3, AnimClass::Entrance, "fade", AnimStart::AfterPrevious),
        ]),
        W,
        H,
    );
    assert_eq!(tl.steps(), 1);
    let e = tl.effects();
    assert!((e[1].start - 1.0).abs() < 1e-9);
    assert!((e[2].start - 2.0).abs() < 1e-9);
    assert!((tl.step_duration(0) - 3.0).abs() < 1e-9);
}

#[test]
fn delays_shift_start() {
    let mut a = anim(2, AnimClass::Entrance, "fade", AnimStart::WithPrevious);
    a.delay_ms = 250;
    let mut b = anim(3, AnimClass::Entrance, "fade", AnimStart::AfterPrevious);
    b.delay_ms = 500;
    let tl = Timeline::new(&slide_with(vec![anim(1, AnimClass::Entrance, "fade", AnimStart::OnClick), a, b]), W, H);
    let e = tl.effects();
    assert!((e[1].start - 0.25).abs() < 1e-9);
    // After previous: after the latest end (1.25) plus its delay.
    assert!((e[2].start - 1.75).abs() < 1e-9);
    assert!((tl.step_duration(0) - 2.75).abs() < 1e-9);
}

#[test]
fn auto_first_step() {
    let tl = Timeline::new(
        &slide_with(vec![anim(1, AnimClass::Entrance, "fade", AnimStart::AfterPrevious), anim(2, AnimClass::Entrance, "fade", AnimStart::OnClick)]),
        W,
        H,
    );
    assert_eq!(tl.steps(), 2);
    assert!(tl.step_is_auto(0));
    assert!(!tl.step_is_auto(1));
}

#[test]
fn hidden_before_entrance() {
    let tl = Timeline::new(&slide_with(vec![anim(1, AnimClass::Entrance, "fade", AnimStart::OnClick)]), W, H);
    assert!(!tl.state(ShapeId(1), None, 0, 0.0).visible);
    let mid = tl.state(ShapeId(1), None, 0, 0.5);
    assert!(mid.visible && mid.opacity > 0.0 && mid.opacity < 1.0);
    let done = tl.state(ShapeId(1), None, 1, 0.0);
    assert!(done.visible && (done.opacity - 1.0).abs() < 1e-9);
    // Unanimated shapes are untouched.
    assert!(tl.state(ShapeId(2), None, 0, 0.0).is_identity());
}

#[test]
fn hidden_after_exit() {
    let tl = Timeline::new(&slide_with(vec![anim(1, AnimClass::Exit, "fadeOut", AnimStart::OnClick)]), W, H);
    assert!(tl.state(ShapeId(1), None, 0, 0.0).visible);
    let mid = tl.state(ShapeId(1), None, 0, 0.5);
    assert!(mid.visible && mid.opacity < 1.0);
    assert!(!tl.state(ShapeId(1), None, 1, 0.0).visible);
    assert!(!tl.state(ShapeId(1), None, 0, 5.0).visible);
}

#[test]
fn entrance_then_exit() {
    let tl = Timeline::new(
        &slide_with(vec![anim(1, AnimClass::Entrance, "appear", AnimStart::OnClick), anim(1, AnimClass::Exit, "disappear", AnimStart::OnClick)]),
        W,
        H,
    );
    assert!(!tl.state(ShapeId(1), None, 0, 0.0).visible);
    assert!(tl.state(ShapeId(1), None, 1, 0.0).visible);
    assert!(!tl.state(ShapeId(1), None, 2, 0.0).visible);
}

#[test]
fn emphasis_returns_or_persists() {
    let mut spin = anim(1, AnimClass::Emphasis, "spin", AnimStart::OnClick);
    spin.amount = Some(90.0);
    let tl = Timeline::new(&slide_with(vec![anim(2, AnimClass::Emphasis, "pulse", AnimStart::OnClick), spin]), W, H);
    let p = tl.state(ShapeId(2), None, 0, 0.5);
    assert!(p.scale_x > 1.0);
    assert!(tl.state(ShapeId(2), None, 1, 0.0).is_identity());
    assert!((tl.state(ShapeId(1), None, 2, 0.0).rotate - 90.0).abs() < 1e-9);
}

#[test]
fn color_emphasis_tints() {
    let mut a = anim(1, AnimClass::Emphasis, "fillColor", AnimStart::OnClick);
    a.color = Some(ColorRef::rgb(Rgba::rgb(255, 0, 0)));
    let tl = Timeline::new(&slide_with(vec![a]), W, H);
    assert_eq!(tl.state(ShapeId(1), None, 1, 0.0).tint, Some(Rgba::rgb(255, 0, 0)));
}

#[test]
fn every_animation_evaluates_finite() {
    for (id, _, class, _, _, opts) in ANIMATIONS {
        let mut options: Vec<&str> = opts.to_vec();
        options.push("");
        for o in options {
            let mut a = anim(1, *class, id, AnimStart::OnClick);
            a.option = o.into();
            let tl = Timeline::new(&slide_with(vec![a]), W, H);
            for (step, t) in [(0, 0.0), (0, 0.001), (0, 0.25), (0, 0.5), (0, 0.999), (0, 1.0), (0, 2.0), (1, 0.0)] {
                let s = tl.state(ShapeId(1), None, step, t);
                assert!(finite(&s), "{id} {o} {step} {t}: {s:?}");
            }
            let end = tl.state(ShapeId(1), None, 1, 0.0);
            match class {
                AnimClass::Entrance => assert!(end.visible && (end.opacity - 1.0).abs() < 1e-9, "{id}"),
                AnimClass::Exit => assert!(!end.visible, "{id}"),
                _ => {}
            }
        }
    }
}

#[test]
fn fly_starts_off_slide() {
    let mut a = anim(1, AnimClass::Entrance, "fly", AnimStart::OnClick);
    a.option = "l".into();
    let tl = Timeline::new(&slide_with(vec![a]), W, H);
    let s = tl.state(ShapeId(1), None, 0, 1e-6);
    // Shape at x=100, w=200: must start at or beyond the left edge.
    assert!(100.0 + s.offset_x + 200.0 <= 0.01, "{s:?}");
    let s = tl.state(ShapeId(1), None, 0, 0.999_999);
    assert!(s.offset_x.abs() < 1.0);
}

#[test]
fn wipe_clips() {
    let mut a = anim(1, AnimClass::Entrance, "wipe", AnimStart::OnClick);
    a.option = "l".into();
    let tl = Timeline::new(&slide_with(vec![a]), W, H);
    let c = tl.state(ShapeId(1), None, 0, 0.5).clip.unwrap();
    assert!(c[0] == 0.0 && c[2] > 0.0 && c[2] < 1.0);
}

#[test]
fn repeat_and_auto_reverse_extend_duration() {
    let mut a = anim(1, AnimClass::Emphasis, "pulse", AnimStart::OnClick);
    a.repeat = 3;
    a.auto_reverse = true;
    let tl = Timeline::new(&slide_with(vec![a]), W, H);
    assert!((tl.step_duration(0) - 6.0).abs() < 1e-9);
    // Until next click: one iteration counts, keeps playing within the step.
    let mut b = anim(2, AnimClass::Emphasis, "spin", AnimStart::OnClick);
    b.repeat = u32::MAX;
    let tl = Timeline::new(&slide_with(vec![b]), W, H);
    assert!((tl.step_duration(0) - 1.0).abs() < 1e-9);
    let s = tl.state(ShapeId(2), None, 0, 10.5);
    assert!(s.rotate > 0.0 && s.rotate < 360.0);
}

#[test]
fn rewind_reverts_entrance() {
    let mut a = anim(1, AnimClass::Entrance, "fade", AnimStart::OnClick);
    a.rewind = true;
    let tl = Timeline::new(&slide_with(vec![a]), W, H);
    assert!(!tl.state(ShapeId(1), None, 1, 0.0).visible);
}

#[test]
fn after_hide_on_next_click() {
    let mut a = anim(1, AnimClass::Entrance, "fade", AnimStart::OnClick);
    a.after = Some("hideOnNextClick".into());
    let tl = Timeline::new(&slide_with(vec![a, anim(2, AnimClass::Entrance, "fade", AnimStart::OnClick)]), W, H);
    assert!(tl.state(ShapeId(1), None, 0, 5.0).visible);
    assert!(!tl.state(ShapeId(1), None, 1, 0.0).visible);
}

#[test]
fn triggered_sequences() {
    let mut a = anim(1, AnimClass::Entrance, "fade", AnimStart::OnClick);
    a.trigger = Some(ShapeId(3));
    let tl = Timeline::new(&slide_with(vec![anim(2, AnimClass::Entrance, "fade", AnimStart::OnClick), a]), W, H);
    assert_eq!(tl.steps(), 1);
    assert_eq!(tl.trigger_shapes(), vec![ShapeId(3)]);
    assert_eq!(tl.trigger_steps(ShapeId(3)), 1);
    // Not played: hidden even after the main sequence.
    assert!(!tl.state(ShapeId(1), None, 1, 0.0).visible);
    let tp = [TriggerPlay { trigger: ShapeId(3), step: 1, t: 0.0 }];
    assert!(tl.state_with(ShapeId(1), None, 0, 0.0, &tp).visible);
}

#[test]
fn text_build_by_paragraph() {
    let mut s = slide_with(vec![]);
    s.shapes[0].text = Some(TextBody::from_text("one\ntwo\nthree"));
    let mut a = anim(1, AnimClass::Entrance, "fade", AnimStart::OnClick);
    a.text_build = TextBuild::ByParagraph;
    s.animations = vec![a.clone()];
    let tl = Timeline::new(&s, W, H);
    let n = tl.paragraph_targets(ShapeId(1)).len();
    assert_eq!(n, 3);
    // On click: one click per paragraph.
    assert_eq!(tl.steps(), n);
    if n >= 2 {
        assert!(tl.state(ShapeId(1), Some(0), 1, 0.0).visible);
        assert!(!tl.state(ShapeId(1), Some(1), 1, 0.0).visible);
    }
    // With previous: staggered within one step.
    a.start = AnimStart::WithPrevious;
    s.animations = vec![a];
    let tl = Timeline::new(&s, W, H);
    assert_eq!(tl.steps(), 1);
    let e = tl.effects();
    if e.len() >= 2 {
        assert!((e[1].start - e[0].start - 0.5).abs() < 1e-9);
    }
    // The shape itself isn't hidden.
    assert!(tl.state(ShapeId(1), None, 0, 0.0).visible);
}

#[test]
fn by_paragraph_groups_sub_bullets_with_their_parent() {
    let mut s = slide_with(vec![]);
    let mut body = TextBody::from_text("one\nsub a\nsub b\ntwo");
    body.paragraphs[1].level = 1;
    body.paragraphs[2].level = 1;
    s.shapes[0].text = Some(body);
    let mut a = anim(1, AnimClass::Entrance, "fade", AnimStart::OnClick);
    a.text_build = TextBuild::ByParagraph;
    s.animations = vec![a];
    let tl = Timeline::new(&s, W, H);
    assert_eq!(tl.steps(), 2);
    assert_eq!(tl.paragraph_targets(ShapeId(1)), vec![0, 1, 2, 3]);
    assert!(tl.state(ShapeId(1), Some(2), 1, 0.0).visible);
    assert!(!tl.state(ShapeId(1), Some(3), 1, 0.0).visible);
    assert!(!tl.state(ShapeId(1), Some(1), 0, 0.0).visible);
}

#[test]
fn hostile_times_and_durations() {
    let mut a = anim(1, AnimClass::Entrance, "fly", AnimStart::OnClick);
    a.duration_ms = u32::MAX;
    a.delay_ms = u32::MAX;
    a.repeat = u32::MAX - 1;
    a.smooth_start = f64::NAN;
    a.smooth_end = -5.0;
    a.bounce_end = f64::INFINITY;
    a.amount = Some(f64::NAN);
    let mut b = anim(2, AnimClass::Emphasis, "growShrink", AnimStart::AfterPrevious);
    b.amount = Some(1e300);
    b.duration_ms = 0;
    let tl = Timeline::new(&slide_with(vec![a, b, anim(99, AnimClass::Path, "customPath", AnimStart::OnClick)]), f64::NAN, -1.0);
    for t in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY, -1.0, 0.0, 1e12, 1e300] {
        for step in [0, 1, 2, 3, usize::MAX] {
            for id in [1, 2, 99] {
                let s = tl.state(ShapeId(id), None, step, t);
                assert!(finite(&s));
                let s = tl.state(ShapeId(id), Some(usize::MAX), step, t);
                assert!(finite(&s));
            }
        }
        let _ = tl.locate(t);
        let _ = tl.state_at(ShapeId(1), None, t);
    }
    assert!(tl.step_duration(usize::MAX) == 0.0);
}

#[test]
fn empty_slide() {
    let tl = Timeline::new(&Slide::default(), W, H);
    assert_eq!(tl.steps(), 0);
    assert!(tl.is_empty());
    assert!(tl.state(ShapeId(1), None, 0, 1.0).is_identity());
    assert_eq!(tl.locate(5.0), (0, 0.0));
}

#[test]
fn start_of_seeks_the_play_all_clock_to_an_effect() {
    let mut late = anim(4, AnimClass::Entrance, "fade", AnimStart::WithPrevious);
    late.delay_ms = 250;
    let mut triggered = anim(4, AnimClass::Emphasis, "pulse", AnimStart::OnClick);
    triggered.trigger = Some(ShapeId(1));
    let tl = Timeline::new(
        &slide_with(vec![
            anim(1, AnimClass::Entrance, "fade", AnimStart::OnClick),
            anim(2, AnimClass::Exit, "fadeOut", AnimStart::OnClick),
            anim(3, AnimClass::Entrance, "fade", AnimStart::OnClick),
            late,
            triggered,
        ]),
        W,
        H,
    );
    assert_eq!(tl.start_of(0), Some(0.0));
    assert_eq!(tl.start_of(2), Some(2.0));
    assert_eq!(tl.start_of(3), Some(2.25));
    assert_eq!(tl.start_of(4), None, "a triggered effect is not on the play-all clock");
    assert_eq!(tl.start_of(9), None);
    // Playing from the third effect: what came before has played, the rest has not.
    let at = tl.start_of(2).unwrap();
    assert!(tl.state_at(ShapeId(1), None, at).visible, "entered before");
    assert!(!tl.state_at(ShapeId(2), None, at).visible, "exited before");
    assert!(!tl.state_at(ShapeId(3), None, at).visible, "not entered yet");
    assert!(!tl.state_at(ShapeId(4), None, at).visible, "not entered yet");
}

#[test]
fn locate_play_all_clock() {
    let tl = Timeline::new(
        &slide_with(vec![anim(1, AnimClass::Entrance, "fade", AnimStart::OnClick), anim(2, AnimClass::Entrance, "fade", AnimStart::OnClick)]),
        W,
        H,
    );
    assert_eq!(tl.locate(0.5), (0, 0.5));
    assert_eq!(tl.locate(1.5), (1, 0.5));
    assert_eq!(tl.locate(9.0), (2, 0.0));
    assert!((tl.total_duration() - 2.0).abs() < 1e-9);
}

#[test]
fn motion_path_parse_and_eval() {
    let p = MotionPath::parse("M 0 0 L 0.25 0 L 0.25 0.25 E");
    assert!((p.length() - 0.5).abs() < 1e-9);
    let (x, y) = p.at(0.5);
    assert!((x - 0.25).abs() < 1e-9 && y.abs() < 1e-9);
    let (x, y) = p.at(1.0);
    assert!((x - 0.25).abs() < 1e-9 && (y - 0.25).abs() < 1e-9);
    // Relative commands and curves.
    let q = MotionPath::parse("m 0 0 l 0.1 0 c 0 0.1 0 0.1 0 0.2 z");
    assert!(q.length() > 0.0);
    let c = MotionPath::parse("M0,0C0.1,0,0.2,0.1,0.2,0.2E");
    assert!(c.points.len() > 2);
    let e = MotionPath::parse("M 0 0 L 1e-1 -2.5E-1 E");
    let (x, y) = e.at(1.0);
    assert!((x - 0.1).abs() < 1e-9 && (y + 0.25).abs() < 1e-9);
}

#[test]
fn malformed_paths_dont_panic() {
    for s in [
        "",
        "E",
        "M",
        "M 0",
        "L 1 1",
        "C 1 2 3",
        "M 0 0 L x y",
        "M NaN 1 L inf 2",
        "M 1e400 0 L 0 0",
        "Z Z Z",
        "M 0 0 Q",
        "--..ee",
        "M 0 0 H 1 V 1 h -1 v -1",
        "😀 M 1 1",
        "M 0 0 L 0.5 0.5 X 9 9",
    ] {
        let p = MotionPath::parse(s);
        for t in [f64::NAN, -1.0, 0.0, 0.5, 1.0, 2.0] {
            let (x, y) = p.at(t);
            assert!(x.is_finite() && y.is_finite(), "{s}");
        }
    }
    let big = "L 1 1 ".repeat(50_000);
    let p = MotionPath::parse(&big);
    assert!(p.points.len() <= 20_000);
}

#[test]
fn preset_paths_move() {
    for (id, _, class, _, _, opts) in ANIMATIONS.iter().filter(|a| a.2 == AnimClass::Path && a.0 != "customPath") {
        let _ = class;
        for o in opts.iter().copied().chain([""]) {
            let p = MotionPath::parse(&default_path(id, o));
            assert!(p.length() > 0.0, "{id} {o}");
        }
    }
    let mut a = anim(1, AnimClass::Path, "lines", AnimStart::OnClick);
    a.option = "right".into();
    let tl = Timeline::new(&slide_with(vec![a]), W, H);
    let s = tl.state(ShapeId(1), None, 1, 0.0);
    assert!((s.offset_x - 0.25 * W).abs() < 1e-6 && s.offset_y.abs() < 1e-6);
}

#[test]
fn custom_path_offsets_in_points() {
    let mut a = anim(1, AnimClass::Path, "customPath", AnimStart::OnClick);
    a.path = Some("M 0 0 L 0.5 0.5 E".into());
    let tl = Timeline::new(&slide_with(vec![a]), W, H);
    let s = tl.state(ShapeId(1), None, 1, 0.0);
    assert!((s.offset_x - 480.0).abs() < 1e-6 && (s.offset_y - 270.0).abs() < 1e-6);
}

#[test]
fn easing_endpoints() {
    for (a, d, b) in [(0.0, 0.0, 0.0), (0.5, 0.5, 0.0), (0.3, 0.0, 0.0), (0.0, 0.7, 0.0), (0.9, 0.9, 0.0), (0.0, 0.0, 0.3)] {
        assert!(ease(0.0, a, d, b).abs() < 1e-9, "{a} {d} {b}");
        assert!((ease(1.0, a, d, b) - 1.0).abs() < 1e-9, "{a} {d} {b}");
        assert!(ease(0.5, a, d, b).is_finite());
    }
    assert!(ease(f64::NAN, f64::NAN, f64::NAN, f64::NAN).is_finite());
}

fn covers_at_end(layers: &[Layer]) -> bool {
    layers.iter().any(|l| {
        l.source == Source::New
            && (l.alpha - 1.0).abs() < 1e-9
            && l.quad == [[0.0, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 1.0]]
            && l.uv == [[0.0, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 1.0]]
    })
}

#[test]
fn every_transition_finite_and_complete() {
    for (id, _, _, _, opts) in TRANSITIONS {
        let mut options: Vec<&str> = opts.to_vec();
        options.push("");
        for o in options {
            for t in [0.0, 0.01, 0.25, 0.5, 0.75, 0.99, 1.0] {
                let ls = transition_layers(id, o, t);
                assert!(!ls.is_empty(), "{id} {o} {t}");
                assert!(ls.len() < 5000, "{id} {o} {t}");
                for l in &ls {
                    assert!(l.alpha.is_finite() && (0.0..=1.0).contains(&l.alpha));
                    assert!(l.quad.iter().chain(l.uv.iter()).all(|p| p[0].is_finite() && p[1].is_finite()));
                    assert!(l.src.iter().all(|v| v.is_finite()));
                }
            }
            assert!(covers_at_end(&transition_layers(id, o, 1.0)), "{id} {o}");
            let start = transition_layers(id, o, 0.0);
            assert!(start.iter().any(|l| l.source == Source::Old && l.alpha == 1.0), "{id} {o}");
        }
    }
}

#[test]
fn transitions_hostile_t() {
    for (id, ..) in TRANSITIONS {
        for t in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY, -5.0, 1e12] {
            let ls = transition_layers(id, "bogus-option", t);
            assert!(!ls.is_empty());
        }
    }
    assert!(!transition_layers("no-such-kind", "", 0.5).is_empty());
}

#[test]
fn fade_and_push_shapes() {
    let ls = transition_layers("fade", "smoothly", 0.5);
    assert_eq!(ls.len(), 2);
    assert_eq!(ls[1].source, Source::New);
    assert!((ls[1].alpha - 0.5).abs() < 1e-9);
    let ls = transition_layers("push", "l", 0.5);
    let new = ls.iter().find(|l| l.source == Source::New).unwrap();
    // Pushing left: the new slide comes from the right.
    assert!(new.quad[0][0] > 0.0);
    let ls = transition_layers("fade", "throughBlack", 0.25);
    assert!(ls.iter().all(|l| l.source != Source::New));
}

#[test]
fn mid_transitions_mix_sources() {
    // Most transitions show something of both slides (or black/white) midway.
    for (id, ..) in TRANSITIONS.iter().filter(|t| !matches!(t.0, "none" | "cut" | "flip")) {
        let ls = transition_layers(id, "", 0.5);
        assert!(ls.iter().any(|l| l.source != Source::New) || ls.len() > 1, "{id}");
    }
}

#[test]
fn random_is_deterministic() {
    assert_eq!(transition_layers("random", "x", 0.4), transition_layers("random", "x", 0.4));
    assert_eq!(transition_layers("dissolve", "", 0.4), transition_layers("dissolve", "", 0.4));
}

#[test]
fn morph_pairs_by_name_text_and_geometry() {
    let mut a = Slide::default();
    let mut b = Slide::default();
    a.shapes = vec![shape(1, "!!Logo"), shape(2, "Title 1"), shape(3, "x"), shape(4, "Rect A")];
    b.shapes = vec![shape(14, "Rect B"), shape(13, "y"), shape(12, "Title 1"), shape(11, "!!Logo")];
    a.shapes[2].text = Some(TextBody::from_text("Hello"));
    b.shapes[1].text = Some(TextBody::from_text("Hello"));
    let p = morph_pairs(&a, &b);
    assert!(p.contains(&(ShapeId(1), ShapeId(11))));
    assert!(p.contains(&(ShapeId(2), ShapeId(12))));
    assert!(p.contains(&(ShapeId(3), ShapeId(13))));
    assert!(p.contains(&(ShapeId(4), ShapeId(14))));
    assert_eq!(p.len(), 4);
    assert!(morph_pairs(&Slide::default(), &b).is_empty());
}

#[test]
fn morph_pairs_leaves_unrelated_text_boxes_alone() {
    // A text box that leaves and an unrelated one that arrives fade in place (#119).
    let p = deckcraft_model::defaults::new_presentation(None);
    let mut a = Slide::default();
    let mut b = Slide::default();
    let tb = |id, name, x, text| Shape { xfrm: Some(Xfrm::new(x, 100.0, 200.0, 100.0)), text: Some(TextBody::from_text(text)), ..shape(id, name) };
    a.shapes = vec![tb(1, "TextBox 1", 0.0, "Gone")];
    b.shapes = vec![tb(2, "TextBox 2", 400.0, "New")];
    assert!(morph_pairs(&a, &b).is_empty());
    let f = morph_frame(&p, &a, &b, 0.5, &|_| false);
    let xs: Vec<(&str, f64)> = f.shapes.iter().map(|(s, _)| (s.name.as_str(), s.xfrm.map_or(f64::NAN, |x| x.x))).collect();
    assert_eq!(xs, [("TextBox 1", 0.0), ("TextBox 2", 400.0)]);
    // Empty shapes of one kind and geometry still pair, and a named text box keeps its partner.
    a.shapes.push(shape(3, "Rect A"));
    b.shapes.push(shape(4, "Rect B"));
    b.shapes[0].name = "TextBox 1".into();
    assert_eq!(morph_pairs(&a, &b), [(ShapeId(1), ShapeId(2)), (ShapeId(3), ShapeId(4))]);
}

#[test]
fn morph_crop_moves_the_image_linearly() {
    let bx = |x, w| Xfrm::new(x, 0.0, w, 100.0);
    // Same box, the crop pans from the left half to the right half of a 200-wide image.
    let c = morph_crop([0.0, 0.0, 0.5, 0.0], bx(0.0, 100.0), [0.5, 0.0, 0.0, 0.0], bx(0.0, 100.0), bx(0.0, 100.0), 0.5);
    assert!(c.iter().zip([0.25, 0.0, 0.25, 0.0]).all(|(a, b)| (a - b).abs() < 1e-9), "{c:?}");
    // The box shrinks over an image that stays put: only the right crop grows.
    let c = morph_crop([0.0; 4], bx(0.0, 100.0), [0.0, 0.0, 0.5, 0.0], bx(0.0, 50.0), bx(0.0, 75.0), 0.5);
    assert!(c.iter().zip([0.0, 0.0, 0.25, 0.0]).all(|(a, b)| (a - b).abs() < 1e-9), "{c:?}");
    // Nothing visible on one side: the crops blend.
    let c = morph_crop([0.5, 0.0, 0.5, 0.0], bx(0.0, 100.0), [0.0; 4], bx(0.0, 100.0), bx(0.0, 100.0), 0.5);
    assert!(c.iter().all(|v| v.is_finite()) && (c[0] - 0.25).abs() < 1e-9, "{c:?}");
}

#[test]
fn morph_frame_morphs_a_picture_crop() {
    // A picture recropped on the next slide stays one picture whose crop morphs (#118).
    let p = deckcraft_model::defaults::new_presentation(None);
    let pic = |id, crop| Shape {
        kind: deckcraft_model::ShapeKind::Picture { fill: deckcraft_model::style::PictureFill { crop, ..Default::default() } },
        ..shape(id, "Picture 1")
    };
    let mut a = Slide::default();
    let mut b = Slide::default();
    a.shapes = vec![pic(1, [0.0, 0.0, 0.5, 0.0])];
    b.shapes = vec![pic(2, [0.5, 0.0, 0.0, 0.0])];
    let f = morph_frame(&p, &a, &b, 0.5, &|_| false);
    assert_eq!(f.shapes.len(), 1);
    assert!(f.opacity.is_empty());
    let deckcraft_model::ShapeKind::Picture { fill } = &f.shapes[0].0.kind else { panic!("not a picture") };
    let want = 0.5 * f.mix;
    assert!((fill.crop[0] - want).abs() < 1e-9 && (fill.crop[2] - (0.5 - want)).abs() < 1e-9, "{:?}", fill.crop);
    assert_eq!(morph_frame(&p, &a, &b, 1.0, &|_| false).shapes[0].0.kind, b.shapes[0].kind);
}

#[test]
fn morph_xfrm_interpolates() {
    let a = Xfrm { rot: 350.0, ..Xfrm::new(0.0, 0.0, 100.0, 100.0) };
    let b = Xfrm { rot: 10.0, ..Xfrm::new(100.0, 50.0, 200.0, 100.0) };
    let m = morph_xfrm(a, b, 0.5);
    assert!((m.x - 50.0).abs() < 1e-9 && (m.w - 150.0).abs() < 1e-9);
    assert!(m.rot.abs() < 1e-9 || (m.rot - 360.0).abs() < 1e-9, "{}", m.rot);
    assert_eq!(morph_xfrm(a, b, 1.0), b);
    assert_eq!(morph_xfrm(a, b, f64::NAN), a);
    let n = Xfrm { x: f64::NAN, rot: f64::INFINITY, ..Xfrm::default() };
    let m = morph_xfrm(n, b, 0.3);
    assert!(m.rot.is_finite());
}

fn solid(s: Shape, r: u8) -> Shape {
    Shape { fill: Some(deckcraft_model::Fill::solid(ColorRef::rgb(Rgba { r, g: 0, b: 0, a: 255 }))), ..s }
}

#[test]
fn morph_frame_moves_blends_and_fades() {
    let p = deckcraft_model::defaults::new_presentation(None);
    let mut a = Slide::default();
    let mut b = Slide::default();
    let moved = |id, x| Shape { xfrm: Some(Xfrm::new(x, 100.0, 200.0, 100.0)), ..shape(id, "Box") };
    a.shapes = vec![solid(moved(1, 0.0), 0), shape(2, "Gone")];
    let new = Shape { text: Some(TextBody::from_text("Hi")), ..shape(3, "New") };
    b.shapes = vec![solid(moved(1, 400.0), 200), new, shape(4, "Later")];
    let f = morph_frame(&p, &a, &b, 0.5, &|id| id == ShapeId(4));
    assert!((f.mix - 0.5).abs() < 1e-9);
    // Gone fades out, Box moves halfway and blends red, New fades in, Later stays out.
    let names: Vec<(&str, bool)> = f.shapes.iter().map(|(s, old)| (s.name.as_str(), *old)).collect();
    assert_eq!(names, [("Gone", true), ("Box", false), ("New", false)]);
    let m = &f.shapes[1].0;
    assert!((m.xfrm.unwrap().x - 200.0).abs() < 1e-9);
    assert_eq!(m.fill, Some(deckcraft_model::Fill::solid(ColorRef::rgb(Rgba { r: 100, g: 0, b: 0, a: 255 }))));
    assert_eq!(f.opacity_of(m.id), 1.0);
    assert!((f.opacity_of(f.shapes[0].0.id) - 0.5).abs() < 1e-9);
    assert!((f.opacity_of(f.shapes[2].0.id) - 0.5).abs() < 1e-9);
    // Ids are fresh and unique.
    let mut ids: Vec<_> = f.shapes.iter().map(|(s, _)| s.id).collect();
    ids.sort();
    ids.dedup();
    assert_eq!(ids.len(), 3);
    assert!(ids.iter().all(|i| i.0 > 4));
    // At the start only the old slide shows.
    let f0 = morph_frame(&p, &a, &b, 0.0, &|id| id == ShapeId(4));
    assert!(f0.shapes.iter().filter(|(s, _)| f0.opacity_of(s.id) > 0.0).all(|(s, _)| s.name != "New"));
    assert_eq!(morph_frame(&p, &a, &b, f64::NAN, &|_| false).mix, 0.0);
}

#[test]
fn morph_frame_cross_fades_changed_content() {
    let p = deckcraft_model::defaults::new_presentation(None);
    let mut a = Slide::default();
    let mut b = Slide::default();
    a.shapes = vec![Shape { text: Some(TextBody::from_text("Before")), ..shape(1, "T") }];
    b.shapes = vec![Shape { text: Some(TextBody::from_text("After")), ..shape(1, "T") }];
    let f = morph_frame(&p, &a, &b, 0.25, &|_| false);
    // The new text under the fading old one, both on the moving box.
    assert_eq!(f.shapes.len(), 2);
    assert_eq!((&f.shapes[0].0.text, f.shapes[0].1), (&b.shapes[0].text, false));
    assert_eq!((&f.shapes[1].0.text, f.shapes[1].1), (&a.shapes[0].text, true));
    let s = f.mix;
    assert!((f.opacity_of(f.shapes[0].0.id) - s).abs() < 1e-9);
    assert!((f.opacity_of(f.shapes[1].0.id) - (1.0 - s)).abs() < 1e-9);
}

#[test]
fn morph_frame_lists_texts_for_words_and_characters() {
    let p = deckcraft_model::defaults::new_presentation(None);
    let mut a = Slide::default();
    let mut b = Slide::default();
    a.shapes = vec![Shape { text: Some(TextBody::from_text("The Science")), ..shape(1, "T") }];
    b.shapes = vec![Shape { text: Some(TextBody::from_text("Science rocks")), ..shape(1, "T") }];
    for (option, want) in [("objects", None), ("words", Some(false)), ("characters", Some(true))] {
        b.transition = Some(Transition { kind: "morph".into(), option: option.into(), ..Default::default() });
        let f = morph_frame(&p, &a, &b, 0.5, &|_| false);
        assert_eq!(f.text.first().map(|m| m.chars), want, "{option}");
        if let Some(m) = f.text.first() {
            assert_eq!((m.new, m.old), (f.shapes[0].0.id, f.shapes[1].0.id));
            assert_eq!(m.from, a.shapes[0].xfrm.unwrap());
        }
    }
    // Same text: one shape moves, nothing to morph.
    b.shapes[0].text = a.shapes[0].text.clone();
    assert!(morph_frame(&p, &a, &b, 0.5, &|_| false).text.is_empty());
}

#[test]
fn morph_frame_morphs_the_outline_of_a_changed_geometry() {
    let p = deckcraft_model::defaults::new_presentation(None);
    let mut a = Slide::default();
    let mut b = Slide::default();
    let geo = |s: Shape, g: &str| Shape { geom: deckcraft_model::Geom::preset(g), ..s };
    a.shapes = vec![solid(geo(shape(1, "Form"), "ellipse"), 0)];
    b.shapes = vec![solid(Shape { xfrm: Some(Xfrm::new(300.0, 100.0, 100.0, 100.0)), ..geo(shape(1, "Form"), "star5") }, 200)];
    let f = morph_frame(&p, &a, &b, 0.5, &|_| false);
    // One shape: the new one, on the moving box, its colour blending and its outline listed to morph.
    assert_eq!(f.shapes.len(), 1);
    let (m, old) = &f.shapes[0];
    assert!(!old && m.geom == b.shapes[0].geom);
    assert_eq!(f.opacity_of(m.id), 1.0);
    assert_eq!(m.fill, Some(deckcraft_model::Fill::solid(ColorRef::rgb(Rgba { r: 100, g: 0, b: 0, a: 255 }))));
    assert_eq!(f.paths.len(), 1);
    assert_eq!(f.paths[0].id, m.id);
    assert_eq!(f.paths[0].from, a.shapes[0].geom);
    assert_eq!((f.paths[0].from_box, f.paths[0].to_box), (a.shapes[0].xfrm.unwrap(), b.shapes[0].xfrm.unwrap()));
    // Other adjust values of the same preset morph too.
    b.shapes[0].geom = deckcraft_model::Geom::Preset { name: "ellipse".into(), adj: vec![1.0] };
    assert_eq!(morph_frame(&p, &a, &b, 0.5, &|_| false).paths.len(), 1);
    // The same geometry only moves.
    b.shapes[0].geom = a.shapes[0].geom.clone();
    assert!(morph_frame(&p, &a, &b, 0.5, &|_| false).paths.is_empty());
    // Another text as well: the two cross-fade as before.
    b.shapes[0].geom = deckcraft_model::Geom::preset("star5");
    b.shapes[0].text = Some(TextBody::from_text("Star"));
    let f = morph_frame(&p, &a, &b, 0.5, &|_| false);
    assert!(f.paths.is_empty() && f.shapes.len() == 2);
    // Tables and groups have no outline to morph.
    let table = |g: &str| Shape { kind: deckcraft_model::ShapeKind::Table(Default::default()), ..geo(shape(1, "Form"), g) };
    a.shapes = vec![table("rect")];
    b.shapes = vec![table("ellipse")];
    assert!(morph_frame(&p, &a, &b, 0.5, &|_| false).paths.is_empty());
}

fn pres(n: usize) -> Presentation {
    Presentation { slides: (0..n).map(|i| Arc::new(Slide { id: SlideId(1000 + i as u32), ..Default::default() })).collect(), ..Default::default() }
}

#[test]
fn show_skips_hidden_slides() {
    let mut p = pres(4);
    Arc::make_mut(&mut p.slides[1]).hidden = true;
    let mut s = ShowState::new(&p);
    assert_eq!(s.slide, 0);
    assert_eq!(s.next(&p), ShowAction::Slide(2));
    assert_eq!(s.prev(&p), ShowAction::Slide(0));
    // Jumping to a hidden slide shows it; next continues after it.
    assert_eq!(s.goto(&p, 1), ShowAction::Slide(1));
    assert_eq!(s.next(&p), ShowAction::Slide(2));
    assert_eq!(s.next(&p), ShowAction::Slide(3));
    assert_eq!(s.next(&p), ShowAction::End);
    assert_eq!(s.next(&p), ShowAction::Exit);
    assert_eq!(s.goto(&p, 99), ShowAction::None);
}

#[test]
fn show_remembers_a_slide_was_reached_by_going_back() {
    // #1531: the UI plays the left slide's transition reversed only for Back.
    let p = pres(3);
    let mut s = ShowState::new(&p);
    assert_eq!(s.next(&p), ShowAction::Slide(1));
    assert!(!s.backward);
    assert_eq!(s.prev(&p), ShowAction::Slide(0));
    assert!(s.backward);
    assert_eq!(s.next(&p), ShowAction::Slide(1));
    assert!(!s.backward);
    assert_eq!(s.goto(&p, 0), ShowAction::Slide(0));
    assert!(!s.backward, "a jump is not Back");
    // Back from the end screen shows the last slide again, without a transition to reverse.
    assert_eq!(s.goto(&p, 2), ShowAction::Slide(2));
    assert_eq!(s.next(&p), ShowAction::End);
    assert_eq!(s.prev(&p), ShowAction::Slide(2));
    assert!(!s.backward);
}

#[test]
fn show_loops() {
    let mut p = pres(3);
    p.show.loop_until_esc = true;
    let mut s = ShowState::new(&p);
    s.next(&p);
    s.next(&p);
    assert_eq!(s.next(&p), ShowAction::Slide(0));
    assert_eq!(s.prev(&p), ShowAction::Slide(2));
}

#[test]
fn show_custom_show_and_range() {
    let mut p = pres(5);
    p.custom_shows = vec![CustomShow { name: "Short".into(), slides: vec![SlideId(1003), SlideId(1001)] }];
    p.show.custom_show = Some("Short".into());
    assert_eq!(show_order(&p), vec![3, 1]);
    let mut s = ShowState::new(&p);
    assert_eq!(s.slide, 3);
    assert_eq!(s.next(&p), ShowAction::Slide(1));
    p.show.custom_show = None;
    p.show.range = Some((2, 4));
    assert_eq!(show_order(&p), vec![1, 2, 3]);
    p.show.range = Some((0, 99));
    assert_eq!(show_order(&p), vec![0, 1, 2, 3, 4]);
}

#[test]
fn playing_a_custom_show_stays_on_its_slides() {
    use deckcraft_model::text::Action;
    let mut p = pres(5);
    p.custom_shows = vec![CustomShow { name: "Ends".into(), slides: vec![SlideId(1000), SlideId(1004)] }];
    assert!(p.show.custom_show.is_none());
    let mut s = ShowState::play_custom(&p, "Ends").expect("the show exists");
    assert_eq!(s.custom_name(), Some("Ends"));
    assert_eq!(s.slide, 0);
    assert_eq!(s.peek_next(&p), Some(4));
    assert_eq!(s.next(&p), ShowAction::Slide(4));
    assert_eq!(s.next(&p), ShowAction::End);
    assert_eq!(s.prev(&p), ShowAction::Slide(4));
    assert_eq!(s.prev(&p), ShowAction::Slide(0));
    assert!(ShowState::play_custom(&p, "Missing").is_none());
    assert!(p.show.custom_show.is_none(), "one-shot playback does not change Set Up");

    Arc::make_mut(&mut p.slides[4]).hidden = true;
    let only = ShowState::play_custom(&p, "Ends").expect("the visible slide remains");
    assert_eq!(only.first_index(&p), Some(0));
    assert_eq!(only.last_index(&p), Some(0));

    let mut s = ShowState::new(&p);
    p.slides[4] = Arc::new(Slide { id: SlideId(1004), ..Default::default() });
    assert_eq!(s.follow(&p, &Action::CustomShow { name: "Ends".into(), returns: true }, None), LinkJump::Show(ShowAction::Slide(0)));
    assert_eq!(s.custom_name(), Some("Ends"));
    assert_eq!(s.next(&p), ShowAction::Slide(4));
    assert_eq!(s.follow(&p, &Action::CustomShow { name: "Missing".into(), returns: true }, None), LinkJump::None);
    assert_eq!(s.slide, 4, "an unknown custom show leaves the current slide");
}

#[test]
fn show_steps_through_builds() {
    let mut p = pres(2);
    Arc::make_mut(&mut p.slides[0]).shapes = vec![shape(1, "A")];
    Arc::make_mut(&mut p.slides[0]).animations = vec![anim(1, AnimClass::Entrance, "fade", AnimStart::OnClick)];
    let mut s = ShowState::new(&p);
    assert!(!s.playing);
    assert_eq!(s.next(&p), ShowAction::PlayStep(0));
    assert!(s.playing);
    assert_eq!(s.step_done(&p), ShowAction::FinishStep(0));
    assert_eq!(s.step, 1);
    assert_eq!(s.prev(&p), ShowAction::StepBack(0));
    assert_eq!(s.next(&p), ShowAction::PlayStep(0));
    assert_eq!(s.next(&p), ShowAction::FinishStep(0));
    assert_eq!(s.next(&p), ShowAction::Slide(1));
    // Back to a fully built previous slide.
    assert_eq!(s.prev(&p), ShowAction::Slide(0));
    assert_eq!(s.step, 1);
}

#[test]
fn show_back_leaves_a_slide_whose_first_step_is_automatic() {
    // #844: undoing the automatic first step left the slide as before it appeared, and the show
    // played that step again at once, so Back never reached the slide before.
    for start in [AnimStart::WithPrevious, AnimStart::AfterPrevious] {
        let mut p = pres(2);
        Arc::make_mut(&mut p.slides[1]).shapes = vec![shape(1, "A"), shape(2, "B")];
        Arc::make_mut(&mut p.slides[1]).animations =
            vec![anim(1, AnimClass::Entrance, "fade", start), anim(2, AnimClass::Entrance, "fade", AnimStart::OnClick)];
        let mut s = ShowState::new(&p);
        assert_eq!(s.next(&p), ShowAction::Slide(1));
        assert!(s.playing, "{start:?}");
        // While the automatic step plays.
        assert_eq!(s.prev(&p), ShowAction::Slide(0), "{start:?}");
        assert_eq!(s.next(&p), ShowAction::Slide(1));
        // Going forward again plays the automatic step again (#2090).
        assert_eq!(s.step_done(&p), ShowAction::FinishStep(0), "{start:?}");
        // Once it has played, the click step is undone first, then Back leaves the slide.
        assert_eq!(s.next(&p), ShowAction::PlayStep(1));
        assert_eq!(s.step_done(&p), ShowAction::FinishStep(1));
        assert_eq!(s.prev(&p), ShowAction::StepBack(1), "{start:?}");
        assert_eq!(s.prev(&p), ShowAction::Slide(0), "{start:?}");
        assert_eq!((s.step, s.playing), (0, false));
    }
}

#[test]
fn a_slide_entered_going_forward_again_plays_its_animations_again() {
    // #2090: the show remembered slide 2 as fully built, so its entrance effects did not play the
    // second time the show advanced to it.
    for start in [AnimStart::WithPrevious, AnimStart::AfterPrevious] {
        let mut p = pres(3);
        Arc::make_mut(&mut p.slides[1]).shapes = vec![shape(1, "A")];
        Arc::make_mut(&mut p.slides[1]).animations = vec![anim(1, AnimClass::Entrance, "fade", start)];
        let mut s = ShowState::new(&p);
        assert_eq!(s.next(&p), ShowAction::Slide(1));
        assert_eq!(s.step_done(&p), ShowAction::FinishStep(0));
        assert_eq!(s.prev(&p), ShowAction::Slide(0), "{start:?}");
        assert_eq!(s.next(&p), ShowAction::Slide(1));
        assert_eq!((s.step, s.playing), (0, true), "{start:?}: the slide starts again");
        let a = ShowState::timeline(&p, 1).state(ShapeId(1), None, s.step, 0.0);
        assert!(!a.visible || a.opacity < 0.01, "{start:?}: the shape has not come in yet");
        // Back from the next slide still shows it as it was left.
        assert_eq!(s.step_done(&p), ShowAction::FinishStep(0));
        assert_eq!(s.next(&p), ShowAction::Slide(2));
        assert_eq!(s.prev(&p), ShowAction::Slide(1));
        assert_eq!((s.step, s.playing), (1, false), "{start:?}");
    }
    // An On Click build left by a jump starts again when the show advances to the slide.
    let mut p = pres(2);
    Arc::make_mut(&mut p.slides[1]).shapes = vec![shape(1, "A")];
    Arc::make_mut(&mut p.slides[1]).animations = vec![anim(1, AnimClass::Entrance, "fade", AnimStart::OnClick)];
    let mut s = ShowState::new(&p);
    assert_eq!(s.next(&p), ShowAction::Slide(1));
    assert_eq!(s.next(&p), ShowAction::PlayStep(0));
    assert_eq!(s.step_done(&p), ShowAction::FinishStep(0));
    assert_eq!(s.goto(&p, 0), ShowAction::Slide(0));
    assert_eq!(s.next(&p), ShowAction::Slide(1));
    assert_eq!((s.step, s.playing), (0, false));
}

#[test]
fn a_looping_show_plays_the_first_slide_again() {
    // #2090: a kiosk or loop-until-Esc show comes round to the first slide going forward.
    let mut p = pres(2);
    p.show.loop_until_esc = true;
    Arc::make_mut(&mut p.slides[0]).shapes = vec![shape(1, "A")];
    Arc::make_mut(&mut p.slides[0]).animations = vec![anim(1, AnimClass::Entrance, "fade", AnimStart::WithPrevious)];
    let mut s = ShowState::new(&p);
    assert!(s.playing);
    assert_eq!(s.step_done(&p), ShowAction::FinishStep(0));
    assert_eq!(s.next(&p), ShowAction::Slide(1));
    assert_eq!(s.next(&p), ShowAction::Slide(0));
    assert_eq!((s.step, s.playing), (0, true));
}

#[test]
fn show_auto_advance() {
    let mut p = pres(2);
    Arc::make_mut(&mut p.slides[0]).transition = Some(Transition { advance_after_ms: Some(2000), ..Default::default() });
    let s = ShowState::new(&p);
    assert_eq!(s.advance_after(&p), Some(2.0));
    assert!(!s.auto_advance_due(&p, 1.0));
    assert!(s.auto_advance_due(&p, 2.5));
    assert!(!s.auto_advance_due(&p, f64::NAN));
    p.show.use_timings = false;
    assert!(!s.auto_advance_due(&p, 5.0));
}

#[test]
fn show_empty_and_all_hidden() {
    let p = pres(0);
    let mut s = ShowState::new(&p);
    assert!(s.ended);
    assert_eq!(s.next(&p), ShowAction::Exit);
    let _ = s.prev(&p);
    let mut p = pres(2);
    for sl in &mut p.slides {
        Arc::make_mut(sl).hidden = true;
    }
    let mut s = ShowState::new(&p);
    assert!(s.ended);
    let _ = s.next(&p);
    let _ = s.prev(&p);
}

#[test]
fn compose_and_sanitize() {
    let a = AnimState { opacity: 0.5, clip: Some([0.0, 0.0, 0.5, 1.0]), ..Default::default() };
    let b = AnimState { opacity: 0.5, offset_x: 3.0, clip: Some([0.25, 0.0, 1.0, 1.0]), ..Default::default() };
    let c = a.compose(&b);
    assert!((c.opacity - 0.25).abs() < 1e-9 && c.offset_x == 3.0 && c.clip == Some([0.25, 0.0, 0.5, 1.0]));
    let bad = AnimState { opacity: f64::NAN, rotate: f64::INFINITY, clip: Some([f64::NAN, 2.0, -1.0, 0.5]), ..Default::default() }.sanitized();
    assert!(finite(&bad));
}

#[test]
fn show_follows_links() {
    use deckcraft_model::text::Action;
    let mut p = pres(4);
    Arc::make_mut(&mut p.slides[3]).hidden = true;
    let mut s = ShowState::new(&p);
    assert_eq!(s.follow(&p, &Action::Slide { slide: SlideId(1002) }, None), LinkJump::Show(ShowAction::Slide(2)));
    assert_eq!(s.follow(&p, &Action::PreviousSlide, None), LinkJump::Show(ShowAction::Slide(1)));
    assert_eq!(s.follow(&p, &Action::NextSlide, None), LinkJump::Show(ShowAction::Slide(2)));
    // Last skips the hidden slide; a link to the hidden slide shows it.
    assert_eq!(s.follow(&p, &Action::LastSlide, None), LinkJump::Show(ShowAction::Slide(2)));
    assert_eq!(s.follow(&p, &Action::Slide { slide: SlideId(1003) }, None), LinkJump::Show(ShowAction::Slide(3)));
    assert_eq!(s.follow(&p, &Action::FirstSlide, None), LinkJump::Show(ShowAction::Slide(0)));
    assert_eq!(s.follow(&p, &Action::LastViewed, Some(3)), LinkJump::Show(ShowAction::Slide(3)));
    assert_eq!(s.slide, 3);
    assert_eq!(s.follow(&p, &Action::Url { url: "https://example.org".into() }, None), LinkJump::Open("https://example.org".into()));
    for bad in
        ["file:///etc/passwd", "javascript:alert(1)", "ms-settings:", "smb://host/share", "C:\\Windows\\calc.exe", "../notes.txt", "https:", ""]
    {
        assert_eq!(s.follow(&p, &Action::Url { url: bad.into() }, None), LinkJump::None, "{bad} must not open");
    }
    assert_eq!(s.follow(&p, &Action::Url { url: " MAILTO:a@example.org".into() }, None), LinkJump::Open("MAILTO:a@example.org".into()));
    assert_eq!(s.follow(&p, &Action::EndShow, None), LinkJump::Exit);
    assert_eq!(s.follow(&p, &Action::Slide { slide: SlideId(9) }, None), LinkJump::None, "a link to a deleted slide does nothing");
    assert_eq!(s.follow(&p, &Action::LastViewed, None), LinkJump::None);
    assert_eq!(s.slide, 3);
}

/// #420: late in the Honeycomb every cell has settled, so the incoming hexagons must cover the
/// whole slide — the old row count stopped at ~75% of a 16:9 slide and left the bottom black.
#[test]
fn honeycomb_cells_reach_the_bottom_before_the_slide_ends() {
    let sign = |a: [f64; 2], b: [f64; 2], c: [f64; 2]| (a[0] - c[0]) * (b[1] - c[1]) - (b[0] - c[0]) * (a[1] - c[1]);
    let inside = |p: [f64; 2], t: [[f64; 2]; 3]| {
        // The dark seams between cells are the effect: at t=0.99 they are ~0.012% of the width
        // open, so a 1% pad forgives them and still fails genuinely uncovered ground.
        let c = [(t[0][0] + t[1][0] + t[2][0]) / 3.0, (t[0][1] + t[1][1] + t[2][1]) / 3.0];
        let t = t.map(|v| [c[0] + (v[0] - c[0]) * 1.01, c[1] + (v[1] - c[1]) * 1.01]);
        let (d1, d2, d3) = (sign(p, t[0], t[1]), sign(p, t[1], t[2]), sign(p, t[2], t[0]));
        !((d1 < 0.0 || d2 < 0.0 || d3 < 0.0) && (d1 > 0.0 || d2 > 0.0 || d3 > 0.0))
    };
    for aspect in [16.0 / 9.0, 3.0 / 2.0, 4.0 / 3.0, 11.0 / 8.5, 1.0] {
        // t = 0.99: every cell is settled and the seams are nearly closed.
        let ls = transition_frame("honeycomb", "", 0.99, aspect);
        let tris: Vec<[[f64; 2]; 3]> =
            ls.iter().filter(|l| l.source == Source::New && l.alpha >= 1.0).map(|l| [l.quad[0], l.quad[1], l.quad[2]]).collect();
        assert!(!tris.is_empty());
        let mut sy = 0.0;
        while sy <= 1.0 {
            for sx in [0.0, 0.2, 0.4, 0.6, 0.8, 0.99, 1.0] {
                let p = [sx, sy];
                assert!(tris.iter().any(|t| inside(p, *t)), "honeycomb aspect {aspect} t=0.99: {sx},{sy} is uncovered (no settled cell reaches it)");
            }
            sy += 0.02;
        }
    }
}

/// #147: exits PowerPoint plays downward leave downward here too.
#[test]
fn sink_down_and_peek_out_to_bottom_move_down() {
    for (effect, option) in [("sinkDown", ""), ("peekOut", "b"), ("floatOut", "d")] {
        let mut a = anim(1, AnimClass::Exit, effect, AnimStart::OnClick);
        a.option = option.into();
        let tl = Timeline::new(&slide_with(vec![a]), W, H);
        let s = tl.state(ShapeId(1), None, 0, 0.5);
        assert!(s.offset_y > 0.0, "{effect} {option}: {s:?}");
    }
}

#[test]
fn trigger_sequences_keep_first_appearance_order_when_interleaved() {
    let mut anims = vec![];
    for (i, tr) in [7u32, 5, 7, 0, 9, 5].iter().enumerate() {
        let mut a = anim(1 + (i as u32 % 4), AnimClass::Entrance, "fade", AnimStart::OnClick);
        a.trigger = (*tr != 0).then_some(ShapeId(*tr));
        anims.push(a);
    }
    let tl = Timeline::new(&slide_with(anims), W, H);
    assert_eq!(tl.trigger_shapes(), vec![ShapeId(7), ShapeId(5), ShapeId(9)]);
    assert_eq!(tl.trigger_steps(ShapeId(7)), 2);
    assert_eq!(tl.trigger_steps(ShapeId(5)), 2);
    assert_eq!(tl.trigger_steps(ShapeId(9)), 1);
    assert_eq!(tl.steps(), 1);
}

#[test]
fn many_distinct_triggers_past_the_effect_cap_build_in_order() {
    // One sequence per animation: grouping used to rescan every animation per trigger
    // (~1.2e10 steps here), now it is one pass, and nothing past the cap is looked at.
    let n = 110_000u32;
    let anims: Vec<Animation> = (0..n)
        .map(|i| {
            let mut a = anim(1, AnimClass::Entrance, "appear", AnimStart::OnClick);
            a.trigger = Some(ShapeId(1000 + i));
            a
        })
        .collect();
    let tl = Timeline::new(&slide_with(anims), W, H);
    assert_eq!(tl.effects().len(), 100_000);
    let shapes = tl.trigger_shapes();
    assert_eq!(shapes.len(), 100_000);
    assert!(shapes.iter().enumerate().all(|(i, s)| *s == ShapeId(1000 + i as u32)));
}

/// #316: the same text in two shapes that differ otherwise (a title slide's big centred title,
/// a content slide's title) morphs word by word with the default Objects option, instead of
/// the old box cross-fading into the new one.
#[test]
fn same_text_morphs_even_by_objects() {
    let p = deckcraft_model::defaults::new_presentation(None);
    let titled = |id, x, size: f64, text: &str| {
        let mut body = TextBody::from_text(text);
        for para in &mut body.paragraphs {
            for r in &mut para.runs {
                r.props.size = Some(size);
            }
        }
        Shape { xfrm: Some(Xfrm::new(x, 100.0, 600.0, 120.0)), text: Some(body), ..shape(id, "Title 1") }
    };
    let mut a = Slide::default();
    let mut b = Slide::default();
    a.shapes = vec![titled(1, 100.0, 60.0, "TEST")];
    b.shapes = vec![titled(2, 40.0, 44.0, "TEST")];
    let f = morph_frame(&p, &a, &b, 0.5, &|_| false);
    assert_eq!(f.text.len(), 1, "the shared text morphs");
    assert!(!f.text[0].chars);
    // Different texts still cross-fade with the Objects option.
    b.shapes = vec![titled(2, 40.0, 44.0, "Other")];
    assert!(morph_frame(&p, &a, &b, 0.5, &|_| false).text.is_empty());
}

#[test]
fn media_in_the_click_sequence_cues_and_does_not_hold_the_click() {
    // #116: a video set to play in the click sequence starts with its step, and the step doesn't
    // last as long as the video (the next click goes on while it plays).
    let play = Animation { duration_ms: 5000, ..anim(1, AnimClass::Media, "play", AnimStart::OnClick) };
    let tl = Timeline::new(&slide_with(vec![play.clone(), anim(2, AnimClass::Exit, "fadeOut", AnimStart::OnClick)]), W, H);
    assert_eq!(tl.steps(), 2);
    assert_eq!(tl.step_duration(0), 0.0);
    let cues = tl.media_cues(0, f64::NEG_INFINITY, 0.0);
    assert_eq!(cues.iter().map(|e| (e.shape, e.anim)).collect::<Vec<_>>(), [(ShapeId(1), 0)]);
    assert!(tl.media_cues(0, 0.0, f64::INFINITY).is_empty(), "set off once");
    assert!(tl.media_cues(1, f64::NEG_INFINITY, f64::INFINITY).is_empty(), "only media effects");
    // An After Previous effect still waits for the video.
    let tl = Timeline::new(&slide_with(vec![play, anim(2, AnimClass::Entrance, "fade", AnimStart::AfterPrevious)]), W, H);
    assert!((tl.step_duration(0) - 6.0).abs() < 1e-9, "{}", tl.step_duration(0));
}

/// Zoom with Return to Zoom: after the zoomed slides the show goes back to the zoom slide, fully
/// built; without it, or after jumping elsewhere, the show carries on in order.
#[test]
fn show_returns_to_the_zoom_slide() {
    use deckcraft_model::text::{Action, Hyperlink};
    let p = pres(6);
    let link = |to: u32, last: Option<u32>| Hyperlink {
        action: Action::Slide { slide: SlideId(1000 + to) },
        tooltip: String::new(),
        highlight_click: false,
        zoom_return: last.map(|l| SlideId(1000 + l)),
        sound: None,
        stop_sound: false,
    };
    let mut s = ShowState::new(&p);
    // A section zoom on slide 0 for slides 2–3.
    assert_eq!(s.follow_link(&p, &link(2, Some(3)), None), LinkJump::Show(ShowAction::Slide(2)));
    assert_eq!(s.next(&p), ShowAction::Slide(3));
    assert_eq!(s.next(&p), ShowAction::Slide(0), "back to the zoom");
    assert_eq!(s.step, ShowState::steps(&p, 0));
    assert_eq!(s.next(&p), ShowAction::Slide(1), "then on in order");
    // Without Return to Zoom the show carries on.
    let mut s = ShowState::new(&p);
    s.follow_link(&p, &link(4, None), None);
    assert_eq!(s.next(&p), ShowAction::Slide(5));
    // Jumping elsewhere (See All Slides, a number) forgets the zoom.
    let mut s = ShowState::new(&p);
    s.follow_link(&p, &link(2, Some(2)), None);
    s.goto(&p, 4);
    assert_eq!(s.next(&p), ShowAction::Slide(5));
}

#[test]
fn custom_show_resolves_duplicate_ids_to_the_first_slide() {
    let mut p = pres(3);
    Arc::make_mut(&mut p.slides[2]).id = p.slides[0].id;
    p.custom_shows = vec![CustomShow { name: "Dup".into(), slides: vec![p.slides[0].id, SlideId(1001), SlideId(424_242)] }];
    p.show.custom_show = Some("Dup".into());
    assert_eq!(show_order(&p), vec![0, 1]);
}

#[test]
fn custom_show_with_many_entries_resolves_quickly() {
    let mut p = pres(20_000);
    let ids: Vec<SlideId> = p.slides.iter().map(|s| s.id).collect();
    p.custom_shows = vec![CustomShow { name: "All".into(), slides: ids.iter().rev().copied().collect() }];
    p.show.custom_show = Some("All".into());
    let t = std::time::Instant::now();
    let order = show_order(&p);
    assert_eq!(order.len(), 20_000);
    assert_eq!(order[0], 19_999);
    assert!(t.elapsed().as_secs() < 5);
}

#[test]
fn timeline_targets_the_first_shape_with_a_duplicate_id() {
    let mut s = slide_with(vec![anim(7, AnimClass::Entrance, "fade", AnimStart::OnClick)]);
    s.shapes[0] = shape(7, "first");
    s.shapes[1] = shape(7, "second");
    s.shapes[0].xfrm = Some(Xfrm::new(10.0, 10.0, 20.0, 20.0));
    let tl = Timeline::new(&s, W, H);
    assert_eq!(tl.effects().len(), 1);
    assert_eq!(tl.steps(), 1);
}

#[test]
fn many_by_paragraph_animations_on_one_shape_share_paragraph_targets() {
    let mut s = slide_with(vec![]);
    s.shapes[0].text = Some(TextBody::from_text("one\ntwo\nthree"));
    s.animations = (0..2_000)
        .map(|_| {
            let mut a = anim(1, AnimClass::Path, "customPath", AnimStart::OnClick);
            a.text_build = TextBuild::ByParagraph;
            a.path = Some("M 0 0 L 0.5 0.5 E".into());
            a
        })
        .collect();
    let tl = Timeline::new(&s, W, H);
    assert_eq!(tl.effects().len(), 6_000);
    assert_eq!(tl.paragraph_targets(ShapeId(1)), vec![0, 1, 2]);
}

#[test]
fn morph_pairs_take_the_first_unused_match_in_slide_order() {
    let mut a = Slide::default();
    let mut b = Slide::default();
    a.shapes = vec![shape(1, "Same"), shape(2, "Same"), shape(3, "Other")];
    b.shapes = vec![shape(11, "Same"), shape(12, "Same"), shape(13, "Same")];
    // By name first, in slide order; the shape of `a` left over pairs by kind and geometry with
    // the shape of `b` left over.
    assert_eq!(morph_pairs(&a, &b), [(ShapeId(1), ShapeId(11)), (ShapeId(2), ShapeId(12)), (ShapeId(3), ShapeId(13))]);
}

#[test]
fn morph_of_large_slides_stays_linear() {
    let n = 20_000u32;
    let text = |i: u32, side: &str| Some(TextBody::from_text(&format!("{side} {i}")));
    let mut a = Slide::default();
    let mut b = Slide::default();
    // Text no shape on the other slide shares: every pass looks for a partner and finds none.
    a.shapes = (0..n).map(|i| Shape { text: text(i, "a"), ..shape(i + 1, "") }).collect();
    b.shapes = (0..n).map(|i| Shape { text: text(i, "b"), ..shape(n + i + 1, "") }).collect();
    // One named pair at opposite ends.
    a.shapes[0].name = "Logo".into();
    b.shapes[n as usize - 1].name = "Logo".into();
    // No wall-clock bound (it would flake on a loaded runner): at this size a quadratic pairing
    // takes minutes, so a regression shows as a test that hangs rather than a failed assertion.
    assert_eq!(morph_pairs(&a, &b), [(ShapeId(1), ShapeId(2 * n))]);
    let p = deckcraft_model::defaults::new_presentation(None);
    let f = morph_frame(&p, &a, &b, 0.5, &|_| false);
    // Every shape fades in place; the named pair differs in text, so it cross-fades as two.
    assert_eq!(f.shapes.len(), 2 * n as usize);
}

#[test]
fn paragraph_states_of_a_large_build_stay_linear() {
    let n = 20_000usize;
    let mut s = slide_with(vec![]);
    let mut body = TextBody::from_text(&vec!["line"; n].join("\n"));
    // One first-level paragraph owning every other paragraph.
    for p in body.paragraphs.iter_mut().skip(1) {
        p.level = 1;
    }
    s.shapes[0].text = Some(body);
    let mut a = anim(1, AnimClass::Entrance, "fade", AnimStart::OnClick);
    a.text_build = TextBuild::ByParagraph;
    s.animations = vec![a];
    let tl = Timeline::new(&s, W, H);
    // No wall-clock bound, as above: a per-paragraph scan of the slide would take minutes here.
    assert_eq!(tl.paragraph_targets(ShapeId(1)).len(), n);
    for p in 0..n {
        assert!(!tl.state(ShapeId(1), Some(p), 0, 0.0).visible);
        assert!(tl.state(ShapeId(1), Some(p), 1, 0.0).visible);
    }
    assert_eq!(tl.animated_shapes(), [ShapeId(1)]);
}
