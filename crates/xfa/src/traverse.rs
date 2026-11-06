//! Tab order of a laid-out form, from the template's `<traversal>` links.
//!
//! What Tab does, as measured black-box in another viewer with a synthetic form:
//! - Without links, a positioned container's children go in rows, top to bottom, then left to
//!   right; a flowed container's go in document order.
//! - A `next` link is followed. Entering a container goes to its `first` link, or else to its
//!   first child. An object that isn't a stop (a draw, or a hidden, invisible or protected
//!   field) passes Tab on by its own link or the default order.
//! - After a container's last child, Tab goes to the container's `next` link, or else on in
//!   the default order after the container.
//! - Read-only fields are stops; hidden, invisible and protected ones are not.
//! - A container that isn't shown is passed over whole, as if it weren't there: a link to it is
//!   ignored (Tab goes on in the default order from the object that had the link), and its own
//!   links are never followed.
//! - Once the chain ends, or comes back to an object it has passed, the stops it never reached
//!   follow in the default order, each going on by its own links.
//!
//! The order is applied by writing each page's widgets in it: the pages have no `/Tabs`, so
//! viewers tab in annotation order.

use std::collections::HashMap;

use crate::data::DataNode;
use crate::layout::{Item, Page, subform_instances};
use crate::model::{Access, Common, Field, Layout, Node, Presence, Subform, Template};

/// Most objects the order is worked out for; a larger form keeps the layout's order.
const MAX_NODES: usize = 200_000;
/// Deepest nesting followed, as in the layout.
const MAX_DEPTH: usize = 64;
/// Most `.`-separated parts a traversal reference may have.
const MAX_REF_PARTS: usize = 64;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
enum Class {
    Subform,
    Area,
    ExclGroup,
    Field,
    Draw,
}

impl Class {
    /// The class a `#class` reference names.
    fn named(name: &str) -> Option<Class> {
        Some(match name {
            "subform" | "subformSet" => Class::Subform,
            "area" => Class::Area,
            "exclGroup" => Class::ExclGroup,
            "field" => Class::Field,
            "draw" => Class::Draw,
            _ => return None,
        })
    }

    /// Subforms, areas and radio groups hold other objects.
    fn is_container(self) -> bool {
        matches!(self, Class::Subform | Class::Area | Class::ExclGroup)
    }
}

/// One object of the form (a repeating subform once per instance).
struct TNode<'t> {
    class: Class,
    name: Option<&'t str>,
    parent: Option<usize>,
    /// Children in document order (what SOM indices count).
    kids: Vec<usize>,
    /// Children in the default Tab order.
    tab: Vec<usize>,
    /// This object's place in its parent's `tab`.
    place: usize,
    /// Top, then left, in its parent's coordinates (positioned containers sort by it).
    at: (f64, f64),
    first: Option<&'t str>,
    next: Option<&'t str>,
    /// Shown, with every container above it: a container that isn't is passed over whole.
    shown: bool,
    /// The laid-out widget (page, item) a field stops at.
    stop: Option<(usize, usize)>,
}

/// A container's children as SOM paths see them: unnamed containers are see-through.
#[derive(Default)]
struct SomIndex<'t> {
    by_name: HashMap<&'t str, Vec<usize>>,
    by_class: HashMap<Class, Vec<usize>>,
}

struct Tree<'t> {
    nodes: Vec<TNode<'t>>,
    /// The laid-out widgets by SOM path, in layout order; each is claimed once.
    widgets: HashMap<String, Vec<(usize, usize)>>,
    /// The template and data the layout counted repeating subforms' instances with.
    tpl: &'t Template,
    data: Option<&'t DataNode>,
    /// SOM indices of the containers references were resolved in.
    som: HashMap<usize, SomIndex<'t>>,
    too_big: bool,
}

/// `parent.name[index]`, or `parent` for an unnamed object (which SOM paths skip), as the
/// layout writes them.
fn with_som(parent: &str, name: Option<&str>, index: usize) -> String {
    match name {
        Some(n) if parent.is_empty() => format!("{n}[{index}]"),
        Some(n) => format!("{parent}.{n}[{index}]"),
        None => parent.to_string(),
    }
}

/// The SOM index of each of `names` among the same-named ones before it (0 when unnamed).
fn som_indices<'a>(names: impl Iterator<Item = Option<&'a str>>) -> Vec<usize> {
    let mut seen: HashMap<&str, usize> = HashMap::new();
    names
        .map(|n| {
            n.map_or(0, |n| {
                let c = seen.entry(n).or_insert(0);
                *c += 1;
                *c - 1
            })
        })
        .collect()
}

impl<'t> Tree<'t> {
    /// An empty tree knowing the widgets laid out on `pages`, from `tpl` with `data`.
    fn new(tpl: &'t Template, data: Option<&'t DataNode>, pages: &[Page]) -> Tree<'t> {
        let mut widgets: HashMap<String, Vec<(usize, usize)>> = HashMap::new();
        for (pi, page) in pages.iter().enumerate() {
            for (ii, item) in page.items.iter().enumerate() {
                if let Item::Widget(w) = item {
                    widgets.entry(w.som.clone()).or_default().push((pi, ii));
                }
            }
        }
        // Claimed from the front in layout order.
        for list in widgets.values_mut() {
            list.reverse();
        }
        Tree { nodes: Vec::new(), widgets, tpl, data, som: HashMap::new(), too_big: false }
    }

    /// Add one object under `parent`; `None` once the tree is full.
    fn push(&mut self, class: Class, c: &'t Common, parent: Option<usize>, shown: bool) -> Option<usize> {
        if self.nodes.len() >= MAX_NODES {
            self.too_big = true;
            return None;
        }
        let (x, y) = c.top_left(c.w.unwrap_or(0.0), c.h.unwrap_or(0.0));
        let id = self.nodes.len();
        self.nodes.push(TNode {
            class,
            name: c.name.as_deref(),
            parent,
            kids: Vec::new(),
            tab: Vec::new(),
            place: 0,
            at: (y, x),
            first: c.traversal.first.as_deref(),
            next: c.traversal.next.as_deref(),
            shown,
            stop: None,
        });
        if let Some(p) = parent.and_then(|p| self.nodes.get_mut(p)) {
            p.kids.push(id);
        }
        Some(id)
    }

    /// How many instances of `sf` under the container at `parent_som` the layout made, counted
    /// as it counts them (one for a subform that isn't shown, which it doesn't lay out).
    fn instances(&self, sf: &Subform, parent_som: &str, shown: bool) -> usize {
        if shown { subform_instances(self.tpl, self.data, sf, parent_som) } else { 1 }
    }

    /// Add `node` (each instance of a repeating subform) and everything in it, under `parent`
    /// whose SOM path is `parent_som`; `sib` is the node's SOM index there, as the layout counts.
    fn add(&mut self, node: &'t Node, parent: Option<usize>, parent_som: &str, sib: usize, shown: bool, depth: usize) {
        if depth > MAX_DEPTH || self.too_big {
            return;
        }
        let c = node.common();
        let shown = shown && c.presence == Presence::Visible;
        match node {
            Node::Subform(sf) => {
                for k in 0..self.instances(sf, parent_som, shown) {
                    let som = with_som(parent_som, c.name.as_deref(), sib + k);
                    let Some(id) = self.push(Class::Subform, c, parent, shown) else { return };
                    let sibs = som_indices(sf.children.iter().map(|k| k.common().name.as_deref()));
                    for (i, kid) in sf.children.iter().enumerate() {
                        self.add(kid, Some(id), &som, sibs.get(i).copied().unwrap_or(0), shown, depth + 1);
                    }
                    self.order(id, sf.layout == Layout::Positioned);
                }
            }
            Node::Area(a) => {
                let som = with_som(parent_som, c.name.as_deref(), sib);
                let Some(id) = self.push(Class::Area, c, parent, shown) else { return };
                let sibs = som_indices(a.children.iter().map(|k| k.common().name.as_deref()));
                for (i, kid) in a.children.iter().enumerate() {
                    self.add(kid, Some(id), &som, sibs.get(i).copied().unwrap_or(0), shown, depth + 1);
                }
                self.order(id, true);
            }
            Node::ExclGroup(g) => {
                let som = with_som(parent_som, c.name.as_deref(), sib);
                let Some(id) = self.push(Class::ExclGroup, c, parent, shown) else { return };
                let sibs = som_indices(g.fields.iter().map(|f| f.common.name.as_deref()));
                for (i, f) in g.fields.iter().enumerate() {
                    self.add_field(f, id, &som, sibs.get(i).copied().unwrap_or(0), shown);
                }
                self.order(id, g.layout == Layout::Positioned);
            }
            Node::Field(f) => {
                if let Some(p) = parent {
                    self.add_field(f, p, parent_som, sib, shown);
                }
            }
            Node::Draw(_) => {
                self.push(Class::Draw, c, parent, shown);
            }
        }
    }

    /// Add field `f` and claim the widget the layout made for it, a stop unless it is protected.
    fn add_field(&mut self, f: &'t Field, parent: usize, parent_som: &str, sib: usize, shown: bool) {
        let shown = shown && f.common.presence == Presence::Visible;
        let Some(id) = self.push(Class::Field, &f.common, Some(parent), shown) else { return };
        if !shown {
            return;
        }
        // The layout names an unnamed field `field`.
        let som = with_som(parent_som, Some(f.common.name.as_deref().unwrap_or("field")), sib);
        let widget = self.widgets.get_mut(&som).and_then(Vec::pop);
        if f.access != Access::Protected
            && let Some(n) = self.nodes.get_mut(id)
        {
            n.stop = widget;
        }
    }

    /// Put `id`'s children in the default Tab order: rows then columns in a positioned
    /// container, document order in a flowed one.
    fn order(&mut self, id: usize, positioned: bool) {
        let Some(kids) = self.nodes.get(id).map(|n| n.kids.clone()) else { return };
        let mut tab = kids;
        if positioned {
            let at = |k: &usize| self.nodes.get(*k).map_or((0.0, 0.0), |n| n.at);
            tab.sort_by(|a, b| {
                let (a, b) = (at(a), at(b));
                a.0.total_cmp(&b.0).then(a.1.total_cmp(&b.1))
            });
        }
        for (place, k) in tab.iter().enumerate() {
            if let Some(n) = self.nodes.get_mut(*k) {
                n.place = place;
            }
        }
        if let Some(n) = self.nodes.get_mut(id) {
            n.tab = tab;
        }
    }

    /// `id`'s children as SOM paths see them, built on first use.
    fn som_index(&mut self, id: usize) -> &SomIndex<'t> {
        if !self.som.contains_key(&id) {
            let mut index = SomIndex::default();
            let mut stack: Vec<usize> = self.nodes.get(id).map(|n| n.kids.iter().rev().copied().collect()).unwrap_or_default();
            while let Some(k) = stack.pop() {
                let Some(n) = self.nodes.get(k) else { continue };
                if n.name.is_none() && n.class.is_container() {
                    stack.extend(n.kids.iter().rev().copied());
                    continue;
                }
                if let Some(name) = n.name {
                    index.by_name.entry(name).or_default().push(k);
                }
                index.by_class.entry(n.class).or_default().push(k);
            }
            self.som.insert(id, index);
        }
        // Inserted just above when missing.
        self.som.entry(id).or_default()
    }

    /// The child of `id` that one part of a SOM reference (`name[2]`, `#draw[0]`, `name`) names.
    fn pick(&mut self, id: usize, part: &str) -> Option<usize> {
        let (head, index) = match part.find('[') {
            Some(i) => (part.get(..i)?, part.get(i + 1..)?.strip_suffix(']')?.trim().parse::<usize>().ok()?),
            None => (part, 0),
        };
        let index_of = self.som_index(id);
        let list = match head.strip_prefix('#') {
            Some(class) => index_of.by_class.get(&Class::named(class)?),
            None => index_of.by_name.get(head),
        };
        list.and_then(|l| l.get(index)).copied()
    }

    /// The object `reference` names, from `id`: its first part is looked for among `id`'s
    /// children, then each container's above it; the rest go down from there.
    fn resolve(&mut self, id: usize, reference: &str) -> Option<usize> {
        let parts: Vec<&str> = reference.split('.').map(str::trim).collect();
        if parts.len() > MAX_REF_PARTS {
            return None;
        }
        let (first, rest) = parts.split_first()?;
        let mut scope = Some(id);
        while let Some(s) = scope {
            if let Some(hit) = self.pick(s, first) {
                return rest.iter().try_fold(hit, |at, part| self.pick(at, part));
            }
            scope = self.nodes.get(s)?.parent;
        }
        None
    }

    /// Whether Tab passes over `id` whole, as if it weren't there: a container not shown.
    fn skipped(&self, id: usize) -> bool {
        self.nodes.get(id).is_some_and(|n| n.class.is_container() && !n.shown)
    }

    /// The first of `id`'s children from place `from` on, in the default order, that Tab
    /// doesn't pass over.
    fn kid_from(&self, id: usize, from: usize) -> Option<usize> {
        self.nodes.get(id)?.tab.get(from..)?.iter().copied().find(|k| !self.skipped(*k))
    }

    /// Where a link from `id` goes: nowhere when it names nothing or something Tab passes over.
    fn target(&mut self, id: usize, reference: &str) -> Option<usize> {
        self.resolve(id, reference).filter(|t| !self.skipped(*t))
    }

    /// Where Tab goes after `id` (after everything inside it, for a container): its `next`
    /// link, else its next sibling in the default order, else on after its parent.
    fn after(&mut self, mut id: usize) -> Option<usize> {
        for _ in 0..=MAX_DEPTH + 1 {
            let node = self.nodes.get(id)?;
            let (next, parent, place) = (node.next, node.parent, node.place);
            if let Some(t) = next.and_then(|r| self.target(id, r)) {
                return Some(t);
            }
            let p = parent?;
            if let Some(s) = self.kid_from(p, place.saturating_add(1)) {
                return Some(s);
            }
            id = p;
        }
        None
    }

    /// Follow Tab from `start` until the chain ends or comes back, adding the stops it reaches.
    fn chain(&mut self, start: usize, passed: &mut [bool], out: &mut Vec<(usize, usize)>) {
        let mut at = Some(start);
        let mut steps = 0usize;
        let limit = self.nodes.len().saturating_mul(MAX_DEPTH + 2).saturating_add(16);
        while let Some(id) = at {
            steps += 1;
            match passed.get_mut(id) {
                Some(p) if !*p && steps <= limit => *p = true,
                _ => return,
            }
            let Some(node) = self.nodes.get(id) else { return };
            let (class, shown, first, stop) = (node.class, node.shown, node.first, node.stop);
            let entry = self.kid_from(id, 0);
            at = if class.is_container() && shown {
                match first.and_then(|r| self.target(id, r)) {
                    Some(t) => Some(t),
                    None => entry.or_else(|| self.after(id)),
                }
            } else {
                if let Some(w) = stop {
                    out.push(w);
                }
                self.after(id)
            };
        }
    }

    /// The stops in the default order: shown containers' children in their Tab order.
    fn default_stops(&self, root: usize) -> Vec<usize> {
        let mut out = Vec::new();
        let mut stack = vec![root];
        while let Some(id) = stack.pop() {
            let Some(n) = self.nodes.get(id) else { continue };
            if n.stop.is_some() {
                out.push(id);
            }
            if n.class.is_container() && n.shown {
                stack.extend(n.tab.iter().rev().copied());
            }
        }
        out
    }
}

/// Write each page's widgets in the template's Tab order. Widgets the order doesn't place
/// (protected fields, master pages, a header row repeated after a page break) keep their
/// places; so does everything when the form is too large to work out.
pub(crate) fn order_widgets(root: &Node, tpl: &Template, data: Option<&DataNode>, pages: &mut [Page]) {
    let mut tree = Tree::new(tpl, data, pages);
    tree.add(root, None, "", 0, true, 0);
    if tree.too_big || tree.nodes.is_empty() {
        return;
    }
    let mut passed = vec![false; tree.nodes.len()];
    let mut order: Vec<(usize, usize)> = Vec::new();
    tree.chain(0, &mut passed, &mut order);
    for s in tree.default_stops(0) {
        if !passed.get(s).copied().unwrap_or(true) {
            tree.chain(s, &mut passed, &mut order);
        }
    }
    let rank: HashMap<(usize, usize), usize> = order.iter().enumerate().map(|(r, w)| (*w, r)).collect();
    for (pi, page) in pages.iter_mut().enumerate() {
        let slots: Vec<usize> = (0..page.items.len()).filter(|i| rank.contains_key(&(pi, *i))).collect();
        let mut ranked: Vec<(usize, usize)> = slots.iter().map(|i| (rank.get(&(pi, *i)).copied().unwrap_or(usize::MAX), *i)).collect();
        ranked.sort_unstable();
        // Slot j takes the widget ranked j-th on the page; everything else stays put.
        let mut perm: Vec<usize> = (0..page.items.len()).collect();
        for (slot, (_, from)) in slots.iter().zip(&ranked) {
            if let Some(p) = perm.get_mut(*slot) {
                *p = *from;
            }
        }
        let mut check = perm.clone();
        check.sort_unstable();
        if !check.iter().enumerate().all(|(i, v)| i == *v) {
            continue;
        }
        let mut items: Vec<Option<Item>> = std::mem::take(&mut page.items).into_iter().map(Some).collect();
        page.items = perm.iter().filter_map(|i| items.get_mut(*i).and_then(Option::take)).collect();
    }
}
