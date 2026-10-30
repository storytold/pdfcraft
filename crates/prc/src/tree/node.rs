//! The assembly tree as a list: what a model-tree panel shows.

use std::ops::Range;

/// Where a [`ModelNode`]'s name came from. PRC does not say which name a
/// viewer should show; this reader takes the first present of these, in order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum NameSource {
    /// The occurrence's own name.
    Occurrence,
    /// The name of an occurrence in its prototype chain.
    Prototype,
    /// The name of the part definition it draws.
    Part,
    /// No name anywhere: [`ModelNode::name`] is `None`.
    Unnamed,
}

/// One product occurrence of a PRC model's assembly tree, as
/// [`PrcFile::model_tree`](crate::PrcFile::model_tree) lists it: depth
/// first, a parent before its children, children in file order.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct ModelNode {
    /// The name to show, by [`Self::name_from`]; `None` when unnamed.
    pub name: Option<String>,
    /// Where [`Self::name`] came from.
    pub name_from: NameSource,
    /// Index of the parent node in the list; `None` for a root.
    pub parent: Option<usize>,
    /// 0 for a root.
    pub depth: usize,
    /// Index into [`crate::PrcFile::file_structures`] of the structure
    /// holding this occurrence.
    pub file_structure: usize,
    /// The occurrence's 0-based index in that structure's tree.
    pub occurrence: usize,
    /// The occurrence's own graphics hide it (Show clear, or Removed)
    /// [WD 7.2.4.2]: the visibility the file stores, which a viewer opens on.
    pub hidden: bool,
    /// The occurrence is suppressed (`product_behavior`) [WD 7.3.10].
    pub suppressed: bool,
    /// Whether anything of this node is drawn: false when it or an
    /// ancestor is hidden or suppressed.
    pub drawn: bool,
    /// Whether the occurrence (or its prototype) carries a part definition,
    /// i.e. geometry of its own rather than only children.
    pub has_part: bool,
    /// The placements this node and its descendants draw: indices into
    /// [`PrcFile::placements`](crate::PrcFile::placements) (or
    /// [`placements_with`](crate::PrcFile::placements_with), which yields the same order):
    /// the meshes attributed to this occurrence's part and its descendants' parts. Empty when
    /// nothing under it is drawn.
    pub placements: Range<usize>,
}

impl ModelNode {
    /// The label the node is listed under: its name, or `occurrence F:I`
    /// (file structure, occurrence) when it has none.
    #[must_use]
    pub fn label(&self) -> String {
        match &self.name {
            Some(name) => name.clone(),
            None => format!("occurrence {}:{}", self.file_structure, self.occurrence),
        }
    }
}
