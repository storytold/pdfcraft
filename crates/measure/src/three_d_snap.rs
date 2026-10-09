//! Bounded screen-space snapping to the model's actual transformed primitives.
use super::{
    three_d::{Point, Scene, cross, dot, subtract},
    three_d_camera::{Camera, Projection},
};
use crate::{Result, invalid};
use serde::Serialize;
use std::collections::BTreeMap;

const MAX_PRIMITIVES: usize = 1_000_000;
const MAX_CANDIDATES: usize = 16;
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Mode {
    Surface,
    Vertex,
    Edge,
    Silhouette,
    Auto,
}
impl Mode {
    pub fn from_name(name: &str) -> Result<Self> {
        match name {
            "surface" => Ok(Self::Surface),
            "vertex" => Ok(Self::Vertex),
            "edge" => Ok(Self::Edge),
            "silhouette" => Ok(Self::Silhouette),
            "auto" => Ok(Self::Auto),
            _ => Err(invalid("3D snap must be surface, vertex, edge, silhouette or auto")),
        }
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    Surface,
    Vertex,
    Edge,
    Silhouette,
    Point,
    Line,
}
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Hit {
    pub instance: usize,
    /// Triangle, point or line index in the instance's mesh.
    pub primitive: usize,
    pub triangle: Option<usize>,
    pub kind: Kind,
    pub point: Point,
    pub screen_distance: f64,
    pub ray_distance: f64,
    pub barycentric: Option<Point>,
}
struct Candidate {
    hit: Hit,
    pixel: [f64; 2],
    depth: f64,
}
impl Candidate {
    fn rank(&self) -> u8 {
        match self.hit.kind {
            Kind::Vertex | Kind::Point => 0,
            _ => 1,
        }
    }
    fn before(&self, other: &Self) -> bool {
        (self.rank(), self.hit.screen_distance, self.depth) < (other.rank(), other.hit.screen_distance, other.depth)
    }
}
type EdgeKey = (usize, [[u64; 3]; 2]);
struct Outline {
    point: Point,
    triangle: usize,
    sides: u8,
    count: usize,
    drawable: bool,
}
struct Search<'a> {
    camera: &'a Camera,
    pixel: [f64; 2],
    viewport: [f64; 2],
    radius: f64,
    candidates: Vec<Candidate>,
    overflow: bool,
    outlines: BTreeMap<EdgeKey, Outline>,
}
impl Search<'_> {
    fn add(&mut self, instance: usize, primitive: usize, triangle: Option<usize>, kind: Kind, point: Point) -> Result<()> {
        let Some(projected) = self.camera.project(point, self.viewport)? else { return Ok(()) };
        if projected[0] < 0. || projected[0] > self.viewport[0] || projected[1] < 0. || projected[1] > self.viewport[1] {
            return Ok(());
        }
        let distance = (projected[0] - self.pixel[0]).hypot(projected[1] - self.pixel[1]);
        if distance > self.radius {
            return Ok(());
        }
        if self.candidates.iter().any(|c| c.hit.instance == instance && c.hit.kind == kind && c.hit.point == point) {
            return Ok(());
        }
        let candidate = Candidate {
            hit: Hit { instance, primitive, triangle, kind, point, screen_distance: distance, ray_distance: 0., barycentric: None },
            pixel: [projected[0], projected[1]],
            depth: projected[2],
        };
        let index = self.candidates.iter().position(|other| candidate.before(other)).unwrap_or(self.candidates.len());
        if self.candidates.len() == MAX_CANDIDATES {
            self.overflow = true;
        }
        if index < MAX_CANDIDATES {
            self.candidates.insert(index, candidate);
            self.candidates.truncate(MAX_CANDIDATES);
        }
        Ok(())
    }
    fn edge(&mut self, instance: usize, primitive: usize, triangle: Option<usize>, kind: Kind, a: Point, b: Point) -> Result<()> {
        if let Some(point) = self.edge_point(a, b)? {
            self.add(instance, primitive, triangle, kind, point)?;
        }
        Ok(())
    }
    fn outline(&mut self, instance: usize, triangle: usize, facing: f64, drawable: bool, a: Point, b: Point) -> Result<()> {
        if facing == 0. {
            return Ok(());
        }
        let Some(point) = self.edge_point(a, b)? else { return Ok(()) };
        let Some(p) = self.camera.project(point, self.viewport)? else { return Ok(()) };
        if (p[0] - self.pixel[0]).hypot(p[1] - self.pixel[1]) > self.radius {
            return Ok(());
        }
        let bits = |p: Point| p.map(|n| if n == 0. { 0 } else { n.to_bits() });
        let mut ends = [bits(a), bits(b)];
        ends.sort();
        let key = (instance, ends);
        if !self.outlines.contains_key(&key) && self.outlines.len() >= 4096 {
            return Err(invalid("3D silhouette exceeds 4096 nearby edges"));
        }
        let outline = self.outlines.entry(key).or_insert(Outline { point, triangle, sides: 0, count: 0, drawable: false });
        outline.sides |= if facing > 0. { 1 } else { 2 };
        outline.count = outline.count.saturating_add(1);
        outline.drawable |= drawable;
        Ok(())
    }
    fn edge_point(&self, a: Point, b: Point) -> Result<Option<Point>> {
        let mut a = self.camera.world_to_camera(a)?;
        let mut b = self.camera.world_to_camera(b)?;
        // Clip in camera space before dividing by depth. Edges crossing the eye
        // or the near plane must not produce huge or mirrored screen targets.
        let mut lo: f64 = 0.;
        let mut hi: f64 = 1.;
        for (plane, above) in [(Some(self.camera.near), true), (self.camera.far, false)] {
            let Some(plane) = plane else { continue };
            let da = if above { a[2] - plane } else { plane - a[2] };
            let db = if above { b[2] - plane } else { plane - b[2] };
            if da < 0. && db < 0. {
                return Ok(None);
            }
            if da < 0. {
                lo = lo.max(da / (da - db));
            }
            if db < 0. {
                hi = hi.min(da / (da - db));
            }
        }
        if hi < lo {
            return Ok(None);
        }
        let original = a;
        let delta = subtract(b, a);
        for i in 0..3 {
            a[i] = original[i] + delta[i] * lo;
            b[i] = original[i] + delta[i] * hi;
        }
        // Avoid a rounding error placing a clipped endpoint just behind its plane.
        a[2] = a[2].max(self.camera.near);
        b[2] = b[2].max(self.camera.near);
        if let Some(far) = self.camera.far {
            a[2] = a[2].min(far);
            b[2] = b[2].min(far);
        }
        let (Some(pa), Some(pb)) = (self.camera.project_camera(a, self.viewport)?, self.camera.project_camera(b, self.viewport)?) else {
            return Ok(None);
        };
        let d = [pb[0] - pa[0], pb[1] - pa[1]];
        let squared = d[0] * d[0] + d[1] * d[1];
        let t = if squared <= 1e-24 { 0. } else { (((self.pixel[0] - pa[0]) * d[0] + (self.pixel[1] - pa[1]) * d[1]) / squared).clamp(0., 1.) };
        // Screen interpolation is perspective correct, so snapping returns a point
        // on the actual model edge even when the endpoints have different depths.
        let t = if matches!(self.camera.projection, Projection::Perspective { .. }) { (t / b[2]) / ((1. - t) / a[2] + t / b[2]) } else { t };
        let mut point = [0.; 3];
        for i in 0..3 {
            point[i] = a[i] + (b[i] - a[i]) * t;
        }
        Ok(Some(self.camera.camera_to_world(point)?))
    }
}
impl Camera {
    /// Snap within a finite screen radius, honoring transforms, face visibility,
    /// projection and clipping. Occlusion checks use at most sixteen face rays.
    /// A bounded search that cannot finish returns an error instead of a partial hit.
    pub fn snap(&self, scene: &Scene, pixel: [f64; 2], viewport: [f64; 2], mode: Mode, radius: f64) -> Result<Option<Hit>> {
        self.ray(pixel, viewport)?;
        if !radius.is_finite() || !(0. ..=64.).contains(&radius) {
            return Err(invalid("3D snap radius must be 0 to 64 pixels"));
        }
        if pixel[0] < 0. || pixel[1] < 0. || pixel[0] > viewport[0] || pixel[1] > viewport[1] {
            return Ok(None);
        }
        if mode != Mode::Surface {
            let mut search = Search { camera: self, pixel, viewport, radius, candidates: Vec::new(), overflow: false, outlines: BTreeMap::new() };
            let mut work = 0usize;
            for (instance_index, instance) in scene.instances.iter().enumerate() {
                work = work.saturating_add(1);
                if work > MAX_PRIMITIVES {
                    return Err(invalid("3D snapping exceeds one million primitive visits"));
                }
                if instance.visibility == 0 {
                    continue;
                }
                let mesh = scene.meshes.get(instance.mesh).ok_or_else(|| invalid("3D instance refers to a missing mesh"))?;
                for triangle in 0..mesh.triangles.len() {
                    work = work.saturating_add(1);
                    if work > MAX_PRIMITIVES {
                        return Err(invalid("3D snapping exceeds one million primitive visits"));
                    }
                    let [a, b, c] = scene.triangle(instance_index, triangle).map_err(|e| invalid(&e.to_string()))?;
                    let ray = if matches!(self.projection, Projection::Perspective { .. }) { subtract(a, self.eye()) } else { self.axes()[2] };
                    let facing = -dot(ray, cross(subtract(b, a), subtract(c, a)));
                    let drawable = !((instance.visibility == 1 && facing <= 0.) || (instance.visibility == 2 && facing >= 0.));
                    if mode == Mode::Silhouette {
                        for (a, b) in [(a, b), (b, c), (c, a)] {
                            search.outline(instance_index, triangle, facing, drawable, a, b)?;
                        }
                        continue;
                    }
                    if !drawable {
                        continue;
                    }
                    if matches!(mode, Mode::Vertex | Mode::Auto) {
                        for point in [a, b, c] {
                            search.add(instance_index, triangle, Some(triangle), Kind::Vertex, point)?;
                        }
                    }
                    if matches!(mode, Mode::Edge | Mode::Auto | Mode::Silhouette) {
                        for (a, b) in [(a, b), (b, c), (c, a)] {
                            search.edge(instance_index, triangle, Some(triangle), Kind::Edge, a, b)?;
                        }
                    }
                }
                for index in 0..mesh.lines.len() {
                    work = work.saturating_add(1);
                    if work > MAX_PRIMITIVES {
                        return Err(invalid("3D snapping exceeds one million primitive visits"));
                    }
                    let [a, b] = scene.line(instance_index, index).map_err(|e| invalid(&e.to_string()))?;
                    // Even a line-only model has exact selectable endpoints.
                    if matches!(mode, Mode::Vertex | Mode::Auto) {
                        for point in [a, b] {
                            search.add(instance_index, index, None, Kind::Vertex, point)?;
                        }
                    }
                    if matches!(mode, Mode::Edge | Mode::Auto | Mode::Silhouette) {
                        search.edge(instance_index, index, None, Kind::Line, a, b)?;
                    }
                }
                for point in 0..mesh.points.len() {
                    work = work.saturating_add(1);
                    if work > MAX_PRIMITIVES {
                        return Err(invalid("3D snapping exceeds one million primitive visits"));
                    }
                    if matches!(mode, Mode::Vertex | Mode::Auto) {
                        search.add(
                            instance_index,
                            point,
                            None,
                            Kind::Point,
                            scene.point(instance_index, point).map_err(|e| invalid(&e.to_string()))?,
                        )?;
                    }
                }
            }
            let outlines = std::mem::take(&mut search.outlines);
            for ((instance, _), outline) in outlines {
                if outline.drawable && (outline.count == 1 || outline.sides == 3) {
                    search.add(instance, outline.triangle, Some(outline.triangle), Kind::Silhouette, outline.point)?;
                }
            }
            let overflow = search.overflow;
            for mut candidate in search.candidates {
                if let Some(front) = self.pick(scene, candidate.pixel, viewport)? {
                    let front_depth = self.world_to_camera(front.point)?[2];
                    let tolerance = 1e-8 * front_depth.abs().max(candidate.depth.abs()).max(1.);
                    if front_depth + tolerance < candidate.depth {
                        continue;
                    }
                }
                let [origin, ray] = self.ray(candidate.pixel, viewport)?;
                candidate.hit.ray_distance = dot(subtract(candidate.hit.point, origin), ray);
                return Ok(Some(candidate.hit));
            }
            if overflow {
                return Err(invalid("3D snap occlusion exceeds sixteen nearby candidates"));
            }
        }
        if matches!(mode, Mode::Surface | Mode::Auto) {
            return Ok(self.pick(scene, pixel, viewport)?.map(|h| Hit {
                instance: h.instance,
                primitive: h.triangle,
                triangle: Some(h.triangle),
                kind: Kind::Surface,
                point: h.point,
                screen_distance: 0.,
                ray_distance: h.ray_distance,
                barycentric: Some(h.barycentric),
            }));
        }
        Ok(None)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::three_d_camera::Binding;
    use pdfcraft_model::three_d::{IDENTITY, Instance, Mesh, NormalEncoding};
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
    fn scene(mesh: Mesh) -> Scene {
        Scene {
            meshes: vec![mesh],
            instances: vec![Instance { name: "part".into(), mesh: 0, transform: IDENTITY, visibility: 3 }],
            meters_per_unit: None,
            normal_encoding: NormalEncoding::VectorDifferences,
        }
    }
    #[test]
    fn vertices_edges_faces_and_perspective_correct_lines() {
        let camera = camera();
        let scene = scene(Mesh { positions: vec![[-2., -2., 5.], [2., -2., 5.], [0., 2., 5.]], triangles: vec![[0, 1, 2]], ..Mesh::default() });
        let h = camera.snap(&scene, [31., 69.], [100., 100.], Mode::Auto, 4.).unwrap().unwrap();
        assert_eq!(h.kind, Kind::Vertex);
        assert_eq!(h.point, [-2., -2., 5.]);
        let h = camera.snap(&scene, [50., 69.], [100., 100.], Mode::Edge, 4.).unwrap().unwrap();
        assert_eq!(h.kind, Kind::Edge);
        assert!(subtract(h.point, [0., -2., 5.]).iter().all(|x| x.abs() < 1e-12));
        assert_eq!(camera.snap(&scene, [50., 50.], [100., 100.], Mode::Auto, 4.).unwrap().unwrap().kind, Kind::Surface);
        assert!(camera.snap(&scene, [50., 50.], [100., 100.], Mode::Vertex, 4.).unwrap().is_none());
        let lines = super::tests::scene(Mesh { positions: vec![[-2., 0., 2.], [4., 0., 8.]], lines: vec![[0, 1]], ..Mesh::default() });
        let h = camera.snap(&lines, [50., 51.], [100., 100.], Mode::Edge, 4.).unwrap().unwrap();
        assert_eq!(h.kind, Kind::Line);
        assert!(subtract(h.point, [0., 0., 4.]).iter().all(|x| x.abs() < 1e-12));
        assert!(h.triangle.is_none());
    }
    #[test]
    fn point_and_line_models_transforms_clipping_and_hidden_targets() {
        let camera = camera();
        let mut model = scene(Mesh { positions: vec![[0., 0., 5.], [1., 0., 0.], [1., 0., 30.]], points: vec![0, 1, 2], ..Mesh::default() });
        model.instances[0].transform[12] = 1.;
        let h = camera.snap(&model, [60., 50.], [100., 100.], Mode::Vertex, 2.).unwrap().unwrap();
        assert_eq!(h.kind, Kind::Point);
        assert_eq!(h.point, [1., 0., 5.]);
        model.instances[0].visibility = 0;
        assert!(camera.snap(&model, [60., 50.], [100., 100.], Mode::Auto, 2.).unwrap().is_none());
        let line = scene(Mesh { positions: vec![[0., 0., -2.], [0., 0., 5.]], lines: vec![[0, 1]], ..Mesh::default() });
        let h = camera.snap(&line, [50., 50.], [100., 100.], Mode::Edge, 2.).unwrap().unwrap();
        assert_eq!(h.point, [0., 0., 1.]);
        assert!(camera.snap(&line, [50., 50.], [100., 100.], Mode::Edge, f64::NAN).is_err());
        assert!(camera.snap(&line, [50., 50.], [100., 100.], Mode::Edge, 65.).is_err());
        assert!(camera.snap(&line, [-1., 50.], [100., 100.], Mode::Auto, 8.).unwrap().is_none());
    }
    #[test]
    fn nearer_surface_occludes_point_and_back_faces_do_not_snap() {
        let camera = camera();
        let mut model = scene(Mesh {
            positions: vec![[-2., -2., 3.], [2., -2., 3.], [0., 2., 3.], [0., 0., 5.]],
            triangles: vec![[0, 1, 2]],
            points: vec![3],
            ..Mesh::default()
        });
        assert!(camera.snap(&model, [50., 50.], [100., 100.], Mode::Vertex, 2.).unwrap().is_none());
        assert_eq!(camera.snap(&model, [50., 50.], [100., 100.], Mode::Auto, 2.).unwrap().unwrap().kind, Kind::Surface);
        model.meshes[0].points.clear();
        model.instances[0].visibility = 1;
        assert!(camera.snap(&model, [16.7, 83.3], [100., 100.], Mode::Auto, 4.).unwrap().is_none());
        model.instances[0].visibility = 2;
        assert_eq!(camera.snap(&model, [16.7, 83.3], [100., 100.], Mode::Auto, 4.).unwrap().unwrap().kind, Kind::Vertex);
    }
    #[test]
    fn invalid_indices_and_aggregate_visits_fail_without_partial_hits() {
        let camera = camera();
        let malformed = scene(Mesh { positions: vec![[0., 0., 5.]], lines: vec![[0, 7]], ..Mesh::default() });
        assert!(camera.snap(&malformed, [50., 50.], [100., 100.], Mode::Auto, 8.).is_err());
        let dense = scene(Mesh { positions: vec![[0., 0., 5.]], points: vec![0; MAX_PRIMITIVES], ..Mesh::default() });
        assert!(camera.snap(&dense, [50., 50.], [100., 100.], Mode::Vertex, 8.).is_err());
    }
    #[test]
    fn silhouettes_exclude_internal_edges_and_follow_transformed_boundaries() {
        let camera = camera();
        let mut model = scene(Mesh {
            positions: vec![[-2., -2., 5.], [2., -2., 5.], [2., 2., 5.], [-2., 2., 5.]],
            triangles: vec![[0, 1, 2], [0, 2, 3]],
            ..Mesh::default()
        });
        assert!(camera.snap(&model, [50., 50.], [100., 100.], Mode::Silhouette, 3.).unwrap().is_none());
        let h = camera.snap(&model, [50., 69.], [100., 100.], Mode::Silhouette, 3.).unwrap().unwrap();
        assert_eq!(h.kind, Kind::Silhouette);
        assert!(subtract(h.point, [0., -2., 5.]).iter().all(|n| n.abs() < 1e-12));
        // Coincident split vertices are still the same geometric internal edge.
        model.meshes[0].positions.extend([[-2., -2., 5.], [2., 2., 5.]]);
        model.meshes[0].triangles[1] = [4, 5, 3];
        assert!(camera.snap(&model, [50., 50.], [100., 100.], Mode::Silhouette, 3.).unwrap().is_none());
        // A front/back transition is a silhouette even on a closed shared edge.
        model.meshes[0].triangles[1] = [3, 5, 4];
        assert_eq!(camera.snap(&model, [50., 50.], [100., 100.], Mode::Silhouette, 3.).unwrap().unwrap().kind, Kind::Silhouette);
        model.instances[0].transform[12] = 1.;
        assert_eq!(camera.snap(&model, [60., 50.], [100., 100.], Mode::Silhouette, 3.).unwrap().unwrap().point, [1., 0., 5.]);
    }
    #[test]
    fn occlusion_budget_does_not_report_an_incomplete_negative_result() {
        let camera = camera();
        let mut mesh = Mesh { positions: vec![[-2., -2., 3.], [2., -2., 3.], [0., 2., 3.]], triangles: vec![[0, 1, 2]], ..Mesh::default() };
        for i in 0..20 {
            mesh.points.push(mesh.positions.len() as u32);
            mesh.positions.push([f64::from(i) * 0.001, 0., 5.]);
        }
        let error = camera.snap(&scene(mesh), [50., 50.], [100., 100.], Mode::Vertex, 2.).unwrap_err();
        assert!(error.to_string().contains("sixteen nearby"));
    }
}
