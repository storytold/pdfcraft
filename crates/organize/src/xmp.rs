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
//! The packet is edited as text, touching only the property that changed: everything else in it
//! (other schemas, padding, unknown properties) stays byte for byte. A file without XMP gets
//! none, and a packet this can't edit safely (not UTF-8, no `rdf:RDF`, too large to decode) is
//! left as it was.

use pdfcraft_cos::{Document, Object, Stream};

use crate::OrganizeError;
use crate::pdfx::esc;

/// Decoding limit for an XMP packet (they are a few kB; padding rarely takes them past 100 kB).
const MAX_XMP: usize = 16 << 20;

const DC: &str = "http://purl.org/dc/elements/1.1/";
const PDF: &str = "http://ns.adobe.com/pdf/1.3/";
const XMP: &str = "http://ns.adobe.com/xap/1.0/";

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
/// packet say the same. Does nothing for keys XMP doesn't mirror, files without XMP, and packets
/// that can't be edited safely.
pub(crate) fn sync_info(doc: &mut Document, key: &str, value: &str) -> Result<(), OrganizeError> {
    let Some((name, ns, shape)) = property(key) else { return Ok(()) };
    edit_packet(doc, |xmp| set_property(xmp, name, ns, shape, value)).map(|_| ())
}

/// Remove every occurrence of the properties `names` from the catalog's XMP packet; whether any
/// was there.
pub(crate) fn remove_properties(doc: &mut Document, names: &[&str]) -> Result<bool, OrganizeError> {
    edit_packet(doc, |xmp| {
        let mut out: Option<String> = None;
        for name in names {
            // Each removal shortens the packet, so this ends.
            while let Some(next) = set_property(out.as_deref().unwrap_or(xmp), name, "", Shape::Text, "") {
                out = Some(next);
            }
        }
        out
    })
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
    let obj = doc.get(r);
    let Object::Stream(stream) = &*obj else { return None };
    let xmp = String::from_utf8(stream.decoded_within(MAX_XMP).ok()?).ok()?;
    get_property(&xmp, name, shape)
}

/// The text of the simple property `name` in `xmp` (see [`get_property`]), for readers of other
/// XMP properties than the document information mirrors (PDF/X identification, `pdf:Trapped`).
/// Those are simple values in their schemas (`pdf:Trapped` a Boolean, `pdfxid:GTS_PDFXVersion` and
/// `pdfxid:GTS_PDFXConformance` Text), so a list (`rdf:Alt`, `rdf:Seq`) is markup, not a value: it reads as none rather than as
/// its items.
pub(crate) fn text_property(xmp: &str, name: &str) -> Option<String> {
    // Trimmed after character references are resolved too: `&#x20;PDF/X-4&#x20;` is `PDF/X-4`.
    read_property(xmp, name, Shape::Text, false).map(|v| v.trim().to_string()).filter(|v| !v.is_empty())
}

/// The text of the property `name` in `xmp`, element or attribute form (see [`info_value`]).
fn get_property(xmp: &str, name: &str, shape: Shape) -> Option<String> {
    read_property(xmp, name, shape, true)
}

/// [`get_property`], reading an element's `rdf:li` items only when `lists` is set.
fn read_property(xmp: &str, name: &str, shape: Shape, lists: bool) -> Option<String> {
    let value = if let Some((start, end)) = element_span(xmp, name) {
        let element = xmp.get(start..end)?;
        let open_end = tag_end(element, 0)?;
        // `<dc:title/>`: no value.
        if element.get(..open_end)?.ends_with('/') {
            return None;
        }
        let inner = element.get(open_end + 1..)?.strip_suffix(&format!("</{name}>"))?;
        let items = if lists { list_items(inner) } else { Vec::new() };
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

/// The `rdf:li` items of a property's content: each one's `xml:lang` (if any) and its text. Only
/// an item tag in character data counts, not one inside a comment, CDATA section, processing
/// instruction or quoted value, and the content is scanned once.
fn list_items(inner: &str) -> Vec<(Option<String>, &str)> {
    const OPEN: &str = "<rdf:li";
    let mut items = Vec::new();
    let mut scan = TextScan::new(inner);
    while let Some(i) = scan.find(OPEN) {
        let after = i + OPEN.len();
        // `<rdf:li` must end there: not `<rdf:link`.
        if !inner.get(after..).is_some_and(|s| s.starts_with(|c: char| c == '>' || c == '/' || c.is_whitespace())) {
            scan.advance(after);
            continue;
        }
        let Some(end) = tag_end(inner, after) else { break };
        let tag = inner.get(i..end).unwrap_or_default();
        // The item's own attribute, not a lookalike inside another attribute's quoted value.
        let lang = attribute_span(tag, "xml:lang").and_then(|(_, s, e, _)| tag.get(s..e)).map(str::to_string);
        scan.advance(end + 1);
        if tag.ends_with('/') {
            items.push((lang, ""));
            continue;
        }
        let Some(close) = scan.find("</rdf:li>") else { break };
        items.push((lang, inner.get(end + 1..close).unwrap_or_default()));
        scan.advance(close + 1);
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
                .and_then(|h| u32::from_str_radix(h, 16).ok())
                .or_else(|| entity.strip_prefix('#').and_then(|d| d.parse().ok()))
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
/// description of the packet: before the last `</rdf:RDF>` in character data, not one inside a
/// comment or CDATA section after it.
fn insert_description(xmp: &str, name: &str, ns: &str, element: &str) -> Option<String> {
    let prefix = name.split(':').next()?;
    let mut scan = TextScan::new(xmp);
    let mut last = None;
    while let Some(at) = scan.find("</rdf:RDF>") {
        last = Some(at);
        scan.advance(at + 1);
    }
    let at = last?;
    // Every description of a packet describes the same resource (XMP: one `rdf:about` value).
    let about = attribute_span(xmp, "rdf:about").and_then(|(_, s, e, _)| xmp.get(s..e)).unwrap_or_default().replace('"', "&quot;");
    let mut out = String::with_capacity(xmp.len() + element.len() + 128);
    out.push_str(xmp.get(..at)?);
    out.push_str(&format!("<rdf:Description rdf:about=\"{about}\" xmlns:{prefix}=\"{ns}\">{element}</rdf:Description>\n"));
    out.push_str(xmp.get(at..)?);
    Some(out)
}

/// Where the element `name` is in `xmp`, from its `<` to past its end tag (or past `/>`). Only a
/// start tag in character data counts, not one inside a comment, CDATA section or quoted value,
/// and the same goes for its end tag.
fn element_span(xmp: &str, name: &str) -> Option<(usize, usize)> {
    let open = format!("<{name}");
    let mut from = 0;
    // Where the text before the match stands, from one pass over the packet (see `attribute_span`).
    let (mut scanned, mut state) = (0, Markup::Text);
    while let Some(i) = xmp.get(from..).and_then(|s| s.find(&open)).map(|i| i + from) {
        let after = i + open.len();
        from = after;
        state = (scanned..i).fold(state, |state, p| state.step(xmp.as_bytes(), p));
        scanned = i;
        if state != Markup::Text {
            continue;
        }
        // `<dc:title` must end there: not `<dc:titles`.
        match xmp.get(after..).and_then(|s| s.chars().next()) {
            Some('>' | '/') => {}
            Some(c) if c.is_whitespace() => {}
            _ => continue,
        }
        let tag_end = tag_end(xmp, after)?;
        if xmp.get(..tag_end)?.ends_with('/') {
            return Some((i, tag_end + 1));
        }
        let close = format!("</{name}>");
        // On from the start tag, which stands in character data.
        let mut scan = TextScan { xml: xmp.as_bytes(), pos: i, state };
        scan.advance(tag_end + 1);
        let end = scan.find(&close)? + close.len();
        return Some((i, end));
    }
    None
}

/// Where a forward scan of XML text is.
#[derive(Clone, Copy, PartialEq)]
enum Markup {
    /// Character data, between tags.
    Text,
    /// Inside a tag: a start or empty-element tag (`start`), or an end tag or declaration; and
    /// inside an attribute value quoted with `quote`.
    Tag { start: bool, quote: Option<u8> },
    /// Inside `<!-- … -->` that starts at byte `start`.
    Comment { start: usize },
    /// Inside `<![CDATA[ … ]]>`.
    Cdata,
    /// Inside a processing instruction, `<? … ?>`, that starts at byte `start`; a `>` alone does
    /// not end it.
    Pi { start: usize },
}

impl Markup {
    /// The state after the byte at `p` of `xml`. A `>` inside a quoted attribute value, which XML
    /// allows, does not end the tag, and a comment or CDATA section is not a tag. A comment's
    /// `-->` and an instruction's `?>` can't share bytes with their own opening: `<!-->` and `<?>`
    /// end nothing.
    fn step(self, xml: &[u8], p: usize) -> Self {
        let byte = xml.get(p).copied();
        let ends = |end: &[u8]| p.checked_sub(end.len() - 1).and_then(|s| xml.get(s..=p)) == Some(end);
        match (self, byte) {
            (Self::Text, Some(b'<')) if xml.get(p..).is_some_and(|s| s.starts_with(b"<!--")) => Self::Comment { start: p },
            (Self::Text, Some(b'<')) if xml.get(p..).is_some_and(|s| s.starts_with(b"<?")) => Self::Pi { start: p },
            (Self::Text, Some(b'<')) if xml.get(p..).is_some_and(|s| s.starts_with(b"<![CDATA[")) => Self::Cdata,
            (Self::Text, Some(b'<')) => Self::Tag { start: !matches!(xml.get(p.saturating_add(1)), Some(b'/' | b'!')), quote: None },
            (Self::Tag { start, quote: None }, Some(q @ (b'"' | b'\''))) => Self::Tag { start, quote: Some(q) },
            (Self::Tag { quote: None, .. }, Some(b'>')) => Self::Text,
            (Self::Tag { start, quote: Some(q) }, Some(b)) if b == q => Self::Tag { start, quote: None },
            (Self::Comment { start }, Some(b'>')) if p >= start.saturating_add(6) && ends(b"-->") => Self::Text,
            (Self::Cdata, Some(b'>')) if ends(b"]]>") => Self::Text,
            (Self::Pi { start }, Some(b'>')) if p >= start.saturating_add(3) && ends(b"?>") => Self::Text,
            (state, _) => state,
        }
    }
}

/// A forward scan of XML text that knows, at each byte, whether it stands in character data.
struct TextScan<'a> {
    xml: &'a [u8],
    pos: usize,
    state: Markup,
}

impl<'a> TextScan<'a> {
    fn new(xml: &'a str) -> Self {
        Self { xml: xml.as_bytes(), pos: 0, state: Markup::Text }
    }

    /// Move forward to byte `to` (never back), following the markup over the bytes passed.
    fn advance(&mut self, to: usize) {
        while self.pos < to.min(self.xml.len()) {
            self.state = self.state.step(self.xml, self.pos);
            self.pos += 1;
        }
    }

    /// The next byte from here, this one included, where `needle` starts in character data; the
    /// scan stays there.
    fn find(&mut self, needle: &str) -> Option<usize> {
        while self.pos < self.xml.len() {
            if self.state == Markup::Text && self.xml.get(self.pos..).is_some_and(|s| s.starts_with(needle.as_bytes())) {
                return Some(self.pos);
            }
            self.state = self.state.step(self.xml, self.pos);
            self.pos += 1;
        }
        None
    }
}

/// Where the start tag that is open at byte `from` of `xml` ends: its `>`, the first one that is
/// not inside a quoted attribute value (XML allows a bare `>` there).
fn tag_end(xml: &str, from: usize) -> Option<usize> {
    let mut quote = None;
    for (i, b) in xml.as_bytes().iter().enumerate().skip(from) {
        match (quote, b) {
            (None, b'>') => return Some(i),
            (None, b'"' | b'\'') => quote = Some(*b),
            (Some(q), b) if *b == q => quote = None,
            _ => {}
        }
    }
    None
}

/// Where the attribute `name` is in `xmp`: its start (including the whitespace before it), the
/// start and end of its value, and its quote character. Only matches inside a start tag (not an
/// end tag, processing instruction, comment or CDATA section), outside any quoted value.
fn attribute_span(xmp: &str, name: &str) -> Option<(usize, usize, usize, char)> {
    let mut from = 0;
    // Where the text before the match stands, found by one pass over the packet rather than by
    // scanning back from every match.
    let (mut scanned, mut state) = (0, Markup::Text);
    while let Some(i) = xmp.get(from..).and_then(|s| s.find(name)).map(|i| i + from) {
        from = i + name.len();
        let before = xmp.get(..i)?;
        #[cfg(test)]
        crate::work(i - scanned);
        state = (scanned..i).fold(state, |state, p| state.step(xmp.as_bytes(), p));
        scanned = i;
        // Preceded by whitespace, and inside a start tag but not inside a quoted value.
        if !before.ends_with(char::is_whitespace) || state != (Markup::Tag { start: true, quote: None }) {
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

    /// Only a list item in character data is an item: not one inside a CDATA section, a processing
    /// instruction or a comment (`<!-->` opens one without closing it), and only the item's own
    /// `xml:lang` attribute is its language, not a lookalike in another attribute's quoted value.
    #[test]
    fn list_items_are_read_from_character_data_only() {
        let get = |content: &str| get_property(&format!("<dc:title><rdf:Alt>{content}</rdf:Alt></dc:title>"), "dc:title", Shape::LangAlt);
        assert_eq!(get("<![CDATA[<rdf:li>Hidden</rdf:li>]]><rdf:li>Shown</rdf:li>").as_deref(), Some("Shown"));
        assert_eq!(get("<?pi > <rdf:li>Hidden</rdf:li> ?><rdf:li>Shown</rdf:li>").as_deref(), Some("Shown"));
        assert_eq!(get("<!--><rdf:li>Hidden</rdf:li>--><rdf:li>Shown</rdf:li>").as_deref(), Some("Shown"));
        let lookalike = "<rdf:li hint=\"a > xml:lang='x-default'\">Draft</rdf:li><rdf:li xml:lang=\"x-default\">Report</rdf:li>";
        assert_eq!(get(lookalike).as_deref(), Some("Report"));
        assert_eq!(get("<rdf:li xml:lang='fr'>Rapport</rdf:li><rdf:li xml:lang='x-default'>Report</rdf:li>").as_deref(), Some("Report"));
        // An element is not found inside `<!-->…-->` or `<?>…?>` either.
        let packet = "<!--><pdf:Producer>Hidden</pdf:Producer>--><pdf:Producer>Shown</pdf:Producer>";
        assert_eq!(get_property(packet, "pdf:Producer", Shape::Text).as_deref(), Some("Shown"));
        let packet = "<?><pdf:Producer>Hidden</pdf:Producer>?><pdf:Producer>Shown</pdf:Producer>";
        assert_eq!(get_property(packet, "pdf:Producer", Shape::Text).as_deref(), Some("Shown"));
    }

    /// An end tag or `</rdf:RDF>` inside a comment is not the real one: replacing a property
    /// replaces all of it, and a new description goes inside the real `rdf:RDF`.
    #[test]
    fn end_tags_inside_comments_are_not_matched() {
        let packet = "<rdf:RDF><rdf:Description rdf:about=\"\"><pdf:Producer>A<!-- </pdf:Producer> -->B</pdf:Producer></rdf:Description></rdf:RDF>";
        let out = set_property(packet, "pdf:Producer", PDF, Shape::Text, "New").unwrap();
        assert_eq!(out, "<rdf:RDF><rdf:Description rdf:about=\"\"><pdf:Producer>New</pdf:Producer></rdf:Description></rdf:RDF>");
        let packet = "<rdf:RDF><rdf:Description rdf:about=\"\"/></rdf:RDF><!-- </rdf:RDF> -->";
        let out = set_property(packet, "pdf:Keywords", PDF, Shape::Text, "k").unwrap();
        assert!(out.ends_with("</rdf:Description>\n</rdf:RDF><!-- </rdf:RDF> -->"), "{out}");
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
