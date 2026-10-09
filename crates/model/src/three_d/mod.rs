//! Embedded 3D model geometry. All coordinates remain in the model's world space.
//! The PDF graph and original model streams are never rewritten by these readers.
mod bitstream;
#[cfg(test)]
mod fixtures;
mod u3d;
use std::fmt;
pub type Point = [f64; 3];
pub type Matrix = [f64; 16];
pub type Result<T> = std::result::Result<T, Error>;
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Error(String);
impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}
impl std::error::Error for Error {}
fn invalid(message: &str) -> Error {
    Error(message.into())
}
pub const IDENTITY: Matrix = [1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0];
const MAX_INSTANCES: usize = 4096;
const MAX_TRIANGLES: usize = 1_000_000;
const MAX_POSITIONS: usize = 1_000_000;
// Shared across every block and resource. A small compressed stream must not
// reset its allowance by creating more modifier chains or mesh resources.
const MAX_DECODE_WORK: usize = 20_000_000;
const MAX_DECODE_RECORD_BYTES: usize = 128 * 1024 * 1024;
#[derive(Default)]
struct DecodeBudget {
    work: std::cell::Cell<usize>,
    bytes: std::cell::Cell<usize>,
    records: [std::cell::Cell<usize>; 4],
}
#[derive(Clone, Copy)]
enum RecordKind {
    Position,
    Normal,
    Attribute,
    Face,
}
impl DecodeBudget {
    fn spend(&self, count: usize) -> Result<()> {
        let work = self.work.get().saturating_add(count);
        if work > MAX_DECODE_WORK {
            return Err(invalid("U3D decode work limit exceeded"));
        }
        self.work.set(work);
        Ok(())
    }
    fn allocate(&self, bytes: usize) -> Result<()> {
        let total = self.bytes.get().saturating_add(bytes);
        if total > MAX_DECODE_RECORD_BYTES {
            return Err(invalid("U3D decoded records exceed 128 MiB"));
        }
        self.bytes.set(total);
        Ok(())
    }
    fn reserve(&self, kind: RecordKind, count: usize, bytes_per_record: usize) -> Result<()> {
        let (slot, limit) = match kind {
            RecordKind::Position => (0, MAX_POSITIONS),
            RecordKind::Normal => (1, MAX_POSITIONS),
            RecordKind::Attribute => (2, MAX_POSITIONS * 3),
            RecordKind::Face => (3, MAX_TRIANGLES),
        };
        let counter = self.records.get(slot).ok_or_else(|| invalid("U3D record budget is missing"))?;
        let total = counter.get().saturating_add(count);
        if total > limit {
            return Err(invalid("U3D aggregate record count exceeds its limit"));
        }
        self.allocate(count.saturating_mul(bytes_per_record))?;
        counter.set(total);
        Ok(())
    }
}
#[derive(Clone, Debug, PartialEq, Default)]
pub struct Mesh {
    pub name: String,
    pub positions: Vec<Point>,
    pub triangles: Vec<[u32; 3]>,
    pub points: Vec<u32>,
    pub lines: Vec<[u32; 2]>,
    pub point_shading: Vec<u32>,
    pub line_shading: Vec<u32>,
    pub point_attributes: Vec<PrimitiveAttributes<1>>,
    pub line_attributes: Vec<PrimitiveAttributes<2>>,
    pub shading: Vec<u32>,
    pub normals: Vec<Point>,
    pub attributes: Vec<FaceAttributes>,
    pub diffuse: Vec<[f64; 4]>,
    pub specular: Vec<[f64; 4]>,
    pub texture_coordinates: Vec<[f64; 4]>,
}
#[derive(Clone, Debug, PartialEq)]
pub struct PrimitiveAttributes<const N: usize> {
    pub normals: Option<[u32; N]>,
    pub diffuse: Option<[u32; N]>,
    pub specular: Option<[u32; N]>,
    pub textures: Vec<[u32; N]>,
}
pub type FaceAttributes = PrimitiveAttributes<3>;
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NormalEncoding {
    /// Coordinate differences and agglomerative prediction from ECMA-363.
    VectorDifferences,
    /// Rotation sines and clustered prediction observed through the reference APIs.
    ReferenceRotations,
}
#[derive(Clone, Debug, PartialEq)]
pub struct Instance {
    pub name: String,
    pub mesh: usize,
    pub transform: Matrix,
    pub visibility: u32,
}
#[derive(Clone, Debug, PartialEq)]
pub struct Scene {
    pub meshes: Vec<Mesh>,
    pub instances: Vec<Instance>,
    /// Metres per model-space unit, if the model explicitly declares its units.
    pub meters_per_unit: Option<f64>,
    pub normal_encoding: NormalEncoding,
}
#[derive(Clone, Debug, PartialEq)]
pub struct Hit {
    pub instance: usize,
    pub triangle: usize,
    pub point: Point,
    pub ray_distance: f64,
    pub barycentric: Point,
}
fn dot(a: Point, b: Point) -> f64 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}
fn sub(a: Point, b: Point) -> Point {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}
fn cross(a: Point, b: Point) -> Point {
    [a[1] * b[2] - a[2] * b[1], a[2] * b[0] - a[0] * b[2], a[0] * b[1] - a[1] * b[0]]
}
fn checked_point(p: Point) -> Result<Point> {
    if p.iter().all(|v| v.is_finite() && v.abs() <= 1e12) { Ok(p) } else { Err(invalid("3D point is not finite or exceeds 1e12")) }
}
pub fn transform(matrix: Matrix, point: Point) -> Result<Point> {
    checked_point([
        matrix[0] * point[0] + matrix[4] * point[1] + matrix[8] * point[2] + matrix[12],
        matrix[1] * point[0] + matrix[5] * point[1] + matrix[9] * point[2] + matrix[13],
        matrix[2] * point[0] + matrix[6] * point[1] + matrix[10] * point[2] + matrix[14],
    ])
}
fn multiply(a: Matrix, b: Matrix) -> Result<Matrix> {
    let mut out = [0.0; 16];
    for row in 0..4 {
        for column in 0..4 {
            for k in 0..4 {
                out[column * 4 + row] += a[k * 4 + row] * b[column * 4 + k];
            }
        }
    }
    if out.iter().any(|v| !v.is_finite() || v.abs() > 1e12) {
        return Err(invalid("3D assembly transform overflows"));
    }
    Ok(out)
}
impl Scene {
    /// Decode the normal convention used by the reference U3D writer.
    /// Select `VectorDifferences` explicitly for a producer that follows the
    /// coordinate-difference wording in the published format tables. The two
    /// conventions can encode different unit normals with identical bytes.
    pub fn decode_u3d(bytes: &[u8]) -> Result<Self> {
        u3d::decode(bytes, NormalEncoding::ReferenceRotations)
    }
    /// Select the producer's normal prediction and difference convention.
    pub fn decode_u3d_with_normals(bytes: &[u8], encoding: NormalEncoding) -> Result<Self> {
        u3d::decode(bytes, encoding)
    }
    /// Resolve an instance's transformed triangle without allocating a second copy of its mesh.
    pub fn triangle(&self, instance: usize, triangle: usize) -> Result<[Point; 3]> {
        let instance = self.instances.get(instance).ok_or_else(|| invalid("no such 3D instance"))?;
        let mesh = self.meshes.get(instance.mesh).ok_or_else(|| invalid("3D instance refers to a missing mesh"))?;
        let indices = mesh.triangles.get(triangle).ok_or_else(|| invalid("no such 3D triangle"))?;
        let mut points = [[0.0; 3]; 3];
        for (point, index) in points.iter_mut().zip(indices) {
            *point =
                transform(instance.transform, *mesh.positions.get(*index as usize).ok_or_else(|| invalid("3D triangle index is out of bounds"))?)?;
        }
        Ok(points)
    }
    /// Resolve a point primitive through its model's actual instance transform.
    pub fn point(&self, instance: usize, point: usize) -> Result<Point> {
        let instance = self.instances.get(instance).ok_or_else(|| invalid("no such 3D instance"))?;
        let mesh = self.meshes.get(instance.mesh).ok_or_else(|| invalid("3D instance refers to a missing mesh"))?;
        let index = *mesh.points.get(point).ok_or_else(|| invalid("no such 3D point primitive"))?;
        transform(instance.transform, *mesh.positions.get(index as usize).ok_or_else(|| invalid("3D point primitive index is out of bounds"))?)
    }
    /// Resolve a line primitive without introducing any surface triangles.
    pub fn line(&self, instance: usize, line: usize) -> Result<[Point; 2]> {
        let instance = self.instances.get(instance).ok_or_else(|| invalid("no such 3D instance"))?;
        let mesh = self.meshes.get(instance.mesh).ok_or_else(|| invalid("3D instance refers to a missing mesh"))?;
        let indices = mesh.lines.get(line).ok_or_else(|| invalid("no such 3D line primitive"))?;
        let mut points = [[0.0; 3]; 2];
        for (point, index) in points.iter_mut().zip(indices) {
            *point = transform(
                instance.transform,
                *mesh.positions.get(*index as usize).ok_or_else(|| invalid("3D line primitive index is out of bounds"))?,
            )?;
        }
        Ok(points)
    }
    /// Return the closest visible mesh intersection, using the actual assembly transforms.
    pub fn pick(&self, origin: Point, direction: Point) -> Result<Option<Hit>> {
        checked_point(origin)?;
        checked_point(direction)?;
        let norm = dot(direction, direction).sqrt();
        if norm <= 1e-15 {
            return Err(invalid("3D pick ray needs a nonzero direction"));
        }
        let direction = direction.map(|v| v / norm);
        let mut closest: Option<Hit> = None;
        let mut work = 0usize;
        for (instance_index, instance) in self.instances.iter().enumerate() {
            if instance.visibility == 0 {
                continue;
            }
            let mesh = self.meshes.get(instance.mesh).ok_or_else(|| invalid("3D instance refers to a missing mesh"))?;
            for triangle in 0..mesh.triangles.len() {
                work += 1;
                if work > MAX_TRIANGLES {
                    return Err(invalid("3D pick exceeds one million transformed triangles"));
                }
                let [a, b, c] = self.triangle(instance_index, triangle)?;
                let e1 = sub(b, a);
                let e2 = sub(c, a);
                let p = cross(direction, e2);
                let determinant = dot(e1, p);
                let epsilon = 1e-12 * dot(e1, e1).sqrt() * dot(e2, e2).sqrt();
                if determinant.abs() <= epsilon
                    || (instance.visibility == 1 && determinant <= 0.0)
                    || (instance.visibility == 2 && determinant >= 0.0)
                {
                    continue;
                }
                let relative = sub(origin, a);
                let u = dot(relative, p) / determinant;
                let q = cross(relative, e1);
                let v = dot(direction, q) / determinant;
                if u < -1e-10 || v < -1e-10 || u + v > 1.0 + 1e-10 {
                    continue;
                }
                let distance = dot(e2, q) / determinant;
                if distance < 0.0 || !distance.is_finite() || closest.as_ref().is_some_and(|hit| distance >= hit.ray_distance) {
                    continue;
                }
                let point =
                    checked_point([origin[0] + direction[0] * distance, origin[1] + direction[1] * distance, origin[2] + direction[2] * distance])?;
                closest = Some(Hit { instance: instance_index, triangle, point, ray_distance: distance, barycentric: [1.0 - u - v, u, v] });
            }
        }
        Ok(closest)
    }
    pub fn bounds(&self) -> Result<Option<[Point; 2]>> {
        let mut min = [f64::INFINITY; 3];
        let mut max = [f64::NEG_INFINITY; 3];
        let mut count = 0usize;
        for instance in &self.instances {
            if instance.visibility == 0 {
                continue;
            }
            let mesh = self.meshes.get(instance.mesh).ok_or_else(|| invalid("3D instance refers to a missing mesh"))?;
            for p in &mesh.positions {
                count += 1;
                if count > MAX_POSITIONS {
                    return Err(invalid("3D bounds exceed one million transformed vertices"));
                }
                let p = transform(instance.transform, *p)?;
                for i in 0..3 {
                    min[i] = min[i].min(p[i]);
                    max[i] = max[i].max(p[i]);
                }
            }
        }
        Ok((count > 0).then_some([min, max]))
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn picking_uses_transforms_and_depth_and_honors_visibility() {
        let mesh = Mesh {
            name: "triangle".into(),
            positions: vec![[0.0, 0.0, 0.0], [10.0, 0.0, 0.0], [0.0, 10.0, 0.0]],
            triangles: vec![[0, 1, 2]],
            shading: vec![0],
            ..Mesh::default()
        };
        let mut matrix = IDENTITY;
        matrix[12] = 20.0;
        matrix[14] = 5.0;
        let mut scene = Scene {
            meshes: vec![mesh],
            instances: vec![
                Instance { name: "front".into(), mesh: 0, transform: matrix, visibility: 3 },
                Instance { name: "back".into(), mesh: 0, transform: IDENTITY, visibility: 3 },
            ],
            meters_per_unit: Some(0.001),
            normal_encoding: NormalEncoding::VectorDifferences,
        };
        let hit = scene.pick([21.0, 1.0, 20.0], [0.0, 0.0, -2.0]).unwrap().unwrap();
        assert_eq!(hit.point, [21.0, 1.0, 5.0]);
        assert_eq!(hit.instance, 0);
        assert_eq!(hit.ray_distance, 15.0);
        assert!(scene.pick([21.0, 1.0, 20.0], [0.0, 0.0, 0.0]).is_err());
        scene.instances[0].visibility = 0;
        assert!(scene.pick([21.0, 1.0, 20.0], [0.0, 0.0, -1.0]).unwrap().is_none());
        assert!(scene.triangle(99, 0).is_err());
    }
}

#[cfg(test)]
mod connected_surface_tests {
    use super::*;
    use std::collections::BTreeSet;
    fn canonical(mut triangle: [usize; 3]) -> [usize; 3] {
        if triangle[1] < triangle[0] && triangle[1] < triangle[2] {
            triangle.rotate_left(1);
        } else if triangle[2] < triangle[0] && triangle[2] < triangle[1] {
            triangle.rotate_left(2);
        }
        triangle
    }
    fn assert_grid(mesh: &Mesh) {
        let expected: Vec<Point> =
            (0..7).flat_map(|y| (0..8).map(move |x| [f64::from(3 * x), f64::from(4 * y), f64::from((x % 3) * (y % 4))])).collect();
        assert_eq!(mesh.positions.len(), expected.len());
        assert_eq!(mesh.triangles.len(), 84);
        let mut indices = Vec::new();
        for point in &mesh.positions {
            let nearest = expected.iter().enumerate().find(|(_, p)| sub(**p, *point).iter().all(|v| v.abs() < 0.001)).unwrap();
            indices.push(nearest.0);
        }
        assert_eq!(indices.iter().copied().collect::<BTreeSet<_>>().len(), 56);
        let actual: BTreeSet<_> = mesh.triangles.iter().map(|t| canonical(t.map(|i| indices[i as usize]))).collect();
        let mut faces = BTreeSet::new();
        for y in 0..6 {
            for x in 0..7 {
                let a = y * 8 + x;
                faces.insert(canonical([a, a + 1, a + 8]));
                faces.insert(canonical([a + 1, a + 9, a + 8]));
            }
        }
        assert_eq!(actual, faces, "decoded topology and winding must match the authored surface");
    }
    #[test]
    fn independently_compressed_connected_grid_preserves_vertices_faces_and_picking() {
        let compressed = Scene::decode_u3d(fixtures::GRID).unwrap();
        let raw = Scene::decode_u3d(fixtures::GRID_RAW).unwrap();
        assert_eq!(compressed, raw);
        assert_grid(&compressed.meshes[0]);
        let bounds = compressed.bounds().unwrap().unwrap();
        for (actual, expected) in bounds.into_iter().flatten().zip([20.0, 30.0, 5.0, 41.0, 54.0, 11.0]) {
            assert!((actual - expected).abs() < 0.001);
        }
        let hit = compressed.pick([20.75, 31.0, 15.0], [0.0, 0.0, -1.0]).unwrap().unwrap();
        assert!((hit.point[2] - 5.0).abs() < 0.001);
    }
    #[test]
    fn independently_encoded_shared_resource_and_nonuniform_assembly_transforms() {
        let scene = Scene::decode_u3d(fixtures::ASSEMBLY).unwrap();
        assert_eq!(scene.meshes.len(), 1);
        assert_eq!(scene.instances.len(), 3);
        assert_grid(&scene.meshes[0]);
        let origin = [0.0, 0.0, 0.0];
        let actual: BTreeSet<_> = scene.instances.iter().map(|i| transform(i.transform, origin).unwrap().map(|x| x.round() as i64)).collect();
        assert_eq!(actual, BTreeSet::from([[20, 30, 5], [40, 5, 2], [30, -45, -69]]));
        let nested = scene.instances.iter().position(|i| transform(i.transform, origin).unwrap() == [30.0, -45.0, -69.0]).unwrap();
        let p = transform(scene.instances[nested].transform, [3.0, 4.0, 6.0]).unwrap();
        assert_eq!(p, [22.0, -36.0, -66.0]);
    }
}

#[cfg(test)]
mod normal_tests {
    use super::*;
    #[test]
    fn independent_reader_matches_every_connected_surface_normal_and_index() {
        let scene = Scene::decode_u3d(fixtures::GRID_NORMALS).unwrap();
        assert_eq!(scene.normal_encoding, NormalEncoding::ReferenceRotations);
        assert_eq!(scene.meshes.len(), 1);
        let mesh = &scene.meshes[0];
        assert_eq!(mesh.positions.len(), 56);
        assert_eq!(mesh.normals.len(), fixtures::GRID_NORMAL_VALUES.len());
        for (index, (actual, expected)) in mesh.normals.iter().zip(fixtures::GRID_NORMAL_VALUES).enumerate() {
            for (a, e) in actual.iter().zip(expected) {
                assert!((a - f64::from(e) / 1_000_000.0).abs() < 0.000001, "normal {index}: {actual:?} != {expected:?}");
            }
        }
        assert_eq!(mesh.triangles.len(), fixtures::GRID_FACE_VALUES.len());
        assert_eq!(mesh.attributes.len(), mesh.triangles.len());
        for ((positions, attributes), expected) in mesh.triangles.iter().zip(&mesh.attributes).zip(fixtures::GRID_FACE_VALUES) {
            assert_eq!(*positions, [expected[0], expected[1], expected[2]]);
            assert_eq!(attributes.normals, Some([expected[3], expected[4], expected[5]]));
        }
        for end in 0..fixtures::GRID_NORMALS.len() {
            assert!(Scene::decode_u3d(&fixtures::GRID_NORMALS[..end]).is_err());
        }
    }
    #[test]
    fn reference_axis_normals_include_zero_difference_negative_hemisphere() {
        for (bytes, expected) in [
            (fixtures::NORMAL_X, [1.0, 0.0, 0.0]),
            (fixtures::NORMAL_Y, [0.0, 1.0, 0.0]),
            (fixtures::NORMAL_NEG_X, [-1.0, 0.0, 0.0]),
            (fixtures::NORMAL_NEG_Y, [0.0, -1.0, 0.0]),
            (fixtures::NORMAL_NEG_Z, [0.0, 0.0, -1.0]),
        ] {
            let scene = Scene::decode_u3d(bytes).unwrap();
            let mesh = &scene.meshes[0];
            assert_eq!(mesh.triangles.len(), 1);
            assert!(!mesh.normals.is_empty());
            for normal in &mesh.normals {
                assert!(sub(*normal, expected).iter().all(|v| v.abs() < 0.000001), "{normal:?} != {expected:?}");
            }
        }
        // Identical bytes have two valid interpretations. Unit length cannot
        // detect this half-turn, so selection is an explicit producer convention.
        let vector = Scene::decode_u3d_with_normals(fixtures::NORMAL_NEG_Z, NormalEncoding::VectorDifferences).unwrap();
        assert!(vector.meshes[0].normals.iter().all(|n| *n == [0.0, 0.0, 1.0]));
    }
}

#[cfg(test)]
mod decode_budget_tests {
    use super::*;
    #[test]
    fn blocks_and_topology_share_their_work_allowance() {
        let budget = std::rc::Rc::new(DecodeBudget::default());
        budget.work.set(MAX_DECODE_WORK - 1);
        let mut first = bitstream::Decoder::with_budget(&[0, 0, 0, 0], true, budget.clone());
        assert_eq!(first.u8().unwrap(), 0);
        let mut second = bitstream::Decoder::with_budget(&[0, 0, 0, 0], true, budget.clone());
        assert!(second.u8().is_err());
        assert!(budget.spend(1).is_err());
        assert_eq!(budget.work.get(), MAX_DECODE_WORK);
    }
    #[test]
    fn multiple_resources_cannot_reset_memory_or_record_counts() {
        let budget = std::rc::Rc::new(DecodeBudget::default());
        let other_resource = budget.clone();
        budget.reserve(RecordKind::Normal, 600_000, 24).unwrap();
        assert!(other_resource.reserve(RecordKind::Normal, 600_000, 24).is_err());
        assert_eq!(budget.records[1].get(), 600_000);
        assert_eq!(budget.bytes.get(), 600_000 * 24);
        budget.allocate(MAX_DECODE_RECORD_BYTES - budget.bytes.get()).unwrap();
        assert!(other_resource.reserve(RecordKind::Position, 1, 24).is_err());
        assert_eq!(budget.records[0].get(), 0);
        assert_eq!(budget.bytes.get(), MAX_DECODE_RECORD_BYTES);
        assert!(budget.allocate(usize::MAX).is_err());
        assert!(budget.reserve(RecordKind::Attribute, usize::MAX, 32).is_err());
    }
}

#[cfg(test)]
mod primitive_tests {
    use super::*;
    #[test]
    fn independent_point_and_line_geometry_colors_and_normal_absence() {
        for (compressed, raw, points, colors, expected_colors) in [
            (fixtures::POINTS_COLORS_COMPRESSED, fixtures::POINTS_COLORS_RAW, true, true, fixtures::POINTS_COLOR_VALUES),
            (fixtures::LINES_COLORS_COMPRESSED, fixtures::LINES_COLORS_RAW, false, true, fixtures::LINES_COLOR_VALUES),
            (fixtures::POINTS_NO_NORMALS_COMPRESSED, fixtures::POINTS_NO_NORMALS_RAW, true, false, &[][..]),
            (fixtures::LINES_NO_NORMALS_COMPRESSED, fixtures::LINES_NO_NORMALS_RAW, false, false, &[][..]),
        ] {
            let scene = Scene::decode_u3d(compressed).unwrap();
            assert_eq!(scene, Scene::decode_u3d(raw).unwrap());
            let mesh = &scene.meshes[0];
            assert!(mesh.triangles.is_empty());
            assert_eq!(mesh.positions.len(), 6);
            let positions = [[0., 0., 0.], [3., 0., 0.], [3., 4., 0.], [0., 4., 0.], [0., 0., 12.], [3., 4., 12.]];
            for (actual, expected) in mesh.positions.iter().zip(positions) {
                assert!(sub(*actual, expected).iter().all(|v| v.abs() < 0.0001));
            }
            assert_eq!(mesh.diffuse.len(), expected_colors.len());
            for (actual, expected) in mesh.diffuse.iter().zip(expected_colors) {
                for (a, e) in actual.iter().zip(expected) {
                    assert!((a - f64::from(*e) / 1_000_000.).abs() < 0.000001);
                }
            }
            if points {
                assert_eq!(mesh.points, vec![0, 0, 1, 2, 3, 4, 5]);
                assert!(mesh.lines.is_empty());
                for (index, attributes) in mesh.point_attributes.iter().enumerate() {
                    assert_eq!(attributes.normals, colors.then_some([index as u32]));
                    assert_eq!(attributes.diffuse, colors.then_some([index.saturating_sub(1) as u32]));
                    assert!(attributes.specular.is_none() && attributes.textures.is_empty());
                }
                assert_eq!(scene.point(0, 0).unwrap(), [20., 30., 5.]);
                assert!(scene.point(0, 7).is_err());
            } else {
                assert_eq!(mesh.lines, vec![[0, 1], [1, 2], [2, 3], [0, 3], [0, 4], [2, 5], [4, 5], [0, 5]]);
                assert!(mesh.points.is_empty());
                let diffuse = [[0, 1], [1, 2], [2, 3], [4, 5], [6, 7], [8, 9], [10, 11], [12, 13]];
                for (index, attributes) in mesh.line_attributes.iter().enumerate() {
                    assert_eq!(attributes.normals, colors.then_some([2 * index as u32, 2 * index as u32 + 1]));
                    assert_eq!(attributes.diffuse, colors.then_some(diffuse[index]));
                    assert!(attributes.specular.is_none() && attributes.textures.is_empty());
                }
                let [a, b] = scene.line(0, 7).unwrap();
                assert_eq!(a, [20., 30., 5.]);
                assert!((dot(sub(b, a), sub(b, a)).sqrt() - 13.).abs() < 0.0001);
                assert!(scene.line(0, 8).is_err());
            }
            assert_eq!(mesh.normals.len(), if colors { if points { 7 } else { 16 } } else { 0 });
            assert!(mesh.normals.iter().all(|n| *n == [0., 0., 1.]));
            assert!(scene.pick([21., 31., 30.], [0., 0., -1.]).unwrap().is_none());
            for bytes in [compressed, raw] {
                for end in 0..bytes.len() {
                    assert!(Scene::decode_u3d(&bytes[..end]).is_err());
                }
            }
        }
    }
}
