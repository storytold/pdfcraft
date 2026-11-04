//! Three-component vector arithmetic for the camera and the rasterizer.

/// A point or direction, `[x, y, z]`.
pub(crate) type Vec3 = [f64; 3];

/// `a - b`.
pub(crate) fn sub([ax, ay, az]: Vec3, [bx, by, bz]: Vec3) -> Vec3 {
    [ax - bx, ay - by, az - bz]
}

/// `a` times `s`.
pub(crate) fn scale(a: Vec3, s: f64) -> Vec3 {
    a.map(|c| c * s)
}

/// The dot product `a · b`.
pub(crate) fn dot([ax, ay, az]: Vec3, [bx, by, bz]: Vec3) -> f64 {
    ax * bx + ay * by + az * bz
}

/// The right-handed cross product `a × b`.
pub(crate) fn cross([ax, ay, az]: Vec3, [bx, by, bz]: Vec3) -> Vec3 {
    [ay * bz - az * by, az * bx - ax * bz, ax * by - ay * bx]
}

/// The Euclidean length of `a`, taken with `a` divided by its largest component so squaring
/// cannot overflow or underflow; infinite or NaN when a component is.
pub(crate) fn length(a: Vec3) -> f64 {
    let big = largest(a);
    if !(big.is_finite() && big > 0.0) {
        return dot(a, a).sqrt();
    }
    let a = a.map(|c| c / big);
    big * dot(a, a).sqrt()
}

/// The largest component of `a` by magnitude; NaN components are passed over.
fn largest(a: Vec3) -> f64 {
    a.iter().fold(0.0_f64, |m, c| m.max(c.abs()))
}

/// `a` scaled to unit length; `None` when its length is zero or not finite. `a` is first divided
/// by its largest component, so squaring cannot overflow or underflow.
pub(crate) fn normalize(a: Vec3) -> Option<Vec3> {
    let big = largest(a);
    if !(big.is_finite() && big > 0.0) {
        return None;
    }
    let a = a.map(|c| c / big);
    let l = dot(a, a).sqrt();
    (l.is_finite() && l > 0.0).then(|| scale(a, 1.0 / l))
}

/// The unit normal of the plane spanned by `a` and `b` (`a × b`, normalized); `None` when they are
/// parallel, zero or not finite. Each is normalized first, so neither huge edges nor edges of
/// very different sizes overflow or underflow the cross product.
pub(crate) fn unit_cross(a: Vec3, b: Vec3) -> Option<Vec3> {
    normalize(cross(normalize(a)?, normalize(b)?))
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
        assert_eq!(normalize([f64::NAN, 0.0, 0.0]), None);
        assert_eq!(normalize([f64::INFINITY, 0.0, 0.0]), None);
    }

    #[test]
    fn normalize_keeps_huge_and_tiny_vectors() {
        assert_eq!(normalize([1e200, 0.0, 0.0]), Some([1.0, 0.0, 0.0]));
        assert_eq!(normalize([0.0, -3e-320, 0.0]), Some([0.0, -1.0, 0.0]));
    }

    #[test]
    fn unit_cross_survives_edges_whose_product_overflows() {
        assert_eq!(unit_cross([1e160, 0.0, 0.0], [0.0, 1e160, 0.0]), Some([0.0, 0.0, 1.0]));
        assert_eq!(unit_cross([1e-170, 0.0, 0.0], [0.0, 1e-170, 0.0]), Some([0.0, 0.0, 1.0]));
        assert_eq!(unit_cross([2.0, 0.0, 0.0], [4.0, 0.0, 0.0]), None);
        assert_eq!(unit_cross([f64::INFINITY, 0.0, 0.0], [0.0, 1.0, 0.0]), None);
    }

    #[test]
    fn unit_cross_survives_edges_of_very_different_sizes() {
        assert_eq!(unit_cross([1e-170, 0.0, 0.0], [0.0, 1e160, 0.0]), Some([0.0, 0.0, 1.0]));
    }

    #[test]
    fn length_survives_huge_and_tiny_vectors() {
        let near = |l: f64, want: f64| (l - want).abs() <= want * 1e-15;
        assert!(near(length([3e200, 4e200, 0.0]), 5e200));
        assert!(near(length([0.0, 3e-200, 4e-200]), 5e-200));
        assert_eq!(length([0.0; 3]), 0.0);
        assert!(length([f64::NAN, 1.0, 0.0]).is_nan());
        assert_eq!(length([f64::INFINITY, 1.0, 0.0]), f64::INFINITY);
    }
}
