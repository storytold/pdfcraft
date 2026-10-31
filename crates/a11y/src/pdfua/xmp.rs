//! The text formats inside a PDF that PDF/UA looks into: the XMP metadata packet and the XFA form.

use pdfcraft_cos::{Dict, Document, Object};

use super::checks::dict_at;

/// XMP packets and XFA streams larger than this are not read.
const MAX_PACKET: usize = 32 << 20;

/// The catalog's XMP metadata packet as text, when the catalog has a metadata stream.
pub(super) fn metadata(doc: &Document, cat: &Dict) -> Option<String> {
    let m = doc.resolve(cat.get(b"Metadata")?);
    let Object::Stream(s) = &*m else { return None };
    s.decoded_within(MAX_PACKET).ok().map(|b| String::from_utf8_lossy(&b).into_owned())
}

/// The value of the simple XMP property `key` (`prefix:name`), written either as an attribute or as
/// an element; for an `rdf:Alt`, `rdf:Seq` or `rdf:Bag`, its first item. Entities are unescaped.
pub(super) fn xmp_value(xmp: &str, key: &str) -> Option<String> {
    for quote in ['"', '\''] {
        let attr = format!("{key}={quote}");
        let mut from = 0;
        while let Some(at) = xmp.get(from..).and_then(|rest| rest.find(&attr)).map(|i| i + from) {
            // An attribute stands after white space; anything else is a longer name ending in `key`.
            let spaced = xmp.get(..at).and_then(|before| before.chars().next_back()).is_some_and(char::is_whitespace);
            let value_at = at + attr.len();
            if spaced {
                let rest = xmp.get(value_at..)?;
                return rest.find(quote).and_then(|end| rest.get(..end)).map(unescape);
            }
            from = value_at;
        }
    }
    let open = format!("<{key}");
    let close = format!("</{key}>");
    let mut from = 0;
    while let Some(at) = xmp.get(from..).and_then(|rest| rest.find(&open)).map(|i| i + from) {
        let after = xmp.get(at + open.len()..)?;
        // `<dc:title>` or `<dc:title xml:lang=…>`, not `<dc:titles>`.
        if after.starts_with('>') || after.starts_with(char::is_whitespace) {
            let body = after.get(after.find('>')? + 1..)?;
            let inner = body.get(..body.find(&close)?)?;
            if let Some(li) = inner.find("<rdf:li") {
                let item = inner.get(li..)?;
                let start = item.find('>')? + 1;
                let text = item.get(start..)?;
                return Some(unescape(text.get(..text.find("</rdf:li>")?)?.trim()));
            }
            return Some(unescape(inner.trim()));
        }
        from = at + open.len();
    }
    None
}

fn unescape(s: &str) -> String {
    s.replace("&lt;", "<").replace("&gt;", ">").replace("&quot;", "\"").replace("&apos;", "'").replace("&amp;", "&")
}

/// Whether the packet names a language anywhere (`xml:lang` other than `x-default`).
pub(super) fn xmp_has_language(xmp: &str) -> bool {
    let mut rest = xmp;
    while let Some(at) = rest.find("xml:lang=") {
        let Some(after) = rest.get(at + "xml:lang=".len()..) else { return false };
        let mut chars = after.chars();
        let Some(quote) = chars.next().filter(|q| matches!(q, '"' | '\'')) else {
            rest = after;
            continue;
        };
        let value = chars.as_str();
        let lang = value.get(..value.find(quote).unwrap_or(0)).unwrap_or("").trim();
        if !lang.is_empty() && !lang.eq_ignore_ascii_case("x-default") {
            return true;
        }
        rest = value;
    }
    false
}

/// Whether the form's XFA asks for dynamic rendering (`<dynamicRender>required</dynamicRender>`).
pub(super) fn xfa_requires_dynamic_render(doc: &Document, cat: &Dict) -> bool {
    let Some(xfa) = dict_at(doc, cat, b"AcroForm").and_then(|f| f.get(b"XFA").map(|x| doc.resolve(x))) else { return false };
    let parts: Vec<std::sync::Arc<Object>> = match &*xfa {
        Object::Array(items) => items.iter().map(|x| doc.resolve(x)).collect(),
        _ => vec![xfa.clone()],
    };
    parts.iter().any(|part| match &**part {
        Object::Stream(s) => s.decoded_within(MAX_PACKET).is_ok_and(|b| dynamic_render_required(&String::from_utf8_lossy(&b))),
        _ => false,
    })
}

pub(super) fn dynamic_render_required(xml: &str) -> bool {
    let tag = "<dynamicRender";
    let mut rest = xml;
    while let Some(at) = rest.find(tag) {
        let Some(after) = rest.get(at + tag.len()..) else { return false };
        if after.starts_with('>') || after.starts_with(char::is_whitespace) {
            let Some(body) = after.find('>').and_then(|close| after.get(close + 1..)) else { return false };
            if body.split('<').next().is_some_and(|value| value.trim() == "required") {
                return true;
            }
        }
        rest = after;
    }
    false
}
