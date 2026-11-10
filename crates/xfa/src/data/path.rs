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
fn page_area_object<'a>(tpl: &'a Template, segments: &[(String, usize)]) -> Option<(&'a Node, usize)> {
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
    Some((target?, *instance))
}

/// Whether `c` has `<bind match="global"/>`.
fn has_global_bind(c: &crate::model::Common) -> bool {
    c.props.elements.iter().any(|p| p.tag == "bind" && p.attrs.iter().any(|(k, v)| k == "match" && v.trim() == "global"))
}

fn page_area_index(tpl: &Template, segments: &[(String, usize)]) -> Option<usize> {
    let (target, instance) = page_area_object(tpl, segments)?;
    let shared = match target {
        Node::Field(f) => f.data_shared,
        Node::ExclGroup(g) => {
            has_global_bind(&g.common)
                || g.common.props.elements.iter().any(|p| p.tag == "bind" && p.attrs.iter().any(|(k, v)| k == "ref" && !v.trim().is_empty()))
        }
        _ => false,
    };
    Some(if shared { 0 } else { instance })
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

/// The first leaf data node named `target` in `n`, in document order, bounded by [`MAX_DEPTH`].
fn find_named_leaf(n: &DataNode, target: &str, path: &mut DataPath, depth: usize) -> Option<DataPath> {
    if depth > MAX_DEPTH {
        return None;
    }
    let mut counts: std::collections::HashMap<&str, usize> = std::collections::HashMap::new();
    for c in &n.children {
        let idx = counts.entry(c.name.as_str()).or_insert(0);
        let cur = *idx;
        *idx += 1;
        path.push((c.name.clone(), cur));
        if c.name == target && !c.group && c.children.is_empty() {
            return Some(path.clone());
        }
        if let Some(found) = find_named_leaf(c, target, path, depth + 1) {
            return Some(found);
        }
        path.pop();
    }
    None
}

/// The data path the object at `som` binds to in `data`: [`data_path`], or for an exclusion
/// group with `<bind match="global"/>` that has no match in its enclosing data scope, the
/// matching node in the top-level data scope (`$record.<name>`) or elsewhere in `data` (XFA 3.3
/// part 1, "Global Binding").
pub(crate) fn bound_data_path(tpl: &Template, data: Option<&DataNode>, som: &str) -> DataPath {
    let path = data_path(tpl, som);
    let Some(d) = data else { return path };
    if d.get(&path).is_some() {
        return path;
    }
    let obj = template_object(tpl, som).or_else(|| page_area_object(tpl, &som_to_path(som)).map(|(n, _)| n));
    if !matches!(obj, Some(Node::ExclGroup(g)) if has_global_bind(&g.common)) {
        return path;
    }
    let (Some(first), Some((last_name, _))) = (path.first(), path.last()) else {
        return path;
    };
    let top = vec![first.clone(), (last_name.clone(), 0)];
    if d.get(&top).is_some() {
        return top;
    }
    if let Some(found) = find_named_leaf(d, last_name, &mut DataPath::new(), 0) {
        return found;
    }
    if path.len() > 2 && d.get(std::slice::from_ref(first)).is_some() { top } else { path }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::data::{FieldData, FieldDatum, read_values_with, write_datasets_with};
    use crate::layout::WidgetKind;
    use crate::script::{NodeKind, Overrides, form_tree};

    const GLOBAL_EXCL_XDP: &str = r#"<xdp:xdp xmlns:xdp="http://ns.adobe.com/xdp/">
<template xmlns="http://www.xfa.org/schema/xfa-template/3.3/">
  <subform name="form1" layout="tb">
    <pageSet>
      <pageArea name="Page1" id="Page1">
        <contentArea x="0.25in" y="0.25in" w="8in" h="10.5in"/>
        <medium stock="letter" short="8.5in" long="11in"/>
      </pageArea>
    </pageSet>
    <subform name="Section1" layout="tb">
      <exclGroup name="LikesPizza" layout="tb">
        <bind match="global"/>
        <field name="rbYes" w="2in" h="0.25in">
          <ui><checkButton shape="round"/></ui>
          <items><text>Yes</text></items>
        </field>
        <field name="rbNo" w="2in" h="0.25in">
          <ui><checkButton shape="round"/></ui>
          <items><text>No</text></items>
        </field>
      </exclGroup>
    </subform>
  </subform>
</template>
<xfa:datasets xmlns:xfa="http://www.xfa.org/schema/xfa-data/1.0/">
  <xfa:data>
    <form1>
      <Section1/>
      <LikesPizza>Yes</LikesPizza>
    </form1>
  </xfa:data>
</xfa:datasets>
</xdp:xdp>"#;

    #[test]
    fn global_excl_group_selects_bound_value_from_outer_scope() -> Result<(), Box<dyn std::error::Error>> {
        let (tpl, _) = crate::parse(GLOBAL_EXCL_XDP)?;
        let data = parse_datasets(GLOBAL_EXCL_XDP).ok_or("datasets missing")?;
        let som = "form1[0].Section1[0].LikesPizza[0]";

        assert_eq!(bound_data_path(&tpl, Some(&data), som), vec![("form1".into(), 0), ("LikesPizza".into(), 0)]);

        // Layout selects the "Yes" radio button.
        let form = crate::layout::layout(&tpl, Some(&data))?;
        let page = form.pages.first().ok_or("no page")?;
        let yes_value = page.items.iter().find_map(|it| match it {
            crate::layout::Item::Widget(w) => match &w.kind {
                WidgetKind::Radio { group, on, .. } if group == "LikesPizza" && on == "Yes" => Some(w.value.clone()),
                _ => None,
            },
            _ => None,
        });
        assert_eq!(yes_value, Some(Some("Yes".into())));

        // Live form tree exposes "Yes" on both the exclusion group and its selected child field.
        let live = form_tree(&tpl, Some(&data), &Overrides::default());
        let sec = live.root.children.iter().find(|c| c.name == "Section1").ok_or("Section1 missing")?;
        let grp = sec.children.iter().find(|c| c.kind == Some(NodeKind::ExclGroup)).ok_or("LikesPizza missing")?;
        assert_eq!(grp.value, "Yes");
        let rb_yes = grp.children.iter().find(|c| c.name == "rbYes").ok_or("rbYes missing")?;
        let rb_no = grp.children.iter().find(|c| c.name == "rbNo").ok_or("rbNo missing")?;
        assert_eq!(rb_yes.value, "Yes");
        assert_eq!(rb_no.value, "");

        // Reading and writing datasets through AcroForm FieldDatum preserves the global path.
        let mut doc = pdfcraft_cos::Document::new_empty();
        let xdp_ref = doc.add(pdfcraft_cos::Object::Stream(pdfcraft_cos::Stream::flate(pdfcraft_cos::Dict::new(), GLOBAL_EXCL_XDP.as_bytes())));
        let mut acro = pdfcraft_cos::Dict::new();
        acro.set(b"XFA".to_vec(), pdfcraft_cos::Object::Ref(xdp_ref));
        let mut cat = pdfcraft_cos::Dict::new();
        cat.set(b"Type".to_vec(), pdfcraft_cos::Object::Name(b"Catalog".to_vec()));
        cat.set(b"AcroForm".to_vec(), pdfcraft_cos::Object::Dict(acro));
        let cat_ref = doc.add(pdfcraft_cos::Object::Dict(cat));
        doc.trailer_mut().set(b"Root".to_vec(), pdfcraft_cos::Object::Ref(cat_ref));
        let _ = crate::pdf::write_form(&mut doc, &form)?;

        let field_map = crate::script::fields_by_som(&doc);
        let field_name = field_map.get(som).ok_or("generated radio group missing")?.clone();
        let acro_fields = doc
            .get(cat_ref)
            .as_dict()
            .and_then(|c| c.get(b"AcroForm"))
            .map(|a| doc.resolve(a))
            .and_then(|a| a.as_dict().and_then(|d| d.get(b"Fields")).map(|f| doc.resolve(f)))
            .and_then(|f| f.as_array().cloned())
            .ok_or("Fields array missing")?;
        let group_ref = acro_fields
            .iter()
            .filter_map(pdfcraft_cos::Object::as_ref)
            .find(|r| {
                doc.get(*r).as_dict().and_then(|d| d.get(crate::pdf::SOM_KEY)).and_then(|s| s.as_string()).map(|s| s.to_text())
                    == Some(som.to_string())
            })
            .ok_or("group object ref missing")?;

        let query = [FieldDatum { obj: group_ref, name: field_name.clone(), data: FieldData::Radio(None) }];
        let read = read_values_with(&doc, Some(&tpl), &query);
        assert_eq!(read, vec![(field_name.clone(), FieldData::Radio(Some("Yes".into())))]);

        // Updating the selection to "No" updates `form1.LikesPizza` in place rather than adding `form1.Section1.LikesPizza`.
        let update = [FieldDatum { obj: group_ref, name: field_name.clone(), data: FieldData::Radio(Some("No".into())) }];
        let written = write_datasets_with(&mut doc, Some(&tpl), &update, None)?;
        assert!(written.written);
        let after = crate::script::data_of(&doc).ok_or("updated data missing")?;
        assert_eq!(after.text_at(&[("form1".into(), 0), ("LikesPizza".into(), 0)]), Some("No"));
        assert_eq!(after.text_at(&[("form1".into(), 0), ("Section1".into(), 0), ("LikesPizza".into(), 0)]), None);
        Ok(())
    }

    #[test]
    fn global_excl_group_prefers_local_scope_when_present() -> Result<(), Box<dyn std::error::Error>> {
        let xdp = GLOBAL_EXCL_XDP.replace(
            "<Section1/>\n      <LikesPizza>Yes</LikesPizza>",
            "<Section1><LikesPizza>No</LikesPizza></Section1>\n      <LikesPizza>Yes</LikesPizza>",
        );
        let (tpl, _) = crate::parse(&xdp)?;
        let data = parse_datasets(&xdp).ok_or("datasets missing")?;
        let som = "form1[0].Section1[0].LikesPizza[0]";
        assert_eq!(bound_data_path(&tpl, Some(&data), som), vec![("form1".into(), 0), ("Section1".into(), 0), ("LikesPizza".into(), 0)]);
        Ok(())
    }
}
