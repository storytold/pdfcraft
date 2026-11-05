//! The tessellation section (305): one `TESS_3D` (172) per primitive, each
//! a single `TESS_Face` (174) of plain triangles [WD 7.3.7, 7.8.5, 7.8.9].

use super::validate::Plan;
use super::vec3;
use super::{BitWriter, MeshScene, PrcWriteError, ScenePrimitive};

const TESSELLATION: u32 = 305;
const TESS_3D: u32 = 172;
const TESS_FACE: u32 = 174;
/// `PRC_FACETESSDATA_Triangle` [WD 7.8.9.2].
const TRIANGLE: u32 = 0x2;
/// `PRC_FACETESSDATA_TriangleTextured`: each corner's normal, texture and
/// point indices [WD 7.8.9.2].
const TRIANGLE_TEXTURED: u32 = 0x0200;

/// The written section and its counts.
pub(super) struct Section {
    pub(super) bytes: Vec<u8>,
    pub(super) triangles: usize,
    pub(super) flat_normals: usize,
    pub(super) textured: usize,
}

/// Every planned primitive's mesh, in plan order.
pub(super) fn section(scene: &MeshScene, plan: &Plan) -> Result<Section, PrcWriteError> {
    let mut w = BitWriter::default();
    w.uint(TESSELLATION).uint(0).bit(true);
    w.uint(plan.tessellations.len() as u32);
    let (mut triangles, mut flat_normals, mut textured) = (0, 0, 0);
    for &(m, p) in &plan.tessellations {
        let Some(prim) = scene.meshes.get(m).and_then(|m| m.primitives.get(p)) else {
            continue;
        };
        triangles += prim.triangles.len();
        flat_normals += usize::from(prim.normals.is_empty());
        let uvs = plan.is_textured(scene, prim);
        textured += usize::from(uvs);
        tess_3d(&mut w, prim, uvs);
    }
    w.uint(0);
    let bytes = w.finish().ok_or(PrcWriteError::BadNumber("position or normal"))?;
    Ok(Section { bytes, triangles, flat_normals, textured })
}

/// The unit normal of triangle `t`, or +z for a degenerate one.
fn face_normal(p: &ScenePrimitive, t: [u32; 3]) -> [f64; 3] {
    let at = |i: u32| p.positions.get(i as usize).copied().unwrap_or([0.0; 3]);
    let (a, b, c) = (at(t[0]), at(t[1]), at(t[2]));
    let n = vec3::cross(vec3::sub(b, a), vec3::sub(c, a));
    vec3::normalize(n).unwrap_or([0.0, 0.0, 1.0])
}

/// `p` as a `TESS_3D`; with `uvs`, its texture coordinates too, a corner's
/// pair at twice its vertex index.
fn tess_3d(w: &mut BitWriter, p: &ScenePrimitive, uvs: bool) {
    // Not calculated; the coordinates.
    w.uint(TESS_3D).bit(false);
    w.uint(3 * p.positions.len() as u32);
    w.doubles(p.positions.iter().flatten().copied());
    // Has faces; no loops (8137 >= 7039); normals stored, not to be
    // recalculated (8137 >= 7047).
    w.bit(true).bit(false).bit(false);
    let flat = p.normals.is_empty();
    if flat {
        w.uint(3 * p.triangles.len() as u32);
        for &t in &p.triangles {
            w.doubles(face_normal(p, t));
        }
    } else {
        w.uint(3 * p.normals.len() as u32);
        w.doubles(p.normals.iter().flatten().copied());
    }
    // No wire; the (normal, [texture,] point) offsets of every corner.
    let per_corner = if uvs { 3 } else { 2 };
    w.uint(0).uint(3 * per_corner * p.triangles.len() as u32);
    for (k, t) in p.triangles.iter().enumerate() {
        for v in t {
            let n = if flat { k as u32 } else { *v };
            w.uint(3 * n);
            if uvs {
                w.uint(2 * v);
            }
            w.uint(3 * v);
        }
    }
    // One face: no line attributes or wires, triangles from index 0, one
    // texture-coordinate set or none, no vertex colours.
    w.uint(1).uint(TESS_FACE).uint(0).uint(0).uint(0);
    w.uint(if uvs { TRIANGLE_TEXTURED } else { TRIANGLE }).uint(0).uint(1).uint(p.triangles.len() as u32);
    w.uint(u32::from(uvs)).bit(false);
    if uvs {
        w.uint(2 * p.uvs.len() as u32);
        w.doubles(p.uvs.iter().flatten().copied());
    } else {
        w.uint(0);
    }
}
