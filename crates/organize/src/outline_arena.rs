//! Index-arena outline model, Euler tour projection, O(1) caravan splicing,
//! regex nesting, and page-offset shifting (`crates/organize`).
//!
//! All traversals are iterative and cycle-guarded (`seen: HashSet<ObjRef>`)
//! with a hard [`MAX_OUTLINE_NODES`] budget so hostile or 100,000-level-deep
//! outlines never overflow the stack or loop indefinitely.

use std::collections::{HashMap, HashSet};

use pdfcraft_cos::{Dict, Document, ObjRef, Object, PdfString};
use regex::RegexBuilder;

use crate::outline::{OutlineError, outline_root, put, title_color, title_of};
use crate::{OrganizeError, walk};

/// Hard traversal budget for outline loading and arena construction.
/// Set to 200,000 so 100,000+ bookmark documents and tests work with headroom.
pub const MAX_OUTLINE_NODES: usize = 200_000;

/// Maximum nesting depth permitted by regex auto-nesting.
const MAX_REGEX_NEST_DEPTH: u32 = 32;

/// Bounded regex compilation limits matching `crates/redact/src/user_regex.rs`.
const MAX_REGEX_LEN: usize = 1024;
const REGEX_SIZE_LIMIT: usize = 256 * 1024;
const REGEX_NEST_LIMIT: u32 = 64;

/// Default numbering regex used by `regex_nest` (`"1.2.3 Title"` -> depth 3).
pub const DEFAULT_REGEX_NEST_PATTERN: &str = r"^(?P<num>\d+(?:\.\d+)*)\s+";

/// Stable index into [`OutlineArena::nodes`]. `ArenaNodeId::ROOT` (`0`) is the virtual `/Outlines` root.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub struct ArenaNodeId(pub u32);

impl ArenaNodeId {
    pub const ROOT: Self = Self(0);

    pub const fn index(self) -> usize {
        self.0 as usize
    }
}

/// Topological insertion target for caravan splicing and batch moves.
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

/// Exact ISO 32000-2 §12.3.2.2 Table 149 explicit destination with `f64` precision.
#[derive(Clone, Debug, PartialEq)]
pub enum ViewDestination {
    Xyz { page: usize, left: Option<f64>, top: Option<f64>, zoom: Option<f64> },
    Fit { page: usize },
    FitH { page: usize, top: Option<f64> },
    FitV { page: usize, left: Option<f64> },
    FitR { page: usize, rect: [f64; 4] },
    FitB { page: usize },
    FitBH { page: usize, top: Option<f64> },
    FitBV { page: usize, left: Option<f64> },
}

impl ViewDestination {
    /// 0-based target page index.
    pub const fn page(&self) -> usize {
        match self {
            Self::Xyz { page, .. }
            | Self::Fit { page }
            | Self::FitH { page, .. }
            | Self::FitV { page, .. }
            | Self::FitR { page, .. }
            | Self::FitB { page }
            | Self::FitBH { page, .. }
            | Self::FitBV { page, .. } => *page,
        }
    }

    /// Return a copy targeting `new_page` while preserving all coordinates and fit parameters.
    #[must_use]
    pub fn with_page(&self, new_page: usize) -> Self {
        match *self {
            Self::Xyz { left, top, zoom, .. } => Self::Xyz { page: new_page, left, top, zoom },
            Self::Fit { .. } => Self::Fit { page: new_page },
            Self::FitH { top, .. } => Self::FitH { page: new_page, top },
            Self::FitV { left, .. } => Self::FitV { page: new_page, left },
            Self::FitR { rect, .. } => Self::FitR { page: new_page, rect },
            Self::FitB { .. } => Self::FitB { page: new_page },
            Self::FitBH { top, .. } => Self::FitBH { page: new_page, top },
            Self::FitBV { left, .. } => Self::FitBV { page: new_page, left },
        }
    }

    /// Validate that target page exists and all numeric coordinates are finite (and positive zoom).
    pub fn validate(&self, page_count: usize) -> Result<(), OutlineError> {
        let page = self.page();
        if page >= page_count {
            return Err(OrganizeError::NoSuchPage(page).into());
        }
        let finite = |v: Option<f64>| v.is_none_or(f64::is_finite);
        match *self {
            Self::Xyz { left, top, zoom, .. } => {
                if !finite(left) || !finite(top) || !finite(zoom) || zoom.is_some_and(|z| z <= 0.0) {
                    return Err(OutlineError::BadPosition);
                }
            }
            Self::FitH { top, .. } | Self::FitBH { top, .. } => {
                if !finite(top) {
                    return Err(OutlineError::BadPosition);
                }
            }
            Self::FitV { left, .. } | Self::FitBV { left, .. } => {
                if !finite(left) {
                    return Err(OutlineError::BadPosition);
                }
            }
            Self::FitR { rect, .. } => {
                if !rect.iter().all(|v| v.is_finite()) {
                    return Err(OutlineError::BadPosition);
                }
            }
            Self::Fit { .. } | Self::FitB { .. } => {}
        }
        Ok(())
    }

    /// Parse an explicit destination array (`[page_ref /XYZ ...]`).
    pub(crate) fn from_cos_array(doc: &Document, page_map: &HashMap<ObjRef, usize>, page_count: usize, arr: &[Object]) -> Option<Self> {
        let page = match arr.first()? {
            Object::Ref(r) => *page_map.get(r)?,
            Object::Int(n) => usize::try_from(*n).ok().filter(|&p| p < page_count)?,
            _ => return None,
        };
        let kind = arr.get(1).map(|k| doc.resolve(k));
        let kind = kind.as_deref().and_then(Object::as_name)?;
        let num = |idx: usize| -> Option<f64> {
            let v = arr.get(idx)?;
            doc.resolve(v).as_f64().filter(|x| x.is_finite())
        };
        match kind {
            b"XYZ" => Some(Self::Xyz { page, left: num(2), top: num(3), zoom: num(4).filter(|z| *z > 0.0) }),
            b"Fit" => Some(Self::Fit { page }),
            b"FitH" => Some(Self::FitH { page, top: num(2) }),
            b"FitV" => Some(Self::FitV { page, left: num(2) }),
            b"FitR" => {
                let (x0, y0, x1, y1) = (num(2)?, num(3)?, num(4)?, num(5)?);
                Some(Self::FitR { page, rect: [x0.min(x1), y0.min(y1), x0.max(x1), y0.max(y1)] })
            }
            b"FitB" => Some(Self::FitB { page }),
            b"FitBH" => Some(Self::FitBH { page, top: num(2) }),
            b"FitBV" => Some(Self::FitBV { page, left: num(2) }),
            _ => None,
        }
    }

    /// Encode as a PDF explicit destination array referencing `pages[self.page()]`.
    pub fn to_cos_array(&self, pages: &[ObjRef]) -> Result<Object, OutlineError> {
        let page_ref = *pages.get(self.page()).ok_or(OrganizeError::NoSuchPage(self.page()))?;
        let opt = |v: Option<f64>| v.map_or(Object::Null, Object::Real);
        let elems = match *self {
            Self::Xyz { left, top, zoom, .. } => vec![Object::Ref(page_ref), Object::name("XYZ"), opt(left), opt(top), opt(zoom)],
            Self::Fit { .. } => vec![Object::Ref(page_ref), Object::name("Fit")],
            Self::FitH { top, .. } => vec![Object::Ref(page_ref), Object::name("FitH"), opt(top)],
            Self::FitV { left, .. } => vec![Object::Ref(page_ref), Object::name("FitV"), opt(left)],
            Self::FitR { rect: [l, b, r, t], .. } => {
                vec![Object::Ref(page_ref), Object::name("FitR"), Object::Real(l), Object::Real(b), Object::Real(r), Object::Real(t)]
            }
            Self::FitB { .. } => vec![Object::Ref(page_ref), Object::name("FitB")],
            Self::FitBH { top, .. } => vec![Object::Ref(page_ref), Object::name("FitBH"), opt(top)],
            Self::FitBV { left, .. } => vec![Object::Ref(page_ref), Object::name("FitBV"), opt(left)],
        };
        Ok(Object::Array(elems))
    }

    /// Convert into `(kind_bytes, values)` compatible with [`crate::OutlineNode::view`].
    #[must_use]
    pub fn to_portable(&self) -> (Vec<u8>, Vec<Option<f64>>) {
        match *self {
            Self::Xyz { left, top, zoom, .. } => (b"XYZ".to_vec(), vec![left, top, zoom]),
            Self::Fit { .. } => (b"Fit".to_vec(), Vec::new()),
            Self::FitH { top, .. } => (b"FitH".to_vec(), vec![top]),
            Self::FitV { left, .. } => (b"FitV".to_vec(), vec![left]),
            Self::FitR { rect: [l, b, r, t], .. } => (b"FitR".to_vec(), vec![Some(l), Some(b), Some(r), Some(t)]),
            Self::FitB { .. } => (b"FitB".to_vec(), Vec::new()),
            Self::FitBH { top, .. } => (b"FitBH".to_vec(), vec![top]),
            Self::FitBV { left, .. } => (b"FitBV".to_vec(), vec![left]),
        }
    }
}

/// One node in [`OutlineArena`].
#[derive(Clone, Debug, PartialEq)]
pub struct OutlineArenaNode {
    pub cos_obj: Option<ObjRef>,
    pub title: String,
    pub dest: Option<ViewDestination>,
    pub is_expanded: bool,
    pub style_flags: u8,
    pub color_rgb: Option<[f32; 3]>,
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
    pub(crate) title_dirty: bool,
    pub(crate) dest_dirty: bool,
    pub(crate) dest_is_goto_action: bool,
    pub(crate) style_dirty: bool,
    pub(crate) had_count: bool,
}

impl OutlineArenaNode {
    fn root(cos_obj: Option<ObjRef>, pdf_count: i32, had_count: bool) -> Self {
        Self {
            cos_obj,
            title: String::new(),
            dest: None,
            is_expanded: true,
            style_flags: 0,
            color_rgb: None,
            parent: None,
            prev_sibling: None,
            next_sibling: None,
            first_child: None,
            last_child: None,
            pdf_count,
            euler_in: 0,
            euler_out: 0,
            depth: 0,
            dirty: false,
            title_dirty: false,
            dest_dirty: false,
            dest_is_goto_action: false,
            style_dirty: false,
            had_count,
        }
    }

    /// Create a brand-new dirty bookmark node ready for arena splicing and COS allocation.
    pub fn new(title: impl Into<String>, dest: Option<ViewDestination>, color_rgb: Option<[f32; 3]>, style_flags: u8, is_expanded: bool) -> Self {
        Self {
            cos_obj: None,
            title: title.into(),
            dest,
            is_expanded,
            style_flags,
            color_rgb,
            parent: None,
            prev_sibling: None,
            next_sibling: None,
            first_child: None,
            last_child: None,
            pdf_count: 0,
            euler_in: 0,
            euler_out: 0,
            depth: 0,
            dirty: true,
            title_dirty: true,
            dest_dirty: true,
            dest_is_goto_action: false,
            style_dirty: true,
            had_count: false,
        }
    }
}

/// Flattened visible row produced by [`OutlineArena::recompute_metadata`].
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct FlatVisibleRow {
    pub node_id: ArenaNodeId,
    pub depth: u32,
    pub has_children: bool,
    pub is_expanded: bool,
}

/// Flat index-arena representation of the document's `/Outlines` tree.
#[derive(Clone, Debug)]
pub struct OutlineArena {
    pub nodes: Vec<OutlineArenaNode>,
    pub flat_visible_rows: Vec<FlatVisibleRow>,
}

impl OutlineArena {
    /// Load the `/Outlines` tree from `doc` iteratively with cycle and node-budget guards.
    pub fn from_doc(doc: &Document) -> Result<Self, OutlineError> {
        let root_ref = outline_root(doc);
        let root_dict = root_ref.and_then(|r| doc.get(r).as_dict().cloned());
        let root_raw_count = root_dict.as_ref().and_then(|d| d.int(b"Count"));
        let root_count = root_raw_count.map_or(0, clamp_i32);

        let mut arena = Self { nodes: vec![OutlineArenaNode::root(root_ref, root_count, root_raw_count.is_some())], flat_visible_rows: Vec::new() };

        let Some(root_ref) = root_ref else {
            arena.recompute_metadata();
            return Ok(arena);
        };
        let pages = walk(doc).unwrap_or_default();
        let page_count = pages.len();
        let page_map: HashMap<ObjRef, usize> = pages.into_iter().enumerate().map(|(i, (r, _))| (r, i)).collect();
        let mut names_cache: Option<HashMap<Vec<u8>, Object>> = None;

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

            let node = load_node(doc, item_ref, d, &page_map, page_count, &mut names_cache);
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

    /// Recompute `depth`, `euler_in`, `euler_out`, and `flat_visible_rows` in O(N) iterative time.
    pub fn recompute_metadata(&mut self) {
        self.flat_visible_rows.clear();
        struct Frame {
            id: ArenaNodeId,
            depth: u32,
            next_child: Option<ArenaNodeId>,
            entered: bool,
            visible: bool,
        }
        let Some(root_first) = self.node(ArenaNodeId::ROOT).map(|n| n.first_child) else { return };
        let mut timer: u32 = 0;
        let mut stack = vec![Frame { id: ArenaNodeId::ROOT, depth: 0, next_child: root_first, entered: false, visible: true }];
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
                if id != ArenaNodeId::ROOT && frame.visible {
                    self.flat_visible_rows.push(FlatVisibleRow {
                        node_id: id,
                        depth: frame.depth,
                        has_children: node.first_child.is_some(),
                        is_expanded: node.is_expanded,
                    });
                }
            }
            if let Some(child_id) = frame.next_child {
                let parent_expanded = self.node(id).is_some_and(|n| n.is_expanded);
                let (next_sibling, first_grandchild) = match self.node(child_id) {
                    Some(c) => (c.next_sibling, c.first_child),
                    None => (None, None),
                };
                frame.next_child = next_sibling;
                let child_visible = frame.visible && parent_expanded;
                let child_depth = frame.depth.saturating_add(1);
                stack.push(Frame { id: child_id, depth: child_depth, next_child: first_grandchild, entered: false, visible: child_visible });
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

    /// Add `delta` to open ancestors from `start_parent` up to `ROOT`, stopping at the first collapsed ancestor.
    pub fn adjust_ancestor_counts(&mut self, start_parent: ArenaNodeId, delta: i32) {
        if delta == 0 {
            return;
        }
        let mut cur = Some(start_parent);
        let mut guard = 0usize;
        while let Some(id) = cur {
            if guard > self.nodes.len() {
                break;
            }
            guard = guard.saturating_add(1);
            let Some(node) = self.node_mut(id) else { break };
            if id == ArenaNodeId::ROOT || node.is_expanded {
                node.pdf_count = node.pdf_count.saturating_add(delta);
                node.dirty = true;
                cur = node.parent;
            } else {
                node.pdf_count = node.pdf_count.saturating_sub(delta);
                node.dirty = true;
                break;
            }
        }
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

    /// Insert new nodes as a contiguous sibling run at `placement` (validating target first).
    pub fn insert_caravan(&mut self, nodes: Vec<OutlineArenaNode>, placement: OutlinePlacement) -> Result<Vec<ArenaNodeId>, OutlineError> {
        if nodes.is_empty() {
            return Ok(Vec::new());
        }
        let target = placement.target_node();
        if matches!(placement, OutlinePlacement::BeforeSibling(_) | OutlinePlacement::AfterSibling(_)) && target == ArenaNodeId::ROOT {
            return Err(OutlineError::NoSuchBookmark(Vec::new()));
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
        if self.nodes.len().saturating_add(nodes.len()) > MAX_OUTLINE_NODES.saturating_add(1) {
            return Err(OutlineError::TooManyBookmarks(MAX_OUTLINE_NODES));
        }

        let mut ids = Vec::with_capacity(nodes.len());
        for node in nodes {
            let id = ArenaNodeId(u32::try_from(self.nodes.len()).map_err(|_| OutlineError::TooManyBookmarks(MAX_OUTLINE_NODES))?);
            self.nodes.push(node);
            ids.push(id);
        }

        let mut affected = Vec::new();
        let mut seen = HashSet::new();
        self.collect_ancestors(new_parent, &mut affected, &mut seen);

        self.link_caravan(&ids, new_parent);
        self.splice_caravan(&ids, new_parent, placement);
        if new_parent != ArenaNodeId::ROOT
            && let Some(np) = self.node_mut(new_parent)
            && !np.is_expanded
        {
            np.is_expanded = true;
            np.dirty = true;
        }
        self.recompute_metadata();
        self.recompute_ancestors_counts(&affected);
        Ok(ids)
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

    /// Toggle expansion of a node in memory (updating `is_expanded`, `pdf_count`, ancestors, and `flat_visible_rows`).
    pub fn set_expanded(&mut self, id: ArenaNodeId, expanded: bool) -> Result<(), OutlineError> {
        let Some(node) = self.node(id) else {
            return Err(OutlineError::NoSuchBookmark(Vec::new()));
        };
        if id == ArenaNodeId::ROOT || node.first_child.is_none() || node.is_expanded == expanded {
            return Ok(());
        }
        let mut affected = Vec::new();
        let mut seen = HashSet::new();
        self.collect_ancestors(id, &mut affected, &mut seen);
        if let Some(n) = self.node_mut(id) {
            n.is_expanded = expanded;
            n.dirty = true;
        }
        self.recompute_metadata();
        self.recompute_ancestors_counts(&affected);
        Ok(())
    }

    /// Re-nest the entire outline in pre-order using a bounded numbering regex and Monotonic Stack.
    ///
    /// Matched items get target depth `segment_count` (clamped so depth never jumps by > 1);
    /// unmatched items attach one level beneath the immediately preceding matched heading
    /// (or depth 1 if before any match).
    pub fn regex_nest(&mut self, pattern: Option<&str>) -> Result<(), OutlineError> {
        let source = pattern.unwrap_or(DEFAULT_REGEX_NEST_PATTERN);
        if source.trim().is_empty() {
            return Err(OrganizeError::Invalid("regex pattern must not be empty".into()).into());
        }
        if source.len() > MAX_REGEX_LEN {
            return Err(OrganizeError::Invalid(format!("regex pattern exceeds {MAX_REGEX_LEN} bytes")).into());
        }
        let re = RegexBuilder::new(source)
            .size_limit(REGEX_SIZE_LIMIT)
            .dfa_size_limit(REGEX_SIZE_LIMIT)
            .nest_limit(REGEX_NEST_LIMIT)
            .build()
            .map_err(|e| OrganizeError::Invalid(format!("invalid regex pattern: {e}")))?;

        let order = self.preorder_items();
        if order.is_empty() {
            return Ok(());
        }

        // Compute target depth (1-based) for each item in pre-order.
        let mut targets: Vec<(ArenaNodeId, u32)> = Vec::with_capacity(order.len());
        let mut last_matched_depth: Option<u32> = None;
        for id in order {
            let title = self.node(id).map(|n| n.title.as_str()).unwrap_or("");
            let depth = if let Some(caps) = re.captures(title) {
                let seg = caps.name("num").or_else(|| caps.get(1)).or_else(|| caps.get(0)).map(|m| m.as_str()).unwrap_or("");
                let count = u32::try_from(seg.split('.').filter(|s| !s.trim().is_empty()).count().max(1)).unwrap_or(MAX_REGEX_NEST_DEPTH);
                let clamped = count.clamp(1, MAX_REGEX_NEST_DEPTH);
                last_matched_depth = Some(clamped);
                clamped
            } else {
                match last_matched_depth {
                    Some(d) => d.saturating_add(1).min(MAX_REGEX_NEST_DEPTH),
                    None => 1,
                }
            };
            targets.push((id, depth));
        }

        // Clear existing topology links on ROOT and every visited item.
        for &(id, _) in &targets {
            if let Some(n) = self.node_mut(id) {
                n.parent = None;
                n.prev_sibling = None;
                n.next_sibling = None;
                n.first_child = None;
                n.last_child = None;
                n.dirty = true;
            }
        }
        if let Some(root) = self.node_mut(ArenaNodeId::ROOT) {
            root.first_child = None;
            root.last_child = None;
            root.dirty = true;
        }

        // Monotonic Stack: `(depth, node_id)`.
        let mut stack: Vec<(u32, ArenaNodeId)> = vec![(0, ArenaNodeId::ROOT)];
        for (id, raw_depth) in targets {
            while stack.len() > 1 && stack.last().is_some_and(|&(d, _)| d >= raw_depth) {
                stack.pop();
            }
            let parent = stack.last().map_or(ArenaNodeId::ROOT, |&(_, pid)| pid);
            if parent != ArenaNodeId::ROOT
                && let Some(pn) = self.node_mut(parent)
            {
                pn.is_expanded = true;
            }
            self.link_append_raw(parent, id);
            stack.push((raw_depth, id));
        }

        self.recompute_metadata();
        let all = self.all_reachable_bottom_up();
        self.recompute_ancestors_counts(&all);
        Ok(())
    }

    /// Shift `/Dest` / `/GoTo` page targets by `page_delta` across `raw_selection` (optionally including subtrees).
    /// Fails atomically before mutating if any targeted page destination falls out of `0..page_count`.
    pub fn shift_pages(
        &mut self,
        raw_selection: &[ArenaNodeId],
        include_children: bool,
        page_delta: i32,
        page_count: usize,
    ) -> Result<usize, OutlineError> {
        if raw_selection.iter().any(|&id| id == ArenaNodeId::ROOT || self.node(id).is_none()) {
            return Err(OutlineError::NoSuchBookmark(Vec::new()));
        }
        let targets = if include_children {
            let roots = self.normalize_and_prune_selection(raw_selection);
            self.expand_subtrees(&roots)
        } else {
            let mut ids: Vec<ArenaNodeId> = raw_selection.to_vec();
            ids.sort_unstable_by_key(|id| self.node(*id).map_or(u32::MAX, |n| n.euler_in));
            ids.dedup();
            ids
        };

        // Pre-validate every targeted node with a page destination before mutating anything.
        let mut updates: Vec<(ArenaNodeId, ViewDestination)> = Vec::new();
        for id in targets {
            let Some(node) = self.node(id) else { continue };
            let Some(dest) = node.dest.as_ref() else { continue };
            let shifted = shift_page_index(dest.page(), page_delta, page_count)?;
            updates.push((id, dest.with_page(shifted)));
        }

        if page_delta == 0 {
            return Ok(updates.len());
        }

        let count = updates.len();
        for (id, new_dest) in updates {
            if let Some(node) = self.node_mut(id) {
                node.dest = Some(new_dest);
                node.dirty = true;
                node.dest_dirty = true;
            }
        }
        Ok(count)
    }

    /// Update a single node's destination directly (used by `"Set Destination to Current View"`).
    pub fn set_destination(&mut self, id: ArenaNodeId, dest: ViewDestination, page_count: usize) -> Result<(), OutlineError> {
        if id == ArenaNodeId::ROOT || self.node(id).is_none() {
            return Err(OutlineError::NoSuchBookmark(Vec::new()));
        }
        dest.validate(page_count)?;
        if let Some(node) = self.node_mut(id) {
            node.dest = Some(dest);
            node.dest_is_goto_action = false;
            node.dest_dirty = true;
            node.dirty = true;
        }
        Ok(())
    }

    /// Flush only `dirty == true` nodes into `doc`'s copy-on-write COS overlay,
    /// leaving untouched subtrees byte-exact in the COS document.
    pub fn commit_dirty_to_cos(&mut self, doc: &mut Document) -> Result<(), OutlineError> {
        let root_has_children = self.node(ArenaNodeId::ROOT).is_some_and(|n| n.first_child.is_some());
        if root_has_children && self.node(ArenaNodeId::ROOT).is_some_and(|n| n.cos_obj.is_none()) {
            let mut o = Dict::new();
            o.set(b"Type".to_vec(), Object::name("Outlines"));
            let r = doc.add(Object::Dict(o));
            let catalog = doc.root().ok_or(OrganizeError::NoPageTree)?;
            doc.update_dict(catalog, |c| c.set(b"Outlines".to_vec(), Object::Ref(r)))?;
            if let Some(root) = self.node_mut(ArenaNodeId::ROOT) {
                root.cos_obj = Some(r);
                root.dirty = true;
            }
        }

        // Pre-allocate COS objects for reachable dirty nodes that don't have one yet.
        let reachable = self.preorder_items();
        let mut reachable_mask = vec![false; self.nodes.len()];
        if let Some(root_slot) = reachable_mask.get_mut(ArenaNodeId::ROOT.index()) {
            *root_slot = true;
        }
        for &id in &reachable {
            if let Some(slot) = reachable_mask.get_mut(id.index()) {
                *slot = true;
            }
            if self.node(id).is_some_and(|n| n.cos_obj.is_none()) {
                let r = doc.add(Object::Dict(Dict::new()));
                if let Some(n) = self.node_mut(id) {
                    n.cos_obj = Some(r);
                    n.dirty = true;
                    n.title_dirty = true;
                    n.dest_dirty = true;
                    n.style_dirty = true;
                }
            }
        }

        let need_pages = self
            .nodes
            .iter()
            .enumerate()
            .any(|(idx, n)| reachable_mask.get(idx).copied().unwrap_or(false) && n.dirty && n.dest_dirty && n.dest.is_some());
        let pages: Vec<ObjRef> = if need_pages { walk(doc)?.into_iter().map(|(r, _)| r).collect() } else { Vec::new() };

        // Map arena IDs to their COS ObjRefs before taking mutable references in the flush loop.
        let obj_of = |nodes: &[OutlineArenaNode], opt_id: Option<ArenaNodeId>| -> Option<ObjRef> {
            opt_id.and_then(|id| nodes.get(id.index())).and_then(|n| n.cos_obj)
        };

        // Snapshot link ObjRefs for every reachable dirty node first to avoid borrow conflicts.
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
            if plan.is_root {
                put(doc, plan.r, b"First", plan.first.map(Object::Ref))?;
                put(doc, plan.r, b"Last", plan.last.map(Object::Ref))?;
                let count_val = if plan.has_children { Some(Object::Int(i64::from(plan.pdf_count))) } else { None };
                put(doc, plan.r, b"Count", count_val)?;
            } else {
                put(doc, plan.r, b"Parent", plan.parent.map(Object::Ref))?;
                put(doc, plan.r, b"Prev", plan.prev.map(Object::Ref))?;
                put(doc, plan.r, b"Next", plan.next.map(Object::Ref))?;
                put(doc, plan.r, b"First", plan.first.map(Object::Ref))?;
                put(doc, plan.r, b"Last", plan.last.map(Object::Ref))?;
                let count_val = if plan.has_children { Some(Object::Int(i64::from(plan.pdf_count))) } else { None };
                if plan.has_children || plan.had_count {
                    put(doc, plan.r, b"Count", count_val)?;
                }

                let Some(node) = self.nodes.get(plan.idx) else { continue };
                if node.title_dirty {
                    put(doc, plan.r, b"Title", Some(Object::String(PdfString::text(&node.title))))?;
                }
                if node.dest_dirty {
                    flush_node_dest(doc, plan.r, node, &pages)?;
                }
                if node.style_dirty {
                    let color_val =
                        node.color_rgb.filter(|c| *c != [0.0; 3]).map(|c| Object::Array(c.iter().map(|&v| Object::Real(f64::from(v))).collect()));
                    put(doc, plan.r, b"C", color_val)?;
                    put(doc, plan.r, b"F", (node.style_flags != 0).then_some(Object::Int(i64::from(node.style_flags))))?;
                }
            }

            if let Some(node) = self.nodes.get_mut(plan.idx) {
                node.dirty = false;
                node.title_dirty = false;
                node.dest_dirty = false;
                node.style_dirty = false;
                node.had_count = plan.has_children;
            }
        }
        Ok(())
    }

    /// Convert each pruned `root` subtree into portable [`crate::OutlineNode`] iteratively.
    pub fn to_outline_nodes(&self, roots: &[ArenaNodeId]) -> Vec<crate::OutlineNode> {
        struct Frame {
            node: crate::OutlineNode,
            next_child: Option<ArenaNodeId>,
        }
        let make_portable = |n: &OutlineArenaNode| crate::OutlineNode {
            title: n.title.clone(),
            page: n.dest.as_ref().map(ViewDestination::page),
            view: n.dest.as_ref().map(ViewDestination::to_portable),
            color: n.color_rgb,
            italic: n.style_flags & 1 != 0,
            bold: n.style_flags & 2 != 0,
            open: n.is_expanded,
            children: Vec::new(),
        };

        let mut out = Vec::with_capacity(roots.len());
        let mut seen = HashSet::new();
        for &root_id in roots {
            if root_id == ArenaNodeId::ROOT || !seen.insert(root_id) {
                continue;
            }
            let Some(root_node) = self.node(root_id) else { continue };
            let mut stack = vec![Frame { node: make_portable(root_node), next_child: root_node.first_child }];
            while let Some(top) = stack.last_mut() {
                if let Some(cid) = top.next_child {
                    let next_sib = self.node(cid).and_then(|c| c.next_sibling);
                    top.next_child = next_sib;
                    if seen.insert(cid)
                        && let Some(cn) = self.node(cid)
                    {
                        stack.push(Frame { node: make_portable(cn), next_child: cn.first_child });
                    }
                } else if let Some(done) = stack.pop() {
                    if let Some(parent) = stack.last_mut() {
                        parent.node.children.push(done.node);
                    } else {
                        out.push(done.node);
                    }
                }
            }
        }
        out
    }

    /// Insert auto-generated [`crate::OutlineEntry`] items using a Monotonic Stack under `ROOT`.
    /// Level 1 headings sit directly at `/Outlines` root (`replace_existing` clears previous top-level items).
    pub fn insert_generated_entries(
        &mut self,
        entries: &[crate::OutlineEntry],
        page_count: usize,
        replace_existing: bool,
    ) -> Result<usize, OutlineError> {
        if entries.is_empty() {
            return Err(OutlineError::NoEntries);
        }
        if self.nodes.len().saturating_add(entries.len()) > MAX_OUTLINE_NODES.saturating_add(1) {
            return Err(OutlineError::TooManyBookmarks(MAX_OUTLINE_NODES));
        }
        for e in entries {
            if e.page >= page_count {
                return Err(OrganizeError::NoSuchPage(e.page).into());
            }
            if e.top.is_some_and(|t| !t.is_finite()) {
                return Err(OutlineError::BadPosition);
            }
        }

        if replace_existing && let Some(root) = self.node_mut(ArenaNodeId::ROOT) {
            root.first_child = None;
            root.last_child = None;
            root.dirty = true;
        }

        let mut affected = vec![ArenaNodeId::ROOT];
        if let Some(existing_tail) = self.node(ArenaNodeId::ROOT).and_then(|r| r.last_child)
            && let Some(tail_node) = self.node_mut(existing_tail)
        {
            tail_node.dirty = true;
        }
        if let Some(root) = self.node_mut(ArenaNodeId::ROOT) {
            root.dirty = true;
        }

        let mut stack: Vec<(u8, ArenaNodeId)> = vec![(0, ArenaNodeId::ROOT)];
        let mut inserted = 0usize;
        for e in entries {
            let level = e.level.max(1);
            let sanitized = sanitize_bookmark_title(&e.title, 128);
            let title = if sanitized.is_empty() { "Untitled".to_string() } else { sanitized };
            let dest = ViewDestination::Xyz { page: e.page, left: None, top: e.top, zoom: None };
            let id = ArenaNodeId(u32::try_from(self.nodes.len()).map_err(|_| OutlineError::TooManyBookmarks(MAX_OUTLINE_NODES))?);
            self.nodes.push(OutlineArenaNode::new(title, Some(dest), None, 0, true));

            while stack.len() > 1 && stack.last().is_some_and(|&(lvl, _)| lvl >= level) {
                stack.pop();
            }
            let parent = stack.last().map_or(ArenaNodeId::ROOT, |&(_, pid)| pid);
            if let Some(prev_tail) = self.node(parent).and_then(|p| p.last_child)
                && let Some(ptn) = self.node_mut(prev_tail)
            {
                ptn.dirty = true;
            }
            if let Some(pn) = self.node_mut(parent) {
                pn.dirty = true;
            }
            self.link_append_raw(parent, id);
            stack.push((level, id));
            affected.push(parent);
            inserted = inserted.saturating_add(1);
        }

        self.recompute_metadata();
        self.recompute_ancestors_counts(&affected);
        Ok(inserted)
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

    fn all_reachable_bottom_up(&self) -> Vec<ArenaNodeId> {
        let mut all = self.preorder_items();
        all.insert(0, ArenaNodeId::ROOT);
        all
    }

    fn expand_subtrees(&self, roots: &[ArenaNodeId]) -> Vec<ArenaNodeId> {
        let mut out = Vec::new();
        let mut seen = HashSet::new();
        for &root in roots {
            let mut stack: Vec<Option<ArenaNodeId>> = vec![Some(root)];
            let mut is_first = true;
            while let Some(top) = stack.last_mut() {
                let Some(id) = top.take() else {
                    stack.pop();
                    continue;
                };
                if !seen.insert(id) {
                    continue;
                }
                let Some(n) = self.node(id) else { continue };
                if is_first {
                    is_first = false;
                } else {
                    *top = n.next_sibling;
                }
                out.push(id);
                if n.first_child.is_some() {
                    stack.push(n.first_child);
                }
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
    let mut ids = Vec::with_capacity(paths.len());
    for p in paths {
        if p.is_empty() {
            return Err(OutlineError::NoSuchBookmark(Vec::new()));
        }
        ids.push(arena.resolve_path(p)?);
    }
    let target_id = arena.resolve_path(target_path)?;
    let slot = match placement {
        OutlinePlacementKind::Before => OutlinePlacement::BeforeSibling(target_id),
        OutlinePlacementKind::After => OutlinePlacement::AfterSibling(target_id),
        OutlinePlacementKind::FirstChild => OutlinePlacement::FirstChildOf(target_id),
        OutlinePlacementKind::LastChild => OutlinePlacement::LastChildOf(target_id),
    };
    let moved = arena.move_caravan(&ids, slot)?;
    arena.commit_dirty_to_cos(doc)?;
    Ok(moved.into_iter().map(|id| arena.node_to_path(id)).collect())
}

/// Placement selector for path-based batch moves and automation tools.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum OutlinePlacementKind {
    Before,
    After,
    FirstChild,
    LastChild,
}

/// Point an existing bookmark at `dest` (exact Table 149 destination) and clear any `/A` action.
pub fn set_bookmark_destination(doc: &mut Document, path: &[usize], dest: &ViewDestination) -> Result<(), OutlineError> {
    if path.is_empty() {
        return Err(OutlineError::NoSuchBookmark(Vec::new()));
    }
    let page_count = walk(doc)?.len();
    let mut arena = OutlineArena::from_doc(doc)?;
    let id = arena.resolve_path(path)?;
    arena.set_destination(id, dest.clone(), page_count)?;
    arena.commit_dirty_to_cos(doc)
}

/// Re-nest the outline by matching bookmark titles against `pattern` (default numbering regex).
pub fn regex_nest_bookmarks(doc: &mut Document, pattern: Option<&str>) -> Result<(), OutlineError> {
    let mut arena = OutlineArena::from_doc(doc)?;
    arena.regex_nest(pattern)?;
    arena.commit_dirty_to_cos(doc)
}

/// Shift `/Dest` / `/GoTo` page targets across `paths` (optionally including subtrees) by `page_delta`.
pub fn shift_bookmark_pages(doc: &mut Document, paths: &[Vec<usize>], include_children: bool, page_delta: i32) -> Result<usize, OutlineError> {
    let page_count = walk(doc)?.len();
    let mut arena = OutlineArena::from_doc(doc)?;
    let mut ids = Vec::with_capacity(paths.len());
    for p in paths {
        if p.is_empty() {
            return Err(OutlineError::NoSuchBookmark(Vec::new()));
        }
        ids.push(arena.resolve_path(p)?);
    }
    let shifted = arena.shift_pages(&ids, include_children, page_delta, page_count)?;
    arena.commit_dirty_to_cos(doc)?;
    Ok(shifted)
}

/// Sanitize a bookmark title into a single whitespace-collapsed line capped at `max_chars` scalar values.
pub fn sanitize_bookmark_title(raw: &str, max_chars: usize) -> String {
    let single_line: String = raw.chars().map(|ch| if ch.is_control() { ' ' } else { ch }).collect();
    let collapsed = single_line.split_whitespace().collect::<Vec<_>>().join(" ");
    if max_chars == 0 {
        return String::new();
    }
    if collapsed.chars().count() <= max_chars {
        return collapsed;
    }
    collapsed.chars().take(max_chars).collect::<String>().trim_end().to_string()
}

/// Delete multiple bookmark subtrees atomically in one surgical COS commit.
/// Descendants of already-selected ancestors are absorbed automatically.
pub fn delete_bookmarks_batch(doc: &mut Document, paths: &[Vec<usize>]) -> Result<(), OutlineError> {
    if paths.is_empty() {
        return Ok(());
    }
    let mut arena = OutlineArena::from_doc(doc)?;
    let mut ids = Vec::with_capacity(paths.len());
    for p in paths {
        if p.is_empty() {
            return Err(OutlineError::NoSuchBookmark(Vec::new()));
        }
        ids.push(arena.resolve_path(p)?);
    }
    arena.delete_caravan(&ids)?;
    arena.commit_dirty_to_cos(doc)
}

/// Portable batch clipboard payload plus COS page `ObjRef` map for same-document validation.
#[derive(Clone, Debug, PartialEq)]
pub struct CopiedOutlines {
    pub tree: crate::OutlineTree,
    /// Map from 0-based `page` index at copy time to its COS page `ObjRef`.
    pub page_refs: HashMap<usize, ObjRef>,
}

/// Export `paths` (in normalized, pruned document order) without the 32-level listing cap.
pub fn outline_trees_batch(doc: &Document, paths: &[Vec<usize>]) -> Result<CopiedOutlines, OutlineError> {
    let arena = OutlineArena::from_doc(doc)?;
    let mut raw_ids = Vec::with_capacity(paths.len());
    for p in paths {
        if p.is_empty() {
            return Err(OutlineError::NoSuchBookmark(Vec::new()));
        }
        raw_ids.push(arena.resolve_path(p)?);
    }
    let pruned = arena.normalize_and_prune_selection(&raw_ids);
    let nodes = arena.to_outline_nodes(&pruned);

    let pages = walk(doc).unwrap_or_default();
    let mut page_refs = HashMap::new();
    let mut stack: Vec<&crate::OutlineNode> = nodes.iter().collect();
    while let Some(node) = stack.pop() {
        if let Some(p) = node.page
            && let Some(&(obj, _)) = pages.get(p)
        {
            page_refs.insert(p, obj);
        }
        for c in &node.children {
            stack.push(c);
        }
    }

    Ok(CopiedOutlines { tree: crate::OutlineTree { nodes, truncated: false }, page_refs })
}

/// Validate and remap `nodes` destinations for same-document paste against the current Page Tree.
/// If a copied page `ObjRef` moved in page order, its index is updated; if deleted, the index
/// clamps to surviving pages (or becomes `None` on an empty document).
pub fn remap_copied_outlines(doc: &Document, nodes: &mut [crate::OutlineNode], page_refs: &HashMap<usize, ObjRef>) -> Result<(), OrganizeError> {
    let pages = walk(doc)?;
    let page_count = pages.len();
    let obj_to_idx: HashMap<ObjRef, usize> = pages.into_iter().enumerate().map(|(i, (r, _))| (r, i)).collect();

    let mut stack: Vec<&mut crate::OutlineNode> = nodes.iter_mut().collect();
    while let Some(node) = stack.pop() {
        if let Some(old_page) = node.page {
            if let Some(&new_idx) = page_refs.get(&old_page).and_then(|obj| obj_to_idx.get(obj)) {
                node.page = Some(new_idx);
            } else if page_count > 0 {
                node.page = Some(old_page.min(page_count.saturating_sub(1)));
            } else {
                node.page = None;
                node.view = None;
            }
        }
        for c in &mut node.children {
            stack.push(c);
        }
    }
    Ok(())
}

/// Insert auto-generated outline entries directly under the `/Outlines` root.
pub fn add_generated_bookmark_tree(doc: &mut Document, entries: &[crate::OutlineEntry], replace_existing: bool) -> Result<usize, OutlineError> {
    let page_count = walk(doc)?.len();
    let mut arena = OutlineArena::from_doc(doc)?;
    let count = arena.insert_generated_entries(entries, page_count, replace_existing)?;
    arena.commit_dirty_to_cos(doc)?;
    Ok(count)
}

fn shift_page_index(page: usize, delta: i32, page_count: usize) -> Result<usize, OutlineError> {
    let p = i64::try_from(page).map_err(|_| OrganizeError::NoSuchPage(page))?;
    let next = p.checked_add(i64::from(delta)).ok_or(OrganizeError::NoSuchPage(page))?;
    let count = i64::try_from(page_count).unwrap_or(i64::MAX);
    if next < 0 || next >= count {
        return Err(OrganizeError::NoSuchPage(usize::try_from(next.max(0)).unwrap_or(page)).into());
    }
    Ok(usize::try_from(next).unwrap_or(page))
}

fn flush_node_dest(doc: &mut Document, r: ObjRef, node: &OutlineArenaNode, pages: &[ObjRef]) -> Result<(), OutlineError> {
    let Some(dest) = node.dest.as_ref() else {
        put(doc, r, b"Dest", None)?;
        return Ok(());
    };
    let arr = dest.to_cos_array(pages)?;
    if node.dest_is_goto_action {
        let mut action =
            doc.get(r).as_dict().and_then(|d| d.get(b"A")).map(|a| doc.resolve(a)).and_then(|a| a.as_dict().cloned()).unwrap_or_default();
        if action.get(b"S").is_none() {
            action.set(b"S".to_vec(), Object::name("GoTo"));
        }
        action.set(b"D".to_vec(), arr);
        put(doc, r, b"A", Some(Object::Dict(action)))?;
        put(doc, r, b"Dest", None)?;
    } else {
        put(doc, r, b"Dest", Some(arr))?;
        put(doc, r, b"A", None)?;
    }
    Ok(())
}

fn clamp_i32(v: i64) -> i32 {
    v.clamp(i64::from(i32::MIN), i64::from(i32::MAX)) as i32
}

fn load_node(
    doc: &Document,
    item_ref: ObjRef,
    d: &Dict,
    page_map: &HashMap<ObjRef, usize>,
    page_count: usize,
    names_cache: &mut Option<HashMap<Vec<u8>, Object>>,
) -> OutlineArenaNode {
    let title = title_of(doc, d);
    let raw_count = d.int(b"Count");
    let pdf_count = raw_count.map_or(0, clamp_i32);
    // ISO 32000-2 Table 151: strictly positive /Count opens expanded.
    let is_expanded = raw_count.is_some_and(|c| c > 0);
    let color_rgb = d.get(b"C").map(|c| doc.resolve(c)).and_then(|c| title_color(doc, &c));
    let flags = d.get(b"F").and_then(|f| doc.resolve(f).as_int()).unwrap_or(0);
    let style_flags = u8::try_from(flags.clamp(0, 255)).unwrap_or(0);

    let (dest, dest_is_goto_action) = load_node_dest(doc, d, page_map, page_count, names_cache);

    OutlineArenaNode {
        cos_obj: Some(item_ref),
        title,
        dest,
        is_expanded,
        style_flags,
        color_rgb,
        parent: None,
        prev_sibling: None,
        next_sibling: None,
        first_child: None,
        last_child: None,
        pdf_count,
        euler_in: 0,
        euler_out: 0,
        depth: 0,
        dirty: false,
        title_dirty: false,
        dest_dirty: false,
        dest_is_goto_action,
        style_dirty: false,
        had_count: raw_count.is_some(),
    }
}

fn load_node_dest(
    doc: &Document,
    d: &Dict,
    page_map: &HashMap<ObjRef, usize>,
    page_count: usize,
    names_cache: &mut Option<HashMap<Vec<u8>, Object>>,
) -> (Option<ViewDestination>, bool) {
    if let Some(raw_dest) = d.get(b"Dest")
        && let Some(arr) = resolve_explicit_array(doc, raw_dest, names_cache, 0)
        && let Some(view) = ViewDestination::from_cos_array(doc, page_map, page_count, &arr)
    {
        return (Some(view), false);
    }
    if let Some(action) = d.get(b"A").map(|a| doc.resolve(a))
        && let Some(ad) = action.as_dict()
        && ad.get(b"S").is_none_or(|s| doc.resolve(s).as_name() == Some(b"GoTo"))
        && let Some(raw_d) = ad.get(b"D")
        && let Some(arr) = resolve_explicit_array(doc, raw_d, names_cache, 0)
        && let Some(view) = ViewDestination::from_cos_array(doc, page_map, page_count, &arr)
    {
        return (Some(view), true);
    }
    (None, false)
}

fn resolve_explicit_array(doc: &Document, dest: &Object, names_cache: &mut Option<HashMap<Vec<u8>, Object>>, depth: u8) -> Option<Vec<Object>> {
    if depth > 8 {
        return None;
    }
    let resolved = doc.resolve(dest);
    match &*resolved {
        Object::Array(a) => Some(a.clone()),
        Object::Dict(d) => resolve_explicit_array(doc, d.get(b"D")?, names_cache, depth + 1),
        Object::String(s) => {
            let map = names_cache.get_or_insert_with(|| crate::outline::name_tree(doc));
            let inner = map.get(&s.bytes).cloned().or_else(|| crate::outline::dests_dict(doc, &s.bytes))?;
            resolve_explicit_array(doc, &inner, names_cache, depth + 1)
        }
        Object::Name(n) => {
            let map = names_cache.get_or_insert_with(|| crate::outline::name_tree(doc));
            let inner = map.get(n.as_slice()).cloned().or_else(|| crate::outline::dests_dict(doc, n))?;
            resolve_explicit_array(doc, &inner, names_cache, depth + 1)
        }
        _ => None,
    }
}
