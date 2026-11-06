//! Three-component vector arithmetic for the writer's normals.

/// A point or direction, `[x, y, z]`.
pub(crate) type Vec3 = [f64; 3];

/// `a - b`.
pub(crate) fn sub(a: Vec3, b: Vec3) -> Vec3 {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

/// The dot product `a · b`.
pub(crate) fn dot(a: Vec3, b: Vec3) -> f64 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

/// The right-handed cross product `a × b`.
pub(crate) fn cross(a: Vec3, b: Vec3) -> Vec3 {
    [a[1] * b[2] - a[2] * b[1], a[2] * b[0] - a[0] * b[2], a[0] * b[1] - a[1] * b[0]]
}

/// `a` scaled to unit length; `None` when its length is zero or not finite. Divided by its
/// largest component first, so a finite vector too long or too short to square keeps its direction.
pub(crate) fn normalize(a: Vec3) -> Option<Vec3> {
    // `f64::max` passes over a NaN, so finiteness is checked on every component.
    let big = a.iter().fold(0.0f64, |m, c| m.max(c.abs()));
    if !(a.iter().all(|c| c.is_finite()) && big > 0.0) {
        return None;
    }
    let a = a.map(|c| c / big);
    let l = dot(a, a).sqrt();
    Some(a.map(|c| c / l))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cross_follows_the_right_hand_rule() {
        assert_eq!(cross([1.0, 0.0, 0.0], [0.0, 1.0, 0.0]), [0.0, 0.0, 1.0]);
        assert_eq!(cross([0.0, 1.0, 0.0], [1.0, 0.0, 0.0]), [0.0, 0.0, -1.0]);
    }

    #[test]
    fn normalize_rejects_degenerate_vectors() {
        assert_eq!(normalize([0.0, 4.0, 0.0]), Some([0.0, 1.0, 0.0]));
        assert_eq!(normalize([0.0; 3]), None);
        assert_eq!(normalize([f64::INFINITY, 0.0, 0.0]), None);
        assert_eq!(normalize([f64::NAN, 1.0, 0.0]), None);
        assert_eq!(normalize([0.0, 0.0, 1e300]), Some([0.0, 0.0, 1.0]));
        assert_eq!(normalize([0.0, -1e-300, 0.0]), Some([0.0, -1.0, 0.0]));
    }
}
