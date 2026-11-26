//! Time easing: acceleration / deceleration fractions and bounce.

use crate::clampf;

/// Map linear progress `p` (0..1) through PowerPoint-style easing: `accel` and `decel` are the
/// fractions of the duration spent speeding up and slowing down (velocity ramps linearly);
/// `bounce` (0..1) makes the last part of the motion bounce against the end.
pub fn ease(p: f64, accel: f64, decel: f64, bounce: f64) -> f64 {
    let p = clampf(p, 0.0, 1.0, 0.0);
    let mut a = clampf(accel, 0.0, 1.0, 0.0);
    let mut d = clampf(decel, 0.0, 1.0, 0.0);
    if a + d > 1.0 {
        let s = a + d;
        a /= s;
        d /= s;
    }
    let v = 2.0 / (2.0 - a - d);
    let mut y = if a > 0.0 && p < a {
        v * p * p / (2.0 * a)
    } else if d > 0.0 && p > 1.0 - d {
        let q = 1.0 - p;
        1.0 - v * q * q / (2.0 * d)
    } else {
        v * a / 2.0 + v * (p - a)
    };
    let b = clampf(bounce, 0.0, 1.0, 0.0);
    if b > 0.0 {
        // Reach the end early, then bounce back with decaying hops over the last `b` fraction.
        let hit = 1.0 - b;
        if p < hit {
            let q = if hit > 0.0 { p / hit } else { 1.0 };
            y = q * q;
        } else {
            let s = (p - hit) / b;
            y = 1.0 - bounce_hops(s) * 0.25;
        }
    }
    clampf(y, -1.0, 2.0, 0.0)
}

/// Decaying hops for s in 0..1: 0 at the ends of each hop, peaks shrinking.
fn bounce_hops(s: f64) -> f64 {
    let s = clampf(s, 0.0, 1.0, 1.0);
    let hops = 3.0;
    let k = (s * hops).floor().min(hops - 1.0);
    let local = s * hops - k;
    let amp = 0.5f64.powf(k);
    amp * (local * std::f64::consts::PI).sin()
}

/// Classic bounce-out: falls to 1 and bounces with decaying height.
pub(crate) fn bounce_out(p: f64) -> f64 {
    let p = clampf(p, 0.0, 1.0, 0.0);
    let n = 7.5625;
    let d = 2.75;
    let y = if p < 1.0 / d {
        n * p * p
    } else if p < 2.0 / d {
        let q = p - 1.5 / d;
        n * q * q + 0.75
    } else if p < 2.5 / d {
        let q = p - 2.25 / d;
        n * q * q + 0.9375
    } else {
        let q = p - 2.625 / d;
        n * q * q + 0.984375
    };
    y.min(1.0)
}

/// Smooth ease-in-out (cubic smoothstep).
pub(crate) fn smooth(p: f64) -> f64 {
    let p = clampf(p, 0.0, 1.0, 0.0);
    p * p * (3.0 - 2.0 * p)
}
