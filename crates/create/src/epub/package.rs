//! The book's package: `META-INF/container.xml` → the package document (metadata, manifest,
//! spine, layout), the table of contents (EPUB 3 navigation document or EPUB 2 NCX), and what
//! `META-INF/encryption.xml` says. Font obfuscation (IDPF and Adobe) is undone as the specification
//! says, which is what every reading system does; any other encryption, and the rights files of
//! DRM schemes, mean the book is protected and isn't converted.

use std::collections::HashMap;

use super::EpubError;
use super::zip::Zip;

const OPF: &str = "http://www.idpf.org/2007/opf";
const DC: &str = "http://purl.org/dc/elements/1.1/";
const IDPF_OBFUSCATION: &str = "http://www.idpf.org/2008/embedding";
const ADOBE_OBFUSCATION: &str = "http://ns.adobe.com/pdf/enc#RC";

/// A spine entry: one content document in reading order.
#[derive(Clone, Debug, PartialEq)]
pub struct SpineItem {
    /// Its path in the container.
    pub path: String,
    /// Laid out as fixed pages (`rendition:layout` pre-paginated, for the book or this item).
    pub fixed: bool,
}

/// A table-of-contents entry, as the book gives it.
#[derive(Clone, Debug, PartialEq)]
pub struct NavEntry {
    pub title: String,
    /// Path in the container, with any `#fragment`.
    pub target: String,
    /// 1 for the top level.
    pub level: u8,
}

/// How a font file is obfuscated.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Obfuscation {
    Idpf,
    Adobe,
}

/// What the package says.
#[derive(Clone, Debug, Default)]
pub struct Package {
    pub title: Option<String>,
    pub creators: Vec<String>,
    pub language: Option<String>,
    /// The package's unique identifier (the obfuscation key's source).
    pub identifier: String,
    pub spine: Vec<SpineItem>,
    /// Container path → media type.
    pub media: HashMap<String, String>,
    pub toc: Vec<NavEntry>,
    /// `page-progression-direction="rtl"`.
    pub rtl: bool,
    /// Container path → how the font is obfuscated.
    pub obfuscated: HashMap<String, Obfuscation>,
}

/// Percent-decoded `href` resolved against the directory of `base` (a container path), `.` and
/// `..` steps applied; `None` when it leaves the container or is not a local file.
pub fn resolve(base: &str, href: &str) -> Option<String> {
    let href = href.trim();
    if href.is_empty() || href.contains("://") || href.starts_with("data:") || href.starts_with("mailto:") || href.starts_with('/') {
        return None;
    }
    let (file, fragment) = match href.split_once('#') {
        Some((f, frag)) => (f, Some(frag)),
        None => (href, None),
    };
    let decoded = percent_encoding::percent_decode_str(file).decode_utf8().ok()?;
    let mut parts: Vec<&str> = match base.rsplit_once('/') {
        Some((dir, _)) if !file.is_empty() => dir.split('/').collect(),
        _ if file.is_empty() => {
            // A fragment alone points into `base` itself.
            return Some(match fragment {
                Some(f) => format!("{base}#{f}"),
                None => base.to_string(),
            });
        }
        _ => Vec::new(),
    };
    for part in decoded.split('/') {
        match part {
            "" | "." => {}
            ".." => {
                parts.pop()?;
            }
            p => parts.push(p),
        }
    }
    let path = parts.join("/");
    Some(match fragment {
        Some(f) => format!("{path}#{f}"),
        None => path,
    })
}

/// XML with the HTML entities XHTML books use without declaring (`&nbsp;`, `&mdash;`…) written as
/// character references, so an XML parser reads them; an unknown entity is kept as text.
pub fn xml_text(bytes: &[u8]) -> String {
    let text = crate::decode_text(bytes);
    if !text.contains('&') {
        return text;
    }
    let mut out = String::with_capacity(text.len());
    let mut rest = text.as_str();
    while let Some(i) = rest.find('&') {
        out.push_str(&rest[..i]);
        rest = &rest[i..];
        let end = rest[1..].find(|c: char| !c.is_ascii_alphanumeric()).map(|e| e + 1);
        match end {
            Some(e) if rest[e..].starts_with(';') && e > 1 => {
                let name = &rest[1..e];
                if matches!(name, "amp" | "lt" | "gt" | "quot" | "apos") {
                    out.push_str(&rest[..=e]);
                } else if let Some(c) = super::entities::lookup(name) {
                    out.push_str(&format!("&#{};", u32::from(c)));
                } else {
                    out.push_str("&amp;");
                    out.push_str(&rest[1..=e]);
                }
                rest = &rest[e + 1..];
            }
            _ if rest[1..].starts_with('#') => {
                out.push('&');
                rest = &rest[1..];
            }
            _ => {
                out.push_str("&amp;");
                rest = &rest[1..];
            }
        }
    }
    out.push_str(rest);
    out
}

/// How deep elements may nest in a file the parser is given. roxmltree recurses once per level
/// while it reads (and while it frees the tree), so tens of thousands of nested elements end the
/// whole process with a stack overflow, which nothing can catch. Real books stay below 50.
pub const MAX_XML_DEPTH: usize = 512;

/// Whether `text` nests elements deeper than `max`, found by one pass over its tags (quoted
/// attribute values, comments, CDATA, processing instructions and declarations are skipped).
pub fn nests_deeper_than(text: &str, max: usize) -> bool {
    let b = text.as_bytes();
    let (mut i, mut depth) = (0, 0usize);
    while let Some(at) = b[i..].iter().position(|&c| c == b'<') {
        i += at;
        let rest = &b[i..];
        let skip_to = |end: &[u8]| rest.windows(end.len()).position(|w| w == end).map(|n| n + end.len()).unwrap_or(rest.len());
        if rest.starts_with(b"<!--") {
            i += skip_to(b"-->");
        } else if rest.starts_with(b"<![CDATA[") {
            i += skip_to(b"]]>");
        } else if rest.starts_with(b"<?") {
            i += skip_to(b"?>");
        } else if rest.starts_with(b"<!") {
            i += skip_to(b">");
        } else {
            let closing = rest.get(1) == Some(&b'/');
            let mut quote = None;
            let mut end = rest.len();
            for (k, &c) in rest.iter().enumerate().skip(1) {
                match (quote, c) {
                    (Some(q), c) if c == q => quote = None,
                    (Some(_), _) => {}
                    (None, b'"' | b'\'') => quote = Some(c),
                    (None, b'>') => {
                        end = k + 1;
                        break;
                    }
                    _ => {}
                }
            }
            let empty = !closing && end >= 2 && rest.get(end - 2) == Some(&b'/');
            if closing {
                depth = depth.saturating_sub(1);
            } else if !empty {
                depth += 1;
                if depth > max {
                    return true;
                }
            }
            i += end;
        }
        if i >= b.len() {
            break;
        }
    }
    false
}

/// Parse XML, DTDs allowed (EPUB 2 content documents carry the XHTML DOCTYPE). A file nested
/// deeper than MAX_XML_DEPTH is refused as if it were not well-formed.
pub fn parse(text: &str) -> Result<roxmltree::Document<'_>, roxmltree::Error> {
    if nests_deeper_than(text, MAX_XML_DEPTH) {
        return Err(roxmltree::Error::NodesLimitReached);
    }
    roxmltree::Document::parse_with_options(text, roxmltree::ParsingOptions { allow_dtd: true, nodes_limit: 4_000_000 })
}

fn text_of(n: roxmltree::Node) -> String {
    n.descendants().filter(|d| d.is_text()).filter_map(|d| d.text()).collect::<String>().split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Refuse a protected book: rights files of DRM schemes, or encryption other than font
/// obfuscation. Returns the obfuscated fonts otherwise.
fn encryption(zip: &Zip) -> Result<HashMap<String, Obfuscation>, EpubError> {
    if zip.has("META-INF/rights.xml") || zip.has("META-INF/license.lcpl") {
        return Err(EpubError::Protected);
    }
    let mut out = HashMap::new();
    let Some(bytes) = zip.read("META-INF/encryption.xml")? else { return Ok(out) };
    let text = xml_text(&bytes);
    let xml = parse(&text).map_err(|_| EpubError::Protected)?;
    for data in xml.descendants().filter(|n| n.has_tag_name("EncryptedData") || n.tag_name().name() == "EncryptedData") {
        let algorithm =
            data.descendants().find(|n| n.tag_name().name() == "EncryptionMethod").and_then(|n| n.attribute("Algorithm")).unwrap_or_default();
        let uri = data.descendants().find(|n| n.tag_name().name() == "CipherReference").and_then(|n| n.attribute("URI")).unwrap_or_default();
        let kind = match algorithm {
            IDPF_OBFUSCATION => Obfuscation::Idpf,
            ADOBE_OBFUSCATION => Obfuscation::Adobe,
            _ => return Err(EpubError::Protected),
        };
        // Paths in encryption.xml are relative to the container's root.
        if let Some(path) = resolve("", uri) {
            out.insert(path, kind);
        }
    }
    Ok(out)
}

/// Undo font obfuscation (EPUB 3 §4 "Font obfuscation"; Adobe's variant keyed by the UUID).
pub fn deobfuscate(font: &mut [u8], kind: Obfuscation, identifier: &str) {
    match kind {
        Obfuscation::Idpf => {
            use sha1::Digest;
            let id: String = identifier.chars().filter(|c| !matches!(c, ' ' | '\t' | '\n' | '\r')).collect();
            let key = sha1::Sha1::digest(id.as_bytes());
            for (i, b) in font.iter_mut().take(1040).enumerate() {
                *b ^= key[i % key.len()];
            }
        }
        Obfuscation::Adobe => {
            let hex: String = identifier.trim().trim_start_matches("urn:uuid:").chars().filter(char::is_ascii_hexdigit).collect();
            let key: Vec<u8> = (0..hex.len() / 2).filter_map(|i| u8::from_str_radix(&hex[2 * i..2 * i + 2], 16).ok()).collect();
            if key.len() != 16 {
                return;
            }
            for (i, b) in font.iter_mut().take(1024).enumerate() {
                *b ^= key[i % 16];
            }
        }
    }
}

/// The EPUB 3 navigation document's `toc` list.
fn nav_toc(zip: &Zip, path: &str) -> Result<Vec<NavEntry>, EpubError> {
    let Some(bytes) = zip.read(path)? else { return Ok(Vec::new()) };
    let text = xml_text(&bytes);
    let Ok(xml) = parse(&text) else { return Ok(Vec::new()) };
    let Some(nav) = xml
        .descendants()
        .filter(|n| n.tag_name().name() == "nav")
        .find(|n| n.attributes().any(|a| a.name() == "type" && a.value().split_whitespace().any(|t| t == "toc")))
        .or_else(|| xml.descendants().find(|n| n.tag_name().name() == "nav"))
    else {
        return Ok(Vec::new());
    };
    let mut out = Vec::new();
    fn walk(list: roxmltree::Node, level: u8, base: &str, out: &mut Vec<NavEntry>) {
        for li in list.children().filter(|c| c.tag_name().name() == "li") {
            let label = li.children().find(|c| matches!(c.tag_name().name(), "a" | "span"));
            if let Some(a) = label {
                let title = text_of(a);
                let target = a.attribute("href").and_then(|h| resolve(base, h)).unwrap_or_default();
                if !title.is_empty() {
                    out.push(NavEntry { title, target, level });
                }
            }
            // The level stops at 32 and so does the walk, which recurses.
            if level < 32 {
                for sub in li.children().filter(|c| c.tag_name().name() == "ol") {
                    walk(sub, level.saturating_add(1), base, out);
                }
            }
        }
    }
    for ol in nav.children().filter(|c| c.tag_name().name() == "ol") {
        walk(ol, 1, path, &mut out);
    }
    Ok(out)
}

/// The EPUB 2 NCX's navMap.
fn ncx_toc(zip: &Zip, path: &str) -> Result<Vec<NavEntry>, EpubError> {
    let Some(bytes) = zip.read(path)? else { return Ok(Vec::new()) };
    let text = xml_text(&bytes);
    let Ok(xml) = parse(&text) else { return Ok(Vec::new()) };
    let mut out = Vec::new();
    fn walk(parent: roxmltree::Node, level: u8, base: &str, out: &mut Vec<NavEntry>) {
        for point in parent.children().filter(|c| c.tag_name().name() == "navPoint") {
            let title = point.children().find(|c| c.tag_name().name() == "navLabel").map(text_of).unwrap_or_default();
            let target = point
                .children()
                .find(|c| c.tag_name().name() == "content")
                .and_then(|c| c.attribute("src"))
                .and_then(|s| resolve(base, s))
                .unwrap_or_default();
            if !title.is_empty() {
                out.push(NavEntry { title, target, level });
            }
            // The level stops at 32 and so does the walk, which recurses.
            if level < 32 {
                walk(point, level.saturating_add(1), base, out);
            }
        }
    }
    if let Some(map) = xml.descendants().find(|n| n.tag_name().name() == "navMap") {
        walk(map, 1, path, &mut out);
    }
    Ok(out)
}

/// Read the book's package. Refuses a protected book ([`EpubError::Protected`]) before reading
/// anything else.
pub fn read(zip: &Zip) -> Result<Package, EpubError> {
    let obfuscated = encryption(zip)?;
    let container = zip.read("META-INF/container.xml")?.ok_or_else(|| EpubError::Damaged("META-INF/container.xml is missing".into()))?;
    let container = xml_text(&container);
    let container = parse(&container).map_err(|e| EpubError::Damaged(format!("META-INF/container.xml: {e}")))?;
    let opf_path = container
        .descendants()
        .filter(|n| n.tag_name().name() == "rootfile")
        .find(|n| n.attribute("media-type").is_none_or(|m| m == "application/oebps-package+xml"))
        .and_then(|n| n.attribute("full-path"))
        .and_then(|p| resolve("", p))
        .ok_or_else(|| EpubError::Damaged("the container names no package document".into()))?;
    let opf = zip.read(&opf_path)?.ok_or_else(|| EpubError::Damaged(format!("the package document {opf_path} is missing")))?;
    let opf = xml_text(&opf);
    let xml = parse(&opf).map_err(|e| EpubError::Damaged(format!("{opf_path}: {e}")))?;
    let root = xml.root_element();
    let mut p = Package { obfuscated, ..Package::default() };

    let unique = root.attribute("unique-identifier");
    let metadata = root.children().find(|n| n.tag_name().name() == "metadata");
    let mut book_fixed = false;
    if let Some(m) = metadata {
        for n in m.descendants().filter(roxmltree::Node::is_element) {
            let ns = n.tag_name().namespace();
            let value = text_of(n);
            match (ns, n.tag_name().name()) {
                (Some(DC), "title") if p.title.is_none() && !value.is_empty() => p.title = Some(value),
                (Some(DC), "creator") if !value.is_empty() => p.creators.push(value),
                (Some(DC), "language") if p.language.is_none() && !value.is_empty() => p.language = Some(value),
                (Some(DC), "identifier") if p.identifier.is_empty() || n.attribute("id") == unique => {
                    if n.attribute("id") == unique || p.identifier.is_empty() {
                        p.identifier = value;
                    }
                }
                (_, "meta") if n.attribute("property") == Some("rendition:layout") => book_fixed = value == "pre-paginated",
                _ => {}
            }
        }
    }

    let manifest = root.children().find(|n| n.tag_name().name() == "manifest");
    let mut by_id: HashMap<&str, (String, &str)> = HashMap::new();
    let mut nav_path = None;
    if let Some(m) = manifest {
        for item in m.children().filter(|n| n.tag_name().name() == "item") {
            let (Some(id), Some(href)) = (item.attribute("id"), item.attribute("href")) else { continue };
            let Some(path) = resolve(&opf_path, href) else { continue };
            let media = item.attribute("media-type").unwrap_or_default();
            if item.attribute("properties").is_some_and(|ps| ps.split_whitespace().any(|x| x == "nav")) {
                nav_path = Some(path.clone());
            }
            p.media.insert(path.clone(), media.to_string());
            by_id.insert(id, (path, media));
        }
    }

    let spine = root.children().find(|n| n.tag_name().name() == "spine").ok_or_else(|| EpubError::Damaged("the package has no spine".into()))?;
    p.rtl = spine.attribute("page-progression-direction") == Some("rtl");
    let ncx_path = spine
        .attribute("toc")
        .and_then(|id| by_id.get(id))
        .map(|(path, _)| path.clone())
        .or_else(|| by_id.values().find(|(_, media)| *media == "application/x-dtbncx+xml").map(|(path, _)| path.clone()));
    for item in spine.children().filter(|n| n.tag_name().name() == "itemref") {
        let Some((path, media)) = item.attribute("idref").and_then(|id| by_id.get(id)) else { continue };
        if !matches!(*media, "application/xhtml+xml" | "text/html" | "image/svg+xml") {
            continue;
        }
        let props = item.attribute("properties").unwrap_or_default();
        let fixed = if props.split_whitespace().any(|x| x == "rendition:layout-pre-paginated") {
            true
        } else if props.split_whitespace().any(|x| x == "rendition:layout-reflowable") {
            false
        } else {
            book_fixed
        };
        p.spine.push(SpineItem { path: path.clone(), fixed });
    }
    if p.spine.is_empty() {
        return Err(EpubError::Damaged("the book's spine lists no content".into()));
    }
    let _ = OPF;
    p.toc = match nav_path {
        Some(path) => nav_toc(zip, &path)?,
        None => Vec::new(),
    };
    if p.toc.is_empty()
        && let Some(path) = ncx_path
    {
        p.toc = ncx_toc(zip, &path)?;
    }
    Ok(p)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::epub::zip::tests::zip;

    #[test]
    fn hrefs_resolve_inside_the_book() {
        assert_eq!(resolve("OEBPS/content.opf", "text/ch%201.xhtml").as_deref(), Some("OEBPS/text/ch 1.xhtml"));
        assert_eq!(resolve("OEBPS/text/a.xhtml", "../images/c.png").as_deref(), Some("OEBPS/images/c.png"));
        assert_eq!(resolve("OEBPS/text/a.xhtml", "b.xhtml#note").as_deref(), Some("OEBPS/text/b.xhtml#note"));
        assert_eq!(resolve("OEBPS/text/a.xhtml", "#top").as_deref(), Some("OEBPS/text/a.xhtml#top"));
        assert_eq!(resolve("a.xhtml", "../../etc/passwd"), None);
        assert_eq!(resolve("OEBPS/a.xhtml", "https://example.com/x"), None);
        assert_eq!(resolve("OEBPS/a.xhtml", "/abs.xhtml"), None);
    }

    #[test]
    fn html_entities_become_character_references() {
        assert_eq!(xml_text(b"a&nbsp;b &mdash; &amp; &#169; &bogus; & x"), "a&#160;b &#8212; &amp; &#169; &amp;bogus; &amp; x");
    }

    #[test]
    fn idpf_obfuscation_is_undone() {
        let font: Vec<u8> = (0..2000u32).map(|i| (i % 251) as u8).collect();
        let mut x = font.clone();
        deobfuscate(&mut x, Obfuscation::Idpf, "urn:uuid:1234");
        assert_ne!(x, font);
        assert_eq!(x[1040..], font[1040..], "only the first 1040 bytes");
        deobfuscate(&mut x, Obfuscation::Idpf, " urn:uuid:12\n34 ");
        assert_eq!(x, font, "whitespace in the identifier is ignored; XOR twice restores");
    }

    #[test]
    fn protected_books_are_refused_and_obfuscated_fonts_are_not() {
        let container = br#"<?xml version="1.0"?><container xmlns="urn:oasis:names:tc:opendocument:xmlns:container" version="1.0"><rootfiles><rootfile full-path="c.opf" media-type="application/oebps-package+xml"/></rootfiles></container>"#;
        let enc = |alg: &str| {
            format!(
                r#"<encryption xmlns="urn:oasis:names:tc:opendocument:xmlns:container" xmlns:enc="http://www.w3.org/2001/04/xmlenc#"><enc:EncryptedData><enc:EncryptionMethod Algorithm="{alg}"/><enc:CipherData><enc:CipherReference URI="f.otf"/></enc:CipherData></enc:EncryptedData></encryption>"#
            )
        };
        let drm = enc("http://www.w3.org/2001/04/xmlenc#aes128-cbc");
        let z = zip(&[("META-INF/container.xml", container, false), ("META-INF/encryption.xml", drm.as_bytes(), false)]);
        assert!(matches!(read(&Zip::open(&z).unwrap()), Err(EpubError::Protected)));
        let z = zip(&[("META-INF/container.xml", container, false), ("META-INF/rights.xml", b"<rights/>", false)]);
        assert!(matches!(read(&Zip::open(&z).unwrap()), Err(EpubError::Protected)));
        let fonts = enc(IDPF_OBFUSCATION);
        let z = zip(&[("META-INF/encryption.xml", fonts.as_bytes(), false)]);
        assert_eq!(encryption(&Zip::open(&z).unwrap()).unwrap().get("f.otf"), Some(&Obfuscation::Idpf));
    }
}
