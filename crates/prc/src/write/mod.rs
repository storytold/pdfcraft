//! Writing a triangle-mesh scene as a PRC stream (Adobe PRC 8137, the
//! format 3D PDF embeds as `/Subtype /PRC`).
//!
//! The scene is a node hierarchy over shared meshes, the shape glTF and
//! most mesh formats carry. Each node becomes a `ProductOccurrence` named
//! for it (the names a viewer's model tree lists), each referenced mesh a
//! `PartDefinition`, each primitive a poly-B-rep item over one `TESS_3D`
//! styled by its material's RGB colour and transparency, or by a base-colour
//! texture over stored (u, v) coordinates.

mod bits;
mod bounds;
mod container;
mod sections;
mod tess;
mod textures;
mod validate;
mod vec3;

pub(crate) use textures::picture_info;

#[cfg(test)]
pub(crate) mod readback;
#[cfg(test)]
mod tests;

use crate::UniqueId;

pub(crate) use bits::BitWriter;

/// How a texture repeats outside the 0–1 coordinate range.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum TextureWrap {
    /// Tiled.
    #[default]
    Repeat,
    /// The edge texels extended.
    Clamp,
    /// Tiled, every other tile mirrored.
    MirroredRepeat,
}

/// A triangle-mesh scene to write as PRC.
///
/// `nodes` form a forest: every index in [`SceneNode::children`] and
/// [`Self::roots`] names a node, no node has two parents, and a root has
/// none. Nodes no root reaches are not written.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct MeshScene {
    /// Every node.
    pub nodes: Vec<SceneNode>,
    /// The top-level nodes. More than one is written under an unnamed
    /// root occurrence.
    pub roots: Vec<usize>,
    /// Meshes, shared by any number of nodes.
    pub meshes: Vec<SceneMesh>,
    /// Materials, named by [`ScenePrimitive::material`].
    pub materials: Vec<SceneMaterial>,
    /// Textures, named by [`SceneMaterial::texture`].
    pub textures: Vec<SceneTexture>,
    /// Millimetres per scene unit: 1000 for metres (glTF), 1 for
    /// millimetres. Positive and finite.
    pub units_in_mm: f64,
}

/// One node: a placement, an optional mesh, children.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct SceneNode {
    /// The name a model tree shows; `None` writes a null name.
    pub name: Option<String>,
    /// Placement relative to the parent, a column-major 4×4 matrix (glTF's
    /// order); `None` for identity. The bottom row must be `0 0 0 1`.
    pub matrix: Option<[f64; 16]>,
    /// Index into [`MeshScene::meshes`].
    pub mesh: Option<usize>,
    /// Indices into [`MeshScene::nodes`].
    pub children: Vec<usize>,
}

/// A mesh: one or more primitives, each with its own material.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct SceneMesh {
    /// The part name.
    pub name: Option<String>,
    /// Its primitives; one with no triangles is not written.
    pub primitives: Vec<ScenePrimitive>,
}

/// One indexed triangle list.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ScenePrimitive {
    /// Vertex positions, in scene units.
    pub positions: Vec<[f64; 3]>,
    /// Per-vertex normals, the same length as `positions`; empty to have
    /// the writer compute one flat normal per triangle (reported in
    /// [`PrcWriteReport::flat_normals`]).
    pub normals: Vec<[f64; 3]>,
    /// Counter-clockwise triangles, indexing `positions`.
    pub triangles: Vec<[u32; 3]>,
    /// Index into [`MeshScene::materials`]; `None` leaves the colour to
    /// the viewer.
    pub material: Option<usize>,
    /// Texture coordinates per vertex, the same length as `positions`, or
    /// empty. (0, 0) is the picture's bottom-left corner and (1, 1) its
    /// top-right. Written only when the material is textured; a textured
    /// material over a primitive with none draws its base colour.
    pub uvs: Vec<[f64; 2]>,
}

/// A material as PRC can display it: one colour, its opacity and the
/// light it gives off.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct SceneMaterial {
    /// The material's name (carried in the scene only; PRC styles are
    /// unnamed).
    pub name: Option<String>,
    /// Display (sRGB-encoded) red, green, blue and opacity, each 0–1;
    /// opacity 1 is opaque. Values outside 0–1 are clamped.
    pub rgba: [f64; 4],
    /// Index into [`MeshScene::textures`]: a picture multiplying `rgba`
    /// over the primitive's [`ScenePrimitive::uvs`].
    pub texture: Option<usize>,
    /// Display (sRGB-encoded) light the surface gives off, each 0–1
    /// (clamped); black for none. A non-black one is written as the
    /// material's emissive colour [WD 7.5.4], so its primitives are drawn
    /// through a material style; a textured material's primitive without
    /// coordinates is drawn in its RGB style and does not glow.
    pub emissive: [f64; 3],
}

/// A base-colour picture.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SceneTexture {
    /// A complete PNG or JPEG file, stored in the PRC as it is.
    pub image: Vec<u8>,
    /// Wrapping along u, then v.
    pub wrap: [TextureWrap; 2],
    /// Whether the picture's alpha multiplies the material's opacity;
    /// otherwise only its colour is used.
    pub alpha: bool,
}

/// A written PRC stream and what the writer had to supply itself.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PrcWriteReport {
    /// The PRC stream, ready for a 3D PDF's `/Subtype /PRC` stream.
    pub prc: Vec<u8>,
    /// Triangles written.
    pub triangles: usize,
    /// Primitives that had no normals, given flat per-triangle ones.
    pub flat_normals: usize,
    /// Primitives left out for holding no triangles.
    pub empty_primitives: usize,
    /// Primitives written with a texture and coordinates.
    pub textured: usize,
}

/// Why a scene could not be written.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum PrcWriteError {
    /// The scene draws nothing: no root reaches a triangle.
    #[error("the scene holds no triangles")]
    Empty,
    /// An index names nothing: `what` with the offending `index`.
    #[error("{what} index {index} is out of range")]
    BadIndex {
        /// What the index was for.
        what: &'static str,
        /// The value found.
        index: usize,
    },
    /// The nodes are not a forest (a node with two parents, a parented
    /// root, or a nesting deeper than [`MAX_DEPTH`]).
    #[error("the node hierarchy is not a tree: {0}")]
    NotATree(&'static str),
    /// A coordinate, normal, matrix entry, colour or the unit is NaN or
    /// infinite, or the unit is not positive.
    #[error("the scene holds a non-finite or invalid number ({0})")]
    BadNumber(&'static str),
    /// The scene's counts or the stream exceed what PRC can address
    /// (32-bit counts and offsets).
    #[error("the scene is too large for a PRC stream")]
    TooLarge,
    /// Texture `index` is not a PNG or JPEG file whose size can be read.
    #[error("texture {index} is not a readable PNG or JPEG file")]
    BadTexture {
        /// Index into [`MeshScene::textures`].
        index: usize,
    },
}

/// The deepest node nesting the writer accepts.
pub const MAX_DEPTH: usize = 1024;

/// Write `scene` as a single-file-structure PRC stream.
///
/// # Errors
///
/// [`PrcWriteError`] when the scene is not a valid forest over valid
/// indices, holds a non-finite number or an unreadable texture, or draws
/// no triangle.
///
/// # Example
///
/// ```
/// use pdfcraft_prc::{MeshScene, SceneMesh, SceneNode, ScenePrimitive, write_prc};
///
/// let tri = ScenePrimitive {
///     positions: vec![[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]],
///     triangles: vec![[0, 1, 2]],
///     ..ScenePrimitive::default()
/// };
/// let scene = MeshScene {
///     nodes: vec![SceneNode { name: Some("tri".into()), mesh: Some(0), ..SceneNode::default() }],
///     roots: vec![0],
///     meshes: vec![SceneMesh { name: None, primitives: vec![tri] }],
///     materials: vec![],
///     textures: vec![],
///     units_in_mm: 1000.0,
/// };
/// let report = write_prc(&scene)?;
/// assert_eq!(report.triangles, 1);
/// assert_eq!(report.flat_normals, 1);
/// let prc = pdfcraft_prc::PrcFile::parse(&report.prc).unwrap();
/// assert_eq!(prc.placements().unwrap().len(), 1);
/// # Ok::<(), pdfcraft_prc::PrcWriteError>(())
/// ```
pub fn write_prc(scene: &MeshScene) -> Result<PrcWriteReport, PrcWriteError> {
    let plan = validate::plan(scene)?;
    let ids = ids(scene);
    let tess = tess::section(scene, &plan)?;
    let globals = sections::globals(scene, &plan).finish();
    let tree = sections::tree(scene, &plan).finish();
    let geometry = sections::empty_geometry(306).finish();
    let extra = sections::empty_geometry(307).finish();
    let model = sections::model_file(scene, &plan, ids.structure).finish();
    let (Some(globals), Some(tree), Some(geometry), Some(extra), Some(model)) = (globals, tree, geometry, extra, model) else {
        return Err(PrcWriteError::BadNumber("matrix, colour or unit"));
    };
    let structure = container::Structure {
        id: ids.structure,
        sections: [&globals, &tree, &tess.bytes, &geometry, &extra],
        files: scene.textures.iter().map(|t| t.image.as_slice()).collect(),
    };
    let prc = container::container(&[structure], &model, &ids.container).ok_or(PrcWriteError::TooLarge)?;
    Ok(PrcWriteReport {
        prc,
        triangles: tess.triangles,
        flat_normals: tess.flat_normals,
        empty_primitives: plan.empty_primitives,
        textured: tess.textured,
    })
}

struct AllIds {
    structure: UniqueId,
    container: container::Ids,
}

/// FNV-1a over the scene's debug form: the same scene always gets the same
/// ids, so writing is deterministic, and different scenes differ.
fn ids(scene: &MeshScene) -> AllIds {
    struct Fnv(u64);
    impl std::fmt::Write for Fnv {
        fn write_str(&mut self, s: &str) -> std::fmt::Result {
            for b in s.bytes() {
                self.0 = (self.0 ^ u64::from(b)).wrapping_mul(0x0100_0000_01b3);
            }
            Ok(())
        }
    }
    let mut fnv = Fnv(0xcbf2_9ce4_8422_2325);
    // Formatting into a hasher cannot fail.
    std::fmt::Write::write_fmt(&mut fnv, format_args!("{scene:?}")).ok();
    let h = fnv.0;
    let (hi, lo) = ((h >> 32) as u32, h as u32);
    AllIds {
        structure: UniqueId([hi, lo, 0x7064_6663, 1]),
        container: container::Ids {
            file: UniqueId([hi, lo, 0x7064_6663, 0]),
            // "pdfc" "raft": who wrote the stream.
            application: UniqueId([0x6366_6470, 0x7466_6172, 0, 1]),
        },
    }
}
