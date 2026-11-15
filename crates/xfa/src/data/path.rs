use std::collections::HashMap;

use super::{DataNode, DataPath, Field, MAX_DEPTH, MAX_NODES, Node, SomIndices, Template, XFA_DATA_NS};
use crate::model::{Common, Subform};

fn to_node(n: crate::xml::Node, depth: usize) -> DataNode {
    let name = n.tag_name().name().to_string();
    let children: Vec<DataNode> =
        if depth < MAX_DEPTH { n.children().filter(|c| c.is_element()).map(|c| to_node(c, depth + 1)).collect() } else { Vec::new() };
    // The text of a leaf is its value (its text nodes: a comment is not part of it); a
    // container's own text is whitespace.
    let text = if children.is_empty() { n.children().filter(|c| c.is_text()).filter_map(|c| c.text()).collect::<String>() } else { String::new() };
    let group = n.attribute((XFA_DATA_NS, "dataNode")) == Some("dataGroup");
    DataNode { name, text, children, group }
}

/// The `xfa:data` element of the datasets packet in `xdp` (or a bare datasets document), as a
/// tree whose root is that element.
pub fn parse_datasets(xdp: &str) -> Option<DataNode> {
    if crate::xml_nesting_depth(xdp) > crate::MAX_XML_DEPTH {
        return None;
    }
    let opts = crate::xml::ParsingOptions { allow_dtd: false, nodes_limit: MAX_NODES };
    let doc = crate::xml::Document::parse_with_options(xdp, opts).ok()?;
    let datasets = doc.descendants().find(|n| n.is_element() && n.tag_name().name() == "datasets")?;
    let data = datasets.children().find(|n| n.is_element() && n.tag_name().name() == "data")?;
    Some(to_node(data, 0))
}

/// The `xfa:data` element of a datasets packet as the datasets writer reads it before changing it:
/// a packet of an array may use the `xfa` and `xdp` prefixes its XDP envelope declares, so it is
/// read inside an element declaring them when it doesn't parse alone, and the datasets element in
/// the XFA data namespace is preferred over another with the same local name. Reading the packet
/// any other way could miss the values the writer put in it.
pub(super) fn parse_datasets_packet(text: &str) -> Option<DataNode> {
    if crate::xml_nesting_depth(text) > crate::MAX_XML_DEPTH {
        return None;
    }
    let opts = || crate::xml::ParsingOptions { allow_dtd: false, nodes_limit: MAX_NODES };
    let wrapped;
    let doc = match crate::xml::Document::parse_with_options(text, opts()) {
        Ok(d) => d,
        Err(_) => {
            wrapped = format!("{}{text}{}", super::merge::WRAP_OPEN, super::merge::WRAP_CLOSE);
            crate::xml::Document::parse_with_options(&wrapped, opts()).ok()?
        }
    };
    let is = |n: &crate::xml::Node, name: &str| n.is_element() && n.tag_name().name() == name;
    let datasets = doc
        .descendants()
        .find(|n| is(n, "datasets") && n.tag_name().namespace() == Some(super::XFA_DATA_NS))
        .or_else(|| doc.descendants().find(|n| is(n, "datasets")))?;
    let data = datasets.children().find(|n| is(n, "data"))?;
    Some(to_node(data, 0))
}

/// The data path a SOM expression names: `form[0].#subform[0].name[1]` → `[(form, 0), (name, 1)]`.
/// Unnamed containers (`#subform`) have no data node.
pub fn som_to_path(som: &str) -> DataPath {
    som.split('.')
        .filter_map(|seg| {
            let seg = seg.trim();
            if seg.is_empty() || seg.starts_with('#') || seg.starts_with('$') {
                return None;
            }
            let (name, idx) = match seg.split_once('[') {
                Some((n, rest)) => (n, rest.trim_end_matches(']').parse::<usize>().unwrap_or(0)),
                None => (seg, 0),
            };
            Some((name.to_string(), idx.min(100_000)))
        })
        .collect()
}

/// The data path the object at `som` binds to: [`som_to_path`] without the containers on the way
/// that are no data scope. An area, and a subform with `<bind match="none"/>`, are named in
/// SOM expressions but bind to no data node: data binding behaves "as though the transparent
/// nodes were removed and their children adopted by the transparent node's parent" (XFA 3.3,
/// "Transparent Nodes", pp. 193–194), so what they hold binds under their parent, as Adobe's
/// viewers write it. For the same reason a segment's data index counts only the same-named
/// siblings that bind. Each segment steps into the object its SOM index names ([`named_at`]),
/// since copies of a subform share its name but may hold different objects. The template tells
/// which segments are transparent; segments it doesn't have (page-set objects, or a SOM from
/// another template) are kept.
pub fn data_path(tpl: &Template, som: &str) -> DataPath {
    let mut out = DataPath::new();
    // The template children the next segment is looked up in; `None` until the root matched,
    // and again once a segment is not a container the template has.
    let mut level: Option<&[Node]> = None;
    let segments = som_to_path(som);
    let page_area_index = page_area_index(tpl, &segments);
    // `$record` in a reference: the data group of the root subform, as the data is written.
    let record = tpl.root.common.name.as_deref();
    for (i, (name, index)) in segments.into_iter().enumerate() {
        // A page area's SOM index is the page made from it; see [`page_area_index`].
        let index = if i == 0 { page_area_index.unwrap_or(index) } else { index };
        // What the segment names, its data index, and the explicit data reference it binds by.
        let (found, index, data_ref) = match level {
            None if i == 0 && tpl.root.common.name.as_deref() == Some(name.as_str()) => (Some(Container::Children(&tpl.root.children)), index, None),
            // Master-page objects are named under their page area, which is a data scope.
            None if i == 0 => match tpl.root.page_set.as_ref().and_then(|ps| ps.areas.iter().find(|a| a.name.as_deref() == Some(name.as_str()))) {
                Some(area) => (Some(Container::Children(&area.items)), index, None),
                None => (None, index, None),
            },
            None => (None, index, None),
            Some(children) => match named_in(children, &name, index, 0) {
                Some((siblings, node)) => {
                    let data_ref = node.common().data_ref.as_deref().map(|r| (r, star(siblings, node, index)));
                    let index = data_index(siblings, &name, index);
                    match node {
                        Node::Subform(sf) if sf.unbound => (Some(Container::Area(&sf.children)), index, None),
                        Node::Subform(sf) => (Some(Container::Children(&sf.children)), index, data_ref),
                        Node::Area(a) => (Some(Container::Area(&a.children)), index, None),
                        Node::Field(_) | Node::ExclGroup(_) => (None, index, data_ref),
                        Node::Draw(_) => (None, index, None),
                    }
                }
                None => (None, index, None),
            },
        };
        match found {
            Some(Container::Area(children)) => level = Some(children),
            Some(Container::Children(children)) => {
                level = Some(children);
                bind(&mut out, name, index, data_ref, record);
            }
            None => {
                level = None;
                bind(&mut out, name, index, data_ref, record);
            }
        }
    }
    out
}

/// Extend `out`, the parent's data path, with the node the object `name[index]` binds to: the
/// node its explicit data reference names (with the instance its `[*]` takes), or else the one
/// its name names. A reference this binding can't follow ([`resolve_ref`]) binds by name, so a
/// value written there still has somewhere to go.
fn bind(out: &mut DataPath, name: String, index: usize, data_ref: Option<(&str, usize)>, record: Option<&str>) {
    match data_ref.and_then(|(r, star)| resolve_ref(r, out, record, star)) {
        Some(path) => *out = path,
        None => out.push((name, index)),
    }
}

/// The instance a `[*]` in `node`'s reference takes, for its instance at SOM index `index`:
/// the instances the objects before it with the same reference took
/// ([`Common::star_ordinal`](crate::model::Common::star_ordinal)), and then its own instance
/// number when it is a repeating subform (its instances take SOM numbers from its own on).
fn star(siblings: &[Node], node: &Node, index: usize) -> usize {
    let instance = match node {
        Node::Subform(sf) if sf.occur.max != Some(1) => {
            let at = siblings.iter().position(|n| std::ptr::eq(n, node)).unwrap_or(0);
            index.saturating_sub(SomIndices::of(siblings).at(at))
        }
        _ => 0,
    };
    node.common().star_ordinal.saturating_add(instance)
}

/// The data path a `bind ref` SOM expression names, from the object's parent data node at
/// `parent` (XFA 3.3, "Explicit Data References", p. 199): `$record.a.b` (`record` is the
/// root subform's name, which names the record's data group), `$data.a.b`, `$.a.b` or `a.b`
/// (relative to the parent), each segment with an optional `[n]`, or `[*]`, which takes the
/// instance `star` (see [`star`]). Anything else (another DOM such as `$form`, `..`,
/// predicates, functions) is `None`.
fn resolve_ref(r: &str, parent: &DataPath, record: Option<&str>, star: usize) -> Option<DataPath> {
    let r = r.trim();
    let after = |prefix: &str| r.strip_prefix(prefix).filter(|rest| rest.is_empty() || rest.starts_with('.'));
    let (mut path, rest) = if let Some(rest) = after("$record") {
        (vec![(record?.to_string(), 0)], rest)
    } else if let Some(rest) = after("$data") {
        (DataPath::new(), rest)
    } else if let Some(rest) = after("$") {
        (parent.clone(), rest)
    } else if r.starts_with('$') || r.starts_with('!') {
        return None;
    } else {
        (parent.clone(), r)
    };
    let rest = rest.strip_prefix('.').unwrap_or(rest);
    if rest.is_empty() {
        return Some(path);
    }
    for seg in rest.split('.') {
        let (name, idx) = match seg.trim().split_once('[') {
            Some((n, idx)) => match idx.strip_suffix(']')?.trim() {
                "*" => (n.trim(), star),
                idx => (n.trim(), idx.parse::<usize>().ok()?),
            },
            None => (seg.trim(), 0),
        };
        if !ref_name(name) {
            return None;
        }
        path.push((name.to_string(), idx.min(100_000)));
        if path.len() > MAX_DEPTH {
            return None;
        }
    }
    Some(path)
}

/// How many instances of the repeating subform `sf` under the container at `parent_som` the
/// data holds: the nodes its reference matches when it ends in `[*]` (from the one its first
/// instance takes on), else the groups named after it under its parent's data node. A
/// subform with no data group of its own ([`Subform::data_name`]) and one that occurs once
/// count none.
pub(crate) fn data_instances(tpl: &Template, data: &DataNode, sf: &Subform, parent_som: &str) -> usize {
    if sf.occur.max == Some(1) {
        return 0;
    }
    let parent = data_path(tpl, parent_som);
    if let Some(r) = sf.common.data_ref.as_deref().filter(|r| r.trim_end().ends_with("[*]")) {
        let record = tpl.root.common.name.as_deref();
        if let Some(mut path) = resolve_ref(r, &parent, record, sf.common.star_ordinal)
            && let Some((name, first)) = path.pop()
        {
            return data.count(&path, &name).saturating_sub(first);
        }
    }
    sf.data_name().map_or(0, |name| data.count(&parent, name))
}

/// Number the objects whose reference has a `[*]` ([`Common::star_ordinal`]): each counts the
/// objects before it, in the form's order, with the same reference in the same data scope (the
/// data path of its container). Master-page objects are left at 0.
pub(crate) fn number_star_refs(tpl: &mut Template) {
    fn has_star(c: &Common) -> bool {
        c.data_ref.as_deref().is_some_and(|r| r.contains("[*]"))
    }
    fn walk(tpl: &Template, nodes: &[Node], som: &str, seen: &mut HashMap<(DataPath, String), usize>, out: &mut Vec<usize>, depth: usize) {
        if depth > MAX_DEPTH {
            return;
        }
        let sibs = SomIndices::of(nodes);
        for (i, n) in nodes.iter().enumerate() {
            let c = n.common();
            if has_star(c) {
                let key = (data_path(tpl, som), c.data_ref.clone().unwrap_or_default());
                let count = seen.entry(key).or_insert(0);
                out.push(*count);
                *count = count.saturating_add(1);
            }
            let here = match &c.name {
                Some(name) if som.is_empty() => format!("{name}[{}]", sibs.at(i)),
                Some(name) => format!("{som}.{name}[{}]", sibs.at(i)),
                None => som.to_string(),
            };
            match n {
                Node::Subform(s) => walk(tpl, &s.children, &here, seen, out, depth + 1),
                Node::Area(a) => walk(tpl, &a.children, &here, seen, out, depth + 1),
                Node::Field(_) | Node::ExclGroup(_) | Node::Draw(_) => {}
            }
        }
    }
    /// Hands the numbers out in the order [`walk`] made them.
    fn assign(nodes: &mut [Node], numbers: &mut std::vec::IntoIter<usize>, depth: usize) {
        if depth > MAX_DEPTH {
            return;
        }
        for n in nodes {
            let c = match n {
                Node::Subform(s) => &mut s.common,
                Node::Area(a) => &mut a.common,
                Node::Field(f) => &mut f.common,
                Node::ExclGroup(g) => &mut g.common,
                Node::Draw(d) => &mut d.common,
            };
            if has_star(c) {
                c.star_ordinal = numbers.next().unwrap_or(0);
            }
            match n {
                Node::Subform(s) => assign(&mut s.children, numbers, depth + 1),
                Node::Area(a) => assign(&mut a.children, numbers, depth + 1),
                Node::Field(_) | Node::ExclGroup(_) | Node::Draw(_) => {}
            }
        }
    }
    let mut numbers = Vec::new();
    let root = tpl.root.common.name.as_ref().map(|n| format!("{n}[0]")).unwrap_or_default();
    walk(tpl, &tpl.root.children, &root, &mut HashMap::new(), &mut numbers, 1);
    if numbers.is_empty() {
        return;
    }
    assign(&mut tpl.root.children, &mut numbers.into_iter(), 1);
}

/// Whether `name` is a plain element name in a `bind ref` (an empty one is part of a `..`).
fn ref_name(name: &str) -> bool {
    !name.is_empty() && !name.chars().any(|c| matches!(c, '(' | ')' | '#' | '$' | '!' | '"' | '\'' | '[' | ']' | '/' | '*') || c.is_whitespace())
}

/// For a SOM path that starts at a page area (`Area[n].…`), the index its first segment takes in
/// the data path, `None` for any other path. Each page made from a page area has its own SOM
/// index, so the master content repeated on each page (a page number, an office-use box) binds
/// apart on every page. A field whose bind shares one data node (`match="global"`, or an
/// explicit data reference) binds as instance 0 on each page instead, so it shows the same value
/// on all of them (XFA 3.3, "Page Background": a page-background field binds only through an
/// explicit data reference).
fn page_area_index(tpl: &Template, segments: &[(String, usize)]) -> Option<usize> {
    let ((first, instance), rest) = segments.split_first()?;
    if tpl.root.common.name.as_deref() == Some(first.as_str()) {
        return None;
    }
    let areas = tpl.root.page_set.as_ref()?;
    let area = areas.areas.iter().find(|a| a.name.as_deref() == Some(first.as_str()))?;
    // The object the rest of the path names, inside the page area's own content.
    let mut nodes: &[Node] = &area.items;
    let mut target = None;
    for (name, index) in rest {
        let found = named_at(nodes, name, *index, 0)?;
        target = Some(found);
        nodes = match found {
            Node::Subform(sf) => &sf.children,
            Node::Area(a) => &a.children,
            _ => &[],
        };
    }
    let shared = matches!(target, Some(Node::Field(f)) if f.data_shared);
    Some(if shared { 0 } else { *instance })
}

enum Container<'a> {
    Children(&'a [Node]),
    /// Contents bind under the area's parent.
    Area(&'a [Node]),
}

/// The child of `nodes` the SOM segment `name[index]` names, numbered as the form numbers its
/// objects ([`SomIndices`]): same-named siblings count up, and the instances of a repeating
/// subform take the numbers from its own on. Unnamed subforms and areas, which have no SOM
/// segment of their own, are looked through.
fn named_at<'a>(nodes: &'a [Node], name: &str, index: usize, depth: usize) -> Option<&'a Node> {
    named_in(nodes, name, index, depth).map(|(_, n)| n)
}

/// [`named_at`], with the siblings the object was found among: `nodes`, or the children of the
/// nameless subform or area it was found in.
fn named_in<'a>(nodes: &'a [Node], name: &str, index: usize, depth: usize) -> Option<(&'a [Node], &'a Node)> {
    if depth > MAX_DEPTH {
        return None;
    }
    let sibs = SomIndices::of(nodes);
    let mut repeating = None;
    for (i, n) in nodes.iter().enumerate() {
        if n.common().name.as_deref() != Some(name) {
            continue;
        }
        let sib = sibs.at(i);
        if sib == index {
            return Some((nodes, n));
        }
        if sib < index && matches!(n, Node::Subform(sf) if sf.occur.max != Some(1)) {
            repeating = Some((nodes, n));
        }
    }
    repeating.or_else(|| {
        nodes.iter().find_map(|n| match n {
            Node::Subform(sf) if sf.common.name.is_none() => named_in(&sf.children, name, index, depth + 1),
            Node::Area(a) if a.common.name.is_none() => named_in(&a.children, name, index, depth + 1),
            _ => None,
        })
    })
}

/// The data index of the object at SOM index `index` among the `siblings` named `name`: the
/// same-named siblings before it that bind to no data node (areas, and subforms with
/// `<bind match="none"/>`) take no data node, so they are not counted.
fn data_index(siblings: &[Node], name: &str, index: usize) -> usize {
    let sibs = SomIndices::of(siblings);
    let transparent = siblings
        .iter()
        .enumerate()
        .filter(|(i, n)| n.common().name.as_deref() == Some(name) && sibs.at(*i) < index)
        .filter(|(_, n)| matches!(n, Node::Area(_)) || matches!(n, Node::Subform(sf) if sf.unbound))
        .count();
    index.saturating_sub(transparent)
}

/// The template field the object at a SOM path is an instance of, each segment's number counted
/// as the form counts it ([`named_at`]). `None` when the template has no such field, or it is
/// a button of an exclusion group (whose buttons take their value from the group).
pub(crate) fn template_field<'a>(tpl: &'a Template, som: &str) -> Option<&'a Field> {
    match template_object(tpl, som)? {
        Node::Field(f) => Some(f),
        _ => None,
    }
}

/// The template object (field, exclusion group, subform...) the object at a SOM path is an
/// instance of, found as [`template_field`] finds a field. `None` for the root and for a button
/// of an exclusion group.
pub(crate) fn template_object<'a>(tpl: &'a Template, som: &str) -> Option<&'a Node> {
    let path = som_to_path(som);
    let ((root, _), rest) = path.split_first()?;
    if tpl.root.common.name.as_deref() != Some(root.as_str()) {
        return None;
    }
    let ((last, last_index), inner) = rest.split_last()?;
    let mut children: &[Node] = &tpl.root.children;
    for (name, index) in inner {
        children = match named_at(children, name, *index, 0)? {
            Node::Subform(sf) => &sf.children,
            Node::Area(a) => &a.children,
            _ => return None,
        };
    }
    named_at(children, last, *last_index, 0)
}

/// Every field with an `id`, with the SOM path the layout gives it: the root subform's
/// fields under the root, a page area's under the area (as master pages lay them out), each
/// named object adding its name and its number among its siblings (repeating subforms as their
/// first instance). Bounded by [`MAX_DEPTH`].
pub(crate) fn fields_with_ids(tpl: &Template) -> Vec<(&str, String, &Field)> {
    fn walk<'a>(nodes: &'a [Node], som: &str, out: &mut Vec<(&'a str, String, &'a Field)>, depth: usize) {
        if depth > MAX_DEPTH {
            return;
        }
        let sibs = SomIndices::of(nodes);
        for (i, n) in nodes.iter().enumerate() {
            let here = match &n.common().name {
                Some(name) if som.is_empty() => format!("{name}[{}]", sibs.at(i)),
                Some(name) => format!("{som}.{name}[{}]", sibs.at(i)),
                None => som.to_string(),
            };
            match n {
                Node::Field(f) => {
                    if let Some(id) = &f.common.id {
                        out.push((id.as_str(), here, f));
                    }
                }
                Node::Subform(s) => {
                    walk(&s.children, &here, out, depth + 1);
                    if let Some(ps) = &s.page_set {
                        for a in &ps.areas {
                            walk(&a.items, &a.name.as_ref().map(|n| format!("{n}[0]")).unwrap_or_default(), out, depth + 1);
                        }
                    }
                }
                Node::Area(a) => walk(&a.children, &here, out, depth + 1),
                Node::ExclGroup(_) | Node::Draw(_) => {}
            }
        }
    }
    let mut out = Vec::new();
    let root = tpl.root.common.name.as_ref().map(|n| format!("{n}[0]")).unwrap_or_default();
    walk(&tpl.root.children, &root, &mut out, 1);
    if let Some(ps) = &tpl.root.page_set {
        for a in &ps.areas {
            walk(&a.items, &a.name.as_ref().map(|n| format!("{n}[0]")).unwrap_or_default(), &mut out, 1);
        }
    }
    out
}

pub(super) fn path_som(path: &DataPath) -> String {
    path.iter().map(|(n, i)| format!("{n}[{i}]")).collect::<Vec<_>>().join(".")
}
