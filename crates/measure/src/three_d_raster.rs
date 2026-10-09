//! Bounded depth-buffered previews of real transformed model geometry.
use crate::{
    Result, invalid,
    three_d::{Point, Scene, cross, direction, dot, subtract},
    three_d_camera::{Camera, Projection},
};
#[derive(Clone, Debug)]
pub struct Raster {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
    pub triangles: usize,
    pub points: usize,
    pub lines: usize,
}
#[derive(Clone, Copy, Debug)]
pub struct Options {
    pub width: u32,
    pub height: u32,
    pub background: [u8; 3],
    pub surface: [u8; 3],
}
impl Default for Options {
    fn default() -> Self {
        Self { width: 640, height: 480, background: [245, 246, 249], surface: [109, 156, 191] }
    }
}
const MAX_WORK: usize = 200_000_000;
struct Painter {
    raster: Raster,
    depth: Vec<f64>,
    work: usize,
}
impl Painter {
    fn spend(&mut self, n: usize) -> Result<()> {
        self.work = self.work.saturating_add(n);
        if self.work > MAX_WORK { Err(invalid("3D preview exceeds 200 million raster operations")) } else { Ok(()) }
    }
    fn pixel(&mut self, x: i64, y: i64, depth: f64, color: [u8; 3]) -> Result<()> {
        self.spend(1)?;
        if x < 0 || y < 0 || x >= i64::from(self.raster.width) || y >= i64::from(self.raster.height) || !depth.is_finite() {
            return Ok(());
        };
        let index = y as usize * self.raster.width as usize + x as usize;
        let previous = self.depth.get_mut(index).ok_or_else(|| invalid("3D depth buffer index is out of bounds"))?;
        if depth >= *previous {
            return Ok(());
        };
        *previous = depth;
        let pixel = self.raster.rgba.get_mut(index * 4..index * 4 + 4).ok_or_else(|| invalid("3D pixel buffer index is out of bounds"))?;
        pixel.copy_from_slice(&[color[0], color[1], color[2], 255]);
        Ok(())
    }
    fn triangle(&mut self, vertices: [Point; 3], perspective: bool, color: [u8; 3]) -> Result<()> {
        let edge = |a: Point, b: Point, p: Point| (b[0] - a[0]) * (p[1] - a[1]) - (b[1] - a[1]) * (p[0] - a[0]);
        let [a, b, c] = vertices;
        let area = edge(a, b, c);
        if area.abs() < 1e-12 {
            return Ok(());
        };
        let min_x = vertices.iter().map(|p| p[0]).fold(f64::INFINITY, f64::min).floor().max(0.) as i64;
        let min_y = vertices.iter().map(|p| p[1]).fold(f64::INFINITY, f64::min).floor().max(0.) as i64;
        let max_x = vertices.iter().map(|p| p[0]).fold(f64::NEG_INFINITY, f64::max).ceil().min(f64::from(self.raster.width) - 1.) as i64;
        let max_y = vertices.iter().map(|p| p[1]).fold(f64::NEG_INFINITY, f64::max).ceil().min(f64::from(self.raster.height) - 1.) as i64;
        if max_x < min_x || max_y < min_y {
            return Ok(());
        };
        self.spend(((max_x - min_x + 1) as usize).saturating_mul((max_y - min_y + 1) as usize))?;
        for y in min_y..=max_y {
            for x in min_x..=max_x {
                let p = [x as f64 + 0.5, y as f64 + 0.5, 0.];
                let weights = [edge(b, c, p) / area, edge(c, a, p) / area, edge(a, b, p) / area];
                if weights.iter().any(|v| *v < -1e-12) {
                    continue;
                };
                let depth = if perspective {
                    1. / weights.iter().zip(vertices).map(|(w, p)| w / p[2]).sum::<f64>()
                } else {
                    weights.iter().zip(vertices).map(|(w, p)| w * p[2]).sum()
                };
                self.pixel(x, y, depth, color)?;
            }
        }
        Ok(())
    }
    fn line(&mut self, a: Point, b: Point, perspective: bool, color: [u8; 3]) -> Result<()> {
        let steps = (b[0] - a[0]).abs().max((b[1] - a[1]).abs()).ceil() as usize;
        self.spend(steps.saturating_add(1))?;
        for step in 0..=steps {
            let t = if steps == 0 { 0. } else { step as f64 / steps as f64 };
            let x = (a[0] + (b[0] - a[0]) * t).floor() as i64;
            let y = (a[1] + (b[1] - a[1]) * t).floor() as i64;
            let z = if perspective { 1. / ((1. - t) / a[2] + t / b[2]) } else { a[2] + (b[2] - a[2]) * t };
            self.pixel(x, y, z, color)?;
        }
        Ok(())
    }
}
fn planes(camera: &Camera) -> Vec<[f64; 4]> {
    let k = camera.scale();
    let perspective = matches!(camera.projection, Projection::Perspective { .. });
    let [x, y] = camera.target.map(|v| v / 2.);
    let [zx, zy, w_x, w_y] = if perspective { [x, y, 0., 0.] } else { [0., 0., x, y] };
    let mut out = vec![[0., 0., 1., -camera.near], [k, 0., zx, w_x], [-k, 0., zx, w_x], [0., k, zy, w_y], [0., -k, zy, w_y]];
    if let Some(far) = camera.far {
        out.push([0., 0., -1., far]);
    }
    out
}
fn side(plane: [f64; 4], p: Point) -> f64 {
    plane[0] * p[0] + plane[1] * p[1] + plane[2] * p[2] + plane[3]
}
fn interpolate(a: Point, b: Point, t: f64) -> Point {
    [a[0] + (b[0] - a[0]) * t, a[1] + (b[1] - a[1]) * t, a[2] + (b[2] - a[2]) * t]
}
fn clip_polygon(mut vertices: Vec<Point>, planes: &[[f64; 4]]) -> Result<Vec<Point>> {
    for plane in planes {
        let Some(mut previous) = vertices.last().copied() else { return Ok(vertices) };
        let mut previous_side = side(*plane, previous);
        let mut next = Vec::new();
        for vertex in &vertices {
            let current_side = side(*plane, *vertex);
            if (current_side >= 0.) != (previous_side >= 0.) {
                let t = previous_side / (previous_side - current_side);
                next.push(interpolate(previous, *vertex, t));
            }
            if current_side >= 0. {
                next.push(*vertex);
            }
            previous = *vertex;
            previous_side = current_side;
        }
        if next.len() > 16 {
            return Err(invalid("3D triangle clipping exceeds its polygon bound"));
        }
        vertices = next;
    }
    Ok(vertices)
}
fn clip_line(mut a: Point, mut b: Point, planes: &[[f64; 4]]) -> Option<[Point; 2]> {
    for plane in planes {
        let x = side(*plane, a);
        let y = side(*plane, b);
        if x < 0. && y < 0. {
            return None;
        };
        if (x >= 0.) != (y >= 0.) {
            let p = interpolate(a, b, x / (x - y));
            if x < 0. {
                a = p;
            } else {
                b = p;
            }
        }
    }
    Some([a, b])
}
/// A failed preview returns an error, so callers never mistake a partial image for a complete one.
pub fn render(scene: &Scene, camera: &Camera, options: Options) -> Result<Raster> {
    camera.validate()?;
    let count = (options.width as usize)
        .checked_mul(options.height as usize)
        .filter(|n| *n <= 4 * 1024 * 1024)
        .ok_or_else(|| invalid("3D preview exceeds four million pixels"))?;
    if options.width == 0 || options.height == 0 || options.width > 2048 || options.height > 2048 {
        return Err(invalid("3D preview dimensions must be between 1 and 2048"));
    }
    let mut rgba = Vec::with_capacity(count * 4);
    for _ in 0..count {
        rgba.extend([options.background[0], options.background[1], options.background[2], 255]);
    }
    let mut painter = Painter {
        raster: Raster { width: options.width, height: options.height, rgba, triangles: 0, points: 0, lines: 0 },
        depth: vec![f64::INFINITY; count],
        work: 0,
    };
    let viewport = [f64::from(options.width), f64::from(options.height)];
    let perspective = matches!(camera.projection, Projection::Perspective { .. });
    let clipping = planes(camera);
    let light = direction(subtract(camera.eye(), camera.center))?;
    for (instance_index, instance) in scene.instances.iter().enumerate() {
        if instance.visibility == 0 {
            continue;
        };
        let mesh = scene.meshes.get(instance.mesh).ok_or_else(|| invalid("3D instance refers to a missing mesh"))?;
        for index in 0..mesh.triangles.len() {
            painter.spend(1)?;
            let world = scene.triangle(instance_index, index).map_err(|e| invalid(&e.to_string()))?;
            let first = match direction(subtract(world[1], world[0])) {
                Ok(v) => v,
                Err(_) => continue,
            };
            let second = match direction(subtract(world[2], world[0])) {
                Ok(v) => v,
                Err(_) => continue,
            };
            let normal = cross(first, second);
            let normal = match direction(normal) {
                Ok(normal) => normal,
                Err(_) => continue,
            };
            let facing = dot(normal, subtract(camera.eye(), world[0]));
            if (instance.visibility == 1 && facing <= 0.) || (instance.visibility == 2 && facing >= 0.) {
                continue;
            };
            let intensity = 0.3 + 0.7 * dot(normal, light).abs();
            let color = options.surface.map(|v| (f64::from(v) * intensity).round() as u8);
            let vertices = world.into_iter().map(|p| camera.world_to_camera(p)).collect::<Result<Vec<_>>>()?;
            let polygon = clip_polygon(vertices, &clipping)?;
            if polygon.len() < 3 {
                continue;
            };
            let vertices = polygon
                .iter()
                .map(|p| camera.project_camera(*p, viewport)?.ok_or_else(|| invalid("clipped 3D triangle escaped its depth range")))
                .collect::<Result<Vec<_>>>()?;
            let first = *vertices.first().ok_or_else(|| invalid("3D clipped triangle is empty"))?;
            for (a, b) in vertices.iter().skip(1).zip(vertices.iter().skip(2)) {
                painter.triangle([first, *a, *b], perspective, color)?;
            }
            painter.raster.triangles += 1;
        }
        for index in 0..mesh.lines.len() {
            painter.spend(1)?;
            let [a, b] = scene.line(instance_index, index).map_err(|e| invalid(&e.to_string()))?;
            if let Some([a, b]) = clip_line(camera.world_to_camera(a)?, camera.world_to_camera(b)?, &clipping) {
                let a = camera.project_camera(a, viewport)?.ok_or_else(|| invalid("clipped 3D line escaped its depth range"))?;
                let b = camera.project_camera(b, viewport)?.ok_or_else(|| invalid("clipped 3D line escaped its depth range"))?;
                painter.line(a, b, perspective, options.surface)?;
                painter.raster.lines += 1;
            }
        }
        for index in 0..mesh.points.len() {
            painter.spend(1)?;
            let world = scene.point(instance_index, index).map_err(|e| invalid(&e.to_string()))?;
            if let Some(p) = camera.project(world, viewport)? {
                let [x, y] = [p[0].floor() as i64, p[1].floor() as i64];
                if x >= 0 && y >= 0 && x < i64::from(options.width) && y < i64::from(options.height) {
                    for yy in -2..=2 {
                        for xx in -2..=2 {
                            if xx * xx + yy * yy <= 4 {
                                painter.pixel(x + xx, y + yy, p[2], options.surface)?;
                            }
                        }
                    }
                    painter.raster.points += 1;
                }
            }
        }
    }
    Ok(painter.raster)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::three_d_camera::{Binding, Projection};
    use pdfcraft_model::three_d::{IDENTITY, Instance, Mesh, NormalEncoding};
    fn camera() -> Camera {
        Camera {
            matrix: [1., 0., 0., 0., 1., 0., 0., 0., 1., 0., 0., -10.],
            center: [0.; 3],
            target: [100., 100.],
            projection: Projection::Perspective { field_of_view: 90., binding: Binding::Width },
            near: 1.,
            far: None,
        }
    }
    fn scene() -> Scene {
        Scene {
            meshes: vec![Mesh {
                name: "original".into(),
                positions: vec![[0., 0., 0.], [10., 0., 0.], [0., 10., 0.], [0., 0., 5.], [10., 0., 15.], [0., 10., 5.]],
                triangles: vec![[0, 1, 2], [3, 4, 5]],
                ..Mesh::default()
            }],
            instances: vec![Instance { name: "part".into(), mesh: 0, transform: IDENTITY, visibility: 3 }],
            meters_per_unit: None,
            normal_encoding: NormalEncoding::VectorDifferences,
        }
    }
    #[test]
    fn depth_buffer_preserves_front_surface_in_both_draw_orders_and_visibility() {
        let mut scene = scene();
        let camera = camera();
        let options = Options { width: 100, height: 100, background: [255; 3], surface: [200, 100, 50] };
        let first = render(&scene, &camera, options).unwrap();
        let index = (44 * 100 + 55) * 4;
        assert_eq!(&first.rgba[index..index + 4], &[200, 100, 50, 255]);
        assert_eq!(first.triangles, 2);
        scene.meshes[0].triangles.reverse();
        let second = render(&scene, &camera, options).unwrap();
        assert_eq!(first.rgba, second.rgba);
        scene.instances[0].visibility = 0;
        let hidden = render(&scene, &camera, options).unwrap();
        assert!(hidden.rgba.as_chunks::<4>().0.iter().all(|p| *p == [255; 4]));
        assert_eq!(hidden.triangles, 0);
    }
    #[test]
    fn triangle_and_line_near_far_and_viewport_clipping_remain_bounded() {
        let mut scene = scene();
        scene.meshes[0].positions = vec![[-100., -100., -9.5], [100., -100., 0.], [0., 100., 0.]];
        scene.meshes[0].triangles = vec![[0, 1, 2]];
        let camera = camera();
        let frame = render(&scene, &camera, Options::default()).unwrap();
        assert_eq!(frame.triangles, 1);
        assert!(frame.rgba.as_chunks::<4>().0.iter().any(|p| p[..3] != [245, 246, 249]));
        scene.meshes[0].positions = vec![[-100., 0., -9.5], [100., 0., 0.], [0., 0., 0.]];
        scene.meshes[0].triangles.clear();
        scene.meshes[0].lines = vec![[0, 1]];
        scene.meshes[0].points = vec![1, 2];
        let frame = render(&scene, &camera, Options::default()).unwrap();
        assert_eq!(frame.lines, 1);
        let mut camera = camera;
        camera.far = Some(8.);
        let frame = render(&scene, &camera, Options::default()).unwrap();
        assert_eq!(frame.lines, 1);
        assert_eq!(frame.points, 0);
        assert!(render(&scene, &camera, Options { width: 0, ..Options::default() }).is_err());
        assert!(render(&scene, &camera, Options { width: 2049, ..Options::default() }).is_err());
        let mut painter = Painter {
            raster: Raster { width: 1, height: 1, rgba: vec![255; 4], triangles: 0, points: 0, lines: 0 },
            depth: vec![f64::INFINITY],
            work: MAX_WORK,
        };
        assert!(painter.pixel(0, 0, 1., [0; 3]).is_err());
        assert_eq!(painter.raster.rgba, vec![255; 4]);
    }
}
