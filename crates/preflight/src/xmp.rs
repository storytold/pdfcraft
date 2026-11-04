//! XMP metadata for PDF/A: the identification schema and the document information, kept
//! consistent (ISO 19005-2 §6.6).

use pdfcraft_cos::Document;

/// The largest XMP packet read; a larger one is treated as missing.
pub(crate) const MAX_XMP_BYTES: usize = 8 * 1024 * 1024;
/// Namespace URIs of the PDF/A and PDF/UA identification schemas.
pub(crate) const PDFA_ID: &str = "http://www.aiim.org/pdfa/ns/id/";
pub(crate) const PDFUA_ID: &str = "http://www.aiim.org/pdfua/ns/id/";

/// The PDF/A and PDF/UA identification a packet declares: each value trimmed, its case kept.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct Identification {
    /// `pdfaid:part`.
    pub pdfa_part: Option<String>,
    /// `pdfaid:conformance`.
    pub pdfa_conformance: Option<String>,
    /// `pdfuaid:part`.
    pub pdfua_part: Option<String>,
}

/// The identification properties read: namespace, conventional prefix, local name, in the order
/// of [`Identification`]'s fields.
const PROPERTIES: [(&str, &str, &str); 3] = [(PDFA_ID, "pdfaid", "part"), (PDFA_ID, "pdfaid", "conformance"), (PDFUA_ID, "pdfuaid", "part")];

/// Read the identification properties by their expanded XML names, not their arbitrary prefixes,
/// in one parse. A packet that is not well-formed XML (an undeclared prefix, an illegal
/// character), or too deep or costly to parse safely, is read by the conventional prefixes
/// (`pdfaid:`, `pdfuaid:`) instead.
pub(crate) fn identification(xmp: &str) -> Identification {
    if xmp.len() > MAX_XMP_BYTES {
        return Identification::default();
    }
    let [pdfa_part, pdfa_conformance, pdfua_part] = match parsed_identification(xmp) {
        Ok(found) => found,
        Err(()) => PROPERTIES.map(|(_, prefix, local)| value(xmp, &format!("{prefix}:{local}"))),
    };
    Identification { pdfa_part, pdfa_conformance, pdfua_part }
}

/// [`identification`] through the XML parser; `Err` when the packet can't be parsed within
/// pdfcraft-xml's bounds and 100,000 nodes. Each property is the first element, in document
/// order, that carries it as an attribute or is named by it (then its text, which may be absent).
fn parsed_identification(xmp: &str) -> Result<[Option<String>; 3], ()> {
    let doc = pdfcraft_xml::parse(xmp, pdfcraft_xml::Limits::new(100_000)).map_err(|_| ())?;
    let mut found: [Option<Option<String>>; 3] = Default::default();
    for node in doc.descendants().filter(roxmltree::Node::is_element) {
        for (slot, (namespace, _, local)) in found.iter_mut().zip(PROPERTIES) {
            if slot.is_some() {
                continue;
            }
            if let Some(value) = node.attribute((namespace, local)) {
                *slot = Some(Some(value.trim().to_owned()));
            } else if node.tag_name().namespace() == Some(namespace) && node.tag_name().name() == local {
                *slot = Some(node.text().map(|value| value.trim().to_owned()));
            }
        }
        if found.iter().all(Option::is_some) {
            break;
        }
    }
    Ok(found.map(Option::flatten))
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

    /// One parse finds every property wherever it is, by namespace and not by prefix, in
    /// attribute or element form; the first element that has a property settles it, even when
    /// that element is empty.
    #[test]
    fn one_parse_reads_every_identification_property() {
        // The second description repeats both parts after the first settled them, while the
        // conformance is still unread.
        let xmp = r#"<x:xmpmeta xmlns:x="adobe:ns:meta/" xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#">
            <rdf:RDF>
                <rdf:Description xmlns:ua="http://www.aiim.org/pdfua/ns/id/" xmlns:id="http://www.aiim.org/pdfa/ns/id/">
                    <ua:part/>
                    <id:part> 2 </id:part>
                </rdf:Description>
                <rdf:Description xmlns:pdfaid="http://www.aiim.org/pdfa/ns/id/" xmlns:pdfuaid="http://www.aiim.org/pdfua/ns/id/" pdfaid:part="3" pdfuaid:part="1"/>
                <rdf:Description xmlns:id="http://www.aiim.org/pdfa/ns/id/" id:conformance=" b "/>
            </rdf:RDF>
        </x:xmpmeta>"#;
        let expected = Identification { pdfa_part: Some("2".into()), pdfa_conformance: Some("b".into()), pdfua_part: None };
        assert_eq!(identification(xmp), expected);
    }

    /// Attribute-heavy packets parse as long as roxmltree's work stays within pdfcraft-xml's budget
    /// (the per-tag and total attribute caps this module once kept refused them); past the budget
    /// the packet is read by the conventional prefix only.
    #[test]
    fn parser_work_is_bounded_by_the_shared_budget() {
        let packet = |attributes: usize| {
            let attrs: String = (0..attributes).map(|i| format!(" x{i}=\"v=1\"")).collect();
            format!(r#"<root xmlns:a="http://www.aiim.org/pdfa/ns/id/" a:part="2"{attrs}/>"#)
        };
        let part = |xmp: &str| identification(xmp).pdfa_part;
        assert_eq!(part(&packet(200)).as_deref(), Some("2"));
        // 12,000 attributes: 1.4·10⁸ comparisons for the duplicate check, past the budget. The
        // fallback looks for `pdfaid:part`, which this packet spells `a:part`.
        assert_eq!(part(&packet(12_000)), None);
        let many_elements = format!(r#"<root xmlns:a="http://www.aiim.org/pdfa/ns/id/" a:part="3">{}</root>"#, r#"<n a="v"/>"#.repeat(5_000));
        assert_eq!(part(&many_elements).as_deref(), Some("3"));
        // Not well-formed (the prefix is never declared): read by the conventional prefix, as before.
        let undeclared = r#"<rdf:Description pdfaid:part="3" pdfaid:conformance="u" pdfuaid:part="1"/>"#;
        let expected = Identification { pdfa_part: Some("3".into()), pdfa_conformance: Some("u".into()), pdfua_part: Some("1".into()) };
        assert_eq!(identification(undeclared), expected);
        let over_bytes = format!("<root>{}</root>", " ".repeat(8 * 1024 * 1024));
        assert_eq!(identification(&over_bytes), Identification::default());
    }
}
