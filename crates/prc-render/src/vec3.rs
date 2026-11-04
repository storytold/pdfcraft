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

/// The Euclidean length of `a`.
pub(crate) fn length(a: Vec3) -> f64 {
    dot(a, a).sqrt()
}

/// `a` scaled to unit length; `None` when its length is zero or not finite.
pub(crate) fn normalize(a: Vec3) -> Option<Vec3> {
    let l = length(a);
    (l.is_finite() && l > 0.0).then(|| scale(a, 1.0 / l))
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
    }
}
