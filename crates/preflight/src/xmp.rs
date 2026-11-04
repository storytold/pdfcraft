//! XMP metadata for PDF/A: the identification schema and the document information, kept
//! consistent (ISO 19005-2 §6.6).

use pdfcraft_cos::Document;

/// The largest XMP packet read; a larger one is treated as missing.
pub(crate) const MAX_XMP_BYTES: usize = 8 * 1024 * 1024;
/// Namespace URIs of the PDF/A and PDF/UA identification schemas.
pub(crate) const PDFA_ID: &str = "http://www.aiim.org/pdfa/ns/id/";
pub(crate) const PDFUA_ID: &str = "http://www.aiim.org/pdfua/ns/id/";

/// Read an identification property by its expanded XML name, not its arbitrary prefix. A packet
/// that is not well-formed XML (an undeclared prefix, an illegal character), or too deep or costly
/// to parse safely, is read by the conventional prefix (`pdfaid:`, `pdfuaid:`) instead, as before.
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

/// [`identifier`] through the XML parser; `Err` when the packet can't be parsed within
/// pdfcraft-xml's bounds and 100,000 nodes.
fn parsed_identifier(xmp: &str, namespace: &str, local: &str) -> Result<Option<String>, ()> {
    let doc = pdfcraft_xml::parse(xmp, pdfcraft_xml::Limits::new(100_000)).map_err(|_| ())?;
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

/// The XMP packet for `doc` declaring `level`, from its document information.
pub fn packet(doc: &Document, level: crate::Level) -> String {
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
    format!(
        "<?xpacket begin=\"\u{feff}\" id=\"W5M0MpCehiHzreSzNTczkc9d\"?>\n\
<x:xmpmeta xmlns:x=\"adobe:ns:meta/\">\n\
 <rdf:RDF xmlns:rdf=\"http://www.w3.org/1999/02/22-rdf-syntax-ns#\">\n\
  <rdf:Description rdf:about=\"\"\n\
    xmlns:dc=\"http://purl.org/dc/elements/1.1/\"\n\
    xmlns:pdf=\"http://ns.adobe.com/pdf/1.3/\"\n\
    xmlns:xmp=\"http://ns.adobe.com/xap/1.0/\"\n\
    xmlns:pdfaid=\"http://www.aiim.org/pdfa/ns/id/\">\n\
   <pdfaid:part>{}</pdfaid:part>\n\
   <pdfaid:conformance>B</pdfaid:conformance>\n\
{props}  </rdf:Description>\n\
 </rdf:RDF>\n\
</x:xmpmeta>\n\
<?xpacket end=\"w\"?>",
        level.part()
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

    /// Attribute-heavy packets parse as long as roxmltree's work stays within pdfcraft-xml's budget
    /// (the per-tag and total attribute caps this module once kept refused them); past the budget
    /// the packet is read by the conventional prefix only.
    #[test]
    fn parser_work_is_bounded_by_the_shared_budget() {
        let packet = |attributes: usize| {
            let attrs: String = (0..attributes).map(|i| format!(" x{i}=\"v=1\"")).collect();
            format!(r#"<root xmlns:a="http://www.aiim.org/pdfa/ns/id/" a:part="2"{attrs}/>"#)
        };
        assert_eq!(identifier(&packet(200), PDFA_ID, "part").as_deref(), Some("2"));
        // 12,000 attributes: 1.4·10⁸ comparisons for the duplicate check, past the budget. The
        // fallback looks for `pdfaid:part`, which this packet spells `a:part`.
        assert_eq!(identifier(&packet(12_000), PDFA_ID, "part"), None);
        let many_elements = format!(r#"<root xmlns:a="http://www.aiim.org/pdfa/ns/id/" a:part="3">{}</root>"#, r#"<n a="v"/>"#.repeat(5_000));
        assert_eq!(identifier(&many_elements, PDFA_ID, "part").as_deref(), Some("3"));
        // Not well-formed (the prefix is never declared): read by the conventional prefix, as before.
        let undeclared = r#"<rdf:Description pdfaid:part="3" pdfaid:conformance="u"/>"#;
        assert_eq!(identifier(undeclared, PDFA_ID, "part").as_deref(), Some("3"));
        assert_eq!(identifier(undeclared, PDFA_ID, "conformance").as_deref(), Some("u"));
        let over_bytes = format!("<root>{}</root>", " ".repeat(8 * 1024 * 1024));
        assert_eq!(identifier(&over_bytes, PDFA_ID, "part"), None);
    }
}
