use super::*;

fn to_node(n: roxmltree::Node, depth: usize) -> DataNode {
    let name = n.tag_name().name().to_string();
    let children: Vec<DataNode> =
        if depth < MAX_DEPTH { n.children().filter(|c| c.is_element()).map(|c| to_node(c, depth + 1)).collect() } else { Vec::new() };
    // The text of a leaf is its value; a container's own text is whitespace.
    let text = if children.is_empty() { n.children().filter_map(|c| c.text()).collect::<String>() } else { String::new() };
    DataNode { name, text, children }
}

/// The `xfa:data` element of the datasets packet in `xdp` (or a bare datasets document), as a
/// tree whose root is that element.
pub fn parse_datasets(xdp: &str) -> Option<DataNode> {
    if crate::xml_nesting_depth(xdp) > crate::MAX_XML_DEPTH {
        return None;
    }
    let opts = roxmltree::ParsingOptions { allow_dtd: false, nodes_limit: MAX_NODES };
    let doc = roxmltree::Document::parse_with_options(xdp, opts).ok()?;
    let datasets = doc.descendants().find(|n| n.is_element() && n.tag_name().name() == "datasets")?;
    let data = datasets.children().find(|n| n.is_element() && n.tag_name().name() == "data")?;
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

/// The data path the object at `som` binds to: [`som_to_path`] without the areas on the way.
/// An area is a named container in the form, so it is part of SOM expressions, but only
/// subforms, exclusion groups and fields are data scopes: what an area holds binds under the
/// area's parent, as Adobe's viewers write it. The template tells which segments are areas;
/// segments it doesn't have (page-set objects, or a SOM from another template) are kept.
pub fn data_path(tpl: &Template, som: &str) -> DataPath {
    let mut out = DataPath::new();
    // The template children the next segment is looked up in; `None` until the root matched,
    // and again once a segment is not a container the template has.
    let mut level: Option<&[Node]> = None;
    for (i, (name, index)) in som_to_path(som).into_iter().enumerate() {
        let found = match level {
            None if i == 0 && tpl.root.common.name.as_deref() == Some(name.as_str()) => Some(Container::Children(&tpl.root.children)),
            None => None,
            Some(children) => match named(children, &name, 0) {
                Some(Node::Subform(sf)) => Some(Container::Children(&sf.children)),
                Some(Node::Area(a)) => Some(Container::Area(&a.children)),
                _ => None,
            },
        };
        match found {
            Some(Container::Area(children)) => level = Some(children),
            Some(Container::Children(children)) => {
                level = Some(children);
                out.push((name, index));
            }
            None => {
                level = None;
                out.push((name, index));
            }
        }
    }
    out
}

enum Container<'a> {
    Children(&'a [Node]),
    /// Contents bind under the area's parent.
    Area(&'a [Node]),
}

/// The child of `nodes` named `name`, looking through unnamed subforms and areas (which have no
/// SOM segment of their own).
fn named<'a>(nodes: &'a [Node], name: &str, depth: usize) -> Option<&'a Node> {
    if depth > MAX_DEPTH {
        return None;
    }
    nodes.iter().find(|n| n.common().name.as_deref() == Some(name)).or_else(|| {
        nodes.iter().find_map(|n| match n {
            Node::Subform(sf) if sf.common.name.is_none() => named(&sf.children, name, depth + 1),
            Node::Area(a) if a.common.name.is_none() => named(&a.children, name, depth + 1),
            _ => None,
        })
    })
}

/// The child of `nodes` the SOM segment `name[index]` names, numbered as the form numbers its
/// objects ([`SomIndices`]): same-named siblings count up, and the instances of a repeating
/// subform take the numbers from its own on. Unnamed subforms and areas are looked through,
/// as [`named`] does.
fn named_at<'a>(nodes: &'a [Node], name: &str, index: usize, depth: usize) -> Option<&'a Node> {
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
            return Some(n);
        }
        if sib < index && matches!(n, Node::Subform(sf) if sf.occur.max != Some(1)) {
            repeating = Some(n);
        }
    }
    repeating.or_else(|| {
        nodes.iter().find_map(|n| match n {
            Node::Subform(sf) if sf.common.name.is_none() => named_at(&sf.children, name, index, depth + 1),
            Node::Area(a) if a.common.name.is_none() => named_at(&a.children, name, index, depth + 1),
            _ => None,
        })
    })
}

/// The template field the object at a SOM path is an instance of, each segment's number counted
/// as the form counts it ([`named_at`]). `None` when the template has no such field, or it is
/// a button of an exclusion group (whose buttons take their value from the group).
pub(crate) fn template_field<'a>(tpl: &'a Template, som: &str) -> Option<&'a Field> {
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
    match named_at(children, last, *last_index, 0)? {
        Node::Field(f) => Some(f),
        _ => None,
    }
}

pub(super) fn path_som(path: &DataPath) -> String {
    path.iter().map(|(n, i)| format!("{n}[{i}]")).collect::<Vec<_>>().join(".")
}
