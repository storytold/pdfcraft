//! Model-space vectors and matrices, and their application to meshes.

use crate::TriangleMesh;

/// A point or direction in model space.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub(crate) struct Vec3(pub(crate) [f64; 3]);

impl Vec3 {
    /// The right-handed cross product `self × b`.
    pub(crate) fn cross(self, b: Vec3) -> Vec3 {
        let ([ax, ay, az], [bx, by, bz]) = (self.0, b.0);
        Vec3([ay * bz - az * by, az * bx - ax * bz, ax * by - ay * bx])
    }
}

/// A 4×4 matrix, `m.0[row][col]`, acting on column vectors.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Matrix(pub [[f64; 4]; 4]);

impl Default for Matrix {
    fn default() -> Self {
        Matrix::IDENTITY
    }
}

impl Matrix {
    /// The identity matrix.
    pub const IDENTITY: Matrix = Matrix([[1.0, 0.0, 0.0, 0.0], [0.0, 1.0, 0.0, 0.0], [0.0, 0.0, 1.0, 0.0], [0.0, 0.0, 0.0, 1.0]]);

    /// The entry at `row`, `col`; 0 outside the matrix.
    #[must_use]
    pub fn at(&self, row: usize, col: usize) -> f64 {
        self.0.get(row).and_then(|r| r.get(col)).copied().unwrap_or(0.0)
    }

    /// `self × b`: `b` applied first.
    #[must_use]
    pub fn multiply(&self, b: &Matrix) -> Matrix {
        let mut m = [[0.0; 4]; 4];
        for (i, row) in m.iter_mut().enumerate() {
            for (j, cell) in row.iter_mut().enumerate() {
                *cell = (0..4).map(|k| self.at(i, k) * b.at(k, j)).sum();
            }
        }
        Matrix(m)
    }

    /// `self × (p, 1)`, divided by the homogeneous coordinate when it is neither 0 nor 1.
    #[must_use]
    pub fn transform_point(&self, p: [f64; 3]) -> [f64; 3] {
        let [x, y, z] = p;
        let row = |i: usize| self.at(i, 0) * x + self.at(i, 1) * y + self.at(i, 2) * z + self.at(i, 3);
        let w = row(3);
        let s = if w == 0.0 || w == 1.0 { 1.0 } else { 1.0 / w };
        [row(0) * s, row(1) * s, row(2) * s]
    }

    /// The determinant of the upper-left 3×3 block.
    fn det3(&self) -> f64 {
        let a = |i, j| self.at(i, j);
        a(0, 0) * (a(1, 1) * a(2, 2) - a(1, 2) * a(2, 1)) - a(0, 1) * (a(1, 0) * a(2, 2) - a(1, 2) * a(2, 0))
            + a(0, 2) * (a(1, 0) * a(2, 1) - a(1, 1) * a(2, 0))
    }

    /// Cofactor `(i, j)` of the upper-left 3×3 block (`i`, `j` < 3).
    fn cofactor(&self, i: usize, j: usize) -> f64 {
        let (r0, r1) = ((i + 1) % 3, (i + 2) % 3);
        let (c0, c1) = ((j + 1) % 3, (j + 2) % 3);
        self.at(r0, c0) * self.at(r1, c1) - self.at(r0, c1) * self.at(r1, c0)
    }
}

impl TriangleMesh {
    /// This mesh with every position mapped through `m` and every stored normal through its
    /// inverse transpose (renormalised); a mirroring `m` (negative determinant) reverses each
    /// triangle so the outside stays counter-clockwise. A projective `m` (a last row other than
    /// `[0, 0, 0, w]`) bends the surface per point, so the stored normals are dropped.
    ///
    /// ```
    /// use pdfcraft_prc::{Matrix, TriangleMesh};
    /// let mut mirror = Matrix::IDENTITY;
    /// mirror.0[0][0] = -1.0;
    /// let mut mesh = TriangleMesh::default();
    /// mesh.positions = vec![[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]];
    /// mesh.triangles = vec![[0, 1, 2]];
    /// let placed = mesh.transformed(&mirror);
    /// assert_eq!(placed.positions, [[-1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]]);
    /// assert_eq!(placed.triangles, [[0, 2, 1]]);
    /// ```
    #[must_use]
    pub fn transformed(&self, m: &Matrix) -> Self {
        let mut out = self.clone();
        for p in &mut out.positions {
            *p = m.transform_point(*p);
        }
        // The cofactor matrix is det times the inverse transpose, so its sign is corrected
        // and the result renormalised.
        let det = m.det3();
        let sign = if det < 0.0 { -1.0 } else { 1.0 };
        if m.at(3, 0) != 0.0 || m.at(3, 1) != 0.0 || m.at(3, 2) != 0.0 {
            out.normals.clear();
            out.triangle_normals.clear();
        }
        for n in &mut out.normals {
            let [x, y, z] = *n;
            let v = [0, 1, 2].map(|i| sign * (m.cofactor(i, 0) * x + m.cofactor(i, 1) * y + m.cofactor(i, 2) * z));
            let len = v.iter().map(|c| c * c).sum::<f64>().sqrt();
            *n = if len > 0.0 && len.is_finite() { v.map(|c| c / len) } else { v };
        }
        if det < 0.0 {
            for t in &mut out.triangles {
                t.swap(1, 2);
            }
            for t in &mut out.triangle_normals {
                t.swap(1, 2);
            }
            for t in out.triangle_uvs.iter_mut().flatten().flatten() {
                t.swap(1, 2);
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cross_follows_the_right_hand_rule() {
        assert_eq!(Vec3([1.0, 0.0, 0.0]).cross(Vec3([0.0, 1.0, 0.0])), Vec3([0.0, 0.0, 1.0]));
        assert_eq!(Vec3([0.0, 1.0, 0.0]).cross(Vec3([1.0, 0.0, 0.0])), Vec3([0.0, 0.0, -1.0]));
    }

    #[test]
    fn multiply_applies_the_right_operand_first() {
        let mut t = Matrix::IDENTITY;
        t.0[0][3] = 5.0;
        let mut s = Matrix::IDENTITY;
        s.0[0][0] = 2.0;
        assert_eq!(t.multiply(&s).transform_point([1.0, 0.0, 0.0]), [7.0, 0.0, 0.0]);
        assert_eq!(s.multiply(&t).transform_point([1.0, 0.0, 0.0]), [12.0, 0.0, 0.0]);
    }

    #[test]
    fn a_projective_point_is_divided_by_w() {
        let mut m = Matrix::IDENTITY;
        m.0[3][3] = 2.0;
        assert_eq!(m.transform_point([2.0, 4.0, 6.0]), [1.0, 2.0, 3.0]);
        m.0[3][3] = 0.0;
        assert_eq!(m.transform_point([2.0, 4.0, 6.0]), [2.0, 4.0, 6.0]);
    }

    #[test]
    fn normals_take_the_inverse_transpose_and_mirror_with_their_triangle() {
        // A shear in x by y turns the plane x = 0 into x = y; its x normal tilts to stay perpendicular.
        let mut shear = Matrix::IDENTITY;
        shear.0[0][1] = 1.0;
        let mesh = TriangleMesh {
            positions: vec![[0.0; 3], [1.0, 0.0, 0.0], [0.0, 0.0, 1.0]],
            triangles: vec![[0, 1, 2]],
            normals: vec![[1.0, 0.0, 0.0]],
            triangle_normals: vec![[0, 0, 0]],
            ..TriangleMesh::default()
        };
        let n = shear.transformed_normal(&mesh);
        let s = std::f64::consts::FRAC_1_SQRT_2;
        assert!((n[0] - s).abs() < 1e-12 && (n[1] + s).abs() < 1e-12 && n[2] == 0.0, "{n:?}");

        let mut mirror = Matrix::IDENTITY;
        mirror.0[2][2] = -1.0;
        let placed = mesh.transformed(&mirror);
        assert_eq!(placed.triangles, [[0, 2, 1]]);
        assert_eq!(placed.triangle_normals, [[0, 0, 0]]);
        assert_eq!(placed.normals, [[1.0, 0.0, 0.0]]);
    }

    #[test]
    fn a_projective_transform_drops_the_stored_normals() {
        let mesh = TriangleMesh {
            positions: vec![[0.0; 3], [1.0, 0.0, 0.0], [0.0, 0.0, 1.0]],
            triangles: vec![[0, 1, 2]],
            normals: vec![[1.0, 0.0, 1.0]],
            triangle_normals: vec![[0, 0, 0]],
            ..TriangleMesh::default()
        };
        let mut projective = Matrix::IDENTITY;
        projective.0[3][0] = 1.0;
        let placed = mesh.transformed(&projective);
        assert!(placed.normals.is_empty() && placed.triangle_normals.is_empty());
        // A uniform w keeps them.
        let mut uniform = Matrix::IDENTITY;
        uniform.0[3][3] = 2.0;
        assert_eq!(mesh.transformed(&uniform).triangle_normals, [[0, 0, 0]]);
    }

    impl Matrix {
        fn transformed_normal(&self, mesh: &TriangleMesh) -> [f64; 3] {
            mesh.transformed(self).normals.first().copied().unwrap_or_default()
        }
    }
}
