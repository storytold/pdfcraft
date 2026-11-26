//! Transition curves and geometry against values measured on PowerPoint's video export
//! (`plan/powerpoint/tools/synth_transitions.ps1`, 2 s transitions, 30 fps).

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use deckcraft_model::anim::TRANSITIONS;

use crate::transition::{Layer, Source, transition_frame, transition_layers, uses_slide_parts};
use crate::transition_geom::{edge, pow_inout, q2, ss};

const A: f64 = 16.0 / 9.0;

fn near(a: f64, b: f64, tol: f64) -> bool {
    (a - b).abs() <= tol
}

fn of(ls: &[Layer], src: Source) -> Vec<&Layer> {
    ls.iter().filter(|l| l.source == src).collect()
}

fn left(l: &Layer) -> f64 {
    l.quad.iter().map(|p| p[0]).fold(f64::INFINITY, f64::min)
}

fn top(l: &Layer) -> f64 {
    l.quad.iter().map(|p| p[1]).fold(f64::INFINITY, f64::min)
}

#[test]
fn fade_follows_powerpoints_s_curve() {
    // Measured share of the new slide in a 2 s Fade at t = 0.25, 0.5, 0.75: 0.155, 0.49, 0.82.
    for (t, want) in [(0.25, 0.155), (0.5, 0.49), (0.75, 0.82)] {
        let ls = transition_layers("fade", "smoothly", t);
        let new = of(&ls, Source::New);
        assert!(near(new[0].alpha, want, 0.04), "t={t}: {}", new[0].alpha);
    }
    // Through black: the old slide is gone to black at the middle, then the new one fades in.
    let mid = transition_layers("fade", "throughBlack", 0.5);
    assert!(mid.iter().all(|l| l.source == Source::Black));
    let q = transition_layers("fade", "throughBlack", 0.25);
    assert!(near(of(&q, Source::Old)[0].alpha, 0.5, 0.01));
}

#[test]
fn push_moves_both_slides_with_an_eased_curve() {
    for (o, dx, dy) in [("l", -1.0, 0.0), ("r", 1.0, 0.0), ("u", 0.0, -1.0), ("d", 0.0, 1.0)] {
        for t in [0.25, 0.5, 0.75] {
            let ls = transition_layers("push", o, t);
            let (old, new) = (of(&ls, Source::Old)[0], of(&ls, Source::New)[0]);
            let s = q2(t);
            assert!(near(old.quad[0][0], dx * s, 1e-9) && near(old.quad[0][1], dy * s, 1e-9), "{o} {t}");
            assert!(near(new.quad[0][0], dx * (s - 1.0), 1e-9) && near(new.quad[0][1], dy * (s - 1.0), 1e-9), "{o} {t}");
        }
    }
    // Ease in-out: slow at both ends.
    assert!(q2(0.1) < 0.05 && q2(0.9) > 0.95 && near(q2(0.5), 0.5, 1e-9));
}

#[test]
fn cover_and_uncover_match_measured_positions() {
    // Offset of the new slide in Cover Left (fraction of the width) at measured times.
    for (t, off) in [(0.445, 0.655), (0.495, 0.512), (0.544, 0.359), (0.643, 0.134), (0.742, 0.046)] {
        let ls = transition_layers("cover", "l", t);
        assert!(near(left(of(&ls, Source::New)[0]), off, 0.05), "cover t={t}");
    }
    // Offset of the old slide in Uncover Left.
    for (t, off) in [(0.244, -0.06), (0.343, -0.14), (0.442, -0.32), (0.508, -0.5), (0.558, -0.63)] {
        let ls = transition_layers("uncover", "l", t);
        assert!(near(left(of(&ls, Source::Old)[0]), off, 0.05), "uncover t={t}");
    }
    // Diagonal Cover shoots in and settles slowly.
    for (t, off) in [(0.142, 0.728), (0.257, 0.49), (0.356, 0.29), (0.455, 0.146), (0.604, 0.053)] {
        let ls = transition_layers("cover", "lu", t);
        let n = of(&ls, Source::New)[0];
        assert!(near(left(n), off, 0.06) && near(top(n), off, 0.06), "cover lu t={t}");
    }
    assert!(near(pow_inout(0.25, 3.0), 0.0625, 1e-9));
}

#[test]
fn wipe_has_a_wide_soft_edge() {
    // Measured along a row of Wipe Right at t = 0.5: about half-way through at x = 0.6, nearly
    // nothing at the right edge, fully new on the left third.
    let c = -0.4 + 1.8 * 1.05 * 0.5;
    assert!(near(edge(c, 0.6, 0.8), 0.49, 0.1));
    assert!(edge(c, 1.0, 0.8) < 0.1);
    assert!(edge(c, 0.2, 0.8) > 0.8);
    // Directions: "r" starts on the left, "l" on the right, "u" at the bottom, "d" at the top,
    // diagonals at the opposite corner.
    let first = |o: &str| {
        let ls = transition_layers("wipe", o, 0.15);
        let n = of(&ls, Source::New);
        assert!(!n.is_empty(), "{o}");
        let cx = n.iter().map(|l| (l.quad[0][0] + l.quad[2][0]) / 2.0).sum::<f64>() / n.len() as f64;
        let cy = n.iter().map(|l| (l.quad[0][1] + l.quad[2][1]) / 2.0).sum::<f64>() / n.len() as f64;
        (cx, cy)
    };
    assert!(first("r").0 < 0.3 && first("l").0 > 0.7);
    assert!(first("u").1 > 0.7 && first("d").1 < 0.3);
    let ru = first("ru");
    assert!(ru.0 < 0.4 && ru.1 > 0.6, "{ru:?}");
}

#[test]
fn split_and_shape_open_from_the_middle() {
    for o in ["horzOut", "vertOut"] {
        let ls = transition_layers("split", o, 0.2);
        let n = of(&ls, Source::New);
        assert!(n.iter().all(|l| {
            let c = [(l.quad[0][0] + l.quad[2][0]) / 2.0, (l.quad[0][1] + l.quad[2][1]) / 2.0];
            if o.starts_with("horz") { (c[1] - 0.5).abs() < 0.4 } else { (c[0] - 0.5).abs() < 0.4 }
        }));
    }
    // Shape In is done with the old slide at half time; Shape Out starts the new one then.
    assert!(of(&transition_layers("shape", "in", 0.55), Source::Old).is_empty());
    assert!(of(&transition_layers("shape", "out", 0.45), Source::New).is_empty());
}

#[test]
fn dynamic_content_curves() {
    // Pan: the new content's offset (fraction of the width) at measured times.
    for (t, off) in [(0.6, 0.134), (0.7, 0.086), (0.8, 0.039), (0.9, 0.011)] {
        let ls = transition_layers("pan", "l", t);
        assert!(near(left(of(&ls, Source::NewContent)[0]), off, 0.07), "pan t={t}");
    }
    // The backgrounds cross-fade under the moving contents.
    let ls = transition_layers("pan", "l", 0.5);
    assert_eq!(ls.first().map(|l| l.source), Some(Source::OldBack));
    assert!(of(&ls, Source::NewBack)[0].alpha > 0.5);
    // Ferris Wheel: the old content drops (top edge, slide heights) as measured.
    for (t, dy) in [(0.147, 0.06), (0.246, 0.17), (0.345, 0.36), (0.444, 0.70)] {
        let ls = transition_layers("ferris", "l", t);
        let q = of(&ls, Source::OldContent)[0].quad;
        let cy = q.iter().map(|p| p[1]).sum::<f64>() / 4.0 - 0.5;
        assert!(near(cy, dy, 0.08), "ferris t={t}: {cy}");
    }
    for k in ["pan", "ferris", "conveyor", "rotate", "window", "orbit", "flythrough"] {
        assert!(uses_slide_parts(k, ""), "{k}");
    }
    assert!(uses_slide_parts("reveal", "smoothlyLeft") && !uses_slide_parts("reveal", "blackLeft"));
    assert!(!uses_slide_parts("cube", "l") && !uses_slide_parts("push", "l"));
}

#[test]
fn cube_turns_toward_the_option() {
    // Cube Left at the middle: the shared edge is near the middle, the old face on the left.
    let ls = transition_frame("cube", "l", 0.5, A);
    let (o, n) = (of(&ls, Source::Old)[0], of(&ls, Source::New)[0]);
    let o_right = o.quad.iter().map(|p| p[0]).fold(f64::NEG_INFINITY, f64::max);
    assert!(near(o_right, left(n), 0.02), "shared edge: {o_right} vs {}", left(n));
    assert!(o_right > 0.3 && o_right < 0.7);
    // Up: the new face comes from below.
    let ls = transition_frame("cube", "u", 0.5, A);
    assert!(top(of(&ls, Source::New)[0]) > top(of(&ls, Source::Old)[0]));
}

#[test]
fn switch_swaps_the_cards_past_halfway() {
    let order = |t: f64| {
        let ls = transition_frame("switch", "l", t, A);
        let pos = |s: Source| ls.iter().position(|l| l.source == s);
        (pos(Source::Old), pos(Source::New))
    };
    let (o, n) = order(0.3);
    assert!(o > n, "old in front early");
    let (o, n) = order(0.8);
    assert!(n > o, "new in front late");
}

#[test]
fn clock_hand_sweeps_at_constant_speed() {
    // The share of the slide covered grows linearly.
    let area = |t: f64| {
        let ls = transition_layers("clock", "clockwise", t);
        of(&ls, Source::New)
            .iter()
            .map(|l| {
                let [a, b, c, _] = l.quad;
                ((b[0] - a[0]) * (c[1] - a[1]) - (c[0] - a[0]) * (b[1] - a[1])).abs() / 2.0 * (l.fade[0] + l.fade[1] + l.fade[2]) / 3.0
            })
            .sum::<f64>()
    };
    let (a1, a2, a3) = (area(0.25), area(0.5), area(0.75));
    assert!(near(a2 - a1, a3 - a2, 0.08), "{a1} {a2} {a3}");
    assert!(near(a2, 0.5, 0.08), "{a2}");
}

#[test]
fn options_change_the_frame() {
    // Every option of a transition draws something different from its other options midway.
    for (id, _, _, _, opts) in TRANSITIONS {
        if matches!(*id, "morph" | "none" | "random") || opts.len() < 2 {
            continue;
        }
        for (i, a) in opts.iter().enumerate() {
            for b in opts.iter().skip(i + 1) {
                let differs = [0.2, 0.35, 0.5, 0.8].iter().any(|t| transition_layers(id, a, *t) != transition_layers(id, b, *t));
                assert!(differs, "{id}: {a} vs {b}");
            }
        }
    }
}

#[test]
fn frames_stay_cheap() {
    // Per frame, the show draws these meshes from cached slide textures: keep them small.
    for (id, _, _, _, opts) in TRANSITIONS {
        let mut options: Vec<&str> = opts.to_vec();
        options.push("");
        for o in options {
            for t in [0.1, 0.3, 0.5, 0.7, 0.9] {
                let tris: usize = transition_frame(id, o, t, A).iter().map(|l| crate::layer_mesh(l).1.len()).sum();
                assert!(tris < 30_000, "{id}/{o} t={t}: {tris} triangles");
            }
        }
    }
}

#[test]
fn hostile_aspects_are_safe() {
    for a in [f64::NAN, 0.0, -3.0, 1e9, f64::INFINITY] {
        for (id, ..) in TRANSITIONS {
            for l in transition_frame(id, "", 0.4, a) {
                assert!(l.quad.iter().all(|p| p[0].is_finite() && p[1].is_finite()));
                assert!(l.fade.iter().all(|f| (0.0..=1.0).contains(f)));
            }
        }
    }
    assert!(near(ss(0.5), 0.5, 1e-12));
}
