//! Motion paths: our SVG-like syntax (`M x y L x y C x1 y1 x2 y2 x y Q x1 y1 x y Z E`, lowercase
//! = relative) in slide-fraction units, relative to the shape's start position. Positions are
//! evaluated by arc length.

use std::fmt::Write as _;

const MAX_TOKENS: usize = 100_000;
const MAX_POINTS: usize = 20_000;
const CURVE_STEPS: usize = 16;

/// A flattened motion path.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct MotionPath {
    /// Polyline vertices (slide fractions).
    pub points: Vec<(f64, f64)>,
    /// Cumulative arc length at each vertex.
    cum: Vec<f64>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum Tok {
    Cmd(char),
    Num(f64),
}

fn tokenize(s: &str) -> Vec<Tok> {
    let cs: Vec<char> = s.chars().take(1_000_000).collect();
    let mut out = Vec::new();
    let mut i = 0;
    while i < cs.len() && out.len() < MAX_TOKENS {
        let c = cs.get(i).copied().unwrap_or(' ');
        if c.is_whitespace() || c == ',' {
            i += 1;
            continue;
        }
        if c.is_ascii_digit() || c == '.' || c == '-' || c == '+' {
            let start = i;
            i += 1;
            let mut seen_dot = c == '.';
            while let Some(&d) = cs.get(i) {
                if d.is_ascii_digit() {
                    i += 1;
                } else if d == '.' && !seen_dot {
                    seen_dot = true;
                    i += 1;
                } else if (d == 'e' || d == 'E') && {
                    let n1 = cs.get(i + 1).copied().unwrap_or(' ');
                    let n2 = cs.get(i + 2).copied().unwrap_or(' ');
                    n1.is_ascii_digit() || ((n1 == '-' || n1 == '+') && n2.is_ascii_digit())
                } {
                    i += 2;
                    while cs.get(i).is_some_and(char::is_ascii_digit) {
                        i += 1;
                    }
                    break;
                } else {
                    break;
                }
            }
            let text: String = cs.get(start..i).map(|s| s.iter().collect()).unwrap_or_default();
            match text.parse::<f64>() {
                Ok(v) if v.is_finite() => out.push(Tok::Num(v)),
                _ => out.push(Tok::Cmd('?')),
            }
            continue;
        }
        out.push(Tok::Cmd(c));
        i += 1;
    }
    out
}

impl MotionPath {
    /// Parse a path. Malformed input yields the part parsed before the error (possibly empty).
    pub fn parse(s: &str) -> MotionPath {
        let toks = tokenize(s);
        let mut pts: Vec<(f64, f64)> = Vec::new();
        let mut cur = (0.0, 0.0);
        let mut start = (0.0, 0.0);
        let mut cmd = 'M';
        let mut i = 0;
        let push = |pts: &mut Vec<(f64, f64)>, p: (f64, f64)| {
            if pts.len() < MAX_POINTS {
                pts.push(p);
            }
        };
        let num = |i: usize| match toks.get(i) {
            Some(Tok::Num(v)) => Some(*v),
            _ => None,
        };
        while i < toks.len() {
            if let Some(Tok::Cmd(c)) = toks.get(i) {
                cmd = *c;
                i += 1;
                match cmd {
                    'Z' | 'z' => {
                        push(&mut pts, start);
                        cur = start;
                        continue;
                    }
                    'E' | 'e' => break,
                    'M' | 'm' | 'L' | 'l' | 'C' | 'c' | 'Q' | 'q' | 'H' | 'h' | 'V' | 'v' => continue,
                    _ => break,
                }
            }
            let rel = cmd.is_ascii_lowercase();
            let base = if rel { cur } else { (0.0, 0.0) };
            let need = match cmd.to_ascii_uppercase() {
                'M' | 'L' => 2,
                'C' => 6,
                'Q' => 4,
                'H' | 'V' => 1,
                _ => break,
            };
            let mut v = [0.0f64; 6];
            let mut ok = true;
            for (k, slot) in v.iter_mut().enumerate().take(need) {
                match num(i + k) {
                    Some(x) => *slot = x,
                    None => ok = false,
                }
            }
            if !ok {
                break;
            }
            i += need;
            match cmd.to_ascii_uppercase() {
                'M' => {
                    cur = (base.0 + v[0], base.1 + v[1]);
                    start = cur;
                    // A move inside a motion path is a jump; it stays continuous as a segment.
                    push(&mut pts, cur);
                    cmd = if rel { 'l' } else { 'L' };
                }
                'L' => {
                    if pts.is_empty() {
                        push(&mut pts, cur);
                    }
                    cur = (base.0 + v[0], base.1 + v[1]);
                    push(&mut pts, cur);
                }
                'H' | 'V' => {
                    if pts.is_empty() {
                        push(&mut pts, cur);
                    }
                    cur = if cmd.eq_ignore_ascii_case(&'H') { (base.0 + v[0], cur.1) } else { (cur.0, base.1 + v[0]) };
                    push(&mut pts, cur);
                }
                'C' => {
                    if pts.is_empty() {
                        push(&mut pts, cur);
                    }
                    let p0 = cur;
                    let p1 = (base.0 + v[0], base.1 + v[1]);
                    let p2 = (base.0 + v[2], base.1 + v[3]);
                    let p3 = (base.0 + v[4], base.1 + v[5]);
                    for k in 1..=CURVE_STEPS {
                        let t = k as f64 / CURVE_STEPS as f64;
                        let u = 1.0 - t;
                        let a = u * u * u;
                        let b = 3.0 * u * u * t;
                        let c = 3.0 * u * t * t;
                        let d = t * t * t;
                        push(&mut pts, (a * p0.0 + b * p1.0 + c * p2.0 + d * p3.0, a * p0.1 + b * p1.1 + c * p2.1 + d * p3.1));
                    }
                    cur = p3;
                }
                'Q' => {
                    if pts.is_empty() {
                        push(&mut pts, cur);
                    }
                    let p0 = cur;
                    let p1 = (base.0 + v[0], base.1 + v[1]);
                    let p2 = (base.0 + v[2], base.1 + v[3]);
                    for k in 1..=CURVE_STEPS {
                        let t = k as f64 / CURVE_STEPS as f64;
                        let u = 1.0 - t;
                        push(&mut pts, (u * u * p0.0 + 2.0 * u * t * p1.0 + t * t * p2.0, u * u * p0.1 + 2.0 * u * t * p1.1 + t * t * p2.1));
                    }
                    cur = p2;
                }
                _ => break,
            }
        }
        pts.retain(|p| p.0.is_finite() && p.1.is_finite());
        let mut cum = Vec::with_capacity(pts.len());
        let mut acc = 0.0;
        let mut prev: Option<(f64, f64)> = None;
        for p in &pts {
            if let Some(q) = prev {
                let d = ((p.0 - q.0).powi(2) + (p.1 - q.1).powi(2)).sqrt();
                if d.is_finite() {
                    acc += d;
                }
            }
            cum.push(acc);
            prev = Some(*p);
        }
        MotionPath { points: pts, cum }
    }

    /// Total arc length (slide fractions).
    pub fn length(&self) -> f64 {
        self.cum.last().copied().unwrap_or(0.0)
    }

    /// Position at arc-length fraction `p` (0..1), relative to the path's origin (0, 0).
    pub fn at(&self, p: f64) -> (f64, f64) {
        let first = self.points.first().copied().unwrap_or((0.0, 0.0));
        let len = self.length();
        if len.is_nan() || len <= 0.0 || !len.is_finite() {
            return first;
        }
        let p = crate::clampf(p, 0.0, 1.0, 0.0);
        let target = p * len;
        let i = self.cum.partition_point(|c| *c < target);
        if i == 0 {
            return first;
        }
        let (Some(a), Some(b), Some(ca), Some(cb)) = (self.points.get(i - 1), self.points.get(i), self.cum.get(i - 1), self.cum.get(i)) else {
            return self.points.last().copied().unwrap_or(first);
        };
        let seg = cb - ca;
        let f = if seg > 0.0 { (target - ca) / seg } else { 0.0 };
        (a.0 + (b.0 - a.0) * f, a.1 + (b.1 - a.1) * f)
    }
}

fn polygon(n: usize, r: f64, rot0: f64) -> String {
    // Regular polygon through (0, 0), centred below it.
    let mut s = String::from("M 0 0");
    let c = (0.0, r);
    let a0 = -std::f64::consts::FRAC_PI_2 + rot0;
    let start = (c.0 + r * a0.cos(), c.1 + r * a0.sin());
    for k in 1..=n {
        let a = a0 + std::f64::consts::TAU * k as f64 / n as f64;
        let _ = write!(s, " L {:.4} {:.4}", c.0 + r * a.cos() - start.0, c.1 + r * a.sin() - start.1);
    }
    s.push_str(" E");
    s
}

/// The path a preset motion-path effect uses when the animation has none of its own.
pub fn default_path(effect: &str, option: &str) -> String {
    let (sx, sy, swap) = match option {
        "up" => (1.0, -1.0, false),
        "right" => (1.0, 1.0, true),
        "left" => (-1.0, 1.0, true),
        _ => (1.0, 1.0, false),
    };
    // Base paths go down; `swap` turns them to the horizontal.
    let tr = |pts: &[(f64, f64)]| -> Vec<(f64, f64)> { pts.iter().map(|&(x, y)| if swap { (y * sx, x * sy) } else { (x * sx, y * sy) }).collect() };
    let fmt = |head: &str, pts: Vec<(f64, f64)>| {
        let mut s = String::from(head);
        for (x, y) in pts {
            let _ = write!(s, " {x:.4} {y:.4}");
        }
        s.push_str(" E");
        s
    };
    match effect {
        "lines" => fmt("M 0 0 L", tr(&[(0.0, 0.25)])),
        "arcs" => fmt("M 0 0 C", tr(&[(0.1, 0.05), (0.1, 0.2), (0.0, 0.25)])),
        "turns" => {
            let p = tr(&[(0.0, 0.12), (0.0, 0.2), (0.05, 0.25), (0.12, 0.25)]);
            let mut s = String::from("M 0 0");
            if let Some((x, y)) = p.first() {
                let _ = write!(s, " L {x:.4} {y:.4} C");
            }
            for (x, y) in p.iter().skip(1) {
                let _ = write!(s, " {x:.4} {y:.4}");
            }
            s.push_str(" E");
            s
        }
        "shapes" => match option {
            "diamond" => polygon(4, 0.1, 0.0),
            "hexagon" => polygon(6, 0.1, 0.0),
            "triangle" => polygon(3, 0.1, 0.0),
            "square" => polygon(4, 0.1, std::f64::consts::FRAC_PI_4),
            "octagon" => polygon(8, 0.1, 0.0),
            "pentagon" => polygon(5, 0.1, 0.0),
            "trapezoid" => "M 0 0 L 0.1 0 L 0.15 0.15 L -0.05 0.15 Z E".into(),
            "parallelogram" => "M 0 0 L 0.15 0 L 0.1 0.15 L -0.05 0.15 Z E".into(),
            _ => {
                "M 0 0 C 0.0552 0 0.1 0.0448 0.1 0.1 C 0.1 0.1552 0.0552 0.2 0 0.2 C -0.0552 0.2 -0.1 0.1552 -0.1 0.1 C -0.1 0.0448 -0.0552 0 0 0 Z E"
                    .into()
            }
        },
        "loops" => "M 0 0 C 0.08 -0.1 0.16 0 0.08 0.05 C 0 0.1 -0.08 0.1 -0.08 0 C -0.08 -0.1 0 -0.05 0 0 E".into(),
        other => deckcraft_model::anim::preset_path(other).unwrap_or_default(),
    }
}

#[cfg(test)]
mod preset_path_tests {
    use super::*;

    #[test]
    fn every_extra_motion_path_preset_starts_at_its_shape_and_moves() {
        use deckcraft_model::anim::{ANIMATIONS, AnimClass, preset_path};
        let extra: Vec<_> = ANIMATIONS.iter().filter(|a| a.2 == AnimClass::Path && preset_path(a.0).is_some()).collect();
        assert!(extra.len() >= 30);
        for a in extra {
            assert_eq!(default_path(a.0, ""), preset_path(a.0).unwrap_or_default());
            let mp = MotionPath::parse(&default_path(a.0, ""));
            let (x, y) = mp.at(0.0);
            assert!(x.abs() < 1e-9 && y.abs() < 1e-9, "{} starts at its shape", a.0);
            let far = mp.points.iter().map(|p| p.0.abs() + p.1.abs()).fold(0.0, f64::max);
            assert!(far > 0.05, "{} moves", a.0);
            assert!(mp.points.iter().all(|p| p.0.abs() < 0.5 && p.1.abs() < 0.5), "{} stays near its shape", a.0);
        }
    }
}
