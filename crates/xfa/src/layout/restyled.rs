//! An instance's own reading of its template object, made once per layout.
//!
//! What a script changed on one instance of a repeated object ([`super::PropChanges`]) is
//! applied where the layout comes to that instance: the object is copied, its properties are
//! written out with the changes and read again ([`crate::restyle::restyle`]). The layout comes
//! to an instance more than once (a container measures what it holds before it draws it, a
//! flow measures again where a page splits, a field split over pages asks for its border), so
//! each reading made is kept here for the layout, and handed back when the same object is
//! asked for with the same changes.
//!
//! The same object is one equal to the one the reading was made from, not one at its address
//! or with its property tree: the layout also reads copies it makes (a group's buttons, a
//! button its group makes required), which share a tree and may differ. The tree's address and
//! a hash of the changes only narrow the search; a kept reading holds the tree, so its address
//! is no other tree's while it is kept. What is kept is bounded ([`MAX_KEPT`],
//! [`MAX_KEPT_BYTES`]): past either, a reading is made each time it is asked for.

use std::cell::{Cell, RefCell};
use std::collections::{BTreeMap, HashMap};
use std::rc::Rc;
use std::sync::Arc;

use super::Layouter;
use crate::memo::{field_bytes, set_bytes, sets_hash, tree_bytes, value_bytes};
use crate::model::{Common, Draw, ExclGroup, Field, Node, Occur, Props, Subform};

/// The property changes of one instance, by property key.
type Sets = BTreeMap<String, String>;

/// Most readings a layout keeps. A form keeps at most [`crate::restyle::MAX_PROP_OVERRIDES`]
/// changes, so at most that many instances have any, and an instance has up to four readings:
/// a field's own and its border where a page splits it; a container's properties as measured
/// and as drawn, its occurrence and its breaks.
const MAX_KEPT: usize = 4 * crate::restyle::MAX_PROP_OVERRIDES;

/// Most bytes the kept readings hold together, counted as the template's memo counts what it
/// keeps and bounded as one generation of it is.
const MAX_KEPT_BYTES: usize = crate::memo::MAX_APPLIED_BYTES;

/// Most readings kept for one property tree and one set of changes: the copies the layout
/// makes of one object are few (see the module documentation).
const MAX_ALIKE: usize = 4;

/// One reading: what it was made from, the changes, and the reading.
struct Entry<B, T> {
    base: B,
    sets: Sets,
    made: T,
}

/// Readings by the address of the property tree of what they were made from and a hash of the
/// changes.
type Kept<B, T> = HashMap<(usize, u64), Vec<Entry<B, T>>>;

/// Whether an instance takes the template's break before and after it, and the subform's own
/// breaks before and after: what [`crate::restyle::instance_breaks`] reads besides the tree.
type Sides = [bool; 4];

/// Where the page breaks before and after an instance by its changes, each side `None` when
/// no change is to it ([`crate::restyle::instance_breaks`]).
type Breaks = (Option<bool>, Option<bool>);

/// The readings one layout has made (see the module documentation).
#[derive(Default)]
pub(super) struct Restyles {
    leaves: RefCell<Kept<Node, Rc<Node>>>,
    commons: RefCell<Kept<Common, Rc<Common>>>,
    occurs: RefCell<Kept<(Arc<Props>, Occur), Occur>>,
    breaks: RefCell<Kept<(Arc<Props>, Sides), Breaks>>,
    /// How many readings are kept, and the bytes they hold.
    count: Cell<usize>,
    bytes: Cell<usize>,
}

/// What a reading is made from: as asked about (borrowed), and as kept beside the reading.
trait Base {
    type Kept;
    /// The property tree of the object read.
    fn props(&self) -> &Arc<Props>;
    /// Whether `kept` is equal to this, so that a reading made from it is this one's.
    fn same(&self, kept: &Self::Kept) -> bool;
    fn keep(&self) -> Self::Kept;
}

/// A field, draw or exclusion group: the template's, or a copy the layout made of one.
#[derive(Clone, Copy)]
pub(super) enum Leaf<'a> {
    Field(&'a Field),
    Draw(&'a Draw),
    Group(&'a ExclGroup),
}

impl Leaf<'_> {
    /// The object as a node of its own.
    fn node(&self) -> Node {
        match *self {
            Leaf::Field(f) => Node::Field(Box::new(f.clone())),
            Leaf::Draw(d) => Node::Draw(Box::new(d.clone())),
            Leaf::Group(g) => Node::ExclGroup(Box::new(g.clone())),
        }
    }
}

impl Base for Leaf<'_> {
    type Kept = Node;

    fn props(&self) -> &Arc<Props> {
        match self {
            Leaf::Field(f) => &f.common.props,
            Leaf::Draw(d) => &d.common.props,
            Leaf::Group(g) => &g.common.props,
        }
    }

    fn same(&self, kept: &Node) -> bool {
        match (*self, kept) {
            (Leaf::Field(f), Node::Field(k)) => *f == **k,
            (Leaf::Draw(d), Node::Draw(k)) => *d == **k,
            (Leaf::Group(g), Node::ExclGroup(k)) => *g == **k,
            _ => false,
        }
    }

    fn keep(&self) -> Node {
        self.node()
    }
}

impl Base for &Common {
    type Kept = Common;

    fn props(&self) -> &Arc<Props> {
        &self.props
    }

    fn same(&self, kept: &Common) -> bool {
        **self == *kept
    }

    fn keep(&self) -> Common {
        (*self).clone()
    }
}

/// A subform's property tree, and what else of the subform its reading is made from.
impl<X: Copy + PartialEq> Base for (&Arc<Props>, X) {
    type Kept = (Arc<Props>, X);

    fn props(&self) -> &Arc<Props> {
        self.0
    }

    fn same(&self, kept: &Self::Kept) -> bool {
        *self.0 == kept.0 && self.1 == kept.1
    }

    fn keep(&self) -> Self::Kept {
        (Arc::clone(self.0), self.1)
    }
}

impl Restyles {
    /// What `make` gives for `base` and the changes `sets`: the reading `kept` holds for an
    /// equal base and equal changes, else made now and kept there while there is room. `heap`
    /// is the bytes a reading holds beside its entry.
    fn reading<B: Base, T: Clone>(
        &self,
        kept: &RefCell<Kept<B::Kept, T>>,
        base: B,
        sets: &Sets,
        heap: impl Fn(&T) -> usize,
        make: impl FnOnce() -> T,
    ) -> T {
        let key = (Arc::as_ptr(base.props()) as usize, sets_hash(sets));
        if let Ok(k) = kept.try_borrow()
            && let Some(e) = k.get(&key).and_then(|alike| alike.iter().find(|e| e.sets == *sets && base.same(&e.base)))
        {
            return e.made.clone();
        }
        // Nothing is borrowed while the reading is made.
        let made = make();
        let cost = std::mem::size_of::<Entry<B::Kept, T>>().saturating_add(set_bytes(sets)).saturating_add(heap(&made));
        let (count, bytes) = (self.count.get(), self.bytes.get().saturating_add(cost));
        if count >= MAX_KEPT || bytes > MAX_KEPT_BYTES {
            return made;
        }
        let Ok(mut k) = kept.try_borrow_mut() else { return made };
        let alike = k.entry(key).or_default();
        if alike.len() >= MAX_ALIKE {
            return made;
        }
        alike.push(Entry { base: base.keep(), sets: sets.clone(), made: made.clone() });
        self.count.set(count.saturating_add(1));
        self.bytes.set(bytes);
        made
    }
}

/// Bytes a field, draw or exclusion group holds, roughly (see [`field_bytes`]).
fn leaf_bytes(node: &Node) -> usize {
    match node {
        Node::Field(f) => field_bytes(f),
        Node::Draw(d) => std::mem::size_of::<Draw>().saturating_add(tree_bytes(&d.common.props)).saturating_add(value_bytes(&d.value)),
        Node::ExclGroup(g) => {
            let own = std::mem::size_of::<ExclGroup>().saturating_add(tree_bytes(&g.common.props));
            g.fields.iter().fold(own, |n, f| n.saturating_add(field_bytes(f)))
        }
        // Not read here: a container's own properties are ([`Layouter::restyled_common`]).
        Node::Subform(_) | Node::Area(_) => std::mem::size_of::<Node>(),
    }
}

/// An object's own properties for one instance: the template's, or the reading of them with
/// the instance's changes.
pub(super) enum Own<'a> {
    Template(&'a Common),
    Changed(Rc<Common>),
}

impl std::ops::Deref for Own<'_> {
    type Target = Common;

    fn deref(&self) -> &Common {
        match self {
            Own::Template(c) => c,
            Own::Changed(c) => c,
        }
    }
}

/// How often subform `node` occurs as the occurrence changes `sets` leave it, `None` when it is
/// no subform.
pub(super) fn occur_with(node: &Node, sets: &Sets) -> Option<Occur> {
    match crate::restyle::restyled(node, sets) {
        Node::Subform(s) => Some(s.occur),
        _ => None,
    }
}

impl Layouter<'_> {
    /// `leaf` with its instance's changes `sets` applied ([`crate::restyle::restyle`]), read
    /// once in this layout for equal objects with equal changes (see the module documentation).
    /// The reading is of the kind `leaf` is.
    pub(super) fn restyled(&self, leaf: Leaf<'_>, sets: &Sets) -> Rc<Node> {
        // The object it was made from is kept beside it.
        let heap = |n: &Rc<Node>| leaf_bytes(n).saturating_mul(2);
        self.restyles.reading(&self.restyles.leaves, leaf, sets, heap, || {
            let mut node = leaf.node();
            crate::restyle::restyle(&mut node, sets);
            Rc::new(node)
        })
    }

    /// The common properties `c` with an instance's changes `sets` applied
    /// ([`crate::restyle::restyled_common`]), read once in this layout.
    pub(super) fn restyled_common(&self, c: &Common, sets: &Sets) -> Rc<Common> {
        let heap = |c: &Rc<Common>| std::mem::size_of::<Common>().saturating_add(tree_bytes(&c.props)).saturating_mul(2);
        self.restyles.reading(&self.restyles.commons, c, sets, heap, || Rc::new(crate::restyle::restyled_common(c, sets)))
    }

    /// How often subform `sf` (the node `node`) occurs as the occurrence changes `sets` leave
    /// it ([`occur_with`]), read once in this layout. The reading is made from the subform's
    /// property tree, and where that can't be read again is the subform's own.
    pub(super) fn restyled_occur(&self, node: &Node, sf: &Subform, sets: &Sets) -> Occur {
        let base = (&sf.common.props, sf.occur);
        self.restyles.reading(&self.restyles.occurs, base, sets, |_| 0, || occur_with(node, sets).unwrap_or(sf.occur))
    }

    /// Where the page breaks before and after one instance of `node` by the break changes
    /// `sets` made to it ([`crate::restyle::instance_breaks`], `template` being whether the
    /// instance takes the template's break before and after), read once in this layout.
    pub(super) fn restyled_breaks(&self, node: &Node, sets: &Sets, template: (bool, bool)) -> Breaks {
        // Only a subform breaks the page by its changes.
        let Node::Subform(sf) = node else { return (None, None) };
        let base = (&sf.common.props, [template.0, template.1, sf.break_before_page, sf.break_after_page]);
        self.restyles.reading(&self.restyles.breaks, base, sets, |_| 0, || crate::restyle::instance_breaks(node, sets, template.0, template.1))
    }
}
