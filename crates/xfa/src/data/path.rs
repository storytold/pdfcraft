use super::{DataNode, DataPath, Field, MAX_DEPTH, MAX_NODES, Node, SomIndices, Template};

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
    let opts = roxmltree::ParsingOptions { allow_dtd: false, nodes_limit: MAX_NODES, ..Default::default() };
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
    let segments = som_to_path(som);
    let page_area_index = page_area_index(tpl, &segments);
    for (i, (name, index)) in segments.into_iter().enumerate() {
        // A page area's SOM index is the page made from it; see [`page_area_index`].
        let index = if i == 0 { page_area_index.unwrap_or(index) } else { index };
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
