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

use crate::xmp::NsScope;
use craft_xml::El;
use pdfcraft_cos::ObjRef;

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
    pub(crate) fn add(&mut self, object: ObjRef, doc: &El) {
        fn walk_containers<'a>(el: &'a El, parent: &NsScope<'a>, out: &mut Vec<(&'a El, NsScope<'a>)>) {
            let scope = parent.child(el);
            if scope.is(el, EXTENSION, "schemas") {
                out.push((el, scope.clone()));
            }
            for child in el.elements() {
                walk_containers(child, &scope, out);
            }
        }
        let mut containers = Vec::new();
        walk_containers(doc, &NsScope::root(), &mut containers);
        for (container, cscope) in containers {
            self.prefixes_in(object, container, &cscope);
            let children: Vec<(&El, NsScope<'_>)> = container.elements().map(|c| (c, cscope.child(c))).collect();
            if !matches!(children.as_slice(), [(bag, bscope)] if is(bag, bscope, RDF, "Bag")) && self.room() {
                let found = children.iter().take(MAX_SHOWN_CHARS).map(|(c, _)| c.name.as_str()).collect::<Vec<_>>().join(", ");
                self.not_bag.push(NotBag { object, found: shown(&found) });
            }
            for (n, (schema, sscope)) in items(container, &cscope).into_iter().enumerate() {
                let id = value(schema, &sscope, SCHEMA, "namespaceURI").unwrap_or_else(|| position(n));
                let at = |item: Option<String>| Place { object, schema: id.clone(), item };
                self.need(&at(None), schema, &sscope, SCHEMA, "pdfaSchema", &["schema", "namespaceURI", "prefix"]);
                let properties = entries(schema, &sscope, SCHEMA, "property").flat_map(|(e, es)| items(e, &es));
                for (n, (property, pscope)) in properties.enumerate() {
                    let name = value(property, &pscope, PROPERTY, "name").unwrap_or_else(|| position(n));
                    self.need(&at(Some(name)), property, &pscope, PROPERTY, "pdfaProperty", &["name", "valueType", "category", "description"]);
                }
                let value_types = entries(schema, &sscope, SCHEMA, "valueType").flat_map(|(e, es)| items(e, &es));
                for (n, (value_type, vtscope)) in value_types.enumerate() {
                    let name = value(value_type, &vtscope, TYPE, "type").unwrap_or_else(|| position(n));
                    self.need(
                        &at(Some(name.clone())),
                        value_type,
                        &vtscope,
                        TYPE,
                        "pdfaType",
                        &["type", "namespaceURI", "prefix", "description", "field"],
                    );
                    for (n, (field, fscope)) in entries(value_type, &vtscope, TYPE, "field").flat_map(|(e, es)| items(e, &es)).enumerate() {
                        let field_name = value(field, &fscope, FIELD, "name").unwrap_or_else(|| position(n));
                        self.need(
                            &at(Some(format!("{name}/{field_name}"))),
                            field,
                            &fscope,
                            FIELD,
                            "pdfaField",
                            &["name", "valueType", "description"],
                        );
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
    fn need(&mut self, at: &Place, d: &El, scope: &NsScope<'_>, ns: &str, prefix: &str, names: &[&str]) {
        for name in names {
            let element = entry(d, scope, ns, name);
            let attribute = scope.attr(d, ns, name);
            let empty = match (element.as_ref(), attribute) {
                (None, None) => false,
                (Some((e, es)), _) => NAMING.contains(name) && is_empty(e, es),
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
    fn prefixes_in(&mut self, object: ObjRef, container: &El, scope: &NsScope<'_>) {
        fn walk(findings: &mut Findings, object: ObjRef, n: &El, scope: &NsScope<'_>) {
            let element = (scope.el_ns(n), n.name.as_str());
            let attributes = n.attrs.iter().map(|(k, _)| (scope.attr_ns(k), k.as_str()));
            for (ns, qname) in std::iter::once(element).chain(attributes) {
                let Some(&(namespace, own)) = PREFIXES.iter().find(|(uri, _)| Some(*uri) == ns) else { continue };
                let written = shown(written_prefix(qname));
                if written != own
                    && findings.room()
                    && !findings.prefixes.iter().any(|p| p.object == object && p.namespace == namespace && p.written == written)
                {
                    findings.prefixes.push(Prefix { object, namespace, own, written });
                }
            }
            for child in n.elements() {
                walk(findings, object, child, &scope.child(child));
            }
        }
        walk(self, object, container, scope);
    }
}

/// The entries that name something, which an empty value leaves without a name: namespace URIs
/// and prefixes, property, value type and field names, value type references and the property
/// category.
const NAMING: [&str; 6] = ["namespaceURI", "prefix", "name", "type", "valueType", "category"];

/// Whether the element entry `e` holds no value: no element, no text but white space, and no
/// `rdf:resource` or `rdf:value` attribute with a value.
fn is_empty(e: &El, scope: &NsScope<'_>) -> bool {
    let blank = |v: Option<&str>| v.is_none_or(|v| v.trim().is_empty());
    !e.children.iter().any(|c| match c {
        craft_xml::Node::El(_) => true,
        craft_xml::Node::Text(t) | craft_xml::Node::CData(t) => !t.trim().is_empty(),
        _ => false,
    }) && blank(scope.attr(e, RDF, "resource"))
        && blank(scope.attr(e, RDF, "value"))
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
fn value(d: &El, scope: &NsScope<'_>, ns: &str, name: &str) -> Option<String> {
    let owned;
    let raw = match entry(d, scope, ns, name) {
        Some((e, _)) => {
            owned = e.text();
            owned.as_ref()
        }
        None => scope.attr(d, ns, name)?,
    };
    let text = raw.trim();
    (!text.is_empty()).then(|| shown(text))
}

/// The prefix of the qualified name `text` begins with, empty when it has none.
fn written_prefix(text: &str) -> &str {
    text.split_once(':').map_or("", |(p, _)| p)
}

fn is(n: &El, scope: &NsScope<'_>, ns: &str, name: &str) -> bool {
    scope.is(n, ns, name)
}

/// Every element entry `name` of `ns` in the description `d`: a list written twice is read twice.
fn entries<'a>(d: &'a El, scope: &'a NsScope<'a>, ns: &'a str, name: &'a str) -> impl Iterator<Item = (&'a El, NsScope<'a>)> {
    d.elements().filter_map(move |c| {
        let cs = scope.child(c);
        is(c, &cs, ns, name).then_some((c, cs))
    })
}

/// The element entry `name` of `ns` in the description `d`.
fn entry<'a>(d: &'a El, scope: &NsScope<'a>, ns: &str, name: &str) -> Option<(&'a El, NsScope<'a>)> {
    d.elements().find_map(|c| {
        let cs = scope.child(c);
        is(c, &cs, ns, name).then_some((c, cs))
    })
}

/// The descriptions in the array `e` holds: each item's `rdf:parseType="Resource"` node or
/// `rdf:Description`.
fn items<'a>(e: &'a El, scope: &NsScope<'a>) -> Vec<(&'a El, NsScope<'a>)> {
    let Some((array, ascope)) = e.elements().find_map(|c| {
        let cs = scope.child(c);
        (is(c, &cs, RDF, "Bag") || is(c, &cs, RDF, "Seq") || is(c, &cs, RDF, "Alt")).then_some((c, cs))
    }) else {
        return Vec::new();
    };
    array
        .elements()
        .filter_map(|li| {
            let ls = ascope.child(li);
            is(li, &ls, RDF, "li").then_some((li, ls))
        })
        .map(|(li, ls)| {
            li.elements()
                .find_map(|c| {
                    let cs = ls.child(c);
                    is(c, &cs, RDF, "Description").then_some((c, cs))
                })
                .unwrap_or((li, ls))
        })
        .collect()
}
