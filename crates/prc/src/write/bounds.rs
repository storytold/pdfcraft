//! The box a scene's triangles fill, for fitting a camera to it.

use super::MeshScene;

impl MeshScene {
    /// The smallest axis-aligned box `[min, max]` holding every vertex of every triangle the
    /// scene draws, placed by its nodes' matrices, in scene units. `None` when it draws no
    /// triangle or a placed coordinate is not finite. Each node is visited once, so a scene that
    /// is not a forest (which [`write_prc`](super::write_prc) refuses) still ends.
    #[must_use]
    pub fn bounds(&self) -> Option<[[f64; 3]; 2]> {
        let mut seen = vec![false; self.nodes.len()];
        let mut stack: Vec<(usize, [f64; 16])> = self.roots.iter().map(|&r| (r, IDENTITY)).collect();
        let mut out: Option<[[f64; 3]; 2]> = None;
        while let Some((index, parent)) = stack.pop() {
            let (Some(node), Some(visited)) = (self.nodes.get(index), seen.get_mut(index)) else { continue };
            if std::mem::replace(visited, true) {
                continue;
            }
            let world = node.matrix.map_or(parent, |m| multiply(&parent, &m));
            for p in node.mesh.and_then(|m| self.meshes.get(m)).into_iter().flat_map(|m| &m.primitives) {
                for i in p.triangles.iter().flatten() {
                    let Some(v) = usize::try_from(*i).ok().and_then(|i| p.positions.get(i)) else { continue };
                    let q = transform(&world, *v);
                    if !q.iter().all(|c| c.is_finite()) {
                        return None;
                    }
                    let [lo, hi] = out.get_or_insert([q, q]);
                    for ((l, h), c) in lo.iter_mut().zip(hi.iter_mut()).zip(q) {
                        *l = l.min(c);
                        *h = h.max(c);
                    }
                }
            }
            stack.extend(node.children.iter().map(|&c| (c, world)));
        }
        out
    }
}

const IDENTITY: [f64; 16] = [1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0];

/// `a × b` for column-major 4×4 matrices.
fn multiply(a: &[f64; 16], b: &[f64; 16]) -> [f64; 16] {
    let at = |m: &[f64; 16], row: usize, col: usize| m.get(col * 4 + row).copied().unwrap_or(0.0);
    std::array::from_fn(|k| (0..4).map(|j| at(a, k % 4, j) * at(b, j, k / 4)).sum())
}

/// The point `p` placed by the column-major matrix `m`.
fn transform(m: &[f64; 16], p: [f64; 3]) -> [f64; 3] {
    let at = |i: usize| m.get(i).copied().unwrap_or(0.0);
    let [x, y, z] = p;
    std::array::from_fn(|r| at(r) * x + at(4 + r) * y + at(8 + r) * z + at(12 + r))
}
