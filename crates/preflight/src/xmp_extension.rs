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
//! - a value type (`pdfaType`, 4.5) has `type`, `namespaceURI`, `prefix` and `description`. Its
//!   `field` list is optional, an "Optional description of the structured fields" (4.5): a simple
//!   type has none, and a description of a type the XMP Specification defines, such as `Part` or
//!   `Font`, which writers often add, needs none;
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
//!
//! The fields a value type describes are the fields its values may have. PDF/A-2 6.6.2.3.1 asks
//! that every property use a predefined schema or an extension schema, and a field is a property
//! of the structure that holds it (ISO 16684-1 6.3.2). So a structure value of a property that an
//! extension schema describes, with a value type the packet describes, has only the fields that
//! type lists in its namespace; [`Findings::undescribed`] holds the others. A property of a
//! predefined namespace keeps the types the XMP Specification gives it, whatever a packet
//! describes for it.

use crate::xmp::NsScope;
use craft_xml::El;
use pdfcraft_cos::ObjRef;

const RDF: &str = "http://www.w3.org/1999/02/22-rdf-syntax-ns#";
const EXTENSION: &str = "http://www.aiim.org/pdfa/ns/extension/";
const SCHEMA: &str = "http://www.aiim.org/pdfa/ns/schema#";
const PROPERTY: &str = "http://www.aiim.org/pdfa/ns/property#";
const TYPE: &str = "http://www.aiim.org/pdfa/ns/type#";
const FIELD: &str = "http://www.aiim.org/pdfa/ns/field#";
const XML: &str = "http://www.w3.org/XML/1998/namespace";

/// Each container namespace with its required prefix (TechNote 0009, 4.1).
const PREFIXES: [(&str, &str); 5] =
    [(EXTENSION, "pdfaExtension"), (SCHEMA, "pdfaSchema"), (PROPERTY, "pdfaProperty"), (TYPE, "pdfaType"), (FIELD, "pdfaField")];

/// How many findings a document's packets are read for. A document past this many has plainly
/// failed the clause, and listing every one of a hostile packet's descriptions helps nobody.
const MAX_FINDINGS: usize = 1000;
/// How much of a name the packet gives (a namespace URI, a property) a finding repeats.
const MAX_SHOWN_CHARS: usize = 200;
/// How many property and value type descriptions, and fields of each type, a packet's values are
/// compared with. A packet that describes more is a hostile one.
const MAX_DESCRIBED: usize = 10_000;

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
    /// Fields of structure values that their value type doesn't describe (6.6.2.3.1), each once
    /// per stream, property and field.
    pub(crate) undescribed: Vec<Undescribed>,
}

/// A field of a property's structure value that the property's value type doesn't describe.
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct Undescribed {
    pub(crate) object: ObjRef,
    /// The property, as the packet writes its name.
    pub(crate) property: String,
    /// The field, as the packet writes its name.
    pub(crate) field: String,
    /// The value type, by its `pdfaType:type`.
    pub(crate) value_type: String,
}

/// A value type a packet describes: its namespace and its fields, each by its name and, when the
/// description gives it, its value type.
struct Described {
    namespace: Option<String>,
    fields: Vec<(String, Option<String>)>,
}

/// How many structure values deep, each a field of the one before, [`Findings::undescribed_in`]
/// checks fields against the value types the packet describes.
const MAX_NESTED: usize = 8;

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
        // The properties the packet describes, by namespace and name, with their value types, and
        // the value types it describes, by name.
        let mut properties_described: Vec<(String, String, String)> = Vec::new();
        let mut types_described: Vec<(String, Described)> = Vec::new();
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
                    if let (Some(ns), Some(name), Some(ty)) = (
                        raw_value(schema, &sscope, SCHEMA, "namespaceURI"),
                        raw_value(property, &pscope, PROPERTY, "name"),
                        raw_value(property, &pscope, PROPERTY, "valueType"),
                    ) && properties_described.len() < MAX_DESCRIBED
                    {
                        properties_described.push((ns, name, ty));
                    }
                    let name = value(property, &pscope, PROPERTY, "name").unwrap_or_else(|| position(n));
                    self.need(&at(Some(name)), property, &pscope, PROPERTY, "pdfaProperty", &["name", "valueType", "category", "description"]);
                }
                let value_types = entries(schema, &sscope, SCHEMA, "valueType").flat_map(|(e, es)| items(e, &es));
                for (n, (value_type, vtscope)) in value_types.enumerate() {
                    if let Some(ty) = raw_value(value_type, &vtscope, TYPE, "type")
                        && types_described.len() < MAX_DESCRIBED
                    {
                        let fields = entries(value_type, &vtscope, TYPE, "field")
                            .flat_map(|(e, es)| items(e, &es))
                            .filter_map(|(field, fscope)| {
                                Some((raw_value(field, &fscope, FIELD, "name")?, raw_value(field, &fscope, FIELD, "valueType")))
                            })
                            .take(MAX_DESCRIBED)
                            .collect();
                        let namespace = raw_value(value_type, &vtscope, TYPE, "namespaceURI");
                        types_described.push((ty, Described { namespace, fields }));
                    }
                    let name = value(value_type, &vtscope, TYPE, "type").unwrap_or_else(|| position(n));
                    self.need(&at(Some(name.clone())), value_type, &vtscope, TYPE, "pdfaType", &["type", "namespaceURI", "prefix", "description"]);
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
        self.undescribed_in(object, doc, &properties_described, &types_described);
    }

    /// Whether another finding may be recorded (see [`MAX_FINDINGS`]).
    fn room(&self) -> bool {
        self.missing.len().saturating_add(self.prefixes.len()).saturating_add(self.not_bag.len()).saturating_add(self.undescribed.len())
            < MAX_FINDINGS
    }

    /// Notes the fields of structure values in the packet `doc`, from the metadata stream
    /// `object`, that their value type doesn't describe: the values of the properties `properties`
    /// (namespace, name, value type) whose type is one of `types`. A property of a predefined
    /// namespace is left to the types the XMP Specification gives it.
    fn undescribed_in(&mut self, object: ObjRef, doc: &El, properties: &[(String, String, String)], types: &[(String, Described)]) {
        fn walk<'a>(el: &'a El, parent: &NsScope<'a>, parent_is_rdf: bool, out: &mut Vec<(&'a El, NsScope<'a>)>) {
            let scope = parent.child(el);
            if parent_is_rdf && scope.is(el, RDF, "Description") {
                out.extend(el.elements().map(|p| (p, scope.child(p))));
            }
            let is_rdf = scope.is(el, RDF, "RDF");
            for child in el.elements() {
                walk(child, &scope, is_rdf, out);
            }
        }
        if properties.is_empty() || types.is_empty() {
            return;
        }
        let mut values = Vec::new();
        walk(doc, &NsScope::root(), false, &mut values);
        for (property, pscope) in values {
            let Some(ns) = pscope.el_ns(property) else { continue };
            if crate::xmp_types::is_predefined(ns) {
                continue;
            }
            let Some((_, _, ty)) = properties.iter().find(|(n, name, _)| n == ns && name == property.local()) else { continue };
            let (array, item_type) = array_item(ty);
            let Some((ty_name, described)) = types.iter().find(|(name, _)| name == item_type) else { continue };
            for (value, vscope) in holders(property, &pscope, array) {
                self.undescribed_fields(object, property, value, &vscope, (ty_name, described), types, 0);
            }
        }
    }

    /// Notes the fields of `value`, a structure value of `property`, that its value type `ty`
    /// doesn't describe, and of each field's value whose type the packet describes in turn, up to
    /// [`MAX_NESTED`] values deep.
    #[allow(clippy::too_many_arguments)]
    fn undescribed_fields(
        &mut self,
        object: ObjRef,
        property: &El,
        value: &El,
        vscope: &NsScope<'_>,
        ty: (&str, &Described),
        types: &[(String, Described)],
        depth: usize,
    ) {
        let (ty_name, described) = ty;
        for field in fields(value, vscope) {
            let own = (described.namespace.as_deref() == field.ns).then(|| described.fields.iter().find(|(f, _)| f == field.local)).flatten();
            // A type the XMP Specification defines keeps its fields, whatever a packet's
            // description of it lists (TechNote 0009, 4.5: the list is optional).
            if own.is_none() && crate::xmp_types::predefined_field(ty_name, field.ns, field.local) != Some(true) {
                let found = Undescribed { object, property: shown(&property.name), field: shown(field.written), value_type: shown(ty_name) };
                if self.room() && !self.undescribed.contains(&found) {
                    self.undescribed.push(found);
                }
                continue;
            }
            // A field whose value type the packet describes: its value's fields are that type's.
            let (Some((_, Some(field_type))), Some((element, escope))) = (own, field.element) else { continue };
            let (array, item_type) = array_item(field_type);
            let Some((nested_name, nested)) = types.iter().find(|(name, _)| name == item_type) else { continue };
            if depth >= MAX_NESTED {
                continue;
            }
            for (value, vscope) in holders(element, &escope, array) {
                self.undescribed_fields(object, property, value, &vscope, (nested_name, nested), types, depth.saturating_add(1));
            }
        }
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
    raw_value(d, scope, ns, name).map(|text| shown(&text))
}

/// The text of the entry `name` of `ns` in the description `d`, as [`value`] reads it but whole,
/// for comparing names.
fn raw_value(d: &El, scope: &NsScope<'_>, ns: &str, name: &str) -> Option<String> {
    let owned;
    let raw = match entry(d, scope, ns, name) {
        Some((e, _)) => {
            owned = e.text();
            owned.as_ref()
        }
        None => scope.attr(d, ns, name)?,
    };
    let text = raw.trim();
    (!text.is_empty()).then(|| text.to_owned())
}

/// A `pdfaProperty:valueType` read as an array or not, and the type of the value or of its items:
/// `bag Font` is an array of `Font` (TechNote 0009 writes the array kinds `bag`, `seq` and `alt`,
/// the XMP Specification `Bag`, `Seq` and `Alt`).
fn array_item(ty: &str) -> (bool, &str) {
    let ty = ty.trim();
    match ty.split_once(char::is_whitespace) {
        Some((kind, item)) if ["bag", "seq", "alt"].iter().any(|k| kind.eq_ignore_ascii_case(k)) => (true, item.trim()),
        _ => (false, ty),
    }
}

/// The fields of the structure value `value` (a property element or an array item): as the packet
/// writes each one's name, its namespace and its local name. They are the elements of its
/// `rdf:parseType="Resource"` node or of its one `rdf:Description`, or the attributes of either or
/// of an empty element, other than RDF and XML syntax (ISO 16684-1 7.9.2.5). A value that isn't a
/// structure has none. A qualified value's fields are those of its `rdf:value` (7.9.2.5, the
/// qualifiers beside it are no fields), read up to [`MAX_QUALIFIED`] values deep.
fn fields<'a>(value: &'a El, scope: &NsScope<'a>) -> Vec<Field<'a>> {
    let (mut value, mut scope) = (value, scope.clone());
    for _ in 0..MAX_QUALIFIED {
        match structure_fields(value, &scope) {
            Fields::Of(found) => return found,
            Fields::Qualified(inner, iscope) => (value, scope) = (inner, iscope),
        }
    }
    Vec::new()
}

/// How many `rdf:value` elements deep [`fields`] reads a qualified value.
const MAX_QUALIFIED: usize = 8;

/// A field of a structure value, as [`fields`] reads it.
struct Field<'a> {
    /// Its name as the packet writes it.
    written: &'a str,
    ns: Option<&'a str>,
    local: &'a str,
    /// The element that holds its value, when it is written as one rather than as an attribute.
    element: Option<(&'a El, NsScope<'a>)>,
}

/// What [`structure_fields`] finds in a value.
enum Fields<'a> {
    /// Its fields.
    Of(Vec<Field<'a>>),
    /// It is qualified: its value is this `rdf:value` element.
    Qualified(&'a El, NsScope<'a>),
}

/// The fields of `value` as [`fields`] reads them, or the `rdf:value` element a qualified value
/// holds.
fn structure_fields<'a>(value: &'a El, scope: &NsScope<'a>) -> Fields<'a> {
    // An array item's `rdf:Description` comes here as the item itself (see [`items`]).
    let holder = if scope.attr(value, RDF, "parseType") == Some("Resource") || is(value, scope, RDF, "Description") {
        Some((value, scope.clone()))
    } else {
        let children: Vec<&El> = value.elements().collect();
        match children.as_slice() {
            [d] if is(d, &scope.child(d), RDF, "Description") => Some((*d, scope.child(d))),
            [] => Some((value, scope.clone())),
            _ => None,
        }
    };
    let Some((holder, hscope)) = holder else { return Fields::Of(Vec::new()) };
    let syntax = |ns: Option<&str>| matches!(ns, Some(RDF) | Some(XML));
    if let Some(inner) = holder.elements().find(|c| hscope.child(c).is(c, RDF, "value")) {
        return Fields::Qualified(inner, hscope.child(inner));
    }
    // A simple qualified value, written as an attribute.
    if hscope.attr(holder, RDF, "value").is_some() {
        return Fields::Of(Vec::new());
    }
    let attributes = holder.attrs.iter().filter(|(k, _)| k != "xmlns" && !k.starts_with("xmlns:")).filter_map(|(k, _)| {
        let ns = hscope.attr_ns(k);
        (!syntax(ns)).then(|| Field { written: k.as_str(), ns, local: k.split_once(':').map_or(k.as_str(), |(_, l)| l), element: None })
    });
    let elements = holder.elements().filter_map(|c| {
        let cscope = hscope.child(c);
        let ns = cscope.el_ns(c);
        (!syntax(ns)).then(|| Field { written: c.name.as_str(), ns, local: c.local(), element: Some((c, cscope)) })
    });
    Fields::Of(attributes.chain(elements).collect())
}

/// The structure values `e`, of a property or field, holds: the items of its array, in its
/// `rdf:value` when it is qualified, or `e` itself.
fn holders<'a>(e: &'a El, scope: &NsScope<'a>, array: bool) -> Vec<(&'a El, NsScope<'a>)> {
    if array {
        // A qualified array's items are in its `rdf:value`.
        let (value, vscope) = qualified_value(e, scope);
        items(value, &vscope)
    } else {
        vec![(e, scope.clone())]
    }
}

/// The `rdf:value` of the qualified property `e` (ISO 16684-1 7.9.2.5), written in it with
/// `rdf:parseType="Resource"` or in its one `rdf:Description`; `e` itself when it has none.
fn qualified_value<'a>(e: &'a El, scope: &NsScope<'a>) -> (&'a El, NsScope<'a>) {
    let holder = if scope.attr(e, RDF, "parseType") == Some("Resource") {
        Some((e, scope.clone()))
    } else {
        e.elements().find_map(|c| {
            let cs = scope.child(c);
            is(c, &cs, RDF, "Description").then_some((c, cs))
        })
    };
    holder
        .and_then(|(d, ds)| {
            d.elements().find_map(|v| {
                let vs = ds.child(v);
                is(v, &vs, RDF, "value").then_some((v, vs))
            })
        })
        .unwrap_or_else(|| (e, scope.clone()))
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
