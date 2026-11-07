//! PDF/A-2 6.6.2.3.3, the extension schema container schema: a packet describes each schema it
//! extends XMP with in `pdfaExtension:schemas`, a bag of schema descriptions, and each description
//! is complete.
//!
//! The schemas, their entries and their prefixes are those of PDF Association TechNote 0009, "XMP
//! Extension Schemas in PDF/A-1" (2008), section 4, for PDF/A-2 6.6.2.3.3:
//! - a schema (`pdfaSchema`, 4.3) has `schema`, `namespaceURI` and `prefix`; its `property` and
//!   `valueType` lists are optional. TechNote 0009, written for PDF/A-1, calls `schema` an
//!   optional description; PDF/A-2 requires it, as its conformance tests show;
//! - a property (`pdfaProperty`, 4.4) has `name`, `valueType`, `category` and `description`;
//! - a value type (`pdfaType`, 4.5) has `type`, `namespaceURI`, `prefix`, `description` and
//!   `field`. TechNote 0009 calls `field` optional; PDF/A-2 requires it, as its conformance tests
//!   show;
//! - a field (`pdfaField`, 4.6) has `name`, `valueType` and `description`;
//! - each of the five namespaces is written with its own prefix (`pdfaExtension`, `pdfaSchema`,
//!   `pdfaProperty`, `pdfaType`, `pdfaField`): "Unlike predefined XMP schemas, the namespace
//!   prefixes are not only preferred, but required" (4.1).
//!
//! An entry counts as present written as an element or as an attribute of the description. An
//! entry that names something is empty when it holds no value: a namespace URI or prefix, a
//! property, value type or field name, a value type reference, a property category. TechNote 0009
//! asks for names that are valid XML names (4.4, 4.6) and a category that is internal or external
//! (4.4), and an empty prefix or namespace binds nothing. The human-readable descriptions
//! (`pdfaSchema:schema` and the `description` entries) are Text and may be empty.

use pdfcraft_cos::ObjRef;
use roxmltree::{Document, Node};

const RDF: &str = "http://www.w3.org/1999/02/22-rdf-syntax-ns#";
const EXTENSION: &str = "http://www.aiim.org/pdfa/ns/extension/";
const SCHEMA: &str = "http://www.aiim.org/pdfa/ns/schema#";
const PROPERTY: &str = "http://www.aiim.org/pdfa/ns/property#";
const TYPE: &str = "http://www.aiim.org/pdfa/ns/type#";
const FIELD: &str = "http://www.aiim.org/pdfa/ns/field#";

/// Each container namespace with its required prefix (TechNote 0009, 4.1).
const PREFIXES: [(&str, &str); 5] =
    [(EXTENSION, "pdfaExtension"), (SCHEMA, "pdfaSchema"), (PROPERTY, "pdfaProperty"), (TYPE, "pdfaType"), (FIELD, "pdfaField")];

/// How many findings a document's packets are read for. A document past this many has plainly
/// failed the clause, and listing every one of a hostile packet's descriptions helps nobody.
const MAX_FINDINGS: usize = 1000;
/// How much of a name the packet gives (a namespace URI, a property) a finding repeats.
const MAX_SHOWN_CHARS: usize = 200;

/// What the extension schema descriptions of a document break of 6.6.2.3.3, in the order the
/// metadata streams and their descriptions come.
#[derive(Debug, Default, PartialEq, Eq)]
pub(crate) struct Findings {
    /// Entries descriptions lack.
    pub(crate) missing: Vec<Missing>,
    /// Description namespaces written with another prefix, each once per stream and prefix.
    pub(crate) prefixes: Vec<Prefix>,
    /// `pdfaExtension:schemas` containers that aren't one `rdf:Bag` (TechNote 0009, 4.2: "bag
    /// Schema").
    pub(crate) not_bag: Vec<NotBag>,
}

/// A description without an entry it needs, or with the entry empty.
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct Missing {
    /// The metadata stream.
    pub(crate) object: ObjRef,
    /// The schema the description is in: its `pdfaSchema:namespaceURI`, or `#n` for the n-th
    /// schema of its container when it has none.
    pub(crate) schema: String,
    /// The property, value type or field description, when it is one of those and not the schema
    /// description itself: its `pdfaProperty:name` or `pdfaType:type`, a field as `type/name`, or
    /// `#n` for the n-th of its list when it has no name.
    pub(crate) item: Option<String>,
    /// The entry it lacks or holds empty, as `prefix:name`.
    pub(crate) entry: String,
    /// Whether the entry is there but empty, rather than absent.
    pub(crate) empty: bool,
}

/// A description namespace written with another prefix than its own.
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct Prefix {
    pub(crate) object: ObjRef,
    pub(crate) namespace: &'static str,
    /// The prefix it needs.
    pub(crate) own: &'static str,
    /// The prefix it is written with, empty when it is written as the default namespace.
    pub(crate) written: String,
}

/// A `pdfaExtension:schemas` container that isn't one `rdf:Bag`.
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct NotBag {
    pub(crate) object: ObjRef,
    /// The elements it holds instead, by their names as the packet writes them, comma-separated;
    /// empty when it holds none.
    pub(crate) found: String,
}

/// Where a description is: its stream, its schema and, below the schema, its own name.
struct Place {
    object: ObjRef,
    schema: String,
    item: Option<String>,
}

impl Findings {
    /// Adds what the packet `doc`, from the metadata stream `object`, breaks.
    pub(crate) fn add(&mut self, object: ObjRef, doc: &Document<'_>) {
        let containers = doc.descendants().filter(|n| is(n, EXTENSION, "schemas"));
        for container in containers {
            self.prefixes_in(object, doc, &container);
            let children: Vec<Node<'_, '_>> = container.children().filter(Node::is_element).collect();
            if !matches!(children.as_slice(), [bag] if is(bag, RDF, "Bag")) && self.room() {
                let found = children.iter().take(MAX_SHOWN_CHARS).map(|c| written_name(doc, c)).collect::<Vec<_>>().join(", ");
                self.not_bag.push(NotBag { object, found: shown(&found) });
            }
            for (n, schema) in items(&container).iter().enumerate() {
                let id = value(schema, SCHEMA, "namespaceURI").unwrap_or_else(|| position(n));
                let at = |item: Option<String>| Place { object, schema: id.clone(), item };
                self.need(&at(None), schema, SCHEMA, "pdfaSchema", &["schema", "namespaceURI", "prefix"]);
                let properties = entries(schema, SCHEMA, "property").flat_map(|e| items(&e));
                for (n, property) in properties.enumerate() {
                    let name = value(&property, PROPERTY, "name").unwrap_or_else(|| position(n));
                    self.need(&at(Some(name)), &property, PROPERTY, "pdfaProperty", &["name", "valueType", "category", "description"]);
                }
                let value_types = entries(schema, SCHEMA, "valueType").flat_map(|e| items(&e));
                for (n, value_type) in value_types.enumerate() {
                    let name = value(&value_type, TYPE, "type").unwrap_or_else(|| position(n));
                    self.need(&at(Some(name.clone())), &value_type, TYPE, "pdfaType", &["type", "namespaceURI", "prefix", "description", "field"]);
                    for (n, field) in entries(&value_type, TYPE, "field").flat_map(|e| items(&e)).enumerate() {
                        let field_name = value(&field, FIELD, "name").unwrap_or_else(|| position(n));
                        self.need(&at(Some(format!("{name}/{field_name}"))), &field, FIELD, "pdfaField", &["name", "valueType", "description"]);
                    }
                }
            }
        }
    }

    /// Whether another finding may be recorded (see [`MAX_FINDINGS`]).
    fn room(&self) -> bool {
        self.missing.len().saturating_add(self.prefixes.len()).saturating_add(self.not_bag.len()) < MAX_FINDINGS
    }

    /// Notes the entries of `ns` in `names` that the description `d`, at `at`, lacks, or holds
    /// empty when they are entries that name something (see [`NAMING`]).
    fn need(&mut self, at: &Place, d: &Node<'_, '_>, ns: &str, prefix: &str, names: &[&str]) {
        for name in names {
            let element = entry(d, ns, name);
            let attribute = d.attribute((ns, *name));
            let empty = match (element, attribute) {
                (None, None) => false,
                (Some(e), _) => NAMING.contains(name) && is_empty(&e),
                (None, Some(a)) => NAMING.contains(name) && a.trim().is_empty(),
            };
            if (empty || (element.is_none() && attribute.is_none())) && self.room() {
                let Place { object, schema, item } = at;
                let entry = format!("{prefix}:{name}");
                self.missing.push(Missing { object: *object, schema: schema.clone(), item: item.clone(), entry, empty });
            }
        }
    }

    /// Notes container namespaces written in `container` (itself included) with another prefix
    /// than their own.
    fn prefixes_in(&mut self, object: ObjRef, doc: &Document<'_>, container: &Node<'_, '_>) {
        let text = doc.input_text();
        for n in container.descendants().filter(Node::is_element) {
            let element = (n.tag_name().namespace(), text.get(n.range()).unwrap_or_default().strip_prefix('<').unwrap_or_default());
            let attributes = n.attributes().map(|a| (a.namespace(), text.get(a.range_qname()).unwrap_or_default()));
            for (ns, qname) in std::iter::once(element).chain(attributes) {
                let Some(&(namespace, own)) = PREFIXES.iter().find(|(uri, _)| Some(*uri) == ns) else { continue };
                let written = shown(written_prefix(qname));
                if written != own
                    && self.room()
                    && !self.prefixes.iter().any(|p| p.object == object && p.namespace == namespace && p.written == written)
                {
                    self.prefixes.push(Prefix { object, namespace, own, written });
                }
            }
        }
    }
}

/// The entries that name something, which an empty value leaves without a name: namespace URIs
/// and prefixes, property, value type and field names, value type references and the property
/// category.
const NAMING: [&str; 6] = ["namespaceURI", "prefix", "name", "type", "valueType", "category"];

/// Whether the element entry `e` holds no value: no element, no text but white space, and no
/// `rdf:resource` or `rdf:value` attribute with a value.
fn is_empty(e: &Node<'_, '_>) -> bool {
    let blank = |v: Option<&str>| v.is_none_or(|v| v.trim().is_empty());
    !e.children().any(|c| c.is_element() || (c.is_text() && c.text().is_some_and(|t| !t.trim().is_empty())))
        && blank(e.attribute((RDF, "resource")))
        && blank(e.attribute((RDF, "value")))
}

/// `#n` for the 0-based position `n`, counted from 1.
fn position(n: usize) -> String {
    format!("#{}", n.saturating_add(1))
}

/// At most [`MAX_SHOWN_CHARS`] of `s`, cut at a char boundary, with `…` when cut.
fn shown(s: &str) -> String {
    let mut out: String = s.chars().take(MAX_SHOWN_CHARS).collect();
    if out.len() < s.len() {
        out.push('…');
    }
    out
}

/// The text of the entry `name` of `ns` in the description `d`, written as an element or an
/// attribute, trimmed and [`shown`]; `None` when it is absent or blank.
fn value(d: &Node<'_, '_>, ns: &str, name: &str) -> Option<String> {
    let text = entry(d, ns, name).and_then(|e| e.text()).or_else(|| d.attribute((ns, name)))?.trim();
    (!text.is_empty()).then(|| shown(text))
}

/// The name of the element `n` as the packet writes it, prefix included.
fn written_name<'i>(doc: &Document<'i>, n: &Node<'_, '_>) -> &'i str {
    qualified_name(doc.input_text().get(n.range()).unwrap_or_default().strip_prefix('<').unwrap_or_default())
}

/// The qualified name the text of a start tag (after its `<`) or of an attribute begins with.
fn qualified_name(text: &str) -> &str {
    text.split(|c: char| c.is_whitespace() || c == '>' || c == '/' || c == '=').next().unwrap_or_default()
}

/// The prefix of the qualified name `text` begins with, empty when it has none.
fn written_prefix(text: &str) -> &str {
    qualified_name(text).split_once(':').map_or("", |(p, _)| p)
}

fn is(n: &Node<'_, '_>, ns: &str, name: &str) -> bool {
    n.is_element() && n.tag_name().namespace() == Some(ns) && n.tag_name().name() == name
}

/// Every element entry `name` of `ns` in the description `d`: a list written twice is read twice.
fn entries<'a, 'i>(d: &Node<'a, 'i>, ns: &'a str, name: &'a str) -> impl Iterator<Item = Node<'a, 'i>> {
    d.children().filter(move |c| is(c, ns, name))
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
