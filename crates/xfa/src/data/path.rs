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
    // `$record` in a reference: the root subform's data node ([`record_path`]), worked out the
    // first time something needs it: most paths bind by name all the way.
    let record_cell: std::cell::OnceCell<Option<DataPath>> = std::cell::OnceCell::new();
    let record = || record_cell.get_or_init(|| record_path(tpl)).as_ref();
    // The `match="none"` subforms passed through since the last data scope ([`data_index`]).
    let mut through: Vec<Step> = Vec::new();
    for (i, (name, index)) in segments.into_iter().enumerate() {
        // A page area's SOM index is the page made from it; see [`page_area_index`].
        let index = if i == 0 { page_area_index.unwrap_or(index) } else { index };
        // The root binds to the record: the node its own reference names, resolved once
        // ([`record_path`]), else the group named after it.
        if i == 0 && level.is_none() && tpl.root.common.name.as_deref() == Some(name.as_str()) {
            level = Some(&tpl.root.children);
            match tpl.root.common.data_ref.as_ref().and_then(|_| record()) {
                Some(path) => out = path.clone(),
                None => out.push((name, index)),
            }
            continue;
        }
        // What the segment names, its data index, and the explicit data reference it binds by.
        let (found, index, data_ref) = match level {
            // Master-page objects are named under their page area, which is a data scope.
            None if i == 0 => match tpl.root.page_set.as_ref().and_then(|ps| ps.areas.iter().find(|a| a.name.as_deref() == Some(name.as_str()))) {
                Some(area) => (Some(Container::Children(&area.items)), index, None),
                None => (None, index, None),
            },
            None => (None, index, None),
            Some(children) => match named_in(children, &name, index, 0) {
                Some(step) => {
                    let node = step.node;
                    let data_ref = node.common().data_ref.as_deref().map(|r| (r, node.common().star_ordinal.saturating_add(step.instance)));
                    match node {
                        Node::Subform(sf) if sf.unbound => {
                            through.push(step);
                            (Some(Container::Area(&sf.children)), index, None)
                        }
                        Node::Area(a) => (Some(Container::Area(&a.children)), index, None),
                        Node::Subform(sf) => (Some(Container::Children(&sf.children)), data_index(&through, &step, &name), data_ref),
                        Node::Field(_) | Node::ExclGroup(_) => (None, data_index(&through, &step, &name), data_ref),
                        Node::Draw(_) => (None, data_index(&through, &step, &name), None),
                    }
                }
                None => (None, index, None),
            },
        };
        match found {
            Some(Container::Area(children)) => level = Some(children),
            Some(Container::Children(children)) => {
                level = Some(children);
                through.clear();
                bind(&mut out, name, index, data_ref, data_ref.and_then(|_| record()));
            }
            None => {
                level = None;
                through.clear();
                bind(&mut out, name, index, data_ref, data_ref.and_then(|_| record()));
            }
        }
    }
    out
}

/// Extend `out`, the parent's data path, with the node the object `name[index]` binds to: the
/// node its explicit data reference names (with the instance its `[*]` takes), or else the one
/// its name names. A reference this binding can't follow ([`resolve_ref`]) binds by name, so a
/// value written there still has somewhere to go.
fn bind(out: &mut DataPath, name: String, index: usize, data_ref: Option<(&str, usize)>, record: Option<&DataPath>) {
    match data_ref.and_then(|(r, star)| resolve_ref(r, out, record, star)) {
        Some(path) => *out = path,
        None => out.push((name, index)),
    }
}

/// The data path a `bind ref` SOM expression names, from the object's parent data node at
/// `parent` (XFA 3.3, "Explicit Data References", p. 199): `$record.a.b` (`record` is the
/// root subform's name, which names the record's data group), `$data.a.b`, `$.a.b` or `a.b`
/// (relative to the parent), each segment with an optional `[n]`, or `[*]`, which takes the
/// instance `star`: the object's `star_ordinal` plus its own instance number when it is a
/// repeating subform. Anything else (another DOM such as `$form`, `..`, signed indexes,
/// predicates, functions) is `None`.
fn resolve_ref(r: &str, parent: &DataPath, record: Option<&DataPath>, star: usize) -> Option<DataPath> {
    let r = r.trim();
    let after = |prefix: &str| r.strip_prefix(prefix).filter(|rest| rest.is_empty() || rest.starts_with('.'));
    let (mut path, rest) = if let Some(rest) = after("$record") {
        (record?.clone(), rest)
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
                idx if !idx.is_empty() && idx.bytes().all(|b| b.is_ascii_digit()) => (n.trim(), idx.parse::<usize>().ok()?),
                // A signed index (`[+1]`, `[-1]`) is relative to the object's own: not followed.
                _ => return None,
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
/// instance takes on), the one node a reference without one names, else the groups named
/// after it under its parent's data node. A subform with no data group of its own
/// ([`Subform::data_name`]) and one that occurs once count none.
pub(crate) fn data_instances(tpl: &Template, data: &DataNode, sf: &Subform, parent_som: &str) -> usize {
    if sf.occur.max == Some(1) {
        return 0;
    }
    let parent = data_path(tpl, parent_som);
    if let Some(r) = sf.common.data_ref.as_deref().filter(|r| r.trim().rsplit('.').next().is_some_and(is_star_segment)) {
        let record = record_path(tpl);
        if let Some(mut path) = resolve_ref(r, &parent, record.as_ref(), sf.common.star_ordinal)
            && let Some((name, first)) = path.pop()
        {
            return data.count(&path, &name).saturating_sub(first);
        }
    }
    // A reference that names one node: that node, once, when the data has it. Groups named after
    // the subform aren't its instances ([`binds_elsewhere`]).
    if let Some(r) = sf.common.data_ref.as_deref().filter(|_| sf.common.follows_ref) {
        let record = record_path(tpl);
        return resolve_ref(r, &parent, record.as_ref(), sf.common.star_ordinal).map_or(0, |path| usize::from(data.get(&path).is_some()));
    }
    sf.data_name().map_or(0, |name| {
        // Its first instance's data index: same-named objects before it in the data scope (one
        // of its name, or what a `match="none"` sibling hands on) take the nodes before that.
        let first = som_index(tpl, parent_som, sf)
            .map(|sib| if parent_som.is_empty() { format!("{name}[{sib}]") } else { format!("{parent_som}.{name}[{sib}]") })
            .and_then(|som| data_path(tpl, &som).last().map(|(_, index)| *index))
            .unwrap_or(0);
        data.count(&parent, name).saturating_sub(first)
    })
}

/// The SOM index of the subform `sf` among the children of the container at `parent_som`, or
/// `None` when it isn't one of them (it sits in a nameless wrapper, or the template lacks it).
fn som_index(tpl: &Template, parent_som: &str, sf: &Subform) -> Option<usize> {
    let path = som_to_path(parent_som);
    let children: &[Node] = if path.len() == 1 && tpl.root.common.name.as_deref() == path.first().map(|(n, _)| n.as_str()) {
        &tpl.root.children
    } else {
        match template_object(tpl, parent_som).or_else(|| master_object(tpl, parent_som))? {
            Node::Subform(s) => &s.children,
            Node::Area(a) => &a.children,
            _ => return None,
        }
    };
    // The layout works on a copy of the template, so a subform equal to `sf` is `sf`.
    let pos =
        children.iter().position(|n| matches!(n, Node::Subform(s) if s.common.name == sf.common.name && (std::ptr::eq(&**s, sf) || **s == *sf)))?;
    let name = sf.common.name.as_deref()?;
    Some(children.get(..pos)?.iter().filter(|n| n.common().name.as_deref() == Some(name)).count())
}

/// Number the objects whose reference has a `[*]` ([`Common::star_ordinal`]): each counts the
/// objects before it, in the form's order, whose reference matches the same nodes: "one more
/// instance of the object matching the `*`" (XFA 3.3 p. 199). A relative reference matches
/// nodes under its container's data node, so the same reference in two scopes counts apart; an
/// absolute one matches the same nodes wherever it is. Master-page objects are numbered too, before
/// the content, as the page set comes first in the template.
pub(crate) fn number_star_refs(tpl: &mut Template) {
    fn has_star(c: &Common) -> bool {
        c.name.is_some() && c.data_ref.as_deref().is_some_and(|r| r.split('.').any(is_star_segment))
    }
    fn walk(tpl: &Template, nodes: &[Node], som: &str, seen: &mut HashMap<(DataPath, String), usize>, out: &mut Vec<usize>, depth: usize) {
        if depth > MAX_DEPTH {
            return;
        }
        let sibs = SomIndices::of(nodes);
        for (i, n) in nodes.iter().enumerate() {
            let c = n.common();
            if has_star(c) {
                // The nodes the reference matches: its path with each `*` left open. A reference this
                // binding can't follow is grouped by its text in its container's scope.
                let parent = data_path(tpl, som);
                let r = c.data_ref.clone().unwrap_or_default();
                let key = match resolve_ref(&r, &parent, record_path(tpl).as_ref(), STAR_OPEN) {
                    Some(matched) => (matched, String::new()),
                    None => (parent, r),
                };
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
    let root = tpl.root.common.name.as_ref().map(|n| format!("{n}[0]")).unwrap_or_default();
    // A container whose own reference has a `[*]` is the data scope of what it holds only once
    // it is numbered, so each pass settles one more level of such nesting; the numbers of the
    // outermost are right after the first. Stop when a pass changes nothing.
    let mut last: Vec<usize> = Vec::new();
    for _ in 0..MAX_STAR_PASSES {
        let mut numbers = Vec::new();
        let mut seen = HashMap::new();
        // The root comes first in the form's order, so its own reference takes its instance
        // before anything inside it does.
        if has_star(&tpl.root.common) {
            let r = tpl.root.common.data_ref.clone().unwrap_or_default();
            // Against the record as it is before the root binds: the group named after it.
            let by_name = tpl.root.common.name.clone().map(|n| vec![(n, 0)]);
            let key = match resolve_ref(&r, &DataPath::new(), by_name.as_ref(), STAR_OPEN) {
                Some(matched) => (matched, String::new()),
                None => (DataPath::new(), r),
            };
            seen.insert(key, 1);
        }
        // The page set comes first in the template, so master-page objects before the content.
        for area in tpl.root.page_set.iter().flat_map(|ps| ps.areas.iter()) {
            let som = area.name.as_ref().map(|n| format!("{n}[0]")).unwrap_or_default();
            walk(tpl, &area.items, &som, &mut seen, &mut numbers, 1);
        }
        walk(tpl, &tpl.root.children, &root, &mut seen, &mut numbers, 1);
        if numbers.is_empty() || numbers == last {
            return;
        }
        let mut handed = numbers.clone().into_iter();
        for area in tpl.root.page_set.iter_mut().flat_map(|ps| ps.areas.iter_mut()) {
            assign(&mut area.items, &mut handed, 1);
        }
        assign(&mut tpl.root.children, &mut handed, 1);
        last = numbers;
    }
}

/// Most passes [`number_star_refs`] makes: one per level of containers nested inside each other
/// that are bound by a `[*]` reference.
const MAX_STAR_PASSES: usize = MAX_DEPTH + 1;

/// The instance a `*` takes while [`number_star_refs`] groups references by the nodes they
/// match: one no real instance number takes, so the `*` stays open.
const STAR_OPEN: usize = usize::MAX;

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
    named_in(nodes, name, index, depth).map(|s| s.node)
}

/// What a SOM segment names, found by [`named_in`]: the object, the siblings it was found among
/// (`nodes`, or the children of the nameless subform or area it was found in), its position
/// there, and which of its instances the segment names (0 unless it is a repeating subform).
struct Step<'a> {
    siblings: &'a [Node],
    pos: usize,
    node: &'a Node,
    instance: usize,
}

/// [`named_at`], as a [`Step`].
fn named_in<'a>(nodes: &'a [Node], name: &str, index: usize, depth: usize) -> Option<Step<'a>> {
    if depth > MAX_DEPTH {
        return None;
    }
    // Same-named siblings count up ([`SomIndices`]); only the ones named `name` matter here, so
    // they are counted on the way rather than numbering every sibling.
    let mut sib = 0usize;
    let mut repeating = None;
    for (i, n) in nodes.iter().enumerate() {
        if n.common().name.as_deref() != Some(name) {
            continue;
        }
        let this = sib;
        sib = sib.saturating_add(1);
        if this == index {
            return Some(Step { siblings: nodes, pos: i, node: n, instance: 0 });
        }
        if this < index && matches!(n, Node::Subform(sf) if sf.occur.max != Some(1)) {
            repeating = Some(Step { siblings: nodes, pos: i, node: n, instance: index - this });
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

/// The data index of the object `at` names, named `name`, reached through the `match="none"`
/// subforms in `through` since its data scope. Data binding treats those subforms as removed
/// and "their children adopted by the transparent node's parent" (XFA 3.3, "Transparent
/// Nodes", p. 193), so the index counts every same-named object before it in that adopted
/// scope: the siblings before each step (and what the `match="none"` ones among them hold),
/// and what the earlier instances of a repeating `match="none"` subform on the way hold. Then
/// its own instance number, for a repeating subform.
fn data_index(through: &[Step], at: &Step, name: &str) -> usize {
    let mut index = 0usize;
    for step in through {
        index = index.saturating_add(adopted(step.siblings.get(..step.pos).unwrap_or(&[]), name));
        if let Node::Subform(sf) = step.node {
            index = index.saturating_add(step.instance.saturating_mul(sf.adopted.get(name).copied().unwrap_or(0)));
        }
    }
    index.saturating_add(adopted(at.siblings.get(..at.pos).unwrap_or(&[]), name)).saturating_add(at.instance)
}

/// How many objects named `name` the data scope these `nodes` sit in takes from them: one for
/// each such field, exclusion group, draw or subform (unless it binds by a reference it follows),
/// and what a `match="none"` subform holds,
/// once for each instance it lays out. Areas and nameless subforms keep their contents' own
/// numbering, as they always have here.
fn adopted(nodes: &[Node], name: &str) -> usize {
    nodes.iter().fold(0usize, |n, node| n.saturating_add(adopted_one(node, name)))
}

/// What one object hands its data scope of the objects named `name`, as [`adopted`] counts.
fn adopted_one(node: &Node, name: &str) -> usize {
    match node {
        // Counted once, after parsing ([`mark_bindings`]).
        Node::Subform(sf) if sf.unbound => sf.adopted.get(name).copied().unwrap_or(0).saturating_mul(crate::layout::instance_count(&sf.occur, 0)),
        Node::Area(_) => 0,
        Node::Subform(sf) if sf.common.name.is_none() => 0,
        // An object bound by a reference it follows takes no node named after it.
        other if other.common().follows_ref => 0,
        other => usize::from(other.common().name.as_deref() == Some(name)),
    }
}

/// Settle what [`data_path`] needs to know of every object, once, after parsing: whether it
/// binds by a reference this binding follows ([`Common::follows_ref`]), and for each
/// `match="none"` subform, how many objects of each name it hands to the enclosing data scope
/// ([`Subform::adopted`]), counted from the inside out. Looking these up when each path is
/// resolved would walk large parts of the template for every field a script reads.
pub(crate) fn mark_bindings(tpl: &mut Template) {
    fn mark(nodes: &mut [Node], depth: usize) {
        if depth > MAX_DEPTH {
            return;
        }
        for node in nodes.iter_mut() {
            match node {
                Node::Subform(sf) => {
                    mark(&mut sf.children, depth + 1);
                    if sf.unbound {
                        let mut counts: HashMap<String, usize> = HashMap::new();
                        for child in &sf.children {
                            let inner = match child {
                                Node::Subform(inner) if inner.unbound => Some(&inner.adopted),
                                _ => None,
                            };
                            match inner {
                                Some(map) => {
                                    let times = match child {
                                        Node::Subform(c) => crate::layout::instance_count(&c.occur, 0),
                                        _ => 1,
                                    };
                                    for (name, n) in map {
                                        let entry = counts.entry(name.clone()).or_insert(0);
                                        *entry = entry.saturating_add(n.saturating_mul(times));
                                    }
                                }
                                None => {
                                    if let Some(name) = child.common().name.as_deref()
                                        && adopted_one(child, name) > 0
                                    {
                                        let entry = counts.entry(name.to_string()).or_insert(0);
                                        *entry = entry.saturating_add(1);
                                    }
                                }
                            }
                        }
                        sf.adopted = counts;
                    }
                }
                Node::Area(a) => mark(&mut a.children, depth + 1),
                Node::Field(_) | Node::ExclGroup(_) | Node::Draw(_) => {}
            }
            let c = match node {
                Node::Subform(s) => &mut s.common,
                Node::Area(a) => &mut a.common,
                Node::Field(f) => &mut f.common,
                Node::ExclGroup(g) => &mut g.common,
                Node::Draw(d) => &mut d.common,
            };
            c.follows_ref = c.data_ref.as_deref().is_some_and(followable);
        }
    }
    let root = &mut tpl.root;
    root.common.follows_ref = root.common.data_ref.as_deref().is_some_and(followable);
    mark(&mut root.children, 1);
    if let Some(ps) = root.page_set.as_mut() {
        for area in &mut ps.areas {
            mark(&mut area.items, 1);
        }
    }
    fn any(nodes: &[Node], depth: usize) -> bool {
        depth <= MAX_DEPTH
            && nodes.iter().any(|n| {
                n.common().follows_ref
                    || match n {
                        Node::Subform(sf) => sf.unbound || any(&sf.children, depth + 1),
                        Node::Area(a) => any(&a.children, depth + 1),
                        Node::ExclGroup(g) => g.fields.iter().any(|f| f.common.follows_ref),
                        Node::Field(_) | Node::Draw(_) => false,
                    }
            })
    }
    let explicit = tpl.root.unbound
        || tpl.root.common.follows_ref
        || any(&tpl.root.children, 1)
        || tpl.root.page_set.iter().flat_map(|ps| ps.areas.iter()).any(|a| any(&a.items, 1));
    tpl.explicit_binds = explicit;
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

/// The record `$record` names in a reference: the root subform's data node, the one its own
/// reference names when it has one this binding follows, else the group named after it.
fn record_path(tpl: &Template) -> Option<DataPath> {
    let by_name = tpl.root.common.name.clone().map(|n| vec![(n, 0)]);
    match tpl.root.common.data_ref.as_deref() {
        Some(r) => resolve_ref(r, &DataPath::new(), by_name.as_ref(), 0).or(by_name),
        None => by_name,
    }
}

/// Whether [`resolve_ref`] follows this reference at all, wherever the object sits.
fn followable(r: &str) -> bool {
    resolve_ref(r, &DataPath::new(), Some(&vec![("record".to_string(), 0)]), 0).is_some()
}

/// Whether a segment of a `bind ref` takes the next instance: `name[*]`, spaces allowed inside
/// the brackets, as [`resolve_ref`] reads it.
fn is_star_segment(seg: &str) -> bool {
    seg.trim().split_once('[').and_then(|(_, idx)| idx.strip_suffix(']')).is_some_and(|idx| idx.trim() == "*")
}

/// Whether the object at `som` is a subform of the template whose instances are not data groups
/// named after it under its parent's data node: one bound by a reference (`match="dataRef"`),
/// or one with no data group of its own ([`Subform::data_name`]). Adding or removing its
/// instances by name would change some other node. `false` for objects the template lacks.
pub fn binds_elsewhere(tpl: &Template, som: &str) -> bool {
    matches!(template_object(tpl, som), Some(Node::Subform(sf)) if sf.data_name().is_none() || sf.common.data_ref.is_some())
}

/// Whether the objects at SOM paths `a` and `b` are instances of the same template object: the
/// instances of one repeating subform's field are, the same-named fields of two copies of a
/// subform (`part[0]`, `part[1]`) are not. Master-page objects (`Page1[n].x`) are found in their
/// page area. `false` when the template has neither.
pub fn same_template_object(tpl: &Template, a: &str, b: &str) -> bool {
    let object = |som: &str| template_object(tpl, som).or_else(|| master_object(tpl, som));
    match (object(a), object(b)) {
        (Some(x), Some(y)) => std::ptr::eq(x, y),
        _ => false,
    }
}

/// The master-page object a SOM path starting at a page area names (`Page1[2].hdr[0].x[0]`):
/// every page made from the area holds the same object, whatever its page's index.
fn master_object<'a>(tpl: &'a Template, som: &str) -> Option<&'a Node> {
    let path = som_to_path(som);
    let ((area, _), rest) = path.split_first()?;
    let area = tpl.root.page_set.as_ref()?.areas.iter().find(|a| a.name.as_deref() == Some(area.as_str()))?;
    let ((last, last_index), inner) = rest.split_last()?;
    let mut children: &[Node] = &area.items;
    for (name, index) in inner {
        children = match named_at(children, name, *index, 0)? {
            Node::Subform(sf) => &sf.children,
            Node::Area(a) => &a.children,
            _ => return None,
        };
    }
    named_at(children, last, *last_index, 0)
}
