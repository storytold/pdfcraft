//! Gradient colour ramps as PowerPoint draws them.
//!
//! A gradient of exactly two stops, at 0 % and 100 % (what a two-colour gradient is), is not
//! blended linearly in sRGB by PowerPoint: it mixes the colours in linear light, and the second
//! colour's share follows an S-shaped curve of the position. Black to white is 105 (not 64) a
//! quarter of the way, 186 (not 128) half way and 238 (not 191) three quarters of the way, in any
//! direction, on shapes and on slide backgrounds. Gradients with three or more stops, or with a
//! stop moved off an end, blend linearly in sRGB (measured from PowerPoint's own slide export).
//! The renderer interpolates linearly in sRGB, so a two-colour gradient is expanded into short
//! pieces whose colours follow PowerPoint's ramp.

use deckcraft_color::Rgba;

/// The next stop's share of the linear-light mix at sixteenths of a segment (0 at the first stop,
/// 1 at the second). Measured, then made symmetric: `w(1 - t) = 1 - w(t)`.
const WEIGHT: [f64; 17] =
    [0.0, 0.0205, 0.0469, 0.0878, 0.1431, 0.2143, 0.2989, 0.3972, 0.5, 0.6028, 0.7011, 0.7857, 0.8569, 0.9122, 0.9531, 0.9795, 1.0];

/// Pieces each segment between two stops is cut into.
const PIECES: usize = 16;

/// The next stop's share at `t` (0..1) between two stops.
pub fn weight(t: f64) -> f64 {
    if !t.is_finite() {
        return 0.0;
    }
    let x = t.clamp(0.0, 1.0) * (WEIGHT.len() - 1) as f64;
    let i = (x.floor() as usize).min(WEIGHT.len() - 2);
    let f = x - i as f64;
    let (a, b) = (WEIGHT.get(i).copied().unwrap_or(0.0), WEIGHT.get(i + 1).copied().unwrap_or(1.0));
    a + (b - a) * f
}

fn to_linear(v: u8) -> f64 {
    let v = v as f64 / 255.0;
    if v <= 0.04045 { v / 12.92 } else { ((v + 0.055) / 1.055).powf(2.4) }
}

fn to_srgb(v: f64) -> u8 {
    let v = v.clamp(0.0, 1.0);
    let s = if v <= 0.0031308 { v * 12.92 } else { 1.055 * v.powf(1.0 / 2.4) - 0.055 };
    (s * 255.0).round().clamp(0.0, 255.0) as u8
}

/// The colour at `t` between stops `a` and `b`. Alpha mixes by the same share.
pub fn mix(a: Rgba, b: Rgba, t: f64) -> Rgba {
    let w = weight(t);
    let ch = |x: u8, y: u8| to_srgb(to_linear(x) + (to_linear(y) - to_linear(x)) * w);
    Rgba { r: ch(a.r, b.r), g: ch(a.g, b.g), b: ch(a.b, b.b), a: (a.a as f64 + (b.a as f64 - a.a as f64) * w).round().clamp(0.0, 255.0) as u8 }
}

/// Sorted stops (position 0..1, colour) as a renderer interpolating linearly in sRGB should get
/// them: a two-colour gradient (two stops, at 0 and 1) expanded to follow PowerPoint's ramp,
/// anything else unchanged.
pub fn expand(stops: &[(f64, Rgba)]) -> Vec<(f64, Rgba)> {
    let two_colour = matches!(stops, [(p, _), (q, _)] if *p <= 1e-6 && *q >= 1.0 - 1e-6);
    if !two_colour {
        return stops.to_vec();
    }
    let mut out = Vec::with_capacity(stops.len() * PIECES + 1);
    for (i, &(p, c)) in stops.iter().enumerate() {
        out.push((p, c));
        let Some(&(q, d)) = stops.get(i + 1) else { continue };
        if q - p <= 1e-6 || c == d {
            continue;
        }
        for k in 1..PIECES {
            let t = k as f64 / PIECES as f64;
            out.push((p + (q - p) * t, mix(c, d, t)));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn black_to_white_follows_powerpoint() {
        // PowerPoint's export: 105 at a quarter, 186 half way, 238 at three quarters.
        let (k, w) = (Rgba::rgb(0, 0, 0), Rgba::rgb(255, 255, 255));
        for (t, want) in [(0.25, 105), (0.5, 186), (0.75, 238)] {
            let got = mix(k, w, t).r as i32;
            assert!((got - want).abs() <= 3, "{t}: {got} vs {want}");
        }
    }

    #[test]
    fn colour_pairs_follow_powerpoint() {
        // Blue (4472C4) to orange (ED7D31) half way: PowerPoint draws (178, 120, 146).
        let m = mix(Rgba::rgb(68, 114, 196), Rgba::rgb(237, 125, 49), 0.5);
        assert!((m.r as i32 - 178).abs() <= 3 && (m.g as i32 - 120).abs() <= 3 && (m.b as i32 - 146).abs() <= 3, "{m:?}");
        // Red to blue a quarter of the way: (238, 0, 105).
        let m = mix(Rgba::rgb(255, 0, 0), Rgba::rgb(0, 0, 255), 0.25);
        assert!((m.r as i32 - 238).abs() <= 3 && (m.b as i32 - 105).abs() <= 3, "{m:?}");
    }

    #[test]
    fn the_ends_are_the_stops_and_the_weight_is_symmetric() {
        let (a, b) = (Rgba::rgb(10, 200, 30), Rgba::rgb(240, 20, 90));
        assert_eq!(mix(a, b, 0.0), a);
        assert_eq!(mix(a, b, 1.0), b);
        for k in 0..=32 {
            let t = k as f64 / 32.0;
            assert!((weight(t) + weight(1.0 - t) - 1.0).abs() < 1e-9);
            assert!(weight(t) <= weight((t + 0.01).min(1.0)), "monotonic");
        }
        assert_eq!(weight(f64::NAN), 0.0);
    }

    #[test]
    fn expand_keeps_stops_and_hard_edges() {
        let (r, g) = (Rgba::rgb(255, 0, 0), Rgba::rgb(0, 255, 0));
        // Three or more stops, or a stop off an end, blend linearly in sRGB: unchanged.
        let e = expand(&[(0.0, r), (0.5, r), (0.5, g), (1.0, g)]);
        assert_eq!(e, vec![(0.0, r), (0.5, r), (0.5, g), (1.0, g)]);
        assert_eq!(expand(&[(0.2, r), (1.0, g)]), vec![(0.2, r), (1.0, g)]);
        assert_eq!(expand(&[(0.0, r), (0.5, g), (1.0, r)]).len(), 3);
        // Two equal colours add nothing.
        assert_eq!(expand(&[(0.0, r), (1.0, r)]), vec![(0.0, r), (1.0, r)]);
        let e = expand(&[(0.0, r), (1.0, g)]);
        assert_eq!(e.len(), PIECES + 1);
        assert_eq!(e.first(), Some(&(0.0, r)));
        assert_eq!(e.last(), Some(&(1.0, g)));
        assert!(e.windows(2).all(|w| w[0].0 <= w[1].0));
    }
}
