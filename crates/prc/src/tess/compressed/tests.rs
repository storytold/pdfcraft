#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use super::*;

const S: f64 = std::f64::consts::SQRT_2;

fn run(points: &[i64], status: &[i32], t: usize, is_ref: &[bool], refs: &[u32]) -> Result<Rebuilt, &'static str> {
    rebuild(&Arrays {
        tolerance: 0.5,
        origin: [10.0, 0.0, 0.0],
        points,
        edge_status: status,
        triangles: t,
        is_reference: is_ref,
        references: refs,
        limit: usize::MAX,
    })
}

fn close(a: V, b: V) -> bool {
    (0..3).all(|i| (a[i] - b[i]).abs() < 1e-9)
}

/// Seed `[0 1 2]`, then the apex across `[2 1]`: `X` runs from vertex 2 to vertex 1, and
/// each of `d`'s three components lands on its own axis with its own sign. Tolerance 0.5
/// halves every step.
#[test]
fn a_seed_and_one_apex_rebuild() {
    let pts = [0, 0, 0, 4, 0, 0, -2, 4, 0, 2, -6, 2];
    let m = run(&pts, &[2, 0, 0, 0, 0, 0], 2, &[false; 4], &[]).unwrap();
    assert_eq!(m.triangles, [[0, 1, 2], [2, 1, 3]]);
    let p: Vec<V> = m.positions.iter().map(|v| [v[0] - 10.0, v[1], v[2]]).collect();
    assert!(close(p[0], [0.0, 0.0, 0.0]) && close(p[1], [2.0, 0.0, 0.0]));
    assert!(close(p[2], [0.0, 2.0, 0.0]), "{:?}", p[2]);
    assert!(close(p[3], [1.0 + 2.0 * S, 1.0 + S, -1.0]), "{:?}", p[3]);
}

/// The walk stops once its arrays would hold more than `limit` elements: the seed-and-apex
/// pair ends holding four vertices (12), two triangles (4), five edges (10) and six logged
/// edges (6), 32 in all.
#[test]
fn a_walk_past_its_limit_stops() {
    let pts = [0, 0, 0, 4, 0, 0, -2, 4, 0, 2, -6, 2];
    let with = |limit| {
        rebuild(&Arrays {
            tolerance: 0.5,
            origin: [0.0; 3],
            points: &pts,
            edge_status: &[2, 0, 0, 0, 0, 0],
            triangles: 2,
            is_reference: &[false; 4],
            references: &[],
            limit,
        })
    };
    assert_eq!(with(32).unwrap().triangles.len(), 2);
    assert_eq!(with(31).unwrap_err(), "the rebuild needs more than the decoded-element allowance left");
}

/// The triangles reserved up front fit the limit too.
#[test]
fn the_walk_reserves_within_its_limit() {
    assert!(Walk::new(1 << 20, 10).tris.capacity() < 1 << 20);
    assert!(Walk::new(1 << 20, 10).tris.capacity() <= 5);
    assert!(Walk::new(4, usize::MAX).tris.capacity() >= 4);
}

/// Bit 0 alone continues across `[A C]`, keeping `W = B`.
#[test]
fn the_right_edge_is_entered_from_a_to_c() {
    let pts = [0, 0, 0, 4, 0, 0, -2, 4, 0, 0, -2, 0];
    let m = run(&pts, &[1, 0, 0, 0, 0, 0], 2, &[false; 4], &[]).unwrap();
    assert_eq!(m.triangles[1], [0, 2, 3]);
}

/// A reference slot reuses a vertex; a closed pushed edge is skipped and the next component
/// starts from a fresh seed.
#[test]
fn references_and_components() {
    let pts = [0; 9 + 9];
    let is_ref = [false, false, false, true, false, false, false];
    let m = run(&pts, &[3, 0, 0, 0, 0, 0, 0, 0, 0], 3, &is_ref, &[0]).unwrap();
    assert_eq!(m.triangles, [[0, 1, 2], [2, 1, 0], [3, 4, 5]]);
}

/// A double-sided panel: four coplanar vertices, four triangles. The second triangle's apex
/// folds back onto the seed's side, so its own continuation takes `[A C]` first; the last two
/// apexes are the seed's first two vertices.
#[test]
fn a_folded_apex_swaps_its_continuation() {
    let is_ref = [false, false, false, false, true, true];
    let status = [3, 3, 2, 0, 0, 0, 0, 0, 0, 0, 0, 0];
    let m = run(&[0, 0, 0, 4, 0, 0, -2, 4, 0, 1, 3, 0], &status, 4, &is_ref, &[0, 1]).unwrap();
    assert_eq!(m.triangles, [[0, 1, 2], [2, 1, 3], [2, 3, 0], [0, 3, 1]]);
    // The same panel unfolded is rescued by the component retry.
    let m = run(&[0, 0, 0, 4, 0, 0, -2, 4, 0, 1, -3, 0], &status, 4, &is_ref, &[0, 1]).unwrap();
    assert_eq!(m.triangles, [[0, 1, 2], [2, 1, 3], [2, 3, 0], [0, 3, 1]]);
}

/// The fourth apex `(1, 3, 0)` lies in its parent's plane on the parent's side, so its
/// triangle folds and continues across `[A C]` to the reference.
#[test]
fn a_fold_past_the_retry_point_is_detected() {
    let pts = [0, 0, 0, 4, 0, 0, -2, 4, 0, 0, -3, 1, 1, 3, 0];
    let is_ref = [false, false, false, false, false, true];
    let m = run(&pts, &[3, 3, 2, 0, 0, 0, 0, 0, 0, 0, 0, 0], 4, &is_ref, &[0]).unwrap();
    assert_eq!(m.triangles[3], [3, 4, 0]);
}

/// A panel whose fold apex is a reference carries no fold signal: the unfolded walk closes an
/// edge it still needs, so the component is rewound and walked again folded at its second
/// triangle.
#[test]
fn a_reference_fold_is_found_by_retrying_the_component() {
    let pts: Vec<i64> = (1..=30).collect();
    let mut is_ref = vec![false; 9];
    is_ref.extend([true, true, false, true, true, true]);
    let mut status = vec![0; 21];
    status[3..7].copy_from_slice(&[3, 3, 2, 0]);
    let m = run(&pts, &status, 7, &is_ref, &[0, 3, 6, 0, 3]).unwrap();
    assert_eq!(m.triangles[3..], [[0, 3, 9], [9, 3, 6], [9, 6, 0], [0, 6, 3]]);
}

/// A tetrahedron closed by two references: after `[3 1 0]` the continuation edge `[0 1]` is
/// already shared twice, so the walk takes the open `[3 0]` instead.
#[test]
fn a_closed_continuation_edge_gives_way_to_the_open_one() {
    let pts = [0, 0, 0, 4, 0, 0, -2, 4, 0, 0, 0, 4];
    let mut status = [0; 12];
    status[..3].copy_from_slice(&[2, 2, 2]);
    let mut is_ref = [false; 6];
    is_ref[4..].copy_from_slice(&[true, true]);
    let m = run(&pts, &status, 4, &is_ref, &[0, 2]).unwrap();
    assert_eq!(m.triangles, [[0, 1, 2], [2, 1, 3], [3, 1, 0], [3, 0, 2]]);
}

#[test]
fn arrays_that_do_not_fit_are_refused() {
    let pts = [0; 9];
    let ok = [false; 3];
    assert!(run(&pts, &[0, 0, 0], 1, &ok, &[]).is_ok());
    assert!(run(&pts, &[0], 1, &ok, &[]).is_err(), "T form");
    assert!(run(&[0; 12], &[0, 0, 0], 1, &ok, &[]).is_err(), "points left");
    assert!(run(&[0; 6], &[0, 0, 0], 1, &ok, &[]).is_err(), "points short");
    assert!(run(&pts, &[0, 0, 0], 1, &[false; 4], &[]).is_err(), "slots left");
    assert!(run(&[0; 6], &[0, 0, 0], 1, &[false, false, true], &[7]).is_err(), "forward reference");
    assert!(run(&[0; 9], &[0, 0, 0], 1, &[false, false, false, true], &[0]).is_err(), "references left");
    // A third triangle on the seed's edge [0 1], each apex a reference.
    let three = [false, false, false, true, true];
    let shared = run(&[0; 9], &[2, 2, 0, 0, 0, 0, 0, 0, 0], 3, &three, &[0, 0]).map(|_| ());
    assert_eq!(shared, Err("an edge is shared by more than two triangles"));
}

/// `PrcPt::Length` rounds the squared length to `f32` and takes Newton's root from a halved
/// exponent, which can settle an ulp off the correctly rounded root; `Unitize` refuses
/// lengths below `FLT_EPSILON` [WD 12.3].
#[test]
fn length_and_unitize_follow_the_wd_pseudocode() {
    assert_eq!(wd_length([3.0, 4.0, 0.0]), Some(5.0));
    let newton = f64::from_bits(S.to_bits() - 1);
    assert_eq!(wd_length([1.0, 1.0, 0.0]), Some(newton));
    let long = 1.0 + 2f64.powi(-30);
    assert_eq!(wd_length([long, 0.0, 0.0]), Some(1.0));
    assert_eq!(unitize([0.0, long, 0.0]), Some([0.0, long, 0.0]));
    assert_eq!(unitize([1e-8, 0.0, 0.0]), None);
    assert_eq!(unitize([0.0; 3]), None);
}

/// A `W` on the edge's line leaves `Z` null, so the frame comes from `MakeOrthoRep(X)`: `Z`
/// from `X x (0, 1, 0)`, or from `X x (1, 0, 0)` when `X` is along `Y` [WD 7.8.9.2, 12.3].
#[test]
fn a_null_z_takes_the_ortho_rep_of_x() {
    let d = [1.0, 2.0, 3.0];
    assert_eq!(apex([0.0; 3], [-2.0, 0.0, 0.0], [5.0, 0.0, 0.0], d), [0.0, -2.0, -3.0]);
    assert_eq!(apex([0.0; 3], [0.0, -2.0, 0.0], [0.0, 5.0, 0.0], d), [-2.0, 0.0, 3.0]);
}

/// A degenerate frame is turned a half-turn about `X` when the sub-epsilon residue of
/// `(W - O) x X` points against `MakeOrthoRep`'s `Z`, and kept when it points along it.
#[test]
fn a_degenerate_frame_follows_its_residue() {
    let (lo, hi, d) = ([0.0; 3], [-2.0, 0.0, 0.0], [1.0, 2.0, 3.0]);
    assert_eq!(apex(lo, hi, [5.0, 1e-9, 0.0], d), [0.0, 2.0, 3.0]);
    assert_eq!(apex(lo, hi, [5.0, -1e-9, 0.0], d), [0.0, -2.0, -3.0]);
}

/// A degenerate frame's `Y` comes from the frame's own `X`, whose length `unitize` left off by
/// a few 1e-8, not from the copy `MakeOrthoRep` unitized again: the two differ by that much,
/// and the walk would carry the difference into later apexes.
#[test]
fn a_degenerate_frame_takes_y_from_its_own_x() {
    let (lo, hi) = ([0.1, 0.2, 0.3], [0.7, -0.4, 0.9]);
    let o = scale(add(lo, hi), 0.5);
    let x = unitize(sub(lo, hi)).unwrap();
    let w = add(o, scale(x, 0.37));
    let d = [1.0, 2.0, 3.0];
    let z = make_ortho_rep_z(x).unwrap();
    let p = apex(lo, hi, w, d);
    let own = unitize(cross(z, x)).unwrap();
    let again = unitize(cross(z, unitize(x).unwrap())).unwrap();
    assert_ne!(own, again, "the probe must tell the two Ys apart");
    let sign = if dot(cross(sub(w, o), x), z) < 0.0 { -1.0 } else { 1.0 };
    let expect = |y: V| sub(sub(add(o, scale(x, d[0])), scale(y, sign * d[1])), scale(z, sign * d[2]));
    assert_eq!(p, expect(own));
    assert_ne!(p, expect(again));
}

/// A strip of thin, non-planar triangles compressed by an encoder written out from the WD:
/// each point is expressed in the frame of the working mesh, rounded to the tolerance and
/// reinjected [WD 7.8.9, 7.8.9.2], with `PrcPt::Length` and `Unitize` transcribed from the
/// pseudocode [WD 12.3] as read. The decoder must land exactly on that working mesh, which
/// keeps every vertex within the rounding bound of its source.
#[test]
fn a_strip_decodes_onto_the_encoders_working_mesh() {
    fn length(v: V) -> f64 {
        let f = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]) as f32;
        let mut bits = f.to_bits();
        let exponent = (bits >> 23) & 0xff;
        if exponent > 127 {
            bits = (bits & 0x807f_ffff) | (((exponent - 127) / 2 + 127) << 23);
        }
        let (squared, mut x0) = (f64::from(f), f64::from(f32::from_bits(bits)));
        for _ in 0..100 {
            let xi = 0.5 * (x0 + squared / x0);
            if xi == x0 {
                return xi;
            }
            x0 = xi;
        }
        -1.0
    }
    fn unitized(v: V) -> V {
        let l = length(v);
        assert!(l >= f64::from(f32::EPSILON), "null axis");
        [v[0] / l, v[1] / l, v[2] / l]
    }
    let (tol, n) = (1e-4, 48);
    let mut state = 0x9e37_79b9_7f4a_7c15_u64;
    let mut jitter = |scale: f64| {
        state = state.wrapping_mul(6_364_136_223_846_793_005).wrapping_add(1_442_695_040_888_963_407);
        ((state >> 11) as f64 / (1u64 << 53) as f64 - 0.5) * scale
    };
    // Two rails 4 apart, 0.05 between a rail's points, 0.02 of relief.
    let source: Vec<V> = (0..n)
        .map(|i| {
            let (along, rail) = (i as f64 * 0.025, (i % 2) as f64 * 4.0);
            [along + jitter(0.01), rail + jitter(0.01), jitter(0.02)]
        })
        .collect();
    let round = |v: f64| (v / tol).round() as i64;
    let (mut points, mut work) = (Vec::new(), Vec::<V>::new());
    for (i, target) in source.iter().take(3).enumerate() {
        let base = match i {
            0 => [0.0; 3],
            1 => work[0],
            _ => scale(add(work[0], work[1]), 0.5),
        };
        let d = [0, 1, 2].map(|c| round(target[c] - base[c]));
        points.extend(d);
        work.push(add(base, d.map(|c| c as f64 * tol)));
    }
    // Triangle k enters across vertices k and k + 1, opposite k - 1, and continues right on
    // odd k, left on even k, unless it folds.
    let mut status = vec![0; 3 * (n - 2)];
    status[0] = 2;
    for k in 1..n - 2 {
        let (lo, hi, w) = (work[k], work[k + 1], work[k - 1]);
        let o = scale(add(hi, lo), 0.5);
        let x = unitized(sub(lo, hi));
        let z = unitized(cross(sub(w, o), x));
        let y = unitized(cross(z, x));
        let r = sub(source[k + 2], o);
        let d = [round(dot(r, x)), round(-dot(r, y)), round(-dot(r, z))];
        points.extend(d);
        let [dx, dy, dz] = d.map(|c| c as f64 * tol);
        work.push(sub(sub(add(o, scale(x, dx)), scale(y, dy)), scale(z, dz)));
        if k < n - 3 {
            let fold = d[2] == 0 && d[1] > 0;
            status[k] = if (k % 2 == 1) != fold { 1 } else { 2 };
        }
    }
    let m = rebuild(&Arrays {
        tolerance: tol,
        origin: [0.0; 3],
        points: &points,
        edge_status: &status,
        triangles: n - 2,
        is_reference: &vec![false; n],
        references: &[],
        limit: usize::MAX,
    })
    .unwrap();
    assert_eq!(m.triangles[1..4], [[2, 1, 3], [2, 3, 4], [4, 3, 5]]);
    assert_eq!(m.positions, work);
    for (p, s) in m.positions.iter().zip(&source) {
        let off = dot(sub(*p, *s), sub(*p, *s)).sqrt() / tol;
        assert!(off < 0.87, "{off} tolerances off");
    }
}
