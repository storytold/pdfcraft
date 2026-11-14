#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use pdfcraft_prc::TriangleMesh;

use super::*;
use crate::nav::Axis;
use crate::{PickIndex, WorldMesh, pick};

fn quad(corners: [[f64; 3]; 4]) -> TriangleMesh {
    let mut mesh = TriangleMesh::default();
    mesh.positions = corners.to_vec();
    mesh.triangles = vec![[0, 1, 2], [0, 2, 3]];
    mesh
}

/// A 20 × 20 floor at z = 0 and a wall across x = 5, z up.
fn room() -> Vec<WorldMesh> {
    let floor = quad([[-10.0, -10.0, 0.0], [10.0, -10.0, 0.0], [10.0, 10.0, 0.0], [-10.0, 10.0, 0.0]]);
    let wall = quad([[5.0, -10.0, 0.0], [5.0, 10.0, 0.0], [5.0, 10.0, 10.0], [5.0, -10.0, 10.0]]);
    vec![WorldMesh::new(floor), WorldMesh::new(wall)]
}

/// [`room`] with a block across x = 2..3 from `z0` up to `z1`; with `z0` above 0 it stands on
/// nothing, as a table top does.
fn room_with_block(z0: f64, z1: f64) -> Vec<WorldMesh> {
    let mut meshes = room();
    let mut block = TriangleMesh::default();
    block.positions = vec![
        [2.0, -10.0, z0],
        [2.0, 10.0, z0],
        [2.0, 10.0, z1],
        [2.0, -10.0, z1],
        [3.0, -10.0, z1],
        [3.0, 10.0, z1],
        [3.0, 10.0, z0],
        [3.0, -10.0, z0],
    ];
    block.triangles = vec![[0, 1, 2], [0, 2, 3], [3, 2, 5], [3, 5, 4], [4, 5, 6], [4, 6, 7], [7, 6, 1], [7, 1, 0]];
    meshes.push(WorldMesh::new(block));
    meshes
}

/// [`room`] with a ramp across it rising `slope` per unit, from x = 1 to the wall at x = 5.
fn room_with_ramp(slope: f64) -> Vec<WorldMesh> {
    let mut meshes = room();
    let top = slope * 4.0;
    meshes.push(WorldMesh::new(quad([[1.0, -10.0, 0.0], [5.0, -10.0, top], [5.0, 10.0, top], [1.0, 10.0, 0.0]])));
    meshes
}

/// Looking along +x, z up, radius 10: eye height 0.8.
fn walker(eye: [f64; 3]) -> Walk {
    let upright = Upright::default().with_front(Axis::PosX);
    Walk::standing(eye, [1.0, 0.0, 0.0], upright, 10.0).unwrap()
}

const SOLID: Rules = Rules { solid: true, level: true, speed: 1.0, stairs: false };
const GHOST: Rules = Rules { solid: false, ..SOLID };
const CLIMBING: Rules = Rules { stairs: true, ..SOLID };

fn ahead(seconds: usize, walk: &mut Walk, rules: Rules, meshes: &[WorldMesh]) {
    let intent = Intent { forward: 1.0, ..Intent::default() };
    for _ in 0..seconds * 20 {
        walk.step(intent, 0.05, rules, &mut |ray| pick(meshes, ray));
    }
}

#[test]
fn a_solid_walker_stops_at_the_wall_and_a_ghost_goes_through() {
    let meshes = room();
    let mut solid = walker([0.0, 0.0, 0.8]);
    ahead(10, &mut solid, SOLID, &meshes);
    assert!(solid.eye()[0] < 5.0 && solid.eye()[0] > 4.0, "{:?}", solid.eye());
    let mut ghost = walker([0.0, 0.0, 0.8]);
    ahead(10, &mut ghost, GHOST, &meshes);
    assert!(ghost.eye()[0] > 10.0, "{:?}", ghost.eye());
}

#[test]
fn a_walker_slides_along_a_wall_it_meets_at_an_angle() {
    let meshes = room();
    let mut walk = walker([3.0, 0.0, 0.8]);
    walk.turn(-std::f64::consts::FRAC_PI_4, 0.0);
    ahead(4, &mut walk, SOLID, &meshes);
    let [x, y, _] = walk.eye();
    assert!(x < 5.0, "{x}");
    assert!(y.abs() > 5.0, "it slid along the wall: {:?}", walk.eye());
}

#[test]
fn a_walker_slides_along_what_only_its_knee_or_only_its_eye_meets() {
    for (name, meshes) in [("a low block", room_with_block(0.0, 0.4)), ("a hanging beam", room_with_block(0.6, 1.0))] {
        let mut walk = walker([0.0, 0.0, 0.8]);
        walk.turn(-std::f64::consts::FRAC_PI_4, 0.0);
        ahead(4, &mut walk, SOLID, &meshes);
        let [x, y, _] = walk.eye();
        assert!(x < 2.0 && y.abs() > 5.0, "{name}: {:?}", walk.eye());
    }
}

#[test]
fn a_walker_stops_in_a_corner_rather_than_sliding_through_it() {
    let mut meshes = room();
    for y in [-3.0, 3.0] {
        meshes.push(WorldMesh::new(quad([[-10.0, y, 0.0], [10.0, y, 0.0], [10.0, y, 10.0], [-10.0, y, 10.0]])));
    }
    let mut walk = walker([3.0, 0.0, 0.8]);
    walk.turn(-std::f64::consts::FRAC_PI_4, 0.0);
    ahead(4, &mut walk, SOLID, &meshes);
    let [x, y, _] = walk.eye();
    assert!(x < 5.0 && y.abs() < 3.0, "{:?}", walk.eye());
}

#[test]
fn a_walker_climbs_a_walkable_ramp_at_any_pace_and_frame_time() {
    for slope in [0.25, 0.5, 0.9] {
        let meshes = room_with_ramp(slope);
        for (run, dt) in [(false, 0.02), (false, 0.05), (false, 0.1), (true, 0.05), (true, 0.1)] {
            let mut walk = walker([0.0, 0.0, 0.8]);
            for _ in 0..(1.0 / dt) as usize {
                walk.step(Intent { forward: 1.0, run, ..Intent::default() }, dt, SOLID, &mut |ray| pick(&meshes, ray));
            }
            let [x, _, z] = walk.eye();
            assert!(x > 2.0, "slope {slope} run {run} dt {dt}: {:?}", walk.eye());
            assert!((z - 0.8 - slope * (x - 1.0)).abs() < 1e-9, "slope {slope} run {run} dt {dt}: {:?}", walk.eye());
        }
    }
}

#[test]
fn a_walker_runs_over_a_crest_where_a_steep_ramp_meets_a_gentle_one() {
    let mut meshes = room();
    meshes.push(WorldMesh::new(quad([[1.0, -10.0, 0.0], [2.0, -10.0, 0.9], [2.0, 10.0, 0.9], [1.0, 10.0, 0.0]])));
    meshes.push(WorldMesh::new(quad([[2.0, -10.0, 0.9], [5.0, -10.0, 1.2], [5.0, 10.0, 1.2], [2.0, 10.0, 0.9]])));
    let mut walk = walker([0.0, 0.0, 0.8]);
    for _ in 0..12 {
        walk.step(Intent { forward: 1.0, run: true, ..Intent::default() }, 0.05, SOLID, &mut |ray| pick(&meshes, ray));
    }
    assert!(walk.eye()[0] > 3.0, "{:?}", walk.eye());
}

/// A ceiling sheet at height `z` over x = `x0`..`x1`.
fn ceiling(x0: f64, x1: f64, z: f64) -> WorldMesh {
    WorldMesh::new(quad([[x0, -10.0, z], [x1, -10.0, z], [x1, 10.0, z], [x0, 10.0, z]]))
}

#[test]
fn a_ramp_under_a_low_ceiling_stops_the_walker_below_it() {
    let mut meshes = room_with_ramp(0.5);
    meshes.push(ceiling(-10.0, 10.0, 1.2));
    let mut walk = walker([0.0, 0.0, 0.8]);
    ahead(3, &mut walk, SOLID, &meshes);
    let [x, _, z] = walk.eye();
    assert!(z < 1.2 && x > 1.6, "{:?}", walk.eye());
    assert!((z - 0.8 - 0.5 * (x - 1.0)).abs() < 1e-9, "{:?}", walk.eye());
    let stopped = walk.eye();
    ahead(3, &mut walk, SOLID, &meshes);
    assert_eq!(walk.eye(), stopped);
}

#[test]
fn a_ramp_never_carries_the_walker_across_a_plate_narrower_than_its_stride() {
    // The eye's line crosses each plate 0.2 along it, and a run at 0.1 s strides 0.75.
    for at in 0..30 {
        let x0 = 1.5 + f64::from(at) * 0.03;
        let z = 0.8 + 0.5 * (x0 - 1.0) + 0.1;
        let mut meshes = room_with_ramp(0.5);
        meshes.push(ceiling(x0, x0 + 0.4, z));
        for (run, dt) in [(false, 0.05), (true, 0.05), (true, 0.1)] {
            let mut walk = walker([0.0, 0.0, 0.8]);
            for _ in 0..(3.0 / dt) as usize {
                walk.step(Intent { forward: 1.0, run, ..Intent::default() }, dt, SOLID, &mut |ray| pick(&meshes, ray));
            }
            assert!(walk.eye()[0] < x0 + 0.2 && walk.eye()[2] < z, "{x0} {run} {dt}: {:?}", walk.eye());
        }
    }
}

#[test]
fn a_step_is_not_climbed_into_a_ceiling() {
    let mut meshes = room_with_block(0.0, 0.2);
    meshes.push(ceiling(-10.0, 10.0, 0.9));
    let mut walk = walker([0.0, 0.0, 0.8]);
    ahead(3, &mut walk, CLIMBING, &meshes);
    assert!(walk.eye()[0] < 2.0 && (walk.eye()[2] - 0.8).abs() < 1e-9, "{:?}", walk.eye());
}

#[test]
fn a_threshold_under_a_door_header_is_walked_over() {
    let mut meshes = room_with_block(0.0, 0.02);
    meshes.push(ceiling(-10.0, 10.0, 0.82 + 0.19 * 0.8));
    let mut walk = walker([0.0, 0.0, 0.8]);
    ahead(1, &mut walk, SOLID, &meshes);
    assert!(walk.eye()[0] > 2.1 && (walk.eye()[2] - 0.82).abs() < 1e-9, "{:?}", walk.eye());
}

#[test]
fn a_walker_under_a_ceiling_lower_than_its_height_walks_out_and_stands_up() {
    let mut meshes = room();
    meshes.push(ceiling(-10.0, 1.0, 0.6));
    let mut walk = walker([-5.0, 0.0, 0.5]);
    ahead(1, &mut walk, SOLID, &meshes);
    assert!((walk.eye()[2] - 0.5).abs() < 1e-9, "it stays under the ceiling: {:?}", walk.eye());
    ahead(3, &mut walk, SOLID, &meshes);
    assert!(walk.eye()[0] > 1.0 && (walk.eye()[2] - 0.8).abs() < 1e-9, "{:?}", walk.eye());
}

#[test]
fn a_walker_placed_on_the_floor_stands_up() {
    let meshes = room();
    let mut walk = walker([0.0, 0.0, 0.0]);
    walk.step(Intent::default(), 0.05, SOLID, &mut |ray| pick(&meshes, ray));
    assert!((walk.eye()[2] - 0.8).abs() < 1e-9, "{:?}", walk.eye());
}

#[test]
fn right_is_right_of_the_heading() {
    let meshes = room();
    let mut walk = walker([0.0, 0.0, 0.8]);
    walk.step(Intent { right: 1.0, ..Intent::default() }, 0.05, SOLID, &mut |ray| pick(&meshes, ray));
    assert!(walk.eye()[1] < 0.0 && walk.eye()[0].abs() < 1e-9, "{:?}", walk.eye());
}

#[test]
fn a_walker_falls_along_any_up_axis_and_stops_at_its_walls() {
    let floor = quad([[-10.0, 0.0, -10.0], [10.0, 0.0, -10.0], [10.0, 0.0, 10.0], [-10.0, 0.0, 10.0]]);
    let wall = quad([[5.0, 0.0, -10.0], [5.0, 10.0, -10.0], [5.0, 10.0, 10.0], [5.0, 0.0, 10.0]]);
    let meshes = [WorldMesh::new(floor), WorldMesh::new(wall)];
    let upright = Upright::default().with_up(Axis::PosY).with_front(Axis::PosX);
    let mut walk = Walk::standing([0.0, 4.0, 0.0], [1.0, 0.0, 0.0], upright, 10.0).unwrap();
    ahead(5, &mut walk, SOLID, &meshes);
    let [x, y, _] = walk.eye();
    assert!((y - 0.8).abs() < 1e-9, "{:?}", walk.eye());
    assert!(x > 4.0 && x < 5.0, "{:?}", walk.eye());
}

#[test]
fn a_ramp_too_steep_to_walk_is_a_wall() {
    let meshes = room_with_ramp(2.0);
    let mut walk = walker([0.0, 0.0, 0.8]);
    ahead(3, &mut walk, SOLID, &meshes);
    assert!(walk.eye()[0] < 1.1 && (walk.eye()[2] - 0.8).abs() < 1e-9, "{:?}", walk.eye());
    // Its edge passes between the wall rays, so only the floor under the far end can stop this.
    let mut meshes = room();
    meshes.push(WorldMesh::new(quad([[2.0, -10.0, 0.1], [3.0, -10.0, 2.1], [3.0, 10.0, 2.1], [2.0, 10.0, 0.1]])));
    let mut walk = walker([0.06, 0.0, 0.8]);
    ahead(3, &mut walk, SOLID, &meshes);
    assert!(walk.eye()[0] < 2.0 && (walk.eye()[2] - 0.8).abs() < 1e-9, "{:?}", walk.eye());
}

#[test]
fn a_tilted_top_higher_than_a_step_is_not_stepped_onto() {
    // Both wall rays miss its edge at 0.35; the floor under it is walkable, but far too high.
    let mut meshes = room();
    meshes.push(WorldMesh::new(quad([[2.0, -10.0, 0.35], [3.0, -10.0, 0.6], [3.0, 10.0, 0.6], [2.0, 10.0, 0.35]])));
    for rules in [SOLID, CLIMBING] {
        let mut walk = walker([0.0, 0.0, 0.8]);
        ahead(3, &mut walk, rules, &meshes);
        assert!(walk.eye()[0] < 2.0 && (walk.eye()[2] - 0.8).abs() < 1e-9, "{rules:?} {:?}", walk.eye());
    }
}

#[test]
fn a_solid_walker_falls_to_eye_height_and_stays_there() {
    let meshes = room();
    let mut walk = walker([0.0, 0.0, 4.0]);
    for _ in 0..100 {
        walk.step(Intent::default(), 0.05, SOLID, &mut |ray| pick(&meshes, ray));
    }
    assert!((walk.eye()[2] - 0.8).abs() < 1e-9, "{:?}", walk.eye());
    assert!(!walk.falling());
}

#[test]
fn a_long_fall_never_lands_below_eye_height() {
    let meshes = room();
    let mut walk = walker([0.0, 0.0, 40.0]);
    for _ in 0..200 {
        walk.step(Intent::default(), 0.05, SOLID, &mut |ray| pick(&meshes, ray));
        assert!(walk.eye()[2] >= 0.8 - 1e-9, "{:?}", walk.eye());
    }
    assert!((walk.eye()[2] - 0.8).abs() < 1e-9, "{:?}", walk.eye());
}

#[test]
fn with_no_floor_under_it_a_walker_keeps_its_height() {
    let mut walk = walker([50.0, 50.0, 4.0]);
    ahead(1, &mut walk, SOLID, &room());
    assert!((walk.eye()[2] - 4.0).abs() < 1e-12, "{:?}", walk.eye());
}

#[test]
fn a_level_walker_keeps_its_height_looking_down_and_a_ghost_not_level_flies_down() {
    let meshes = room();
    let mut walk = walker([-5.0, 0.0, 3.0]);
    walk.turn(0.0, 0.5);
    ahead(2, &mut walk, GHOST, &meshes);
    assert!((walk.eye()[2] - 3.0).abs() < 1e-9, "{:?}", walk.eye());
    let mut flying = walker([-5.0, 0.0, 3.0]);
    flying.turn(0.0, 0.5);
    ahead(2, &mut flying, Rules { level: false, ..GHOST }, &meshes);
    assert!(flying.eye()[2] < 2.0, "{:?}", flying.eye());
}

#[test]
fn stair_climbing_climbs_under_a_third_of_the_eye_height_and_off_it_only_bumps() {
    // Eye height 0.8: 0.2 is under a third of it, 0.4 over; a step of 0.03 is under the bump.
    let low = room_with_block(0.0, 0.2);
    let mut over = walker([0.0, 0.0, 0.8]);
    ahead(3, &mut over, CLIMBING, &low);
    assert!(over.eye()[0] > 3.0, "{:?}", over.eye());
    let mut bumped = walker([0.0, 0.0, 0.8]);
    ahead(3, &mut bumped, SOLID, &low);
    assert!(bumped.eye()[0] < 2.0, "{:?}", bumped.eye());
    let mut stopped = walker([0.0, 0.0, 0.8]);
    ahead(3, &mut stopped, CLIMBING, &room_with_block(0.0, 0.4));
    assert!(stopped.eye()[0] < 2.0, "{:?}", stopped.eye());
    let mut seam = walker([0.0, 0.0, 0.8]);
    ahead(3, &mut seam, SOLID, &room_with_block(0.0, 0.03));
    assert!(seam.eye()[0] > 3.0, "{:?}", seam.eye());
}

#[test]
fn a_solid_walker_cannot_rise_over_a_block_and_a_ghost_rises() {
    let high = room_with_block(0.0, 0.4);
    let lifting = Intent { forward: 1.0, rise: 1.0, ..Intent::default() };
    let mut solid = walker([0.0, 0.0, 0.8]);
    for _ in 0..60 {
        solid.step(lifting, 0.05, SOLID, &mut |ray| pick(&high, ray));
    }
    assert!(solid.eye()[0] < 2.0, "{:?}", solid.eye());
    assert!((solid.eye()[2] - 0.8).abs() < 1e-9, "{:?}", solid.eye());
    let mut flying = walker([0.0, 0.0, 0.8]);
    flying.step(lifting, 0.05, GHOST, &mut |ray| pick(&high, ray));
    assert!(flying.eye()[2] > 0.8, "{:?}", flying.eye());
}

#[test]
fn a_solid_walker_never_climbs_onto_a_table_it_walks_under() {
    // Eye height 0.8: both wall rays miss a slab from 0.3 to 0.4; the floor under it does not.
    let table = room_with_block(0.3, 0.4);
    for rules in [SOLID, CLIMBING] {
        let mut walk = walker([0.0, 0.0, 0.8]);
        ahead(3, &mut walk, rules, &table);
        assert!((walk.eye()[2] - 0.8).abs() < 1e-9, "{rules:?} {:?}", walk.eye());
        assert!(walk.eye()[0] < 2.0, "{rules:?} {:?}", walk.eye());
    }
}

#[test]
fn the_index_walks_as_the_linear_pick_does() {
    let meshes = room_with_block(0.0, 0.2);
    let index = PickIndex::new(&meshes);
    let (mut linear, mut indexed) = (walker([0.0, 0.0, 3.0]), walker([0.0, 0.0, 3.0]));
    let intent = Intent { forward: 1.0, right: 0.3, ..Intent::default() };
    for _ in 0..80 {
        linear.step(intent, 0.05, CLIMBING, &mut |ray| pick(&meshes, ray));
        indexed.step(intent, 0.05, CLIMBING, &mut |ray| index.pick(ray));
    }
    assert_eq!(linear, indexed);
}

#[test]
fn the_camera_turns_apart_from_the_heading_and_centres_back() {
    let meshes = room();
    let mut walk = walker([-5.0, 0.0, 0.8]);
    walk.look(std::f64::consts::FRAC_PI_2, 0.3);
    ahead(1, &mut walk, SOLID, &meshes);
    assert!(walk.eye()[0] > -5.0 && walk.eye()[1].abs() < 1e-9, "{:?}", walk.eye());
    assert!(walk.direction()[1].abs() > 0.9, "{:?}", walk.direction());
    walk.centre_look();
    walk.level();
    assert!((walk.direction()[0] - 1.0).abs() < 1e-9, "{:?}", walk.direction());
}

#[test]
fn turning_and_facing_agree_and_pitch_stops_short_of_the_up_axis() {
    let mut walk = walker([0.0; 3]);
    walk.face([0.0, 1.0, 0.0]);
    assert!((walk.direction()[1] - 1.0).abs() < 1e-9, "{:?}", walk.direction());
    walk.turn(0.0, 10.0);
    assert!((walk.pitch() - PITCH_LIMIT).abs() < 1e-12);
    walk.turn(f64::NAN, 0.0);
    walk.look(0.0, f64::INFINITY);
    walk.look(f64::NAN, 0.0);
    assert!((walk.pitch() - PITCH_LIMIT).abs() < 1e-12 && walk.direction().iter().all(|c| c.is_finite()));
}

#[test]
fn any_number_of_huge_turns_keeps_the_view_valid() {
    let mut walk = walker([0.0; 3]);
    for _ in 0..3 {
        walk.turn(f64::MAX, 0.0);
        walk.look(f64::MAX, 0.0);
    }
    assert!(walk.direction().iter().all(|c| c.is_finite()), "{:?}", walk.direction());
    assert!(walk.camera(1.0).is_ok());
    for power in 0..1000 {
        let angle = 1.37_f64.powi(power);
        for angle in [angle, -angle] {
            let wrapped = wrap(angle);
            assert!(wrapped.abs() <= std::f64::consts::PI, "{angle} wrapped to {wrapped}");
        }
    }
}

fn fov(walk: &Walk, aspect: f64) -> f64 {
    let Projection::Perspective { fov_y } = walk.camera(aspect).unwrap().projection else { panic!("perspective") };
    fov_y
}

#[test]
fn zoom_narrows_the_view_within_its_range() {
    let mut walk = walker([0.0; 3]);
    walk.zoom_by(2.0);
    assert!((fov(&walk, 1.0) - FOV / 2.0).abs() < 1e-9);
    walk.zoom_by(1000.0);
    assert!((walk.zoom() - ZOOM_IN).abs() < 1e-9);
    walk.zoom_by(1e-6);
    assert!((fov(&walk, 1.0) - WIDEST_FOV).abs() < 1e-9, "{}", fov(&walk, 1.0));
    for bad in [0.0, -2.0, f64::NAN, f64::INFINITY] {
        walk.zoom_by(bad);
        assert!((fov(&walk, 1.0) - WIDEST_FOV).abs() < 1e-9, "{bad}");
    }
}

#[test]
fn a_wide_picture_keeps_a_16_9_sideways_field() {
    let walk = walker([0.0; 3]);
    let across = |aspect: f64| 2.0 * ((fov(&walk, aspect).to_radians() / 2.0).tan() * aspect).atan().to_degrees();
    let screen = across(WIDEST);
    assert!((across(3.0) - screen).abs() < 1e-9, "{}", across(3.0));
    assert!((across(5.0) - screen).abs() < 1e-9, "{}", across(5.0));
    assert!((fov(&walk, 4.0 / 3.0) - FOV).abs() < 1e-9, "a narrower picture keeps the vertical field");
    for bad in [f64::NAN, f64::INFINITY, 0.0, -1.0] {
        assert!((fov(&walk, bad) - FOV).abs() < 1e-9, "{bad} counts as 1");
    }
}

#[test]
fn a_set_eye_height_is_stood_at_and_a_bad_one_goes_back_to_the_default() {
    let meshes = room();
    let mut walk = walker([0.0, 0.0, 4.0]);
    walk.set_height(Some(1.7));
    for _ in 0..100 {
        walk.step(Intent::default(), 0.05, SOLID, &mut |ray| pick(&meshes, ray));
    }
    assert!((walk.eye()[2] - 1.7).abs() < 1e-9, "{:?}", walk.eye());
    for bad in [Some(0.0), Some(-1.0), Some(f64::NAN), None] {
        walk.set_height(bad);
        assert!((walk.body() - 0.8).abs() < 1e-12, "{bad:?}");
    }
}

#[test]
fn bad_input_never_moves_the_walker_off_the_finite_range() {
    let meshes = room();
    let mut walk = walker([0.0, 0.0, 0.8]);
    let wild = Intent { forward: f64::NAN, right: f64::INFINITY, rise: -1e300, run: true };
    for dt in [f64::NAN, f64::INFINITY, -1.0, 1e300] {
        walk.step(wild, dt, GHOST, &mut |ray| pick(&meshes, ray));
        walk.step(Intent { forward: 1e300, ..Intent::default() }, dt, Rules { speed: f64::MAX, ..SOLID }, &mut |ray| pick(&meshes, ray));
    }
    assert!(walk.eye().iter().all(|c| c.is_finite()), "{:?}", walk.eye());
    assert!(walk.camera(1.0).is_ok());
    let upright = Upright::default();
    assert!(Walk::standing([f64::NAN, 0.0, 0.0], [1.0, 0.0, 0.0], upright, 1.0).is_none());
    assert!(Walk::standing([0.0; 3], [0.0; 3], upright, 1.0).is_none());
    assert!(Walk::standing([0.0; 3], [1.0, 0.0, 0.0], upright, 0.0).is_none());
    assert!(Walk::standing([0.0; 3], [0.0, 0.0, -1.0], upright, 1.0).is_some(), "looking straight down stops short of it");
}

#[test]
fn a_bad_time_or_speed_loses_no_fall_and_asks_the_model_nothing_out_of_range() {
    let meshes = room();
    let mut world = |ray: &Ray| {
        assert!(ray.origin.iter().chain(&ray.direction).all(|c| c.is_finite()), "{ray:?}");
        pick(&meshes, ray)
    };
    let intent = Intent { forward: 1.0, ..Intent::default() };
    let mut falling = walker([0.0, 0.0, 4.0]);
    falling.step(intent, f64::NAN, SOLID, &mut world);
    assert_eq!(falling.eye(), [0.0, 0.0, 4.0]);
    assert!(!falling.falling());
    let mut standing = walker([0.0, 0.0, 0.8]);
    standing.step(intent, 0.05, Rules { speed: f64::NAN, ..SOLID }, &mut world);
    assert!((standing.eye()[0]).abs() < 1e-12, "{:?}", standing.eye());
}

#[test]
fn speed_is_capped_and_an_overflowing_pace_does_not_move_the_walker() {
    let intent = Intent { forward: 1.0, ..Intent::default() };
    let mut fast = walker([0.0, 0.0, 0.8]);
    fast.step(intent, 0.05, Rules { speed: 1e6, ..GHOST }, &mut |_| None);
    assert!((fast.eye()[0] - 10.0 * PACE * MAX_SPEED * 0.05).abs() < 1e-9, "{:?}", fast.eye());
    let mut endless = walker([0.0, 0.0, 0.8]);
    endless.step(intent, 0.05, Rules { speed: f64::INFINITY, ..GHOST }, &mut |_| None);
    assert_eq!(endless.eye(), [0.0, 0.0, 0.8]);
    let upright = Upright::default().with_front(Axis::PosX);
    let mut huge = Walk::standing([0.0, 0.0, 1.0], [1.0, 0.0, 0.0], upright, f64::MAX).unwrap();
    huge.step(intent, 0.05, Rules { speed: 100.0, ..GHOST }, &mut |_| None);
    assert_eq!(huge.eye(), [0.0, 0.0, 1.0]);
    let run = Intent { run: true, ..intent };
    huge.step(run, 0.05, Rules { speed: 100.0, ..SOLID }, &mut |ray| {
        assert!(ray.origin.iter().chain(&ray.direction).all(|c| c.is_finite()), "{ray:?}");
        None
    });
    assert_eq!(huge.eye(), [0.0, 0.0, 1.0]);
    let upright = Upright::default().with_front(Axis::PosX);
    let mut far = Walk::standing([1.7976e308, 0.0, 1.0], [1.0, 0.0, 0.0], upright, 1e308).unwrap();
    far.step(intent, 0.05, SOLID, &mut |ray| {
        assert!(ray.origin.iter().chain(&ray.direction).all(|c| c.is_finite()), "{ray:?}");
        None
    });
    assert_eq!(far.eye(), [1.7976e308, 0.0, 1.0]);
}

#[test]
fn a_long_frame_moves_no_farther_than_the_longest_step() {
    let mut walk = walker([-5.0, 0.0, 0.8]);
    walk.step(Intent { forward: 1.0, ..Intent::default() }, 60.0, SOLID, &mut |ray| pick(&room(), ray));
    let moved = walk.eye()[0] + 5.0;
    assert!((moved - 10.0 * PACE * MAX_STEP).abs() < 1e-9, "{moved}");
}

#[test]
fn going_to_a_picked_floor_stands_on_it_and_to_a_wall_stands_back_from_it() {
    let meshes = room();
    let mut walk = walker([0.0, 0.0, 0.8]);
    let from = [-2.0, 1.0, 4.0];
    let floor = pick(&meshes, &Ray { origin: from, direction: [0.6, 0.0, -0.8], min_distance: 0.0 }).unwrap();
    walk.go_to(&floor, from);
    assert!(walk.eye().iter().zip([1.0, 1.0, 0.8]).all(|(a, b)| (a - b).abs() < 1e-9), "{:?}", walk.eye());
    let wall = pick(&meshes, &Ray { origin: [0.0, 0.0, 2.0], direction: [1.0, 0.0, 0.0], min_distance: 0.0 }).unwrap();
    walk.go_to(&wall, [0.0, 0.0, 2.0]);
    assert!((walk.eye()[0] - 4.2).abs() < 1e-9, "{:?}", walk.eye());
    let mut falling = walker([0.0, 0.0, 4.0]);
    falling.step(Intent::default(), 0.05, SOLID, &mut |ray| pick(&meshes, ray));
    assert!(falling.falling());
    falling.go_to(&floor, from);
    assert!(!falling.falling());
}

#[test]
fn a_walker_starts_where_a_camera_is_looking_where_it_looks() {
    let camera = Camera { eye: [1.0, 2.0, 3.0], target: [1.0, 5.0, 3.0], up: [0.0, 0.0, 1.0], projection: Projection::Perspective { fov_y: 30.0 } };
    let walk = Walk::from_camera(&camera, Upright::default(), 10.0).unwrap();
    assert_eq!(walk.eye(), [1.0, 2.0, 3.0]);
    assert!((walk.direction()[1] - 1.0).abs() < 1e-9, "{:?}", walk.direction());
    let drawn = walk.camera(1.5).unwrap();
    assert_eq!(drawn.eye, camera.eye);
}
