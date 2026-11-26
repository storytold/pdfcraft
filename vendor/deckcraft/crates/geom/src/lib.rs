//! DeckCraft geometry: units, shape transforms and preset shape generators.
//!
//! The document model keeps positions in points (1/72 inch) as `f64`. Office Open XML stores EMU
//! (English Metric Units, 12 700 per point); [`emu_to_pt`] and [`pt_to_emu`] convert losslessly for
//! integer EMU values, so files round-trip exactly.
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]
#![forbid(unsafe_code)]

pub mod morph;
pub mod squircle;

pub use kurbo::{Affine, BezPath, PathEl, Point, Rect, Shape as KurboShape, Size, Vec2};
// Placement, units and preset and warp geometry are DrawingML's, shared in craft-libs
// (`craft_drawingml`, craft-libs#778 and #792).
pub use craft_drawingml::{EMU_PER_CM, EMU_PER_IN, EMU_PER_PT, Xfrm, emu_to_pt, group_child_affine, preset, pt_to_emu, warp};
use serde::{Deserialize, Serialize};

/// Display units for measurements (inches on US English systems, centimetres elsewhere).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Unit {
    #[default]
    Inches,
    Centimeters,
    Points,
    Pixels,
}

impl Unit {
    pub fn points_per_unit(self) -> f64 {
        match self {
            Unit::Inches => 72.0,
            Unit::Centimeters => 72.0 / 2.54,
            Unit::Points => 1.0,
            Unit::Pixels => 0.75,
        }
    }
    pub fn suffix(self) -> &'static str {
        match self {
            Unit::Inches => "\"",
            Unit::Centimeters => " cm",
            Unit::Points => " pt",
            Unit::Pixels => " px",
        }
    }
    /// `2.08"` style text for a length in points.
    pub fn format(self, pt: f64) -> String {
        let v = pt / self.points_per_unit();
        let mut s = format!("{v:.2}");
        if s.starts_with('-') && s[1..].bytes().all(|b| b == b'0' || b == b'.') {
            s.remove(0);
        }
        format!("{s}{}", self.suffix())
    }
    /// Parses `2.5`, `2.5"`, `2.5 in`, `6 cm`, `30 pt`, `40px`, `5mm` into points. A bare number uses `self`.
    pub fn parse(self, text: &str) -> Option<f64> {
        let t = text.trim().to_ascii_lowercase();
        let (num, unit) = split_number(&t)?;
        let ppu = match unit.trim() {
            "" => self.points_per_unit(),
            "\"" | "in" | "inch" | "inches" => 72.0,
            "cm" => 72.0 / 2.54,
            "mm" => 72.0 / 25.4,
            "pt" | "pts" => 1.0,
            "px" => 0.75,
            _ => return None,
        };
        let v = num * ppu;
        v.is_finite().then_some(v)
    }
}

fn split_number(t: &str) -> Option<(f64, &str)> {
    let end = t.char_indices().find(|(_, c)| !(c.is_ascii_digit() || *c == '.' || *c == '-' || *c == '+')).map(|(i, _)| i).unwrap_or(t.len());
    let n: f64 = t.get(..end)?.parse().ok()?;
    Some((n, t.get(end..).unwrap_or("")))
}

/// Distance from `p` to the segment `a`–`b`.
pub fn dist_to_segment(p: Point, a: Point, b: Point) -> f64 {
    let ab = b - a;
    let len2 = ab.hypot2();
    if len2 < 1e-12 {
        return (p - a).hypot();
    }
    let t = ((p - a).dot(ab) / len2).clamp(0.0, 1.0);
    (p - (a + ab * t)).hypot()
}

/// Is `p` within `tol` of the outline of `path` (flattened)?
pub fn near_path(path: &BezPath, p: Point, tol: f64) -> bool {
    let mut hit = false;
    let mut start = Point::ZERO;
    let mut last = Point::ZERO;
    kurbo::flatten(path.iter(), 0.25, |el| {
        if hit {
            return;
        }
        match el {
            PathEl::MoveTo(q) => {
                start = q;
                last = q;
            }
            PathEl::LineTo(q) => {
                if dist_to_segment(p, last, q) <= tol {
                    hit = true;
                }
                last = q;
            }
            PathEl::ClosePath => {
                if dist_to_segment(p, last, start) <= tol {
                    hit = true;
                }
                last = start;
            }
            _ => {}
        }
    });
    hit
}

/// Snap `v` to the nearest multiple of `step` (step <= 0 returns `v`).
pub fn snap(v: f64, step: f64) -> f64 {
    if step > 0.0 && step.is_finite() { (v / step).round() * step } else { v }
}

/// Flatten a path to move/line/close elements (tolerance 0.2).
pub fn preset_flatten(path: &BezPath, f: &mut dyn FnMut(PathEl)) {
    kurbo::flatten(path.iter(), 0.2, f);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn format_has_no_negative_zero() {
        assert_eq!(Unit::Points.format(-0.001), "0.00 pt");
        assert_eq!(Unit::Inches.format(-0.001), "0.00\"");
        assert!(!Unit::Centimeters.format(-0.01).starts_with('-'));
        assert_eq!(Unit::Points.format(-1.5), "-1.50 pt");
    }

    #[test]
    fn emu_roundtrip_is_exact() {
        for emu in [0i64, 1, 12_699, 12_700, 914_400, 12_192_000, 6_858_000, -5, 123_456_789] {
            assert_eq!(pt_to_emu(emu_to_pt(emu)), emu);
        }
        assert_eq!(pt_to_emu(f64::NAN), 0);
        assert_eq!(pt_to_emu(f64::INFINITY), 0);
    }

    #[test]
    fn unit_parse_and_format() {
        assert_eq!(Unit::Inches.parse("2"), Some(144.0));
        assert_eq!(Unit::Inches.parse("2\""), Some(144.0));
        assert!((Unit::Inches.parse("2.54 cm").unwrap() - 72.0).abs() < 1e-9);
        assert_eq!(Unit::Inches.parse("36pt"), Some(36.0));
        assert_eq!(Unit::Inches.parse("abc"), None);
        assert_eq!(Unit::Inches.parse(""), None);
        assert_eq!(Unit::Inches.format(150.0), "2.08\"");
    }

    #[test]
    fn xfrm_affine_rotation_about_center() {
        let x = Xfrm { x: 10.0, y: 20.0, w: 100.0, h: 50.0, rot: 90.0, ..Default::default() };
        let c = x.affine() * Point::new(50.0, 25.0);
        assert!((c.x - 60.0).abs() < 1e-9 && (c.y - 45.0).abs() < 1e-9);
        let b = x.bounds();
        assert!((b.width() - 50.0).abs() < 1e-9 && (b.height() - 100.0).abs() < 1e-9);
    }

    #[test]
    fn flips_mirror_local_space() {
        let x = Xfrm { x: 0.0, y: 0.0, w: 100.0, h: 50.0, flip_h: true, ..Default::default() };
        let p = x.affine() * Point::new(0.0, 0.0);
        assert!((p.x - 100.0).abs() < 1e-9 && p.y.abs() < 1e-9);
    }

    #[test]
    fn group_child_mapping() {
        let g = Xfrm::new(100.0, 100.0, 200.0, 100.0);
        let a = group_child_affine(&g, Rect::new(0.0, 0.0, 100.0, 50.0));
        let p = a * Point::new(100.0, 50.0);
        assert!((p.x - 300.0).abs() < 1e-9 && (p.y - 200.0).abs() < 1e-9);
    }

    #[test]
    fn rot_norm_stays_below_360() {
        for rot in [-1e-15, -1e-13, -f64::EPSILON, -0.0, 360.0, 720.0, -360.0] {
            let n = Xfrm { rot, ..Default::default() }.rot_norm();
            assert!((0.0..360.0).contains(&n), "rot {rot} -> {n}");
        }
    }

    #[test]
    fn segment_distance() {
        assert!((dist_to_segment(Point::new(5.0, 5.0), Point::new(0.0, 0.0), Point::new(10.0, 0.0)) - 5.0).abs() < 1e-9);
        assert!((dist_to_segment(Point::new(0.0, 3.0), Point::new(0.0, 0.0), Point::new(0.0, 0.0)) - 3.0).abs() < 1e-9);
    }
}
