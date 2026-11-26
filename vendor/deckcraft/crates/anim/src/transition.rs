//! Slide transitions as layers of textured quads.
//!
//! A frame is drawn over black: each [`Layer`] maps part of a source image (old slide, new slide,
//! their backgrounds or contents alone, or a solid colour) onto a quad in slide-unit space (0..1, y
//! down), with an opacity per corner. Perspective comes from projecting 3-D cards
//! ([`crate::layer_mesh`] keeps their textures straight); soft edges come from per-corner opacity
//! ramps; wedges are triangles (4th corner repeats the 3rd).
//!
//! The motions, curves, directions and option meanings were measured frame by frame on
//! PowerPoint's video export of synthetic decks (`plan/powerpoint/tools/synth_transitions.ps1`).
//! The effects themselves live in [`crate::transition_fx`].

use serde::{Deserialize, Serialize};

use crate::clampf;

/// What a layer shows.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Source {
    /// The outgoing slide.
    Old,
    /// The incoming slide.
    New,
    Black,
    White,
    /// The outgoing slide's background (background fill plus master and layout graphics) alone.
    OldBack,
    /// The incoming slide's background alone.
    NewBack,
    /// The outgoing slide's own shapes alone, over transparency.
    OldContent,
    /// The incoming slide's own shapes alone, over transparency.
    NewContent,
}

impl Source {
    /// The whole-slide source to draw instead when the parts aren't available: a background
    /// becomes its slide, content becomes nothing.
    pub fn whole(self) -> Option<Source> {
        match self {
            Source::OldBack => Some(Source::Old),
            Source::NewBack => Some(Source::New),
            Source::OldContent | Source::NewContent => None,
            s => Some(s),
        }
    }
    /// Does this source need the slide rendered in parts (background / content)?
    pub fn is_part(self) -> bool {
        matches!(self, Source::OldBack | Source::NewBack | Source::OldContent | Source::NewContent)
    }
}

fn ones() -> [f64; 4] {
    [1.0; 4]
}

/// One textured quad of a transition frame.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Layer {
    pub source: Source,
    /// Source sub-rectangle in uv space (0..1): l, t, r, b (bounds of `uv`).
    pub src: [f64; 4],
    /// Destination corners in slide-unit space: tl, tr, br, bl.
    pub quad: [[f64; 2]; 4],
    /// Texture coordinate of each destination corner (tl, tr, br, bl). For plain rectangles these
    /// are the corners of `src`; triangles and mirrored flaps carry their own. Draw with these.
    pub uv: [[f64; 2]; 4],
    pub alpha: f64,
    /// Opacity of each corner (tl, tr, br, bl), multiplied by `alpha` and interpolated across the
    /// quad: soft edges and feathered masks.
    #[serde(default = "ones")]
    pub fade: [f64; 4],
}

pub(crate) type R = [f64; 4];
pub(crate) type P = [f64; 2];
pub(crate) type Q = [[f64; 2]; 4];
pub(crate) const FULL: R = [0.0, 0.0, 1.0, 1.0];

pub(crate) fn corners(r: R) -> Q {
    [[r[0], r[1]], [r[2], r[1]], [r[2], r[3]], [r[0], r[3]]]
}

impl Layer {
    /// The whole source over the whole slide.
    pub fn full(source: Source, alpha: f64) -> Layer {
        Layer::rect(source, FULL, FULL, alpha)
    }
    /// `src` rectangle of the source drawn into the `dst` rectangle.
    pub fn rect(source: Source, src: R, dst: R, alpha: f64) -> Layer {
        Layer { source, src, quad: corners(dst), uv: corners(src), alpha, fade: ones() }
    }
    /// `src` rectangle of the source drawn onto a quad.
    pub fn quad(source: Source, src: R, quad: Q, alpha: f64) -> Layer {
        Layer { source, src, quad, uv: corners(src), alpha, fade: ones() }
    }
    /// The part of the source under `dst` drawn in place.
    pub(crate) fn inplace(source: Source, dst: R, alpha: f64) -> Layer {
        Layer::rect(source, dst, dst, alpha)
    }
    /// A triangle with its own texture coordinates and corner opacities.
    pub(crate) fn tri_uv(source: Source, p: [P; 3], uv: [P; 3], fade: [f64; 3], alpha: f64) -> Layer {
        let xs = [uv[0][0], uv[1][0], uv[2][0]];
        let ys = [uv[0][1], uv[1][1], uv[2][1]];
        let mn = |v: [f64; 3]| v.iter().copied().fold(f64::INFINITY, f64::min);
        let mx = |v: [f64; 3]| v.iter().copied().fold(f64::NEG_INFINITY, f64::max);
        Layer {
            source,
            src: [mn(xs), mn(ys), mx(xs), mx(ys)],
            quad: [p[0], p[1], p[2], p[2]],
            uv: [uv[0], uv[1], uv[2], uv[2]],
            alpha,
            fade: [fade[0], fade[1], fade[2], fade[2]],
        }
    }
    /// With corner opacities.
    pub(crate) fn faded(mut self, fade: [f64; 4]) -> Layer {
        self.fade = fade;
        self
    }
    /// A black copy of this layer's shape at `k` of its opacity: shading a card.
    pub(crate) fn shade(&self, k: f64) -> Layer {
        Layer { source: Source::Black, alpha: self.alpha * k.clamp(0.0, 1.0), ..*self }
    }
    fn sanitized(mut self) -> Layer {
        let f = |v: f64| if v.is_finite() { v.clamp(-1.0e6, 1.0e6) } else { 0.0 };
        for v in self.src.iter_mut() {
            *v = f(*v).clamp(0.0, 1.0);
        }
        for c in self.quad.iter_mut().chain(self.uv.iter_mut()) {
            c[0] = f(c[0]);
            c[1] = f(c[1]);
        }
        for c in self.uv.iter_mut() {
            c[0] = c[0].clamp(0.0, 1.0);
            c[1] = c[1].clamp(0.0, 1.0);
        }
        for a in self.fade.iter_mut() {
            *a = clampf(*a, 0.0, 1.0, 0.0);
        }
        self.alpha = clampf(self.alpha, 0.0, 1.0, 0.0);
        self
    }
}

/// Slide aspect assumed by [`transition_layers`] (16:9).
pub const DEFAULT_ASPECT: f64 = 16.0 / 9.0;

/// Layers of the transition `kind` (a [`deckcraft_model::anim::TRANSITIONS`] id) with `option` at
/// progress `t` (0..1) on a 16:9 slide. See [`transition_frame`].
pub fn transition_layers(kind: &str, option: &str, t: f64) -> Vec<Layer> {
    transition_frame(kind, option, t, DEFAULT_ASPECT)
}

/// Layers of the transition `kind` with `option` at linear progress `t` (0..1; the effect applies
/// its own easing) on a slide `aspect` wide per unit of height. Draw in order over black. At
/// `t >= 1` the result is the new slide alone; at `t <= 0` (or NaN) the old slide alone. Morph
/// returns a plain crossfade (the UI renders morph frames with [`crate::morph_frame`]). Layers
/// with [`Source::is_part`] sources need the slides rendered as background and content
/// ([`uses_slide_parts`]).
pub fn transition_frame(kind: &str, option: &str, t: f64, aspect: f64) -> Vec<Layer> {
    let t = clampf(t, 0.0, 1.0, 0.0);
    let aspect = if aspect.is_finite() && aspect > 0.01 { aspect.min(100.0) } else { DEFAULT_ASPECT };
    let v = if t >= 1.0 {
        vec![Layer::full(Source::New, 1.0)]
    } else if t <= 0.0 {
        vec![Layer::full(Source::Old, 1.0)]
    } else {
        crate::transition_fx::layers(kind, option, t, aspect, 0)
    };
    v.into_iter().map(Layer::sanitized).filter(|l| l.alpha > 0.0 && l.fade.iter().any(|a| *a > 0.0)).collect()
}

/// Does `kind` animate the slides' backgrounds and contents separately (PowerPoint's Dynamic
/// Content transitions and Reveal)? Then the frame uses [`Source::OldBack`] & co.
pub fn uses_slide_parts(kind: &str, option: &str) -> bool {
    match kind {
        "pan" | "ferris" | "conveyor" | "rotate" | "window" | "orbit" | "flythrough" => true,
        "reveal" => !option.starts_with("black"),
        _ => false,
    }
}
