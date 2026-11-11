//! Triangle reconstruction for a `TESS_3D_Compressed` (173) whose normals are recalculated
//! [WD 7.8.9, 7.8.9.2, 7.8.9.7].
//!
//! The WD names the arrays but not the traversal that wrote them; the rules below are the ones
//! the arrays fit when every slot, reference and point is consumed exactly. They are inferred
//! from compressed tessellations and are not WD text.
//!
//! Traversal. A component starts with a seed triangle of three slots; every later triangle
//! takes one slot, its apex. A slot flagged in `point_is_reference` takes the next
//! `point_reference_array` value, a vertex number; any other slot makes the next vertex from
//! the next `point_array` triple. Triangle `[A B C]`, entered across `[A B]`, reads
//! `edge_status & 3`: bit 1 continues across `[C B]`, bit 0 across `[A C]`; with both,
//! `[C B]` is taken and `[A C]` pushed. With neither, pushed edges are popped, skipping any
//! already shared by two triangles; an empty stack starts the next component. A triangle
//! whose continuation edge is already shared by two triangles, while the other edge is open,
//! continues across the open one.
//!
//! Geometry, in units of `tolerance`:
//! - seed: `V0 = origin + d`, `V1 = V0 + d`, `V2 = mid(V0, V1) + d`;
//! - apex across `[P Q]` with the triangle's third vertex `W` [WD 7.8.9.2]: with
//!   `O = (P + Q) * 0.5`, `L` and `H` the lower- and higher-numbered of `P`, `Q`,
//!   `X = unitize(L - H)`, `Z = unitize((W - O) x X)` and `Y = unitize(Z x X)`, the apex is
//!   `((O + d.x X) - d.y Y) - d.z Z`.
//!
//! Arithmetic. The encoder frames each apex on the points it already approximated [WD 7.8.9],
//! so the decoder repeats the encoder's floating point: a last-bit difference compounds from
//! apex to apex. `unitize` therefore matches the WD's `PrcPt::Unitize` pseudocode [WD 12.3] as
//! read, whose length is not exact, and the operations keep the order above.
//!
//! Fold. A triangle folded against its parent swaps bits 1 and 0 for its own continuation.
//! Without stored normals to orient it, a new apex lying in its parent's plane on the parent's
//! side (`d.z == 0`, `d.y > 0`) is folded. A component that does not fit is rewound and walked
//! again with the fold inverted at its second triangle, where an apex that is a reference
//! leaves no other trace of its fold.
//!
//! Only the three-statuses-per-triangle `edge_status` form is reconstructed (the first `T`
//! are read); the one-per-triangle form walks differently.

use std::collections::HashMap;

type V = [f64; 3];

/// The arrays of one `TESS_3D_Compressed` the walk reads.
pub(super) struct Arrays<'a> {
    pub(super) tolerance: f64,
    pub(super) origin: V,
    pub(super) points: &'a [i64],
    pub(super) edge_status: &'a [i32],
    pub(super) triangles: usize,
    pub(super) is_reference: &'a [bool],
    pub(super) references: &'a [u32],
    /// Budget elements the walk may hold at once (see [`Walk::held`]).
    pub(super) limit: usize,
}

/// The rebuilt vertices and triangles, in walk order.
#[derive(Debug)]
pub(super) struct Rebuilt {
    pub(super) positions: Vec<V>,
    pub(super) triangles: Vec<[u32; 3]>,
}

/// Rebuilds the mesh, or says why the arrays do not fit the traversal.
pub(super) fn rebuild(a: &Arrays<'_>) -> Result<Rebuilt, &'static str> {
    if a.edge_status.len() != a.triangles.saturating_mul(3) {
        return Err("the one-status-per-triangle edge form is not reconstructed");
    }
    let mut w = Walk::new(a.triangles, a.limit);
    w.walk(a)?;
    if w.slot != a.is_reference.len() || w.ri != a.references.len() || w.pi != a.points.len() {
        return Err("the decoded triangles do not use up the stored arrays");
    }
    Ok(Rebuilt { positions: w.pos, triangles: w.tris })
}

fn add(a: V, b: V) -> V {
    [a[0] + b[0], a[1] + b[1], a[2] + b[2]]
}
fn sub(a: V, b: V) -> V {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}
fn scale(a: V, s: f64) -> V {
    a.map(|c| c * s)
}
fn dot(a: V, b: V) -> f64 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}
fn cross(a: V, b: V) -> V {
    [a[1] * b[2] - a[2] * b[1], a[2] * b[0] - a[0] * b[2], a[0] * b[1] - a[1] * b[0]]
}
fn key(a: u32, b: u32) -> (u32, u32) {
    if a < b { (a, b) } else { (b, a) }
}

/// `PrcPt::Length` [WD 12.3]: the squared length summed in `f64` and rounded to `f32`, then
/// Newton's square root in `f64`, seeded with that `f32` with its exponent halved. The printed
/// loop never advances its iterate; it is read as iterating until two successive values are
/// equal, `None` (the WD's -1) when 100 steps do not settle.
fn wd_length(a: V) -> Option<f64> {
    let squared = (a[0] * a[0] + a[1] * a[1] + a[2] * a[2]) as f32;
    let bits = squared.to_bits();
    let exponent = (bits >> 23) & 0xff;
    let seed = if exponent > 127 { f32::from_bits((bits & !(0xff << 23)) | (((exponent - 127) / 2 + 127) << 23)) } else { squared };
    let squared = f64::from(squared);
    let mut x = f64::from(seed);
    for _ in 0..100 {
        let next = 0.5 * (x + squared / x);
        if next == x {
            return Some(next);
        }
        x = next;
    }
    None
}

/// `PrcPt::Unitize` [WD 12.3]: each component divided by [`wd_length`]; `None` below
/// `FLT_EPSILON`.
fn unitize(a: V) -> Option<V> {
    let l = wd_length(a).filter(|&l| l >= f64::from(f32::EPSILON))?;
    Some([a[0] / l, a[1] / l, a[2] / l])
}

/// The `Z` of `PrcPt::MakeOrthoRep` [WD 12.3] on `X`: `unitize(X x (0, 1, 0))`, or
/// `X x (1, 0, 0)` when that is null, from `X` unitized again as the pseudocode does.
fn make_ortho_rep_z(x: V) -> Option<V> {
    let x = unitize(x)?;
    unitize(cross(x, [0.0, 1.0, 0.0])).or_else(|| unitize(cross(x, [1.0, 0.0, 0.0])))
}

/// The apex across `[lo hi]` (`lo` the lower-numbered vertex) opposite `w`, `d` already scaled
/// by the tolerance [WD 7.8.9.2]. A null `Z` or `Y` takes `MakeOrthoRep(X)`'s `Z` as the WD
/// prescribes, with `Y = unitize(Z x X)` from the frame's own `X` (not the copy `MakeOrthoRep`
/// unitized again, whose last bits differ), turned a half-turn about `X` when the residue of
/// `(W - O) x X` points against that `Z`. A null `X`, an edge the WD's non-degeneracy rule
/// excludes, is used as it is.
fn apex(lo: V, hi: V, w: V, d: V) -> V {
    let o = scale(add(lo, hi), 0.5);
    let x = sub(lo, hi);
    let x = unitize(x).unwrap_or(x);
    let residue = cross(sub(w, o), x);
    let (y, z) = unitize(residue)
        .and_then(|z| Some((unitize(cross(z, x))?, z)))
        .or_else(|| {
            let z = make_ortho_rep_z(x)?;
            let y = unitize(cross(z, x))?;
            Some(if dot(residue, z) < 0.0 { (scale(y, -1.0), scale(z, -1.0)) } else { (y, z) })
        })
        .unwrap_or_default();
    sub(sub(add(o, scale(x, d[0])), scale(y, d[1])), scale(z, d[2]))
}

/// Traversal state; `log` records every edge-count increment so a component can be rewound.
/// Everything it holds is checked against [`Arrays::limit`] after each triangle.
#[derive(Default)]
struct Walk {
    pos: Vec<V>,
    tris: Vec<[u32; 3]>,
    edges: HashMap<(u32, u32), u8>,
    log: Vec<(u32, u32)>,
    stack: Vec<(u32, u32, u32)>,
    next: Option<(u32, u32, u32)>,
    slot: usize,
    ri: usize,
    pi: usize,
}

/// Where a component starts, to rewind to.
#[derive(Clone, Copy)]
struct Mark {
    tri: usize,
    pos: usize,
    log: usize,
    slot: usize,
    ri: usize,
    pi: usize,
}

impl Walk {
    /// Reserves room for `t` triangles, no more than `limit` elements' worth (two each).
    fn new(t: usize, limit: usize) -> Self {
        Walk { tris: Vec::with_capacity(t.min(limit / 2).min(1 << 20)), ..Walk::default() }
    }

    fn mark(&self, tri: usize) -> Mark {
        Mark { tri, pos: self.pos.len(), log: self.log.len(), slot: self.slot, ri: self.ri, pi: self.pi }
    }

    fn rewind(&mut self, m: Mark) {
        for k in self.log.drain(m.log..) {
            if let Some(n) = self.edges.get_mut(&k) {
                *n = n.saturating_sub(1);
            }
        }
        self.pos.truncate(m.pos);
        self.tris.truncate(m.tri);
        self.stack.clear();
        self.next = None;
        (self.slot, self.ri, self.pi) = (m.slot, m.ri, m.pi);
    }

    /// One slot: `Some(v)` a reference to vertex `v`, `None` a new point.
    fn take(&mut self, a: &Arrays<'_>) -> Result<Option<u32>, &'static str> {
        let flagged = *a.is_reference.get(self.slot).ok_or("slots run out")?;
        self.slot = self.slot.saturating_add(1);
        if !flagged {
            return Ok(None);
        }
        let v = *a.references.get(self.ri).ok_or("references run out")?;
        self.ri = self.ri.saturating_add(1);
        self.get(v)?;
        Ok(Some(v))
    }

    fn point(&mut self, a: &Arrays<'_>) -> Result<V, &'static str> {
        let Some(&[x, y, z]) = a.points.get(self.pi..self.pi.saturating_add(3)) else {
            return Err("points run out");
        };
        self.pi = self.pi.saturating_add(3);
        let tol = a.tolerance;
        Ok([x as f64 * tol, y as f64 * tol, z as f64 * tol])
    }

    fn get(&self, v: u32) -> Result<V, &'static str> {
        self.pos.get(v as usize).copied().ok_or("a triangle refers to a vertex not yet decoded")
    }

    fn push(&mut self, p: V) -> Result<u32, &'static str> {
        let id = u32::try_from(self.pos.len()).map_err(|_| "too many vertices")?;
        self.pos.push(p);
        Ok(id)
    }

    /// The next triangle and its default fold: the seed, or the apex across the edge to
    /// continue from.
    fn triangle(&mut self, a: &Arrays<'_>) -> Result<([u32; 3], bool), &'static str> {
        if let Some((p, q, w)) = self.next.take() {
            if let Some(r) = self.take(a)? {
                return Ok(([p, q, r], false));
            }
            let (pp, qq, ww) = (self.get(p)?, self.get(q)?, self.get(w)?);
            let (lo, hi) = if p < q { (pp, qq) } else { (qq, pp) };
            let fold = matches!(a.points.get(self.pi..self.pi.saturating_add(3)), Some(&[_, dy, 0]) if dy > 0);
            let d = self.point(a)?;
            let r = self.push(apex(lo, hi, ww, d))?;
            return Ok(([p, q, r], fold));
        }
        let mut v = [0u32; 3];
        for k in 0..3 {
            let id = match self.take(a)? {
                Some(r) => r,
                None => {
                    let d = self.point(a)?;
                    let p = match k {
                        0 => add(a.origin, d),
                        1 => add(self.get(v[0])?, d),
                        _ => add(scale(add(self.get(v[0])?, self.get(v[1])?), 0.5), d),
                    };
                    self.push(p)?
                }
            };
            if let Some(s) = v.get_mut(k) {
                *s = id;
            }
        }
        Ok((v, false))
    }

    /// Budget elements (8 bytes each) the walk's arrays hold, by length: growth can add up to
    /// as much again in spare capacity.
    fn held(&self) -> usize {
        let pos = self.pos.len().saturating_mul(3);
        let tris = self.tris.len().saturating_mul(2);
        let edges = self.edges.len().saturating_mul(2);
        let stack = self.stack.len().saturating_mul(2);
        pos.saturating_add(tris).saturating_add(edges).saturating_add(self.log.len()).saturating_add(stack)
    }

    fn closed(&self, e: (u32, u32, u32)) -> bool {
        self.edges.get(&key(e.0, e.1)).copied().unwrap_or(0) >= 2
    }

    /// Adds one triangle; `flip` inverts its fold.
    fn step(&mut self, a: &Arrays<'_>, status: i32, flip: bool) -> Result<(), &'static str> {
        let ([ta, tb, tc], fold) = self.triangle(a)?;
        for (x, y) in [(ta, tb), (tb, tc), (tc, ta)] {
            let n = self.edges.entry(key(x, y)).or_insert(0);
            *n = n.saturating_add(1);
            let shared = *n;
            self.log.push(key(x, y));
            if shared > 2 {
                return Err("an edge is shared by more than two triangles");
            }
        }
        self.tris.push([ta, tb, tc]);
        if self.held() > a.limit {
            return Err("the rebuild needs more than the decoded-element allowance left");
        }
        let (mut left, mut right) = ((tc, tb, ta), (ta, tc, tb));
        if fold != flip {
            std::mem::swap(&mut left, &mut right);
        }
        let (taken, other) = if status & 2 != 0 { (left, right) } else { (right, left) };
        if status & 3 != 0 && self.closed(taken) && !self.closed(other) {
            std::mem::swap(&mut left, &mut right);
        }
        match (status & 2 != 0, status & 1 != 0) {
            (true, true) => {
                self.next = Some(left);
                self.stack.push(right);
            }
            (true, false) => self.next = Some(left),
            (false, true) => self.next = Some(right),
            (false, false) => {}
        }
        while self.next.is_none() {
            let Some(e) = self.stack.pop() else { break };
            if !self.closed(e) {
                self.next = Some(e);
            }
        }
        Ok(())
    }

    /// Walks every component. One that fails is rewound and walked once more with the fold
    /// inverted at its second triangle, so the walk is at most twice the triangle count.
    fn walk(&mut self, a: &Arrays<'_>) -> Result<(), &'static str> {
        let mut start = self.mark(0);
        let mut flip_at = None;
        let mut i = 0;
        while i < a.triangles {
            if self.next.is_none() && !(flip_at.is_some() && i == start.tri) {
                start = self.mark(i);
                flip_at = None;
            }
            let status = a.edge_status.get(i).copied().unwrap_or(0);
            match self.step(a, status, flip_at == Some(i)) {
                Ok(()) => i = i.saturating_add(1),
                Err(e) => {
                    if flip_at.is_some() || i == start.tri {
                        return Err(e);
                    }
                    self.rewind(start);
                    flip_at = Some(start.tri.saturating_add(1));
                    i = start.tri;
                }
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests;
