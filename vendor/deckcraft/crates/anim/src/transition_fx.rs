//! The transition effects, as measured on PowerPoint (see [`crate::transition`]).
//!
//! `t` is linear progress (0..1) through the transition's duration; each effect applies its own
//! curve. Directions follow the file format's option: for Push, Wipe, Cover, Uncover, Pan, Vortex,
//! Cube… `dir` is where things move (`l`: toward the left); for Glitter it is the side the effect
//! starts from; Ripple's corner is where the wave heads.

use std::f64::consts::PI;

use crate::easing::ease;
use crate::transition::{FULL, Layer, R, Source};
use crate::transition_geom::{
    Cam, V3, boundary, card, center, clip, dir, edge, ein, eout, facing_shade, fan, field, hash64, moved, offset, poly_layers, pow_inout, q2,
    rect_poly, rnd, rot, rot_quad, scale_rect, scaled, soft_rect, ss, v3, win,
};

use Source::{Black, New, NewBack, NewContent, Old, OldBack, OldContent, White};

const RANDOM_KINDS: &[&str] = &[
    "fade",
    "push",
    "wipe",
    "split",
    "reveal",
    "randomBar",
    "shape",
    "uncover",
    "cover",
    "fallOver",
    "drape",
    "curtains",
    "wind",
    "prestige",
    "fracture",
    "crush",
    "peelOff",
    "pageCurlDouble",
    "airplane",
    "origami",
    "dissolve",
    "checker",
    "blinds",
    "clock",
    "ripple",
    "honeycomb",
    "glitter",
    "vortex",
    "shred",
    "switch",
    "flip",
    "gallery",
    "cube",
    "doors",
    "box",
    "comb",
    "zoom",
    "pan",
    "ferris",
    "conveyor",
    "rotate",
    "window",
    "orbit",
    "flythrough",
];

pub(crate) fn layers(kind: &str, o: &str, t: f64, a: f64, depth: u32) -> Vec<Layer> {
    match kind {
        "none" => vec![Layer::full(New, 1.0)],
        "cut" => {
            if o == "throughBlack" && t < 0.5 {
                vec![Layer::full(Black, 1.0)]
            } else {
                vec![Layer::full(New, 1.0)]
            }
        }
        "fade" if o == "throughBlack" => through_black(t, 0.5, 0.5),
        "fade" | "morph" => vec![Layer::full(Old, 1.0), Layer::full(New, ss(t))],
        "push" => {
            let (dx, dy) = dir(o, (-1.0, 0.0));
            let s = q2(t);
            vec![moved(Old, dx * s, dy * s, 1.0), moved(New, dx * (s - 1.0), dy * (s - 1.0), 1.0)]
        }
        "uncover" => {
            let (dx, dy) = dir(o, (-1.0, 0.0));
            // Diagonal: the old slide accelerates off in the first third.
            let s = if o.len() == 2 { ein(win(t, 0.05, 0.35)) } else { pow_inout(t, 3.0) };
            vec![Layer::full(New, 1.0), moved(Old, dx * s, dy * s, 1.0)]
        }
        "cover" => {
            let (dx, dy) = dir(o, (-1.0, 0.0));
            // Diagonal: the new slide shoots in and settles slowly.
            let s = if o.len() == 2 { 1.0 - (1.0 - win(t, 0.07, 0.95)).powi(3) } else { pow_inout(t, 4.0) };
            vec![Layer::full(Old, 1.0), moved(New, dx * (s - 1.0), dy * (s - 1.0), 1.0)]
        }
        "wipe" => wipe(o, t),
        "split" => split(o, t),
        "reveal" => reveal(o, t),
        "randomBar" => random_bars(o == "vert", t),
        "shape" => shape(o, t, a),
        "flash" => flash(t),
        "dissolve" => dissolve(t),
        "checker" => checker(o == "vert", t, a),
        "blinds" => blinds(o == "vert", t, a),
        "clock" => clock(o, t, a),
        "ripple" => ripple(o, t, a),
        "comb" => comb(o == "vert", t),
        "zoom" => zoom(o, t, a),
        "pan" => pan(o, t),
        "fallOver" => fall_over(o == "r", t, a),
        "drape" => drape(o == "r", t, a),
        "curtains" => curtains(t, a),
        "wind" => wind(o == "r", t, a),
        "prestige" => prestige(t, a),
        "fracture" => fracture(t, a),
        "crush" => crush(t, a),
        "peelOff" => peel_off(o == "r", t),
        "pageCurlDouble" => page_curl(o, t),
        "airplane" => airplane(o == "r", t, a),
        "origami" => origami(o == "r", t, a),
        "honeycomb" => honeycomb(t, a),
        "glitter" => glitter(o, t, a),
        "vortex" => vortex(o, t, a),
        "shred" => shred(o, t, a),
        "switch" => switch(o == "r", t, a),
        "flip" => flip(o == "r", t, a),
        "gallery" => gallery(o == "r", t, a),
        "conveyor" => conveyor(o == "r", t, a),
        "cube" | "box" | "rotate" | "orbit" => prism(kind, o, t, a),
        "doors" | "window" => doors(kind == "window", o == "horz", t, a),
        "ferris" => ferris(o == "r", t, a),
        "flythrough" => flythrough(o, t),
        "random" if depth == 0 => {
            let h = o.bytes().fold(0x51ED_u64, |h, b| hash64(h ^ b as u64));
            let k = RANDOM_KINDS.get((h % RANDOM_KINDS.len() as u64) as usize).copied().unwrap_or("fade");
            let opt = deckcraft_model::anim::TRANSITIONS.iter().find(|x| x.0 == k).and_then(|x| x.4.first().copied()).unwrap_or("");
            layers(k, opt, t, a, depth + 1)
        }
        _ => vec![Layer::full(Old, 1.0), Layer::full(New, ss(t))],
    }
}

/// Old fades to black over [0, `out`], new fades in from black over [`inn`, 1] (smoothstep).
fn through_black(t: f64, out: f64, inn: f64) -> Vec<Layer> {
    let mut v = vec![Layer::full(Black, 1.0)];
    if t < out {
        v.push(Layer::full(Old, 1.0 - ss(win(t, 0.0, out))));
    }
    if t > inn {
        v.push(Layer::full(New, ss(win(t, inn, 1.0))));
    }
    v
}

// ---------- Subtle ----------

/// Width of the soft edge of Wipe and Split (slide units along the motion).
const WIPE_SOFT: f64 = 0.8;

/// Where the middle of the soft edge is at `t`: from fully before the slide to fully past it.
fn wipe_pos(t: f64) -> f64 {
    -WIPE_SOFT / 2.0 + (1.0 + WIPE_SOFT) * 1.05 * t
}

fn wipe(o: &str, t: f64) -> Vec<Layer> {
    let c = wipe_pos(t);
    // Position along the motion, 0 where the new slide starts showing.
    let p: Box<dyn Fn(f64, f64) -> f64> = match o {
        "l" => Box::new(|x, _| 1.0 - x),
        "u" => Box::new(|_, y| 1.0 - y),
        "d" => Box::new(|_, y| y),
        "lu" => Box::new(|x, y| (2.0 - x - y) / 2.0),
        "ru" => Box::new(|x, y| (1.0 + x - y) / 2.0),
        "ld" => Box::new(|x, y| (1.0 - x + y) / 2.0),
        "rd" => Box::new(|x, y| (x + y) / 2.0),
        _ => Box::new(|x, _| x),
    };
    let diag = o.len() == 2;
    let (n, m) = if diag {
        (32, 18)
    } else if matches!(o, "u" | "d") {
        (1, 36)
    } else {
        (48, 1)
    };
    let mut v = vec![Layer::full(Old, 1.0)];
    v.extend(field(New, n, m, &|x, y| edge(c, p(x, y), WIPE_SOFT)));
    v
}

fn split(o: &str, t: f64) -> Vec<Layer> {
    let horz = o.starts_with("horz");
    let inward = o.ends_with("In");
    let c = wipe_pos(t);
    let p = move |x: f64, y: f64| {
        let d = if horz { (y - 0.5).abs() * 2.0 } else { (x - 0.5).abs() * 2.0 };
        if inward { 1.0 - d } else { d }
    };
    let (n, m) = if horz { (1, 36) } else { (48, 1) };
    let mut v = vec![Layer::full(Old, 1.0)];
    v.extend(field(New, n, m, &|x, y| edge(c, p(x, y), WIPE_SOFT)));
    v
}

fn reveal(o: &str, t: f64) -> Vec<Layer> {
    let dx = if o.ends_with("Right") { 1.0 } else { -1.0 };
    if o.starts_with("black") {
        // Old slides away fading to black, a pause on black, new slides in from black.
        let mut v = vec![Layer::full(Black, 1.0)];
        if t < 0.4 {
            let u = win(t, 0.0, 0.38);
            v.push(moved(Old, dx * 0.12 * ein(u), 0.0, 1.0 - ss(u)));
        }
        if t > 0.55 {
            let u = win(t, 0.55, 1.0);
            v.push(moved(New, -dx * 0.12 * (1.0 - eout(u)), 0.0, ss(u)));
        }
        return v;
    }
    // Smoothly: the backgrounds cross-fade while the old content slides off fading and the new
    // content follows.
    let mut v = vec![Layer::full(OldBack, 1.0), Layer::full(NewBack, ss(t))];
    if t < 0.5 {
        let u = win(t, 0.0, 0.45);
        v.push(moved(OldContent, dx * 0.12 * ein(u), 0.0, 1.0 - ss(u)));
    }
    if t > 0.45 {
        let u = win(t, 0.47, 0.95);
        v.push(moved(NewContent, -dx * 0.12 * (1.0 - eout(u)), 0.0, ss(u)));
    }
    v
}

fn random_bars(vert: bool, t: f64) -> Vec<Layer> {
    // Thin bars, each cross-fading quickly at its own random moment.
    let n = 160usize;
    let w = 0.2;
    let mut v = vec![Layer::full(Old, 1.0)];
    for i in 0..n {
        let start = rnd(i as u64 * 7 + 17) * (1.0 - w);
        let k = win(t, start, start + w);
        if k <= 0.0 {
            continue;
        }
        let (a, b) = (i as f64 / n as f64, (i + 1) as f64 / n as f64);
        let r: R = if vert { [a, 0.0, b, 1.0] } else { [0.0, a, 1.0, b] };
        v.push(Layer::inplace(New, r, k));
    }
    v
}

fn shape(o: &str, t: f64, a: f64) -> Vec<Layer> {
    match o {
        // Shape In: the old slide shrinks into the centre over the first half.
        "in" => {
            let s = 1.0 - win(t, 0.0, 0.5);
            let mut v = vec![Layer::full(New, 1.0)];
            if s > 0.0 {
                v.extend(soft_rect(Old, FULL, scale_rect(FULL, s), 0.012, 1.0));
            }
            v
        }
        // Shape Out: the new slide grows out of the centre over the second half.
        "out" => {
            let s = win(t, 0.5, 1.0);
            let mut v = vec![Layer::full(Old, 1.0)];
            if s > 0.0 {
                v.extend(soft_rect(New, FULL, scale_rect(FULL, s), 0.012, 1.0));
            }
            v
        }
        _ => {
            let soft = 0.18;
            let c = -soft / 2.0 + (1.0 + soft) * t;
            let f: Box<dyn Fn(f64, f64) -> f64> = match o {
                "diamond" => Box::new(move |x, y| edge(c, (x - 0.5).abs() + (y - 0.5).abs(), soft)),
                "plus" => Box::new(move |x, y| edge(c, (x - 0.5).abs().min((y - 0.5).abs()) * 2.0, soft)),
                _ => {
                    let rmax = ((0.5 * a).powi(2) + 0.25).sqrt();
                    Box::new(move |x, y| edge(c, (((x - 0.5) * a).powi(2) + (y - 0.5).powi(2)).sqrt() / rmax, soft))
                }
            };
            let mut v = vec![Layer::full(Old, 1.0)];
            v.extend(field(New, 48, 27, &*f));
            v
        }
    }
}

fn flash(t: f64) -> Vec<Layer> {
    // A flash bulb: the old slide burns out to white fast, the white holds, then fades off the new
    // slide.
    let white = if t < 0.1 {
        t / 0.1
    } else if t < 0.47 {
        1.0
    } else {
        1.0 - win(t, 0.47, 0.88)
    };
    let base = if t < 0.2 { Old } else { New };
    vec![Layer::full(base, 1.0), Layer::full(White, white)]
}

// ---------- Exciting: patterns ----------

fn dissolve(t: f64) -> Vec<Layer> {
    let (n, m) = (48usize, 27usize);
    let mut v = vec![Layer::full(Old, 1.0)];
    let cell = |c: usize, r: usize| [c as f64 / n as f64, r as f64 / m as f64, (c + 1) as f64 / n as f64, (r + 1) as f64 / m as f64];
    for r in 0..m {
        for c in 0..n {
            if t > rnd((r * n + c) as u64 + 991) * 0.97 {
                v.push(Layer::inplace(New, cell(c, r), 1.0));
            }
        }
    }
    v
}

/// One tile flipping in 3-D about its own vertical (or horizontal) axis: old face for the first
/// half of `k`, new face for the second.
fn flip_tile(cam: &Cam, cell: R, k: f64, vertical_axis: bool, out: &mut Vec<Layer>) {
    if k <= 0.0 {
        out.push(Layer::inplace(Old, cell, 1.0));
        return;
    }
    if k >= 1.0 {
        out.push(Layer::inplace(New, cell, 1.0));
        return;
    }
    let ang = 180.0 * k;
    let cc = center(cell);
    let c3 = cam.unit(cc[0], cc[1]);
    let (src, deg) = if k < 0.5 { (Old, ang) } else { (New, ang - 180.0) };
    let pts = cam.rect(cell).map(|p| if vertical_axis { p.yaw(c3, deg) } else { p.pitch(c3, deg) });
    let sh = facing_shade(pts, 0.6);
    out.extend(card(cam, src, cell, pts, 1.0, sh, true));
}

fn checker(vert: bool, t: f64, a: f64) -> Vec<Layer> {
    let (n, m) = (10usize, 8usize);
    let cam = Cam { a, d: 3.0 };
    let mut v = vec![Layer::full(Black, 1.0)];
    for r in 0..m {
        for c in 0..n {
            let cell = [c as f64 / n as f64, r as f64 / m as f64, (c + 1) as f64 / n as f64, (r + 1) as f64 / m as f64];
            let across = if vert { r as f64 / m as f64 } else { c as f64 / n as f64 };
            let start = across * 0.55 + rnd((r * n + c) as u64 + 5) * 0.12;
            let k = q2(win(t, start, start + 0.3));
            flip_tile(&cam, cell, k, !vert, &mut v);
        }
    }
    v
}

fn blinds(vert: bool, t: f64, a: f64) -> Vec<Layer> {
    let n = 12usize;
    let cam = Cam { a, d: 3.0 };
    let mut v = vec![Layer::full(Black, 1.0)];
    for i in 0..n {
        let (s, e) = (i as f64 / n as f64, (i + 1) as f64 / n as f64);
        let cell = if vert { [s, 0.0, e, 1.0] } else { [0.0, s, 1.0, e] };
        let start = 0.1 + 0.3 * i as f64 / n as f64;
        let k = q2(win(t, start, start + 0.4));
        flip_tile(&cam, cell, k, vert, &mut v);
    }
    v
}

fn clock(o: &str, t: f64, a: f64) -> Vec<Layer> {
    // The hand sweeps at a constant speed with a soft leading edge.
    let soft = 0.12;
    let c = -soft / 2.0 + (1.0 + soft) * t;
    let f = move |ang: f64| -> f64 {
        let p = match o {
            "counterClockwise" => (2.0 * PI - ang) / (2.0 * PI),
            "wedge" => {
                let d = if ang > PI { 2.0 * PI - ang } else { ang };
                d / PI
            }
            _ => ang / (2.0 * PI),
        };
        edge(c, p, soft)
    };
    let mut v = vec![Layer::full(Old, 1.0)];
    v.extend(fan(New, a, 240, &f));
    v
}

fn ripple(o: &str, t: f64, a: f64) -> Vec<Layer> {
    // A soft disc of the new slide spreads from the centre, or from the corner opposite to where
    // the wave heads.
    let origin = match o {
        "lu" => [1.0, 1.0],
        "ru" => [0.0, 1.0],
        "ld" => [1.0, 0.0],
        "rd" => [0.0, 0.0],
        _ => [0.5, 0.5],
    };
    let far = [[0.0, 0.0], [1.0, 0.0], [0.0, 1.0], [1.0, 1.0]]
        .iter()
        .map(|p: &[f64; 2]| (((p[0] - origin[0]) * a).powi(2) + (p[1] - origin[1]).powi(2)).sqrt())
        .fold(0.0, f64::max);
    let soft = 0.16;
    let c = -soft / 2.0 + (1.0 + soft) * t;
    let mut v = vec![Layer::full(Old, 1.0)];
    let f = |x: f64, y: f64| {
        let d = (((x - origin[0]) * a).powi(2) + (y - origin[1]).powi(2)).sqrt() / far.max(1e-6);
        edge(c, d, soft)
    };
    v.extend(field(New, 48, 27, &f));
    // Rings of shade travelling inside the wave.
    let rings = |x: f64, y: f64| {
        let d = (((x - origin[0]) * a).powi(2) + (y - origin[1]).powi(2)).sqrt() / far.max(1e-6);
        let inside = edge(c, d, soft);
        let w = ((d - c) * 30.0).sin().abs();
        0.18 * inside * (1.0 - w) * (1.0 - t)
    };
    v.extend(field(Black, 48, 27, &rings));
    v
}

fn comb(vert: bool, t: f64) -> Vec<Layer> {
    // Bands of the old slide slide off to alternate sides, one after another from the last band,
    // uncovering the new slide.
    let n = if vert { 10usize } else { 7usize };
    let mut v = vec![Layer::full(New, 1.0)];
    for i in 0..n {
        let (a, b) = (i as f64 / n as f64, (i + 1) as f64 / n as f64);
        let d = if (n - 1 - i) % 2 == 0 { 1.0 } else { -1.0 };
        let start = 0.06 + 0.05 * (n - 1 - i) as f64 * 7.0 / n as f64;
        let s = ein(win(t, start, start + 0.55));
        if s >= 1.0 {
            continue;
        }
        let band = if vert { [a, 0.0, b, 1.0] } else { [0.0, a, 1.0, b] };
        let (ox, oy) = if vert { (0.0, d * s) } else { (d * s, 0.0) };
        v.push(Layer::rect(Old, band, offset(band, ox, oy), 1.0));
    }
    v
}

fn zoom(o: &str, t: f64, a: f64) -> Vec<Layer> {
    match o {
        // Zoom and Rotate: the new slide spins in from the centre, a full clockwise turn, fading
        // in as it grows.
        "rotate" => {
            let u = win(t, 0.1, 0.98);
            let s = 0.1 + 0.9 * u;
            let ang = 360.0 * (1.0 - u);
            let r = scale_rect(FULL, s);
            let q = rot_quad(crate::transition::corners(r), [0.5, 0.5], ang, a);
            vec![Layer::full(Old, 1.0), Layer::quad(New, FULL, q, 0.35 + 0.65 * ss(win(t, 0.0, 0.45)))]
        }
        // Zoom Out: the new slide comes down from large underneath while the old one shrinks away
        // fading.
        "out" => {
            let s = eout(win(t, 0.0, 0.8));
            vec![
                scaled(New, 1.9 - 0.9 * s, 0.5, 0.5, 0.0, 0.0, 1.0),
                scaled(Old, 1.0 - 0.5 * eout(t), 0.5, 0.5, 0.0, 0.0, 1.0 - ss(win(t, 0.0, 0.95))),
            ]
        }
        // Zoom In: the old slide grows toward the viewer fading, the new one grows into place
        // from small.
        _ => vec![
            Layer::full(Black, 1.0),
            scaled(Old, 1.0 + 0.3 * ss(t), 0.5, 0.5, 0.0, 0.0, 1.0 - ss(win(t, 0.0, 0.9))),
            scaled(New, 0.5 + 0.5 * ss(win(t, 0.15, 0.95)), 0.5, 0.5, 0.0, 0.0, ss(win(t, 0.0, 0.85))),
        ],
    }
}

fn fall_over(right: bool, t: f64, a: f64) -> Vec<Layer> {
    // The old slide tips forward about its bottom edge and falls out of view.
    let cam = Cam { a, d: 2.2 };
    let u = ein(win(t, 0.0, 0.88));
    let ang = 92.0 * u;
    let pivot = v3(0.0, 0.5, 0.0);
    let side = if right { 1.0 } else { -1.0 };
    let pts = cam.rect(FULL).map(|p| p.pitch(pivot, ang).roll(pivot, side * 6.0 * u));
    let mut v = vec![Layer::full(New, 1.0)];
    if u < 1.0 {
        v.extend(card(&cam, Old, FULL, pts, 1.0, facing_shade(pts, 0.4), true));
    }
    v
}

fn drape(right: bool, t: f64, a: f64) -> Vec<Layer> {
    // The new slide drops from its top edge like cloth over the old one.
    let cam = Cam { a, d: 2.2 };
    let u = q2(win(t, 0.25, 0.62));
    let wob = 4.0 * (1.0 - win(t, 0.6, 1.0)) * (win(t, 0.6, 1.0) * PI * 2.0).sin();
    let ang = 90.0 * (1.0 - u) + wob * u;
    let pivot = v3(0.0, -0.5, 0.0);
    let side = if right { -1.0 } else { 1.0 };
    let pts = cam.rect(FULL).map(|p| p.pitch(pivot, -ang).roll(pivot, side * 3.0 * (1.0 - u)));
    let mut v = vec![Layer::full(Old, 1.0)];
    if u > 0.0 {
        v.extend(card(&cam, New, FULL, pts, 1.0, facing_shade(pts, 0.5), true));
    }
    v
}

fn curtains(t: f64, a: f64) -> Vec<Layer> {
    // The old slide parts in the middle into two curtains drawn to the sides, then lifted away.
    let open = q2(win(t, 0.0, 0.35));
    let lift = ein(win(t, 0.62, 0.9));
    let mut v = vec![Layer::full(New, 1.0)];
    let w_top = 0.5 - 0.22 * open;
    let w_bot = 0.5 - 0.12 * open;
    let up = 0.15 * lift;
    let sw = 0.75 * lift;
    let left = [[-sw, -up], [w_top - sw, -up], [w_bot - 0.6 * sw, 1.0 - up], [-0.6 * sw, 1.0 - up]];
    let right = [[1.0 - w_top + sw, -up], [1.0 + sw, -up], [1.0 + 0.6 * sw, 1.0 - up], [1.0 - w_bot + 0.6 * sw, 1.0 - up]];
    v.push(Layer::quad(Old, [0.0, 0.0, 0.5, 1.0], left, 1.0));
    v.push(Layer::quad(Old, [0.5, 0.0, 1.0, 1.0], right, 1.0));
    let _ = a;
    v
}

fn wind(right: bool, t: f64, a: f64) -> Vec<Layer> {
    // The old slide blows away like a sheet, lifting and receding toward one side.
    let cam = Cam { a, d: 2.2 };
    let u = ein(win(t, 0.12, 0.8));
    let side = if right { 1.0 } else { -1.0 };
    let pivot = v3(0.0, 0.0, 0.0);
    let pts = cam.rect(FULL).map(|p| {
        let w = p.pitch(v3(0.0, -0.5, 0.0), -25.0 * u).yaw(pivot, -side * 20.0 * u);
        v3(w.x + side * 1.6 * u * a / 2.0, w.y - 0.8 * u, w.z + 0.5 * u)
    });
    let mut v = vec![Layer::full(New, 1.0)];
    if u < 1.0 {
        v.extend(card(&cam, Old, FULL, pts, 1.0, 0.0, true));
    }
    v
}

fn prestige(t: f64, a: f64) -> Vec<Layer> {
    // The old slide lifts at its bottom edge and flies off upward.
    let cam = Cam { a, d: 2.2 };
    let lift = q2(win(t, 0.4, 0.62));
    let fly = ein(win(t, 0.5, 0.85));
    let pivot = v3(0.0, -0.5, 0.0);
    let pts = cam.rect(FULL).map(|p| {
        let w = p.pitch(pivot, -40.0 * lift).roll(pivot, -12.0 * fly);
        v3(w.x - 0.4 * fly * a / 2.0, w.y - 1.3 * fly, w.z - 0.2 * fly)
    });
    let mut v = vec![Layer::full(New, 1.0)];
    if fly < 1.0 {
        v.extend(card(&cam, Old, FULL, pts, 1.0, facing_shade(pts, 0.3), true));
    }
    v
}

fn fracture(t: f64, a: f64) -> Vec<Layer> {
    // The old slide cracks into long shards radiating from its centre; the shards part a little,
    // then fall away, the top ones first.
    let mut v = vec![Layer::full(New, 1.0)];
    let crack = ss(win(t, 0.06, 0.2));
    if crack <= 0.0 {
        v.push(Layer::full(Old, 1.0));
        return v;
    }
    let n = 36usize;
    let rings = [0.0, 0.3, 0.6, 1.0];
    let c = [0.5, 0.5];
    for i in 0..n {
        let a0 = 2.0 * PI * (i as f64 + 0.35 * rnd(i as u64)) / n as f64;
        let a1 = 2.0 * PI * (i as f64 + 1.0 + 0.35 * rnd((i + 1) as u64 % n as u64)) / n as f64;
        for r in 0..3 {
            let (r0, r1) = (rings[r], if r == 2 { 1.6 } else { rings[r + 1] });
            let p = |ang: f64, rr: f64| -> [f64; 2] {
                let b = boundary(ang, a);
                [c[0] + (b[0] - c[0]) * rr, c[1] + (b[1] - c[1]) * rr]
            };
            let pts = [p(a0, r0), p(a1, r0), p(a1, r1), p(a0, r1)];
            let id = (i * 3 + r) as u64;
            let mid = [(pts[0][0] + pts[1][0] + pts[2][0] + pts[3][0]) / 4.0, (pts[0][1] + pts[1][1] + pts[2][1] + pts[3][1]) / 4.0];
            let start = 0.3 + 0.28 * mid[1].clamp(0.0, 1.0) + 0.1 * rnd(id + 300);
            let k = win(t, start, start + 0.35);
            let gap = 0.015 * crack;
            let out = [(mid[0] - c[0]) * gap * 4.0, (mid[1] - c[1]) * gap * 4.0 + 1.4 * k * k];
            let ang = (rnd(id + 7) - 0.5) * 50.0 * k;
            let q = pts.map(|p| {
                let p = [mid[0] + (p[0] - mid[0]) * (1.0 - 0.06 * crack) + out[0], mid[1] + (p[1] - mid[1]) * (1.0 - 0.06 * crack) + out[1]];
                rot(p, [mid[0] + out[0], mid[1] + out[1]], ang, a)
            });
            let uv = pts.map(|p| [p[0].clamp(0.0, 1.0), p[1].clamp(0.0, 1.0)]);
            if k < 1.0 {
                let l = Layer { source: Old, src: FULL, quad: q, uv, alpha: 1.0, fade: [1.0; 4] };
                v.push(l);
                v.push(l.shade(0.1 * crack * rnd(id + 9) + 0.2 * k));
            }
        }
    }
    v
}

fn crush(t: f64, a: f64) -> Vec<Layer> {
    // The old slide crumples into a ball that drops away.
    let mut v = vec![Layer::full(New, 1.0)];
    let k = q2(win(t, 0.08, 0.55));
    let drop = ein(win(t, 0.65, 1.0));
    let size = 1.0 - 0.45 * k;
    let cx = 0.5 - 0.1 * k + 0.05 * drop;
    let cy = 0.5 + 0.05 * k + 0.8 * drop;
    let n = 40usize;
    let c = [0.5, 0.5];
    for i in 0..n {
        let a0 = 2.0 * PI * i as f64 / n as f64;
        let a1 = 2.0 * PI * (i + 1) as f64 / n as f64;
        let j = |ang: f64, idx: usize| -> [f64; 2] {
            let b = boundary(ang, a);
            // Corners pull in toward a round ball, with crumpled jags.
            let round = {
                let (dx, dy) = ((b[0] - 0.5) * a, b[1] - 0.5);
                let len = (dx * dx + dy * dy).sqrt().max(1e-9);
                let r = 0.45;
                [0.5 + dx / len * r / a, 0.5 + dy / len * r]
            };
            let jag = 1.0 - 0.18 * k * rnd(idx as u64 % n as u64 + 77);
            let p = [b[0] + (round[0] - b[0]) * k, b[1] + (round[1] - b[1]) * k];
            [cx + (p[0] - c[0]) * size * jag, cy + (p[1] - c[1]) * size * jag]
        };
        let (p0, p1) = (j(a0, i), j(a1, i + 1));
        let (u0, u1) = (boundary(a0, a), boundary(a1, a));
        let l = Layer::tri_uv(Old, [[cx, cy], p0, p1], [c, u0, u1], [1.0; 3], 1.0);
        v.push(l);
        v.push(l.shade(0.35 * k * rnd(i as u64 + 13)));
    }
    v
}

fn peel_off(right: bool, t: f64) -> Vec<Layer> {
    // The old slide peels from a bottom corner toward the opposite top corner; the peeled flap
    // shows the page's back folded over.
    let u = ss(win(t, 0.2, 0.95));
    // Fold line perpendicular to the diagonal, sweeping from the corner.
    let (cx, cy) = if right { (0.0, 1.0) } else { (1.0, 1.0) };
    let nd: [f64; 2] = if right { [1.0, -1.0] } else { [-1.0, -1.0] };
    let len = (nd[0] * nd[0] + nd[1] * nd[1]).sqrt();
    let n = [nd[0] / len, nd[1] / len];
    let span = 2f64.sqrt();
    let d0 = n[0] * cx + n[1] * cy;
    let fold = d0 + span * 1.25 * u;
    let mut v = vec![Layer::full(New, 1.0)];
    let full = rect_poly(FULL);
    // Kept part: beyond the fold line from the corner.
    let keep = clip(&full, [-n[0], -n[1]], -fold);
    v.extend(poly_layers(Old, &keep, 1.0, &|p| p));
    // Peeled part, reflected over the fold line.
    let peeled = clip(&full, n, fold);
    let refl = |p: [f64; 2]| {
        let d = n[0] * p[0] + n[1] * p[1] - fold;
        [p[0] - 2.0 * d * n[0], p[1] - 2.0 * d * n[1]]
    };
    let flap = poly_layers(Old, &peeled, 1.0, &refl);
    for l in flap {
        v.push(l);
        v.push(l.shade(0.25));
    }
    v
}

fn page_curl(o: &str, t: f64) -> Vec<Layer> {
    let single = o.starts_with("single");
    let right = o.ends_with('r') || o.ends_with('R');
    let mut v = vec![Layer::full(New, 1.0)];
    // A roll travelling across the old slide: x of the roll's front edge.
    let roll = |area: R, k: f64, toward_left: bool, v: &mut Vec<Layer>| {
        let w = area[2] - area[0];
        let r = 0.06;
        if toward_left {
            let e = area[2] - (w + r) * k;
            let keep = [area[0], area[1], e.max(area[0]), area[3]];
            if keep[2] > keep[0] {
                v.push(Layer::inplace(Old, keep, 1.0));
            }
            // The roll: a band squeezing the rolled-up part.
            let band = [e.max(area[0]), area[1], (e + r).min(area[2]).max(area[0]), area[3]];
            if band[2] > band[0] && k < 1.0 {
                let src = [band[0], area[1], area[2], area[3]];
                let l = Layer::rect(Old, src, band, 1.0);
                v.push(l);
                v.push(l.shade(0.3).faded([0.2, 1.0, 1.0, 0.2]));
            }
        } else {
            let e = area[0] + (w + r) * k;
            let keep = [e.min(area[2]), area[1], area[2], area[3]];
            if keep[2] > keep[0] {
                v.push(Layer::inplace(Old, keep, 1.0));
            }
            let band = [(e - r).max(area[0]).min(area[2]), area[1], e.min(area[2]), area[3]];
            if band[2] > band[0] && k < 1.0 {
                let src = [area[0], area[1], band[2], area[3]];
                let l = Layer::rect(Old, src, band, 1.0);
                v.push(l);
                v.push(l.shade(0.3).faded([1.0, 0.2, 0.2, 1.0]));
            }
        }
    };
    if single {
        let k = q2(win(t, 0.25, 0.85));
        roll(FULL, k, !right, &mut v);
    } else {
        // Double: the half on the far side rolls first, then the near half.
        let k1 = q2(win(t, 0.1, 0.6));
        let k2 = q2(win(t, 0.45, 0.85));
        if right {
            roll([0.0, 0.0, 0.5, 1.0], k1, false, &mut v);
            roll([0.5, 0.0, 1.0, 1.0], k2, false, &mut v);
        } else {
            roll([0.5, 0.0, 1.0, 1.0], k1, true, &mut v);
            roll([0.0, 0.0, 0.5, 1.0], k2, true, &mut v);
        }
    }
    v
}

fn airplane(right: bool, t: f64, a: f64) -> Vec<Layer> {
    // The old slide folds into a big paper dart (nose to the right for "l") that hangs low, then
    // flies off up and away.
    let fold = q2(win(t, 0.0, 0.3));
    let fly = ein(win(t, 0.82, 0.95));
    let mut v = vec![Layer::full(New, 1.0)];
    if fly >= 1.0 {
        return v;
    }
    let mx = |p: [f64; 2]| if right { [1.0 - p[0], p[1]] } else { p };
    // The dart in its hanging pose ("l"), then flown off.
    let drift = 0.04 * win(t, 0.3, 0.82);
    let pose = |p: [f64; 2]| -> [f64; 2] {
        let p = [p[0] + drift, p[1] + drift];
        let c = [0.75, 0.65];
        let k = 1.0 - 0.85 * fly;
        [c[0] + (p[0] - c[0]) * k - 0.4 * fly, c[1] + (p[1] - c[1]) * k - 1.0 * fly]
    };
    let dart = [[0.42, 0.28], [1.02, 0.66], [1.0, 0.72], [0.46, 1.08]];
    let flat = [[0.0, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 1.0]];
    let pts: Vec<[f64; 2]> = (0..4)
        .map(|i| {
            let (f, d) = (flat[i], dart[i]);
            mx(pose([f[0] + (d[0] - f[0]) * fold, f[1] + (d[1] - f[1]) * fold]))
        })
        .collect();
    let uv: Vec<[f64; 2]> = flat.iter().map(|p| mx(*p)).collect();
    if let (Some(p0), Some(p1), Some(p2), Some(p3)) = (pts.first(), pts.get(1), pts.get(2), pts.get(3)) {
        let l1 = Layer::tri_uv(Old, [*p0, *p1, *p2], [uv[0], uv[1], uv[2]], [1.0; 3], 1.0);
        let l2 = Layer::tri_uv(Old, [*p0, *p2, *p3], [uv[0], uv[2], uv[3]], [1.0; 3], 1.0);
        v.push(l1);
        v.push(l1.shade(0.1 * fold));
        v.push(l2);
        v.push(l2.shade(0.3 * fold));
    }
    let _ = a;
    v
}

fn origami(right: bool, t: f64, a: f64) -> Vec<Layer> {
    // The old slide folds into a bird that flies off.
    let fold = q2(win(t, 0.0, 0.2));
    let fly = ein(win(t, 0.2, 0.85));
    let side = if right { 1.0 } else { -1.0 };
    let mut v = vec![Layer::full(New, 1.0)];
    if win(t, 0.2, 0.92) >= 1.0 {
        return v;
    }
    let size = 1.0 - 0.35 * fold;
    let cx = 0.47 + side * 0.65 * fly;
    let cy = 0.42 + 0.05 * fly;
    let flap = (t * 18.0).sin() * 0.08 * fold;
    let body = |p: [f64; 2]| [cx + (p[0] - 0.5) * size, cy + (p[1] - 0.5) * size];
    let shape = [
        ([0.0, 0.0], [-0.45 * side, -0.25 - flap]),
        ([1.0, 0.0], [0.15 * side, -0.4 + flap]),
        ([1.0, 1.0], [0.45 * side, 0.05]),
        ([0.0, 1.0], [-0.1 * side, 0.2]),
    ];
    let pts: Vec<[f64; 2]> =
        shape.iter().map(|(flat, bird)| body([flat[0] + (0.5 + bird[0] - flat[0]) * fold, flat[1] + (0.5 + bird[1] - flat[1]) * fold])).collect();
    let uv: Vec<[f64; 2]> = shape.iter().map(|(flat, _)| *flat).collect();
    if let (Some(p0), Some(p1), Some(p2), Some(p3)) = (pts.first(), pts.get(1), pts.get(2), pts.get(3)) {
        let l1 = Layer::tri_uv(Old, [*p0, *p1, *p2], [uv[0], uv[1], uv[2]], [1.0; 3], 1.0);
        let l2 = Layer::tri_uv(Old, [*p0, *p2, *p3], [uv[0], uv[2], uv[3]], [1.0; 3], 1.0);
        v.push(l1);
        v.push(l1.shade(0.15 * fold));
        v.push(l2);
        v.push(l2.shade(0.3 * fold));
    }
    let _ = a;
    v
}

fn honeycomb(t: f64, a: f64) -> Vec<Layer> {
    // The old slide splits into hexagonal cells along dark seams; the cells fly at the viewer and
    // out of sight, leaving black; cells of the new slide pop in one by one and close the seams.
    let cols = 11usize;
    let w = 1.0 / cols as f64;
    let rh = w * a * 0.5;
    // Rows sit 0.866*rh apart; a settled hex reaches only 0.577*rh from its centre at the side
    // vertices, and the flat bottom zone 0.289*rh. Count the rows whose FLAT zone still covers
    // the slide's bottom: counting the vertex reach instead left V-notches of black above the
    // bottom edge on about a third of the aspect ratios (#420).
    let rows = ((1.0 / (rh * 0.866) - 1.0 / 3.0).ceil().max(1.0)) as usize + 1;
    let mut v = vec![Layer::full(Black, 1.0)];
    let seam = ss(win(t, 0.0, 0.12)) * (1.0 - ss(win(t, 0.85, 1.0)));
    let hex = |cx: f64, cy: f64, s: f64| -> Vec<[f64; 2]> {
        (0..6)
            .map(|k| {
                let ang = PI / 3.0 * k as f64 + PI / 6.0;
                [cx + ang.cos() * s * w * 0.577, cy + ang.sin() * s * rh * 0.577 / 0.5 * 0.5]
            })
            .collect()
    };
    for r in 0..rows {
        for c in 0..=cols {
            let cx = (c as f64 + if r % 2 == 1 { 0.5 } else { 0.0 }) * w;
            let cy = r as f64 * rh * 0.866;
            let id = (r * 64 + c) as u64;
            let src_hex = hex(cx, cy, 1.0);
            let out_start = 0.1 + 0.2 * rnd(id + 1);
            let out_k = ein(win(t, out_start, out_start + 0.12));
            let in_start = 0.38 + 0.42 * rnd(id + 3);
            let in_k = eout(win(t, in_start, in_start + 0.1));
            let gap = 1.0 - 0.1 * seam;
            let mut cell = |src: Source, s: f64, alpha: f64| {
                let dst = hex(cx, cy, s);
                for k in 0..6 {
                    let (p0, p1) = (dst[k], dst[(k + 1) % 6]);
                    let (u0, u1) = (src_hex[k], src_hex[(k + 1) % 6]);
                    v.push(Layer::tri_uv(src, [[cx, cy], p0, p1], [[cx, cy], u0, u1], [1.0; 3], alpha));
                }
            };
            if t < 0.5 && out_k < 1.0 {
                cell(Old, gap * (1.0 + 3.0 * out_k), 1.0 - out_k * out_k);
            }
            if in_k > 0.0 {
                cell(New, gap * (0.3 + 0.7 * in_k), 1.0);
            }
        }
    }
    v
}

fn glitter(o: &str, t: f64, a: f64) -> Vec<Layer> {
    // Tiles flip from the old slide to the new in a sparkling sweep starting on the option's side.
    let hexes = o.starts_with("hex");
    let d = o.trim_start_matches("hex").to_ascii_lowercase();
    let (n, m) = if hexes { (28usize, 16usize) } else { (40usize, 22usize) };
    let cam = Cam { a, d: 3.0 };
    let mut v = vec![Layer::full(Old, 1.0)];
    for r in 0..m {
        for c in 0..n {
            let cell = [c as f64 / n as f64, r as f64 / m as f64, (c + 1) as f64 / n as f64, (r + 1) as f64 / m as f64];
            let cc = center(cell);
            // Distance from the side the sweep starts on ("r": from the right).
            let grad = match d.as_str() {
                "r" => 1.0 - cc[0],
                "u" => cc[1],
                "d" => 1.0 - cc[1],
                _ => cc[0],
            };
            let start = 0.75 * grad * 0.85 + rnd((r * n + c) as u64 + 77) * 0.12;
            let k = win(t, start, start + 0.25);
            if k <= 0.0 {
                continue;
            }
            v.push(Layer::inplace(Black, cell, 1.0));
            flip_tile(&cam, cell, k, true, &mut v);
        }
    }
    v
}

fn vortex(o: &str, t: f64, a: f64) -> Vec<Layer> {
    // The old slide breaks into fine particles that whirl off toward the option's side, the
    // trailing side first; the new slide's particles whirl in and settle, the leading side first.
    let (dx, dy) = dir(o, (-1.0, 0.0));
    let (n, m) = (48usize, 27usize);
    let mut v = vec![Layer::full(Black, 1.0)];
    for r in 0..m {
        for c in 0..n {
            let id = (r * n + c) as u64;
            let cell = [c as f64 / n as f64, r as f64 / m as f64, (c + 1) as f64 / n as f64, (r + 1) as f64 / m as f64];
            let cc = center(cell);
            // 1 on the side things move toward, 0 on the other.
            let lead = (0.5 + (cc[0] - 0.5) * dx + (cc[1] - 0.5) * dy).clamp(0.0, 1.0);
            let go = 0.4 * lead + 0.08 * rnd(id + 31);
            let ko = win(t, go, go + 0.35);
            let end = 0.62 + 0.3 * (1.0 - lead) + 0.06 * rnd(id + 41);
            let kn = 1.0 - win(t, end - 0.35, end);
            let mut piece = |src: Source, k: f64| {
                let swirl = 2.0 * PI * (rnd(id + 5) - 0.5) * k;
                let off = [dx * k * 0.8 + swirl.sin() * 0.15 * k, dy * k * 0.8 - swirl.cos().abs() * 0.12 * k];
                let s = 0.8 - 0.5 * k;
                let r2 = offset(scale_rect(cell, s), off[0], off[1]);
                let q = rot_quad(crate::transition::corners(r2), center(r2), rnd(id) * 360.0 * k, a);
                v.push(Layer::quad(src, cell, q, 1.0 - k * k));
            };
            if kn < 1.0 {
                piece(New, kn);
            }
            if ko > 0.0 && ko < 1.0 && kn >= 1.0 {
                piece(Old, ko);
            } else if ko <= 0.0 {
                v.push(Layer::inplace(Old, cell, 1.0));
            }
        }
    }
    v
}

fn shred(o: &str, t: f64, a: f64) -> Vec<Layer> {
    let particles = o.starts_with("particles") || o.starts_with("rect");
    let out = o.ends_with("Out");
    let cam = Cam { a, d: 2.5 };
    let mut v = vec![Layer::full(Black, 1.0)];
    // First half: the old slide comes apart and recedes; second half: the new one assembles.
    let (src, k) = if t < 0.55 { (Old, q2(win(t, 0.05, 0.55))) } else { (New, 1.0 - q2(win(t, 0.55, 0.95))) };
    if particles {
        let (n, m) = (64usize, 36usize);
        for r in 0..m {
            for c in 0..n {
                let id = (r * n + c) as u64;
                let cell = [c as f64 / n as f64, r as f64 / m as f64, (c + 1) as f64 / n as f64, (r + 1) as f64 / m as f64];
                let cc = center(cell);
                let z = if out { -0.6 } else { 1.5 } * k * (0.5 + rnd(id));
                let jitter = [(rnd(id + 1) - 0.5) * 0.4 * k, (rnd(id + 2) - 0.5) * 0.4 * k];
                let p3 = cam.unit(cc[0] + jitter[0], cc[1] + jitter[1]);
                let pc = cam.p(v3(p3.x, p3.y, z));
                let s = 0.8 * (1.0 - 0.3 * k) * cam.d / (cam.d + z).max(0.1);
                let r2 = offset(scale_rect(cell, s), pc[0] - cc[0], pc[1] - cc[1]);
                let l = Layer::rect(src, cell, r2, 1.0 - 0.3 * k * rnd(id + 5));
                v.push(l);
                v.push(l.shade(0.4 * k));
            }
        }
    } else {
        let n = 30usize;
        for i in 0..n {
            let id = i as u64;
            let (x0, x1) = (i as f64 / n as f64, (i + 1) as f64 / n as f64);
            let cell = [x0, 0.0, x1, 1.0];
            let z = if out { -0.3 } else { 1.0 } * k * k * (0.6 + rnd(id + 11));
            let tilt = (rnd(id + 12) - 0.5) * 30.0 * k;
            let drop = (rnd(id + 13) - 0.3) * 0.25 * k;
            let inset = 0.004 * k;
            let w3 = cam.rect([x0 + inset, 0.0, x1 - inset, 1.0]);
            let c3 = cam.unit((x0 + x1) / 2.0, 0.5);
            let pts = w3.map(|p| {
                let q = p.roll(c3, tilt * 0.2).yaw(c3, tilt);
                v3(q.x, q.y + drop, q.z + z)
            });
            v.extend(card(&cam, src, cell, pts, 1.0, 0.3 * k, true));
        }
    }
    v
}

fn switch(right: bool, t: f64, a: f64) -> Vec<Layer> {
    // Switching cards: the old slide is pulled out to one side ("l": left) and slipped behind
    // while the new one, pulled out to the other side from behind it, comes to the front. Both
    // turn a little away at their outer edges.
    let cam = Cam { a, d: 2.5 };
    let side = if right { -1.0 } else { 1.0 };
    let u = (t + ss(t)) / 2.0;
    let k = (u * PI).sin();
    let shift = 0.5 * k * a;
    let old = cam.rect(FULL).map(|p| {
        let q = p.yaw(v3(0.0, 0.0, 0.0), side * 14.0 * k);
        v3(q.x - side * shift, q.y, q.z + 0.12 * k)
    });
    let new = cam.rect(FULL).map(|p| {
        let q = p.yaw(v3(0.0, 0.0, 0.0), -side * 14.0 * k);
        v3(q.x + side * shift, q.y, q.z + 0.12 * k)
    });
    let mut v = vec![Layer::full(Black, 1.0)];
    let o = card(&cam, Old, FULL, old, 1.0, facing_shade(old, 0.3), true);
    let n = card(&cam, New, FULL, new, 1.0, facing_shade(new, 0.3), true);
    if u < 0.5 {
        v.extend(n);
        v.extend(o);
    } else {
        v.extend(o);
        v.extend(n);
    }
    v
}

fn flip(right: bool, t: f64, a: f64) -> Vec<Layer> {
    // The slide turns over like a card: the old face, then the new one on its back.
    let cam = Cam { a, d: 2.5 };
    let s = q2(t);
    let side = if right { 1.0 } else { -1.0 };
    let deg = 180.0 * s * side;
    let back = 0.6 * (s * PI).sin();
    let c = v3(0.0, 0.0, 0.0);
    let pts = cam.rect(FULL).map(|p| {
        let q = p.yaw(c, deg);
        v3(q.x, q.y, q.z + back)
    });
    let mut v = vec![Layer::full(Black, 1.0)];
    if s < 0.5 {
        v.extend(card(&cam, Old, FULL, pts, 1.0, facing_shade(pts, 0.5), true));
    } else {
        // The back face: the new slide, mirrored so it reads right way round.
        let m = [pts[1], pts[0], pts[3], pts[2]];
        v.extend(card(&cam, New, FULL, m, 1.0, facing_shade(m, 0.5), true));
    }
    v
}

fn gallery(right: bool, t: f64, a: f64) -> Vec<Layer> {
    // The slides hang on a wall: it turns away and recedes, the slides slide along it, then it
    // turns back. A faint reflection shows below.
    let cam = Cam { a, d: 2.5 };
    let side = if right { 1.0 } else { -1.0 };
    let k = ss(win(t, 0.0, 0.35)) * (1.0 - ss(win(t, 0.7, 1.0)));
    let s = q2(win(t, 0.3, 0.8));
    let gap = 0.15;
    let back = 0.7 * k;
    let yaw = -side * 22.0 * k;
    let mut v = vec![Layer::full(Black, 1.0)];
    for (src, shift) in [(Old, side * s), (New, side * (s - 1.0))] {
        let x0 = shift * (a + gap);
        let pts = cam.rect(FULL).map(|p| v3(p.x + x0, p.y, p.z + back)).map(|p| p.yaw(v3(0.0, 0.0, back), yaw));
        // Reflection: the slide flipped below its bottom edge, faint.
        let refl = [pts[3], pts[2], v3(pts[2].x, pts[2].y + 0.25, pts[2].z), v3(pts[3].x, pts[3].y + 0.25, pts[3].z)];
        let rq = cam.quad(refl);
        v.push(Layer {
            source: src,
            src: [0.0, 0.75, 1.0, 1.0],
            quad: rq,
            uv: [[0.0, 1.0], [1.0, 1.0], [1.0, 0.75], [0.0, 0.75]],
            alpha: 0.3 * k,
            fade: [1.0, 1.0, 0.0, 0.0],
        });
        v.extend(card(&cam, src, FULL, pts, 1.0, 0.0, true));
    }
    v
}

fn conveyor(right: bool, t: f64, a: f64) -> Vec<Layer> {
    // The contents ride a belt: it turns away, the contents slide along, it turns back; the
    // backgrounds cross-fade underneath.
    let cam = Cam { a, d: 2.5 };
    let side = if right { 1.0 } else { -1.0 };
    let k = ss(win(t, 0.0, 0.35)) * (1.0 - ss(win(t, 0.65, 1.0)));
    let s = q2(win(t, 0.33, 0.75));
    let back = 0.45 * k;
    let yaw = -side * 18.0 * k;
    let mut v = vec![Layer::full(OldBack, 1.0), Layer::full(NewBack, ss(t))];
    for (src, shift) in [(OldContent, side * s), (NewContent, side * (s - 1.0))] {
        let x0 = shift * a * 1.05;
        let pts = cam.rect(FULL).map(|p| v3(p.x + x0, p.y, p.z + back)).map(|p| p.yaw(v3(0.0, 0.0, back), yaw));
        v.extend(card(&cam, src, FULL, pts, 1.0, 0.0, true));
    }
    v
}

fn prism(kind: &str, o: &str, t: f64, a: f64) -> Vec<Layer> {
    // Cube: the slides are two faces of a cube turning; Box: two inside walls of a box; Rotate and
    // Orbit: the same with the contents only, over cross-fading backgrounds.
    let (dx, dy) = dir(o, (-1.0, 0.0));
    let vertical = dy != 0.0 && dx == 0.0;
    let content = matches!(kind, "rotate" | "orbit");
    let inverted = matches!(kind, "box" | "orbit");
    let cam = Cam { a, d: 2.5 };
    let s = q2(t);
    let half = if vertical { 0.5 } else { a / 2.0 };
    // Faces on a cube around (0, 0, half) (outside), or the inside of a box around (0, 0, -half).
    let c = v3(0.0, 0.0, if inverted { -half } else { half });
    // Positive turns the right (or top) side away.
    let sign = if vertical { -dy } else { dx };
    let deg = 90.0 * s * sign * if inverted { -1.0 } else { 1.0 };
    let turn = |p: V3, base: f64| -> V3 { if vertical { p.pitch(c, base + deg) } else { p.yaw(c, base + deg) } };
    let side_base = -90.0 * sign * if inverted { -1.0 } else { 1.0 };
    let (os, ns) = if content { (OldContent, NewContent) } else { (Old, New) };
    let old = cam.rect(FULL).map(|p| turn(p, 0.0));
    let new = cam.rect(FULL).map(|p| turn(p, side_base));
    // Keep the nearest edge on the screen plane, as PowerPoint's camera does.
    let near = old.iter().chain(new.iter()).map(|p| p.z).fold(f64::INFINITY, f64::min);
    let lift = if inverted { 0.0 } else { -near.min(0.0) - near.max(0.0) };
    // The box is seen from further back mid-turn.
    let back = if inverted { 0.5 } else { 0.15 } * (s * PI).sin();
    let push = |q: [V3; 4]| q.map(|p| v3(p.x, p.y, p.z + lift + back));
    let (old, new) = (push(old), push(new));
    let mut v = if content { vec![Layer::full(OldBack, 1.0), Layer::full(NewBack, ss(t))] } else { vec![Layer::full(Black, 1.0)] };
    v.extend(card(&cam, os, FULL, old, 1.0, if content { 0.0 } else { facing_shade(old, 0.6) }, false));
    v.extend(card(&cam, ns, FULL, new, 1.0, if content { 0.0 } else { facing_shade(new, 0.6) }, false));
    v
}

fn doors(window: bool, horz: bool, t: f64, a: f64) -> Vec<Layer> {
    // The old slide parts in two halves that swing open toward the viewer and darken; the new one
    // comes forward from the middle, fading in over them. Window does it with the contents only,
    // over cross-fading backgrounds.
    let cam = Cam { a, d: 2.2 };
    let open = ein(win(t, 0.05, 0.68));
    let come = q2(win(t, 0.0, 0.72));
    let (os, ns) = if window { (OldContent, NewContent) } else { (Old, New) };
    let mut v = if window { vec![Layer::full(OldBack, 1.0), Layer::full(NewBack, ss(t))] } else { vec![Layer::full(Black, 1.0)] };
    let halves: [(R, f64); 2] =
        if horz { [([0.0, 0.0, 1.0, 0.5], -1.0), ([0.0, 0.5, 1.0, 1.0], 1.0)] } else { [([0.0, 0.0, 0.5, 1.0], -1.0), ([0.5, 0.0, 1.0, 1.0], 1.0)] };
    if open < 1.0 {
        for (r, sgn) in halves {
            let hinge = if horz { cam.unit(0.5, if sgn < 0.0 { 0.0 } else { 1.0 }) } else { cam.unit(if sgn < 0.0 { 0.0 } else { 1.0 }, 0.5) };
            let ang = 30.0 * open;
            let slide = 0.55 * open;
            let pts = cam.rect(r).map(|p| {
                let q = if horz { p.pitch(hinge, sgn * ang) } else { p.yaw(hinge, -sgn * ang) };
                if horz { v3(q.x, q.y + sgn * slide, q.z) } else { v3(q.x + sgn * slide * a, q.y, q.z) }
            });
            let dark = if window { 0.0 } else { 0.7 * open };
            let alpha = if window { 1.0 - ss(win(t, 0.2, 0.6)) } else { 1.0 };
            v.extend(card(&cam, os, r, pts, alpha, dark, true));
        }
    }
    let sc = 0.5 + 0.5 * come;
    let fade_in = if window { ss(t) } else { ss(win(t, 0.05, 0.62)) };
    v.push(scaled(ns, sc, 0.5, 0.5, 0.0, 0.0, fade_in));
    v
}

fn ferris(right: bool, t: f64, a: f64) -> Vec<Layer> {
    // The contents ride a big wheel turning about a hub far off the lower corner: the old ones
    // drop away down and sideways, the new ones come down round from the top, over cross-fading
    // backgrounds.
    let side = if right { -1.0 } else { 1.0 };
    let mut v = vec![Layer::full(OldBack, 1.0), Layer::full(NewBack, ss(t))];
    let out = ein(win(t, 0.0, 0.5));
    if out < 1.0 {
        let (dx, dy) = (-side * 0.45 * out.powf(1.5), 0.9 * out);
        let q = rot_quad(crate::transition::corners(offset(FULL, dx, dy)), [0.5 + dx, 0.5 + dy], side * -15.0 * out, a);
        v.push(Layer::quad(OldContent, FULL, q, 1.0));
    }
    let inn = q2(win(t, 0.5, 1.0));
    if inn > 0.0 {
        let k = 1.0 - inn;
        let (dx, dy) = (-side * 0.5 * k.powf(1.3), -0.75 * k);
        let r = offset(scale_rect(FULL, 1.0 - 0.25 * k), dx, dy);
        let q = rot_quad(crate::transition::corners(r), center(r), side * -18.0 * k, a);
        v.push(Layer::quad(NewContent, FULL, q, 1.0));
    }
    v
}

fn pan(o: &str, t: f64) -> Vec<Layer> {
    // The contents slide along together while the backgrounds cross-fade.
    let (dx, dy) = dir(o, (-1.0, 0.0));
    // Quick start, long glide.
    let s = ease(t, 0.2, 0.8, 0.0);
    vec![
        Layer::full(OldBack, 1.0),
        Layer::full(NewBack, s),
        moved(OldContent, dx * s, dy * s, 1.0),
        moved(NewContent, dx * (s - 1.0), dy * (s - 1.0), 1.0),
    ]
}

fn flythrough(o: &str, t: f64) -> Vec<Layer> {
    // The contents fly past the viewer (in) or recede (out) while the backgrounds cross-fade.
    let bounce = o.ends_with("Bounce");
    let out = o.starts_with("out");
    let s = q2(t);
    let b = if bounce { 0.08 * (win(t, 0.6, 1.0) * PI).sin() } else { 0.0 };
    let old_a = 1.0 - ss(win(t, 0.0, 0.6));
    let new_a = ss(win(t, 0.3, 0.95));
    let (os, ns) = if out { (1.0 - 0.5 * s, 1.8 - 0.8 * s - b) } else { (1.0 + 1.5 * s, 0.4 + 0.6 * s + b) };
    vec![
        Layer::full(OldBack, 1.0),
        Layer::full(NewBack, ss(t)),
        scaled(NewContent, ns, 0.5, 0.5, 0.0, 0.0, new_a),
        scaled(OldContent, os, 0.5, 0.5, 0.0, 0.0, old_a),
    ]
}
