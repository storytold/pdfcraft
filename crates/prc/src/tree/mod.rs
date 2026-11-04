//! The product tree: the tree section (`FileStructureTree`, 304), the globals' styles and
//! coordinate systems (303) and the model file (301), walked into placed tessellations
//! [WD 7.3.3-7.3.11, 7.5, 7.6].
//!
//! Markups (PMI), views, scene lights and clipping planes are read past and not modelled.

// prc-rs: where the working draft leaves an entity's field order open, the order here is
// prc-rs's format description's; each such entity's reader carries its own marker.

use std::sync::Arc;

use crate::bits::{BitReader, Budget};
use crate::container::offset;
use crate::entity::{Ctx, malformed};
use crate::geometry::{Matrix, Vec3};
use crate::{PrcError, PrcFile, Schema, SectionKind, UniqueId};

mod markup;
mod node;
mod overrides;
mod read;
mod style;
mod walk;

pub use node::{ModelNode, NameSource};
pub use overrides::EntityOverrides;
use overrides::{Override, Target};
pub(crate) use style::Graphics;
use style::{Globals, Paint, resolve_style};
pub use style::{Rgba, StyleAlpha};
use walk::{At, Walk};

const BASE_WITH_GRAPHICS: u32 = 2;
const MODEL_FILE: u32 = 301;
const FILE_STRUCTURE: u32 = 302;
const FILE_STRUCTURE_GLOBALS: u32 = 303;
const FILE_STRUCTURE_TREE: u32 = 304;
const PRODUCT_OCCURRENCE: u32 = 310;
const PART_DEFINITION: u32 = 311;
const FILTER: u32 = 320;
const SURF_PLANE: u32 = 86;
const CARTESIAN_TRANSFORMATION: u32 = 202;
const ENTITY_REFERENCE: u32 = 203;
const MARKUP_LINKED_ITEM: u32 = 204;
const REFERENCE_ON_PRC_BASE: u32 = 205;
const REFERENCE_ON_TOPOLOGY: u32 = 206;
const GENERAL_TRANSFORMATION: u32 = 207;
const RI_BREP_MODEL: u32 = 232;
const RI_CURVE: u32 = 233;
const RI_DIRECTION: u32 = 234;
const RI_PLANE: u32 = 235;
const RI_POINT_SET: u32 = 236;
const RI_POLY_BREP_MODEL: u32 = 237;
const RI_POLY_WIRE: u32 = 238;
const RI_SET: u32 = 239;
const RI_COORDINATE_SYSTEM: u32 = 240;
const MKP_VIEW: u32 = 501;
const MKP_MARKUP: u32 = 502;
const MKP_LEADER: u32 = 503;
const ANNOTATION_ITEM: u32 = 504;
const ANNOTATION_SET: u32 = 505;
const ANNOTATION_REFERENCE: u32 = 506;
const STYLE: u32 = 701;
const MATERIAL: u32 = 702;
const PICTURE: u32 = 703;
const TEXTURE_APPLICATION: u32 = 711;
const TEXTURE_DEFINITION: u32 = 712;
const TEXTURE_TRANSFORMATION: u32 = 713;
const LINE_PATTERN: u32 = 721;
const AMBIENT_LIGHT: u32 = 731;
const POINT_LIGHT: u32 = 732;
const DIRECTIONAL_LIGHT: u32 = 733;
const SPOT_LIGHT: u32 = 734;
const SCENE_DISPLAY_PARAMETERS: u32 = 741;
const CAMERA: u32 = 742;
/// prc-rs: a scene's `is_absolute` exists from this authoring version.
const ABSOLUTE_SCENE_FROM: u32 = 8137;
/// prc-rs: a view's flags, linked items and filters exist from this authoring version.
const VIEW_FLAGS_FROM: u32 = 8137;
/// prc-rs: a directional light's intensity exists from this authoring version.
const LIGHT_INTENSITY_FROM: u32 = 8030;
/// `TextureDefinition` mapping type "operator" and function "blend", 0-based
/// (pdf-issues #485).
const TEXTURE_MAPPING_OPERATOR: i32 = 3;
const TEXTURE_FUNCTION_BLEND: i32 = 3;
/// Texture application mode: alpha test [WD 7.5.7].
const TEXTURE_ALPHA_TEST: u8 = 0x02;

/// Ceiling on `RI_Set` nesting, the occurrence walk's depth and a prototype chain's length.
const MAX_DEPTH: usize = 64;
/// Ceiling on occurrences visited in one walk: prototypes share subtrees, so a small file can
/// describe an exponential walk.
const MAX_VISITS: usize = 1 << 20;
/// Ceiling on placements one walk yields.
pub const MAX_PLACEMENTS: usize = 1 << 20;

/// `GraphicsContent` behaviour bits [WD 7.2.4.2].
const SHOW: u16 = 0x0001;
const SON_HERIT_COLOR: u16 = 0x0008;
const FATHER_HERIT_COLOR: u16 = 0x0010;
const REMOVED: u16 = 0x2000;
/// `product_behavior` SUPPRESSED [WD 7.3.10].
const SUPPRESSED: u8 = 0x01;

/// Per file structure, entry `b` is the paint of biased style `b`.
type Palettes = Arc<[Arc<[Option<Paint>]>]>;

/// A tessellation drawn at a place in the model: world = [`Self::matrix`] × the
/// tessellation's own coordinates.
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub struct Placement {
    /// Index into [`PrcFile::file_structures`].
    pub file_structure: usize,
    /// Index into that file structure's [`tessellations`](crate::FileStructure::tessellations),
    /// as the tree stores it: [`PrcFile::placements`] does not decode that section, so an
    /// index past its end is the caller's to refuse.
    pub tessellation: usize,
    /// The occurrence path's locations, then the representation item's coordinate systems.
    pub matrix: Matrix,
    /// The colour of the style the tree resolves for this item; `None` when no style
    /// reaches it or the style names no colour this reader resolves (a textured material
    /// without a base). Faces with their own style are coloured by
    /// [`Self::triangle_colours`].
    pub colour: Option<Rgba>,
    /// The graphics from the root to this item, outermost first.
    chain: Vec<Graphics>,
    palettes: Palettes,
    /// An occurrence's entity reference replacing the item's style.
    item_override: Option<Graphics>,
    /// Per face index, sorted, an entity reference replacing that face's style.
    face_overrides: Vec<(u32, Graphics)>,
}

/// The paint of the style `won` names, in its file structure's globals.
fn paint_of(palettes: &Palettes, won: Graphics) -> Option<Paint> {
    palettes.get(won.fs)?.get(offset(won.style)).copied().flatten()
}

impl Placement {
    /// Per triangle of `mesh` (this placement's tessellation), its colour as
    /// [`Self::colour`] describes, with each face's own style taking part in the inheritance
    /// [WD 7.2.4, 7.8.6]; `None` when no face of `mesh` carries a style or an override, so
    /// every triangle is [`Self::colour`].
    #[must_use]
    pub fn triangle_colours(&self, mesh: &crate::TriangleMesh) -> Option<Vec<Option<Rgba>>> {
        if mesh.triangle_styles.is_empty() && self.face_overrides.is_empty() {
            return None;
        }
        let faces = overrides::triangle_faces(mesh);
        let by_face = |k: usize| {
            let f = faces.get(k)?;
            let i = self.face_overrides.binary_search_by_key(f, |(i, _)| *i).ok()?;
            self.face_overrides.get(i).map(|(_, g)| *g)
        };
        let colour = |won| paint_of(&self.palettes, won).map(|p| p.rgba);
        if mesh.triangle_styles.is_empty() {
            let whole = self.item_override.unwrap_or_else(|| resolve_style(&self.chain));
            return Some((0..mesh.triangles.len()).map(|k| colour(by_face(k).unwrap_or(whole))).collect());
        }
        let mut chain = self.chain.clone();
        chain.push(Graphics::default());
        let looks = mesh.triangle_styles.iter().enumerate().map(|(k, s)| {
            if let Some(won) = by_face(k).or(self.item_override) {
                return colour(won);
            }
            if let Some(last) = chain.last_mut() {
                *last = Graphics { style: s.style, bits: s.behaviour, fs: self.file_structure };
            }
            colour(resolve_style(&chain))
        });
        Some(looks.collect())
    }
}

/// A representation item that can be drawn, flattened out of any `RI_Set`.
#[derive(Debug, Clone, PartialEq)]
struct Item {
    /// `index_local_coordinate_system + 1` of each enclosing set and the item, outermost
    /// first; 0 entries dropped.
    local: Vec<u32>,
    /// `index_tessellation + 1`; never 0 here.
    tessellation: u32,
    /// The part's graphics, each enclosing set's, then the item's own.
    graphics: Vec<Graphics>,
    /// The PRC unique id of each enclosing set, then the item's own.
    uids: Vec<u32>,
    /// A B-rep model's stored `(context, body)` ids [WD 7.6.2].
    brep: Option<(u32, u32)>,
}

/// A `ProductOccurrence` (310) as far as placing and naming.
#[derive(Debug, Clone, PartialEq, Default)]
struct Product {
    part: u32,
    prototype: u32,
    prototype_fs: Option<UniqueId>,
    external: u32,
    external_fs: Option<UniqueId>,
    sons: Vec<u32>,
    /// Its own graphics hide it (Show clear or Removed set).
    hidden: bool,
    suppressed: bool,
    graphics: Graphics,
    location: Option<Matrix>,
    name: Option<String>,
    /// Its entity references that name a target.
    overrides: Vec<Override>,
}

/// One file structure's tree section.
#[derive(Debug, Clone, PartialEq, Default)]
struct Tree {
    parts: Vec<Vec<Item>>,
    /// Each part definition's name, parallel to `parts`.
    part_names: Vec<Option<String>>,
    products: Vec<Product>,
}

/// The model file's roots: file structure id and biased occurrence index.
#[derive(Debug, Clone, PartialEq)]
struct ModelFile {
    roots: Vec<(UniqueId, u32)>,
}

impl PrcFile {
    /// Where each drawable tessellation sits in the model: the product-occurrence tree walked
    /// from the model file's roots, each occurrence's location composed onto its father's,
    /// and each representation item's local coordinate systems applied last [WD 7.3.10.1,
    /// 7.6.3.2], coloured with [`StyleAlpha::ZeroUnset`] and entity references applied by
    /// [`EntityOverrides::Subtree`]. Suppressed occurrences and hidden entities are left
    /// out; a tessellation several occurrences draw appears once per occurrence. Empty when
    /// the file stores no tree ([`Self::stores_tree`]).
    ///
    /// # Errors
    /// [`PrcError::Unsupported`] when an item or an entity reference names a reference
    /// coordinate system and the globals carry fonts or fill patterns; [`PrcError::Malformed`] for a tree index naming no entity,
    /// a cycle of occurrences or prototypes, or a tree past [`MAX_PLACEMENTS`] or the depth
    /// ceiling; read errors as [`crate::FileStructure::tessellations`].
    pub fn placements(&self) -> Result<Vec<Placement>, PrcError> {
        self.placements_with(StyleAlpha::default(), EntityOverrides::default())
    }

    /// [`Self::placements`], combining style transparency with material alpha by `rule` and
    /// applying occurrences' entity references by `scope`.
    ///
    /// # Errors
    /// As [`Self::placements`].
    pub fn placements_with(&self, rule: StyleAlpha, scope: EntityOverrides) -> Result<Vec<Placement>, PrcError> {
        Ok(self.walk(rule, scope)?.0)
    }

    /// The assembly tree as a model-tree panel lists it: every product occurrence the model
    /// file's roots reach, depth first, with its display name and stored visibility. Hidden
    /// and suppressed subtrees are listed (with `drawn` false) though [`Self::placements`]
    /// leaves them out; each node's `placements` range indexes that list, so attributes each
    /// placed mesh to its part. Empty when the file stores no tree.
    ///
    /// # Errors
    /// As [`Self::placements`].
    pub fn model_tree(&self) -> Result<Vec<ModelNode>, PrcError> {
        Ok(self.walk(StyleAlpha::default(), EntityOverrides::default())?.1)
    }

    /// Whether the file stores a tree: its model file holds an entity. ISO 14739-1 requires
    /// one, but a tessellation-only file still draws (unplaced), so its absence reads as an
    /// empty tree, not an error.
    #[must_use]
    pub fn stores_tree(&self) -> bool {
        self.model_file.iter().any(|&b| b != 0)
    }

    fn walk(&self, rule: StyleAlpha, scope: EntityOverrides) -> Result<(Vec<Placement>, Vec<ModelNode>), PrcError> {
        self.walk_within(rule, scope, Budget::FULL)
    }

    /// [`Self::walk`] with `budget` shared by all its readers.
    pub(crate) fn walk_within(&self, rule: StyleAlpha, scope: EntityOverrides, budget: Budget) -> Result<(Vec<Placement>, Vec<ModelNode>), PrcError> {
        if !self.stores_tree() {
            return Ok((Vec::new(), Vec::new()));
        }
        // One allowance for the whole walk, so many sections cannot each spend a full one.
        let mut left = budget;
        let mut trees = Vec::with_capacity(self.file_structures.len());
        for fs in &self.file_structures {
            // The globals open with the schema every reader of this structure uses: read it
            // once, charged to the walk, and go on from there for the globals themselves.
            let mut globals_reader = BitReader::with_budget(fs.section(SectionKind::Globals), left);
            let schema = Schema::read(&mut globals_reader)?;
            left = globals_reader.budget();
            let mut ctx = Ctx::new(BitReader::with_budget(fs.section(SectionKind::Tree), left), &schema, fs.authoring_version);
            let tree = ctx.file_structure_tree()?;
            left = ctx.r.budget();
            // The globals carry the coordinate systems and the colours; colours alone are not
            // worth refusing the placements over.
            let mut ctx = Ctx::new(globals_reader.with_remaining(left), &schema, fs.authoring_version);
            let globals = ctx.globals();
            left = ctx.r.budget();
            let globals = match globals {
                Ok(g) => g,
                Err(e) if places_by_system(&tree) => return Err(e),
                Err(_) => Globals::default(),
            };
            trees.push((fs.id, tree, globals));
        }
        let mut r = BitReader::with_budget(&self.model_file, left);
        let schema = Schema::read(&mut r)?;
        let model = Ctx::new(r, &schema, self.header.authoring_version).model_file()?;
        let mut walk = Walk::new(trees.iter().map(|(id, t, g)| (*id, t, g)).collect(), rule, scope);
        for (id, root) in model.roots {
            let fs = trees.iter().position(|t| t.0 == id).ok_or_else(|| malformed("a root names an unknown file structure".into()))?;
            if let Some(index) = root.checked_sub(1) {
                walk.occurrence(fs, offset(index), &Matrix::IDENTITY, &[], At::ROOT)?;
            }
        }
        Ok((walk.out, walk.nodes))
    }
}

/// Whether an item of `tree`, or an entity reference one of its occurrences holds, names a
/// coordinate system: then the globals that store them must have been read.
fn places_by_system(tree: &Tree) -> bool {
    tree.parts.iter().flatten().any(|i| !i.local.is_empty()) || tree.products.iter().flat_map(|p| &p.overrides).any(|o| o.local != 0)
}

#[cfg(test)]
mod markup_tests;
#[cfg(test)]
mod tests;
