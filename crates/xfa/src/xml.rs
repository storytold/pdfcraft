//! Lightweight namespace-aware XML DOM with source byte ranges, backed by `craft_xml`.

use std::ops::Range;

const XML_NS: &str = "http://www.w3.org/XML/1998/namespace";
const XMLNS_NS: &str = "http://www.w3.org/2000/xmlns/";

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) struct NodeId(pub(crate) usize);

#[derive(Clone, Debug)]
pub(crate) struct ParsingOptions {
    pub(crate) allow_dtd: bool,
    pub(crate) nodes_limit: u32,
}

impl Default for ParsingOptions {
    fn default() -> Self {
        Self { allow_dtd: false, nodes_limit: u32::MAX }
    }
}

#[derive(Clone, Debug)]
struct AttrData {
    ns: Option<String>,
    name: String,
    value: String,
}

#[derive(Clone, Debug)]
enum NodeKind {
    Element {
        ns: Option<String>,
        name: String,
        attrs: Vec<AttrData>,
        /// In-scope `(prefix, uri)` bindings at this element (`""` for default namespace).
        namespaces: Vec<(String, String)>,
        range: Range<usize>,
    },
    Text(String),
}

#[derive(Clone, Debug)]
struct NodeData {
    kind: NodeKind,
    children: Vec<usize>,
}

pub(crate) struct Document {
    nodes: Vec<NodeData>,
    root: usize,
}

#[derive(Clone, Copy)]
pub(crate) struct Node<'a> {
    doc: &'a Document,
    id: usize,
}

impl PartialEq for Node<'_> {
    fn eq(&self, other: &Self) -> bool {
        self.id == other.id && std::ptr::eq(self.doc, other.doc)
    }
}

impl Eq for Node<'_> {}

#[derive(Clone, Copy)]
pub(crate) struct ExpandedName<'a> {
    ns: Option<&'a str>,
    name: &'a str,
}

impl<'a> ExpandedName<'a> {
    pub(crate) fn name(&self) -> &'a str {
        self.name
    }

    pub(crate) fn namespace(&self) -> Option<&'a str> {
        self.ns
    }
}

pub(crate) trait AttributeKey {
    fn matches(&self, ns: Option<&str>, name: &str) -> bool;
}

impl AttributeKey for &str {
    fn matches(&self, ns: Option<&str>, name: &str) -> bool {
        ns.is_none() && name == *self
    }
}

impl AttributeKey for (&str, &str) {
    fn matches(&self, ns: Option<&str>, name: &str) -> bool {
        ns == Some(self.0) && name == self.1
    }
}

#[derive(Clone, Copy)]
pub(crate) struct Attribute<'a> {
    attr: &'a AttrData,
}

impl<'a> Attribute<'a> {
    pub(crate) fn name(&self) -> &'a str {
        &self.attr.name
    }

    pub(crate) fn namespace(&self) -> Option<&'a str> {
        self.attr.ns.as_deref()
    }

    pub(crate) fn value(&self) -> &'a str {
        &self.attr.value
    }
}

#[derive(Clone, Copy)]
pub(crate) struct Namespace<'a> {
    prefix: &'a str,
    uri: &'a str,
}

impl<'a> Namespace<'a> {
    pub(crate) fn name(&self) -> Option<&'a str> {
        (!self.prefix.is_empty()).then_some(self.prefix)
    }

    pub(crate) fn uri(&self) -> &'a str {
        self.uri
    }
}

fn split_qname(qname: &str) -> (&str, &str) {
    match qname.split_once(':') {
        Some((p, l)) => (p, l),
        None => ("", qname),
    }
}

fn resolve_ns(scope: &[(String, String)], prefix: &str, is_attr: bool) -> Result<Option<String>, String> {
    if prefix == "xml" {
        return Ok(Some(XML_NS.to_string()));
    }
    if prefix == "xmlns" {
        return Ok(Some(XMLNS_NS.to_string()));
    }
    if prefix.is_empty() {
        if is_attr {
            return Ok(None);
        }
        return Ok(scope.iter().rev().find(|(p, _)| p.is_empty()).and_then(|(_, u)| (!u.is_empty()).then(|| u.clone())));
    }
    match scope.iter().rev().find(|(p, _)| p == prefix) {
        Some((_, u)) if !u.is_empty() => Ok(Some(u.clone())),
        _ => Err(format!("undeclared namespace prefix: {prefix}")),
    }
}

impl Document {
    #[cfg(test)]
    pub(crate) fn parse(text: &str) -> Result<Self, String> {
        Self::parse_with_options(text, ParsingOptions::default())
    }

    pub(crate) fn parse_with_options(text: &str, options: ParsingOptions) -> Result<Self, String> {
        let mut opts = craft_xml::Options::default();
        opts.strict = true;
        opts.doctype = if options.allow_dtd { craft_xml::Doctype::Ignore } else { craft_xml::Doctype::Reject };
        opts.limits.max_nodes = options.nodes_limit as usize;
        opts.limits.max_depth = crate::MAX_XML_DEPTH;

        let mut events = opts.events(text.as_bytes());
        let mut nodes: Vec<NodeData> = Vec::new();
        let mut open: Vec<(usize, usize)> = Vec::new(); // (node_id, scope_len_before)
        let mut scope: Vec<(String, String)> = Vec::new();
        let mut root: Option<usize> = None;

        while let Some(ev) = events.next().map_err(|e| e.to_string())? {
            let span = events.span();
            match ev {
                craft_xml::Event::Start { name, attrs } => {
                    if nodes.len() >= options.nodes_limit as usize {
                        return Err("nodes limit reached".to_string());
                    }
                    let scope_mark = scope.len();
                    for (k, v) in &attrs {
                        if k == "xmlns" {
                            scope.push((String::new(), v.clone()));
                        } else if let Some(prefix) = k.strip_prefix("xmlns:") {
                            scope.push((prefix.to_string(), v.clone()));
                        }
                    }
                    let (el_prefix, el_local) = split_qname(&name);
                    let el_ns = resolve_ns(&scope, el_prefix, false)?;
                    let mut el_attrs = Vec::new();
                    for (k, v) in attrs {
                        if k == "xmlns" || k.starts_with("xmlns:") {
                            continue;
                        }
                        let (ap, al) = split_qname(&k);
                        let ans = resolve_ns(&scope, ap, true)?;
                        el_attrs.push(AttrData { ns: ans, name: al.to_string(), value: v });
                    }
                    // Deduplicate in-scope namespace bindings, keeping the innermost binding per prefix.
                    let mut namespaces: Vec<(String, String)> = Vec::new();
                    for (p, u) in scope.iter().rev() {
                        if !namespaces.iter().any(|(ep, _)| ep == p) {
                            namespaces.push((p.clone(), u.clone()));
                        }
                    }
                    namespaces.retain(|(_, u)| !u.is_empty());
                    let id = nodes.len();
                    nodes.push(NodeData {
                        kind: NodeKind::Element { ns: el_ns, name: el_local.to_string(), attrs: el_attrs, namespaces, range: span.start..span.end },
                        children: Vec::new(),
                    });
                    if let Some(&(parent, _)) = open.last() {
                        nodes[parent].children.push(id);
                    } else if root.is_none() {
                        root = Some(id);
                    }
                    open.push((id, scope_mark));
                }
                craft_xml::Event::End { .. } => {
                    if let Some((id, scope_mark)) = open.pop() {
                        if let NodeKind::Element { range, .. } = &mut nodes[id].kind {
                            range.end = span.end;
                        }
                        scope.truncate(scope_mark);
                    }
                }
                craft_xml::Event::Text(t) | craft_xml::Event::CData(t) => {
                    if let Some(&(parent, _)) = open.last() {
                        if let Some(&last_child) = nodes[parent].children.last()
                            && let NodeKind::Text(existing) = &mut nodes[last_child].kind
                        {
                            existing.push_str(&t);
                            continue;
                        }
                        if nodes.len() >= options.nodes_limit as usize {
                            return Err("nodes limit reached".to_string());
                        }
                        let id = nodes.len();
                        nodes.push(NodeData { kind: NodeKind::Text(t), children: Vec::new() });
                        nodes[parent].children.push(id);
                    }
                }
                craft_xml::Event::Pi { .. } | craft_xml::Event::Comment(_) => {}
            }
        }

        let Some(root) = root else {
            return Err("empty XML document".to_string());
        };
        Ok(Self { nodes, root })
    }

    pub(crate) fn root_element(&self) -> Node<'_> {
        Node { doc: self, id: self.root }
    }

    pub(crate) fn get_node(&self, id: NodeId) -> Option<Node<'_>> {
        (id.0 < self.nodes.len()).then_some(Node { doc: self, id: id.0 })
    }

    pub(crate) fn descendants(&self) -> Descendants<'_> {
        Descendants { doc: self, stack: vec![self.root] }
    }
}

pub(crate) struct Descendants<'a> {
    doc: &'a Document,
    stack: Vec<usize>,
}

impl<'a> Iterator for Descendants<'a> {
    type Item = Node<'a>;

    fn next(&mut self) -> Option<Self::Item> {
        let id = self.stack.pop()?;
        let children = &self.doc.nodes[id].children;
        for &c in children.iter().rev() {
            self.stack.push(c);
        }
        Some(Node { doc: self.doc, id })
    }
}

impl<'a> Node<'a> {
    pub(crate) fn id(&self) -> NodeId {
        NodeId(self.id)
    }

    pub(crate) fn document(&self) -> &'a Document {
        self.doc
    }

    pub(crate) fn is_element(&self) -> bool {
        matches!(self.doc.nodes[self.id].kind, NodeKind::Element { .. })
    }

    pub(crate) fn is_text(&self) -> bool {
        matches!(self.doc.nodes[self.id].kind, NodeKind::Text(_))
    }

    pub(crate) fn tag_name(&self) -> ExpandedName<'a> {
        match &self.doc.nodes[self.id].kind {
            NodeKind::Element { ns, name, .. } => ExpandedName { ns: ns.as_deref(), name },
            NodeKind::Text(_) => ExpandedName { ns: None, name: "" },
        }
    }

    pub(crate) fn attribute<K: AttributeKey>(&self, key: K) -> Option<&'a str> {
        match &self.doc.nodes[self.id].kind {
            NodeKind::Element { attrs, .. } => attrs.iter().find(|a| key.matches(a.ns.as_deref(), &a.name)).map(|a| a.value.as_str()),
            NodeKind::Text(_) => None,
        }
    }

    pub(crate) fn attributes(&self) -> impl ExactSizeIterator<Item = Attribute<'a>> + use<'a> {
        let attrs = match &self.doc.nodes[self.id].kind {
            NodeKind::Element { attrs, .. } => attrs.as_slice(),
            NodeKind::Text(_) => &[],
        };
        attrs.iter().map(|attr| Attribute { attr })
    }

    pub(crate) fn namespaces(&self) -> impl ExactSizeIterator<Item = Namespace<'a>> + use<'a> {
        let ns = match &self.doc.nodes[self.id].kind {
            NodeKind::Element { namespaces, .. } => namespaces.as_slice(),
            NodeKind::Text(_) => &[],
        };
        ns.iter().map(|(p, u)| Namespace { prefix: p, uri: u })
    }

    pub(crate) fn lookup_prefix(&self, uri: &str) -> Option<&'a str> {
        if uri == XML_NS {
            return Some("xml");
        }
        match &self.doc.nodes[self.id].kind {
            NodeKind::Element { namespaces, .. } => namespaces.iter().find(|(_, u)| u == uri).map(|(p, _)| p.as_str()),
            NodeKind::Text(_) => None,
        }
    }

    pub(crate) fn lookup_namespace_uri(&self, prefix: Option<&str>) -> Option<&'a str> {
        if prefix == Some("xml") {
            return Some(XML_NS);
        }
        if prefix == Some("xmlns") {
            return Some(XMLNS_NS);
        }
        let target = prefix.unwrap_or("");
        match &self.doc.nodes[self.id].kind {
            NodeKind::Element { namespaces, .. } => namespaces.iter().find(|(p, _)| p == target).map(|(_, u)| u.as_str()),
            NodeKind::Text(_) => None,
        }
    }

    pub(crate) fn children(&self) -> impl Iterator<Item = Node<'a>> + use<'a> {
        let doc = self.doc;
        doc.nodes[self.id].children.iter().map(move |&id| Node { doc, id })
    }

    pub(crate) fn descendants(&self) -> Descendants<'a> {
        Descendants { doc: self.doc, stack: vec![self.id] }
    }

    pub(crate) fn text(&self) -> Option<&'a str> {
        match &self.doc.nodes[self.id].kind {
            NodeKind::Text(t) => Some(t.as_str()),
            NodeKind::Element { .. } => self.doc.nodes[self.id].children.first().and_then(|&cid| match &self.doc.nodes[cid].kind {
                NodeKind::Text(t) => Some(t.as_str()),
                _ => None,
            }),
        }
    }

    pub(crate) fn range(&self) -> Range<usize> {
        match &self.doc.nodes[self.id].kind {
            NodeKind::Element { range, .. } => range.clone(),
            NodeKind::Text(_) => 0..0,
        }
    }
}
