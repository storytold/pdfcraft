//! The occurrence walk that turns the trees into placements [WD 7.3.10.1, 7.6.3.2].

use super::overrides::{Active, Reached};
use super::{
    EntityOverrides, Globals, Graphics, MAX_DEPTH, MAX_PLACEMENTS, MAX_VISITS, Matrix, ModelNode, NameSource, Override, Palettes, Placement, Product,
    StyleAlpha, Target, Tree, UniqueId, paint_of, resolve_style, style,
};
use crate::PrcError;
use crate::container::offset;
use crate::entity::malformed;

/// Ceiling on face-override entries all placements of one walk carry together.
const MAX_FACE_OVERRIDES: usize = 1 << 24;

/// Where an occurrence sits: its depth, its parent node and whether its ancestors are drawn.
#[derive(Debug, Clone, Copy)]
pub(super) struct At {
    depth: usize,
    parent: Option<usize>,
    drawn: bool,
}

impl At {
    /// A root occurrence.
    pub(super) const ROOT: At = At { depth: 0, parent: None, drawn: true };
}

/// An occurrence's part and sons as `(file structure, value)`, after the prototype fallback.
struct Resolved<'t> {
    part: (usize, u32),
    sons: (usize, &'t [u32]),
    proto_name: Option<&'t str>,
    /// The entity references in force: the occurrence's own, else its prototype chain's
    /// first.
    overrides: (usize, &'t [Override]),
}

/// The occurrence walk over every file structure's tree.
pub(super) struct Walk<'t> {
    /// Per file structure: its id, tree and globals.
    trees: Vec<(UniqueId, &'t Tree, &'t Globals)>,
    pub(super) out: Vec<Placement>,
    pub(super) nodes: Vec<ModelNode>,
    visits: usize,
    face_overrides: usize,
    palettes: Palettes,
    scope: EntityOverrides,
    /// The entity references of the occurrences being walked, outermost first; under
    /// [`EntityOverrides::Everywhere`], every occurrence's.
    active: Vec<Active<'t>>,
    /// The `(file structure, occurrence)` of each occurrence being walked, root first: an
    /// occurrence met again on its own path is a cycle.
    path: Vec<(usize, usize)>,
}

impl<'t> Walk<'t> {
    /// A walk over `trees`, with no placements yet, colouring by `rule` and applying entity
    /// references by `scope`.
    pub(super) fn new(trees: Vec<(UniqueId, &'t Tree, &'t Globals)>, rule: StyleAlpha, scope: EntityOverrides) -> Self {
        let palettes = trees
            .iter()
            .map(|(_, _, g)| (0..=g.styles.len()).map(|b| u32::try_from(b).ok().and_then(|b| g.style_paint(b, rule))).collect())
            .collect();
        let mut walk =
            Walk { trees, out: Vec::new(), nodes: Vec::new(), visits: 0, face_overrides: 0, palettes, scope, active: Vec::new(), path: Vec::new() };
        if scope == EntityOverrides::Everywhere {
            let trees = walk.trees.clone();
            for (fs, t) in trees.iter().enumerate() {
                for p in &t.1.products {
                    walk.activate(fs, &p.overrides);
                }
            }
        }
        walk
    }

    /// Puts `overrides`, held by an occurrence of structure `owner`, in force. One naming an
    /// unknown structure is left out.
    fn activate(&mut self, owner: usize, overrides: &'t [Override]) {
        for ov in overrides {
            let named = match (ov.target.fs(), &ov.target) {
                (Some(id), _) => match self.fs(id) {
                    Ok(n) => Some(n),
                    Err(_) => continue,
                },
                (None, Target::Entity { .. }) => Some(owner),
                (None, Target::Topology { .. }) => None,
            };
            self.active.push((owner, named, ov));
        }
    }

    fn fs(&self, id: UniqueId) -> Result<usize, PrcError> {
        self.trees.iter().position(|t| t.0 == id).ok_or_else(|| malformed("reference to an unknown file structure".into()))
    }

    fn product(&self, fs: usize, index: usize) -> Result<&'t Product, PrcError> {
        self.trees.get(fs).and_then(|t| t.1.products.get(index)).ok_or_else(|| malformed(format!("product occurrence {index} does not exist")))
    }

    /// The structure a file identifier names: `id`, or `fs` itself when it names none.
    fn named(&self, id: Option<UniqueId>, fs: usize) -> Result<usize, PrcError> {
        id.map_or(Ok(fs), |id| self.fs(id))
    }

    /// Walks occurrence `index` of structure `fs` under a father placed at `father`, whose
    /// occurrences' graphics are `graphics` (root first), and lists it as a node. A hidden or
    /// suppressed subtree is listed with `drawn` false and placed nowhere.
    pub(super) fn occurrence(&mut self, fs: usize, index: usize, father: &Matrix, graphics: &[Graphics], at: At) -> Result<(), PrcError> {
        self.visits = self.visits.saturating_add(1);
        if at.depth > MAX_DEPTH || self.visits > MAX_VISITS {
            return Err(malformed("the occurrence tree is too deep or too large".into()));
        }
        if self.path.contains(&(fs, index)) {
            return Err(malformed(format!("product occurrence {index} contains itself")));
        }
        let p = self.product(fs, index)?;
        let m = p.location.map_or(*father, |l| father.multiply(&l));
        let mut chain = graphics.to_vec();
        chain.push(Graphics { fs, ..p.graphics });
        let r = self.resolve(fs, index, p)?;
        let drawn = at.drawn && !p.hidden && !p.suppressed;
        let in_force = self.active.len();
        if self.scope == EntityOverrides::Subtree {
            self.activate(r.overrides.0, r.overrides.1);
        }
        let node = self.nodes.len();
        let first = self.out.len();
        let (name, name_from) = self.name_of(p, &r);
        self.nodes.push(ModelNode {
            name,
            name_from,
            parent: at.parent,
            depth: at.depth,
            file_structure: fs,
            occurrence: index,
            hidden: p.hidden,
            suppressed: p.suppressed,
            drawn,
            has_part: r.part.1 != 0,
            placements: first..first,
        });
        if let (true, Some(part)) = (drawn, r.part.1.checked_sub(1)) {
            self.part(r.part.0, offset(part), &m, &chain)?;
        }
        // Son indices are 0-based; the external one is biased.
        let external = match p.external.checked_sub(1) {
            Some(e) => Some((self.named(p.external_fs, fs)?, e)),
            None => None,
        };
        let child = At { depth: at.depth + 1, parent: Some(node), drawn };
        self.path.push((fs, index));
        let walked =
            r.sons.1.iter().map(|&s| (r.sons.0, s)).chain(external).try_for_each(|(sfs, s)| self.occurrence(sfs, offset(s), &m, &chain, child));
        self.path.pop();
        walked?;
        self.active.truncate(in_force);
        let end = self.out.len();
        if let Some(n) = self.nodes.get_mut(node) {
            n.placements = first..end;
        }
        Ok(())
    }

    /// The part and sons of `p` (occurrence `index` of `fs`), falling back to its prototype
    /// chain's for whichever it leaves empty [WD 7.3.10.1], and the first name on that chain.
    fn resolve(&self, fs: usize, index: usize, p: &'t Product) -> Result<Resolved<'t>, PrcError> {
        let (mut part, mut sons) = ((fs, p.part), (fs, p.sons.as_slice()));
        let mut proto_name = None;
        let mut overrides = (fs, p.overrides.as_slice());
        let mut proto = (fs, p.prototype, p.prototype_fs);
        let mut seen = vec![(fs, index)];
        while proto.1 != 0 && (part.1 == 0 || sons.1.is_empty() || proto_name.is_none() || overrides.1.is_empty()) {
            let pfs = self.named(proto.2, proto.0)?;
            let pi = offset(proto.1.saturating_sub(1));
            if seen.contains(&(pfs, pi)) {
                return Err(malformed(format!("product occurrence {pi} is its own prototype")));
            }
            if seen.len() > MAX_DEPTH {
                return Err(malformed("prototype chain too long".into()));
            }
            seen.push((pfs, pi));
            let q = self.product(pfs, pi)?;
            if part.1 == 0 {
                part = (pfs, q.part);
            }
            if sons.1.is_empty() {
                sons = (pfs, q.sons.as_slice());
            }
            if proto_name.is_none() {
                proto_name = q.name.as_deref();
            }
            if overrides.1.is_empty() {
                overrides = (pfs, q.overrides.as_slice());
            }
            proto = (pfs, q.prototype, q.prototype_fs);
        }
        Ok(Resolved { part, sons, proto_name, overrides })
    }

    /// The display name: the occurrence's, its prototype chain's, then its part's.
    fn name_of(&self, p: &Product, r: &Resolved<'_>) -> (Option<String>, NameSource) {
        if let Some(n) = &p.name {
            return (Some(n.clone()), NameSource::Occurrence);
        }
        if let Some(n) = r.proto_name {
            return (Some(n.to_owned()), NameSource::Prototype);
        }
        let part = r.part.1.checked_sub(1).and_then(|i| self.trees.get(r.part.0)?.1.part_names.get(offset(i))).and_then(Option::as_ref);
        match part {
            Some(n) => (Some(n.clone()), NameSource::Part),
            None => (None, NameSource::Unnamed),
        }
    }

    /// Places part definition `index` of structure `fs` at `m`, under `graphics`.
    fn part(&mut self, fs: usize, index: usize, m: &Matrix, graphics: &[Graphics]) -> Result<(), PrcError> {
        let (_, tree, _) = self.trees.get(fs).copied().ok_or_else(|| malformed("file structure out of range".into()))?;
        let items = tree.parts.get(index).ok_or_else(|| malformed(format!("part definition {index} does not exist")))?;
        for item in items {
            let reached = Reached::collect(&self.active, fs, item);
            if reached.hidden {
                continue;
            }
            if self.out.len() >= MAX_PLACEMENTS {
                return Err(malformed(format!("more than {MAX_PLACEMENTS} placements")));
            }
            self.face_overrides = self.face_overrides.saturating_add(reached.faces.len());
            if self.face_overrides > MAX_FACE_OVERRIDES {
                return Err(malformed("too many face overrides".into()));
            }
            let matrix = match reached.local {
                Some((owner, cs)) => self.located(m, owner, &[cs])?,
                None => self.located(m, fs, &item.local)?,
            };
            let chain: Vec<Graphics> = graphics.iter().copied().chain(item.graphics.iter().map(|&g| Graphics { fs, ..g })).collect();
            let won = reached.item.unwrap_or_else(|| resolve_style(&chain));
            self.out.push(Placement {
                file_structure: fs,
                tessellation: offset(item.tessellation.saturating_sub(1)),
                matrix,
                colour: paint_of(&self.palettes, won).map(|p| p.rgba),
                palettes: self.palettes.clone(),
                chain,
                item_override: reached.item,
                face_overrides: reached.faces,
            });
        }
        Ok(())
    }

    /// `m` × each of structure `fs`'s coordinate systems `local` (biased).
    fn located(&self, m: &Matrix, fs: usize, local: &[u32]) -> Result<Matrix, PrcError> {
        let globals = self.trees.get(fs).ok_or_else(|| malformed("file structure out of range".into()))?.2;
        let mut placed = *m;
        for &cs in local {
            let l = style::biased(&globals.systems, cs).ok_or_else(|| malformed(format!("coordinate system {cs} does not exist")))?;
            placed = placed.multiply(l);
        }
        Ok(placed)
    }
}
