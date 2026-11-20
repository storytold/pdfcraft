#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use super::*;
use crate::{Projection, RenderOptions, render};
use pdfcraft_prc::TriangleMesh;

/// A cube of half-size 1 at the origin, each face wound counter-clockwise from outside: faces
/// +x, -x, +y, -y, +z, -z, two triangles each.
pub(crate) fn cube_mesh() -> TriangleMesh {
    let mut mesh = TriangleMesh::default();
    for axis in 0..3 {
        for sign in [1.0, -1.0] {
            let mut c = [0.0; 3];
            c[axis] = sign;
            let (mut u, mut v) = ([0.0; 3], [0.0; 3]);
            u[(axis + 1) % 3] = 1.0;
            v[(axis + 2) % 3] = 1.0;
            let mut quad = [(-1.0, -1.0), (1.0, -1.0), (1.0, 1.0), (-1.0, 1.0)];
            if sign < 0.0 {
                quad.reverse();
            }
            let base = u32::try_from(mesh.positions.len()).unwrap();
            for (a, b) in quad {
                mesh.positions.push(add(c, add(scale(u, a), scale(v, b))));
            }
            mesh.triangles.push([base, base + 1, base + 2]);
            mesh.triangles.push([base, base + 2, base + 3]);
        }
    }
    mesh
}

pub(crate) fn cube() -> WorldMesh {
    WorldMesh::new(cube_mesh())
}

fn camera(eye: [f64; 3], projection: Projection) -> Camera {
    Camera { eye, target: [0.0; 3], up: [0.0, 1.0, 0.0], projection }
}

fn near(a: [f64; 3], b: [f64; 3]) -> bool {
    a.iter().zip(b).all(|(x, y)| (x - y).abs() < 1e-9)
}

#[test]
fn the_centre_pixel_hits_the_facing_face_with_its_outward_normal() {
    let cam = camera([0.0, 0.0, 10.0], Projection::Perspective { fov_y: 30.0 });
    let hit = pick(&[cube()], &cam.ray_through(50.0, 50.0, 100, 100).unwrap()).expect("hits");
    assert!(near(hit.point, [0.0, 0.0, 1.0]), "{hit:?}");
    assert!(near(hit.normal, [0.0, 0.0, 1.0]), "{hit:?}");
    assert!((hit.distance - 9.0).abs() < 1e-9);
    assert!((8..10).contains(&hit.triangle), "the +z face is triangles 8 and 9");
}

/// A hit projects back onto the position it was picked from, under both projections and a
/// non-square image.
#[test]
fn a_hit_projects_back_onto_its_pixel() {
    for projection in [Projection::Perspective { fov_y: 40.0 }, Projection::Orthographic { height: 4.0 }] {
        let cam = camera([6.0, 5.0, 8.0], projection);
        let (w, h) = (160, 90);
        let view = View::new(&cam, f64::from(w) / f64::from(h)).unwrap();
        let [right, up, forward] = cam.basis().unwrap();
        for (x, y) in [(80.5, 45.5), (75.5, 40.5), (88.5, 50.5)] {
            let hit = pick(&[cube()], &cam.ray_through(x, y, w, h).unwrap()).expect("hits");
            let d = sub(hit.point, cam.eye);
            let at = view.project([dot(d, right), dot(d, up), dot(d, forward), 0.0, 0.0, 0.0], w, h).at;
            assert!((at[0] - x).abs() < 1e-6 && (at[1] - y).abs() < 1e-6, "{projection:?}: {at:?} for {x}, {y}");
        }
    }
}

/// Every pixel the renderer paints picks a triangle, and every background pixel picks none.
#[test]
fn a_pick_hits_exactly_where_the_render_draws() {
    for projection in [Projection::Perspective { fov_y: 30.0 }, Projection::Orthographic { height: 3.0 }] {
        let cam = camera([4.0, 3.0, 5.0], projection);
        let (w, h) = (48, 36);
        let options = RenderOptions { width: w, height: h, background: [0, 0, 0, 0], ..RenderOptions::default() };
        let image = render(&[cube()], &cam, &options).unwrap();
        let mut drawn = 0;
        for j in 0..h {
            for i in 0..w {
                let alpha = image.rgba[((j * w + i) * 4 + 3) as usize];
                let ray = cam.ray_through(f64::from(i) + 0.5, f64::from(j) + 0.5, w, h).unwrap();
                assert_eq!(pick(&[cube()], &ray).is_some(), alpha != 0, "{projection:?} pixel {i}, {j}");
                drawn += usize::from(alpha != 0);
            }
        }
        assert!(drawn > 100, "the cube fills part of the picture");
    }
}

#[test]
fn the_nearer_face_wins_and_a_ray_past_the_model_misses() {
    let cam = camera([0.0, 0.0, -10.0], Projection::Orthographic { height: 4.0 });
    let hit = pick(&[cube()], &cam.ray_through(50.0, 50.0, 100, 100).unwrap()).unwrap();
    assert!(near(hit.normal, [0.0, 0.0, -1.0]), "{hit:?}");
    assert!(near(hit.point, [0.0, 0.0, -1.0]));
    assert_eq!(pick(&[cube()], &cam.ray_through(1.0, 1.0, 100, 100).unwrap()), None);
}

#[test]
fn behind_the_perspective_eye_is_not_hit() {
    let cam = Camera { target: [0.0, 0.0, -5.0], ..camera([0.0; 3], Projection::Perspective { fov_y: 30.0 }) };
    let hit = pick(&[cube()], &cam.ray_through(50.0, 50.0, 100, 100).unwrap()).unwrap();
    assert!(near(hit.point, [0.0, 0.0, -1.0]), "the face in front, not behind");
    assert!(near(hit.normal, [0.0, 0.0, -1.0]), "a back face keeps its own normal");
}

/// The renderer does not draw a fully transparent triangle, so a pick passes through it.
#[test]
fn a_fully_transparent_triangle_is_picked_through() {
    let cam = camera([0.0, 0.0, 10.0], Projection::Orthographic { height: 4.0 });
    let ray = cam.ray_through(50.0, 50.0, 100, 100).unwrap();
    let mut clear = cube();
    clear.triangle_colours = Some(vec![None, None, None, None, None, None, None, None, Some([9, 9, 9, 0]), Some([9, 9, 9, 0])]);
    let hit = pick(&[clear], &ray).expect("the far face");
    assert!(near(hit.point, [0.0, 0.0, -1.0]), "{hit:?}");
    assert!((10..12).contains(&hit.triangle));
}

#[test]
fn a_tie_goes_to_the_first_mesh() {
    let cam = camera([0.0, 0.0, 10.0], Projection::Orthographic { height: 4.0 });
    let hit = pick(&[cube(), cube()], &cam.ray_through(50.0, 50.0, 100, 100).unwrap()).unwrap();
    assert_eq!(hit.mesh, 0);
}

#[test]
fn a_degenerate_image_or_position_is_refused() {
    let cam = camera([0.0, 0.0, 10.0], Projection::Perspective { fov_y: 30.0 });
    assert!(matches!(cam.ray_through(0.0, 0.0, 0, 10), Err(RenderError::Size { .. })));
    for (x, y) in [(f64::NAN, 1.0), (1.0, f64::INFINITY)] {
        let ortho = camera([0.0, 0.0, 10.0], Projection::Orthographic { height: 4.0 });
        for c in [&cam, &ortho] {
            let said = c.ray_through(x, y, 10, 10);
            assert!(matches!(said, Err(RenderError::Camera("the image position is not a finite number"))), "{x}, {y}: {said:?}");
        }
    }
    // An orthographic ray whose start overflows: the picture's right is +x, and the eye is at the float range.
    let far = Camera { eye: [f64::MAX, 0.0, f64::MAX], ..camera([0.0; 3], Projection::Orthographic { height: f64::MAX }) };
    assert!(matches!(far.ray_through(9.5, 5.0, 10, 10), Err(RenderError::Camera(_))));
    let bad = Camera { target: [0.0, 0.0, 10.0], ..cam };
    assert!(matches!(bad.ray_through(1.0, 1.0, 10, 10), Err(RenderError::Camera(_))));
}

/// A triangle near the float range is hit without an overflowed point, or missed; never a
/// non-finite hit.
#[test]
fn a_model_near_the_float_range_gives_no_non_finite_hit() {
    let big = f64::MAX / 2.0;
    let mut mesh = TriangleMesh::default();
    mesh.positions = vec![[-big, -big, -big], [big, -big, big], [0.0, big, 0.0]];
    mesh.triangles = vec![[0, 1, 2]];
    let ray = Ray { origin: [0.0, 0.0, f64::MAX], direction: [0.0, 0.0, -1.0], min_distance: f64::NEG_INFINITY };
    if let Some(hit) = pick(&[WorldMesh::new(mesh)], &ray) {
        assert!(hit.point.iter().chain(&hit.normal).all(|c| c.is_finite()) && hit.distance.is_finite(), "{hit:?}");
    }
}
