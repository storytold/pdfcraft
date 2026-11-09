//! The PDF/A and PDF/UA identification of a document XMP packet the metadata check removes
//! (#2539 review).
//!
//! The scrub removes the catalog's XMP packet whole when it holds the removed text, and the
//! packet is where a file says it conforms to PDF/A (ISO 19005, the `pdfaid` schema) or PDF/UA
//! (ISO 14289, `pdfuaid`). Rather than drop that claim silently, the scrub writes back a packet
//! holding only those identification properties, read by namespace (not by prefix), each
//! checked to be the short code it must be (a part number, a conformance letter, a year), and
//! none of them holding the removed text. Nothing else of the old packet is copied. What became
//! of each claim is reported ([`Identification`]).

use pdfcraft_cos::{Dict, Document, Object, Stream};

use crate::metadata::Search;

const PDFA_ID: &str = "http://www.aiim.org/pdfa/ns/id/";
const PDFUA_ID: &str = "http://www.aiim.org/pdfua/ns/id/";
/// Packets with more `=` than this (attributes, roughly) aren't parsed: the parser checks each
/// element's attributes against each other, so a huge attribute list would be slow.
const MAX_EQUALS: usize = 4096;
/// The most XML nodes parsed.
const MAX_NODES: u32 = 100_000;
/// The PDF/A extension schema describing `pdfuaid:part`, fixed text.
const PDFUA_EXTENSION: &str = concat!(
    r#"<rdf:Description rdf:about="" xmlns:pdfaExtension="http://www.aiim.org/pdfa/ns/extension/" xmlns:pdfaSchema="http://www.aiim.org/pdfa/ns/schema#" xmlns:pdfaProperty="http://www.aiim.org/pdfa/ns/property#">"#,
    r#"<pdfaExtension:schemas><rdf:Bag><rdf:li rdf:parseType="Resource">"#,
    r#"<pdfaSchema:schema>PDF/UA identification schema</pdfaSchema:schema><pdfaSchema:namespaceURI>http://www.aiim.org/pdfua/ns/id/</pdfaSchema:namespaceURI><pdfaSchema:prefix>pdfuaid</pdfaSchema:prefix>"#,
    r#"<pdfaSchema:property><rdf:Seq><rdf:li rdf:parseType="Resource">"#,
    r#"<pdfaProperty:name>part</pdfaProperty:name><pdfaProperty:valueType>Integer</pdfaProperty:valueType><pdfaProperty:category>internal</pdfaProperty:category><pdfaProperty:description>The part of ISO 14289 the file conforms to</pdfaProperty:description>"#,
    r#"</rdf:li></rdf:Seq></pdfaSchema:property></rdf:li></rdf:Bag></pdfaExtension:schemas></rdf:Description>"#,
);

/// What became of a conformance claim when the packet holding it was removed.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Claim {
    /// No removed packet made the claim.
    #[default]
    None,
    /// The packet written in its place makes it again.
    Kept,
    /// The removed packet made it (or, when it couldn't be read, seems to have), and it couldn't
    /// be written back: a property wasn't a valid code, held the removed text, or the packet
    /// couldn't be read.
    Lost,
}

/// What became of the document's PDF/A and PDF/UA identification when the metadata check
/// removed the catalog's XMP packet.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Identification {
    /// PDF/A (`pdfaid`: part, conformance, and amendment and revision when present).
    pub pdfa: Claim,
    /// PDF/UA (`pdfuaid`: part, and revision when present).
    pub pdfua: Claim,
}

impl Identification {
    /// A claim was lost.
    pub fn lost(&self) -> bool {
        self.pdfa == Claim::Lost || self.pdfua == Claim::Lost
    }
}

/// The identification of the removed catalog packet `xmp`, and the packet to write in its place
/// (`None` when nothing is kept).
pub(crate) fn keep(doc: &Document, xmp: &Object, search: &Search) -> (Identification, Option<Stream>) {
    let Some(text) = crate::metadata::xmp_text(doc, xmp) else {
        // Unreadable: a claim it seems to make, by its namespace or usual prefix in the raw
        // bytes, is lost.
        let seen = |raw: &[u8], ns: &str, prefix: &[u8]| {
            let has = |n: &[u8]| raw.windows(n.len()).any(|w| w == n);
            if has(ns.as_bytes()) || has(prefix) { Claim::Lost } else { Claim::None }
        };
        let id = match &*doc.resolve(xmp) {
            Object::Stream(s) => Identification { pdfa: seen(&s.raw, PDFA_ID, b"pdfaid:"), pdfua: seen(&s.raw, PDFUA_ID, b"pdfuaid:") },
            _ => Identification::default(),
        };
        return (id, None);
    };
    let Some(found) = read(&text) else {
        let seen = |ns: &str, prefix: &str| if text.contains(ns) || text.contains(prefix) { Claim::Lost } else { Claim::None };
        return (Identification { pdfa: seen(PDFA_ID, "pdfaid:"), pdfua: seen(PDFUA_ID, "pdfuaid:") }, None);
    };
    // A value is kept when it is a valid code and doesn't hold the removed text. One letter or
    // digit can't be a term, so it is kept even when everything counts as holding the text.
    let ok = |v: &Option<String>, valid: fn(&str) -> bool| -> Option<String> {
        let v = v.as_deref()?;
        let short = v.chars().filter(|c| c.is_alphanumeric()).count() < 2;
        (valid(v) && (short || !search.text(v))).then(|| v.to_string())
    };
    let mut id = Identification::default();
    let mut body = String::new();

    let a = &found.pdfa;
    if a.any() {
        let part = ok(&a.part, part);
        let conformance = ok(&a.conformance, |v| v.len() == 1 && v.bytes().all(|b| b.is_ascii_alphabetic()));
        // PDF/A-1 to -3 require a conformance level; PDF/A-4 has one only for -4e and -4f.
        if part.is_some() && (conformance.is_some() || a.conformance.is_none()) {
            id.pdfa = Claim::Kept;
            body.push_str(r#"<rdf:Description rdf:about="" xmlns:pdfaid="http://www.aiim.org/pdfa/ns/id/">"#);
            let props = [("part", part), ("conformance", conformance), ("amd", ok(&a.amd, amendment)), ("rev", ok(&a.rev, year))];
            for (name, v) in props {
                if let Some(v) = v {
                    body.push_str(&format!("<pdfaid:{name}>{v}</pdfaid:{name}>"));
                }
            }
            body.push_str("</rdf:Description>");
        } else {
            id.pdfa = Claim::Lost;
        }
    }
    if found.pdfua.any() {
        // PDF/A-1 to -3 accept only the schemas they define or that the packet describes in an
        // extension schema, which pdfuaid needs there. Its fixed text is ours, but a file whose
        // redacted words it happens to use doesn't get it (nor, then, the PDF/UA claim).
        let extension = id.pdfa == Claim::Kept && a.part.as_deref().is_some_and(|p| matches!(p, "1" | "2" | "3"));
        let extension_ok = !extension || !crate::metadata::xml_values(PDFUA_EXTENSION).iter().any(|v| search.text(v));
        match ok(&found.pdfua.part, part).filter(|_| extension_ok) {
            Some(p) => {
                id.pdfua = Claim::Kept;
                // PDF/UA-2 adds a revision year (PDF 2.0, so never beside PDF/A-1 to -3).
                let rev = ok(&found.pdfua.rev, year).map(|r| format!("<pdfuaid:rev>{r}</pdfuaid:rev>")).unwrap_or_default();
                body.push_str(&format!(r#"<rdf:Description rdf:about="" xmlns:pdfuaid="http://www.aiim.org/pdfua/ns/id/"><pdfuaid:part>{p}</pdfuaid:part>{rev}</rdf:Description>"#));
                if extension {
                    body.push_str(PDFUA_EXTENSION);
                }
            }
            None => id.pdfua = Claim::Lost,
        }
    }
    if body.is_empty() {
        return (id, None);
    }
    let packet = format!(
        "<?xpacket begin=\"\u{FEFF}\" id=\"W5M0MpCehiHzreSzNTczkc9d\"?>\n<x:xmpmeta xmlns:x=\"adobe:ns:meta/\"><rdf:RDF xmlns:rdf=\"http://www.w3.org/1999/02/22-rdf-syntax-ns#\">{body}</rdf:RDF></x:xmpmeta>\n<?xpacket end=\"w\"?>"
    );
    let mut dict = Dict::new();
    dict.set(b"Type".to_vec(), Object::name("Metadata"));
    dict.set(b"Subtype".to_vec(), Object::name("XML"));
    // Unfiltered, as PDF/A requires of metadata streams.
    (id, Some(Stream::from_raw(dict, packet.into_bytes())))
}

/// A part number: one or two digits.
fn part(v: &str) -> bool {
    (1..=2).contains(&v.len()) && v.bytes().all(|b| b.is_ascii_digit())
}

/// An amendment identifier, such as "2005" or "1:2005".
fn amendment(v: &str) -> bool {
    (1..=16).contains(&v.len()) && v.bytes().all(|b| b.is_ascii_digit() || b == b':' || b == b'.' || b == b'-')
}

/// A revision year.
fn year(v: &str) -> bool {
    v.len() == 4 && v.bytes().all(|b| b.is_ascii_digit())
}

#[derive(Default)]
struct PdfaId {
    part: Option<String>,
    conformance: Option<String>,
    amd: Option<String>,
    rev: Option<String>,
}

impl PdfaId {
    fn any(&self) -> bool {
        self.part.is_some() || self.conformance.is_some() || self.amd.is_some() || self.rev.is_some()
    }
}

#[derive(Default)]
struct Found {
    pdfa: PdfaId,
    pdfua: PdfaId,
}

/// The identification properties of a packet, by namespace, written as attributes or as
/// elements; the first of each wins. `None` when the packet can't be parsed.
fn read(text: &str) -> Option<Found> {
    if text.bytes().filter(|&b| b == b'=').count() > MAX_EQUALS {
        return None;
    }
    let options = roxmltree::ParsingOptions { allow_dtd: false, nodes_limit: MAX_NODES, ..Default::default() };
    let xml = roxmltree::Document::parse_with_options(text, options).ok()?;
    let mut found = Found::default();
    for node in xml.descendants().filter(roxmltree::Node::is_element) {
        for (ns, id) in [(PDFA_ID, &mut found.pdfa), (PDFUA_ID, &mut found.pdfua)] {
            for (local, slot) in [("part", &mut id.part), ("conformance", &mut id.conformance), ("amd", &mut id.amd), ("rev", &mut id.rev)] {
                if slot.is_some() {
                    continue;
                }
                if let Some(v) = node.attribute((ns, local)) {
                    *slot = Some(v.trim().to_string());
                } else if node.tag_name().namespace() == Some(ns) && node.tag_name().name() == local {
                    *slot = Some(node.text().unwrap_or("").trim().to_string());
                }
            }
        }
    }
    Some(found)
}
