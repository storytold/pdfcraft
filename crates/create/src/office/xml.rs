//! Small helpers over `roxmltree` for OOXML parts. Elements and attributes are matched by local
//! name, so Transitional and Strict documents (which differ only in namespace URIs) read alike.
//! The one place two attributes share a local name, `id` and `r:id`, has its own helper.

use std::collections::{HashMap, HashSet};

use roxmltree::{Document, Node, ParsingOptions};

/// Most XML nodes one part may hold; a larger part is refused rather than held in memory.
const MAX_NODES: u32 = 8_000_000;

pub(crate) fn parse(text: &str) -> Result<Document<'_>, String> {
    // DTDs stay disallowed (the default): OOXML never uses one, and entities are an expansion risk.
    Document::parse_with_options(text, ParsingOptions { allow_dtd: false, nodes_limit: MAX_NODES, ..Default::default() })
        .map_err(|e| format!("malformed XML ({e})"))
}

pub(crate) fn is(n: Node<'_, '_>, name: &str) -> bool {
    n.is_element() && n.tag_name().name() == name
}

/// The first child element called `name`.
pub(crate) fn child<'a, 'i>(n: Node<'a, 'i>, name: &str) -> Option<Node<'a, 'i>> {
    n.children().find(|c| is(*c, name))
}

/// Child elements called `name`.
pub(crate) fn children<'a, 'i: 'a>(n: Node<'a, 'i>, name: &'a str) -> impl Iterator<Item = Node<'a, 'i>> + 'a {
    n.children().filter(move |c| is(*c, name))
}

/// Follow a path of child element names.
pub(crate) fn path<'a, 'i>(n: Node<'a, 'i>, names: &[&str]) -> Option<Node<'a, 'i>> {
    names.iter().try_fold(n, |n, name| child(n, name))
}

/// An attribute by local name, outside the relationships namespace.
pub(crate) fn attr<'a>(n: Node<'a, '_>, name: &str) -> Option<&'a str> {
    n.attributes().find(|a| a.name() == name && !a.namespace().is_some_and(is_rel_ns)).map(|a| a.value())
}

/// An attribute in the relationships namespace (`r:id`, `r:embed`, `r:link`).
pub(crate) fn rel_attr<'a>(n: Node<'a, '_>, name: &str) -> Option<&'a str> {
    n.attributes().find(|a| a.name() == name && a.namespace().is_some_and(is_rel_ns)).map(|a| a.value())
}

fn is_rel_ns(ns: &str) -> bool {
    ns.ends_with("/officeDocument/2006/relationships") || ns.ends_with("/officeDocument/relationships")
}

pub(crate) fn attr_f64(n: Node<'_, '_>, name: &str) -> Option<f64> {
    attr(n, name).and_then(|v| v.trim().parse::<f64>().ok()).filter(|v| v.is_finite())
}

/// An OOXML boolean (`true`/`1`/`on`; an element with no value counts as true).
pub(crate) fn flag(v: Option<&str>) -> bool {
    !matches!(v.map(str::trim), Some("0" | "false" | "off" | "none"))
}

/// The relationships of a part: id → (resolved target part, external, type), with an index by id
/// so each lookup is one hash probe however many relationships the part lists.
pub(crate) struct Rels {
    list: Vec<(String, String, bool, String)>,
    /// The first internal relationship with each id, the first with each id, and the ids that
    /// have an external one: what the lookups below find.
    internal: HashMap<String, usize>,
    first: HashMap<String, usize>,
    external: HashSet<String>,
}

impl Rels {
    pub(crate) fn load(zip: &super::zip::Zip<'_>, part: &str) -> Result<Rels, String> {
        let part = part.trim_start_matches('/');
        let rels_name = match part.rsplit_once('/') {
            Some((dir, file)) => format!("{dir}/_rels/{file}.rels"),
            None => format!("_rels/{part}.rels"),
        };
        let Some(text) = zip.text(&rels_name)? else { return Ok(Rels::new(Vec::new())) };
        let doc = parse(&text).map_err(|e| format!("{rels_name}: {e}"))?;
        let mut out = Vec::new();
        for r in doc.root_element().children().filter(|n| is(*n, "Relationship")) {
            let (Some(id), Some(target)) = (attr(r, "Id"), attr(r, "Target")) else { continue };
            let external = attr(r, "TargetMode").is_some_and(|m| m.eq_ignore_ascii_case("External"));
            let ty = attr(r, "Type").unwrap_or_default().rsplit('/').next().unwrap_or_default().to_string();
            let resolved = if external { target.to_string() } else { super::zip::resolve(part, target) };
            out.push((id.to_string(), resolved, external, ty));
        }
        Ok(Rels::new(out))
    }

    fn new(list: Vec<(String, String, bool, String)>) -> Rels {
        let (mut internal, mut first, mut external) = (HashMap::new(), HashMap::new(), HashSet::new());
        for (k, (id, _, ext, _)) in list.iter().enumerate() {
            first.entry(id.clone()).or_insert(k);
            if *ext {
                external.insert(id.clone());
            } else {
                internal.entry(id.clone()).or_insert(k);
            }
        }
        Rels { list, internal, first, external }
    }

    /// The internal part an id points at.
    pub(crate) fn target(&self, id: &str) -> Option<&str> {
        self.internal.get(id).and_then(|k| self.list.get(*k)).map(|(_, t, _, _)| t.as_str())
    }

    /// The first internal part of a relationship type (the last segment of its URI, e.g.
    /// `officeDocument`, `styles`, `theme`).
    pub(crate) fn of_type(&self, ty: &str) -> Option<&str> {
        self.list.iter().find(|(_, _, ext, t)| t == ty && !ext).map(|(_, t, _, _)| t.as_str())
    }

    /// The relationship type of an id (the last segment of its URI, e.g. `worksheet`).
    pub(crate) fn type_of(&self, id: &str) -> Option<&str> {
        self.first.get(id).and_then(|k| self.list.get(*k)).map(|(.., t)| t.as_str())
    }

    /// Whether the id names an external target (a linked, not embedded, resource).
    pub(crate) fn is_external(&self, id: &str) -> bool {
        self.external.contains(id)
    }
}

/// The text content of a node's descendants called `t` (w:t, a:t, the shared-string t).
pub(crate) fn text_of(n: Node<'_, '_>, t: &str) -> String {
    n.descendants().filter(|d| is(*d, t)).filter_map(|d| d.text()).collect()
}
