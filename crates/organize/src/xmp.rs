//! Keeping the catalog's XMP metadata (`/Root /Metadata`, ISO 32000-2 §14.3.2) in step with the
//! document information when Document Properties ▸ Description changes it.
//!
//! Files that carry XMP state the title and author twice, and viewers that read XMP first show
//! the packet's value, so changing only `/Info` leaves the file disagreeing with itself. Each
//! information key maps to the XMP property PDF/A pairs it with (ISO 19005-1 §6.7.3, the same
//! mapping `pdfcraft-preflight` writes): Title → `dc:title`, Author → `dc:creator`, Subject →
//! `dc:description`, Keywords → `pdf:Keywords`, Creator → `xmp:CreatorTool`, Producer →
//! `pdf:Producer`.
//!
//! The packet is read through one bounded XML parse (`pdfcraft_xml::parse`), and properties are
//! found by namespace, whatever prefix the packet binds it to. An edit splices the packet's text
//! at the spans the parser reports, touching only the property that changed: everything else in
//! it (other schemas, padding, unknown properties, comments) stays byte for byte, and the result
//! must parse again or the edit is dropped. A file without XMP gets none, and a packet this can't
//! read safely (not UTF-8, not well-formed XML, too large or costly to parse, no `rdf:RDF`) is
//! left as it was and gives no values.

use std::ops::Range;

use pdfcraft_cos::{Document, Object, Stream};
use pdfcraft_xml::span;
use roxmltree::Node;

use crate::OrganizeError;
use crate::pdfx::esc;

/// Decoding limit for an XMP packet (they are a few kB; padding rarely takes them past 100 kB).
const MAX_XMP: usize = 16 << 20;
/// The most nodes a packet may hold. A PDF/A packet with extension schemas has a few hundred.
const MAX_NODES: u32 = 200_000;

const RDF: &str = "http://www.w3.org/1999/02/22-rdf-syntax-ns#";
const DC: &str = "http://purl.org/dc/elements/1.1/";
const PDF: &str = "http://ns.adobe.com/pdf/1.3/";
const XMP: &str = "http://ns.adobe.com/xap/1.0/";
/// The PDF/A identification schema (ISO 19005).
pub(crate) const PDFAID: &str = "http://www.aiim.org/pdfa/ns/id/";
/// The PDF/X identification schema (ISO 15930).
pub(crate) const PDFXID: &str = "http://www.npes.org/pdfx/ns/id/";

/// An XMP property: its namespace and local name, and the prefix a description this module adds
/// declares for it. The packet's own prefix may be different; it is what matching ignores.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Prop {
    prefix: &'static str,
    ns: &'static str,
    local: &'static str,
}

impl Prop {
    pub(crate) const fn new(prefix: &'static str, ns: &'static str, local: &'static str) -> Self {
        Prop { prefix, ns, local }
    }
}

/// How an XMP property holds its value.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Shape {
    /// A language alternative: `rdf:Alt` with an `x-default` item.
    LangAlt,
    /// An ordered array: `rdf:Seq` (the authors).
    Seq,
    /// Plain text.
    Text,
}

/// The XMP property that mirrors a document information key.
fn property(key: &str) -> Option<(Prop, Shape)> {
    Some(match key {
        "Title" => (Prop::new("dc", DC, "title"), Shape::LangAlt),
        "Author" => (Prop::new("dc", DC, "creator"), Shape::Seq),
        "Subject" => (Prop::new("dc", DC, "description"), Shape::LangAlt),
        "Keywords" => (Prop::new("pdf", PDF, "Keywords"), Shape::Text),
        "Creator" => (Prop::new("xmp", XMP, "CreatorTool"), Shape::Text),
        "Producer" => (Prop::new("pdf", PDF, "Producer"), Shape::Text),
        _ => return None,
    })
}

/// After the information entry `key` became `value` (empty: removed), make the catalog's XMP
/// packet say the same. Does nothing for keys XMP doesn't mirror, files without XMP, and packets
/// that can't be edited safely.
pub(crate) fn sync_info(doc: &mut Document, key: &str, value: &str) -> Result<(), OrganizeError> {
    let Some((prop, shape)) = property(key) else { return Ok(()) };
    edit_packet(doc, |xmp| set_property(xmp, prop, shape, value)).map(|_| ())
}

/// Remove every occurrence of the properties `props` from the catalog's XMP packet; whether any
/// was there.
pub(crate) fn remove_properties(doc: &mut Document, props: &[Prop]) -> Result<bool, OrganizeError> {
    edit_packet(doc, |xmp| without(xmp, props))
}

/// Rewrite the catalog's XMP packet with `edit` (`None`: unchanged); whether it changed. Files
/// without XMP and packets that can't be edited safely are left alone.
fn edit_packet(doc: &mut Document, edit: impl FnOnce(&str) -> Option<String>) -> Result<bool, OrganizeError> {
    let Some(root) = doc.root() else { return Ok(false) };
    // A metadata stream is always indirect (§7.3.8); anything else is left alone.
    let Some(r) = doc.get(root).as_dict().and_then(|c| c.reference(b"Metadata")) else { return Ok(false) };
    let obj = doc.get(r);
    let Object::Stream(stream) = &*obj else { return Ok(false) };
    let Ok(bytes) = stream.decoded_within(MAX_XMP) else { return Ok(false) };
    let Ok(xmp) = String::from_utf8(bytes) else { return Ok(false) };
    let Some(updated) = edit(&xmp) else { return Ok(false) };
    // Written unfiltered, as PDF/A asks of metadata streams, so other tools can read it.
    let mut dict = stream.dict.clone();
    for k in [&b"Filter"[..], b"DecodeParms", b"DL", b"Length"] {
        dict.remove(k);
    }
    doc.set(r, Object::Stream(Stream::from_raw(dict, updated.into_bytes())));
    Ok(true)
}

/// The value the catalog's XMP packet gives the document information key `key`, for files that
/// state their title, author and so on only there (`/Info` is deprecated in PDF 2.0, and some
/// writers leave it out). A language alternative gives its `x-default` item (else the first),
/// several authors are joined with "; ". `None` for keys XMP doesn't mirror, files without XMP,
/// packets that can't be read safely, and empty values.
pub(crate) fn info_value(doc: &Document, key: &str) -> Option<String> {
    let (prop, shape) = property(key)?;
    let root = doc.root()?;
    let r = doc.get(root).as_dict()?.reference(b"Metadata")?;
    let obj = doc.get(r);
    let Object::Stream(stream) = &*obj else { return None };
    let xmp = String::from_utf8(stream.decoded_within(MAX_XMP).ok()?).ok()?;
    get_property(&xmp, prop, shape)
}

/// The text of a simple property in the packet `xmp` (`pdfxid:GTS_PDFXVersion`), element or
/// attribute form; `None` when it is absent, empty, or the packet can't be read safely.
pub(crate) fn simple_value(xmp: &str, prop: Prop) -> Option<String> {
    get_property(xmp, prop, Shape::Text)
}

/// `xmp` as a tree, within the bounds every XMP read here shares.
fn parse(xmp: &str) -> Option<roxmltree::Document<'_>> {
    pdfcraft_xml::parse(xmp, pdfcraft_xml::Limits::new(MAX_NODES)).ok()
}

/// The packet's `rdf:RDF` element.
fn rdf_root<'a, 'i>(doc: &'a roxmltree::Document<'i>) -> Option<Node<'a, 'i>> {
    doc.descendants().find(|n| is(*n, RDF, "RDF"))
}

/// Whether `n` is the element `local` of the namespace `ns`.
fn is(n: Node, ns: &str, local: &str) -> bool {
    n.is_element() && n.tag_name().namespace() == Some(ns) && n.tag_name().name() == local
}

/// Where a property was found.
enum Found<'a, 'i> {
    /// The element form, `<dc:title>…</dc:title>`.
    Element(Node<'a, 'i>),
    /// The attribute form, `pdf:Keywords="…"`: its range (name through closing quote) and value.
    Attribute(Range<usize>, &'a str),
}

/// Every place `prop` is a property of the packet's resource, in document order: an attribute of,
/// or an element in, an `rdf:Description` directly under `rdf:RDF`. Property sets nested inside a
/// value (the resources an Illustrator or InDesign file embeds, in `xmpMM:Pantry` or
/// `xmpMM:Ingredients`) describe something else, and are never read, edited or removed here.
fn occurrences<'a, 'i: 'a>(rdf: Node<'a, 'i>, prop: Prop) -> impl Iterator<Item = Found<'a, 'i>> + 'a {
    rdf.children().filter(|d| is(*d, RDF, "Description")).flat_map(move |d| {
        let attributes =
            d.attributes().filter(move |a| a.namespace() == Some(prop.ns) && a.name() == prop.local).map(|a| Found::Attribute(a.range(), a.value()));
        let elements = d.children().filter(move |n| is(*n, prop.ns, prop.local)).map(Found::Element);
        attributes.chain(elements)
    })
}

/// The text of the property `prop` in `xmp` (see [`info_value`]).
fn get_property(xmp: &str, prop: Prop, shape: Shape) -> Option<String> {
    let doc = parse(xmp)?;
    let value = match occurrences(rdf_root(&doc)?, prop).next()? {
        Found::Attribute(_, value) => value.trim().to_string(),
        Found::Element(e) => element_value(e, shape)?,
    };
    Some(value).filter(|v| !v.is_empty())
}

/// The value of a property element: its text, or the items of the `rdf:Alt`, `rdf:Seq` or
/// `rdf:Bag` it holds. Any other structure has no value here, so markup never shows as text.
fn element_value(e: Node, shape: Shape) -> Option<String> {
    if !e.children().any(|c| c.is_element()) {
        return text(e);
    }
    let container = e.children().find(|c| ["Alt", "Seq", "Bag"].iter().any(|name| is(*c, RDF, name)))?;
    let items: Vec<Node> = container.children().filter(|c| is(*c, RDF, "li")).collect();
    if shape == Shape::LangAlt {
        let item = items.iter().find(|li| li.attribute((roxmltree::NS_XML_URI, "lang")) == Some("x-default")).or(items.first())?;
        return text(*item);
    }
    let values: Vec<String> = items.iter().filter_map(|li| text(*li)).filter(|t| !t.is_empty()).collect();
    (!values.is_empty()).then(|| values.join("; "))
}

/// An element's character data, trimmed (comments and processing instructions split it, so it is
/// every text child, not only the first); `None` when it holds an element.
fn text(n: Node) -> Option<String> {
    if n.children().any(|c| c.is_element()) {
        return None;
    }
    let all: String = n.children().filter(|c| c.is_text()).filter_map(|c| c.text()).collect();
    Some(all.trim().to_string())
}

/// The markup a property of `shape` holds for `value`, with `rdf` the packet's prefix for the
/// RDF namespace (needed by the structured shapes only).
fn inner_markup(shape: Shape, rdf: Option<&str>, value: &str) -> Option<String> {
    let v = esc(value);
    Some(match shape {
        Shape::Text => v,
        Shape::LangAlt => {
            let r = rdf?;
            format!("<{r}:Alt><{r}:li xml:lang=\"x-default\">{v}</{r}:li></{r}:Alt>")
        }
        Shape::Seq => {
            let r = rdf?;
            format!("<{r}:Seq><{r}:li>{v}</{r}:li></{r}:Seq>")
        }
    })
}

/// The packet's prefix for the RDF namespace where `n` is; `None` when RDF is the default
/// namespace there, which leaves no prefix to write new RDF markup with.
fn rdf_prefix<'i>(n: Node<'_, 'i>) -> Option<&'i str> {
    n.lookup_prefix(RDF).filter(|p| !p.is_empty())
}

/// `xmp` with the property `prop` set to `value`, or removed (every occurrence) when `value` is
/// empty; `None` when nothing changes, the packet can't be read safely, or the edit can't be
/// written in it.
fn set_property(xmp: &str, prop: Prop, shape: Shape, value: &str) -> Option<String> {
    if value.is_empty() {
        return without(xmp, &[prop]);
    }
    let doc = parse(xmp)?;
    let rdf = rdf_root(&doc)?;
    let edited = match occurrences(rdf, prop).next() {
        // The element form: its content is replaced and its start tag kept as written, with any
        // namespace declarations and attributes it carries.
        Some(Found::Element(e)) => {
            let range = e.range();
            let inner = inner_markup(shape, rdf_prefix(e), value)?;
            match span::content(xmp, &range) {
                Some(content) => splice(xmp, content, &inner)?,
                // `<dc:title/>` becomes `<dc:title>…</dc:title>`.
                None => {
                    let name = span::qname(xmp, &range)?;
                    let slash = span::start_tag_end(xmp, &range)?.checked_sub(1)?;
                    splice(xmp, slash..range.end, &format!(">{inner}</{name}>"))?
                }
            }
        }
        // The attribute form: only simple text can be written there.
        Some(Found::Attribute(range, _)) if shape == Shape::Text => {
            let (value_range, quote) = span::attribute_value(xmp, &range)?;
            let escaped = if quote == '"' { esc(value) } else { esc(value).replace('\'', "&apos;") };
            splice(xmp, value_range, &escaped)?
        }
        // A structured value moves to an element of its own: every occurrence goes in one pass,
        // then the new description goes in through one fresh parse of what is left.
        Some(Found::Attribute(..)) => {
            let removed = without(xmp, &[prop])?;
            let rest = parse(&removed)?;
            let rdf = rdf_root(&rest)?;
            insert_description(&removed, rdf, prop, &inner_markup(shape, rdf_prefix(rdf), value)?)?
        }
        None => insert_description(xmp, rdf, prop, &inner_markup(shape, rdf_prefix(rdf), value)?)?,
    };
    checked(edited)
}

/// `xmp` with a new `rdf:Description` holding `prop` (with `inner` as its content, and declaring
/// its namespace) as the last description of the packet. `None` for an empty `<rdf:RDF/>` or an
/// RDF namespace without a prefix.
fn insert_description(xmp: &str, rdf: Node, prop: Prop, inner: &str) -> Option<String> {
    let r = rdf_prefix(rdf)?;
    let at = span::content(xmp, &rdf.range())?.end;
    // Every description of a packet describes the same resource (XMP: one `rdf:about` value).
    let about = rdf.children().filter(|d| is(*d, RDF, "Description")).find_map(|d| d.attribute((RDF, "about"))).unwrap_or_default();
    let Prop { prefix: p, ns, local } = prop;
    let description =
        format!("<{r}:Description {r}:about=\"{}\" xmlns:{p}=\"{}\"><{p}:{local}>{inner}</{p}:{local}></{r}:Description>\n", esc(about), esc(ns));
    splice(xmp, at..at, &description)
}

/// What removing the attribute at `range` takes out: the attribute and the whitespace before it.
fn attribute_span(xmp: &str, range: &Range<usize>) -> Option<Range<usize>> {
    Some(xmp.get(..range.start)?.trim_end().len()..range.end)
}

/// `xmp` without any occurrence of `props`; `None` when there is none, or the packet can't be
/// read safely.
fn without(xmp: &str, props: &[Prop]) -> Option<String> {
    let doc = parse(xmp)?;
    let rdf = rdf_root(&doc)?;
    let mut spans: Vec<Range<usize>> = props
        .iter()
        .flat_map(|p| occurrences(rdf, *p))
        .filter_map(|found| match found {
            Found::Element(e) => Some(e.range()),
            Found::Attribute(range, _) => attribute_span(xmp, &range),
        })
        .collect();
    spans.sort_by_key(|s| s.start);
    let mut out = String::with_capacity(xmp.len());
    let mut at = 0;
    for s in spans {
        // Top-level properties never overlap. A span inside one already taken out would make
        // `at..s.start` an inverted range, whose `get` is `None` and would abort every removal,
        // so it is skipped instead.
        if s.start < at {
            continue;
        }
        out.push_str(xmp.get(at..s.start)?);
        at = s.end;
    }
    if at == 0 {
        return None;
    }
    out.push_str(xmp.get(at..)?);
    checked(out)
}

/// `xmp` with `range` replaced by `text`.
fn splice(xmp: &str, range: Range<usize>, text: &str) -> Option<String> {
    let mut out = String::with_capacity(xmp.len().saturating_add(text.len()));
    out.push_str(xmp.get(..range.start)?);
    out.push_str(text);
    out.push_str(xmp.get(range.end..)?);
    Some(out)
}

/// The edited packet, if it still parses: an edit that would leave the packet unreadable is not
/// written.
fn checked(edited: String) -> Option<String> {
    parse(&edited).is_some().then_some(edited)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The shape of a LibreOffice export: one description per schema, element forms.
    const PACKET: &str = "<?xpacket begin=\"\u{feff}\" id=\"W5M0MpCehiHzreSzNTczkc9d\"?>\n\
<x:xmpmeta xmlns:x=\"adobe:ns:meta/\">\n <rdf:RDF xmlns:rdf=\"http://www.w3.org/1999/02/22-rdf-syntax-ns#\">\n\
  <rdf:Description rdf:about=\"\" xmlns:pdf=\"http://ns.adobe.com/pdf/1.3/\">\n   <pdf:Producer>Writer 1.0</pdf:Producer>\n  </rdf:Description>\n\
  <rdf:Description rdf:about=\"\" xmlns:dc=\"http://purl.org/dc/elements/1.1/\">\n\
   <dc:title><rdf:Alt><rdf:li xml:lang=\"x-default\">Untitled 1</rdf:li></rdf:Alt></dc:title>\n\
   <dc:creator><rdf:Seq><rdf:li>Old Author</rdf:li></rdf:Seq></dc:creator>\n\
   <dc:description><rdf:Alt><rdf:li xml:lang=\"x-default\">Old subject</rdf:li></rdf:Alt></dc:description>\n\
  </rdf:Description>\n </rdf:RDF>\n</x:xmpmeta>\n<?xpacket end=\"w\"?>";

    const TITLE: Prop = Prop::new("dc", DC, "title");
    const CREATOR: Prop = Prop::new("dc", DC, "creator");
    const DESCRIPTION: Prop = Prop::new("dc", DC, "description");
    const KEYWORDS: Prop = Prop::new("pdf", PDF, "Keywords");
    const PRODUCER: Prop = Prop::new("pdf", PDF, "Producer");
    const CREATOR_TOOL: Prop = Prop::new("xmp", XMP, "CreatorTool");

    /// `body` inside an `rdf:Description` of a packet declaring the usual prefixes.
    fn packet(body: &str) -> String {
        format!(
            "<x:xmpmeta xmlns:x=\"adobe:ns:meta/\"><rdf:RDF xmlns:rdf=\"{RDF}\"><rdf:Description rdf:about=\"\" \
             xmlns:dc=\"{DC}\" xmlns:pdf=\"{PDF}\">{body}</rdf:Description></rdf:RDF></x:xmpmeta>"
        )
    }

    #[test]
    fn values_are_read_back_unescaped() {
        let refs = packet("<pdf:Keywords>a &amp; b &lt;c&gt; &#65;&#x42; &#xD800;</pdf:Keywords>");
        assert_eq!(get_property(&refs, KEYWORDS, Shape::Text).as_deref(), Some("a & b <c> AB \u{FFFD}"));
        assert_eq!(get_property(PACKET, TITLE, Shape::LangAlt).as_deref(), Some("Untitled 1"));
        assert_eq!(get_property(PACKET, CREATOR, Shape::Seq).as_deref(), Some("Old Author"));
        assert_eq!(get_property(PACKET, PRODUCER, Shape::Text).as_deref(), Some("Writer 1.0"));
        assert_eq!(get_property(PACKET, KEYWORDS, Shape::Text), None);
        // No x-default: the first language; empty and self-closed items are skipped.
        let alt = packet("<dc:title><rdf:Alt><rdf:li xml:lang='fr'>Rapport</rdf:li><rdf:li xml:lang='en'>Report</rdf:li></rdf:Alt></dc:title>");
        assert_eq!(get_property(&alt, TITLE, Shape::LangAlt).as_deref(), Some("Rapport"));
        let seq = packet("<dc:creator><rdf:Seq><rdf:li/><rdf:li> </rdf:li><rdf:li>Ada</rdf:li></rdf:Seq></dc:creator>");
        assert_eq!(get_property(&seq, CREATOR, Shape::Seq).as_deref(), Some("Ada"));
        // An empty alternative or list, or an item holding markup, has no value: never the markup.
        for body in ["<dc:title><rdf:Alt/></dc:title>", "<dc:title><rdf:Alt></rdf:Alt></dc:title>", "<dc:title/>"] {
            assert_eq!(get_property(&packet(body), TITLE, Shape::LangAlt), None, "{body}");
        }
        assert_eq!(get_property(&packet("<dc:creator><rdf:Seq/></dc:creator>"), CREATOR, Shape::Seq), None);
        let nested = packet("<dc:creator><rdf:Seq><rdf:li><rdf:Description/></rdf:li><rdf:li>Ada</rdf:li></rdf:Seq></dc:creator>");
        assert_eq!(get_property(&nested, CREATOR, Shape::Seq).as_deref(), Some("Ada"));
    }

    /// The string reader took the first `</rdf:li>` or `<dc:title>` it saw, even inside a comment
    /// or CDATA section, and matched the prefix as written (rs-xml-end-tag-find).
    #[test]
    fn markup_in_comments_and_other_prefixes_are_read_as_xml() {
        let commented = packet(
            "<!-- <dc:title>fake</dc:title> --><dc:title><rdf:Alt><rdf:li xml:lang=\"x-default\">Re<!-- </rdf:li> -->port</rdf:li></rdf:Alt></dc:title>",
        );
        assert_eq!(get_property(&commented, TITLE, Shape::LangAlt).as_deref(), Some("Report"));
        let out = set_property(&commented, TITLE, Shape::LangAlt, "New").unwrap();
        assert!(out.contains("<!-- <dc:title>fake</dc:title> -->"), "the comment is not edited: {out}");
        assert_eq!(get_property(&out, TITLE, Shape::LangAlt).as_deref(), Some("New"));
        let cdata = packet("<pdf:Keywords><![CDATA[a </pdf:Keywords> b]]></pdf:Keywords>");
        assert_eq!(get_property(&cdata, KEYWORDS, Shape::Text).as_deref(), Some("a </pdf:Keywords> b"));
        // DC bound to `d`, RDF to `r`: found by namespace, and written with the packet's prefixes.
        let other = format!(
            "<r:RDF xmlns:r=\"{RDF}\"><r:Description r:about=\"\" xmlns:d=\"{DC}\"><d:title><r:Alt><r:li xml:lang=\"x-default\">Old</r:li></r:Alt></d:title></r:Description></r:RDF>"
        );
        assert_eq!(get_property(&other, TITLE, Shape::LangAlt).as_deref(), Some("Old"));
        let out = set_property(&other, CREATOR, Shape::Seq, "Ada").unwrap();
        assert!(out.contains("<r:Description r:about=\"\" xmlns:dc=\"http://purl.org/dc/elements/1.1/\"><dc:creator><r:Seq><r:li>Ada</r:li></r:Seq></dc:creator></r:Description>\n</r:RDF>"), "{out}");
        let out = set_property(&other, TITLE, Shape::LangAlt, "New").unwrap();
        assert!(out.contains("<d:title><r:Alt><r:li xml:lang=\"x-default\">New</r:li></r:Alt></d:title>"), "{out}");
    }

    /// A character reference XML doesn't allow makes the packet not well-formed, so it gives no
    /// value. The string reader decoded each of these as `A`: a sign (`&#x+41;`, `&#+65;`, which
    /// `from_str_radix` and `parse` accept) and an uppercase hex marker (`&#X41;`, XML has only
    /// `&#x`).
    #[test]
    fn character_references_xml_rejects_give_no_value() {
        for reference in ["&#x+41;", "&#+65;", "&#X41;"] {
            let bad = packet(&format!("<pdf:Keywords>{reference}</pdf:Keywords>"));
            assert_eq!(get_property(&bad, KEYWORDS, Shape::Text), None, "{reference}");
            assert_eq!(set_property(&bad, KEYWORDS, Shape::Text, "new"), None, "{reference}");
            assert_eq!(without(&bad, &[KEYWORDS]), None, "{reference}");
        }
        // The forms XML does allow still decode.
        for reference in ["&#x41;", "&#65;", "&#0065;", "&#x0041;"] {
            let good = packet(&format!("<pdf:Keywords>{reference}</pdf:Keywords>"));
            assert_eq!(get_property(&good, KEYWORDS, Shape::Text).as_deref(), Some("A"), "{reference}");
        }
    }

    /// A packet that isn't well-formed XML is neither read nor edited.
    #[test]
    fn packets_that_are_not_xml_are_left_alone() {
        for bad in [packet("<pdf:Keywords>a & b</pdf:Keywords>"), packet("<pdf:Keywords>open")] {
            assert_eq!(get_property(&bad, KEYWORDS, Shape::Text), None, "{bad}");
            assert_eq!(set_property(&bad, KEYWORDS, Shape::Text, "new"), None, "{bad}");
            assert_eq!(without(&bad, &[KEYWORDS]), None, "{bad}");
        }
        // No rdf:RDF, or an empty one: nothing to add a description to.
        assert_eq!(set_property("<x:xmpmeta xmlns:x=\"adobe:ns:meta/\"/>", KEYWORDS, Shape::Text, "a"), None);
        assert_eq!(set_property(&format!("<rdf:RDF xmlns:rdf=\"{RDF}\"/>"), KEYWORDS, Shape::Text, "a"), None);
        // RDF as the default namespace leaves no prefix to write an rdf:Alt with; text still works.
        let default_ns =
            format!("<RDF xmlns=\"{RDF}\"><Description about=\"\" xmlns:pdf=\"{PDF}\"><pdf:Keywords>a</pdf:Keywords></Description></RDF>");
        assert_eq!(set_property(&default_ns, TITLE, Shape::LangAlt, "T"), None);
        assert!(set_property(&default_ns, KEYWORDS, Shape::Text, "b").is_some_and(|out| out.contains("<pdf:Keywords>b</pdf:Keywords>")));
    }

    /// A packet past pdfcraft-xml's work budget is not parsed, so it is neither read nor edited.
    /// 12,000 attributes on one tag: about 7·10⁷ comparisons for roxmltree's duplicate check, past
    /// the 2²⁶ budget, refused by the linear scan before the parser runs.
    #[test]
    fn packets_past_the_parse_budget_are_left_alone() {
        let attrs = |n: usize| (0..n).map(|i| format!(" a{i}=\"v\"")).collect::<String>();
        let costly = packet(&format!("<pdf:Keywords{}>k</pdf:Keywords>", attrs(12_000)));
        assert_eq!(get_property(&costly, KEYWORDS, Shape::Text), None);
        assert_eq!(set_property(&costly, KEYWORDS, Shape::Text, "new"), None);
        let fine = packet(&format!("<pdf:Keywords{}>k</pdf:Keywords>", attrs(120)));
        assert_eq!(get_property(&fine, KEYWORDS, Shape::Text).as_deref(), Some("k"));
    }

    #[test]
    fn an_edit_that_would_not_parse_is_dropped() {
        assert_eq!(checked("<a><b></a>".to_string()), None);
        assert!(checked("<a><b/></a>".to_string()).is_some());
    }

    #[test]
    fn element_values_are_replaced_in_place() {
        let out = set_property(PACKET, TITLE, Shape::LangAlt, "Guide & <draft>").unwrap();
        assert!(out.contains("<dc:title><rdf:Alt><rdf:li xml:lang=\"x-default\">Guide &amp; &lt;draft&gt;</rdf:li></rdf:Alt></dc:title>\n"), "{out}");
        // Everything else is untouched.
        assert_eq!(out.replace("Guide &amp; &lt;draft&gt;", "Untitled 1"), PACKET);
        let out = set_property(PACKET, CREATOR, Shape::Seq, "Demo Consulting").unwrap();
        assert!(out.contains("<dc:creator><rdf:Seq><rdf:li>Demo Consulting</rdf:li></rdf:Seq></dc:creator>"), "{out}");
        // The start tag stays as written, with what it declares.
        let declaring = packet(&format!("<pdf:Producer xmlns:pdf=\"{PDF}\" rdf:parseType=\"Literal\">Old</pdf:Producer>"));
        let out = set_property(&declaring, PRODUCER, Shape::Text, "New").unwrap();
        assert!(out.contains(&format!("<pdf:Producer xmlns:pdf=\"{PDF}\" rdf:parseType=\"Literal\">New</pdf:Producer>")), "{out}");
        // An empty element gains content.
        let out = set_property(&packet("<pdf:Producer a='x/'/>"), PRODUCER, Shape::Text, "New").unwrap();
        assert!(out.contains("<pdf:Producer a='x/'>New</pdf:Producer>"), "{out}");
    }

    #[test]
    fn an_empty_value_removes_the_property() {
        let out = set_property(PACKET, DESCRIPTION, Shape::LangAlt, "").unwrap();
        assert!(!out.contains("dc:description"), "{out}");
        assert!(out.contains("Untitled 1") && out.contains("Old Author"));
        // Nothing to remove: no change.
        assert_eq!(set_property(PACKET, KEYWORDS, Shape::Text, ""), None);
    }

    #[test]
    fn a_missing_property_gets_a_description_of_its_own() {
        let out = set_property(PACKET, KEYWORDS, Shape::Text, "alpha, beta").unwrap();
        let added =
            "<rdf:Description rdf:about=\"\" xmlns:pdf=\"http://ns.adobe.com/pdf/1.3/\"><pdf:Keywords>alpha, beta</pdf:Keywords></rdf:Description>\n";
        assert!(out.find(added).is_some_and(|at| at > out.find("<dc:creator>").unwrap() && at < out.find("</rdf:RDF>").unwrap()), "{out}");
        assert_eq!(out.replacen(added, "", 1), PACKET);
    }

    #[test]
    fn attribute_values_are_rewritten_where_they_stand() {
        let packet = "<x:xmpmeta xmlns:x=\"adobe:ns:meta/\"><rdf:RDF xmlns:rdf=\"http://www.w3.org/1999/02/22-rdf-syntax-ns#\"><rdf:Description rdf:about='uuid:1' \
                      xmlns:xmp=\"http://ns.adobe.com/xap/1.0/\" xmlns:dc=\"http://purl.org/dc/elements/1.1/\" xmp:CreatorTool='Old \"tool\" &gt;' dc:title=\"Odd\"/></rdf:RDF></x:xmpmeta>";
        assert_eq!(get_property(packet, CREATOR_TOOL, Shape::Text).as_deref(), Some("Old \"tool\" >"));
        let out = set_property(packet, CREATOR_TOOL, Shape::Text, "Writer's tool").unwrap();
        assert!(out.contains("xmp:CreatorTool='Writer&apos;s tool'"), "{out}");
        // A title can't be an attribute: it moves to an element, in a description for the same resource.
        let out = set_property(packet, TITLE, Shape::LangAlt, "Guide").unwrap();
        assert!(!out.contains("dc:title=\"Odd\""), "{out}");
        assert!(out.contains("<rdf:Description rdf:about=\"uuid:1\" xmlns:dc=\"http://purl.org/dc/elements/1.1/\"><dc:title><rdf:Alt>"), "{out}");
        let out = set_property(packet, CREATOR_TOOL, Shape::Text, "").unwrap();
        assert!(out.contains("xmlns:dc=\"http://purl.org/dc/elements/1.1/\" dc:title=\"Odd\"/>"), "{out}");
    }

    #[test]
    fn lookalike_names_and_text_are_not_matched() {
        let packet = packet("<dc:titles>keep</dc:titles><dc:subject>say dc:title=\"no\"</dc:subject><dc:title/>");
        let out = set_property(&packet, TITLE, Shape::LangAlt, "New").unwrap();
        assert!(out.contains("<dc:titles>keep</dc:titles>") && out.contains("say dc:title=\"no\""), "{out}");
        assert!(out.contains("<dc:title><rdf:Alt><rdf:li xml:lang=\"x-default\">New</rdf:li></rdf:Alt></dc:title></rdf:Description>"), "{out}");
    }

    #[test]
    fn every_occurrence_is_removed_in_one_pass() {
        let ids = Prop::new("pdfaid", PDFAID, "part");
        let conformance = Prop::new("pdfaid", PDFAID, "conformance");
        let packet = format!(
            "<rdf:RDF xmlns:rdf=\"{RDF}\"><rdf:Description rdf:about=\"\" xmlns:a=\"{PDFAID}\" a:conformance='B'><a:part>2</a:part></rdf:Description>\
             <rdf:Description rdf:about=\"\" xmlns:pdfaid=\"{PDFAID}\" pdfaid:part=\"3\"/></rdf:RDF>"
        );
        let out = without(&packet, &[ids, conformance]).unwrap();
        assert_eq!(
            out,
            format!(
                "<rdf:RDF xmlns:rdf=\"{RDF}\"><rdf:Description rdf:about=\"\" xmlns:a=\"{PDFAID}\"></rdf:Description><rdf:Description rdf:about=\"\" xmlns:pdfaid=\"{PDFAID}\"/></rdf:RDF>"
            )
        );
        assert_eq!(without(&out, &[ids, conformance]), None, "nothing left to remove");
    }

    /// A structured value replacing attribute forms takes out every one in a single pass and adds
    /// one element: no re-parse per occurrence (and no recursion a packet of thousands could
    /// overflow). Clearing a property removes every occurrence, not only the first.
    #[test]
    fn every_occurrence_goes_in_one_pass() {
        let many: String = (0..3_000).map(|i| format!("<rdf:Description rdf:about=\"\" xmlns:dc=\"{DC}\" dc:title=\"t{i}\"/>")).collect();
        let packet = format!("<rdf:RDF xmlns:rdf=\"{RDF}\">{many}</rdf:RDF>");
        let out = set_property(&packet, TITLE, Shape::LangAlt, "T").unwrap();
        assert!(!out.contains("dc:title=") && out.matches("<dc:title>").count() == 1, "one element, no attributes left");
        assert_eq!(get_property(&out, TITLE, Shape::LangAlt).as_deref(), Some("T"));
        let both = packet.replacen("/>", "><dc:title>e</dc:title></rdf:Description>", 1);
        let cleared = set_property(&both, TITLE, Shape::LangAlt, "").unwrap();
        assert!(!cleared.contains("dc:title"), "every occurrence, element and attribute forms");
    }

    /// Embedded resources (Illustrator's `xmpMM:Pantry`) carry property sets of their own: they are
    /// not the document's, so they are neither read, edited nor removed.
    #[test]
    fn nested_property_sets_are_not_the_documents() {
        let pantry = format!(
            "<rdf:RDF xmlns:rdf=\"{RDF}\"><rdf:Description rdf:about=\"\" xmlns:xmpMM=\"http://ns.adobe.com/xap/1.0/mm/\" xmlns:xmp=\"{XMP}\" \
             xmlns:pdfaid=\"{PDFAID}\"><xmpMM:Pantry><rdf:Bag><rdf:li><rdf:Description xmp:CreatorTool=\"Illustrator\" pdfaid:part=\"1\">\
             <dc:title xmlns:dc=\"{DC}\">Logo</dc:title></rdf:Description></rdf:li></rdf:Bag></xmpMM:Pantry></rdf:Description></rdf:RDF>"
        );
        assert_eq!(get_property(&pantry, CREATOR_TOOL, Shape::Text), None);
        assert_eq!(get_property(&pantry, TITLE, Shape::LangAlt), None);
        assert_eq!(without(&pantry, &[Prop::new("pdfaid", PDFAID, "part")]), None, "the embedded claim stays");
        let out = set_property(&pantry, CREATOR_TOOL, Shape::Text, "PdfCraft").unwrap();
        assert!(out.contains("xmp:CreatorTool=\"Illustrator\""), "{out}");
        assert_eq!(get_property(&out, CREATOR_TOOL, Shape::Text).as_deref(), Some("PdfCraft"));
    }

    #[test]
    fn hostile_packets_do_not_panic() {
        for body in ["", "<", "<dc:title", "<dc:title>", "<dc:title x='", " dc:title=", "<a dc:title=\"", "</rdf:RDF>", "é<dc:title é>"] {
            for packet in [body.to_string(), packet(body)] {
                for value in ["", "v"] {
                    let _ = set_property(&packet, TITLE, Shape::LangAlt, value);
                    let _ = set_property(&packet, KEYWORDS, Shape::Text, value);
                    let _ = get_property(&packet, TITLE, Shape::LangAlt);
                    let _ = without(&packet, &[TITLE, KEYWORDS]);
                }
            }
        }
    }
}
