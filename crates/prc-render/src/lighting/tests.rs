#![allow(clippy::unwrap_used, clippy::panic)]

use super::*;
use crate::{Camera, Image, Lighting, Projection, RenderOptions, WorldMesh, render};
use craft_prc::TriangleMesh;

/// A 2 × 2 square through the origin with corners `corners`, as one world mesh.
fn square(corners: [[f64; 3]; 4]) -> WorldMesh {
    let mut m = TriangleMesh::default();
    m.positions = corners.to_vec();
    m.triangles = vec![[0, 1, 2], [0, 2, 3]];
    WorldMesh::new(m)
}

/// Facing +z.
fn square_z() -> WorldMesh {
    square([[-1.0, -1.0, 0.0], [1.0, -1.0, 0.0], [1.0, 1.0, 0.0], [-1.0, 1.0, 0.0]])
}

/// An orthographic camera at `eye` looking at the origin.
fn camera(eye: [f64; 3], up: [f64; 3]) -> Camera {
    Camera { eye, target: [0.0; 3], up, projection: Projection::Orthographic { height: 4.0 } }
}

/// The centre pixel of `mesh` drawn from `camera` under `lighting`.
fn centre(mesh: &WorldMesh, camera: &Camera, lighting: Lighting) -> [u8; 4] {
    let image: Image =
        render(std::slice::from_ref(mesh), camera, &RenderOptions { width: 40, height: 40, lighting, ..RenderOptions::default() }).unwrap();
    let i = (20 * 40 + 20) * 4;
    image.rgba[i..i + 4].try_into().unwrap()
}

#[test]
fn scheme_names_choose_the_lighting_and_unknown_names_are_ignored() {
    assert_eq!(LightingScheme::lighting_for("CAD"), Some(Lighting::Scheme(LightingScheme::Cad)));
    assert_eq!(LightingScheme::lighting_for("Headlamp"), Some(Lighting::Scheme(LightingScheme::Headlamp)));
    assert_eq!(LightingScheme::lighting_for("Artwork"), Some(Lighting::Headlight));
    assert_eq!(LightingScheme::lighting_for("None"), Some(Lighting::Unlit));
    // Names are case-sensitive PDF names; one Table 310 does not list is not a scheme.
    assert_eq!(LightingScheme::lighting_for("cad"), None);
    assert_eq!(LightingScheme::lighting_for("Sunset"), None);
    for s in LightingScheme::ALL {
        assert_eq!(LightingScheme::lighting_for(s.name()), Some(Lighting::Scheme(s)));
    }
}

#[test]
fn the_headlamp_lights_a_facing_surface_with_its_colour_plus_the_ambient_term() {
    let lit = centre(&square_z(), &camera([0.0, 0.0, 10.0], [0.0, 1.0, 0.0]), Lighting::Scheme(LightingScheme::Headlamp));
    // [190, 192, 200] × (0.1 + [0.8, 0.8, 0.9]).
    assert_eq!(lit, [171, 173, 200, 255]);
}

#[test]
fn primary_lights_tint_each_channel_from_its_own_light() {
    let lit = centre(&square_z(), &camera([0.0, 0.0, 10.0], [0.0, 1.0, 0.0]), Lighting::Scheme(LightingScheme::Primary));
    // Facing the camera, the three lights meet the face at |cos| 0.196, 0.739 and 1: red takes
    // 1 × 0.196 + 0.2 × 0.739, green 0.2 × 0.196 + 1 × 0.739, blue saturates.
    let expected = [65.0, 149.4, 255.0];
    for (got, want) in lit.iter().zip(expected) {
        assert!((f64::from(*got) - want).abs() <= 1.0, "{lit:?}");
    }
}

#[test]
fn scheme_lights_turn_with_the_camera() {
    // The same face, seen head on from +z and from +x: lights fixed to the camera light both alike.
    let from_x = square([[0.0, -1.0, -1.0], [0.0, 1.0, -1.0], [0.0, 1.0, 1.0], [0.0, -1.0, 1.0]]);
    for s in LightingScheme::ALL {
        let a = centre(&square_z(), &camera([0.0, 0.0, 10.0], [0.0, 1.0, 0.0]), Lighting::Scheme(s));
        let b = centre(&from_x, &camera([10.0, 0.0, 0.0], [0.0, 0.0, 1.0]), Lighting::Scheme(s));
        assert_eq!(a, b, "{}", s.name());
    }
}

#[test]
fn light_past_full_colour_saturates() {
    // Hard's ambient 0.5 plus three lights adds to about 2.1 on a facing surface, so the default
    // grey [190, 192, 200] saturates to white.
    let lit = centre(&square_z(), &camera([0.0, 0.0, 10.0], [0.0, 1.0, 0.0]), Lighting::Scheme(LightingScheme::Hard));
    assert_eq!(lit, [255, 255, 255, 255]);
}

#[test]
fn a_tilted_face_takes_each_light_by_its_own_direction() {
    // A face with normal (1, 1, 1)/√3 seen from +z under Day, on grey 100: the lights meet it at
    // |cos| 0.679, 0.955 and 0.569, so red and green take 0.5·0.679 + 0.8·0.955 + 0.9·0.569 and
    // blue 0.5·0.679 + 0.9·0.955 + 0.9·0.569. A light mirrored in x or y changes these.
    let (r2, r6) = (std::f64::consts::SQRT_2, 6f64.sqrt());
    let (u, v) = ([1.0 / r2, -1.0 / r2, 0.0], [1.0 / r6, 1.0 / r6, -2.0 / r6]);
    let corner = |a: f64, b: f64| [0, 1, 2].map(|i| a * u[i] + b * v[i]);
    let face = square([corner(-1.0, -1.0), corner(1.0, -1.0), corner(1.0, 1.0), corner(-1.0, 1.0)]);
    let grey = WorldMesh { colour: Some([100, 100, 100, 255]), ..face };
    let lit = centre(&grey, &camera([0.0, 0.0, 10.0], [0.0, 1.0, 0.0]), Lighting::Scheme(LightingScheme::Day));
    for (got, want) in lit.iter().zip([161.6, 161.6, 171.1, 255.0]) {
        assert!((f64::from(*got) - want).abs() <= 1.0, "{lit:?}");
    }
}
