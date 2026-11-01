//! The XDP packets inside a PDF (`/AcroForm /XFA`, ISO 32000-1 §12.7.8): either one stream
//! holding the whole XDP, or an array of (name, stream) pairs.

use std::sync::Arc;

use pdfcraft_cos::{Document, Object};

use crate::XfaError;

/// Most decoded packet bytes read in total.
pub(crate) const MAX_XDP: usize = 64 << 20;

// Real-limit regression fixtures share one allocation-heavy slot even with parallel test runners.
#[cfg(test)]
pub(crate) static LARGE_PACKET_TEST: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// Resolve the required document references and read the PDF layout markers without decoding
/// packet streams. Returns `None` when the root or required catalog/AcroForm dictionary path is
/// missing or has the wrong shape, or when `/XFA` is absent. The resolved XFA object is returned
/// without checking whether it is a stream or array; `read_packets` handles that distinction.
/// A missing or non-array `/Fields` entry counts as having no fields.
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

/// Apply only the PDF marker rule for layout, without decoding XML packets.
/// `None` means the marker objects could not be resolved; this is not a replacement for the
/// broader successful-packet classification in [`crate::is_dynamic`].
pub(crate) fn marked_for_layout(doc: &Document) -> Option<bool> {
    let (_, needs_rendering, has_fields) = packet_markers(doc)?;
    Some(wants_layout(needs_rendering, has_fields))
}

/// Return whether the PDF markers alone request layout.
/// `/NeedsRendering true` is sufficient; otherwise an empty or absent AcroForm field list is the
/// existing fallback. This marker-only predicate does not decide configuration-based eligibility
/// or signature safeguards; those remain separate classification policy.
pub(crate) fn wants_layout(needs_rendering: bool, has_fields: bool) -> bool {
    needs_rendering || !has_fields
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

/// Decode complete UTF-16 code units without replacing broken surrogate pairs.
fn utf16(bytes: &[u8], from: fn([u8; 2]) -> u16) -> Option<String> {
    let (chunks, rest) = bytes.as_chunks::<2>();
    if !rest.is_empty() {
        return None;
    }
    char::decode_utf16(chunks.iter().map(|c| from(*c))).collect::<Result<String, _>>().ok()
}

/// Detect the existing BOM/leading-code-unit convention and borrow the body without its BOM,
/// without interpreting legacy labels or changing exact/lossy decoding policy.
fn byte_encoding(bytes: &[u8]) -> (Encoding, &[u8]) {
    match bytes {
        [0xFF, 0xFE, rest @ ..] => (Encoding::Utf16Le { bom: true }, rest),
        [0xFE, 0xFF, rest @ ..] => (Encoding::Utf16Be { bom: true }, rest),
        [0xEF, 0xBB, 0xBF, rest @ ..] => (Encoding::Utf8 { bom: true }, rest),
        [b'<', 0, ..] => (Encoding::Utf16Le { bom: false }, bytes),
        [0, b'<', ..] => (Encoding::Utf16Be { bom: false }, bytes),
        _ => (Encoding::Utf8 { bom: false }, bytes),
    }
}

/// Exact encoded size, including a BOM, without allocating the encoded candidate.
/// None means its byte count cannot be represented.
pub(crate) fn encoded_len(text: &str, encoding: Encoding) -> Option<usize> {
    match encoding {
        Encoding::Utf8 { bom } => text.len().checked_add(if bom { 3 } else { 0 }),
        Encoding::Utf16Le { bom } | Encoding::Utf16Be { bom } => text.encode_utf16().try_fold(if bom { 2usize } else { 0 }, |n, _| n.checked_add(2)),
    }
}

/// A packet's text and how it was encoded, when it decodes without loss: UTF-8 or UTF-16,
/// told apart by a byte order mark or (without one) by how `<` is written. `None` for
/// anything else (invalid UTF-8, a legacy encoding, a broken surrogate), which can be read
/// only approximately and so must not be written back.
pub fn decode(bytes: &[u8]) -> Option<(String, Encoding)> {
    let (encoding, body) = byte_encoding(bytes);
    let text = match encoding {
        Encoding::Utf8 { .. } => String::from_utf8(body.to_vec()).ok(),
        Encoding::Utf16Le { .. } => utf16(body, u16::from_le_bytes),
        Encoding::Utf16Be { .. } => utf16(body, u16::from_be_bytes),
    }?;
    Some((text, encoding))
}

/// `text` in `encoding` (the inverse of [`decode`]).
pub fn encode(text: &str, encoding: Encoding) -> Vec<u8> {
    let units = |bom: bool, to: fn(u16) -> [u8; 2]| {
        let mut out = Vec::with_capacity(encoded_len(text, encoding).unwrap_or_default());
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
            let mut out = Vec::with_capacity(encoded_len(text, encoding).unwrap_or_default());
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
    let (encoding, body) = byte_encoding(bytes);
    match encoding {
        Encoding::Utf16Le { .. } => lossy16(body, u16::from_le_bytes),
        Encoding::Utf16Be { .. } => lossy16(body, u16::from_be_bytes),
        Encoding::Utf8 { .. } => String::from_utf8_lossy(body).into_owned(),
    }
}

/// Validate an exact packet-leading XML declaration and return its body and borrowed encoding label.
/// A non-declaration processing instruction is returned unchanged; an incomplete or malformed
/// declaration returns `XfaError::Malformed`. The grammar follows XML 1.0 §2.8/§4.3.3: ordered
/// pseudo-attributes, XML whitespace, and literal values without entity references. This borrows
/// the input, allocates no declaration buffer, and performs no recursive XML parsing.
fn declaration(text: &str) -> Result<(&str, Option<&str>), XfaError> {
    const SPACE: [char; 4] = [' ', '\t', '\r', '\n'];
    let Some(mut rest) = text.strip_prefix("<?xml") else { return Ok((text, None)) };
    if !rest.is_empty() && !rest.starts_with(SPACE) && !rest.starts_with("?>") {
        return Ok((text, None)); // A different PI target, such as xml-stylesheet.
    }
    let malformed = || XfaError::Malformed("invalid packet-leading XML declaration".into());
    let mut encoding = None;
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
        if name == "encoding" {
            encoding = Some(value);
        }
        rest = tail;
    }
    rest.trim_start_matches(SPACE).strip_prefix("?>").map(|body| (body, encoding)).ok_or_else(malformed)
}

/// Validate declaration grammar with the shared parser, without making any encoding decision.
pub(crate) fn after_declaration(text: &str) -> Result<&str, XfaError> {
    declaration(text).map(|(body, _)| body)
}

/// Reject recognized UTF declarations that contradict byte order/BOM evidence. Unknown legacy
/// labels retain the reader's existing tolerance. Call only at a genuine declaration boundary.
/// BOM checks recognize an opposing XML opening code unit without guessing arbitrary legacy data.
pub(crate) fn validate_encoding(bytes: &[u8], text: &str) -> Result<(), XfaError> {
    validate_packet_encoding(bytes, text, true)
}

/// A writer must know the declared character set: legacy labels are readable by policy, but
/// cannot be re-encoded safely. Reuse the reader's declaration grammar and BOM/endian checks.
pub(crate) fn validate_write_encoding(bytes: &[u8], text: &str) -> Result<(), XfaError> {
    validate_packet_encoding(bytes, text, false)
}

/// Apply the shared byte/declaration checks, allowing unsupported labels only for reading.
fn validate_packet_encoding(bytes: &[u8], text: &str, allow_legacy: bool) -> Result<(), XfaError> {
    let (encoding, _) = byte_encoding(bytes);
    let contradictory_bom = matches!(
        bytes,
        [0xFF, 0xFE, 0, b'<', ..]
            | [0xFE, 0xFF, b'<', 0, ..]
            | [0xFF, 0xFE, b'<', b'?', ..]
            | [0xFE, 0xFF, b'<', b'?', ..]
            | [0xEF, 0xBB, 0xBF, b'<', 0, ..]
            | [0xEF, 0xBB, 0xBF, 0, b'<', ..]
    );
    if contradictory_bom {
        return Err(XfaError::Malformed("XFA packet byte order mark contradicts its XML byte order; the original stream was kept".into()));
    }
    let (_, label) = declaration(text)?;
    let Some(label) = label else { return Ok(()) };
    let consistent = if label.eq_ignore_ascii_case("UTF-8") {
        matches!(encoding, Encoding::Utf8 { .. })
    } else if label.eq_ignore_ascii_case("UTF-16") {
        matches!(encoding, Encoding::Utf16Le { .. } | Encoding::Utf16Be { .. })
    } else if label.eq_ignore_ascii_case("UTF-16LE") {
        matches!(encoding, Encoding::Utf16Le { .. })
    } else if label.eq_ignore_ascii_case("UTF-16BE") {
        matches!(encoding, Encoding::Utf16Be { .. })
    } else if allow_legacy {
        true
    } else {
        return Err(XfaError::Malformed(format!(
            "XFA declared encoding {label:.64} is unsupported for writing; only UTF-8 and UTF-16 are supported, so the original stream was kept"
        )));
    };
    if !consistent {
        return Err(XfaError::Malformed(format!("XFA packet declares {label} but its bytes indicate {encoding:?}; the original stream was kept")));
    }
    Ok(())
}

/// Visit exactly the packet slots the reader consumes: a raw string names the next object;
/// other objects consume one slot. Missing trailing packet values are skipped.
/// The index identifies the stream slot, allowing the writer to validate a proposed replacement.
pub(crate) fn packet_entries(items: &[Object]) -> impl Iterator<Item = (usize, Option<String>, &Object)> {
    let mut i = 0usize;
    std::iter::from_fn(move || {
        let item = items.get(i)?;
        let name = item.as_string().map(|s| s.to_text());
        let slot = if name.is_some() { i.saturating_add(1) } else { i };
        i = slot.saturating_add(1);
        items.get(slot).map(|object| (slot, name, object))
    })
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

/// Read the XFA packets, returning `None` if the root, catalog, or AcroForm dictionary is absent
/// or malformed, or if `/XFA` is absent. A present XFA entry with an unsupported top-level type
/// returns `Some(Packets)` with empty `xdp` text; array elements that do not resolve to packet
/// streams are skipped.
/// Array packets are decoded in order, capped at [`MAX_XDP`] total bytes, and assembled while
/// omitting legal declarations at later packet boundaries; the initial declaration and stored
/// PDF stream bytes are preserved. Malformed packet-leading declarations return `Malformed`,
/// while oversized decoded data returns `TooLarge`.
pub fn read_packets(doc: &Document) -> Result<Option<Packets>, XfaError> {
    let Some((xfa, needs_rendering, has_fields)) = packet_markers(doc) else { return Ok(None) };
    let mut total = 0usize;
    // A stream listed again adds nothing but another decode, which may cost far more than the
    // bytes it yields (a Flate stream of hex whitespace).
    let mut seen = std::collections::HashSet::new();
    let mut read = |o: &Object, declaration_boundary: bool| -> Result<Option<String>, XfaError> {
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
        let text = to_text(&bytes);
        if declaration_boundary {
            validate_encoding(&bytes, &text)?;
        }
        Ok(Some(text))
    };
    let mut xdp = String::new();
    match &*xfa {
        Object::Array(items) => {
            let mut template = None;
            let mut context = PacketContext::default();
            for (_, name, o) in packet_entries(items) {
                if let Some(text) = read(o, matches!(context, PacketContext::Content))? {
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
            }
            if !xdp.contains("<template") {
                xdp = template.unwrap_or_default();
            }
        }
        Object::Stream(_) => {
            xdp = read(&xfa, true)?.unwrap_or_default();
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
