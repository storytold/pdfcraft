//! The XDP packets inside a PDF (`/AcroForm /XFA`, ISO 32000-1 §12.7.8): either one stream
//! holding the whole XDP, or an array of (name, stream) pairs.

use std::sync::Arc;

use pdfcraft_cos::{Document, Object};

use crate::XfaError;

/// Most decoded packet bytes read in total.
const MAX_XDP: usize = 64 << 20;

fn packet_markers(doc: &Document) -> Option<(Arc<Object>, bool, bool)> {
    let root = doc.root()?;
    let catalog = doc.get(root);
    let acro = catalog.as_dict()?.get(b"AcroForm").map(|a| doc.resolve(a))?;
    let acro = acro.as_dict()?;
    let xfa = acro.get(b"XFA").map(|x| doc.resolve(x))?;
    // `/NeedsRendering` is a catalog entry (ISO 32000-1 Table 28); some producers put it on
    // the form dictionary, so look there too.
    let bool_of = |v: Option<&Object>| match v.map(|v| doc.resolve(v)).as_deref() {
        Some(Object::Bool(b)) => Some(*b),
        _ => None,
    };
    let needs_rendering = bool_of(catalog.as_dict().and_then(|c| c.get(b"NeedsRendering"))).or_else(|| bool_of(acro.get(b"NeedsRendering")));
    let has_fields = acro.get(b"Fields").map(|f| doc.resolve(f)).and_then(|f| f.as_array().map(|a| !a.is_empty())).unwrap_or(false);
    Some((xfa, needs_rendering.unwrap_or(false), has_fields))
}

/// Whether the PDF marks this XFA entry for layout, without decoding its XML packets.
pub(crate) fn marked_for_layout(doc: &Document) -> Option<bool> {
    let (_, needs_rendering, has_fields) = packet_markers(doc)?;
    Some(needs_rendering || !has_fields)
}

/// What was found in the XFA entry.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Packets {
    /// The XDP as one XML document (packets concatenated in order), or the template packet
    /// alone when there is no XDP envelope. Empty when the entry holds no template.
    pub xdp: String,
    /// `/NeedsRendering true`: the form expects to be laid out from the template.
    pub needs_rendering: bool,
    /// Whether the AcroForm has any fields of its own.
    pub has_fields: bool,
}

/// How a packet's bytes encode its text.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Encoding {
    /// UTF-8, with or without a byte order mark.
    Utf8 { bom: bool },
    /// UTF-16, little-endian, with or without a byte order mark.
    Utf16Le { bom: bool },
    /// UTF-16, big-endian, with or without a byte order mark.
    Utf16Be { bom: bool },
}

fn utf16(bytes: &[u8], from: fn([u8; 2]) -> u16) -> Option<String> {
    let (chunks, rest) = bytes.as_chunks::<2>();
    if !rest.is_empty() {
        return None;
    }
    char::decode_utf16(chunks.iter().map(|c| from(*c))).collect::<Result<String, _>>().ok()
}

/// A packet's text and how it was encoded, when it decodes without loss: UTF-8 or UTF-16,
/// told apart by a byte order mark or (without one) by how `<` is written. `None` for
/// anything else (invalid UTF-8, a legacy encoding, a broken surrogate), which can be read
/// only approximately and so must not be written back.
pub fn decode(bytes: &[u8]) -> Option<(String, Encoding)> {
    match bytes {
        [0xFF, 0xFE, rest @ ..] => utf16(rest, u16::from_le_bytes).map(|t| (t, Encoding::Utf16Le { bom: true })),
        [0xFE, 0xFF, rest @ ..] => utf16(rest, u16::from_be_bytes).map(|t| (t, Encoding::Utf16Be { bom: true })),
        [0xEF, 0xBB, 0xBF, rest @ ..] => String::from_utf8(rest.to_vec()).ok().map(|t| (t, Encoding::Utf8 { bom: true })),
        [b'<', 0, ..] => utf16(bytes, u16::from_le_bytes).map(|t| (t, Encoding::Utf16Le { bom: false })),
        [0, b'<', ..] => utf16(bytes, u16::from_be_bytes).map(|t| (t, Encoding::Utf16Be { bom: false })),
        _ => String::from_utf8(bytes.to_vec()).ok().map(|t| (t, Encoding::Utf8 { bom: false })),
    }
}

/// `text` in `encoding` (the inverse of [`decode`]).
pub fn encode(text: &str, encoding: Encoding) -> Vec<u8> {
    let units = |bom: bool, to: fn(u16) -> [u8; 2]| {
        let mut out = Vec::with_capacity(text.len().saturating_mul(2).saturating_add(2));
        if bom {
            out.extend_from_slice(&to(0xFEFF));
        }
        for u in text.encode_utf16() {
            out.extend_from_slice(&to(u));
        }
        out
    };
    match encoding {
        Encoding::Utf8 { bom } => {
            let mut out = Vec::with_capacity(text.len() + 3);
            if bom {
                out.extend_from_slice(&[0xEF, 0xBB, 0xBF]);
            }
            out.extend_from_slice(text.as_bytes());
            out
        }
        Encoding::Utf16Le { bom } => units(bom, u16::to_le_bytes),
        Encoding::Utf16Be { bom } => units(bom, u16::to_be_bytes),
    }
}

/// A packet's text for reading: exact when it decodes, else UTF-8 with replacement characters.
fn to_text(bytes: &[u8]) -> String {
    if let Some((text, _)) = decode(bytes) {
        return text;
    }
    let lossy16 = |rest: &[u8], from: fn([u8; 2]) -> u16| -> String {
        char::decode_utf16(rest.as_chunks::<2>().0.iter().map(|c| from(*c))).map(|c| c.unwrap_or('\u{FFFD}')).collect()
    };
    match bytes {
        [0xFF, 0xFE, rest @ ..] => lossy16(rest, u16::from_le_bytes),
        [0xFE, 0xFF, rest @ ..] => lossy16(rest, u16::from_be_bytes),
        [b'<', 0, ..] => lossy16(bytes, u16::from_le_bytes),
        [0, b'<', ..] => lossy16(bytes, u16::from_be_bytes),
        _ => String::from_utf8_lossy(bytes.strip_prefix(&[0xEF, 0xBB, 0xBF]).unwrap_or(bytes)).into_owned(),
    }
}

/// Validate an exact packet-leading XML declaration, returning the text after it.
/// XML 1.0 §2.8/§4.3.3: ordered pseudo-attributes, XML whitespace and literal values.
/// Unlike ordinary attributes, declaration values cannot contain entity references.
/// This borrows the packet and does no allocation or recursive XML parsing.
fn after_declaration(text: &str) -> Result<&str, XfaError> {
    const SPACE: [char; 4] = [' ', '\t', '\r', '\n'];
    let Some(mut rest) = text.strip_prefix("<?xml") else { return Ok(text) };
    if !rest.is_empty() && !rest.starts_with(SPACE) && !rest.starts_with("?>") {
        return Ok(text); // A different PI target, such as xml-stylesheet.
    }
    let malformed = || XfaError::Malformed("invalid packet-leading XML declaration".into());
    for name in ["version", "encoding", "standalone"] {
        let spaced = rest.trim_start_matches(SPACE);
        let Some(value) = spaced.strip_prefix(name) else {
            if name == "version" {
                return Err(malformed());
            }
            continue;
        };
        if spaced.len() == rest.len() {
            return Err(malformed());
        }
        let value = value.trim_start_matches(SPACE).strip_prefix('=').ok_or_else(malformed)?.trim_start_matches(SPACE);
        let quote = value.chars().next().filter(|c| matches!(c, '\'' | '"')).ok_or_else(malformed)?;
        // The quote is one ASCII byte, so this is always a character boundary.
        let (value, tail) = value.get(1..).and_then(|v| v.split_once(quote)).ok_or_else(malformed)?;
        let valid = match name {
            "version" => value.strip_prefix("1.").is_some_and(|v| !v.is_empty() && v.bytes().all(|b| b.is_ascii_digit())),
            "encoding" => {
                value.as_bytes().first().is_some_and(u8::is_ascii_alphabetic)
                    && value.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'))
            }
            _ => matches!(value, "yes" | "no"),
        };
        if !valid {
            return Err(malformed());
        }
        rest = tail;
    }
    rest.trim_start_matches(SPACE).strip_prefix("?>").ok_or_else(malformed)
}

/// Lexical context carried across packet boundaries. In particular, a declaration-shaped
/// prefix in a continued comment, CDATA section, PI or attribute must remain literal text.
#[derive(Default)]
enum PacketContext {
    #[default]
    Content,
    Tag,
    Quote(char),
    Section(&'static str),
}

impl PacketContext {
    /// Advance linearly without allocations. Malformed XML is still checked by the parser;
    /// DTDs are forbidden there, so no DTD grammar is needed to preserve accepted input.
    fn advance(&mut self, mut text: &str) {
        loop {
            let next = match self {
                Self::Content => {
                    let Some((_, tail)) = text.split_once('<') else { return };
                    let section = [("!--", "-->"), ("![CDATA[", "]]>"), ("?", "?>")]
                        .into_iter()
                        .find_map(|(start, end)| tail.strip_prefix(start).map(|rest| (rest, end)));
                    if let Some((rest, end)) = section {
                        *self = Self::Section(end);
                        rest
                    } else {
                        *self = Self::Tag;
                        tail
                    }
                }
                Self::Tag => {
                    let Some(pos) = text.find(['\'', '"', '>']) else { return };
                    let Some(tail) = text.get(pos..) else { return };
                    let mut chars = tail.chars();
                    *self = match chars.next() {
                        Some('>') => Self::Content,
                        Some(quote) => Self::Quote(quote),
                        None => return,
                    };
                    chars.as_str()
                }
                Self::Quote(quote) => {
                    let Some((_, tail)) = text.split_once(*quote) else { return };
                    *self = Self::Tag;
                    tail
                }
                Self::Section(end) => {
                    let Some((_, tail)) = text.split_once(*end) else { return };
                    *self = Self::Content;
                    tail
                }
            };
            text = next;
        }
    }
}

/// Read the XFA packets, if the document has any. Array assembly omits legal declarations
/// at later packet boundaries, retaining the initial declaration and all stored stream bytes.
pub fn read_packets(doc: &Document) -> Result<Option<Packets>, XfaError> {
    let Some((xfa, needs_rendering, has_fields)) = packet_markers(doc) else { return Ok(None) };
    let mut total = 0usize;
    // A stream listed again adds nothing but another decode, which may cost far more than the
    // bytes it yields (a Flate stream of hex whitespace).
    let mut seen = std::collections::HashSet::new();
    let mut read = |o: &Object| -> Result<Option<String>, XfaError> {
        if o.as_ref().is_some_and(|r| !seen.insert(r)) {
            return Ok(None);
        }
        let s = doc.resolve(o);
        let Object::Stream(s) = &*s else { return Ok(None) };
        let bytes = s.decoded_within(MAX_XDP)?;
        total = total.saturating_add(bytes.len());
        if total > MAX_XDP {
            return Err(XfaError::TooLarge("the XFA packets are larger than 64 MB".into()));
        }
        Ok(Some(to_text(&bytes)))
    };
    let mut xdp = String::new();
    match &*xfa {
        Object::Array(items) => {
            let mut template = None;
            let mut context = PacketContext::default();
            let mut i = 0;
            while i < items.len() {
                let name = items.get(i).and_then(|n| n.as_string()).map(|s| s.to_text());
                let stream = if name.is_some() { items.get(i + 1) } else { items.get(i) };
                if let Some(o) = stream
                    && let Some(text) = read(o)?
                {
                    if name.as_deref() == Some("template") {
                        template = Some(text.clone());
                    }
                    let content = if matches!(context, PacketContext::Content) {
                        let body = after_declaration(&text)?;
                        if xdp.is_empty() { &text } else { body }
                    } else {
                        &text
                    };
                    context.advance(content);
                    xdp.push_str(content);
                    xdp.push('\n');
                }
                i += if name.is_some() { 2 } else { 1 };
            }
            if !xdp.contains("<template") {
                xdp = template.unwrap_or_default();
            }
        }
        Object::Stream(_) => {
            xdp = read(&xfa)?.unwrap_or_default();
        }
        // Not a stream or an array: an XFA entry all the same, with nothing to lay out.
        _ => {}
    }
    if !xdp.contains("<template") {
        xdp.clear();
    }
    Ok(Some(Packets { xdp, needs_rendering, has_fields }))
}

#[cfg(test)]
mod tests;
