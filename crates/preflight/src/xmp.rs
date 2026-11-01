//! XMP metadata for PDF/A: the identification schema and the document information, kept
//! consistent (ISO 19005-2 §6.6).

use pdfcraft_cos::Document;

/// The largest XMP packet read; a larger one is treated as missing.
pub(crate) const MAX_XMP_BYTES: usize = 8 * 1024 * 1024;
/// Namespace URIs of the PDF/A and PDF/UA identification schemas.
pub(crate) const PDFA_ID: &str = "http://www.aiim.org/pdfa/ns/id/";
pub(crate) const PDFUA_ID: &str = "http://www.aiim.org/pdfua/ns/id/";

/// Read an identification property by its expanded XML name, not its arbitrary prefix. A packet
/// that is not well-formed XML (an undeclared prefix, an illegal character) or is too deep or wide
/// to parse safely is read by the conventional prefix (`pdfaid:`, `pdfuaid:`) instead, as before.
pub(crate) fn identifier(xmp: &str, namespace: &str, local: &str) -> Option<String> {
    if xmp.len() > MAX_XMP_BYTES {
        return None;
    }
    match parsed_identifier(xmp, namespace, local) {
        Ok(found) => found,
        Err(()) => {
            let prefix = match namespace {
                PDFA_ID => "pdfaid",
                PDFUA_ID => "pdfuaid",
                _ => return None,
            };
            value(xmp, &format!("{prefix}:{local}"))
        }
    }
}

/// [`identifier`] through an XML parser; `Err` when the packet can't be parsed within the limits.
fn parsed_identifier(xmp: &str, namespace: &str, local: &str) -> Result<Option<String>, ()> {
    if !xml_limits_ok(xmp) {
        return Err(());
    }
    let options = roxmltree::ParsingOptions { allow_dtd: false, nodes_limit: 100_000, ..Default::default() };
    let doc = roxmltree::Document::parse_with_options(xmp, options).map_err(|_| ())?;
    for node in doc.descendants().filter(roxmltree::Node::is_element) {
        if let Some(value) = node.attribute((namespace, local)) {
            return Ok(Some(value.trim().to_owned()));
        }
        if node.tag_name().namespace() == Some(namespace) && node.tag_name().name() == local {
            return Ok(node.text().map(|value| value.trim().to_owned()));
        }
    }
    Ok(None)
}

/// Reject deep XML and start tags with too many attributes before roxmltree checks duplicates.
/// Each unquoted `=` in a valid start tag introduces an attribute, including namespace bindings.
/// Every `b[i]` and `hay[p..]` below is guarded by `i < b.len()` / `p < hay.len()`.
fn xml_limits_ok(text: &str) -> bool {
    fn find(hay: &[u8], from: usize, needle: &[u8]) -> Option<usize> {
        (from..hay.len()).find(|&p| hay[p..].starts_with(needle))
    }
    let b = text.as_bytes();
    let (mut i, mut depth, mut total_attributes) = (0usize, 0usize, 0usize);
    while i < b.len() {
        if b[i] != b'<' {
            i += 1;
        } else if b[i..].starts_with(b"<!--") {
            i = find(b, i + 4, b"-->").map_or(b.len(), |p| p + 3);
        } else if b[i..].starts_with(b"<![CDATA[") {
            i = find(b, i + 9, b"]]>").map_or(b.len(), |p| p + 3);
        } else if b[i..].starts_with(b"<?") {
            i = find(b, i + 2, b"?>").map_or(b.len(), |p| p + 2);
        } else if b[i..].starts_with(b"<!") {
            i = find(b, i + 2, b">").map_or(b.len(), |p| p + 1);
        } else if b[i..].starts_with(b"</") {
            depth = depth.saturating_sub(1);
            i = find(b, i + 2, b">").map_or(b.len(), |p| p + 1);
        } else {
            depth += 1;
            if depth > 1024 {
                return false;
            }
            // Skip the rest of the start tag, over any quoted attribute value, and remember its last
            // significant byte: a tag that ends in `/` closes itself, so it is one level deep while it
            // is read rather than a level left open.
            i += 1;
            let mut quote = 0u8;
            let mut last = 0u8;
            let mut attributes = 0usize;
            while i < b.len() {
                let c = b[i];
                if quote != 0 {
                    if c == quote {
                        quote = 0;
                    }
                } else if c == b'"' || c == b'\'' {
                    quote = c;
                } else if c == b'>' {
                    i += 1;
                    break;
                } else if c == b'=' {
                    attributes += 1;
                    total_attributes += 1;
                    if attributes > 128 || total_attributes > 4096 {
                        return false;
                    }
                    last = c;
                } else if !c.is_ascii_whitespace() {
                    last = c;
                }
                i += 1;
            }
            if last == b'/' {
                depth -= 1;
            }
        }
    }
    true
}

/// The value of a simple XMP property, written either as an attribute (`pdfaid:part="2"`) or
/// as an element (`<pdfaid:part>2</pdfaid:part>`, or inside an `rdf:Alt`/`rdf:Seq`).
pub fn value(xmp: &str, key: &str) -> Option<String> {
    let attr = format!("{key}=\"");
    if let Some(i) = xmp.find(&attr) {
        let rest = &xmp[i + attr.len()..];
        return rest.find('"').map(|e| unescape(&rest[..e]));
    }
    let attr1 = format!("{key}='");
    if let Some(i) = xmp.find(&attr1) {
        let rest = &xmp[i + attr1.len()..];
        return rest.find('\'').map(|e| unescape(&rest[..e]));
    }
    let open = format!("<{key}>");
    let close = format!("</{key}>");
    let start = xmp.find(&open)? + open.len();
    let end = start + xmp[start..].find(&close)?;
    let inner = &xmp[start..end];
    // rdf:Alt / rdf:Seq: the first rdf:li.
    if let Some(li) = inner.find("<rdf:li") {
        let s = li + inner[li..].find('>')? + 1;
        let e = s + inner[s..].find("</rdf:li>")?;
        return Some(unescape(inner[s..e].trim()));
    }
    Some(unescape(inner.trim()))
}

fn unescape(s: &str) -> String {
    s.replace("&lt;", "<").replace("&gt;", ">").replace("&quot;", "\"").replace("&apos;", "'").replace("&amp;", "&")
}

fn esc(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;")
}

/// A PDF date (`D:YYYYMMDDHHmmSSOHH'mm'`) as an XMP (ISO 8601) date.
pub fn iso_date(pdf: &str) -> Option<String> {
    let d = pdf.trim().trim_start_matches("D:");
    let digits: String = d.chars().take_while(char::is_ascii_digit).collect();
    if digits.len() < 4 {
        return None;
    }
    let part = |a: usize, b: usize, def: &str| digits.get(a..b).unwrap_or(def).to_string();
    let (y, mo, da, h, mi, s) = (part(0, 4, "0000"), part(4, 6, "01"), part(6, 8, "01"), part(8, 10, "00"), part(10, 12, "00"), part(12, 14, "00"));
    let rest = &d[digits.len()..];
    let tz = match rest.chars().next() {
        Some('Z') | None => "Z".to_string(),
        Some(sign @ ('+' | '-')) => {
            let t: String = rest[1..].chars().filter(char::is_ascii_digit).collect();
            format!("{sign}{}:{}", t.get(0..2).unwrap_or("00"), t.get(2..4).unwrap_or("00"))
        }
        _ => "Z".to_string(),
    };
    Some(format!("{y}-{mo}-{da}T{h}:{mi}:{s}{tz}"))
}

fn info(doc: &Document, key: &str) -> Option<String> {
    let i = doc.trailer().get(b"Info").map(|i| doc.resolve(i))?;
    i.as_dict()?.get(key.as_bytes()).and_then(|v| doc.resolve(v).as_string().map(|s| s.to_text())).filter(|s| !s.is_empty())
}

/// The XMP packet for `doc` declaring `level`, from its document information. A PDF/UA
/// identification the document's metadata had is kept.
pub fn packet(doc: &Document, level: crate::Level) -> String {
    let pdfua = crate::metadata(doc).and_then(|x| value(&x, "pdfuaid:part")).and_then(|p| p.trim().parse().ok());
    packet_with(doc, &Ids { pdfa: Some((level.part(), level.conformance())), pdfua, ..Ids::default() })
}

/// What a packet identifies besides the document information.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct Ids {
    /// PDF/A part and conformance letter.
    pub pdfa: Option<(u8, &'static str)>,
    /// PDF/UA part.
    pub pdfua: Option<u8>,
    /// `pdfxid:GTS_PDFXVersion` (PDF/X-4).
    pub pdfx: Option<&'static str>,
    /// `pdf:Trapped`.
    pub trapped: Option<bool>,
    /// `xmpMM:DocumentID`; with it, `xmpMM:InstanceID`, `xmpMM:VersionID` and
    /// `xmpMM:RenditionClass` are written too.
    pub document_id: Option<String>,
    /// `xmp:MetadataDate`, as a PDF date.
    pub now: Option<String>,
}

/// An extension schema (ISO 19005-2 6.6.2.3): PDF/A accepts properties outside the predefined
/// schemas only when the packet describes them.
fn extension(name: &str, uri: &str, prefix: &str, property: &str, value_type: &str, description: &str) -> String {
    format!(
        "     <rdf:li rdf:parseType=\"Resource\">\n\
      <pdfaSchema:schema>{name}</pdfaSchema:schema>\n\
      <pdfaSchema:namespaceURI>{uri}</pdfaSchema:namespaceURI>\n\
      <pdfaSchema:prefix>{prefix}</pdfaSchema:prefix>\n\
      <pdfaSchema:property><rdf:Seq><rdf:li rdf:parseType=\"Resource\">\n\
       <pdfaProperty:name>{property}</pdfaProperty:name>\n\
       <pdfaProperty:valueType>{value_type}</pdfaProperty:valueType>\n\
       <pdfaProperty:category>internal</pdfaProperty:category>\n\
       <pdfaProperty:description>{description}</pdfaProperty:description>\n\
      </rdf:li></rdf:Seq></pdfaSchema:property>\n\
     </rdf:li>\n"
    )
}

/// The XMP packet for `doc`: its document information and `ids`.
pub(crate) fn packet_with(doc: &Document, ids: &Ids) -> String {
    let mut props = String::new();
    if let Some(t) = info(doc, "Title") {
        props.push_str(&format!("   <dc:title><rdf:Alt><rdf:li xml:lang=\"x-default\">{}</rdf:li></rdf:Alt></dc:title>\n", esc(&t)));
    }
    if let Some(a) = info(doc, "Author") {
        props.push_str(&format!("   <dc:creator><rdf:Seq><rdf:li>{}</rdf:li></rdf:Seq></dc:creator>\n", esc(&a)));
    }
    if let Some(s) = info(doc, "Subject") {
        props.push_str(&format!("   <dc:description><rdf:Alt><rdf:li xml:lang=\"x-default\">{}</rdf:li></rdf:Alt></dc:description>\n", esc(&s)));
    }
    if let Some(k) = info(doc, "Keywords") {
        props.push_str(&format!("   <pdf:Keywords>{}</pdf:Keywords>\n", esc(&k)));
    }
    if let Some(p) = info(doc, "Producer") {
        props.push_str(&format!("   <pdf:Producer>{}</pdf:Producer>\n", esc(&p)));
    }
    if let Some(c) = info(doc, "Creator") {
        props.push_str(&format!("   <xmp:CreatorTool>{}</xmp:CreatorTool>\n", esc(&c)));
    }
    if let Some(d) = info(doc, "CreationDate").and_then(|d| iso_date(&d)) {
        props.push_str(&format!("   <xmp:CreateDate>{d}</xmp:CreateDate>\n"));
    }
    if let Some(d) = info(doc, "ModDate").and_then(|d| iso_date(&d)) {
        props.push_str(&format!("   <xmp:ModifyDate>{d}</xmp:ModifyDate>\n"));
    }
    if let Some(d) = ids.now.as_deref().and_then(iso_date) {
        props.push_str(&format!("   <xmp:MetadataDate>{d}</xmp:MetadataDate>\n"));
    }
    if let Some(t) = ids.trapped {
        props.push_str(&format!("   <pdf:Trapped>{}</pdf:Trapped>\n", if t { "True" } else { "False" }));
    }
    let mut namespaces = String::from(
        "    xmlns:dc=\"http://purl.org/dc/elements/1.1/\"\n    xmlns:pdf=\"http://ns.adobe.com/pdf/1.3/\"\n    xmlns:xmp=\"http://ns.adobe.com/xap/1.0/\"",
    );
    if let Some(id) = &ids.document_id {
        namespaces.push_str("\n    xmlns:xmpMM=\"http://ns.adobe.com/xap/1.0/mm/\"");
        props.push_str(&format!("   <xmpMM:DocumentID>{}</xmpMM:DocumentID>\n", esc(id)));
        props.push_str(&format!("   <xmpMM:InstanceID>{}</xmpMM:InstanceID>\n", esc(id)));
        props.push_str("   <xmpMM:VersionID>1</xmpMM:VersionID>\n   <xmpMM:RenditionClass>default</xmpMM:RenditionClass>\n");
    }
    if let Some((part, conformance)) = ids.pdfa {
        namespaces.push_str("\n    xmlns:pdfaid=\"http://www.aiim.org/pdfa/ns/id/\"");
        props.push_str(&format!("   <pdfaid:part>{part}</pdfaid:part>\n   <pdfaid:conformance>{conformance}</pdfaid:conformance>\n"));
    }
    if let Some(part) = ids.pdfua {
        namespaces.push_str("\n    xmlns:pdfuaid=\"http://www.aiim.org/pdfua/ns/id/\"");
        props.push_str(&format!("   <pdfuaid:part>{part}</pdfuaid:part>\n"));
    }
    if let Some(version) = ids.pdfx {
        namespaces.push_str("\n    xmlns:pdfxid=\"http://www.npes.org/pdfx/ns/id/\"");
        props.push_str(&format!("   <pdfxid:GTS_PDFXVersion>{}</pdfxid:GTS_PDFXVersion>\n", esc(version)));
    }
    // A PDF/A packet describes the schemas PDF/A doesn't predefine.
    let mut schemas = String::new();
    if ids.pdfa.is_some() {
        if ids.pdfua.is_some() {
            schemas.push_str(&extension(
                "PDF/UA Universal Accessibility Schema",
                "http://www.aiim.org/pdfua/ns/id/",
                "pdfuaid",
                "part",
                "Integer",
                "The part of ISO 14289 the document conforms to",
            ));
        }
        if ids.pdfx.is_some() {
            schemas.push_str(&extension(
                "PDF/X Identification Schema",
                "http://www.npes.org/pdfx/ns/id/",
                "pdfxid",
                "GTS_PDFXVersion",
                "Text",
                "The PDF/X version the document conforms to",
            ));
        }
    }
    let extensions = if schemas.is_empty() {
        String::new()
    } else {
        format!(
            "  <rdf:Description rdf:about=\"\"\n\
    xmlns:pdfaExtension=\"http://www.aiim.org/pdfa/ns/extension/\"\n\
    xmlns:pdfaSchema=\"http://www.aiim.org/pdfa/ns/schema#\"\n\
    xmlns:pdfaProperty=\"http://www.aiim.org/pdfa/ns/property#\">\n\
   <pdfaExtension:schemas>\n\
    <rdf:Bag>\n\
{schemas}    </rdf:Bag>\n\
   </pdfaExtension:schemas>\n\
  </rdf:Description>\n"
        )
    };
    format!(
        "<?xpacket begin=\"\u{feff}\" id=\"W5M0MpCehiHzreSzNTczkc9d\"?>\n\
<x:xmpmeta xmlns:x=\"adobe:ns:meta/\">\n\
 <rdf:RDF xmlns:rdf=\"http://www.w3.org/1999/02/22-rdf-syntax-ns#\">\n\
  <rdf:Description rdf:about=\"\"\n\
{namespaces}>\n\
{props}  </rdf:Description>\n\
{extensions} </rdf:RDF>\n\
</x:xmpmeta>\n\
<?xpacket end=\"w\"?>"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_attribute_and_element_forms() {
        let a = "<rdf:Description pdfaid:part=\"2\" pdfaid:conformance='B'/>";
        assert_eq!(value(a, "pdfaid:part").as_deref(), Some("2"));
        assert_eq!(value(a, "pdfaid:conformance").as_deref(), Some("B"));
        let e = "<dc:title><rdf:Alt><rdf:li xml:lang=\"x-default\">A &amp; B</rdf:li></rdf:Alt></dc:title><pdfaid:part>3</pdfaid:part>";
        assert_eq!(value(e, "dc:title").as_deref(), Some("A & B"));
        assert_eq!(value(e, "pdfaid:part").as_deref(), Some("3"));
        assert_eq!(value(e, "pdfuaid:part"), None);
    }

    #[test]
    fn pdf_dates_become_iso_dates() {
        assert_eq!(iso_date("D:20240105093000+01'00'").as_deref(), Some("2024-01-05T09:30:00+01:00"));
        assert_eq!(iso_date("D:20240105").as_deref(), Some("2024-01-05T00:00:00Z"));
        assert_eq!(iso_date("D:20240105120000Z").as_deref(), Some("2024-01-05T12:00:00Z"));
        assert_eq!(iso_date("junk"), None);
    }
}

#[cfg(test)]
mod limit_tests {
    use super::*;

    #[test]
    fn attribute_and_namespace_budgets_bound_parser_work() {
        let prefix = r#"<root xmlns:a="http://www.aiim.org/pdfa/ns/id/" a:part="2" "#;
        let attrs = |count| (0..count).map(|i| format!("x{i}=\"v=1\" ")).collect::<String>();
        let at_limit = format!("{prefix}{}/>", attrs(126));
        assert_eq!(identifier(&at_limit, PDFA_ID, "part").as_deref(), Some("2"));
        let over_limit = format!("{prefix}{}/>", attrs(127));
        assert_eq!(identifier(&over_limit, PDFA_ID, "part"), None);
        let namespaces = (0..129).map(|i| format!("xmlns:n{i}=\"urn:{i}\" ")).collect::<String>();
        assert_eq!(identifier(&format!("<root {namespaces}/>"), PDFA_ID, "part"), None);
        let many_elements = format!("<root>{}</root>", r#"<n a="v"/>"#.repeat(4097));
        assert!(!xml_limits_ok(&many_elements));
        // Not well-formed (the prefix is never declared): read by the conventional prefix, as before.
        let undeclared = r#"<rdf:Description pdfaid:part="3" pdfaid:conformance="u"/>"#;
        assert_eq!(identifier(undeclared, PDFA_ID, "part").as_deref(), Some("3"));
        assert_eq!(identifier(undeclared, PDFA_ID, "conformance").as_deref(), Some("u"));
        let over_bytes = format!("<root>{}</root>", " ".repeat(8 * 1024 * 1024));
        assert_eq!(identifier(&over_bytes, PDFA_ID, "part"), None);
    }
}
