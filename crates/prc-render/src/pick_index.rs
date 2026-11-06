//! A reusable picking index: a bounding-volume hierarchy over every triangle of a model, built
//! once, so repeated picks (hover, measuring, collision) visit only the boxes the ray crosses.

use crate::WorldMesh;
use crate::pick::{Hit, Ray, intersect};
use crate::raster::at;
use crate::render::{corners, invisible};

/// Triangles per leaf; more costs intersection tests, fewer costs nodes.
const LEAF_SIZE: usize = 4;

/// A triangle the renderer draws, with its corners resolved.
#[derive(Debug, Clone)]
struct Prim {
    mesh: usize,
    triangle: usize,
    corners: [[f64; 3]; 3],
}

#[derive(Debug, Clone, Copy)]
struct Box3 {
    lo: [f64; 3],
    hi: [f64; 3],
}

#[derive(Debug, Clone)]
enum Node {
    Leaf { bounds: Box3, start: usize, end: usize },
    Inner { bounds: Box3, left: usize, right: usize },
}

/// A picking index over a model's meshes: the same answers as [`crate::pick`], faster when one
/// model is picked many times. It copies the triangles' corners, so it does not borrow the
/// meshes; rebuild it when they change. It holds about 130 bytes per drawn triangle.
#[derive(Debug, Clone, Default)]
pub struct PickIndex {
    prims: Vec<Prim>,
    nodes: Vec<Node>,
}

impl PickIndex {
    /// The index over `meshes`; [`Hit::mesh`] indexes this slice. Triangles [`crate::pick`]
    /// skips are left out. Each split halves its range, so the tree is about log2(n) deep.
    #[must_use]
    pub fn new(meshes: &[WorldMesh]) -> Self {
        let mut prims = Vec::new();
        for (m, world) in meshes.iter().enumerate() {
            for (t, tri) in world.mesh.triangles.iter().enumerate() {
                if let Some(corners) = corners(&world.mesh, tri).filter(|_| !invisible(world, t)) {
                    prims.push(Prim { mesh: m, triangle: t, corners });
                }
            }
        }
        let mut index = PickIndex { prims, nodes: Vec::new() };
        if !index.prims.is_empty() {
            index.build(0, index.prims.len());
        }
        index
    }

    /// The nearest triangle `ray` meets at or beyond [`Ray::min_distance`], as [`crate::pick`]
    /// answers for the indexed meshes.
    #[must_use]
    pub fn pick(&self, ray: &Ray) -> Option<Hit> {
        self.pick_where(ray, f64::INFINITY, |_| true)
    }

    /// [`Self::pick`] limited to the meshes `include` accepts (by index in the slice given to
    /// [`Self::new`]) and to hits no farther than `max_distance` along the ray; infinity or NaN
    /// sets no limit.
    #[must_use]
    pub fn pick_where(&self, ray: &Ray, max_distance: f64, mut include: impl FnMut(usize) -> bool) -> Option<Hit> {
        let mut best: Option<Hit> = None;
        let mut limit = if max_distance.is_nan() { f64::INFINITY } else { max_distance };
        let mut stack = Vec::with_capacity(64);
        if !self.nodes.is_empty() {
            stack.push(0_usize);
        }
        while let Some(n) = stack.pop() {
            match self.nodes.get(n) {
                Some(&Node::Inner { bounds, left, right }) if crosses(&bounds, ray, limit) => {
                    stack.push(right);
                    stack.push(left);
                }
                Some(&Node::Leaf { bounds, start, end }) if crosses(&bounds, ray, limit) => {
                    for p in self.prims.get(start..end).unwrap_or_default() {
                        if !include(p.mesh) {
                            continue;
                        }
                        let Some(hit) = intersect(&p.corners, ray).filter(|h| h.distance <= limit) else { continue };
                        let hit = Hit { mesh: p.mesh, triangle: p.triangle, ..hit };
                        if best.is_none_or(|b| nearer(&hit, &b)) {
                            limit = hit.distance;
                            best = Some(hit);
                        }
                    }
                }
                _ => {}
            }
        }
        best
    }

    /// Builds the subtree over `prims[start..end]` and returns its node index: a median split on
    /// the centroids' longest axis.
    fn build(&mut self, start: usize, end: usize) -> usize {
        let range = self.prims.get(start..end).unwrap_or_default();
        let bounds = box_of(range.iter().flat_map(|p| p.corners));
        let slot = self.nodes.len();
        let count = end.saturating_sub(start);
        if count <= LEAF_SIZE {
            self.nodes.push(Node::Leaf { bounds, start, end });
            return slot;
        }
        let axis = longest_axis(&box_of(range.iter().map(|p| centroid(&p.corners))));
        let half = count / 2;
        if let Some(slice) = self.prims.get_mut(start..end) {
            slice.select_nth_unstable_by(half, |a, b| at(centroid(&a.corners), axis).total_cmp(&at(centroid(&b.corners), axis)));
        }
        self.nodes.push(Node::Leaf { bounds, start, end });
        let mid = start.saturating_add(half);
        let left = self.build(start, mid);
        let right = self.build(mid, end);
        if let Some(node) = self.nodes.get_mut(slot) {
            *node = Node::Inner { bounds, left, right };
        }
        slot
    }
}

/// `a` is nearer than `b`, a tie going to the lower (mesh, triangle) as [`crate::pick`] does.
fn nearer(a: &Hit, b: &Hit) -> bool {
    a.distance < b.distance || (a.distance == b.distance && (a.mesh, a.triangle) < (b.mesh, b.triangle))
}

/// Whether `ray` crosses `b` somewhere in [min_distance, limit], inclusive so a hit at exactly
/// `limit` can still win a tie.
fn crosses(b: &Box3, ray: &Ray, limit: f64) -> bool {
    let (mut t0, mut t1) = (ray.min_distance, limit);
    for axis in 0..3 {
        let (o, d) = (at(ray.origin, axis), at(ray.direction, axis));
        let (lo, hi) = (at(b.lo, axis), at(b.hi, axis));
        if d == 0.0 {
            if o < lo || o > hi {
                return false;
            }
            continue;
        }
        let (a, c) = ((lo - o) / d, (hi - o) / d);
        let (a, c) = if a > c { (c, a) } else { (a, c) };
        // The box and triangle tests round differently; the box must never reject a hit.
        let slack = 1e-9 * a.abs().max(c.abs()).max(1.0);
        t0 = t0.max(a - slack);
        t1 = t1.min(c + slack);
        if t0 > t1 {
            return false;
        }
    }
    true
}

fn box_of(points: impl Iterator<Item = [f64; 3]>) -> Box3 {
    let mut b = Box3 { lo: [f64::INFINITY; 3], hi: [f64::NEG_INFINITY; 3] };
    for p in points {
        for ((lo, hi), v) in b.lo.iter_mut().zip(b.hi.iter_mut()).zip(p) {
            *lo = lo.min(v);
            *hi = hi.max(v);
        }
    }
    b
}

/// The corners' mean, each divided first so a model near the float range stays finite.
fn centroid(&[a, b, c]: &[[f64; 3]; 3]) -> [f64; 3] {
    [0, 1, 2].map(|i| at(a, i) / 3.0 + at(b, i) / 3.0 + at(c, i) / 3.0)
}

fn longest_axis(b: &Box3) -> usize {
    let ext = |i| at(b.hi, i) - at(b.lo, i);
    let (x, y, z) = (ext(0), ext(1), ext(2));
    if x >= y && x >= z {
        0
    } else if y >= z {
        1
    } else {
        2
    }
}

#[cfg(test)]
mod tests;
