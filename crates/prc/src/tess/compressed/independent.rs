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
    assert_positions(&rebuilt.positions, &encoded.positions);
}
