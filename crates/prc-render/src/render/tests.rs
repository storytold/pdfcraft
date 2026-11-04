#![allow(clippy::unwrap_used, clippy::panic, clippy::unreachable)]

use super::*;

fn mesh(positions: Vec<[f64; 3]>, triangles: Vec<[u32; 3]>) -> TriangleMesh {
    let mut m = TriangleMesh::default();
    m.positions = positions;
    m.triangles = triangles;
    m
}

fn quad_mesh(z: f64, half: f64, tilt: f64) -> TriangleMesh {
    mesh(vec![[-half, -half, z - tilt], [half, -half, z + tilt], [half, half, z + tilt], [-half, half, z - tilt]], vec![[0, 1, 2], [0, 2, 3]])
}

fn quad(z: f64, half: f64, tilt: f64) -> WorldMesh {
    WorldMesh::new(quad_mesh(z, half, tilt))
}

fn painted(m: WorldMesh, colour: Option<[u8; 4]>) -> WorldMesh {
    WorldMesh { colour, ..m }
}

fn ortho(height: f64) -> Camera {
    Camera { eye: [0.0, 0.0, 10.0], target: [0.0, 0.0, 0.0], up: [0.0, 1.0, 0.0], projection: Projection::Orthographic { height } }
}

fn small() -> RenderOptions {
    RenderOptions { width: 40, height: 40, ..RenderOptions::default() }
}

fn pixel(image: &Image, x: u32, y: u32) -> [u8; 4] {
    let i = (y * image.width + x) as usize * 4;
    image.rgba[i..i + 4].try_into().unwrap()
}

#[test]
fn a_facing_square_fills_the_middle_and_leaves_the_corners() {
    let image = render(&[quad(0.0, 1.0, 0.0)], &ortho(4.0), &small()).unwrap();
    assert_eq!(pixel(&image, 20, 20), [190, 192, 200, 255]);
    assert_eq!(pixel(&image, 1, 1), [255, 255, 255, 255]);
    // Half the view height is covered: pixels 10..30.
    assert_eq!(pixel(&image, 10, 20)[0], 190);
    assert_eq!(pixel(&image, 9, 20)[0], 255);
}

#[test]
fn stored_normals_shade_smoothly_across_a_flat_square() {
    let mut m = quad_mesh(0.0, 1.0, 0.0);
    let s = 3f64.sqrt() / 2.0;
    // The left corners lean 60° away from the viewer; the right face it.
    m.normals = vec![[s, 0.0, 0.5], [0.0, 0.0, 2.0]];
    m.triangle_normals = vec![[0, 1, 1], [0, 1, 0]];
    let image = render(&[WorldMesh::new(m.clone())], &ortho(4.0), &small()).unwrap();
    let red = |x| pixel(&image, x, 20)[0];
    assert!(red(11) < red(20) && red(20) < red(29), "{} {} {}", red(11), red(20), red(29));
    // Shade 0.3 + 0.7 cos θ at the edges: 0.65 left, 1.0 right.
    assert!((i32::from(red(10)) - 128).abs() <= 4, "{}", red(10));
    assert!(red(29) >= 186);
    m.normals.clear();
    m.triangle_normals.clear();
    let flat = render(&[WorldMesh::new(m)], &ortho(4.0), &small()).unwrap();
    assert_eq!(pixel(&flat, 11, 20), pixel(&flat, 29, 20));
}

#[test]
fn the_nearer_surface_hides_the_farther_whatever_the_order() {
    let far = quad(-1.0, 2.0, 1.0);
    let near = quad(1.0, 0.5, 0.0);
    for meshes in [[far.clone(), near.clone()], [near, far]] {
        let image = render(&meshes, &ortho(8.0), &small()).unwrap();
        assert_eq!(pixel(&image, 20, 20), [190, 192, 200, 255]);
        assert!(pixel(&image, 20, 12)[0] < 190, "the tilted far square is darker");
    }
}

const RED: Option<[u8; 4]> = Some([255, 0, 0, 255]);
const GLASS: Option<[u8; 4]> = Some([0, 0, 255, 102]);

/// Glass (40% blue) in front of red blends whichever is listed first, glass behind red is hidden,
/// a clear mesh draws nothing, and a mesh with no colour takes the options' colour.
#[test]
fn meshes_draw_in_their_own_colours_and_glass_blends() {
    let unlit = RenderOptions { lighting: Lighting::Unlit, ..small() };
    let at_middle = |meshes: &[(f64, Option<[u8; 4]>)]| {
        let meshes: Vec<WorldMesh> = meshes.iter().map(|&(z, c)| painted(quad(z, 1.0, 0.0), c)).collect();
        pixel(&render(&meshes, &ortho(4.0), &unlit).unwrap(), 20, 20)
    };
    assert_eq!(at_middle(&[(1.0, GLASS), (-1.0, RED)]), [153, 0, 102, 255]);
    assert_eq!(at_middle(&[(-1.0, RED), (1.0, GLASS)]), [153, 0, 102, 255]);
    assert_eq!(at_middle(&[(-1.0, GLASS), (1.0, RED)]), [255, 0, 0, 255]);
    assert_eq!(at_middle(&[(1.0, Some([0, 0, 255, 0])), (-1.0, RED)]), [255, 0, 0, 255]);
    assert_eq!(at_middle(&[(1.0, None)]), [190, 192, 200, 255]);
    assert_eq!(at_middle(&[(2.0, GLASS), (1.0, GLASS), (-1.0, RED)]), [92, 0, 163, 255], "glass does not hide glass");
    let clear = RenderOptions { background: [0, 0, 0, 0], ..unlit };
    let image = render(&[painted(quad(1.0, 1.0, 0.0), GLASS)], &ortho(4.0), &clear).unwrap();
    assert_eq!(pixel(&image, 20, 20), [0, 0, 255, 102], "glass over nothing");
}

#[test]
fn a_triangle_colour_overrides_its_mesh_colour() {
    let mut m = painted(quad(0.0, 1.0, 0.0), RED);
    // The lower-right triangle is green; the upper-left keeps the mesh's red.
    m.triangle_colours = Some(vec![Some([0, 255, 0, 255]), None]);
    let unlit = RenderOptions { lighting: Lighting::Unlit, ..small() };
    let image = render(&[m], &ortho(4.0), &unlit).unwrap();
    assert_eq!(pixel(&image, 25, 25), [0, 255, 0, 255]);
    assert_eq!(pixel(&image, 14, 14), [255, 0, 0, 255]);
}

#[test]
fn each_edge_bounds_a_triangle() {
    let tri = WorldMesh::new(mesh(vec![[-1.0, -1.0, 0.0], [1.0, -1.0, 0.0], [0.0, 1.0, 0.0]], vec![[0, 1, 2]]));
    let image = render(&[tri], &ortho(4.0), &small()).unwrap();
    assert_ne!(pixel(&image, 20, 25)[0], 255);
    // Just past each edge: below the base, left and right of the apex.
    for (x, y) in [(20, 31), (13, 15), (27, 15)] {
        assert_eq!(pixel(&image, x, y)[0], 255, "({x},{y})");
    }
}

#[test]
fn a_face_turned_away_is_still_drawn() {
    let mut back = quad_mesh(0.0, 1.0, 0.0);
    for t in &mut back.triangles {
        t.swap(1, 2);
    }
    let image = render(&[WorldMesh::new(back)], &ortho(4.0), &small()).unwrap();
    assert_eq!(pixel(&image, 20, 20), [190, 192, 200, 255]);
}

#[test]
fn perspective_clips_what_is_behind_the_camera() {
    let camera = Camera { projection: Projection::Perspective { fov_y: 60.0 }, ..ortho(1.0) };
    let image = render(&[quad(20.0, 1.0, 0.0)], &camera, &small()).unwrap();
    assert!(image.rgba.chunks(4).all(|p| p == [255, 255, 255, 255]));
    // A floor running from behind the camera to beyond the target.
    let floor = WorldMesh::new(mesh(vec![[-5.0, -1.0, 20.0], [5.0, -1.0, 20.0], [0.0, -1.0, -20.0]], vec![[0, 1, 2]]));
    let image = render(&[floor], &camera, &small()).unwrap();
    assert_ne!(pixel(&image, 20, 38), [255, 255, 255, 255]);
    assert_eq!(pixel(&image, 20, 2), [255, 255, 255, 255]);
}

#[test]
fn a_fitted_camera_frames_the_model() {
    // Depth matters in perspective: the nearer square looks larger.
    let meshes = [quad(3.0, 3.0, 0.0), quad(-3.0, 3.0, 0.0)];
    let bounds = Bounds::of(&meshes).unwrap();
    for perspective in [true, false] {
        let camera = Camera::fit(&bounds, [0.0, 0.0, -1.0], [0.0, 1.0, 0.0], perspective, 2.0).unwrap();
        let image = render(&meshes, &camera, &RenderOptions { width: 80, height: 40, ..RenderOptions::default() }).unwrap();
        assert_ne!(pixel(&image, 40, 20)[0], 255);
        // Nothing drawn touches the image border.
        for x in 0..80 {
            assert_eq!(pixel(&image, x, 0)[0], 255);
            assert_eq!(pixel(&image, x, 39)[0], 255);
        }
    }
}

#[test]
fn a_mesh_fit_ignores_empty_box_corners() {
    // A thin strip along the box's diagonal: seen across the diagonal, the box is as tall as it
    // is wide, the strip is not.
    let meshes = [WorldMesh::new(mesh(vec![[0.0, 0.0, 0.0], [10.0, 10.0, 0.0], [10.0, 10.01, 0.0]], vec![[0, 1, 2]]))];
    let (dir, up) = ([0.0, 0.0, -1.0], [1.0, -1.0, 0.0]);
    let height = |c: Camera| match c.projection {
        Projection::Orthographic { height } => height,
        Projection::Perspective { .. } => unreachable!(),
    };
    let boxed = height(Camera::fit(&Bounds::of(&meshes).unwrap(), dir, up, false, 2.0).unwrap());
    let tight = height(Camera::fit_meshes(&meshes, dir, up, false, 2.0).unwrap());
    assert!((boxed / tight - 2.0).abs() < 0.01, "{boxed} vs {tight}");
    let near = Camera::fit_meshes(&meshes, dir, up, true, 2.0).unwrap();
    let far = Camera::fit(&Bounds::of(&meshes).unwrap(), dir, up, true, 2.0).unwrap();
    assert!(length(sub(near.eye, near.target)) < 0.6 * length(sub(far.eye, far.target)));

    // Lopsided in its box: the limiting vertex sits exactly on the margin, measured from the
    // re-centred target. Narrow (width limits) and wide (height limits).
    let tri = WorldMesh::new(mesh(vec![[0.0, 0.0, 0.0], [2.0, 0.0, 0.0], [1.0, 2.0, 1.0]], vec![[0, 1, 2]]));
    for aspect in [0.5, 3.0] {
        let c = Camera::fit_meshes(std::slice::from_ref(&tri), [0.0, 0.3, -1.0], [1.0, 1.0, 0.0], true, aspect).unwrap();
        let [right, upward, forward] = c.basis().unwrap();
        let ty = 15f64.to_radians().tan() / FIT_MARGIN;
        let fill = tri
            .mesh
            .positions
            .iter()
            .map(|&p| {
                let r = sub(p, c.eye);
                (dot(r, right).abs() / (ty * aspect)).max(dot(r, upward).abs() / ty) / dot(r, forward)
            })
            .fold(0.0, f64::max);
        assert!((fill - 1.0).abs() < 1e-6, "aspect {aspect}: {fill}");
    }
}

#[test]
fn a_fitted_camera_fills_the_tighter_axis() {
    let drawn = |image: &Image, horizontal: bool, line: u32| {
        let n = if horizontal { image.width } else { image.height };
        (0..n)
            .filter(|&i| {
                let (x, y) = if horizontal { (i, line) } else { (line, i) };
                pixel(image, x, y)[0] != 255
            })
            .count()
    };
    // Height-limited: a 6 x 6 square in an 80 x 40 image.
    let square = [quad(0.0, 3.0, 0.0)];
    // Width-limited and off-origin: a 10 x 1 strip in a 40 x 40 image.
    let strip = [WorldMesh::new(mesh(vec![[0.0, 0.0, 0.0], [10.0, 0.0, 0.0], [10.0, 1.0, 0.0], [0.0, 1.0, 0.0]], vec![[0, 1, 2], [0, 2, 3]]))];
    for perspective in [true, false] {
        let fit =
            |meshes: &[WorldMesh], aspect| Camera::fit(&Bounds::of(meshes).unwrap(), [0.0, 0.0, -1.0], [0.0, 1.0, 0.0], perspective, aspect).unwrap();
        let wide = RenderOptions { width: 80, height: 40, ..RenderOptions::default() };
        let rows = drawn(&render(&square, &fit(&square, 2.0), &wide).unwrap(), false, 40);
        assert!((34..=37).contains(&rows), "{perspective}: {rows} of 40 rows");
        let image = render(&strip, &fit(&strip, 1.0), &small()).unwrap();
        let cols = drawn(&image, true, 20);
        assert!((34..=37).contains(&cols), "{perspective}: {cols} of 40 columns");
        // Centred: the margins differ by at most a pixel.
        let left = (0..40).take_while(|&x| pixel(&image, x, 20)[0] == 255).count();
        assert!(left.abs_diff(40 - cols - left) <= 1, "{perspective}: left {left}");
    }
}

#[test]
fn bad_sizes_and_cameras_are_refused() {
    let m = [quad(0.0, 1.0, 0.0)];
    let size = |width, height| RenderOptions { width, height, ..RenderOptions::default() };
    assert!(matches!(render(&m, &ortho(1.0), &size(0, 10)), Err(RenderError::Size { .. })));
    assert!(matches!(render(&m, &ortho(1.0), &size(10_000, 10_000)), Err(RenderError::Size { .. })));
    let bad = [
        Camera { eye: [0.0; 3], target: [0.0; 3], ..ortho(1.0) },
        Camera { up: [0.0, 0.0, 1.0], ..ortho(1.0) },
        Camera { projection: Projection::Perspective { fov_y: 180.0 }, ..ortho(1.0) },
        Camera { projection: Projection::Perspective { fov_y: f64::NAN }, ..ortho(1.0) },
        Camera { projection: Projection::Perspective { fov_y: 1e-310 }, ..ortho(1.0) },
        ortho(0.0),
        ortho(f64::INFINITY),
    ];
    for camera in bad {
        assert!(matches!(render(&m, &camera, &small()), Err(RenderError::Camera(_))), "{camera:?}");
    }
    let b = Bounds::of(&m).unwrap();
    assert!(Camera::fit(&b, [0.0; 3], [0.0, 1.0, 0.0], true, 1.0).is_err());
    assert!(Camera::fit(&b, [0.0, 0.0, 1.0], [0.0, 1.0, 0.0], true, 0.0).is_err());
    assert!(Camera::fit_meshes(&[], [0.0, 0.0, 1.0], [0.0, 1.0, 0.0], true, 1.0).is_err());
}

#[test]
fn non_finite_and_missing_vertices_are_skipped() {
    let m = WorldMesh::new(mesh(vec![[0.0, 0.0, 0.0], [f64::NAN, 0.0, 0.0], [0.0, 1.0, 0.0]], vec![[0, 1, 2], [0, 2, 9], [0, 2, u32::MAX]]));
    let image = render(std::slice::from_ref(&m), &ortho(4.0), &small()).unwrap();
    assert!(image.rgba.chunks(4).all(|p| p == [255, 255, 255, 255]));
    assert_eq!(Bounds::of(&[m]).unwrap().max, [0.0, 1.0, 0.0]);
}

#[test]
fn unlit_draws_the_plain_colour_and_the_headlight_shades_by_angle() {
    let lit = |lighting, tilt| {
        let image = render(&[quad(0.0, 1.0, tilt)], &ortho(4.0), &RenderOptions { lighting, ..small() }).unwrap();
        pixel(&image, 20, 20)
    };
    assert_eq!(lit(Lighting::Headlight, 0.0), [190, 192, 200, 255]);
    // Tilted 45°: 0.3 + 0.7 cos 45° ≈ 0.795 of the colour.
    assert_eq!(lit(Lighting::Headlight, 1.0), [151, 153, 159, 255]);
    assert_eq!(lit(Lighting::Unlit, 1.0), [190, 192, 200, 255]);
}

#[test]
fn a_huge_square_is_drawn() {
    // Its edges' cross product (about 1e320) overflows unless they are scaled first.
    let image = render(&[quad(0.0, 1e160, 0.0)], &ortho(4e160), &small()).unwrap();
    assert_eq!(pixel(&image, 20, 20), [190, 192, 200, 255]);
    assert_eq!(pixel(&image, 1, 1), [255, 255, 255, 255]);
}
