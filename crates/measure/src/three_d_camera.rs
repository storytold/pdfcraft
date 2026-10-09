//! PDF camera coordinates face the positive Z axis (ISO 32000-1 section 13.6.5).
use super::three_d::{Artwork, Point, Scene, View, check, cross, direction, dot, length, number, subtract};
use crate::{Result, invalid};
use pdfcraft_cos::{Document, Object};
use serde::Serialize;
#[derive(Clone, Copy, Debug, PartialEq, Serialize)]
pub enum Binding {
    Width,
    Height,
    Minimum,
    Maximum,
    Absolute,
    Number(f64),
}
impl Binding {
    fn value(self, target: [f64; 2]) -> f64 {
        match self {
            Self::Width => target[0],
            Self::Height => target[1],
            Self::Minimum => target[0].min(target[1]),
            Self::Maximum => target[0].max(target[1]),
            Self::Absolute => 1.,
            Self::Number(n) => n,
        }
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Serialize)]
pub enum Projection {
    Perspective { field_of_view: f64, binding: Binding },
    Orthographic { scale: f64, binding: Binding },
}
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Camera {
    /// Camera-to-world columns: right, up, forward and eye position.
    pub matrix: [f64; 12],
    pub center: Point,
    pub target: [f64; 2],
    pub projection: Projection,
    pub near: f64,
    pub far: Option<f64>,
}
#[derive(Clone, Copy, Debug, PartialEq, Serialize)]
pub struct CaptionFrame {
    pub origin: [f64; 2],
    pub x: [f64; 2],
    pub up: [f64; 2],
}
impl CaptionFrame {
    pub fn point(&self, right: f64, up: f64) -> [f64; 2] {
        std::array::from_fn(|i| self.origin[i] + self.x[i] * right + self.up[i] * up)
    }
}
fn binding(value: Option<&Object>, default: Binding) -> Result<Binding> {
    match value {
        None => Ok(default),
        Some(Object::Name(name)) => match name.as_slice() {
            b"W" => Ok(Binding::Width),
            b"H" => Ok(Binding::Height),
            b"Min" => Ok(Binding::Minimum),
            b"Max" => Ok(Binding::Maximum),
            b"Absolute" => Ok(Binding::Absolute),
            _ => Err(invalid("invalid 3D projection binding")),
        },
        Some(value) => value
            .as_f64()
            .filter(|v| v.is_finite() && *v > 0. && *v <= 1e12)
            .map(Binding::Number)
            .ok_or_else(|| invalid("invalid 3D projection binding scale")),
    }
}
impl Camera {
    pub fn fit(scene: &Scene, target: [f64; 2]) -> Result<Self> {
        let [min, max] = scene.bounds().map_err(|e| invalid(&e.to_string()))?.ok_or_else(|| invalid("3D model has no visible positions"))?;
        let center = [(min[0] + max[0]) / 2., (min[1] + max[1]) / 2., (min[2] + max[2]) / 2.];
        let radius = (length(subtract(max, min)) / 2.).max(1e-6);
        let forward = direction([-0.5, -0.3, -1.])?;
        let right = direction(cross(forward, [0., 1., 0.]))?;
        let up = direction(cross(right, forward))?;
        let distance = radius / (22.5_f64.to_radians().sin()) * 1.2;
        let eye = [center[0] - forward[0] * distance, center[1] - forward[1] * distance, center[2] - forward[2] * distance];
        let camera = Self {
            matrix: [right[0], right[1], right[2], up[0], up[1], up[2], forward[0], forward[1], forward[2], eye[0], eye[1], eye[2]],
            center,
            target,
            projection: Projection::Perspective { field_of_view: 45., binding: Binding::Minimum },
            near: radius * 1e-5,
            far: None,
        };
        camera.validate()?;
        Ok(camera)
    }
    pub fn from_artwork(doc: &Document, artwork: &Artwork, view: Option<&View>) -> Result<Self> {
        let target = [artwork.rect[2] - artwork.rect[0], artwork.rect[3] - artwork.rect[1]];
        let mut camera = Self::fit(&artwork.scene, target)?;
        let Some(view) = view.or(artwork.default_view.as_ref()) else { return Ok(camera) };
        if let Some(matrix) = view.camera_to_world {
            camera.matrix = matrix;
            let distance = view.orbit_distance.unwrap_or_else(|| length(subtract(camera.eye(), camera.center)));
            let forward = camera.column(2);
            let eye = camera.eye();
            camera.center = [eye[0] + forward[0] * distance, eye[1] + forward[1] * distance, eye[2] + forward[2] * distance];
        } else if view.source.name(b"MS") == Some(b"U3D") {
            return Err(invalid("embedded U3D camera paths need a decoded view resource"));
        }
        if let Some(value) = view.source.get(b"P") {
            let object = doc.resolve(value);
            let p = object.as_dict().ok_or_else(|| invalid("invalid 3D projection dictionary"))?;
            camera.projection = if view.perspective {
                Projection::Perspective {
                    field_of_view: view.field_of_view,
                    binding: binding(p.get(b"PS").map(|v| doc.resolve(v)).as_deref(), Binding::Width)?,
                }
            } else {
                Projection::Orthographic {
                    scale: view.orthographic_scale,
                    binding: binding(p.get(b"OB").map(|v| doc.resolve(v)).as_deref(), Binding::Absolute)?,
                }
            };
            match p.name(b"CS") {
                Some(b"XNF") => {
                    camera.near = number(doc, p, b"N", if view.perspective { f64::NAN } else { 0. })?;
                    camera.far = if p.contains(b"F") { Some(number(doc, p, b"F", f64::NAN)?) } else { None };
                }
                None | Some(b"ANF") => {}
                _ => return Err(invalid("invalid 3D clipping style")),
            }
        } else {
            camera.projection = Projection::Perspective { field_of_view: 90., binding: Binding::Width };
        }
        camera.validate()?;
        Ok(camera)
    }
    fn column(&self, index: usize) -> Point {
        let i = index.min(3) * 3;
        [self.matrix[i], self.matrix[i + 1], self.matrix[i + 2]]
    }
    pub fn axes(&self) -> [Point; 3] {
        [self.column(0), self.column(1), self.column(2)]
    }
    pub fn eye(&self) -> Point {
        self.column(3)
    }
    pub fn validate(&self) -> Result<()> {
        for point in [self.column(0), self.column(1), self.column(2), self.eye(), self.center] {
            check(point)?;
        }
        let determinant = dot(self.column(0), cross(self.column(1), self.column(2)));
        let size = length(self.column(0)) * length(self.column(1)) * length(self.column(2));
        if size <= 1e-36 || determinant.abs() <= size * 1e-12 {
            return Err(invalid("3D camera matrix is singular"));
        }
        if self.target.iter().any(|v| !v.is_finite() || *v < 1e-12 || *v > 1e12)
            || !self.near.is_finite()
            || self.near < 0.
            || self.near > 1e12
            || self.far.is_some_and(|f| !f.is_finite() || f <= self.near || f > 1e12)
        {
            return Err(invalid("invalid 3D camera target or clipping planes"));
        }
        match self.projection {
            Projection::Perspective { field_of_view, binding } => {
                if !(0.0..180.0).contains(&field_of_view)
                    || field_of_view == 0.
                    || self.near <= 0.
                    || (!binding.value(self.target).is_finite() || binding.value(self.target) <= 0. || binding.value(self.target) > 1e12)
                {
                    return Err(invalid("invalid perspective camera"));
                }
            }
            Projection::Orthographic { scale, binding } => {
                if !scale.is_finite()
                    || scale <= 0.
                    || scale > 1e12
                    || (!binding.value(self.target).is_finite() || binding.value(self.target) <= 0. || binding.value(self.target) > 1e12)
                {
                    return Err(invalid("invalid orthographic camera"));
                }
            }
        }
        if !self.scale().is_finite() || !(1e-24..=1e24).contains(&self.scale()) {
            return Err(invalid("3D projection scale exceeds its bounds"));
        }
        Ok(())
    }
    pub fn world_to_camera(&self, point: Point) -> Result<Point> {
        check(point)?;
        let [x, y, z] = [self.column(0), self.column(1), self.column(2)];
        let determinant = dot(x, cross(y, z));
        let p = subtract(point, self.eye());
        let result = [dot(p, cross(y, z)) / determinant, dot(p, cross(z, x)) / determinant, dot(p, cross(x, y)) / determinant];
        check(result)?;
        Ok(result)
    }
    pub fn camera_to_world(&self, point: Point) -> Result<Point> {
        check(point)?;
        let eye = self.eye();
        let [x, y, z] = [self.column(0), self.column(1), self.column(2)];
        let mut out = eye;
        for i in 0..3 {
            out[i] += x[i] * point[0] + y[i] * point[1] + z[i] * point[2];
        }
        check(out)?;
        Ok(out)
    }
    pub(crate) fn scale(&self) -> f64 {
        match self.projection {
            Projection::Perspective { field_of_view, binding } => binding.value(self.target) / (2. * (field_of_view.to_radians() / 2.).tan()),
            Projection::Orthographic { scale, binding } => scale * binding.value(self.target),
        }
    }
    pub fn project_camera(&self, point: Point, viewport: [f64; 2]) -> Result<Option<Point>> {
        self.validate()?;
        check(point)?;
        if viewport.iter().any(|v| !v.is_finite() || *v <= 0. || *v > 1e6) {
            return Err(invalid("invalid 3D viewport dimensions"));
        }
        if point[2] < self.near || self.far.is_some_and(|far| point[2] > far) {
            return Ok(None);
        };
        let scale = self.scale() / if matches!(self.projection, Projection::Perspective { .. }) { point[2] } else { 1. };
        let out = [(0.5 + point[0] * scale / self.target[0]) * viewport[0], (0.5 - point[1] * scale / self.target[1]) * viewport[1], point[2]];
        check(out)?;
        Ok(Some(out))
    }
    pub fn project(&self, point: Point, viewport: [f64; 2]) -> Result<Option<Point>> {
        self.project_camera(self.world_to_camera(point)?, viewport)
    }
    /// The local projection derivative of a world-space direction. Unlike
    /// projecting an arbitrary second point, this remains accurate at all zooms.
    pub fn project_direction(&self, anchor: Point, axis: Point, viewport: [f64; 2]) -> Result<Option<[f64; 2]>> {
        let point = self.world_to_camera(anchor)?;
        if self.project_camera(point, viewport)?.is_none() {
            return Ok(None);
        }
        let axis = direction(axis)?;
        let [x, y, z] = self.axes();
        let determinant = dot(x, cross(y, z));
        let vector = [dot(axis, cross(y, z)) / determinant, dot(axis, cross(z, x)) / determinant, dot(axis, cross(x, y)) / determinant];
        let [dx, dy] = if matches!(self.projection, Projection::Perspective { .. }) {
            if point[2] <= 0. {
                return Err(invalid("3D direction cannot be projected at the eye plane"));
            }
            [
                self.scale() / point[2] * (vector[0] - point[0] * vector[2] / point[2]),
                self.scale() / point[2] * (vector[1] - point[1] * vector[2] / point[2]),
            ]
        } else {
            [self.scale() * vector[0], self.scale() * vector[1]]
        };
        let result = [dx * viewport[0] / self.target[0], -dy * viewport[1] / self.target[1]];
        if result.iter().any(|v| !v.is_finite() || v.abs() > 1e12) {
            return Err(invalid("3D projected direction exceeds its bounds"));
        }
        Ok(Some(result))
    }
    /// Zoom-invariant text frame. Linear, perpendicular and angular TP is the
    /// lower-left corner. Radial text starts at the extension's left centre.
    pub fn caption_frame(&self, measurement: &super::three_d::Measurement, viewport: [f64; 2], height: f64) -> Result<Option<CaptionFrame>> {
        if !height.is_finite() || !(0.0..=4096.0).contains(&height) {
            return Err(invalid("invalid 3D caption height"));
        }
        measurement.dictionary()?;
        let [x, up] = measurement.text_axes()?;
        let Some(position) = self.project(measurement.text_position, viewport)? else { return Ok(None) };
        let Some(x) = self.project_direction(measurement.text_position, x, viewport)? else { return Ok(None) };
        let Some(up) = self.project_direction(measurement.text_position, up, viewport)? else { return Ok(None) };
        let unit = |axis: [f64; 2]| -> Result<[f64; 2]> {
            let length = axis[0].hypot(axis[1]);
            if length <= 1e-12 {
                return Err(invalid("3D caption plane is edge-on"));
            }
            Ok(axis.map(|v| v / length))
        };
        let [x, up] = [unit(x)?, unit(up)?];
        if (x[0] * up[1] - x[1] * up[0]).abs() <= 1e-8 {
            return Err(invalid("3D caption plane is edge-on"));
        }
        let mut origin = [position[0], position[1]];
        if measurement.geometry.kind() == super::three_d::Kind::Radial {
            for i in 0..2 {
                origin[i] += measurement.extension_length * x[i] - height * 0.5 * up[i];
            }
        }
        Ok(Some(CaptionFrame { origin, x, up }))
    }
    pub fn ray(&self, pixel: [f64; 2], viewport: [f64; 2]) -> Result<[Point; 2]> {
        self.validate()?;
        if pixel.iter().any(|v| !v.is_finite()) || viewport.iter().any(|v| !v.is_finite() || *v <= 0. || *v > 1e6) {
            return Err(invalid("invalid 3D ray screen coordinates"));
        }
        let [x, y] = [(pixel[0] / viewport[0] - 0.5) * self.target[0] / self.scale(), (0.5 - pixel[1] / viewport[1]) * self.target[1] / self.scale()];
        if matches!(self.projection, Projection::Perspective { .. }) {
            let point = self.camera_to_world([x, y, 1.])?;
            Ok([self.eye(), direction(subtract(point, self.eye()))?])
        } else {
            let origin = self.camera_to_world([x, y, 0.])?;
            Ok([origin, direction(self.column(2))?])
        }
    }
    /// Pick only geometry between this view's near and far clipping planes.
    pub fn pick(&self, scene: &Scene, pixel: [f64; 2], viewport: [f64; 2]) -> Result<Option<super::three_d::Hit>> {
        let [origin, ray] = self.ray(pixel, viewport)?;
        let start = self.world_to_camera(origin)?;
        let next = self.world_to_camera([origin[0] + ray[0], origin[1] + ray[1], origin[2] + ray[2]])?;
        let rate = next[2] - start[2];
        if rate <= 1e-15 {
            return Err(invalid("3D pick ray cannot intersect the view clipping planes"));
        }
        let shift = (self.near - start[2]) / rate;
        let clipped = [origin[0] + ray[0] * shift, origin[1] + ray[1] * shift, origin[2] + ray[2] * shift];
        let mut hit = scene.pick(clipped, ray).map_err(|e| invalid(&e.to_string()))?;
        if let Some(value) = &mut hit {
            let depth = self.world_to_camera(value.point)?[2];
            if self.far.is_some_and(|far| depth > far) {
                return Ok(None);
            };
            value.ray_distance += shift;
        }
        Ok(hit)
    }
    /// Intersect a screen position with a model-space annotation plane. The
    /// caller supplies a point on the plane and its normal, so caption movement
    /// stays independent of mesh picking and cannot jump to another surface.
    pub fn plane_point(&self, pixel: [f64; 2], viewport: [f64; 2], anchor: Point, normal: Point) -> Result<Point> {
        check(anchor)?;
        let normal = direction(normal)?;
        let [origin, ray] = self.ray(pixel, viewport)?;
        if pixel[0] < 0. || pixel[1] < 0. || pixel[0] > viewport[0] || pixel[1] > viewport[1] {
            return Err(invalid("3D caption position must be inside the viewport"));
        }
        let denominator = dot(normal, ray);
        if denominator.abs() <= 1e-12 {
            return Err(invalid("the annotation plane is edge-on to the camera"));
        }
        let distance = dot(normal, subtract(anchor, origin)) / denominator;
        if !distance.is_finite() || distance < 0. {
            return Err(invalid("the annotation plane is behind the camera"));
        }
        let point = std::array::from_fn(|i| origin[i] + distance * ray[i]);
        check(point)?;
        if self.project(point, viewport)?.is_none() {
            return Err(invalid("3D caption position lies outside the view clipping planes"));
        }
        Ok(point)
    }
    pub fn orbit(&mut self, yaw: f64, pitch: f64) -> Result<()> {
        if [yaw, pitch].iter().any(|v| !v.is_finite() || v.abs() > std::f64::consts::TAU) {
            return Err(invalid("invalid 3D orbit angles"));
        }
        let mut next = self.clone();
        for (axis, angle) in [(direction(self.column(1))?, yaw), (direction(self.column(0))?, pitch)] {
            let rotate = |v: Point| {
                let c = angle.cos();
                let s = angle.sin();
                let cross = cross(axis, v);
                let dot = dot(axis, v);
                [
                    v[0] * c + cross[0] * s + axis[0] * dot * (1. - c),
                    v[1] * c + cross[1] * s + axis[1] * dot * (1. - c),
                    v[2] * c + cross[2] * s + axis[2] * dot * (1. - c),
                ]
            };
            for index in 0..3 {
                let value = rotate(next.column(index));
                for (target, value) in next.matrix.iter_mut().skip(index * 3).take(3).zip(value) {
                    *target = value;
                }
            }
            let eye = rotate(subtract(next.eye(), next.center));
            for (target, (eye, center)) in next.matrix.iter_mut().skip(9).zip(eye.into_iter().zip(next.center)) {
                *target = eye + center;
            }
        }
        next.validate()?;
        *self = next;
        Ok(())
    }
    pub fn pan(&mut self, delta: [f64; 2], viewport: [f64; 2]) -> Result<()> {
        self.validate()?;
        if delta.iter().any(|v| !v.is_finite()) || viewport.iter().any(|v| !v.is_finite() || *v <= 0.) {
            return Err(invalid("invalid 3D pan"));
        }
        let depth = self.world_to_camera(self.center)?[2];
        let scale = self.scale() / if matches!(self.projection, Projection::Perspective { .. }) { depth } else { 1. };
        if scale.abs() < 1e-12 {
            return Err(invalid("3D pan scale is degenerate"));
        }
        let [x, y] = [delta[0] * self.target[0] / viewport[0] / scale, -delta[1] * self.target[1] / viewport[1] / scale];
        let [right, up] = [self.column(0), self.column(1)];
        let mut next = self.clone();
        for (i, center) in next.center.iter_mut().enumerate() {
            let shift = right[i] * x + up[i] * y;
            *center -= shift;
            if let Some(eye) = next.matrix.get_mut(9 + i) {
                *eye -= shift;
            }
        }
        next.validate()?;
        *self = next;
        Ok(())
    }
    pub fn zoom(&mut self, factor: f64) -> Result<()> {
        if !factor.is_finite() || !(0.01..=100.).contains(&factor) {
            return Err(invalid("3D zoom factor must be between 0.01 and 100"));
        }
        let mut next = self.clone();
        match &mut next.projection {
            Projection::Orthographic { scale, .. } => *scale *= factor,
            Projection::Perspective { .. } => {
                let offset = subtract(self.eye(), self.center);
                for (eye, (center, offset)) in next.matrix.iter_mut().skip(9).zip(self.center.into_iter().zip(offset)) {
                    *eye = center + offset / factor;
                }
            }
        }
        next.validate()?;
        *self = next;
        Ok(())
    }
}

#[cfg(test)]
mod caption_tests {
    use super::*;
    fn camera() -> Camera {
        Camera {
            matrix: [1., 0., 0., 0., 1., 0., 0., 0., 1., 0., 0., 0.],
            center: [0., 0., 5.],
            target: [100., 100.],
            projection: Projection::Perspective { field_of_view: 90., binding: Binding::Width },
            near: 1.,
            far: Some(20.),
        }
    }
    #[test]
    fn caption_plane_intersections_match_projection_and_preserve_camera() {
        let mut camera = camera();
        for projection in [
            Projection::Perspective { field_of_view: 90., binding: Binding::Width },
            Projection::Orthographic { scale: 10., binding: Binding::Absolute },
        ] {
            camera.projection = projection;
            let original = camera.clone();
            let point = camera.plane_point([70., 40.], [100., 100.], [0., 0., 5.], [0., 0., 2.]).unwrap();
            assert!(subtract(point, [2., 1., 5.]).iter().all(|n| n.abs() < 1e-12));
            let screen = camera.project(point, [100., 100.]).unwrap().unwrap();
            assert!((screen[0] - 70.).abs() < 1e-12 && (screen[1] - 40.).abs() < 1e-12);
            assert_eq!(camera, original);
        }
        camera.orbit(0.2, -0.1).unwrap();
        camera.pan([7., 3.], [100., 100.]).unwrap();
        camera.zoom(1.2).unwrap();
        let expected = [1., 2., 5.];
        let screen = camera.project(expected, [100., 100.]).unwrap().unwrap();
        let actual = camera.plane_point([screen[0], screen[1]], [100., 100.], [0., 0., 5.], [0., 0., 1.]).unwrap();
        assert!(subtract(actual, expected).iter().all(|n| n.abs() < 1e-10));
    }
    #[test]
    fn hostile_caption_planes_are_errors_without_camera_changes() {
        let camera = camera();
        let original = camera.clone();
        for (pixel, anchor, normal) in [
            ([50., 50.], [0., 0., 5.], [0., 0., 0.]),
            ([50., 50.], [1., 0., 0.], [1., 0., 0.]),
            ([50., 50.], [0., 0., -5.], [0., 0., 1.]),
            ([50., 50.], [0., 0., 0.5], [0., 0., 1.]),
            ([50., 50.], [0., 0., 30.], [0., 0., 1.]),
            ([101., 50.], [0., 0., 5.], [0., 0., 1.]),
            ([f64::NAN, 50.], [0., 0., 5.], [0., 0., 1.]),
            ([50., 50.], [f64::INFINITY, 0., 5.], [0., 0., 1.]),
        ] {
            assert!(camera.plane_point(pixel, [100., 100.], anchor, normal).is_err());
            assert_eq!(camera, original);
        }
    }
    #[test]
    fn caption_frames_honour_plane_axes_up_direction_and_zoom_invariant_height() {
        use super::super::three_d::{Geometry, Measurement, Units};
        let mut camera = camera();
        let mut measurement =
            Measurement::new(Geometry::Linear { a: [0., 0., 5.], b: [2., 0., 5.] }, &Units::default(), [0., 0., 1.], [1., 1., 5.]).unwrap();
        let frame = camera.caption_frame(&measurement, [100., 100.], 12.).unwrap().unwrap();
        assert!(subtract([frame.origin[0], frame.origin[1], 0.], [60., 40., 0.]).iter().all(|n| n.abs() < 1e-12));
        assert_eq!(frame.x, [1., -0.]);
        assert_eq!(frame.up, [0., -1.]);
        assert_eq!(frame.point(0., 12.), [60., 28.]);
        measurement.text_y = [0., -4., 0.];
        assert_eq!(camera.caption_frame(&measurement, [100., 100.], 12.).unwrap().unwrap().up, [-0., 1.]);
        measurement.text_y = [0., 1., 0.];
        for zoom in [0.5, 2.] {
            camera.zoom(zoom).unwrap();
            let frame = camera.caption_frame(&measurement, [100., 100.], 12.).unwrap().unwrap();
            let top = frame.point(0., 12.);
            assert!(((top[0] - frame.origin[0]).hypot(top[1] - frame.origin[1]) - 12.).abs() < 1e-12);
        }
        measurement.geometry = Geometry::Perpendicular { a: [0., 0., 5.], b: [2., 3., 5.], direction: [0., 2., 0.] };
        assert_eq!(measurement.text_axes().unwrap(), [[1., 0., 0.], [0., 1., 0.]]);
        measurement.text_y = [1., 0., 0.];
        assert!(measurement.text_axes().is_err());
        measurement.text_y = [0., 1., 0.];
        measurement.plane = [1., 0., 0.];
        assert!(measurement.text_axes().is_err());
    }
    #[test]
    fn radial_caption_uses_extension_length_and_left_centre_anchor() {
        use super::super::three_d::{Geometry, Measurement, Units};
        let camera = camera();
        let mut measurement = Measurement::new(
            Geometry::Radial { center: [0., 0., 5.], on_circle: [1., 0., 5.], diameter: false, arc: None },
            &Units::default(),
            [0., 0., 1.],
            [0., 0., 5.],
        )
        .unwrap();
        let frame = camera.caption_frame(&measurement, [100., 100.], 12.).unwrap().unwrap();
        assert_eq!(frame.origin, [110., 56.]);
        assert_eq!(frame.point(0., 6.), [110., 50.]);
        measurement.extension_length = 0.;
        assert_eq!(camera.caption_frame(&measurement, [100., 100.], 12.).unwrap().unwrap().origin, [50., 56.]);
        for value in [-1., 4097., f64::NAN, f64::INFINITY] {
            measurement.extension_length = value;
            assert!(measurement.dictionary().is_err());
        }
    }
    #[test]
    fn analytic_projected_directions_match_perspective_and_orthographic_projections() {
        let mut camera = camera();
        camera.orbit(0.2, -0.1).unwrap();
        for projection in [
            Projection::Perspective { field_of_view: 60., binding: Binding::Height },
            Projection::Orthographic { scale: 10., binding: Binding::Absolute },
        ] {
            camera.projection = projection;
            let point = [1., 1., 5.];
            let axis = direction([1., 2., 0.5]).unwrap();
            let derivative = camera.project_direction(point, axis, [320., 200.]).unwrap().unwrap();
            let a = camera.project(point, [320., 200.]).unwrap().unwrap();
            let b = camera.project(std::array::from_fn(|i| point[i] + axis[i] * 1e-6), [320., 200.]).unwrap().unwrap();
            for i in 0..2 {
                assert!(((b[i] - a[i]) / 1e-6 - derivative[i]).abs() < 1e-4);
            }
        }
        assert!(camera.project_direction([0., 0., 5.], [0.; 3], [100., 100.]).is_err());
        assert!(camera.project_direction([0., 0., 5.], [1., 0., 0.], [f64::NAN, 100.]).is_err());
        assert!(camera.project_direction([0., 0., 30.], [1., 0., 0.], [100., 100.]).unwrap().is_none());
    }
    #[test]
    fn point_construction_planes_follow_the_requested_view_facing_normal() {
        use super::super::three_d::{Kind, Measurement, Units};
        for (kind, points) in [
            (Kind::Linear, vec![[0., 0., 5.], [2., 0., 5.]]),
            (Kind::Perpendicular, vec![[0., 2., 5.], [0., 0., 5.], [2., 0., 5.]]),
            (Kind::Angular, vec![[2., 0., 5.], [0., 0., 5.], [0., 2., 5.]]),
            (Kind::Radial, vec![[2., 0., 5.], [0., 2., 5.], [-2., 0., 5.]]),
        ] {
            for hint in [[0., 0., 1.], [0., 0., -1.]] {
                let measurement = Measurement::from_points(kind, &points, &Units::default(), hint, false).unwrap();
                assert!(dot(measurement.plane, hint) > 0.);
                assert!(measurement.text_axes().is_ok());
            }
        }
    }
}
