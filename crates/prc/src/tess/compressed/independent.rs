#![allow(
    clippy::cast_possible_truncation,
    clippy::cast_precision_loss,
    clippy::expect_used,
    clippy::float_cmp,
    clippy::many_single_char_names,
    clippy::panic,
    clippy::unwrap_used
)]

use super::*;

const S: f64 = std::f64::consts::SQRT_2;

#[derive(Clone, Copy)]
enum Slot {
    Point(V),
    #[allow(dead_code)]
    Reference(u32),
}

#[derive(Clone, Copy)]
enum Recipe {
    Seed { slots: [Slot; 3], status: i32, face: u32 },
    Apex { edge: (u32, u32), opposite: u32, slot: Slot, status: i32, face: u32 },
}

impl Recipe {
    fn status(self) -> i32 {
        match self {
            Self::Seed { status, .. } | Self::Apex { status, .. } => status,
        }
    }

    fn face(self) -> u32 {
        match self {
            Self::Seed { face, .. } | Self::Apex { face, .. } => face,
        }
    }
}

#[derive(Clone)]
struct Encoded {
    tolerance: f64,
    origin: V,
    points: Vec<i64>,
    edge_status: Vec<i32>,
    face_of: Vec<u32>,
    is_reference: Vec<bool>,
    references: Vec<u32>,
    positions: Vec<V>,
    triangles: Vec<[u32; 3]>,
}

impl Encoded {
    fn arrays(&self, limit: usize) -> Arrays<'_> {
        Arrays {
            tolerance: self.tolerance,
            origin: self.origin,
            points: &self.points,
            edge_status: &self.edge_status,
            triangles: self.triangles.len(),
            is_reference: &self.is_reference,
            references: &self.references,
            limit,
        }
    }

    #[allow(dead_code)]
    fn faces(&self) -> usize {
        self.face_of.iter().max().map_or(0, |&face| face as usize + 1)
    }

    fn push_reference(&mut self, vertex: u32) -> u32 {
        let index = usize::try_from(vertex).unwrap();
        assert!(index < self.positions.len(), "a recipe reference must name an earlier vertex");
        self.is_reference.push(true);
        self.references.push(vertex);
        vertex
    }

    fn push_position(&mut self, position: V, point: [i64; 3]) -> u32 {
        self.points.extend(point);
        self.is_reference.push(false);
        self.positions.push(position);
        u32::try_from(self.positions.len() - 1).unwrap()
    }

    fn push_seed_point(&mut self, index: usize, target: V, triangle: [u32; 3]) -> u32 {
        let base = match index {
            0 => self.origin,
            1 => self.positions[usize::try_from(triangle[0]).unwrap()],
            2 => {
                let first = self.positions[usize::try_from(triangle[0]).unwrap()];
                let second = self.positions[usize::try_from(triangle[1]).unwrap()];
                p_scale(p_add(first, second), 0.5)
            }
            _ => panic!("a seed has exactly three slots"),
        };
        let point = quantize(p_sub(target, base), self.tolerance);
        let position = p_add(base, scaled(point, self.tolerance));
        self.push_position(position, point)
    }

    fn push_apex_point(&mut self, edge: (u32, u32), opposite: u32, target: V) -> u32 {
        let (p, q) = edge;
        let (lo, hi) = if p < q { (p, q) } else { (q, p) };
        let lo = self.positions[usize::try_from(lo).unwrap()];
        let hi = self.positions[usize::try_from(hi).unwrap()];
        let opposite = self.positions[usize::try_from(opposite).unwrap()];
        let (origin, x, y, z) = encoder_frame(lo, hi, opposite);
        let offset = p_sub(target, origin);
        let point = [
            quantize_component(p_dot(offset, x), self.tolerance),
            quantize_component(-p_dot(offset, y), self.tolerance),
            quantize_component(-p_dot(offset, z), self.tolerance),
        ];
        let delta = scaled(point, self.tolerance);
        let position = p_sub(p_sub(p_add(origin, p_scale(x, delta[0])), p_scale(y, delta[1])), p_scale(z, delta[2]));
        self.push_position(position, point)
    }
}

fn encode(origin: V, tolerance: f64, recipes: &[Recipe]) -> Encoded {
    assert!(tolerance.is_finite() && tolerance > 0.0, "a positive finite tolerance");
    let mut edge_status: Vec<i32> = recipes.iter().map(|recipe| recipe.status()).collect();
    edge_status.resize(recipes.len().saturating_mul(3), 0);
    let mut encoded = Encoded {
        tolerance,
        origin,
        points: Vec::new(),
        edge_status,
        face_of: Vec::new(),
        is_reference: Vec::new(),
        references: Vec::new(),
        positions: Vec::new(),
        triangles: Vec::new(),
    };

    for &recipe in recipes {
        encoded.face_of.push(recipe.face());
        match recipe {
            Recipe::Seed { slots, .. } => {
                let mut triangle = [0; 3];
                for (index, slot) in slots.into_iter().enumerate() {
                    triangle[index] = match slot {
                        Slot::Point(target) => encoded.push_seed_point(index, target, triangle),
                        Slot::Reference(vertex) => encoded.push_reference(vertex),
                    };
                }
                encoded.triangles.push(triangle);
            }
            Recipe::Apex { edge: (p, q), opposite, slot, .. } => {
                let vertex = match slot {
                    Slot::Point(target) => encoded.push_apex_point((p, q), opposite, target),
                    Slot::Reference(vertex) => encoded.push_reference(vertex),
                };
                encoded.triangles.push([p, q, vertex]);
            }
        }
    }
    encoded
}

fn p_add(a: V, b: V) -> V {
    [a[0] + b[0], a[1] + b[1], a[2] + b[2]]
}

fn p_sub(a: V, b: V) -> V {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

fn p_scale(a: V, factor: f64) -> V {
    a.map(|component| component * factor)
}

fn p_dot(a: V, b: V) -> f64 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

fn p_cross(a: V, b: V) -> V {
    [a[1] * b[2] - a[2] * b[1], a[2] * b[0] - a[0] * b[2], a[0] * b[1] - a[1] * b[0]]
}

fn quantize_component(value: f64, tolerance: f64) -> i64 {
    let quantized = (value / tolerance).round();
    assert!(quantized.is_finite() && quantized >= i64::MIN as f64 && quantized <= i64::MAX as f64);
    quantized as i64
}

fn quantize(value: V, tolerance: f64) -> [i64; 3] {
    value.map(|component| quantize_component(component, tolerance))
}

fn scaled(point: [i64; 3], tolerance: f64) -> V {
    point.map(|component| component as f64 * tolerance)
}

fn encoder_length(value: V) -> Option<f64> {
    let squared = (value[0] * value[0] + value[1] * value[1] + value[2] * value[2]) as f32;
    let bits = squared.to_bits();
    let exponent = (bits >> 23) & 0xff;
    let seed = if exponent > 127 { f32::from_bits((bits & !(0xff << 23)) | (((exponent - 127) / 2 + 127) << 23)) } else { squared };
    let squared = f64::from(squared);
    let mut current = f64::from(seed);
    for _ in 0..100 {
        let next = 0.5 * (current + squared / current);
        if next == current {
            return Some(next);
        }
        current = next;
    }
    None
}

fn encoder_unit(value: V) -> Option<V> {
    let length = encoder_length(value).filter(|&length| length >= f64::from(f32::EPSILON))?;
    Some([value[0] / length, value[1] / length, value[2] / length])
}

fn encoder_ortho_z(x: V) -> Option<V> {
    let x = encoder_unit(x)?;
    encoder_unit(p_cross(x, [0.0, 1.0, 0.0])).or_else(|| encoder_unit(p_cross(x, [1.0, 0.0, 0.0])))
}

fn encoder_frame(lo: V, hi: V, opposite: V) -> (V, V, V, V) {
    let origin = p_scale(p_add(lo, hi), 0.5);
    let difference = p_sub(lo, hi);
    let x = encoder_unit(difference).unwrap_or(difference);
    let residue = p_cross(p_sub(opposite, origin), x);
    let (y, z) = encoder_unit(residue)
        .and_then(|z| Some((encoder_unit(p_cross(z, x))?, z)))
        .or_else(|| {
            let z = encoder_ortho_z(x)?;
            let y = encoder_unit(p_cross(z, x))?;
            Some(if p_dot(residue, z) < 0.0 { (p_scale(y, -1.0), p_scale(z, -1.0)) } else { (y, z) })
        })
        .unwrap_or_default();
    (origin, x, y, z)
}

fn assert_positions(actual: &[V], expected: &[V]) {
    assert_eq!(actual.len(), expected.len(), "vertex counts differ");
    for (index, (actual, expected)) in actual.iter().zip(expected).enumerate() {
        for component in 0..3 {
            assert!(
                (actual[component] - expected[component]).abs() < 1e-9,
                "vertex {index}, component {component}: actual {actual:?}, expected {expected:?}",
            );
        }
    }
}

#[allow(dead_code)]
fn assert_within_tolerance(actual: &[V], source: &[V], tolerance: f64) {
    assert_eq!(actual.len(), source.len(), "vertex counts differ");
    let limit = tolerance * 3.0_f64.sqrt() / 2.0;
    for (index, (actual, source)) in actual.iter().zip(source).enumerate() {
        let difference = p_sub(*actual, *source);
        let distance = p_dot(difference, difference).sqrt();
        assert!(distance <= limit + 1e-12, "vertex {index}: distance {distance} exceeds {limit}; actual {actual:?}, source {source:?}");
    }
}

fn triangle_seed(x: f64, status: i32, face: u32) -> Recipe {
    Recipe::Seed { slots: [Slot::Point([x, 0.0, 0.0]), Slot::Point([x + 2.0, 0.0, 0.0]), Slot::Point([x, 2.0, 0.0])], status, face }
}

fn panel(target: V) -> Encoded {
    encode(
        [0.0; 3],
        0.5,
        &[
            triangle_seed(0.0, 3, 0),
            Recipe::Apex { edge: (2, 1), opposite: 0, slot: Slot::Point(target), status: 3, face: 0 },
            Recipe::Apex { edge: (2, 3), opposite: 1, slot: Slot::Reference(0), status: 2, face: 0 },
            Recipe::Apex { edge: (0, 3), opposite: 2, slot: Slot::Reference(1), status: 0, face: 0 },
        ],
    )
}

#[test]
fn independent_seed_and_apex_match_a_hand_computed_stream() {
    let encoded = encode(
        [10.0, 0.0, 0.0],
        0.5,
        &[
            Recipe::Seed { slots: [Slot::Point([10.0, 0.0, 0.0]), Slot::Point([12.0, 0.0, 0.0]), Slot::Point([10.0, 2.0, 0.0])], status: 2, face: 0 },
            Recipe::Apex { edge: (2, 1), opposite: 0, slot: Slot::Point([11.0 + 2.0 * S, 1.0 + S, -1.0]), status: 0, face: 0 },
        ],
    );

    assert_eq!(encoded.points, [0, 0, 0, 4, 0, 0, -2, 4, 0, 2, -6, 2]);
    let rebuilt = rebuild(&encoded.arrays(usize::MAX)).unwrap();
    assert_eq!(rebuilt.triangles, [[0, 1, 2], [2, 1, 3]]);
    let expected = [[10.0, 0.0, 0.0], [12.0, 0.0, 0.0], [10.0, 2.0, 0.0], [11.0 + 2.0 * S, 1.0 + S, -1.0]];
    assert_positions(&rebuilt.positions, &expected);
}

#[test]
fn independent_components_and_a_reference_are_rebuilt() {
    let encoded = encode(
        [0.0; 3],
        0.5,
        &[
            triangle_seed(0.0, 2, 0),
            Recipe::Apex { edge: (2, 1), opposite: 0, slot: Slot::Reference(0), status: 0, face: 0 },
            triangle_seed(5.0, 0, 1),
        ],
    );

    let rebuilt = rebuild(&encoded.arrays(usize::MAX)).unwrap();
    assert_eq!(rebuilt.triangles, [[0, 1, 2], [2, 1, 0], [3, 4, 5]]);
    assert_eq!(encoded.references, [0]);
    assert_eq!(encoded.face_of, [0, 0, 1]);
    assert_eq!(rebuilt.positions.len(), 6);
    let expected = [[0.0, 0.0, 0.0], [2.0, 0.0, 0.0], [0.0, 2.0, 0.0], [5.0, 0.0, 0.0], [7.0, 0.0, 0.0], [5.0, 2.0, 0.0]];
    assert_positions(&rebuilt.positions, &expected);
}

#[test]
fn independent_fold_and_component_retry_rebuild_the_same_panel() {
    let folded = [1.0 - 1.0 / S, 1.0 - S, 0.0];
    let unfolded = [1.0 + S, 1.0 + 1.0 / S, 0.0];
    for target in [folded, unfolded] {
        let source = [[0.0, 0.0, 0.0], [2.0, 0.0, 0.0], [0.0, 2.0, 0.0], target];
        let encoded = panel(target);
        assert_within_tolerance(&encoded.positions, &source, 0.5);
        let rebuilt = rebuild(&encoded.arrays(usize::MAX)).unwrap();
        assert_eq!(rebuilt.triangles, [[0, 1, 2], [2, 1, 3], [2, 3, 0], [0, 3, 1]]);
        assert_eq!(rebuilt.positions.len(), 4);
        assert_within_tolerance(&rebuilt.positions, &source, 0.5);
    }
}

#[test]
fn independent_closed_continuation_uses_the_open_edge() {
    let encoded = encode(
        [0.0; 3],
        0.5,
        &[
            triangle_seed(0.0, 2, 0),
            Recipe::Apex { edge: (2, 1), opposite: 0, slot: Slot::Point([0.0, 0.0, 2.0]), status: 2, face: 0 },
            Recipe::Apex { edge: (3, 1), opposite: 2, slot: Slot::Reference(0), status: 2, face: 0 },
            Recipe::Apex { edge: (3, 0), opposite: 1, slot: Slot::Reference(2), status: 0, face: 0 },
        ],
    );

    let source = [[0.0, 0.0, 0.0], [2.0, 0.0, 0.0], [0.0, 2.0, 0.0], [0.0, 0.0, 2.0]];
    assert_within_tolerance(&encoded.positions, &source, 0.5);
    let rebuilt = rebuild(&encoded.arrays(usize::MAX)).unwrap();
    assert_eq!(rebuilt.triangles, [[0, 1, 2], [2, 1, 3], [3, 1, 0], [3, 0, 2]]);
    assert_within_tolerance(&rebuilt.positions, &source, 0.5);
}

#[test]
fn independent_non_planar_strip_tracks_the_quantized_working_mesh() {
    let source: Vec<V> = (0..16).map(|index| [f64::from(index) * 0.05, f64::from(index % 2) * 2.0, (f64::from(index % 5) - 2.0) * 0.03]).collect();
    let mut recipes = vec![Recipe::Seed { slots: [Slot::Point(source[0]), Slot::Point(source[1]), Slot::Point(source[2])], status: 2, face: 0 }];
    let (mut p, mut q, mut opposite) = (2_u32, 1_u32, 0_u32);

    for index in 1..source.len() - 2 {
        let vertex = u32::try_from(index + 2).unwrap();
        let status = if index % 2 == 1 { 1 } else { 2 };
        recipes.push(Recipe::Apex { edge: (p, q), opposite, slot: Slot::Point(source[index + 2]), status, face: 0 });
        if status == 1 {
            (p, q, opposite) = (p, vertex, q);
        } else {
            (p, q, opposite) = (vertex, q, p);
        }
    }

    let encoded = encode([0.0; 3], 1e-4, &recipes);
    assert_within_tolerance(&encoded.positions, &source, 1e-4);
    let rebuilt = rebuild(&encoded.arrays(usize::MAX)).unwrap();
    assert_eq!(rebuilt.triangles.len(), source.len() - 2);
    assert_eq!(rebuilt.triangles[1..4], [[2, 1, 3], [2, 3, 4], [4, 3, 5]]);
    assert_eq!(encoded.points.len(), source.len() * 3);
    assert_within_tolerance(&rebuilt.positions, &source, 1e-4);
}

#[test]
fn independent_near_degenerate_parent_uses_an_orthogonal_frame() {
    let encoded = encode(
        [0.0; 3],
        1e-8,
        &[
            Recipe::Seed {
                slots: [Slot::Point([1.0, 1.0 + 1e-8, 0.0]), Slot::Point([2.0, 0.0, 0.0]), Slot::Point([0.0, 2.0, 0.0])],
                status: 2,
                face: 0,
            },
            Recipe::Apex { edge: (2, 1), opposite: 0, slot: Slot::Point([0.3, 0.7, 0.2]), status: 0, face: 0 },
        ],
    );

    let source = [[1.0, 1.0 + 1e-8, 0.0], [2.0, 0.0, 0.0], [0.0, 2.0, 0.0], [0.3, 0.7, 0.2]];
    assert_within_tolerance(&encoded.positions, &source, 1e-8);
    let rebuilt = rebuild(&encoded.arrays(usize::MAX)).unwrap();
    assert_eq!(rebuilt.triangles, [[0, 1, 2], [2, 1, 3]]);
    assert_within_tolerance(&rebuilt.positions, &source, 1e-8);
}

#[test]
fn independent_rebuild_requires_exact_arrays_and_enough_budget() {
    let encoded = encode([0.0; 3], 0.5, &[triangle_seed(0.0, 0, 0)]);
    assert!(rebuild(&encoded.arrays(usize::MAX)).is_ok());

    let mut extra_point = encoded.clone();
    extra_point.points.push(0);
    assert_eq!(rebuild(&extra_point.arrays(usize::MAX)).unwrap_err(), "the decoded triangles do not use up the stored arrays",);

    let mut extra_slot = encoded.clone();
    extra_slot.is_reference.push(false);
    assert_eq!(rebuild(&extra_slot.arrays(usize::MAX)).unwrap_err(), "the decoded triangles do not use up the stored arrays",);

    let mut extra_reference = encoded.clone();
    extra_reference.references.push(0);
    assert_eq!(rebuild(&extra_reference.arrays(usize::MAX)).unwrap_err(), "the decoded triangles do not use up the stored arrays",);

    assert_eq!(rebuild(&encoded.arrays(0)).unwrap_err(), "the rebuild needs more than the decoded-element allowance left",);
}

#[test]
fn independent_faces_group_walk_order_triangles() {
    let encoded = encode([0.0; 3], 0.5, &[triangle_seed(0.0, 0, 1), triangle_seed(5.0, 0, 0)]);
    let rebuilt = rebuild(&encoded.arrays(usize::MAX)).unwrap();
    let mesh = super::super::by_face(rebuilt, &encoded.face_of, encoded.faces(), &[]);

    assert_eq!(mesh.faces, [0..1, 1..2]);
    assert_eq!(mesh.triangles, [[3, 4, 5], [0, 1, 2]]);
    let expected = [[0.0, 0.0, 0.0], [2.0, 0.0, 0.0], [0.0, 2.0, 0.0], [5.0, 0.0, 0.0], [7.0, 0.0, 0.0], [5.0, 2.0, 0.0]];
    assert_positions(&mesh.positions, &expected);
}
