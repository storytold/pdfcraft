//! A `TESS_Face`'s triangulated data turned into triangles [WD 7.8.5.5, 7.8.6].

use super::TriangleStyle;
use crate::PrcError;
use crate::entity::malformed;

/// A fan/strip count's point-count bits; bit 30 is `NORMAL_Single` [WD 7.8.6.2].
const COUNT_MASK: u32 = 0x3FFF_FFFF;
const NORMAL_SINGLE: u32 = 0x4000_0000;

/// Texture-coordinate sets a face may carry. Each set costs a slot per point and a corner
/// triple per triangle of the whole mesh, so the count is capped.
pub(super) const MAX_TEXTURE_SETS: usize = 8;

pub(super) struct Face {
    pub(super) flags: u32,
    pub(super) start: usize,
    pub(super) data: Vec<u32>,
    pub(super) textures: usize,
    /// `line_attributes`, each a style index + 1: none = the owner's graphics, one = the
    /// whole face's, more = one per triangulation entity [WD 7.8.6].
    pub(super) styles: Vec<u32>,
    /// The behaviour bits read when `styles` is not empty.
    pub(super) behaviour: u16,
}

/// A `used_entities_flag` block's shape [WD 7.8.5.5].
#[derive(Clone, Copy)]
enum Shape {
    Triangles,
    Fan,
    Strip,
}

/// One `used_entities_flag` block.
#[derive(Clone, Copy)]
struct Block {
    bit: u32,
    shape: Shape,
    /// One normal per entity rather than per point.
    one_normal: bool,
    textured: bool,
}

const fn block(bit: u32, shape: Shape, one_normal: bool, textured: bool) -> Block {
    Block { bit, shape, one_normal, textured }
}

/// The blocks in stream order, low bit first [WD 7.8.5.5].
const BLOCKS: [Block; 12] = [
    block(0x0002, Shape::Triangles, false, false),
    block(0x0004, Shape::Fan, false, false),
    block(0x0008, Shape::Strip, false, false),
    block(0x0020, Shape::Triangles, true, false),
    block(0x0040, Shape::Fan, true, false),
    block(0x0080, Shape::Strip, true, false),
    block(0x0200, Shape::Triangles, false, true),
    block(0x0400, Shape::Fan, false, true),
    block(0x0800, Shape::Strip, false, true),
    block(0x2000, Shape::Triangles, true, true),
    block(0x4000, Shape::Fan, true, true),
    block(0x8000, Shape::Strip, true, true),
];

/// One triangulation entity's index slots: per point its position, normal slot (when the
/// array stores normals) and texture slots.
#[derive(Default)]
struct Slots {
    verts: Vec<u32>,
    normals: Vec<u32>,
    /// Per texture set, each point's slot.
    textures: Vec<Vec<u32>>,
}

/// What a mesh's triangles carry besides positions, built in step with them: corner normal
/// slots and, per texture set, corner texture slots (`None` for a triangle whose face stores
/// fewer sets).
#[derive(Default)]
pub(super) struct Corners {
    pub(super) normals: Vec<[u32; 3]>,
    pub(super) textures: Vec<Vec<Option<[u32; 3]>>>,
}

impl Face {
    /// Entity sizes per set block, from `TriangulatedData` [WD 7.8.6]. A triangle block is
    /// one size word (its triangle count); fans and strips list a vertex count each.
    fn entities(&self) -> Result<Vec<(Block, Vec<u32>)>, PrcError> {
        let mut d = self.data.iter().copied();
        let mut next = || d.next().ok_or_else(|| malformed("triangulated data ends early".into()));
        let mut out = Vec::new();
        for b in BLOCKS {
            if self.flags & b.bit == 0 {
                continue;
            }
            let sizes = match b.shape {
                Shape::Triangles => vec![next()?],
                Shape::Fan | Shape::Strip => {
                    let n = next()? as usize;
                    if n > self.data.len() {
                        return Err(malformed("triangulated data ends early".into()));
                    }
                    (0..n).map(|_| next()).collect::<Result<_, _>>()?
                }
            };
            out.push((b, sizes));
        }
        Ok(out)
    }

    /// Points the face's index slots name: the vertex-colour count (pdf-issues #820).
    pub(super) fn point_count(&self) -> Result<usize, PrcError> {
        let mut n = 0usize;
        for (b, sizes) in self.entities()? {
            for s in sizes {
                let s = (s & COUNT_MASK) as usize;
                n = n.saturating_add(if matches!(b.shape, Shape::Triangles) { s.saturating_mul(3) } else { s });
            }
        }
        Ok(n)
    }

    /// Triangles [`Self::triangulate`] makes: one per triangle-block count, two fewer than the
    /// points of each fan and strip.
    pub(super) fn triangle_count(&self) -> Result<usize, PrcError> {
        let mut n = 0usize;
        for (b, sizes) in self.entities()? {
            for s in sizes {
                let s = (s & COUNT_MASK) as usize;
                n = n.saturating_add(if matches!(b.shape, Shape::Triangles) { s } else { s.saturating_sub(2) });
            }
        }
        Ok(n)
    }

    /// Emit the face's triangles into `out`; `normals` = the index array stores normal slots,
    /// in which case each triangle's corner normal slots go to `corners`, as do its corner
    /// texture slots. Returns the triangles each triangulation entity emitted, in order: one
    /// entity per triangle of a triangle block, per fan, per strip [WD 7.8.6].
    pub(super) fn triangulate(
        &self,
        indices: &[u32],
        normals: bool,
        n_points: usize,
        out: &mut Vec<[u32; 3]>,
        corners: &mut Corners,
    ) -> Result<Vec<usize>, PrcError> {
        if self.textures > MAX_TEXTURE_SETS {
            return Err(malformed(format!("{} texture-coordinate sets on a face", self.textures)));
        }
        let mut entities = Vec::new();
        let mut slots = indices.get(self.start..).unwrap_or(&[]).iter().copied();
        for (b, sizes) in self.entities()? {
            let t = if b.textured { self.textures } else { 0 };
            for word in sizes {
                let count = (word & COUNT_MASK) as usize;
                // Points in this entity, and whether each carries its own normal slot.
                let (points, per_point_normal) = match b.shape {
                    Shape::Triangles => (count.saturating_mul(3), !b.one_normal),
                    Shape::Fan | Shape::Strip => (count, !b.one_normal || word & NORMAL_SINGLE == 0),
                };
                if points.saturating_mul(t.saturating_add(1)) > indices.len() {
                    return Err(malformed("face runs past the triangulated index array".into()));
                }
                let s = read_slots(&mut slots, b.shape, points, t, normals.then_some(per_point_normal), n_points)?;
                let made = emit(b.shape, &s, out, corners);
                match b.shape {
                    Shape::Triangles => entities.extend(std::iter::repeat_n(1, made)),
                    Shape::Fan | Shape::Strip => entities.push(made),
                }
            }
        }
        Ok(entities)
    }

    /// Append one [`TriangleStyle`] per triangle of the face, whose entities emitted
    /// `entities[i]` triangles each. Entities past the face's styles inherit (style 0).
    pub(super) fn styles_into(&self, entities: &[usize], out: &mut Vec<TriangleStyle>) {
        let whole = match self.styles.as_slice() {
            [] => Some(0),
            [one] => Some(*one),
            _ => None,
        };
        for (i, &n) in entities.iter().enumerate() {
            let style = whole.unwrap_or_else(|| self.styles.get(i).copied().unwrap_or(0));
            out.extend(std::iter::repeat_n(TriangleStyle { style, behaviour: self.behaviour }, n));
        }
    }
}

/// One entity's `points` index slots. `normals` is `None` when the array stores no normal
/// slots, else whether every point has its own (rather than one per entity).
fn read_slots(
    slots: &mut impl Iterator<Item = u32>,
    shape: Shape,
    points: usize,
    texture_sets: usize,
    normals: Option<bool>,
    n_points: usize,
) -> Result<Slots, PrcError> {
    let mut take = || slots.next().ok_or_else(|| malformed("face runs past the triangulated index array".into()));
    // The caller bounded points * (texture_sets + 1) by the index array's length.
    let mut s = Slots { textures: vec![Vec::with_capacity(points); texture_sets], ..Slots::default() };
    let mut current = 0;
    for k in 0..points {
        let new_entity = match shape {
            Shape::Triangles => k % 3 == 0,
            Shape::Fan | Shape::Strip => k == 0,
        };
        if let Some(per_point) = normals {
            if per_point || new_entity {
                current = take()?;
            }
            s.normals.push(current);
        }
        for set in &mut s.textures {
            set.push(take()?);
        }
        let p = take()?;
        if !p.is_multiple_of(3) || p as usize / 3 >= n_points {
            return Err(malformed(format!("point index {p} is not a point")));
        }
        s.verts.push(p / 3);
    }
    Ok(s)
}

/// Append the triangles of one entity's `slots` to `out` and `corners`, keeping every
/// texture set as long as `out`. Returns how many it made.
fn emit(shape: Shape, slots: &Slots, out: &mut Vec<[u32; 3]>, corners: &mut Corners) -> usize {
    let before = out.len();
    assemble(shape, &slots.verts, out);
    assemble(shape, &slots.normals, &mut corners.normals);
    let made = out.len().saturating_sub(before);
    while corners.textures.len() < slots.textures.len() {
        corners.textures.push(vec![None; before]);
    }
    for (k, set) in corners.textures.iter_mut().enumerate() {
        match slots.textures.get(k) {
            Some(own) => {
                let mut tris = Vec::with_capacity(made);
                assemble(shape, own, &mut tris);
                set.extend(tris.into_iter().map(Some));
            }
            None => set.extend(std::iter::repeat_n(None, made)),
        }
    }
    made
}

/// Append the triangles `shape` makes of `verts`.
fn assemble(shape: Shape, verts: &[u32], out: &mut Vec<[u32; 3]>) {
    match shape {
        Shape::Triangles => out.extend(verts.as_chunks::<3>().0.iter().copied()),
        Shape::Fan => {
            if let Some((&c, rim)) = verts.split_first() {
                out.extend(rim.windows(2).filter_map(|w| match w {
                    [a, b] => Some([c, *a, *b]),
                    _ => None,
                }));
            }
        }
        Shape::Strip => out.extend(verts.windows(3).enumerate().filter_map(|(k, w)| match w {
            [a, b, c] if k % 2 == 0 => Some([*a, *b, *c]),
            [a, b, c] => Some([*b, *a, *c]),
            _ => None,
        })),
    }
}
