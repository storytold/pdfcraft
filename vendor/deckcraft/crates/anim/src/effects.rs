//! Evaluation of one animation effect at a progress value.

use std::f64::consts::PI;

use deckcraft_model::{AnimClass, Rgba, Xfrm};

use crate::AnimState;
use crate::easing::{bounce_out, ease, smooth};
use crate::path::{MotionPath, default_path};

/// Everything an effect needs besides its progress.
#[derive(Clone, Debug)]
pub(crate) struct EffectSpec {
    pub class: AnimClass,
    /// Base effect id (exit ids mapped to their entrance counterpart).
    pub effect: String,
    pub option: String,
    pub accel: f64,
    pub decel: f64,
    pub bounce: f64,
    pub amount: Option<f64>,
    pub color: Option<Rgba>,
    pub path: Option<MotionPath>,
    /// Shape box in points (sanitized).
    pub xf: Xfrm,
    pub slide_w: f64,
    pub slide_h: f64,
}

/// Map an exit effect id to the entrance effect it mirrors.
pub(crate) fn exit_base(effect: &str) -> &str {
    match effect {
        "disappear" => "appear",
        "fadeOut" => "fade",
        "flyOut" => "fly",
        "floatOut" => "float",
        "splitOut" => "split",
        "wipeOut" => "wipe",
        "shapeOut" => "shape",
        "wheelOut" => "wheel",
        "randomBarsOut" => "randomBars",
        "shrinkTurn" => "grow",
        "zoomOut" => "zoom",
        "swivelOut" => "swivel",
        "bounceOut" => "bounce",
        "dissolveOut" => "dissolve",
        "blindsOut" => "blinds",
        "boxOut" => "box",
        "checkerboardOut" => "checkerboard",
        "diamondOut" => "diamond",
        "peekOut" => "peek",
        "stripsOut" => "strips",
        "sinkDown" => "rise",
        "spinnerOut" => "spinner",
        "contract" => "expand",
        other => other,
    }
}

/// Emphasis effects whose end state persists after they finish (PowerPoint keeps the spun angle,
/// the new size, the new colour and transparency).
pub(crate) fn emphasis_persists(effect: &str) -> bool {
    matches!(
        effect,
        "spin"
            | "growShrink"
            | "desaturate"
            | "darken"
            | "lighten"
            | "transparency"
            | "objectColor"
            | "complementaryColor"
            | "lineColor"
            | "fillColor"
            | "fontColor"
    )
}

fn uses_default_smoothing(effect: &str) -> bool {
    matches!(effect, "fly" | "float" | "rise" | "peek" | "lines" | "arcs" | "turns" | "shapes" | "loops" | "customPath")
        || deckcraft_model::anim::preset_path(effect).is_some()
}

impl EffectSpec {
    /// Eased progress.
    pub fn eased(&self, p: f64) -> f64 {
        if self.accel <= 0.0 && self.decel <= 0.0 && self.bounce <= 0.0 && uses_default_smoothing(&self.effect) {
            smooth(p)
        } else {
            ease(p, self.accel, self.decel, self.bounce)
        }
    }

    /// The state delta at linear progress `p` (0..1) while the effect plays. `cur` is the shape's
    /// current fill colour (for colour emphasis).
    pub fn eval(&self, p: f64, cur: Rgba) -> AnimState {
        let p = crate::clampf(p, 0.0, 1.0, 0.0);
        let s = match self.class {
            AnimClass::Entrance => self.entrance(p, false),
            AnimClass::Exit => self.entrance(1.0 - p, true),
            AnimClass::Emphasis => self.emphasis(p, cur),
            AnimClass::Path => self.motion(p),
            AnimClass::Media => AnimState::default(),
        };
        s.sanitized()
    }

    /// The state the effect leaves behind once done.
    pub fn end_state(&self, cur: Rgba) -> AnimState {
        match self.class {
            AnimClass::Entrance | AnimClass::Media => AnimState::default(),
            AnimClass::Exit => AnimState::hidden(),
            AnimClass::Emphasis if emphasis_persists(&self.effect) => self.emphasis(1.0, cur).sanitized(),
            AnimClass::Emphasis => AnimState::default(),
            AnimClass::Path => self.motion(1.0).sanitized(),
        }
    }

    fn entrance(&self, p: f64, exit: bool) -> AnimState {
        let e = self.eased(p);
        let q = 1.0 - e;
        let o = self.option.as_str();
        let Xfrm { x, y, w, h, .. } = self.xf;
        let (sw, sh) = (self.slide_w, self.slide_h);
        let mut st = AnimState::default();
        let center_clip = |f: f64, horz: bool, vert: bool| {
            let f = f.clamp(0.0, 1.0) / 2.0;
            let (l, r) = if horz { (0.5 - f, 0.5 + f) } else { (0.0, 1.0) };
            let (t, b) = if vert { (0.5 - f, 0.5 + f) } else { (0.0, 1.0) };
            Some([l, t, r, b])
        };
        match self.effect.as_str() {
            "appear" => st.visible = p > 0.0,
            "fade" | "dissolve" | "randomBars" | "blinds" | "checkerboard" | "wheel" => st.opacity = e,
            "fly" => {
                let o = if o.is_empty() { "b" } else { o };
                if o.contains('l') {
                    st.offset_x = -(x + w) * q;
                }
                if o.contains('r') {
                    st.offset_x = (sw - x) * q;
                }
                if o.contains('t') {
                    st.offset_y = -(y + h) * q;
                }
                if o.contains('b') {
                    st.offset_y = (sh - y) * q;
                }
            }
            "float" | "rise" => {
                // Float Up enters from below and leaves upward; Float Down the reverse.
                let down = o == "d";
                let dir = if down { -1.0 } else { 1.0 };
                let dir = if exit { -dir } else { dir };
                st.offset_y = dir * 0.1 * sh * q;
                st.opacity = e;
            }
            "split" => {
                let horz = o.starts_with("horz");
                // "In" closes from the edges: approximated with the centre reveal plus a fade.
                st.clip = if horz { center_clip(e, false, true) } else { center_clip(e, true, false) };
                if o.ends_with("In") {
                    st.opacity = e;
                }
            }
            "wipe" => {
                st.clip = Some(match o {
                    "l" => [0.0, 0.0, e, 1.0],
                    "t" => [0.0, 0.0, 1.0, e],
                    "r" => [1.0 - e, 0.0, 1.0, 1.0],
                    _ => [0.0, 1.0 - e, 1.0, 1.0],
                });
            }
            "shape" | "circle" | "box" | "diamond" | "expandClip" => {
                let round = matches!(self.effect.as_str(), "circle" | "diamond")
                    || o.starts_with("circle")
                    || o.starts_with("diamond")
                    || o.starts_with("plus");
                let f = if round { (e * std::f64::consts::SQRT_2).min(1.0) } else { e };
                st.clip = center_clip(f, true, true);
                if o.ends_with("In") || o == "in" {
                    st.opacity = e;
                }
            }
            "grow" => {
                st.scale_x = e;
                st.scale_y = e;
                st.rotate = -90.0 * q;
                st.opacity = e;
            }
            "zoom" => {
                let s = if o == "out" { 1.0 + 3.0 * q } else { e };
                st.scale_x = s;
                st.scale_y = s;
                st.opacity = e;
            }
            "swivel" => {
                let s = e * (q * 2.5 * PI).cos();
                if o == "vert" {
                    st.scale_y = s;
                } else {
                    st.scale_x = s;
                }
                st.opacity = e.min(1.0);
            }
            "bounce" => {
                let b = bounce_out(p);
                st.offset_y = -(0.25 * sh) * (1.0 - b);
                st.offset_x = -0.1 * sw * (1.0 - p);
                st.opacity = (p * 4.0).min(1.0);
            }
            "peek" => match o {
                "l" => {
                    st.offset_x = -w * q;
                    st.clip = Some([q, 0.0, 1.0, 1.0]);
                }
                "r" => {
                    st.offset_x = w * q;
                    st.clip = Some([0.0, 0.0, e, 1.0]);
                }
                "t" => {
                    st.offset_y = -h * q;
                    st.clip = Some([0.0, q, 1.0, 1.0]);
                }
                _ => {
                    st.offset_y = h * q;
                    st.clip = Some([0.0, 0.0, 1.0, e]);
                }
            },
            "strips" => {
                st.clip = Some(match o {
                    "ru" => [0.0, q, e, 1.0],
                    "ld" => [q, 0.0, 1.0, e],
                    "rd" => [0.0, 0.0, e, e],
                    _ => [q, q, 1.0, 1.0],
                });
            }
            "expand" => {
                st.scale_x = 0.3 + 0.7 * e;
                st.opacity = e;
            }
            "spinner" => {
                st.rotate = 360.0 * q;
                st.scale_x = 0.5 + 0.5 * e;
                st.scale_y = 0.5 + 0.5 * e;
                st.opacity = e;
            }
            _ => st.opacity = e,
        }
        st
    }

    fn emphasis(&self, p: f64, cur: Rgba) -> AnimState {
        let e = self.eased(p);
        let bump = (p * PI).sin();
        let mut st = AnimState::default();
        let target = self.color.unwrap_or(Rgba::rgb(0xC0, 0x00, 0x00));
        match self.effect.as_str() {
            "pulse" => {
                st.scale_x = 1.0 + 0.1 * bump;
                st.scale_y = st.scale_x;
            }
            "colorPulse" => st.tint = Some(cur.lerp(target, bump)),
            "teeter" => st.rotate = 8.0 * (p * 4.0 * PI).sin() * (1.0 - p * 0.5),
            "spin" => {
                let amt = self.amount.filter(|a| a.is_finite()).unwrap_or(360.0).clamp(-36_000.0, 36_000.0);
                let sign = if self.option == "counterClockwise" { -1.0 } else { 1.0 };
                st.rotate = sign * amt * e;
            }
            "growShrink" => {
                let amt = self.amount.filter(|a| a.is_finite() && *a > 0.0).unwrap_or(1.5).clamp(0.0, 100.0);
                let s = 1.0 + (amt - 1.0) * e;
                match self.option.as_str() {
                    "horz" => st.scale_x = s,
                    "vert" => st.scale_y = s,
                    _ => {
                        st.scale_x = s;
                        st.scale_y = s;
                    }
                }
            }
            "desaturate" => {
                let l = (0.299 * cur.r as f64 + 0.587 * cur.g as f64 + 0.114 * cur.b as f64).round().clamp(0.0, 255.0) as u8;
                st.tint = Some(cur.lerp(Rgba::rgba(l, l, l, cur.a), e));
            }
            "darken" => st.tint = Some(cur.lerp(Rgba::rgba(0, 0, 0, cur.a), 0.4 * e)),
            "lighten" => st.tint = Some(cur.lerp(Rgba::rgba(255, 255, 255, cur.a), 0.4 * e)),
            "transparency" => {
                // `amount` is the transparency (0.5 = 50 %).
                let a = self.amount.filter(|a| a.is_finite()).unwrap_or(0.5).clamp(0.0, 1.0);
                st.opacity = 1.0 - a * e;
            }
            "objectColor" | "fillColor" | "lineColor" | "fontColor" => st.tint = Some(cur.lerp(target, e)),
            "complementaryColor" => st.tint = Some(cur.lerp(Rgba::rgba(255 - cur.r, 255 - cur.g, 255 - cur.b, cur.a), e)),
            "underline" | "boldFlash" => {
                st.scale_x = 1.0 + 0.04 * bump;
                st.scale_y = st.scale_x;
                if self.effect == "boldFlash" {
                    st.tint = Some(cur.lerp(Rgba::rgba(0, 0, 0, cur.a), 0.3 * bump));
                }
            }
            "wave" => {
                st.offset_y = -0.02 * self.slide_h * bump;
                st.scale_y = 1.0 + 0.05 * bump;
            }
            _ => {}
        }
        st
    }

    fn motion(&self, p: f64) -> AnimState {
        let e = self.eased(p);
        let (fx, fy) = match &self.path {
            Some(mp) => mp.at(e),
            None => (0.0, 0.0),
        };
        AnimState { offset_x: fx * self.slide_w, offset_y: fy * self.slide_h, ..Default::default() }
    }
}

/// The path for a motion-path animation: its own, or the preset default.
pub(crate) fn motion_path(effect: &str, option: &str, own: Option<&str>) -> MotionPath {
    match own {
        Some(s) if !s.trim().is_empty() => MotionPath::parse(s),
        _ => MotionPath::parse(&default_path(effect, option)),
    }
}
