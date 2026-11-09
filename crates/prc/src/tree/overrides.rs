//! Product-occurrence entity references: the colour, visibility and placement an occurrence
//! imposes on an entity it reaches [WD 7.3.10.1, 7.4.4].

use super::{Graphics, Item, RI_BREP_MODEL, RI_COORDINATE_SYSTEM, UniqueId};

/// `PRC_TYPE_TOPO_Face` [WD 6.2].
const TOPO_FACE: u32 = 149;

/// Which entities an occurrence's entity references reach. ISO 14739-1 says a reference
/// overrides the referenced entity's properties, and not where.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
#[non_exhaustive]
pub enum EntityOverrides {
    /// Only where the occurrence's own subtree places the target, so one instance of a
    /// shared part can be coloured alone.
    #[default]
    Subtree,
    /// Every placement of the target anywhere in the model.
    Everywhere,
    /// Read past them: every entity keeps its own and inherited graphics.
    Ignore,
}

/// One `MISC_EntityReference` held by a product occurrence.
#[derive(Debug, Clone, PartialEq)]
pub(super) struct Override {
    /// The reference's graphics; style 0 leaves the target's colour alone.
    pub(super) graphics: Graphics,
    /// The graphics hide the target (Show clear or Removed set).
    pub(super) hidden: bool,
    /// `index_local_coordinate_system + 1` replacing the target's; 0 none.
    pub(super) local: u32,
    pub(super) target: Target,
}

/// What an entity reference names.
#[derive(Debug, Clone, PartialEq)]
pub(super) enum Target {
    /// `ReferenceOnPRCBase`: the entity of type `kind` whose PRC unique id is `uid` in
    /// structure `fs` (`None`: the occurrence's own).
    Entity { kind: u32, fs: Option<UniqueId>, uid: u32 },
    /// `ReferenceOnTopology` into the B-rep stored as `(context, body)`: its topological
    /// items of type `kind` at `indices` (0-based). With `fs` `None` it matches that B-rep in
    /// any structure the subtree reaches: producers write "same structure" for an assembly's
    /// reference into a part's own structure.
    Topology { kind: u32, fs: Option<UniqueId>, brep: (u32, u32), indices: Vec<u32> },
}

impl Target {
    /// The structure the reference names, if it names one.
    pub(super) fn fs(&self) -> Option<UniqueId> {
        match self {
            Target::Entity { fs, .. } | Target::Topology { fs, .. } => *fs,
        }
    }
}

/// An entity reference in force: the structure of the occurrence holding it, the structure
/// it names (`None`: any), and the reference.
pub(super) type Active<'t> = (usize, Option<usize>, &'t Override);

/// The overrides that reach one representation item.
#[derive(Debug, Default)]
pub(super) struct Reached {
    /// Graphics replacing the item's resolved style, in its owner's structure.
    pub(super) item: Option<Graphics>,
    pub(super) hidden: bool,
    /// The owner's structure and its local coordinate system.
    pub(super) local: Option<(usize, u32)>,
    /// Per face index, sorted, graphics replacing that face's style.
    pub(super) faces: Vec<(u32, Graphics)>,
}

impl Reached {
    /// Applies `active` innermost first, so an outer occurrence's override wins over an
    /// inner one's, as an assembly's appearance beats its sub-assembly's.
    pub(super) fn collect(active: &[Active<'_>], fs: usize, item: &Item) -> Self {
        let mut r = Reached::default();
        // A map, not a list searched per index: one reference may name millions of faces.
        let mut faces = std::collections::BTreeMap::new();
        for &(owner, named, ov) in active.iter().rev() {
            let in_fs = named.is_none_or(|n| n == fs);
            let g = Graphics { fs: owner, ..ov.graphics };
            match &ov.target {
                Target::Entity { kind, uid, .. } => {
                    if !in_fs || !is_item(*kind) || !item.uids.contains(uid) {
                        continue;
                    }
                    if ov.graphics.style != 0 {
                        r.item = Some(g);
                    }
                    r.hidden = ov.hidden;
                    if ov.local != 0 {
                        r.local = Some((owner, ov.local));
                    }
                }
                Target::Topology { kind, brep, indices, .. } => {
                    if !in_fs || *kind != TOPO_FACE || item.brep != Some(*brep) || ov.graphics.style == 0 {
                        continue;
                    }
                    for &f in indices {
                        faces.insert(f, g);
                    }
                }
            }
        }
        r.faces = faces.into_iter().collect();
        r
    }
}

/// Representation-item types a `ReferenceOnPRCBase` can colour.
fn is_item(kind: u32) -> bool {
    (RI_BREP_MODEL..=RI_COORDINATE_SYSTEM).contains(&kind)
}

/// Per triangle of `mesh`, the index of the `TESS_Face` it was read from (`u32::MAX` for
/// none).
pub(super) fn triangle_faces(mesh: &crate::TriangleMesh) -> Vec<u32> {
    let mut out = vec![u32::MAX; mesh.triangles.len()];
    for (i, range) in mesh.faces.iter().enumerate() {
        let face = u32::try_from(i).unwrap_or(u32::MAX);
        for f in out.get_mut(range.clone()).into_iter().flatten() {
            *f = face;
        }
    }
    out
}
