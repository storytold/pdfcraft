use super::super::axes::Axis;
use super::*;

fn cube() -> Bounds {
    Bounds { min: [-1.0; 3], max: [1.0; 3] }
}

fn unit(v: Vec3) -> Vec3 {
    normalize(v).unwrap()
}

fn parallel(a: Vec3, b: Vec3) -> bool {
    (dot(unit(a), unit(b)) - 1.0).abs() < 1e-9
}

fn gap(c: &Camera) -> f64 {
    length(sub(c.eye, c.target))
}

/// Looking along +x and down, with +y up: no named view's axes.
fn corner() -> Orbit {
    Orbit::looking([0.6, 0.0, -0.8], [0.0, 1.0, 0.0], true).unwrap()
}

#[test]
fn the_named_views_look_the_way_their_names_say() {
    let look = |v| Orbit::named(v, false, Upright::default()).direction();
    let front = look(NamedView::Front);
    assert!((front[1] - 1.0).abs() < 1e-9, "front looks along +y: {front:?}");
    let right = look(NamedView::Right);
    assert!((right[0] + 1.0).abs() < 1e-9, "right looks along -x, from +x: {right:?}");
    let top = look(NamedView::Top);
    assert!(top[2] < -0.99, "top looks down: {top:?}");
    let iso = look(NamedView::Isometric);
    assert!(iso[0] < 0.0 && iso[1] > 0.0 && iso[2] < 0.0, "{iso:?}");
    for v in NamedView::ALL {
        assert_eq!(NamedView::from_token(v.token()), Some(v));
    }
}

#[test]
fn with_y_up_the_named_views_turn_about_y() {
    let upright = Upright::default().with_up(Axis::PosY);
    let front = Orbit::named(NamedView::Front, false, upright).direction();
    assert!(front[2] < -0.999, "front looks along -z: {front:?}");
    let top = Orbit::named(NamedView::Top, false, upright).direction();
    assert!(top[1] < -0.99, "top looks down -y: {top:?}");
}

#[test]
fn a_view_of_its_own_looks_along_its_direction_with_its_own_up() {
    let camera = corner().camera(&cube(), 1.0, 1.5).unwrap();
    assert!(parallel(sub(camera.target, camera.eye), [0.6, 0.0, -0.8]));
    assert!(parallel(camera.up, [0.0, 1.0, 0.0]), "{:?}", camera.up);
    // An up that is not perpendicular to the look keeps only its perpendicular part.
    let tilted = Orbit::looking([1.0, 0.0, 0.0], [1.0, 0.0, 1.0], true).unwrap();
    assert!(parallel(tilted.axes.up, [0.0, 0.0, 1.0]));
    assert!(parallel(tilted.direction(), [1.0, 0.0, 0.0]));
}

#[test]
fn a_view_with_its_up_along_its_look_or_out_of_range_is_refused() {
    assert!(Orbit::looking([0.0, 0.0, 2.0], [0.0, 0.0, 1.0], true).is_none());
    assert!(Orbit::looking([0.0; 3], [0.0, 0.0, 1.0], true).is_none());
    assert!(Orbit::looking([f64::NAN, 0.0, 1.0], [0.0, 1.0, 0.0], true).is_none());
    assert!(Orbit::looking([f64::MAX, f64::MAX, 0.0], [0.0, 0.0, f64::MAX], true).is_some());
    for v in [[1.0, 1.0, 1.0], [0.1, 0.7, 0.3], [-3.0, 1e-3, 7.0]] {
        assert!(Orbit::looking(v, v, true).is_none(), "{v:?} along itself");
        assert!(Orbit::looking(v, scale(v, -2.0), true).is_none(), "{v:?} against itself");
    }
}

#[test]
fn zoom_brings_the_eye_closer_and_pan_moves_the_target() {
    let bounds = cube();
    let mut orbit = Orbit::named(NamedView::Front, true, Upright::default());
    let fitted = orbit.camera(&bounds, 1.0, 1.0).unwrap();
    orbit.zoom_at(&bounds, 1.0, 1.0, 2.0, [0.0, 0.0]);
    orbit.pan_by([0.5, 0.0]);
    let moved = orbit.camera(&bounds, 1.0, 1.0).unwrap();
    assert!((gap(&moved) - gap(&fitted) / 2.0).abs() < 1e-9);
    assert!(moved.target != fitted.target);
    assert!(moved.target[2].abs() < 1e-9, "a sideways pan keeps the height");
}

/// The model point on the target plane at `offset` (picture heights, right and up from the centre).
fn under(orbit: &Orbit, bounds: &Bounds, aspect: f64, offset: [f64; 2]) -> Vec3 {
    let camera = orbit.camera(bounds, 2.0, aspect).unwrap();
    let (right, up) = orbit.axes.picture(orbit.direction()).unwrap();
    let h = visible_height(&camera);
    add(camera.target, scale(add(scale(right, offset[0]), scale(up, offset[1])), h))
}

#[test]
fn a_wheel_zoom_keeps_the_point_under_the_pointer_still() {
    let bounds = Bounds { min: [-1.0, -2.0, -0.5], max: [3.0, 1.0, 2.0] };
    for start in
        [Orbit::named(NamedView::Isometric, true, Upright::default()), Orbit::named(NamedView::Isometric, false, Upright::default()), corner()]
    {
        let mut orbit = start;
        orbit.pan_by([0.1, -0.2]);
        let (aspect, offset) = (1.6, [0.3, -0.15]);
        let before = under(&orbit, &bounds, aspect, offset);
        orbit.zoom_at(&bounds, 2.0, aspect, 2.5, offset);
        let after = under(&orbit, &bounds, aspect, offset);
        for (b, a) in before.iter().zip(after) {
            assert!((b - a).abs() < 1e-9, "{before:?} vs {after:?}");
        }
        assert!((orbit.zoom() - 2.5).abs() < 1e-12);
    }
}

#[test]
fn a_wheel_zoom_at_the_centre_does_not_pan_and_stays_in_range() {
    let mut orbit = Orbit::named(NamedView::Front, true, Upright::default());
    orbit.zoom_at(&cube(), 1.0, 1.0, 3.0, [0.0, 0.0]);
    assert_eq!(orbit.pan(), [0.0, 0.0]);
    orbit.zoom_at(&cube(), 1.0, 1.0, 1e300, [0.0, 0.0]);
    assert_eq!(orbit.zoom(), ZOOM_RANGE.1);
    orbit.zoom_at(&cube(), 1.0, 1.0, 1e-300, [0.0, 0.0]);
    assert_eq!(orbit.zoom(), ZOOM_RANGE.0);
}

#[test]
fn turning_or_widening_the_picture_keeps_the_models_size() {
    let bounds = Bounds { min: [-4.0, -1.0, 0.0], max: [4.0, 1.0, 0.5] };
    let mut orbit = Orbit::named(NamedView::Isometric, true, Upright::default());
    let first = orbit.camera(&bounds, 4.0, 1.5).unwrap();
    for yaw in [0.3, 0.8, 0.9] {
        orbit.turn(yaw, 0.0);
        for aspect in [1.2, 1.5, 3.0] {
            let c = orbit.camera(&bounds, 4.0, aspect).unwrap();
            assert!((gap(&c) - gap(&first)).abs() < 1e-9, "{yaw} {aspect}");
            assert_eq!(c.target, first.target);
        }
    }
}

#[test]
fn a_turn_holds_the_pitch_off_the_up_axis_and_ignores_non_finite_input() {
    let mut orbit = Orbit::named(NamedView::Front, true, Upright::default());
    orbit.turn(-1.0, 10.0);
    assert_eq!(orbit.pitch(), PITCH_LIMIT);
    assert!((0.0..TAU).contains(&orbit.yaw()));
    assert!(orbit.camera(&cube(), 1.0, 1.0).is_ok(), "the top-most view still forms");
    let held = orbit;
    orbit.turn(f64::NAN, 0.0);
    orbit.pan_by([f64::INFINITY, 0.0]);
    orbit.zoom_at(&cube(), 1.0, 1.0, f64::NAN, [0.0, 0.0]);
    orbit.zoom_at(&cube(), 1.0, 1.0, -2.0, [0.0, 0.0]);
    orbit.look_along([0.0; 3]);
    assert_eq!(orbit, held);
}

#[test]
fn a_far_pan_is_held_and_an_out_of_range_model_is_an_error_not_a_panic() {
    let mut orbit = Orbit::named(NamedView::Front, true, Upright::default());
    orbit.pan_by([1e300, -1e300]);
    assert_eq!(orbit.pan(), [PAN_LIMIT, -PAN_LIMIT]);
    let huge = Bounds { min: [-f64::MAX; 3], max: [f64::MAX; 3] };
    assert!(orbit.camera(&huge, f64::MAX, 1.0).is_err());
    let flat = Bounds { min: [0.0; 3], max: [0.0; 3] };
    assert!(orbit.camera(&flat, 0.0, f64::NAN).is_ok(), "an empty extent falls back to a unit radius");
    assert!(orbit.camera(&cube(), f64::INFINITY, 1.0).is_err(), "an overflowing reach is refused, not replaced");
    assert!(orbit.camera(&cube(), f64::NAN, 1.0).is_err());
    // A shift that overflows to NaN (a zoom about a model at the edge of the range) leaves the pan.
    orbit.set_pan([f64::NAN, 0.0]);
    assert_eq!(orbit.pan(), [PAN_LIMIT, -PAN_LIMIT]);
}

#[test]
fn looking_along_the_up_axis_stops_just_short_of_it_and_refits() {
    let mut orbit = Orbit::named(NamedView::Front, true, Upright::default());
    orbit.zoom_at(&cube(), 1.0, 1.0, 2.0, [0.1, 0.1]);
    orbit.look_along([0.0, 0.0, -1.0]);
    assert_eq!(orbit.pitch(), PITCH_LIMIT);
    assert_eq!((orbit.zoom(), orbit.pan()), (1.0, [0.0, 0.0]));
}

#[test]
fn an_anchor_keeps_the_picture_then_turns_about_its_point() {
    let bounds = cube();
    for perspective in [true, false] {
        let mut orbit = Orbit::named(NamedView::Isometric, perspective, Upright::default());
        orbit.zoom_at(&bounds, 1.5, 1.4, 1.7, [0.0, 0.0]);
        orbit.pan_by([0.2, -0.1]);
        let before = orbit.camera(&bounds, 1.5, 1.4).unwrap();
        let point = add(before.target, scale(orbit.direction(), 0.4));
        assert!(orbit.anchor_at(&before, point));
        assert_eq!(orbit.anchor(), Some(point));
        let after = orbit.camera(&bounds, 1.5, 1.4).unwrap();
        if perspective {
            for (a, b) in after.eye.iter().zip(before.eye) {
                assert!((a - b).abs() < 1e-9);
            }
        } else {
            assert!((visible_height(&after) - visible_height(&before)).abs() < 1e-9);
        }
        orbit.turn(0.8, 0.4);
        let turned = orbit.camera(&bounds, 1.5, 1.4).unwrap();
        for (t, p) in turned.target.iter().zip(point) {
            assert!((t - p).abs() < 1e-9);
        }
    }
}

#[test]
fn a_point_behind_the_eye_is_not_turned_about() {
    let mut orbit = Orbit::named(NamedView::Front, true, Upright::default());
    let camera = orbit.camera(&cube(), 1.0, 1.0).unwrap();
    let behind = sub(camera.eye, orbit.direction());
    assert!(!orbit.anchor_at(&camera, behind));
    assert!(!orbit.anchor_at(&camera, [f64::NAN, 0.0, 0.0]));
    assert_eq!(orbit.anchor(), None);
}

#[test]
fn a_refit_forgets_the_point_turned_about() {
    let bounds = cube();
    let mut orbit = Orbit::named(NamedView::Isometric, true, Upright::default());
    let fitted = orbit.camera(&bounds, 1.5, 1.4).unwrap();
    assert!(orbit.anchor_at(&fitted, [0.3, 0.2, 0.1]));
    orbit.zoom_at(&bounds, 1.5, 1.4, 2.0, [0.0, 0.0]);
    orbit.refit();
    assert_eq!(orbit.anchor(), None);
    assert_eq!(orbit.camera(&bounds, 1.5, 1.4).unwrap(), fitted);
}

#[test]
fn centring_on_a_point_puts_it_at_the_target() {
    let bounds = cube();
    for mut orbit in [Orbit::named(NamedView::Isometric, true, Upright::default()), corner()] {
        let point = [0.5, -0.25, 0.75];
        orbit.centre_on(&bounds, point);
        let c = orbit.camera(&bounds, 1.0, 1.0).unwrap();
        // The target is the point moved along the look, so the point shows at the centre.
        assert!(parallel(sub(point, c.eye), sub(c.target, c.eye)) || length(sub(point, c.target)) < 1e-9);
    }
}

/// Where `p` shows on `c`'s picture: right and up of the centre, as a share of the visible height
/// at `p`'s depth.
fn shown(c: &Camera, p: Vec3) -> [f64; 2] {
    let f = unit(sub(c.target, c.eye));
    let r = unit(cross(f, c.up));
    let u = cross(r, f);
    let v = sub(p, c.eye);
    let h = match c.projection {
        Projection::Perspective { fov_y } => 2.0 * dot(v, f) * (fov_y / 2.0).to_radians().tan(),
        Projection::Orthographic { height } => height,
    };
    [dot(v, r) / h, dot(v, u) / h]
}

#[test]
fn a_pressed_point_off_centre_keeps_its_place_as_the_view_turns() {
    let bounds = cube();
    for perspective in [true, false] {
        let mut orbit = Orbit::named(NamedView::Isometric, perspective, Upright::default());
        orbit.zoom_at(&bounds, 1.5, 1.4, 1.3, [0.0, 0.0]);
        let before = orbit.camera(&bounds, 1.5, 1.4).unwrap();
        let f = orbit.direction();
        let (r, u) = orbit.axes.picture(f).unwrap();
        let point = add(before.target, add(add(scale(r, 0.3), scale(u, 0.2)), scale(f, 0.4)));
        let at = shown(&before, point);
        assert!(orbit.anchor_at(&before, point));
        for (yaw, pitch) in [(0.0, 0.0), (0.8, 0.4), (-1.9, -1.1)] {
            orbit.turn(yaw, pitch);
            let c = orbit.camera(&bounds, 1.5, 1.4).unwrap();
            let now = shown(&c, point);
            for (n, a) in now.iter().zip(at) {
                assert!((n - a).abs() < 1e-9, "{perspective} {now:?} {at:?}");
            }
        }
    }
}

#[test]
fn a_vertex_too_far_to_measure_gives_an_infinite_reach_not_a_near_one() {
    let mut mesh = craft_prc::TriangleMesh::default();
    mesh.positions = vec![[-f64::MAX, -f64::MAX, 0.0], [f64::MAX, f64::MAX, 0.0], [f64::MAX, -f64::MAX, 0.0], [0.0, 0.0, 0.0]];
    mesh.triangles = vec![[0, 1, 2], [3, 3, 3]];
    let meshes = [WorldMesh::new(mesh)];
    let bounds = Bounds::of(&meshes).unwrap();
    assert_eq!(reach(&meshes, &bounds), f64::INFINITY);
    assert!(Orbit::named(NamedView::Front, true, Upright::default()).camera(&bounds, reach(&meshes, &bounds), 1.0).is_err());
}
