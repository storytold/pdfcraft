//! SOM numbering: the number each template object takes among the same-named objects of its
//! scope (`Name[n]`), worked out once, when the template is read.
//!
//! XFA 3.3 ch. 3, "Transparent Nodes" (pp. 94-95): an unnamed subform "is invisible to the
//! normal SOM syntax… It is as though the nameless subform was removed and its children adopted
//! by the nameless subform's parent", so Example 3.12's two `Description` fields, each in an
//! unnamed subform of `Receipt`, are `Receipt.Description[0]` and `Receipt.Description[1]`. A
//! scope is therefore a named container (the root, a page area, a named subform or area, an
//! exclusion group) with everything in it down to the next named container, and same-named
//! objects count up through it in document order.
//!
//! What the numbers leave as it was:
//! - A repeating *named* subform's later instances count on from its number (`Row[0]`, `Row[1]`,
//!   …) when the form is built; an object of the same name after it in the scope keeps the
//!   number the template gives it.
//! - An unnamed subform the template makes more than once (`occur initial`; data makes none of
//!   an unnamed one): what follows it is numbered past all of its instances, and each instance
//!   after the first names its content as the first does.
//! - An unnamed object is number 0 (a field the form calls `field[0]`, a group `group[0]`).

use std::collections::HashMap;

use crate::model::{Node, Subform};

/// Deeper than the parser keeps anything (it reads 64 levels), so every object is numbered.
const MAX_DEPTH: usize = 256;

/// The largest number an object is given. `occur` counts multiply up through nested unnamed
/// subforms, so a hostile template reaches any number with a few lines; no form has an object
/// numbered near this, and adding an instance count to it
/// ([`crate::layout::MAX_INSTANCES`]) can't overflow, on a 32-bit target either.
pub(crate) const MAX_SOM_INDEX: usize = 1 << 30;

/// The same-named objects counted so far in one scope. A layer over `outer` counts what one
/// instance of an unnamed subform adds, so the instances after it can be counted in too.
#[derive(Default)]
struct Scope<'o> {
    outer: Option<&'o Scope<'o>>,
    seen: HashMap<String, usize>,
}

impl Scope<'_> {
    /// How many objects named `name` came before, here and in the layers under this one, up to
    /// [`MAX_SOM_INDEX`].
    fn count(&self, name: &str) -> usize {
        let mut total = 0usize;
        let mut layer = Some(self);
        while let Some(l) = layer {
            total = total.saturating_add(l.seen.get(name).copied().unwrap_or(0));
            layer = l.outer;
        }
        total.min(MAX_SOM_INDEX)
    }

    /// Count `more` further objects named `name`.
    fn add(&mut self, name: &str, more: usize) {
        match self.seen.get_mut(name) {
            Some(n) => *n = n.saturating_add(more),
            None => {
                self.seen.insert(name.to_string(), more);
            }
        }
    }

    /// The number the next object named `name` takes.
    fn next(&mut self, name: &str) -> usize {
        let at = self.count(name);
        self.add(name, 1);
        at
    }
}

/// Number every object of the template under `root` ([`crate::model::Common::som_index`]): the
/// root's own content and each page area's, a scope each.
pub(crate) fn number(root: &mut Subform) {
    root.common.som_index = 0;
    walk(&mut root.children, &mut Scope::default(), 1);
    page_areas(root, 1);
}

/// The content of each page area of `sf`'s page set, numbered as a scope of its own.
fn page_areas(sf: &mut Subform, depth: usize) {
    for area in sf.page_set.iter_mut().flat_map(|ps| ps.areas.iter_mut()) {
        walk(&mut area.items, &mut Scope::default(), depth.saturating_add(1));
    }
}

fn walk(nodes: &mut [Node], scope: &mut Scope<'_>, depth: usize) {
    if depth > MAX_DEPTH {
        return;
    }
    for n in nodes {
        let index = n.common().name.as_deref().map_or(0, |name| scope.next(name));
        n.common_mut().som_index = index;
        let named = n.common().name.is_some();
        match n {
            Node::Subform(sf) => {
                page_areas(sf, depth);
                if named {
                    walk(&mut sf.children, &mut Scope::default(), depth + 1);
                    continue;
                }
                // As many as the template makes: data makes no instance of an unnamed subform.
                let instances = crate::layout::instance_count(&sf.occur, 0);
                if instances < 2 {
                    walk(&mut sf.children, scope, depth + 1);
                    continue;
                }
                let mut one = Scope { outer: Some(&*scope), seen: HashMap::new() };
                walk(&mut sf.children, &mut one, depth + 1);
                for (name, n) in one.seen {
                    scope.add(&name, n.saturating_mul(instances));
                }
            }
            Node::Area(a) if named => walk(&mut a.children, &mut Scope::default(), depth + 1),
            Node::Area(a) => walk(&mut a.children, scope, depth + 1),
            // Its buttons are named in the group, which has a path segment even when unnamed.
            Node::ExclGroup(g) => {
                let mut own = Scope::default();
                for f in &mut g.fields {
                    f.common.som_index = f.common.name.as_deref().map_or(0, |name| own.next(name));
                }
            }
            Node::Field(_) | Node::Draw(_) => {}
        }
    }
}
