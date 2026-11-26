//! 3-D cameras (`a:scene3d/a:camera`) turning a flat shape, approximated in 2-D.
//!
//! A shape seen through a 3-D camera preset (isometric, perspective…) or rotated with
//! `a:camera/a:rot` is drawn by PowerPoint as its plane turned in space and projected. DeckCraft
//! turns the shape's corners the same way and draws the shape under the affine transform that best
//! fits them: exact for orthographic cameras, within about 2% of the shape's size for the
//! perspective ones (whose foreshortening an affine transform can't show). Depth, bevels' sides and
//! lighting are not drawn.
//!
//! The presets' turns were fitted to PowerPoint's exports (black-box: a picture with coloured
//! corner markers under each preset, all fits within half a point on a 100 × 75 pt shape):
//! points relative to the shape's centre are turned `p = Rz(-rev) · Rx(lat) · Ry(lon) · (x, y, 0)`
//! and, for perspective cameras, scaled by `D / (D - p.z)` with `D = dist · max(w, h)`.

use kurbo::{Affine, Point};

/// (preset, lat, lon, rev in degrees, perspective distance in shape sizes or 0 for orthographic).
const PRESETS: &[(&str, f64, f64, f64, f64)] = &[
    ("orthographicFront", 0.0, 0.0, 0.0, 0.0),
    ("isometricTopUp", 215.52, 134.75, 240.17, 0.0),
    ("isometricTopDown", 324.69, 45.30, 299.88, 0.0),
    ("isometricBottomUp", 215.52, 134.75, 240.17, 0.0),
    ("isometricBottomDown", 215.36, 225.28, 119.86, 0.0),
    ("isometricLeftUp", 324.94, 44.77, 0.03, 0.0),
    ("isometricLeftDown", 144.84, 224.73, 179.95, 0.0),
    ("isometricRightUp", 324.81, 44.75, 359.98, 0.0),
    ("isometricRightDown", 144.84, 224.73, 179.95, 0.0),
    ("isometricOffAxis1Left", 17.94, 64.06, 359.94, 0.0),
    ("isometricOffAxis1Right", 341.80, 25.84, 0.0, 0.0),
    ("isometricOffAxis1Top", 238.56, 127.22, 237.03, 0.0),
    ("isometricOffAxis2Left", 161.88, 205.84, 179.98, 0.0),
    ("isometricOffAxis2Right", 197.91, 244.06, 180.06, 0.0),
    ("isometricOffAxis2Top", 301.42, 52.80, 302.97, 0.0),
    ("isometricOffAxis3Left", 197.95, 244.05, 180.06, 0.0),
    ("isometricOffAxis3Right", 161.88, 205.84, 179.98, 0.0),
    ("isometricOffAxis3Bottom", 301.42, 52.78, 302.98, 0.0),
    ("isometricOffAxis4Left", 198.00, 205.84, 180.05, 0.0),
    ("isometricOffAxis4Right", 17.94, 64.06, 359.94, 0.0),
    ("isometricOffAxis4Bottom", 121.42, 232.75, 237.00, 0.0),
    ("perspectiveLeft", 359.91, 341.75, 0.02, 12.14),
    ("perspectiveRight", 180.27, 198.23, 179.94, 8.02),
    ("perspectiveAbove", 18.92, 0.38, 0.09, 16.61),
    ("perspectiveBelow", 198.83, 180.39, 179.89, 9.31),
    ("perspectiveAboveLeftFacing", 320.78, 346.02, 340.95, 11.54),
    ("perspectiveAboveRightFacing", 219.25, 194.14, 198.98, 9.28),
    ("perspectiveContrastingLeftFacing", 190.42, 136.41, 176.42, 12.21),
    ("perspectiveContrastingRightFacing", 190.52, 223.59, 183.56, 9.53),
    ("perspectiveHeroicLeftFacing", 10.89, 346.28, 2.75, 13.22),
    ("perspectiveHeroicRightFacing", 169.53, 194.00, 177.22, 8.45),
    ("perspectiveHeroicExtremeLeftFacing", 351.98, 325.89, 357.00, 5.98),
    ("perspectiveHeroicExtremeRightFacing", 188.39, 214.22, 182.81, 5.26),
    ("perspectiveRelaxed", 50.02, 0.03, 0.0, 11.32),
    ("perspectiveRelaxedModerately", 34.42, 0.33, 0.16, 11.14),
];

/// Perspective distance for a perspective camera given its own rotation (fitted on
/// `perspectiveFront` with `a:rot`, about 11 shape sizes).
const PERSPECTIVE_DIST: f64 = 11.0;

/// A camera's turn of the shape's plane.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Camera {
    pub lat: f64,
    pub lon: f64,
    pub rev: f64,
    /// Perspective distance in shape sizes; 0 for an orthographic camera.
    pub dist: f64,
}

/// The value of attribute `name` in the first `tag` element of `raw` (XML as kept from the file).
fn attr<'a>(tag: &'a str, name: &str) -> Option<&'a str> {
    let key = format!(" {name}=\"");
    let s = tag.find(&key)? + key.len();
    let rest = tag.get(s..)?;
    rest.get(..rest.find('"')?)
}

/// The camera of 3-D XML kept from the file, when it turns the shape.
pub fn from_raw(raw: &str) -> Option<Camera> {
    let i = raw.find("camera ")?;
    let rest = raw.get(i..)?;
    let open_end = rest.find('>')?;
    let open = rest.get(..open_end)?;
    let prst = attr(open, "prst").unwrap_or("orthographicFront");
    let preset = PRESETS.iter().find(|p| p.0 == prst);
    let perspective = prst.starts_with("perspective");
    // Its own `a:rot` (inside the camera element, not the light rig's).
    let self_closing = open.ends_with('/');
    let own_rot = if self_closing {
        None
    } else {
        let body = rest.get(open_end..rest.find("camera>").unwrap_or(rest.len()))?;
        body.find("rot ").and_then(|j| {
            let t = body.get(j..)?;
            let t = t.get(..t.find('>')?)?;
            let deg = |n: &str| attr(t, n).and_then(|v| v.parse::<f64>().ok()).map(|v| v / 60000.0).unwrap_or(0.0);
            Some((deg("lat"), deg("lon"), deg("rev")))
        })
    };
    let cam = match (own_rot, preset) {
        // PowerPoint turns the other way about the horizontal and vertical axes for `a:rot`.
        (Some((lat, lon, rev)), p) => {
            let dist = match p {
                Some(p) if p.4 > 0.0 => p.4,
                _ if perspective => PERSPECTIVE_DIST,
                _ => 0.0,
            };
            Camera { lat: -lat, lon: -lon, rev, dist }
        }
        (None, Some(p)) => Camera { lat: p.1, lon: p.2, rev: p.3, dist: p.4 },
        (None, None) => return None,
    };
    let flat = [cam.lat, cam.lon, cam.rev].iter().all(|a| (a.rem_euclid(360.0)).min(360.0 - a.rem_euclid(360.0)) < 1e-6);
    (!flat).then_some(cam)
}

/// Where a point of the shape (relative to its centre) lands.
fn project(c: &Camera, p: Point, size: f64) -> Point {
    project3(c, p.x, p.y, 0.0, size)
}

/// Direction `v` (shape space: x right, y down, z to the viewer) turned by camera `c`, without
/// perspective: a surface normal as the camera sees it.
pub fn rotate(c: &Camera, v: [f64; 3]) -> [f64; 3] {
    let (a, b, r) = (c.lat.to_radians(), c.lon.to_radians(), -c.rev.to_radians());
    let (x, y, z) = (v[0], v[1], v[2]);
    let (x1, y1, z1) = (x * b.cos() + z * b.sin(), y, -x * b.sin() + z * b.cos());
    let (x2, y2, z2) = (x1, y1 * a.cos() - z1 * a.sin(), y1 * a.sin() + z1 * a.cos());
    [x2 * r.cos() - y2 * r.sin(), x2 * r.sin() + y2 * r.cos(), z2]
}

/// Where a point in the shape's space (relative to its centre; z towards the viewer, so an
/// extrusion runs to negative z) lands.
pub fn project3(c: &Camera, x: f64, y: f64, z: f64, size: f64) -> Point {
    let p = Point::new(x, y);
    let (a, b, r) = (c.lat.to_radians(), c.lon.to_radians(), -c.rev.to_radians());
    let (x1, y1, z1) = (p.x * b.cos() + z * b.sin(), p.y, -p.x * b.sin() + z * b.cos());
    let (x2, y2, z2) = (x1, y1 * a.cos() - z1 * a.sin(), y1 * a.sin() + z1 * a.cos());
    let (x3, y3) = (x2 * r.cos() - y2 * r.sin(), x2 * r.sin() + y2 * r.cos());
    if c.dist > 0.0 {
        let d = c.dist * size;
        let s = d / (d - z2);
        if s.is_finite() && s > 0.0 {
            return Point::new(x3 * s, y3 * s);
        }
    }
    Point::new(x3, y3)
}

/// The transform (shape space to shape space) that turns a `w` × `h` shape as camera `c` does:
/// the affine map best fitting its turned corners.
pub fn affine(c: &Camera, w: f64, h: f64) -> Option<Affine> {
    if !(w.is_finite() && h.is_finite()) || w <= 0.0 || h <= 0.0 {
        return None;
    }
    let size = w.max(h);
    let src = [(-w / 2.0, -h / 2.0), (w / 2.0, -h / 2.0), (w / 2.0, h / 2.0), (-w / 2.0, h / 2.0)];
    let dst: Vec<Point> = src.iter().map(|(x, y)| project(c, Point::new(*x, *y), size)).collect();
    // Least squares over the four corners. The corners are symmetric about the centre, so the
    // normal equations are diagonal: x' = a x + b y + e with a = Σ x x' / Σ x², and so on.
    let (sxx, syy) = (src.iter().map(|p| p.0 * p.0).sum::<f64>(), src.iter().map(|p| p.1 * p.1).sum::<f64>());
    let dot = |f: &dyn Fn(&(f64, f64), &Point) -> f64| src.iter().zip(&dst).map(|(s, d)| f(s, d)).sum::<f64>();
    let a = dot(&|s, d| s.0 * d.x) / sxx;
    let b = dot(&|s, d| s.1 * d.x) / syy;
    let cc = dot(&|s, d| s.0 * d.y) / sxx;
    let dd = dot(&|s, d| s.1 * d.y) / syy;
    let e = dot(&|_, d| d.x) / 4.0;
    let f = dot(&|_, d| d.y) / 4.0;
    let local = Affine::new([a, cc, b, dd, e, f]);
    let m = Affine::translate((w / 2.0, h / 2.0)) * local * Affine::translate((-w / 2.0, -h / 2.0));
    (m.is_finite() && m.determinant().abs() > 1e-9).then_some(m)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn corners(m: Affine, w: f64, h: f64) -> Vec<Point> {
        [(0.0, 0.0), (w, 0.0), (w, h), (0.0, h)].iter().map(|(x, y)| m * Point::new(*x, *y)).collect()
    }

    #[test]
    fn turning_about_the_vertical_axis_narrows_the_shape() {
        // orthographicFront with lon 60: half as wide, as tall (PowerPoint: 0.506 of the width).
        let c = from_raw(r#"<a:scene3d><a:camera prst="orthographicFront"><a:rot lat="0" lon="3600000" rev="0"/></a:camera><a:lightRig rig="threePt" dir="t"><a:rot lat="0" lon="0" rev="1200000"/></a:lightRig></a:scene3d>"#).unwrap();
        let m = affine(&c, 100.0, 75.0).unwrap();
        let k = corners(m, 100.0, 75.0);
        assert!((k[1].x - k[0].x - 50.0).abs() < 0.01 && (k[3].y - k[0].y - 75.0).abs() < 0.01, "{k:?}");
        assert!(((k[0].x + k[1].x) / 2.0 - 50.0).abs() < 1e-9, "about the centre");
    }

    #[test]
    fn presets_turn_the_shape_as_powerpoint_does() {
        // isometricTopUp, 100 × 75: PowerPoint's corners at (-1.247, 0.127), (0.170, -0.955),
        // (1.237, -0.141), (-0.181, 0.941) of the half sizes from the centre.
        let c = from_raw(r#"<a:scene3d><a:camera prst="isometricTopUp"/><a:lightRig rig="threePt" dir="t"/></a:scene3d>"#).unwrap();
        let k = corners(affine(&c, 100.0, 75.0).unwrap(), 100.0, 75.0);
        for (p, (u, v)) in k.iter().zip([(-1.247, 0.127), (0.170, -0.955), (1.237, -0.141), (-0.181, 0.941)]) {
            let (x, y) = ((p.x - 50.0) / 50.0, (p.y - 37.5) / 37.5);
            assert!((x - u).abs() < 0.03 && (y - v).abs() < 0.03, "({x:.3}, {y:.3}) vs ({u}, {v})");
        }
        // Front-facing cameras and the oblique ones leave a flat shape as it is.
        for raw in [r#"<a:scene3d><a:camera prst="orthographicFront"/></a:scene3d>"#, r#"<a:scene3d><a:camera prst="obliqueTopLeft"/></a:scene3d>"#] {
            assert_eq!(from_raw(raw), None);
        }
        assert_eq!(from_raw("<a:sp3d/>"), None);
    }

    #[test]
    fn a_perspective_camera_is_approximated_closely() {
        // perspectiveContrastingLeftFacing on 100 × 75: PowerPoint's corners within 3% of the size.
        let c = from_raw(r#"<a:scene3d><a:camera prst="perspectiveContrastingLeftFacing"/></a:scene3d>"#).unwrap();
        let k = corners(affine(&c, 100.0, 75.0).unwrap(), 100.0, 75.0);
        for (p, (u, v)) in k.iter().zip([(-0.657, -1.189), (0.791, -0.795), (0.685, 1.244), (-0.742, 0.729)]) {
            let (dx, dy) = (p.x - 50.0 - u * 50.0, p.y - 37.5 - v * 37.5);
            assert!(dx.hypot(dy) < 3.0, "{p:?} vs ({u}, {v})");
        }
    }
}
