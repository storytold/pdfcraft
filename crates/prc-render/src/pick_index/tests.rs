#![allow(clippy::unwrap_used, clippy::expect_used)]

use super::*;
use crate::pick;
use crate::vec3::{normalize, sub};
use pdfcraft_prc::TriangleMesh;

/// A deterministic scatter of `n` small triangles in a 10-unit cube.
fn scatter(n: usize, seed: u64) -> WorldMesh {
    let mut s = seed;
    let mut next = || {
        s = s.wrapping_mul(6_364_136_223_846_793_005).wrapping_add(1_442_695_040_888_963_407);
        (s >> 11) as f64 / (1_u64 << 53) as f64 * 10.0
    };
    let mut mesh = TriangleMesh::default();
    for i in 0..n {
        let c = [next(), next(), next()];
        for k in 0..3 {
            let mut p = c;
            p[k] += 0.8;
            mesh.positions.push(p);
        }
        let b = u32::try_from(i * 3).unwrap();
        mesh.triangles.push([b, b + 1, b + 2]);
    }
    WorldMesh::new(mesh)
}

fn rays() -> Vec<Ray> {
    let slant = normalize([0.3, -0.2, 1.0]).unwrap();
    let mut out = Vec::new();
    for i in 0..20 {
        for j in 0..20 {
            let (x, y) = (f64::from(i) * 0.55, f64::from(j) * 0.55);
            out.push(Ray { origin: [x, y, -5.0], direction: [0.0, 0.0, 1.0], min_distance: 0.0 });
            out.push(Ray { origin: [x, y, 5.0], direction: slant, min_distance: f64::NEG_INFINITY });
        }
    }
    out
}

#[test]
fn the_index_answers_exactly_as_the_linear_pick() {
    let meshes = vec![scatter(300, 1), scatter(200, 2), WorldMesh::default()];
    let index = PickIndex::new(&meshes);
    let mut hits = 0;
    for ray in rays() {
        let want = pick(&meshes, &ray);
        hits += usize::from(want.is_some());
        assert_eq!(index.pick(&ray), want, "{ray:?}");
    }
    assert!(hits > 50, "the scatter must actually be hit ({hits})");
}

#[test]
fn a_masked_mesh_is_never_hit() {
    let meshes = vec![scatter(300, 1), scatter(200, 2)];
    let index = PickIndex::new(&meshes);
    let only_second = [meshes[1].clone()];
    for ray in rays() {
        let got = index.pick_where(&ray, f64::INFINITY, |m| m == 1);
        assert_eq!(got, pick(&only_second, &ray).map(|h| Hit { mesh: 1, ..h }), "{ray:?}");
    }
}

#[test]
fn a_maximum_distance_drops_farther_hits() {
    let index = PickIndex::new(&[scatter(300, 1)]);
    let mut dropped = 0;
    for ray in rays() {
        let full = index.pick(&ray);
        let limited = index.pick_where(&ray, 8.0, |_| true);
        match full {
            Some(h) if h.distance <= 8.0 => assert_eq!(limited, full),
            Some(_) => {
                dropped += 1;
                assert_eq!(limited, None);
            }
            None => assert_eq!(limited, None),
        }
        assert_eq!(index.pick_where(&ray, f64::NAN, |_| true), full, "NaN sets no limit");
    }
    assert!(dropped > 0, "some hit must lie beyond the limit");
}

#[test]
fn coincident_triangles_tie_to_the_lower_index() {
    let one = scatter(1, 7);
    let c = centroid(&[one.mesh.positions[0], one.mesh.positions[1], one.mesh.positions[2]]);
    let meshes = vec![one; 9];
    let ray = Ray { origin: [c[0], c[1], c[2] - 10.0], direction: [0.0, 0.0, 1.0], min_distance: 0.0 };
    let want = pick(&meshes, &ray);
    assert_eq!(want.map(|h| h.mesh), Some(0), "the ray must meet the first copy");
    assert_eq!(PickIndex::new(&meshes).pick(&ray), want);
    // Whatever order the tree visits the copies in, an equal distance never displaces a lower index.
    let hit = want.unwrap();
    let later = Hit { mesh: 5, ..hit };
    assert!(nearer(&hit, &later) && !nearer(&later, &hit));
    assert!(nearer(&Hit { triangle: 0, ..later }, &Hit { triangle: 1, ..later }));
}

/// Rays aimed exactly at corners and edges, where the box and triangle tests round differently,
/// find what the linear pick finds.
#[test]
fn rays_at_corners_and_edges_are_never_lost() {
    let meshes = vec![scatter(200, 11), scatter(200, 12)];
    let index = PickIndex::new(&meshes);
    let mut checked = 0;
    for (m, world) in meshes.iter().enumerate() {
        for (t, tri) in world.mesh.triangles.iter().enumerate().take(60) {
            let [a, b, c] = tri.map(|i| world.mesh.positions[i as usize]);
            let mid = |p: [f64; 3], q: [f64; 3]| [0, 1, 2].map(|i| p[i] / 2.0 + q[i] / 2.0);
            for aim in [a, b, c, mid(a, b), mid(b, c), mid(c, a)] {
                for from in [[-3.0, -4.0, -7.0], [13.0, 12.0, 17.0], [aim[0] + 0.1, aim[1] - 0.3, -20.0]] {
                    let Some(direction) = normalize(sub(aim, from)) else { continue };
                    let ray = Ray { origin: from, direction, min_distance: 0.0 };
                    assert_eq!(index.pick(&ray), pick(&meshes, &ray), "mesh {m} triangle {t} aim {aim:?} from {from:?}");
                    checked += 1;
                }
            }
        }
    }
    assert!(checked > 1000);
}

/// What the renderer does not draw, the index does not hold.
#[test]
fn a_fully_transparent_or_broken_triangle_is_left_out() {
    let mut clear = scatter(300, 1);
    clear.colour = Some([1, 2, 3, 0]);
    let mut broken = scatter(1, 3);
    broken.mesh.triangles.push([0, 1, 99]);
    broken.mesh.positions[0][0] = f64::NAN;
    let index = PickIndex::new(&[clear, broken]);
    assert!(index.prims.is_empty(), "{:?}", index.prims.len());
    assert!(rays().iter().all(|r| index.pick(r).is_none()));
}

/// The worst split still builds a shallow tree: median splits halve each range.
#[test]
fn the_tree_stays_shallow() {
    fn depth(index: &PickIndex, n: usize) -> usize {
        match index.nodes.get(n) {
            Some(Node::Inner { left, right, .. }) => 1 + depth(index, *left).max(depth(index, *right)),
            _ => 1,
        }
    }
    // Every triangle at one point puts all centroids on one value, the worst split.
    let mut mesh = TriangleMesh::default();
    mesh.positions = vec![[0.0; 3], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]];
    mesh.triangles = vec![[0, 1, 2]; 4096];
    let index = PickIndex::new(&[WorldMesh::new(mesh)]);
    assert!(depth(&index, 0) <= 12, "{}", depth(&index, 0));
}
