//! PdfCraft patch (104): an sRGB colour as the process ink that draws back to it through the
//! CMYK profile pages are drawn with, for an additive colour painted into a `DeviceCMYK`
//! blending space (ISO 32000-2 §11.3.4).
//!
//! Black is `K = 0.4 · k₀² + 0.6 · k₀⁸` with `k₀ = 1 − max(r, g, b)`, light in the mid tones
//! and full for black; cyan, magenta and yellow are then solved for by Newton's method at that
//! black, clamped to the ink range. A colour inside the profile's gamut draws as itself; one
//! outside it lands where the clamped solve stops. The curve matches Acrobat's output for the
//! greys, black and red measured in a CMYK page group, plain and under a blend mode;
//! §10.4.2.4's black generation and undercolour removal is for a device without a profile.

use moxcms::{ColorProfile, Layout, TransformF32Executor, TransformOptions};
use std::cell::Cell;
use std::sync::{Arc, LazyLock};

use crate::color::CMYK_PROFILE;

/// The weight of `k₀²` in the black ink; the rest is on `k₀⁸`.
const BLACK_GENERATION: f32 = 0.4;
/// Newton steps; each costs four profile evaluations.
const MAX_STEPS: usize = 12;
/// Close enough: well under one 8-bit level.
const TOLERANCE: f32 = 0.5 / 255.0;
/// The finite-difference step for the Jacobian.
const STEP: f32 = 0.02;

static FORWARD: LazyLock<Option<Arc<TransformF32Executor>>> = LazyLock::new(|| {
    ColorProfile::new_from_slice(CMYK_PROFILE)
        .ok()?
        .create_transform_f32(
            Layout::Rgba,
            &ColorProfile::new_srgb(),
            Layout::Rgb,
            TransformOptions::default(),
        )
        .ok()
});

/// The process ink `[C, M, Y, K]` (0 = no ink) that the CMYK profile draws as `rgb`; for a
/// colour outside its gamut, where the clamped solve stops. `None` when the
/// profile can't be evaluated or `rgb` isn't finite.
///
/// The last colour converted on this thread is kept, as a run of fills (the glyphs of a
/// line of text) usually shares one colour.
pub(crate) fn srgb_to_cmyk(rgb: [f32; 3]) -> Option<[f32; 4]> {
    if !rgb.iter().all(|v| v.is_finite()) {
        return None;
    }
    let rgb = rgb.map(|v| v.clamp(0.0, 1.0));
    let key = rgb.map(f32::to_bits);
    if let Some((last, ink)) = LAST.get()
        && last == key
    {
        return Some(ink);
    }
    let ink = solve_ink(rgb)?;
    LAST.set(Some((key, ink)));
    Some(ink)
}

thread_local! {
    static LAST: Cell<Option<([u32; 3], [f32; 4])>> = const { Cell::new(None) };
}

fn solve_ink(rgb: [f32; 3]) -> Option<[f32; 4]> {
    let forward: &TransformF32Executor = FORWARD.as_ref()?.as_ref();
    let k0 = 1.0 - rgb[0].max(rgb[1]).max(rgb[2]);
    let k = BLACK_GENERATION * k0 * k0 + (1.0 - BLACK_GENERATION) * k0.powi(8);
    let eval = |cmy: [f32; 3]| draw(forward, [cmy[0], cmy[1], cmy[2], k]);

    let mut cmy = rgb.map(|v| 1.0 - v);
    for _ in 0..MAX_STEPS {
        let drawn = eval(cmy)?;
        let residual: [f32; 3] = std::array::from_fn(|i| drawn[i] - rgb[i]);
        if residual.iter().all(|r| r.abs() < TOLERANCE) {
            break;
        }
        // Column `j` of the Jacobian: how the drawn colour moves with ink `j`.
        let mut columns = [[0.0_f32; 3]; 3];
        for (j, column) in columns.iter_mut().enumerate() {
            let step = if cmy[j] <= 1.0 - STEP { STEP } else { -STEP };
            let mut moved = cmy;
            moved[j] += step;
            let shifted = eval(moved)?;
            *column = std::array::from_fn(|i| (shifted[i] - drawn[i]) / step);
        }
        let Some(delta) = solve(columns, residual.map(|r| -r)) else {
            break;
        };
        cmy = std::array::from_fn(|i| (cmy[i] + delta[i]).clamp(0.0, 1.0));
    }
    Some([cmy[0], cmy[1], cmy[2], k])
}

fn draw(forward: &TransformF32Executor, cmyk: [f32; 4]) -> Option<[f32; 3]> {
    let mut out = [0.0_f32; 3];
    forward.transform(&cmyk, &mut out).ok()?;
    out.iter().all(|v| v.is_finite()).then_some(out)
}

/// `x` with `A x = b`, `A` given by its columns (Cramer's rule). `None` when `A` is
/// singular or the result isn't finite.
fn solve(a: [[f32; 3]; 3], b: [f32; 3]) -> Option<[f32; 3]> {
    let det = |[c0, c1, c2]: [[f32; 3]; 3]| {
        c0[0] * (c1[1] * c2[2] - c2[1] * c1[2]) - c1[0] * (c0[1] * c2[2] - c2[1] * c0[2])
            + c2[0] * (c0[1] * c1[2] - c1[1] * c0[2])
    };
    let d = det(a);
    if !d.is_finite() || d.abs() < 1e-9 {
        return None;
    }
    let x: [f32; 3] = std::array::from_fn(|j| {
        let mut m = a;
        m[j] = b;
        det(m) / d
    });
    x.iter().all(|v| v.is_finite()).then_some(x)
}
