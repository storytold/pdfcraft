//! Checking a [`MeshScene`] and laying it out as PRC entities: the
//! occurrence order, which meshes become parts, which primitives become
//! tessellations.

use super::{MAX_DEPTH, MeshScene, PrcWriteError, ScenePrimitive};

/// One `ProductOccurrence`: the node it is for (`None` for the synthetic
/// root over several roots) and its sons as occurrence indices.
pub(super) struct Occ {
    pub(super) node: Option<usize>,
    pub(super) sons: Vec<u32>,
}

/// The scene as PRC entities.
pub(super) struct Plan {
    /// Occurrences, sons before fathers, the root last.
    pub(super) occurrences: Vec<Occ>,
    /// Mesh index of each part.
    pub(super) parts: Vec<usize>,
    /// Part index of each mesh, when it is placed.
    pub(super) part_of_mesh: Vec<Option<u32>>,
    /// `(mesh, primitive)` of each tessellation, in section order.
    pub(super) tessellations: Vec<(usize, usize)>,
    pub(super) empty_primitives: usize,
    /// Each texture's picture format, width and height.
    pub(super) pictures: Vec<(u32, u32, u32)>,
    /// The textured materials, in material order; the `j`th has material
    /// style `materials.len() + j`.
    pub(super) textured: Vec<usize>,
    /// The untextured materials that glow, in material order; the `g`th
    /// has material style `materials.len() + textured.len() + g`.
    pub(super) glowing: Vec<usize>,
}

impl Plan {
    /// Whether `p` is drawn with its material's texture: the material has
    /// one and `p` has coordinates.
    pub(super) fn is_textured(&self, scene: &MeshScene, p: &ScenePrimitive) -> bool {
        !p.uvs.is_empty() && p.material.and_then(|m| scene.materials.get(m)).is_some_and(|m| m.texture.is_some())
    }

    /// The 0-based style `p` names: its material's textured style when
    /// [`Self::is_textured`], else its RGB style.
    pub(super) fn style_of(&self, scene: &MeshScene, p: &ScenePrimitive) -> Option<u32> {
        let m = p.material?;
        if self.is_textured(scene, p) {
            // `textured` and `glowing` are in material order.
            let j = self.textured.binary_search(&m).ok()?;
            return Some((scene.materials.len() + j) as u32);
        }
        if let Ok(g) = self.glowing.binary_search(&m) {
            return Some((scene.materials.len() + self.textured.len() + g) as u32);
        }
        Some(m as u32)
    }

    /// The last unique id the globals take: the line pattern, the styles,
    /// the texture definitions, two materials per textured material and
    /// one per glowing one.
    pub(super) fn global_ids(&self, scene: &MeshScene) -> u32 {
        let materials = 2 * self.textured.len() + self.glowing.len();
        (1 + self.styles(scene) + scene.textures.len() + materials) as u32
    }

    /// How many styles the globals write: one RGB style per material, then
    /// the textured and the glowing materials' material styles.
    pub(super) fn styles(&self, scene: &MeshScene) -> usize {
        scene.materials.len() + self.textured.len() + self.glowing.len()
    }
}

fn bad(what: &'static str, index: usize) -> PrcWriteError {
    PrcWriteError::BadIndex { what, index }
}

fn count(n: usize) -> Result<u32, PrcWriteError> {
    u32::try_from(n).map_err(|_| PrcWriteError::TooLarge)
}

/// Check `scene` and lay it out.
pub(super) fn plan(scene: &MeshScene) -> Result<Plan, PrcWriteError> {
    check_numbers(scene)?;
    check_forest(scene)?;
    // Style ids and double-scaled colour indices (a base and an emissive
    // colour per material) are u32s.
    count(scene.materials.len().saturating_mul(6).saturating_add(2))?;
    count(scene.textures.len().saturating_mul(3))?;
    // Every global id sum (`global_ids`, the first texture id) is under
    // four per material, three per texture and a constant.
    count(scene.materials.len().saturating_mul(4).saturating_add(scene.textures.len().saturating_mul(3)).saturating_add(8))?;
    let occurrences = occurrences(scene)?;
    let mut plan = Plan {
        occurrences,
        parts: Vec::new(),
        part_of_mesh: vec![None; scene.meshes.len()],
        tessellations: Vec::new(),
        empty_primitives: 0,
        pictures: pictures(scene)?,
        textured: textured(scene)?,
        glowing: scene.materials.iter().enumerate().filter(|(_, m)| m.texture.is_none() && super::textures::glows(m)).map(|(i, _)| i).collect(),
    };
    for o in &plan.occurrences {
        let Some(m) = o.node.and_then(|n| scene.nodes.get(n)?.mesh) else {
            continue;
        };
        let Some(slot) = plan.part_of_mesh.get_mut(m) else {
            return Err(bad("mesh", m));
        };
        if slot.is_none() {
            *slot = Some(count(plan.parts.len())?);
            plan.parts.push(m);
        }
    }
    for &m in &plan.parts {
        for (p, prim) in scene.meshes.get(m).into_iter().flat_map(|m| m.primitives.iter().enumerate()) {
            check_primitive(scene, prim)?;
            if prim.triangles.is_empty() {
                plan.empty_primitives += 1;
            } else {
                plan.tessellations.push((m, p));
            }
        }
    }
    if plan.tessellations.is_empty() {
        return Err(PrcWriteError::Empty);
    }
    count(plan.tessellations.len())?;
    Ok(plan)
}

/// Each texture's [`super::textures::picture_info`].
fn pictures(scene: &MeshScene) -> Result<Vec<(u32, u32, u32)>, PrcWriteError> {
    scene.textures.iter().enumerate().map(|(index, t)| super::textures::picture_info(&t.image).ok_or(PrcWriteError::BadTexture { index })).collect()
}

/// The materials with a texture, each naming one that exists.
fn textured(scene: &MeshScene) -> Result<Vec<usize>, PrcWriteError> {
    let mut out = Vec::new();
    for (m, mat) in scene.materials.iter().enumerate() {
        match mat.texture {
            Some(t) if t >= scene.textures.len() => return Err(bad("texture", t)),
            Some(_) => out.push(m),
            None => {}
        }
    }
    Ok(out)
}

fn check_primitive(scene: &MeshScene, p: &ScenePrimitive) -> Result<(), PrcWriteError> {
    if let Some(m) = p.material.filter(|&m| m >= scene.materials.len()) {
        return Err(bad("material", m));
    }
    if !p.normals.is_empty() && p.normals.len() != p.positions.len() {
        return Err(bad("normal count", p.normals.len()));
    }
    if !p.uvs.is_empty() && p.uvs.len() != p.positions.len() {
        return Err(bad("texture coordinate count", p.uvs.len()));
    }
    // Every count the tessellation writes is a u32, coordinates three to a
    // vertex and corner indices up to nine to a triangle.
    count(p.positions.len().saturating_mul(3))?;
    count(p.triangles.len().saturating_mul(9))?;
    let n = p.positions.len();
    match p.triangles.iter().flatten().find(|&&v| v as usize >= n) {
        Some(&v) => Err(bad("vertex", v as usize)),
        None => Ok(()),
    }
}

fn finite(vs: impl IntoIterator<Item = f64>, what: &'static str) -> Result<(), PrcWriteError> {
    match vs.into_iter().all(f64::is_finite) {
        true => Ok(()),
        false => Err(PrcWriteError::BadNumber(what)),
    }
}

fn check_numbers(scene: &MeshScene) -> Result<(), PrcWriteError> {
    if !(scene.units_in_mm.is_finite() && scene.units_in_mm > 0.0) {
        return Err(PrcWriteError::BadNumber("units_in_mm"));
    }
    for n in &scene.nodes {
        if let Some(m) = n.matrix {
            finite(m, "matrix")?;
            if [m[3], m[7], m[11], m[15]] != [0.0, 0.0, 0.0, 1.0] {
                return Err(PrcWriteError::BadNumber("matrix bottom row"));
            }
        }
    }
    finite(scene.materials.iter().flat_map(|m| m.rgba), "colour")?;
    for p in scene.meshes.iter().flat_map(|m| &m.primitives) {
        finite(p.positions.iter().flatten().copied(), "position")?;
        finite(p.normals.iter().flatten().copied(), "normal")?;
        finite(p.uvs.iter().flatten().copied(), "texture coordinate")?;
    }
    Ok(())
}

/// Every child and root names a node, no node has two parents, no root
/// has one or is listed twice, and no chain is deeper than [`MAX_DEPTH`].
/// With one parent at most each and distinct parentless roots, what the
/// roots reach has no cycle and holds each node once, so the occurrences
/// number no more than the nodes.
fn check_forest(scene: &MeshScene) -> Result<(), PrcWriteError> {
    let n = scene.nodes.len();
    let mut parent = vec![None; n];
    for (i, node) in scene.nodes.iter().enumerate() {
        for &c in &node.children {
            let slot = parent.get_mut(c).ok_or_else(|| bad("child node", c))?;
            if slot.replace(i).is_some() {
                return Err(PrcWriteError::NotATree("a node has two parents"));
            }
        }
    }
    let mut rooted = vec![false; n];
    for &r in &scene.roots {
        match (parent.get(r), rooted.get_mut(r)) {
            (Some(None), Some(seen)) if !*seen => *seen = true,
            (Some(None), Some(_)) => return Err(PrcWriteError::NotATree("a root is listed twice")),
            (Some(Some(_)), _) => return Err(PrcWriteError::NotATree("a root has a parent")),
            _ => return Err(bad("root node", r)),
        }
    }
    let mut depth = vec![0usize; n];
    let mut stack: Vec<usize> = scene.roots.clone();
    while let Some(i) = stack.pop() {
        let d = depth.get(i).copied().unwrap_or(0).saturating_add(1);
        if d > MAX_DEPTH {
            return Err(PrcWriteError::NotATree("nested too deeply"));
        }
        for &c in scene.nodes.get(i).map(|n| n.children.as_slice()).unwrap_or(&[]) {
            if let Some(slot) = depth.get_mut(c) {
                *slot = d;
            }
            stack.push(c);
        }
    }
    Ok(())
}

/// Occurrences with every son before its father and the root last
/// [WD 7.3.6: a son precedes the occurrence that uses it], under a
/// synthetic root when there is not exactly one.
fn occurrences(scene: &MeshScene) -> Result<Vec<Occ>, PrcWriteError> {
    let mut out = Vec::new();
    let synthetic = scene.roots.len() != 1;
    if synthetic {
        out.push(Occ { node: None, sons: Vec::new() });
    }
    // (node, parent occurrence)
    let mut stack: Vec<(usize, Option<usize>)> = scene.roots.iter().rev().map(|&r| (r, synthetic.then_some(0))).collect();
    while let Some((node, parent)) = stack.pop() {
        let me = out.len();
        if let Some(p) = parent.and_then(|p| out.get_mut(p)) {
            p.sons.push(count(me)?);
        }
        out.push(Occ { node: Some(node), sons: Vec::new() });
        let children = scene.nodes.get(node).map(|n| n.children.as_slice()).unwrap_or(&[]);
        stack.extend(children.iter().rev().map(|&c| (c, Some(me))));
    }
    let last = count(out.len())?.saturating_sub(1);
    // Reversed pre-order: each subtree precedes its root; sons keep their order.
    out.reverse();
    for o in &mut out {
        for s in &mut o.sons {
            *s = last.saturating_sub(*s);
        }
    }
    Ok(out)
}
