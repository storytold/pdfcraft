//! Index-arena outline model, Euler tour intervals, and O(subtree) caravan
//! splicing for atomic batch bookmark move and delete (`crates/organize`).
//!
//! All traversals are iterative and cycle-guarded (`seen: HashSet<ObjRef>`)
//! with a hard [`MAX_OUTLINE_NODES`] budget so hostile or deeply nested outlines
//! never overflow the stack or loop indefinitely.

use std::collections::{HashMap, HashSet};

use pdfcraft_cos::{Document, ObjRef, Object};

use crate::outline::{OutlineError, outline_root, put};

/// Hard traversal budget for outline loading and batch operations.
pub const MAX_OUTLINE_NODES: usize = 200_000;

/// Stable index into [`OutlineArena::nodes`]. `ArenaNodeId::ROOT` (`0`) is the virtual `/Outlines` root.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub struct ArenaNodeId(pub u32);

impl ArenaNodeId {
    pub const ROOT: Self = Self(0);

    pub const fn index(self) -> usize {
        self.0 as usize
    }
}

/// Topological insertion target for caravan splicing inside [`OutlineArena`].
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum OutlinePlacement {
    BeforeSibling(ArenaNodeId),
    AfterSibling(ArenaNodeId),
    FirstChildOf(ArenaNodeId),
    LastChildOf(ArenaNodeId),
}

impl OutlinePlacement {
    pub const fn target_node(self) -> ArenaNodeId {
        match self {
            Self::BeforeSibling(id) | Self::AfterSibling(id) | Self::FirstChildOf(id) | Self::LastChildOf(id) => id,
        }
    }
}

/// Placement selector relative to a target outline path for batch moves.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum OutlinePlacementKind {
    Before,
    After,
    FirstChild,
    LastChild,
}

/// One node in [`OutlineArena`].
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct OutlineArenaNode {
    pub cos_obj: Option<ObjRef>,
    pub is_expanded: bool,
    pub parent: Option<ArenaNodeId>,
    pub prev_sibling: Option<ArenaNodeId>,
    pub next_sibling: Option<ArenaNodeId>,
    pub first_child: Option<ArenaNodeId>,
    pub last_child: Option<ArenaNodeId>,
    pub pdf_count: i32,
    pub euler_in: u32,
    pub euler_out: u32,
    pub depth: u32,
    pub dirty: bool,
    pub(crate) had_count: bool,
}

impl OutlineArenaNode {
    fn from_cos(cos_obj: Option<ObjRef>, pdf_count: i32, had_count: bool, is_expanded: bool) -> Self {
        Self { cos_obj, is_expanded, pdf_count, had_count, ..Self::default() }
    }
}

/// Flat index-arena representation of the document's `/Outlines` tree.
#[derive(Clone, Debug)]
pub struct OutlineArena {
    pub nodes: Vec<OutlineArenaNode>,
}

impl OutlineArena {
    /// Load the `/Outlines` tree from `doc` iteratively with cycle and node-budget guards.
    pub fn from_doc(doc: &Document) -> Result<Self, OutlineError> {
        let root_ref = outline_root(doc);
        let root_raw_count = root_ref.and_then(|r| doc.get(r).as_dict().and_then(|d| d.int(b"Count")));
        let root_count = root_raw_count.map_or(0, clamp_i32);

        let mut arena = Self { nodes: vec![OutlineArenaNode::from_cos(root_ref, root_count, root_raw_count.is_some(), true)] };

        let Some(root_ref) = root_ref else {
            arena.recompute_metadata();
            return Ok(arena);
        };

        let mut seen = HashSet::from([root_ref]);
        let first_top = doc.get(root_ref).as_dict().and_then(|d| d.reference(b"First"));
        let mut stack: Vec<(ArenaNodeId, Option<ObjRef>)> = vec![(ArenaNodeId::ROOT, first_top)];

        while let Some(top) = stack.last_mut() {
            let parent_id = top.0;
            let Some(item_ref) = top.1.take() else {
                stack.pop();
                continue;
            };
            if !seen.insert(item_ref) {
                continue;
            }
            let obj = doc.get(item_ref);
            let Some(d) = obj.as_dict() else { continue };
            top.1 = d.reference(b"Next");

            // Bookmark item budget (excluding virtual ROOT).
            if arena.nodes.len() > MAX_OUTLINE_NODES {
                return Err(OutlineError::TooManyBookmarks(MAX_OUTLINE_NODES));
            }
            let node_id = ArenaNodeId(u32::try_from(arena.nodes.len()).map_err(|_| OutlineError::TooManyBookmarks(MAX_OUTLINE_NODES))?);

            let raw_count = d.int(b"Count");
            let pdf_count = raw_count.map_or(0, clamp_i32);
            // ISO 32000-2 Table 151: strictly positive /Count opens expanded.
            let is_expanded = raw_count.is_some_and(|c| c > 0);
            let node = OutlineArenaNode::from_cos(Some(item_ref), pdf_count, raw_count.is_some(), is_expanded);
            let first_child = d.reference(b"First");
            arena.nodes.push(node);
            arena.link_append_raw(parent_id, node_id);

            if let Some(first) = first_child {
                stack.push((node_id, Some(first)));
            }
        }

        arena.recompute_metadata();
        Ok(arena)
    }

    fn link_append_raw(&mut self, parent: ArenaNodeId, child: ArenaNodeId) {
        let prev = self.nodes.get(parent.index()).and_then(|p| p.last_child);
        if let Some(c) = self.nodes.get_mut(child.index()) {
            c.parent = Some(parent);
            c.prev_sibling = prev;
            c.next_sibling = None;
        }
        if let Some(prev_id) = prev
            && let Some(p) = self.nodes.get_mut(prev_id.index())
        {
            p.next_sibling = Some(child);
        }
        if let Some(p) = self.nodes.get_mut(parent.index()) {
            if p.first_child.is_none() {
                p.first_child = Some(child);
            }
            p.last_child = Some(child);
        }
    }

    pub fn node(&self, id: ArenaNodeId) -> Option<&OutlineArenaNode> {
        self.nodes.get(id.index())
    }

    pub fn node_mut(&mut self, id: ArenaNodeId) -> Option<&mut OutlineArenaNode> {
        self.nodes.get_mut(id.index())
    }

    /// Resolve a 0-based outline path (`[]` = `ROOT`) to its [`ArenaNodeId`].
    pub fn resolve_path(&self, path: &[usize]) -> Result<ArenaNodeId, OutlineError> {
        let mut current = ArenaNodeId::ROOT;
        for &child_idx in path {
            let mut cursor = self.node(current).and_then(|n| n.first_child);
            let mut step = 0usize;
            while let Some(c) = cursor {
                if step == child_idx {
                    break;
                }
                cursor = self.node(c).and_then(|n| n.next_sibling);
                step = step.saturating_add(1);
            }
            current = cursor.ok_or_else(|| OutlineError::NoSuchBookmark(path.to_vec()))?;
        }
        Ok(current)
    }

    /// Compute the 0-based outline path from `ROOT` to `id`.
    pub fn node_to_path(&self, mut id: ArenaNodeId) -> Vec<usize> {
        let mut rev = Vec::new();
        let mut guard = 0usize;
        while id != ArenaNodeId::ROOT && guard <= self.nodes.len() {
            guard = guard.saturating_add(1);
            let Some(node) = self.node(id) else { break };
            let Some(parent) = node.parent else { break };
            let mut idx = 0usize;
            let mut cur = self.node(parent).and_then(|p| p.first_child);
            while let Some(c) = cur {
                if c == id {
                    break;
                }
                idx = idx.saturating_add(1);
                cur = self.node(c).and_then(|n| n.next_sibling);
            }
            rev.push(idx);
            id = parent;
        }
        rev.reverse();
        rev
    }

    fn collect_children(&self, parent: ArenaNodeId) -> Vec<ArenaNodeId> {
        let mut kids = Vec::new();
        let mut cur = self.node(parent).and_then(|n| n.first_child);
        let mut guard = 0usize;
        while let Some(c) = cur {
            if guard > self.nodes.len() {
                break;
            }
            guard = guard.saturating_add(1);
            kids.push(c);
            cur = self.node(c).and_then(|n| n.next_sibling);
        }
        kids
    }

    fn resolve_path_cached(&self, cache: &mut HashMap<ArenaNodeId, Vec<ArenaNodeId>>, path: &[usize]) -> Result<ArenaNodeId, OutlineError> {
        let mut current = ArenaNodeId::ROOT;
        for &child_idx in path {
            let kids = cache.entry(current).or_insert_with(|| self.collect_children(current));
            current = kids.get(child_idx).copied().ok_or_else(|| OutlineError::NoSuchBookmark(path.to_vec()))?;
        }
        Ok(current)
    }

    fn resolve_nonempty_paths(
        &self,
        cache: &mut HashMap<ArenaNodeId, Vec<ArenaNodeId>>,
        paths: &[Vec<usize>],
    ) -> Result<Vec<ArenaNodeId>, OutlineError> {
        if paths.len() > MAX_OUTLINE_NODES {
            return Err(OutlineError::TooManyBookmarks(MAX_OUTLINE_NODES));
        }
        let mut ids = Vec::with_capacity(paths.len());
        for p in paths {
            if p.is_empty() {
                return Err(OutlineError::NoSuchBookmark(Vec::new()));
            }
            ids.push(self.resolve_path_cached(cache, p)?);
        }
        Ok(ids)
    }

    fn collect_child_indices(&self, parent: ArenaNodeId) -> HashMap<ArenaNodeId, usize> {
        let mut indices = HashMap::new();
        let mut cur = self.node(parent).and_then(|n| n.first_child);
        let mut idx = 0usize;
        let mut guard = 0usize;
        while let Some(c) = cur {
            if guard > self.nodes.len() {
                break;
            }
            guard = guard.saturating_add(1);
            indices.entry(c).or_insert(idx);
            idx = idx.saturating_add(1);
            cur = self.node(c).and_then(|n| n.next_sibling);
        }
        indices
    }

    fn node_to_path_cached(&self, cache: &mut HashMap<ArenaNodeId, HashMap<ArenaNodeId, usize>>, mut id: ArenaNodeId) -> Vec<usize> {
        let mut rev = Vec::new();
        let mut guard = 0usize;
        while id != ArenaNodeId::ROOT && guard <= self.nodes.len() {
            guard = guard.saturating_add(1);
            let Some(node) = self.node(id) else { break };
            let Some(parent) = node.parent else { break };
            let indices = cache.entry(parent).or_insert_with(|| self.collect_child_indices(parent));
            let idx = indices.get(&id).copied().unwrap_or(indices.len());
            rev.push(idx);
            id = parent;
        }
        rev.reverse();
        rev
    }

    /// Recompute `depth`, `euler_in`, and `euler_out` in O(N) iterative time.
    pub fn recompute_metadata(&mut self) {
        struct Frame {
            id: ArenaNodeId,
            depth: u32,
            next_child: Option<ArenaNodeId>,
            entered: bool,
        }
        let Some(root_first) = self.node(ArenaNodeId::ROOT).map(|n| n.first_child) else { return };
        let mut timer: u32 = 0;
        let mut visited = HashSet::from([ArenaNodeId::ROOT]);
        let mut stack = vec![Frame { id: ArenaNodeId::ROOT, depth: 0, next_child: root_first, entered: false }];
        while let Some(frame) = stack.last_mut() {
            let id = frame.id;
            let Some(node) = self.nodes.get_mut(id.index()) else {
                stack.pop();
                continue;
            };
            if !frame.entered {
                frame.entered = true;
                node.depth = frame.depth;
                node.euler_in = timer;
                timer = timer.saturating_add(1);
            }
            if let Some(child_id) = frame.next_child {
                let (next_sibling, first_grandchild) = match self.node(child_id) {
                    Some(c) => (c.next_sibling, c.first_child),
                    None => (None, None),
                };
                let next_depth = frame.depth.saturating_add(1);
                frame.next_child = next_sibling;
                if visited.insert(child_id) {
                    stack.push(Frame { id: child_id, depth: next_depth, next_child: first_grandchild, entered: false });
                }
            } else {
                if let Some(done) = self.nodes.get_mut(id.index()) {
                    done.euler_out = timer.saturating_sub(1);
                }
                stack.pop();
            }
        }
    }

    /// Sort selected nodes in document order (`euler_in`) and absorb any node whose ancestor is also selected.
    pub fn normalize_and_prune_selection(&self, selected: &[ArenaNodeId]) -> Vec<ArenaNodeId> {
        let mut valid: Vec<(u32, ArenaNodeId)> =
            selected.iter().copied().filter(|&id| id != ArenaNodeId::ROOT).filter_map(|id| self.node(id).map(|n| (n.euler_in, id))).collect();
        valid.sort_unstable_by_key(|(ein, id)| (*ein, id.0));
        valid.dedup_by_key(|(_, id)| *id);

        let mut pruned = Vec::with_capacity(valid.len());
        let mut last_kept: Option<(u32, u32)> = None;
        for (ein, id) in valid {
            let Some(node) = self.node(id) else { continue };
            if let Some((root_in, root_out)) = last_kept
                && ein >= root_in
                && ein <= root_out
            {
                continue;
            }
            pruned.push(id);
            last_kept = Some((node.euler_in, node.euler_out));
        }
        pruned
    }

    /// Guard against dropping `pruned_roots` onto any of themselves or any of their descendants.
    pub fn is_valid_drop_target(&self, pruned_roots: &[ArenaNodeId], target: ArenaNodeId) -> bool {
        let Some(target_node) = self.node(target) else { return false };
        !pruned_roots.iter().any(|&u| self.node(u).is_some_and(|un| target_node.euler_in >= un.euler_in && target_node.euler_in <= un.euler_out))
    }

    /// Visibility weight `W` contributed by `node_id` to an open parent.
    pub fn visibility_weight(&self, node_id: ArenaNodeId) -> i32 {
        let Some(node) = self.node(node_id) else { return 0 };
        if node.first_child.is_none() || !node.is_expanded || node.pdf_count <= 0 { 1 } else { node.pdf_count.saturating_add(1) }
    }

    /// 3-Pass O(1)-per-root Caravan Splicing (`move_caravan`).
    ///
    /// Detaches all `pruned_roots`, forms a contiguous in-memory caravan, splices it into
    /// `placement`, auto-expands the target parent if needed, and recomputes `/Count` bottom-up
    /// along the affected old and new ancestor chains.
    pub fn move_caravan(&mut self, raw_selection: &[ArenaNodeId], placement: OutlinePlacement) -> Result<Vec<ArenaNodeId>, OutlineError> {
        if raw_selection.iter().any(|&id| id == ArenaNodeId::ROOT || self.node(id).is_none()) {
            return Err(OutlineError::NoSuchBookmark(Vec::new()));
        }
        let pruned_roots = self.normalize_and_prune_selection(raw_selection);
        if pruned_roots.is_empty() {
            return Ok(Vec::new());
        }
        let target = placement.target_node();
        if matches!(placement, OutlinePlacement::BeforeSibling(_) | OutlinePlacement::AfterSibling(_)) && target == ArenaNodeId::ROOT {
            return Err(OutlineError::NoSuchBookmark(Vec::new()));
        }
        if !self.is_valid_drop_target(&pruned_roots, target) {
            return Err(OutlineError::IntoItself);
        }
        let new_parent = match placement {
            OutlinePlacement::BeforeSibling(t) | OutlinePlacement::AfterSibling(t) => {
                self.node(t).and_then(|n| n.parent).ok_or_else(|| OutlineError::NoSuchBookmark(Vec::new()))?
            }
            OutlinePlacement::FirstChildOf(p) | OutlinePlacement::LastChildOf(p) => {
                if self.node(p).is_none() {
                    return Err(OutlineError::NoSuchBookmark(Vec::new()));
                }
                p
            }
        };

        // Collect all old and new ancestors before Pass A mutates `.parent` pointers.
        let mut affected = Vec::new();
        let mut seen_anc = HashSet::new();
        for &u in &pruned_roots {
            if let Some(op) = self.node(u).and_then(|n| n.parent) {
                self.collect_ancestors(op, &mut affected, &mut seen_anc);
            }
        }
        self.collect_ancestors(new_parent, &mut affected, &mut seen_anc);

        // Pass A: Detach each root cleanly without touching its subtree.
        for &u in &pruned_roots {
            self.detach_node(u);
        }

        // Pass B: Form contiguous caravan.
        self.link_caravan(&pruned_roots, new_parent);

        // Pass C: Splice caravan head/tail into destination & auto-expand target parent.
        self.splice_caravan(&pruned_roots, new_parent, placement);
        if new_parent != ArenaNodeId::ROOT
            && let Some(np) = self.node_mut(new_parent)
            && !np.is_expanded
        {
            np.is_expanded = true;
            np.dirty = true;
        }

        self.recompute_metadata();
        self.recompute_ancestors_counts(&affected);
        Ok(pruned_roots)
    }

    /// Detach `raw_selection` (pruned to top-most roots) from the tree cleanly.
    pub fn delete_caravan(&mut self, raw_selection: &[ArenaNodeId]) -> Result<Vec<ArenaNodeId>, OutlineError> {
        if raw_selection.iter().any(|&id| id == ArenaNodeId::ROOT || self.node(id).is_none()) {
            return Err(OutlineError::NoSuchBookmark(Vec::new()));
        }
        let pruned = self.normalize_and_prune_selection(raw_selection);
        if pruned.is_empty() {
            return Ok(Vec::new());
        }
        let mut affected = Vec::new();
        let mut seen = HashSet::new();
        for &u in &pruned {
            if let Some(op) = self.node(u).and_then(|n| n.parent) {
                self.collect_ancestors(op, &mut affected, &mut seen);
            }
        }
        for &u in &pruned {
            self.detach_node(u);
        }
        self.recompute_metadata();
        self.recompute_ancestors_counts(&affected);
        Ok(pruned)
    }

    /// Flush only `dirty == true` nodes into `doc`'s copy-on-write COS overlay,
    /// leaving untouched subtrees byte-exact in the COS document.
    pub fn commit_dirty_to_cos(&mut self, doc: &mut Document) -> Result<(), OutlineError> {
        let reachable = self.preorder_items();
        let mut reachable_mask = vec![false; self.nodes.len()];
        if let Some(root_slot) = reachable_mask.get_mut(ArenaNodeId::ROOT.index()) {
            *root_slot = true;
        }
        for &id in &reachable {
            if let Some(slot) = reachable_mask.get_mut(id.index()) {
                *slot = true;
            }
        }

        let obj_of = |nodes: &[OutlineArenaNode], opt_id: Option<ArenaNodeId>| -> Option<ObjRef> {
            opt_id.and_then(|id| nodes.get(id.index())).and_then(|n| n.cos_obj)
        };

        struct FlushPlan {
            idx: usize,
            is_root: bool,
            r: ObjRef,
            parent: Option<ObjRef>,
            prev: Option<ObjRef>,
            next: Option<ObjRef>,
            first: Option<ObjRef>,
            last: Option<ObjRef>,
            has_children: bool,
            pdf_count: i32,
            had_count: bool,
        }
        let mut plans = Vec::new();
        for (idx, n) in self.nodes.iter().enumerate() {
            if !n.dirty || !reachable_mask.get(idx).copied().unwrap_or(false) {
                continue;
            }
            let Some(r) = n.cos_obj else { continue };
            plans.push(FlushPlan {
                idx,
                is_root: idx == ArenaNodeId::ROOT.index(),
                r,
                parent: obj_of(&self.nodes, n.parent),
                prev: obj_of(&self.nodes, n.prev_sibling),
                next: obj_of(&self.nodes, n.next_sibling),
                first: obj_of(&self.nodes, n.first_child),
                last: obj_of(&self.nodes, n.last_child),
                has_children: n.first_child.is_some(),
                pdf_count: n.pdf_count,
                had_count: n.had_count,
            });
        }

        for plan in plans {
            let count_val = if plan.has_children { Some(Object::Int(i64::from(plan.pdf_count))) } else { None };
            if plan.is_root {
                put(doc, plan.r, b"First", plan.first.map(Object::Ref))?;
                put(doc, plan.r, b"Last", plan.last.map(Object::Ref))?;
                if plan.has_children || plan.had_count {
                    put(doc, plan.r, b"Count", count_val)?;
                }
            } else {
                put(doc, plan.r, b"Parent", plan.parent.map(Object::Ref))?;
                put(doc, plan.r, b"Prev", plan.prev.map(Object::Ref))?;
                put(doc, plan.r, b"Next", plan.next.map(Object::Ref))?;
                put(doc, plan.r, b"First", plan.first.map(Object::Ref))?;
                put(doc, plan.r, b"Last", plan.last.map(Object::Ref))?;
                if plan.has_children || plan.had_count {
                    put(doc, plan.r, b"Count", count_val)?;
                }
            }

            if let Some(node) = self.nodes.get_mut(plan.idx) {
                node.dirty = false;
                node.had_count = plan.has_children;
            }
        }
        Ok(())
    }

    /// Pre-order traversal of all reachable bookmark item IDs (excluding `ROOT`), iterative.
    pub fn preorder_items(&self) -> Vec<ArenaNodeId> {
        let mut out = Vec::new();
        let mut stack: Vec<Option<ArenaNodeId>> = vec![self.node(ArenaNodeId::ROOT).and_then(|n| n.first_child)];
        let mut seen = HashSet::new();
        while let Some(top) = stack.last_mut() {
            let Some(id) = top.take() else {
                stack.pop();
                continue;
            };
            if !seen.insert(id) {
                continue;
            }
            let Some(n) = self.node(id) else { continue };
            *top = n.next_sibling;
            out.push(id);
            if n.first_child.is_some() {
                stack.push(n.first_child);
            }
        }
        out
    }

    fn collect_ancestors(&self, start: ArenaNodeId, out: &mut Vec<ArenaNodeId>, seen: &mut HashSet<ArenaNodeId>) {
        let mut cur = Some(start);
        let mut guard = 0usize;
        while let Some(id) = cur {
            if guard > self.nodes.len() || !seen.insert(id) {
                break;
            }
            guard = guard.saturating_add(1);
            out.push(id);
            cur = self.node(id).and_then(|n| n.parent);
        }
    }

    fn detach_node(&mut self, u: ArenaNodeId) {
        let (old_parent, prev, next) = match self.node(u) {
            Some(n) => (n.parent, n.prev_sibling, n.next_sibling),
            None => return,
        };
        if let Some(p) = prev {
            if let Some(pn) = self.node_mut(p) {
                pn.next_sibling = next;
                pn.dirty = true;
            }
        } else if let Some(op) = old_parent
            && let Some(opn) = self.node_mut(op)
        {
            opn.first_child = next;
            opn.dirty = true;
        }
        if let Some(nx) = next {
            if let Some(nxn) = self.node_mut(nx) {
                nxn.prev_sibling = prev;
                nxn.dirty = true;
            }
        } else if let Some(op) = old_parent
            && let Some(opn) = self.node_mut(op)
        {
            opn.last_child = prev;
            opn.dirty = true;
        }
        if let Some(un) = self.node_mut(u) {
            un.parent = None;
            un.prev_sibling = None;
            un.next_sibling = None;
            un.dirty = true;
        }
    }

    fn link_caravan(&mut self, roots: &[ArenaNodeId], parent: ArenaNodeId) {
        for (k, &u) in roots.iter().enumerate() {
            let prev = k.checked_sub(1).and_then(|p| roots.get(p).copied());
            let next = roots.get(k.saturating_add(1)).copied();
            if let Some(un) = self.node_mut(u) {
                un.parent = Some(parent);
                un.prev_sibling = prev;
                un.next_sibling = next;
                un.dirty = true;
            }
        }
    }

    fn splice_caravan(&mut self, roots: &[ArenaNodeId], new_parent: ArenaNodeId, placement: OutlinePlacement) {
        let (Some(&head), Some(&tail)) = (roots.first(), roots.last()) else { return };
        match placement {
            OutlinePlacement::BeforeSibling(t) => {
                let left = self.node(t).and_then(|n| n.prev_sibling);
                if let Some(hn) = self.node_mut(head) {
                    hn.prev_sibling = left;
                }
                if let Some(tn) = self.node_mut(tail) {
                    tn.next_sibling = Some(t);
                }
                if let Some(tn) = self.node_mut(t) {
                    tn.prev_sibling = Some(tail);
                    tn.dirty = true;
                }
                if let Some(l) = left {
                    if let Some(ln) = self.node_mut(l) {
                        ln.next_sibling = Some(head);
                        ln.dirty = true;
                    }
                } else if let Some(np) = self.node_mut(new_parent) {
                    np.first_child = Some(head);
                    np.dirty = true;
                }
            }
            OutlinePlacement::AfterSibling(t) => {
                let right = self.node(t).and_then(|n| n.next_sibling);
                if let Some(hn) = self.node_mut(head) {
                    hn.prev_sibling = Some(t);
                }
                if let Some(tn) = self.node_mut(tail) {
                    tn.next_sibling = right;
                }
                if let Some(tn) = self.node_mut(t) {
                    tn.next_sibling = Some(head);
                    tn.dirty = true;
                }
                if let Some(r) = right {
                    if let Some(rn) = self.node_mut(r) {
                        rn.prev_sibling = Some(tail);
                        rn.dirty = true;
                    }
                } else if let Some(np) = self.node_mut(new_parent) {
                    np.last_child = Some(tail);
                    np.dirty = true;
                }
            }
            OutlinePlacement::FirstChildOf(p) => {
                let old_first = self.node(p).and_then(|n| n.first_child);
                if let Some(hn) = self.node_mut(head) {
                    hn.prev_sibling = None;
                }
                if let Some(tn) = self.node_mut(tail) {
                    tn.next_sibling = old_first;
                }
                if let Some(f) = old_first
                    && let Some(fn_) = self.node_mut(f)
                {
                    fn_.prev_sibling = Some(tail);
                    fn_.dirty = true;
                }
                if let Some(np) = self.node_mut(p) {
                    np.first_child = Some(head);
                    if old_first.is_none() {
                        np.last_child = Some(tail);
                    }
                    np.dirty = true;
                }
            }
            OutlinePlacement::LastChildOf(p) => {
                let old_last = self.node(p).and_then(|n| n.last_child);
                if let Some(hn) = self.node_mut(head) {
                    hn.prev_sibling = old_last;
                }
                if let Some(tn) = self.node_mut(tail) {
                    tn.next_sibling = None;
                }
                if let Some(l) = old_last
                    && let Some(ln) = self.node_mut(l)
                {
                    ln.next_sibling = Some(head);
                    ln.dirty = true;
                }
                if let Some(np) = self.node_mut(p) {
                    if old_last.is_none() {
                        np.first_child = Some(head);
                    }
                    np.last_child = Some(tail);
                    np.dirty = true;
                }
            }
        }
    }

    fn recompute_ancestors_counts(&mut self, ancestors: &[ArenaNodeId]) {
        let mut sorted: Vec<ArenaNodeId> = ancestors.to_vec();
        sorted.sort_unstable_by(|a, b| {
            let da = self.node(*a).map_or(0, |n| n.depth);
            let db = self.node(*b).map_or(0, |n| n.depth);
            db.cmp(&da).then(a.0.cmp(&b.0))
        });
        sorted.dedup();

        for anc in sorted {
            let Some((first_child, is_expanded, old_count, had_count)) =
                self.node(anc).map(|n| (n.first_child, n.is_expanded, n.pdf_count, n.had_count))
            else {
                continue;
            };
            let new_count = if first_child.is_none() {
                0
            } else {
                let mut sum: i32 = 0;
                let mut cur = first_child;
                let mut guard = 0usize;
                while let Some(c) = cur {
                    if guard > self.nodes.len() {
                        break;
                    }
                    guard = guard.saturating_add(1);
                    sum = sum.saturating_add(self.visibility_weight(c));
                    cur = self.node(c).and_then(|n| n.next_sibling);
                }
                if anc == ArenaNodeId::ROOT || is_expanded { sum } else { sum.saturating_neg() }
            };
            if (new_count != old_count || (first_child.is_none() && had_count))
                && let Some(an) = self.node_mut(anc)
            {
                an.pdf_count = new_count;
                an.dirty = true;
            }
        }
    }
}

/// Move `paths` (0-based outline paths) to `placement` relative to `target_path` in one surgical COS commit.
pub fn move_bookmarks_batch(
    doc: &mut Document,
    paths: &[Vec<usize>],
    target_path: &[usize],
    placement: OutlinePlacementKind,
) -> Result<Vec<Vec<usize>>, OutlineError> {
    let mut arena = OutlineArena::from_doc(doc)?;
    let mut children_cache = HashMap::new();
    let ids = arena.resolve_nonempty_paths(&mut children_cache, paths)?;
    let target_id = arena.resolve_path_cached(&mut children_cache, target_path)?;
    let slot = match placement {
        OutlinePlacementKind::Before => OutlinePlacement::BeforeSibling(target_id),
        OutlinePlacementKind::After => OutlinePlacement::AfterSibling(target_id),
        OutlinePlacementKind::FirstChild => OutlinePlacement::FirstChildOf(target_id),
        OutlinePlacementKind::LastChild => OutlinePlacement::LastChildOf(target_id),
    };
    let moved = arena.move_caravan(&ids, slot)?;
    arena.commit_dirty_to_cos(doc)?;
    let mut index_cache = HashMap::new();
    Ok(moved.into_iter().map(|id| arena.node_to_path_cached(&mut index_cache, id)).collect())
}

/// Delete multiple bookmark subtrees atomically in one surgical COS commit.
/// Descendants of already-selected ancestors are absorbed automatically.
pub fn delete_bookmarks_batch(doc: &mut Document, paths: &[Vec<usize>]) -> Result<(), OutlineError> {
    if paths.is_empty() {
        return Ok(());
    }
    let mut arena = OutlineArena::from_doc(doc)?;
    let mut children_cache = HashMap::new();
    let ids = arena.resolve_nonempty_paths(&mut children_cache, paths)?;
    arena.delete_caravan(&ids)?;
    arena.commit_dirty_to_cos(doc)
}

fn clamp_i32(v: i64) -> i32 {
    v.clamp(i64::from(i32::MIN), i64::from(i32::MAX)) as i32
}
