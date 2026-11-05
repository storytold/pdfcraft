//! PDF/A-2 6.6.2.3.3, the extension schema container schema: a packet describes each schema it
//! extends XMP with in `pdfaExtension:schemas`, a bag of schema descriptions, and each description
//! is complete.
//!
//! The entries each description needs, and the prefixes it is written with, are those of the
//! requirement as stated with the conformance corpus:
//! - a schema (`pdfaSchema`) has `schema`, `namespaceURI` and `prefix`; its `property` and
//!   `valueType` lists are optional;
//! - a property (`pdfaProperty`) has `name`, `valueType`, `category` and `description`;
//! - a value type (`pdfaType`) has `type`, `namespaceURI`, `prefix`, `description` and `field`;
//! - a field (`pdfaField`) has `name`, `valueType` and `description`;
//! - each of the four namespaces is written with its own prefix (`pdfaSchema`, `pdfaProperty`,
//!   `pdfaType`, `pdfaField`), as a PDF/A reader matches them by prefix.
//!
//! An entry counts as present written as an element or as an attribute of the description.

use std::collections::BTreeSet;

use roxmltree::{Document, Node};

const RDF: &str = "http://www.w3.org/1999/02/22-rdf-syntax-ns#";
const EXTENSION: &str = "http://www.aiim.org/pdfa/ns/extension/";
const SCHEMA: &str = "http://www.aiim.org/pdfa/ns/schema#";
const PROPERTY: &str = "http://www.aiim.org/pdfa/ns/property#";
const TYPE: &str = "http://www.aiim.org/pdfa/ns/type#";
const FIELD: &str = "http://www.aiim.org/pdfa/ns/field#";

/// Each description namespace with its prefix.
const PREFIXES: [(&str, &str); 4] = [(SCHEMA, "pdfaSchema"), (PROPERTY, "pdfaProperty"), (TYPE, "pdfaType"), (FIELD, "pdfaField")];

/// What the extension schema descriptions of a document break of 6.6.2.3.3.
#[derive(Debug, Default, PartialEq, Eq)]
pub(crate) struct Findings {
    /// Entries a description lacks, as `prefix:name`, in name order.
    pub(crate) missing: BTreeSet<String>,
    /// Description namespaces written with another prefix, by their own prefix, in name order.
    pub(crate) prefixes: BTreeSet<&'static str>,
}

impl Findings {
    /// Adds what the packet `doc` breaks.
    pub(crate) fn add(&mut self, doc: &Document<'_>) {
        let containers = doc.descendants().filter(|n| is(n, EXTENSION, "schemas"));
        for container in containers {
            self.prefixes_in(doc, &container);
            for schema in items(&container) {
                self.need(&schema, SCHEMA, "pdfaSchema", &["schema", "namespaceURI", "prefix"]);
                for property in entry(&schema, SCHEMA, "property").iter().flat_map(items) {
                    self.need(&property, PROPERTY, "pdfaProperty", &["name", "valueType", "category", "description"]);
                }
                for value_type in entry(&schema, SCHEMA, "valueType").iter().flat_map(items) {
                    self.need(&value_type, TYPE, "pdfaType", &["type", "namespaceURI", "prefix", "description", "field"]);
                    for field in entry(&value_type, TYPE, "field").iter().flat_map(items) {
                        self.need(&field, FIELD, "pdfaField", &["name", "valueType", "description"]);
                    }
                }
            }
        }
    }

    /// Notes the entries of `ns` in `names` that the description `d` lacks.
    fn need(&mut self, d: &Node<'_, '_>, ns: &str, prefix: &str, names: &[&str]) {
        for name in names {
            if entry(d, ns, name).is_none() && d.attribute((ns, *name)).is_none() {
                self.missing.insert(format!("{prefix}:{name}"));
            }
        }
    }

    /// Notes description namespaces written in `container` with another prefix than their own.
    fn prefixes_in(&mut self, doc: &Document<'_>, container: &Node<'_, '_>) {
        let text = doc.input_text();
        for n in container.descendants().filter(Node::is_element) {
            if let Some(own) = own_prefix(n.tag_name().namespace())
                && written_prefix(text.get(n.range()).unwrap_or_default().strip_prefix('<').unwrap_or_default()) != own
            {
                self.prefixes.insert(own);
            }
            for a in n.attributes() {
                if let Some(own) = own_prefix(a.namespace())
                    && written_prefix(text.get(a.range_qname()).unwrap_or_default()) != own
                {
                    self.prefixes.insert(own);
                }
            }
        }
    }
}

fn own_prefix(ns: Option<&str>) -> Option<&'static str> {
    PREFIXES.iter().find(|(uri, _)| Some(*uri) == ns).map(|(_, p)| *p)
}

/// The prefix of the qualified name `qname` starts with, empty when it has none.
fn written_prefix(qname: &str) -> &str {
    let name = qname.split(|c: char| c.is_whitespace() || c == '>' || c == '/').next().unwrap_or_default();
    name.split_once(':').map_or("", |(p, _)| p)
}

fn is(n: &Node<'_, '_>, ns: &str, name: &str) -> bool {
    n.is_element() && n.tag_name().namespace() == Some(ns) && n.tag_name().name() == name
}

/// The element entry `name` of `ns` in the description `d`.
fn entry<'a, 'i>(d: &Node<'a, 'i>, ns: &str, name: &str) -> Option<Node<'a, 'i>> {
    d.children().find(|c| is(c, ns, name))
}

/// The descriptions in the array `e` holds: each item's `rdf:parseType="Resource"` node or
/// `rdf:Description`.
fn items<'a, 'i>(e: &Node<'a, 'i>) -> Vec<Node<'a, 'i>> {
    let Some(array) = e.children().find(|c| is(c, RDF, "Bag") || is(c, RDF, "Seq") || is(c, RDF, "Alt")) else { return Vec::new() };
    array.children().filter(|i| is(i, RDF, "li")).map(|li| li.children().find(|c| is(c, RDF, "Description")).unwrap_or(li)).collect()
}
