//! Keeping the catalog's XMP metadata (`/Root /Metadata`, ISO 32000-2 §14.3.2) in step with the
//! document information when Document Properties ▸ Description changes it.
//!
//! Files that carry XMP state the title and author twice, and viewers that read XMP first show
//! the packet's value, so changing only `/Info` leaves the file disagreeing with itself. Each
//! information key maps to the XMP property PDF/A pairs it with (ISO 19005-1 §6.7.3, the same
//! mapping `pdfcraft-preflight` writes): Title → `dc:title`, Author → `dc:creator`, Subject →
//! `dc:description`, Keywords → `pdf:Keywords`, Creator → `xmp:CreatorTool`, Producer →
//! `pdf:Producer`. A custom property (Document Properties ▸ Custom) is kept as `pdfx:<name>`, plain
//! text in the `http://ns.adobe.com/pdfx/1.3/` schema where Acrobat keeps them, so tools that read
//! XMP see it too; one whose name can't be an XML element name stays in `/Info` only, and a
//! PDF/A packet gets no new one (see [`sync_info`]).
//!
//! The packet is edited as text, touching only the property that changed: everything else in it
//! (other schemas, padding, unknown properties) stays byte for byte. A file without XMP gets
//! none, and a packet this can't edit safely (not UTF-8, no `rdf:RDF`, too large to decode) is
//! left as it was.

use std::cell::RefCell;
use std::sync::{Arc, Weak};

use pdfcraft_cos::{Document, Object, Stream};

use crate::OrganizeError;
use crate::pdfx::esc;

/// Decoding limit for an XMP packet (they are a few kB; padding rarely takes them past 100 kB).
const MAX_XMP: usize = 16 << 20;

const DC: &str = "http://purl.org/dc/elements/1.1/";
const PDF: &str = "http://ns.adobe.com/pdf/1.3/";
const XMP: &str = "http://ns.adobe.com/xap/1.0/";
/// Adobe's PDF extension schema: custom document properties, each under its own name.
const PDFX: &str = "http://ns.adobe.com/pdfx/1.3/";

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

/// The XMP property (qualified name, namespace) that mirrors a document information key.
fn property(key: &str) -> Option<(&'static str, &'static str, Shape)> {
    Some(match key {
        "Title" => ("dc:title", DC, Shape::LangAlt),
        "Author" => ("dc:creator", DC, Shape::Seq),
        "Subject" => ("dc:description", DC, Shape::LangAlt),
        "Keywords" => ("pdf:Keywords", PDF, Shape::Text),
        "Creator" => ("xmp:CreatorTool", XMP, Shape::Text),
        "Producer" => ("pdf:Producer", PDF, Shape::Text),
        _ => return None,
    })
}

/// After the information entry `key` became `value` (empty: removed), make the catalog's XMP
/// packet say the same: the standard keys' properties, and `pdfx:<key>` for a custom property.
/// Does nothing for standard keys XMP doesn't mirror (the dates, Trapped), custom names that
/// aren't XML names, files without XMP, and packets that can't be edited safely.
///
/// A custom property is left alone where `pdfx:` would not mean Adobe's schema (a packet binding
/// the prefix to another namespace keeps its own property), and in a packet that declares PDF/A
/// one is changed or removed where it stands but never added: PDF/A-1 to -3 allow a schema only
/// when the packet describes it in an extension schema (ISO 19005-2 §6.6.2.3), so a new `pdfx:`
/// property would end the file's conformance. `/Info` keeps it either way.
pub(crate) fn sync_info(doc: &mut Document, key: &str, value: &str) -> Result<(), OrganizeError> {
    if let Some((name, ns, shape)) = property(key) {
        return edit_packet(doc, |xmp| set_property(xmp, name, ns, shape, value)).map(|_| ());
    }
    if crate::STANDARD_INFO_KEYS.contains(&key) || !is_xml_name(key) {
        return Ok(());
    }
    let name = format!("pdfx:{key}");
    edit_packet(doc, |xmp| {
        if binds_prefix_elsewhere(xmp, "pdfx", PDFX) {
            return None;
        }
        // A declaration counts whatever its value says (CDATA, markup): it is the claim that matters.
        if !has_property(xmp, &name) && has_property(xmp, "pdfaid:part") {
            return None;
        }
        set_property(xmp, &name, PDFX, Shape::Text, value)
    })
    .map(|_| ())
}

/// Remove the `pdfx:` copies of the custom properties `keys` (see [`sync_info`]), unless the packet
/// binds `pdfx` to another namespace, whose properties are its own; whether any was there.
pub(crate) fn remove_custom_properties(doc: &mut Document, keys: &[&str]) -> Result<bool, OrganizeError> {
    let names: Vec<String> = keys.iter().map(|k| format!("pdfx:{k}")).collect();
    edit_packet(doc, |xmp| if binds_prefix_elsewhere(xmp, "pdfx", PDFX) { None } else { remove_all(xmp, &names) })
}

/// Whether `xmp` has the property `name`, as an element or an attribute, whatever its value.
fn has_property(xmp: &str, name: &str) -> bool {
    element_span(xmp, name).is_some() || attribute_span(xmp, name).is_some()
}

/// Whether `xmp` declares `prefix` for a namespace other than `ns` anywhere (`xmlns:pdfx="…"` on
/// any element), so a property written with that prefix may belong to another schema. A
/// declaration whose value never ends counts as another namespace.
fn binds_prefix_elsewhere(xmp: &str, prefix: &str, ns: &str) -> bool {
    let attr = format!("xmlns:{prefix}");
    // Each declaration is read only up to its closing quote, and the first other value stops the
    // search, so this is linear in the packet.
    xmp.match_indices(&attr).any(|(i, _)| {
        // `xmlns:pdfx` must end there (not `xmlns:pdfxid`), then `=` and a quoted value.
        let Some(rest) = xmp.get(i + attr.len()..).and_then(|r| r.trim_start().strip_prefix('=')).map(str::trim_start) else {
            return false;
        };
        let Some(quote) = rest.chars().next().filter(|c| matches!(c, '"' | '\'')) else { return false };
        rest.get(1..).and_then(|v| v.get(..v.find(quote)?)).is_none_or(|value| value != ns)
    })
}

/// Whether `name` can follow `pdfx:` in an element name: an XML name without a colon (XML 1.0
/// §2.3 `NameStartChar` then `NameChar`s; Namespaces in XML `NCName`). Custom property names may
/// hold characters XML names can't (`1st`, `Q&A`, `a:b`); those are not mirrored rather than
/// written under an escaped name of our own.
fn is_xml_name(name: &str) -> bool {
    let start = |c: char| {
        matches!(c,
            'A'..='Z' | '_' | 'a'..='z' | '\u{C0}'..='\u{D6}' | '\u{D8}'..='\u{F6}' | '\u{F8}'..='\u{2FF}' | '\u{370}'..='\u{37D}'
            | '\u{37F}'..='\u{1FFF}' | '\u{200C}'..='\u{200D}' | '\u{2070}'..='\u{218F}' | '\u{2C00}'..='\u{2FEF}'
            | '\u{3001}'..='\u{D7FF}' | '\u{F900}'..='\u{FDCF}' | '\u{FDF0}'..='\u{FFFD}' | '\u{10000}'..='\u{EFFFF}')
    };
    let rest = |c: char| start(c) || matches!(c, '-' | '.' | '0'..='9' | '\u{B7}' | '\u{300}'..='\u{36F}' | '\u{203F}'..='\u{2040}');
    let mut chars = name.chars();
    chars.next().is_some_and(start) && chars.all(rest)
}

/// Remove every occurrence of the properties `names` from the catalog's XMP packet; whether any
/// was there.
pub(crate) fn remove_properties(doc: &mut Document, names: &[&str]) -> Result<bool, OrganizeError> {
    edit_packet(doc, |xmp| remove_all(xmp, names))
}

/// `xmp` without any occurrence of the properties `names`; `None` when none was there.
fn remove_all(xmp: &str, names: &[impl AsRef<str>]) -> Option<String> {
    let mut out: Option<String> = None;
    for name in names {
        // Each removal shortens the packet, so this ends.
        while let Some(next) = set_property(out.as_deref().unwrap_or(xmp), name.as_ref(), "", Shape::Text, "") {
            out = Some(next);
        }
    }
    out
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
    let (name, _, shape) = property(key)?;
    let root = doc.root()?;
    let r = doc.get(root).as_dict()?.reference(b"Metadata")?;
    let xmp = decoded_packet(&doc.get(r))?;
    get_property(&xmp, name, shape)
}

/// A decoded packet (`None`: unreadable) and the stream object it came from.
type Held = (Weak<Object>, Option<Arc<str>>);

thread_local! {
    /// The packet [`decoded_packet`] last decoded (`None`: unreadable), with the stream object it
    /// came from. The tab title can come from XMP and is read every frame; a packet decodes to
    /// up to [`MAX_XMP`] bytes.
    static PACKET: RefCell<Option<Held>> = const { RefCell::new(None) };
}

/// The text of the XMP stream `obj`, `None` when it isn't a stream or isn't UTF-8 within
/// [`MAX_XMP`]. Decoded once while the document keeps handing out the same object: an edit or
/// another document gives another one. The [`Weak`] keeps its address from being reused.
fn decoded_packet(obj: &Arc<Object>) -> Option<Arc<str>> {
    let same = |held: &Held| std::ptr::eq(held.0.as_ptr(), Arc::as_ptr(obj));
    if let Ok(Some(text)) = PACKET.try_with(|p| p.borrow().as_ref().filter(|held| same(held)).map(|held| held.1.clone())) {
        return text;
    }
    let Object::Stream(stream) = &**obj else { return None };
    #[cfg(test)]
    XMP_DECODES.with(|n| n.set(n.get() + 1));
    let text = stream.decoded_within(MAX_XMP).ok().and_then(|b| String::from_utf8(b).ok()).map(Arc::<str>::from);
    // Past the thread's end (a destructor reading it) the packet just isn't kept.
    let _ = PACKET.try_with(|p| *p.borrow_mut() = Some((Arc::downgrade(obj), text.clone())));
    text
}

#[cfg(test)]
thread_local! {
    /// Packets [`decoded_packet`] decoded on this thread.
    pub(crate) static XMP_DECODES: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

/// The text of the property `name` in `xmp`, element or attribute form (see [`info_value`]).
fn get_property(xmp: &str, name: &str, shape: Shape) -> Option<String> {
    let value = if let Some((start, end)) = element_span(xmp, name) {
        let element = xmp.get(start..end)?;
        let open_end = element.find('>')?;
        // `<dc:title/>`: no value.
        if element.get(..open_end)?.ends_with('/') {
            return None;
        }
        let inner = element.get(open_end + 1..)?.strip_suffix(&format!("</{name}>"))?;
        let items = list_items(inner);
        if items.is_empty() {
            // An empty `<rdf:Alt/>` or `<rdf:Seq></rdf:Seq>` has no value, not its markup.
            text_value(inner)?
        } else if shape == Shape::LangAlt {
            let (_, text) = items.iter().find(|(lang, _)| lang.as_deref() == Some("x-default")).or(items.first())?;
            text_value(text)?
        } else {
            items.iter().filter_map(|(_, text)| text_value(text)).filter(|t| !t.is_empty()).collect::<Vec<_>>().join("; ")
        }
    } else {
        let (_, value_start, value_end, _) = attribute_span(xmp, name)?;
        unescape(xmp.get(value_start..value_end)?.trim())
    };
    Some(value).filter(|v| !v.trim().is_empty())
}

/// Character data `text` as a value, unescaped; `None` when it still holds markup (a `<`, which
/// a value escapes as `&lt;`), so a structure the reader doesn't take apart never shows as text.
fn text_value(text: &str) -> Option<String> {
    let text = text.trim();
    (!text.contains('<')).then(|| unescape(text))
}

/// The `rdf:li` items of a property's content: each one's `xml:lang` (if any) and its text.
fn list_items(inner: &str) -> Vec<(Option<String>, &str)> {
    let mut items = Vec::new();
    let mut rest = inner;
    while let Some(i) = rest.find("<rdf:li") {
        let Some(after) = rest.get(i + "<rdf:li".len()..) else { break };
        // `<rdf:li` must end there: not `<rdf:link`.
        if !after.starts_with(|c: char| c == '>' || c == '/' || c.is_whitespace()) {
            rest = after;
            continue;
        }
        let Some(tag_end) = after.find('>') else { break };
        let tag = after.get(..tag_end).unwrap_or_default();
        let lang = tag.find("xml:lang=").and_then(|at| {
            let v = tag.get(at + "xml:lang=".len()..)?;
            let quote = v.chars().next().filter(|c| matches!(c, '"' | '\''))?;
            let v = v.get(1..)?;
            Some(v.get(..v.find(quote)?)?.to_string())
        });
        let Some(body) = after.get(tag_end + 1..) else { break };
        if tag.ends_with('/') {
            items.push((lang, ""));
            rest = body;
            continue;
        }
        let Some(close) = body.find("</rdf:li>") else { break };
        items.push((lang, body.get(..close).unwrap_or_default()));
        rest = body.get(close..).unwrap_or_default();
    }
    items
}

/// XML text with its character and entity references resolved (an unknown one is kept as is).
fn unescape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(i) = rest.find('&') {
        out.push_str(rest.get(..i).unwrap_or_default());
        let after = rest.get(i..).unwrap_or_default();
        let Some(semi) = after.find(';').filter(|&n| n <= 12) else {
            out.push('&');
            rest = after.get(1..).unwrap_or_default();
            continue;
        };
        let entity = after.get(1..semi).unwrap_or_default();
        let c = match entity {
            "amp" => Some('&'),
            "lt" => Some('<'),
            "gt" => Some('>'),
            "quot" => Some('"'),
            "apos" => Some('\''),
            _ => entity
                .strip_prefix("#x")
                .or_else(|| entity.strip_prefix("#X"))
                .filter(|h| h.bytes().all(|b| b.is_ascii_hexdigit()))
                .and_then(|h| u32::from_str_radix(h, 16).ok())
                .or_else(|| entity.strip_prefix('#').filter(|d| d.bytes().all(|b| b.is_ascii_digit())).and_then(|d| d.parse().ok()))
                .and_then(char::from_u32),
        };
        match c {
            Some(c) => {
                out.push(c);
                rest = after.get(semi + 1..).unwrap_or_default();
            }
            None => {
                out.push('&');
                rest = after.get(1..).unwrap_or_default();
            }
        }
    }
    out.push_str(rest);
    out
}

/// `xmp` with the property `name` (namespace `ns`) set to `value`, or removed when `value` is
/// empty; `None` when nothing changes or the packet has no `rdf:RDF` to add the property to.
fn set_property(xmp: &str, name: &str, ns: &str, shape: Shape, value: &str) -> Option<String> {
    let element = (!value.is_empty()).then(|| match shape {
        Shape::LangAlt => format!("<{name}><rdf:Alt><rdf:li xml:lang=\"x-default\">{}</rdf:li></rdf:Alt></{name}>", esc(value)),
        Shape::Seq => format!("<{name}><rdf:Seq><rdf:li>{}</rdf:li></rdf:Seq></{name}>", esc(value)),
        Shape::Text => format!("<{name}>{}</{name}>", esc(value)),
    });
    // The element form, `<dc:title>…</dc:title>` (or `<dc:title/>`): replaced where it stands, so
    // it keeps the namespace declaration of the description around it.
    if let Some((start, end)) = element_span(xmp, name) {
        let mut out = String::with_capacity(xmp.len());
        out.push_str(xmp.get(..start)?);
        out.push_str(element.as_deref().unwrap_or_default());
        out.push_str(xmp.get(end..)?);
        return Some(out);
    }
    // The attribute form, `pdf:Keywords="…"` on an `rdf:Description`: only simple text can be
    // written there; a structured value moves to an element of its own.
    if let Some((start, value_start, value_end, quote)) = attribute_span(xmp, name) {
        let mut out = String::with_capacity(xmp.len());
        if shape == Shape::Text && !value.is_empty() {
            let escaped = if quote == '"' { esc(value) } else { esc(value).replace('\'', "&apos;") };
            out.push_str(xmp.get(..value_start)?);
            out.push_str(&escaped);
            out.push_str(xmp.get(value_end..)?);
            return Some(out);
        }
        out.push_str(xmp.get(..start)?);
        out.push_str(xmp.get(value_end + 1..)?);
        return match element {
            Some(e) => insert_description(&out, name, ns, &e),
            None => Some(out),
        };
    }
    insert_description(xmp, name, ns, element.as_deref()?)
}

/// `xmp` with a new `rdf:Description` holding `element` (and declaring its namespace) as the last
/// description of the packet.
fn insert_description(xmp: &str, name: &str, ns: &str, element: &str) -> Option<String> {
    let prefix = name.split(':').next()?;
    let at = xmp.rfind("</rdf:RDF>")?;
    // Every description of a packet describes the same resource (XMP: one `rdf:about` value).
    let about = attribute_span(xmp, "rdf:about").and_then(|(_, s, e, _)| xmp.get(s..e)).unwrap_or_default().replace('"', "&quot;");
    let mut out = String::with_capacity(xmp.len() + element.len() + 128);
    out.push_str(xmp.get(..at)?);
    out.push_str(&format!("<rdf:Description rdf:about=\"{about}\" xmlns:{prefix}=\"{ns}\">{element}</rdf:Description>\n"));
    out.push_str(xmp.get(at..)?);
    Some(out)
}

/// Where the element `name` is in `xmp`, from its `<` to past its end tag (or past `/>`).
fn element_span(xmp: &str, name: &str) -> Option<(usize, usize)> {
    let open = format!("<{name}");
    let mut from = 0;
    while let Some(i) = xmp.get(from..).and_then(|s| s.find(&open)).map(|i| i + from) {
        let after = i + open.len();
        from = after;
        // `<dc:title` must end there: not `<dc:titles`.
        match xmp.get(after..).and_then(|s| s.chars().next()) {
            Some('>' | '/') => {}
            Some(c) if c.is_whitespace() => {}
            _ => continue,
        }
        let tag_end = after + xmp.get(after..)?.find('>')?;
        if xmp.get(..tag_end)?.ends_with('/') {
            return Some((i, tag_end + 1));
        }
        let close = format!("</{name}>");
        let end = tag_end + xmp.get(tag_end..)?.find(&close)? + close.len();
        return Some((i, end));
    }
    None
}

/// Where the attribute `name` is in `xmp`: its start (including the whitespace before it), the
/// start and end of its value, and its quote character. Only matches inside a start tag.
fn attribute_span(xmp: &str, name: &str) -> Option<(usize, usize, usize, char)> {
    let mut from = 0;
    // Track tag and quote state in one pass rather than searching back from every match.
    let (mut scanned, mut in_tag, mut quote) = (0, false, None);
    while let Some(i) = xmp.get(from..).and_then(|s| s.find(name)).map(|i| i + from) {
        from = i + name.len();
        let before = xmp.get(..i)?;
        let gap = xmp.get(scanned..i)?;
        #[cfg(test)]
        crate::work(gap.len());
        for c in gap.chars() {
            if let Some(q) = quote {
                if c == q {
                    quote = None;
                }
            } else {
                match c {
                    '<' => in_tag = true,
                    '>' if in_tag => in_tag = false,
                    '\'' | '\"' if in_tag => quote = Some(c),
                    _ => {}
                }
            }
        }
        scanned = i;
        // Preceded by whitespace, and inside a start tag.
        if !before.ends_with(char::is_whitespace) || !in_tag || quote.is_some() {
            continue;
        }
        let rest = xmp.get(from..)?;
        let trimmed = rest.trim_start();
        let Some(after_eq) = trimmed.strip_prefix('=') else { continue };
        let after_eq = after_eq.trim_start();
        let Some(quote) = after_eq.chars().next().filter(|c| matches!(c, '"' | '\'')) else { continue };
        let value_start = xmp.len() - after_eq.len() + 1;
        let value_end = value_start + xmp.get(value_start..)?.find(quote)?;
        let start = before.trim_end().len();
        return Some((start, value_start, value_end, quote));
    }
    None
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

    #[test]
    fn unescape_rejects_signs_in_numeric_references() {
        assert_eq!(unescape("a&#x+41;b&#+66;"), "a&#x+41;b&#+66;");
        assert_eq!(unescape("&#x;&#;"), "&#x;&#;");
        assert_eq!(unescape("&#x41;&#65;"), "AA");
    }

    #[test]
    fn values_are_read_back_unescaped() {
        assert_eq!(unescape("a &amp; b &lt;c&gt; &#65;&#x42; &bogus; &#xD800; & end"), "a & b <c> AB &bogus; &#xD800; & end");
        assert_eq!(get_property(PACKET, "dc:title", Shape::LangAlt).as_deref(), Some("Untitled 1"));
        assert_eq!(get_property(PACKET, "dc:creator", Shape::Seq).as_deref(), Some("Old Author"));
        assert_eq!(get_property(PACKET, "pdf:Producer", Shape::Text).as_deref(), Some("Writer 1.0"));
        assert_eq!(get_property(PACKET, "pdf:Keywords", Shape::Text), None);
        // No x-default: the first language; empty and self-closed items are skipped.
        let alt = "<dc:title><rdf:Alt><rdf:li xml:lang='fr'>Rapport</rdf:li><rdf:li xml:lang='en'>Report</rdf:li></rdf:Alt></dc:title>";
        assert_eq!(get_property(alt, "dc:title", Shape::LangAlt).as_deref(), Some("Rapport"));
        let seq = "<dc:creator><rdf:Seq><rdf:li/><rdf:li> </rdf:li><rdf:li>Ada</rdf:li></rdf:Seq></dc:creator>";
        assert_eq!(get_property(seq, "dc:creator", Shape::Seq).as_deref(), Some("Ada"));
        // Unfinished markup reads as nothing rather than as a wrong value.
        assert_eq!(get_property("<dc:title><rdf:Alt><rdf:li>Cut", "dc:title", Shape::LangAlt), None);
        // An empty alternative or list, or an item holding markup, has no value: never the markup.
        assert_eq!(get_property("<dc:title><rdf:Alt/></dc:title>", "dc:title", Shape::LangAlt), None);
        assert_eq!(get_property("<dc:title><rdf:Alt></rdf:Alt></dc:title>", "dc:title", Shape::LangAlt), None);
        assert_eq!(get_property("<dc:creator><rdf:Seq/></dc:creator>", "dc:creator", Shape::Seq), None);
        let nested = "<dc:creator><rdf:Seq><rdf:li><rdf:Description/></rdf:li><rdf:li>Ada</rdf:li></rdf:Seq></dc:creator>";
        assert_eq!(get_property(nested, "dc:creator", Shape::Seq).as_deref(), Some("Ada"));
    }

    #[test]
    fn element_values_are_replaced_in_place() {
        let out = set_property(PACKET, "dc:title", DC, Shape::LangAlt, "Guide & <draft>").unwrap();
        assert!(out.contains("<dc:title><rdf:Alt><rdf:li xml:lang=\"x-default\">Guide &amp; &lt;draft&gt;</rdf:li></rdf:Alt></dc:title>\n"), "{out}");
        assert!(!out.contains("Untitled 1"));
        // Everything else is untouched.
        assert_eq!(out.replace("Guide &amp; &lt;draft&gt;", "Untitled 1"), PACKET);
        let out = set_property(PACKET, "dc:creator", DC, Shape::Seq, "Demo Consulting").unwrap();
        assert!(out.contains("<dc:creator><rdf:Seq><rdf:li>Demo Consulting</rdf:li></rdf:Seq></dc:creator>"), "{out}");
    }

    #[test]
    fn an_empty_value_removes_the_property() {
        let out = set_property(PACKET, "dc:description", DC, Shape::LangAlt, "").unwrap();
        assert!(!out.contains("dc:description"), "{out}");
        assert!(out.contains("Untitled 1") && out.contains("Old Author"));
        // Nothing to remove: no change.
        assert_eq!(set_property(PACKET, "pdf:Keywords", PDF, Shape::Text, ""), None);
    }

    #[test]
    fn a_missing_property_gets_a_description_of_its_own() {
        let out = set_property(PACKET, "pdf:Keywords", PDF, Shape::Text, "alpha, beta").unwrap();
        let added =
            "<rdf:Description rdf:about=\"\" xmlns:pdf=\"http://ns.adobe.com/pdf/1.3/\"><pdf:Keywords>alpha, beta</pdf:Keywords></rdf:Description>\n";
        assert!(out.find(added).is_some_and(|at| at > out.find("<dc:creator>").unwrap() && at < out.find("</rdf:RDF>").unwrap()), "{out}");
        assert_eq!(out.replacen(added, "", 1), PACKET);
        // A packet with no rdf:RDF to add to is left alone.
        assert_eq!(set_property("<x:xmpmeta/>", "pdf:Keywords", PDF, Shape::Text, "alpha"), None);
    }

    #[test]
    fn attribute_values_are_rewritten_where_they_stand() {
        let packet = "<x:xmpmeta xmlns:x=\"adobe:ns:meta/\"><rdf:RDF xmlns:rdf=\"r\"><rdf:Description rdf:about='uuid:1' \
                      xmlns:xmp=\"http://ns.adobe.com/xap/1.0/\" xmlns:dc=\"http://purl.org/dc/elements/1.1/\" xmp:CreatorTool='Old \"tool\"' dc:title=\"Odd\"/></rdf:RDF></x:xmpmeta>";
        let out = set_property(packet, "xmp:CreatorTool", XMP, Shape::Text, "Writer's tool").unwrap();
        assert!(out.contains("xmp:CreatorTool='Writer&apos;s tool'"), "{out}");
        // A title can't be an attribute: it moves to an element, in a description for the same resource.
        let out = set_property(packet, "dc:title", DC, Shape::LangAlt, "Guide").unwrap();
        assert!(!out.contains("dc:title=\"Odd\""), "{out}");
        assert!(out.contains("<rdf:Description rdf:about=\"uuid:1\" xmlns:dc=\"http://purl.org/dc/elements/1.1/\"><dc:title><rdf:Alt>"), "{out}");
        let out = set_property(packet, "xmp:CreatorTool", XMP, Shape::Text, "").unwrap();
        assert!(out.contains("xmlns:dc=\"http://purl.org/dc/elements/1.1/\" dc:title=\"Odd\"/>"), "{out}");
    }

    #[test]
    fn lookalike_names_and_text_are_not_matched() {
        let packet = "<rdf:RDF xmlns:rdf=\"r\"><rdf:Description rdf:about=\"\"><dc:titles>keep</dc:titles>\
                      <dc:subject>say dc:title=\"no\"</dc:subject><dc:title/></rdf:Description></rdf:RDF>";
        let out = set_property(packet, "dc:title", DC, Shape::LangAlt, "New").unwrap();
        assert!(out.contains("<dc:titles>keep</dc:titles>") && out.contains("say dc:title=\"no\""), "{out}");
        assert!(out.contains("<dc:title><rdf:Alt><rdf:li xml:lang=\"x-default\">New</rdf:li></rdf:Alt></dc:title></rdf:Description>"), "{out}");
    }

    #[test]
    fn only_names_an_element_can_have_are_mirrored() {
        for name in ["Project", "_draft", "Größe", "a.b-c_1", "日本語", "x·y", "Ω"] {
            assert!(is_xml_name(name), "{name}");
        }
        for name in ["", "1st", "-x", ".x", "·x", "a:b", "Q&A", "it's", "a\"b", "×x", "a b", "a=b", "x\u{FFFE}"] {
            assert!(!is_xml_name(name), "{name}");
        }
    }

    #[test]
    fn a_prefix_bound_to_another_namespace_is_found() {
        for (xmp, other) in [
            ("<d xmlns:pdfx=\"http://ns.adobe.com/pdfx/1.3/\"/>", false),
            ("<d xmlns:pdfx = 'http://ns.adobe.com/pdfx/1.3/'/><e xmlns:pdfxid=\"http://www.npes.org/pdfx/ns/id/\"/>", false),
            ("<d xmlns:pdfx=\"http://ns.adobe.com/pdfx/1.3/\"/><e xmlns:pdfx=\"http://www.npes.org/pdfx/ns/id/\"/>", true),
            ("<d xmlns:pdfx='http://ns.adobe.com/pdfx/1.3", true),
            ("<d xmlns:pdfx>", false),
            ("", false),
        ] {
            assert_eq!(binds_prefix_elsewhere(xmp, "pdfx", PDFX), other, "{xmp}");
        }
    }

    #[test]
    fn attribute_like_text_inside_another_quoted_attribute_is_not_a_property() {
        let packet = "<rdf:Description note='someone wrote dc:title=\"forged\" here'/>";
        assert_eq!(attribute_span(packet, "dc:title"), None);
    }

    #[test]
    fn apostrophe_in_text_does_not_hide_a_later_attribute() {
        let packet = "<rdf:Description>don't</rdf:Description><rdf:Description dc:title='real'/>";
        let found = attribute_span(packet, "dc:title").and_then(|(_, start, end, quote)| packet.get(start..end).map(|value| (value, quote)));
        assert_eq!(found, Some(("real", '\'')));
    }

    #[test]
    fn hostile_packets_do_not_panic() {
        for packet in ["", "<", "<dc:title", "<dc:title>", "<dc:title x='", " dc:title=", "<a dc:title=\"", "</rdf:RDF>", "é<dc:title é>"] {
            for value in ["", "v"] {
                let _ = set_property(packet, "dc:title", DC, Shape::LangAlt, value);
                let _ = set_property(packet, "pdf:Keywords", PDF, Shape::Text, value);
            }
        }
    }

    #[test]
    fn attribute_search_scans_the_packet_once() {
        // Thousands of attribute-like words in one text node: none is inside a tag.
        let packet = format!("<x:xmpmeta><rdf:RDF><rdf:Description>{}</rdf:Description></rdf:RDF></x:xmpmeta>", " rdf:about".repeat(5_000));
        crate::take_work();
        assert_eq!(attribute_span(&packet, "rdf:about"), None);
        let work = crate::take_work();
        assert!(work <= 2 * packet.len(), "{work} bytes scanned in a {}-byte packet", packet.len());
        // A real attribute after them is still found.
        let packet = packet.replace("</rdf:Description>", "</rdf:Description><rdf:Description rdf:about='u'/>");
        let (_, s, e, q) = attribute_span(&packet, "rdf:about").unwrap();
        assert_eq!((packet.get(s..e), q), (Some("u"), '\''));
    }
}
