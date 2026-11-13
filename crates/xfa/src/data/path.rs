use super::{DataNode, DataPath, Field, MAX_DEPTH, MAX_NODES, Node, SomIndices, Template, XFA_DATA_NS};

fn to_node(n: roxmltree::Node, depth: usize) -> DataNode {
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
    let opts = roxmltree::ParsingOptions { allow_dtd: false, nodes_limit: MAX_NODES, ..Default::default() };
    let doc = roxmltree::Document::parse_with_options(xdp, opts).ok()?;
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
    let opts = || roxmltree::ParsingOptions { allow_dtd: false, nodes_limit: MAX_NODES, ..Default::default() };
    let wrapped;
    let doc = match roxmltree::Document::parse_with_options(text, opts()) {
        Ok(d) => d,
        Err(_) => {
            wrapped = format!("{}{text}{}", super::merge::WRAP_OPEN, super::merge::WRAP_CLOSE);
            roxmltree::Document::parse_with_options(&wrapped, opts()).ok()?
        }
    };
    let is = |n: &roxmltree::Node, name: &str| n.is_element() && n.tag_name().name() == name;
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

/// The data path the object at `som` binds to: [`som_to_path`] without the areas on the way.
/// An area is a named container in the form, so it is part of SOM expressions, but only
/// subforms, exclusion groups and fields are data scopes: what an area holds binds under the
/// area's parent, as Adobe's viewers write it. The template tells which segments are areas,
/// each looked up by its number as the form numbers its objects ([`named_toward`]), so a
/// subform that shares its name with an area keeps its segment; segments the template doesn't
/// have (page-set objects, or a SOM from another template) are kept.
pub fn data_path(tpl: &Template, som: &str) -> DataPath {
    let mut out = DataPath::new();
    // The template children the next segment is looked up in; `None` until the root matched,
    // and again once a segment is not a container the template has.
    let mut level: Option<&[Node]> = None;
    let segments = som_to_path(som);
    let page_area_index = page_area_index(tpl, &segments);
    for (i, (name, index)) in segments.iter().enumerate() {
        // A page area's SOM index is the page made from it; see [`page_area_index`].
        let index = if i == 0 { page_area_index.unwrap_or(*index) } else { *index };
        let found = match level {
            None if i == 0 && tpl.root.common.name.as_deref() == Some(name.as_str()) => Some(Container::Children(&tpl.root.children)),
            None => None,
            // By its number, not its name alone: a subform and an area may share a name, and
            // only the area's segment is dropped.
            Some(children) => match named_toward(children, name, index, i.checked_add(1).and_then(|next| segments.get(next))) {
                Some(Node::Subform(sf)) => Some(Container::Children(&sf.children)),
                Some(Node::Area(a)) => Some(Container::Area(&a.children)),
                _ => None,
            },
        };
        match found {
            Some(Container::Area(children)) => level = Some(children),
            Some(Container::Children(children)) => {
                level = Some(children);
                out.push((name.clone(), index));
            }
            None => {
                level = None;
                out.push((name.clone(), index));
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
    for (i, (name, index)) in rest.iter().enumerate() {
        let found = named_toward(nodes, name, *index, i.checked_add(1).and_then(|next| rest.get(next)))?;
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

/// The object the SOM segment `name[index]` names among `nodes`, the content of a named
/// container, numbered as the form numbers its objects ([`crate::model::Common::som_index`]):
/// same-named objects count up through unnamed subforms and areas, which are looked through
/// (XFA 3.3 pp. 94-95), and the instances of a repeating subform take the numbers from its own
/// on. A number no object has is an instance of the nearest subform below it that the template
/// lets repeat, else (a path the form doesn't write, read leniently) of the nearest one below
/// it.
fn named_at<'a>(nodes: &'a [Node], name: &str, index: usize, depth: usize) -> Option<&'a Node> {
    let mut repeating = None;
    exactly_at(nodes, name, index, depth, &mut repeating).or(repeating)
}

/// [`named_at`] for a segment on the way to `next`, the segment after it. A later instance of
/// a subform that may repeat has the number the template gave a same-named object after the
/// subform (a `row` area after the `row` subform is `row[1]`, and so is the second row), so
/// two objects answer to the segment: when the one numbered so exactly doesn't hold what
/// `next` names and the subform does, the path is the instance's. Otherwise, and for a
/// path's last segment, the object numbered exactly.
fn named_toward<'a>(nodes: &'a [Node], name: &str, index: usize, next: Option<&(String, usize)>) -> Option<&'a Node> {
    let mut repeating = None;
    let exact = exactly_at(nodes, name, index, 0, &mut repeating);
    let (Some(numbered), Some(instance), Some((next, at))) = (exact, repeating, next) else { return exact.or(repeating) };
    let holds = |n: &Node| match n {
        Node::Subform(sf) => named_at(&sf.children, next, *at, 0).is_some(),
        Node::Area(a) => named_at(&a.children, next, *at, 0).is_some(),
        _ => false,
    };
    let may_repeat = matches!(instance, Node::Subform(sf) if sf.occur.max != Some(1));
    if may_repeat && !holds(numbered) && holds(instance) { Some(instance) } else { exact }
}

/// [`named_at`]'s walk: the object numbered `index` exactly, noting in `repeating` the last
/// subform of the name numbered below it on the way (one the template lets repeat, when there
/// is one).
fn exactly_at<'a>(nodes: &'a [Node], name: &str, index: usize, depth: usize, repeating: &mut Option<&'a Node>) -> Option<&'a Node> {
    if depth > MAX_DEPTH {
        return None;
    }
    for n in nodes {
        let c = n.common();
        let found = match (c.name.as_deref(), n) {
            (Some(own), _) if own == name => {
                if c.som_index == index {
                    return Some(n);
                }
                if c.som_index < index
                    && let Node::Subform(sf) = n
                    && (sf.occur.max != Some(1) || !repeating.is_some_and(|r| matches!(r, Node::Subform(s) if s.occur.max != Some(1))))
                {
                    *repeating = Some(n);
                }
                None
            }
            (None, Node::Subform(sf)) => exactly_at(&sf.children, name, index, depth + 1, repeating),
            (None, Node::Area(a)) => exactly_at(&a.children, name, index, depth + 1, repeating),
            _ => None,
        };
        if found.is_some() {
            return found;
        }
    }
    None
}

/// Whether more than one object of the live form may have the SOM path `som`, so that
/// nothing can be kept by it for one object. Named objects have a path each: same-named ones are numbered
/// through unnamed subforms and areas, which add no segment (XFA 3.3 ch. 3, "Transparent
/// Nodes", pp. 94-95, Example 3.12; [`crate::model::Common::som_index`]). The paths that are
/// shared:
///
/// - an unnamed field is `field[0]` and an unnamed exclusion group `group[0]`, so two of them
///   in one named container share theirs (XFA 3.3 gives unnamed objects no path of their own:
///   they are "invisible to the normal SOM syntax", p. 94);
/// - the instances of an unnamed subform the template makes more than once each name their
///   content as the first does;
/// - a later instance of a subform that may repeat takes the next numbers, which a same-named
///   object after it in the template has too. The template can't tell how many instances the
///   data makes, so such an object's path counts as shared whenever the subform before it may
///   repeat, and so do the paths of what is inside either.
///
/// Who owns which path is worked out once per template ([`crate::script::SomOwners`]); each
/// call looks up the segments of `som`. `false` when the template has no object at `som`, and
/// for the root and a page area themselves.
pub fn som_is_shared(tpl: &Template, som: &str) -> bool {
    tpl.memo.som_owners(tpl).shared(som)
}

/// Whether the form keeps what scripts change about the properties of the object at the SOM
/// path `som`: it does for an object the template makes no more than once, whose path is its
/// own whatever the data holds. That is so when the path starts at the root subform and each
/// segment after it names one object that occurs once (`occur max="1"`, as fields, draws, areas
/// and exclusion groups always do), with no instance of a subform that may repeat able to take
/// its number: same-named objects are numbered through unnamed subforms and areas, which add no
/// segment (XFA 3.3 ch. 3, "Transparent Nodes", pp. 94-95, Example 3.12;
/// [`crate::model::Common::som_index`]), and a repeating subform's later instances take the
/// numbers after its own. Not so, and `false`:
///
/// - a subform that may repeat and anything inside one (a change there would be one
///   instance's, and the template object stands for all of them);
/// - a same-named object after a subform that may repeat, and what is inside it;
/// - what an unnamed subform the template makes more than once holds (each instance names its
///   content as the first does);
/// - two unnamed fields, or two unnamed exclusion groups, of one container (each is `field[0]`
///   or `group[0]`: unnamed objects are "invisible to the normal SOM syntax", p. 94);
/// - a page area and its content (a page area makes a page each time it is used, each with its
///   own path), and everything in a form whose root subform has no name;
/// - a path the template has no object at, or one not written as the form writes paths
///   (`name[n]` for each segment);
/// - every path of a template that has `[` or `]` in an object's name (a path there doesn't
///   say where a segment ends).
///
/// An object made once never shares its path ([`som_is_shared`]). Who owns which path is worked
/// out once per template ([`crate::script::SomOwners`]); each call looks up the segments of
/// `som` once.
pub fn som_made_once(tpl: &Template, som: &str) -> bool {
    tpl.memo.som_owners(tpl).made_once(som)
}

/// [`som_made_once`] for one template, to ask away from it: on the thread scripts run on, by
/// the scripting host, which refuses a property write the form would refuse. It holds the
/// template's record of who owns each path (made once per template, and shared), so both give
/// one answer for every path.
#[derive(Clone)]
pub struct SomPaths(std::sync::Arc<crate::script::SomOwners>);

impl SomPaths {
    /// The paths of `tpl`.
    pub fn of(tpl: &Template) -> SomPaths {
        SomPaths(std::sync::Arc::clone(tpl.memo.som_owners(tpl)))
    }

    /// [`som_made_once`] of the template and `som`.
    pub fn made_once(&self, som: &str) -> bool {
        self.0.made_once(som)
    }
}

impl std::fmt::Debug for SomPaths {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("SomPaths")
    }
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
    for (i, (name, index)) in inner.iter().enumerate() {
        // `rest` is `inner` and the last segment, so the one after an inner segment is there.
        children = match named_toward(children, name, *index, i.checked_add(1).and_then(|next| rest.get(next)))? {
            Node::Subform(sf) => &sf.children,
            Node::Area(a) => &a.children,
            _ => return None,
        };
    }
    named_at(children, last, *last_index, 0)
}

/// Every field with an `id`, with the SOM path the layout gives it: the root subform's
/// fields under the root, a page area's under the area (as master pages lay them out), each
/// named object adding its name and its SOM number ([`crate::model::Common::som_index`];
/// repeating subforms as their first instance). Bounded by [`MAX_DEPTH`].
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
