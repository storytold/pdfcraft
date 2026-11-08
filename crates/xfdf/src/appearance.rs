//! A comment's appearance dictionary (`/AP`) in XFDF.
//!
//! ISO 19444-1 (XFDF 3.0, "appearance") carries a stamp's `/AP` as a base 64 encoded string. What
//! the string holds is the dictionary written as XML, one element per PDF object: `DICT`, `STREAM`,
//! `ARRAY`, `NAME`, `INT`, `FIXED` (a real number), `BOOL`, `STRING` and `NULL`, each with its
//! dictionary key in `KEY` and a simple value in `VAL`; a stream's data is a `DATA` element whose
//! `MODE` and `ENCODING` follow XFDF 3.0 "Stream encoding" (`filtered`/`ascii` for short printable
//! data, `raw`/`hex` otherwise). That is what the XFDF files in the wild hold (for example the
//! stamps attached to PDFBOX-4437 and PDFBOX-4478). Indirect objects are written in place, so the
//! XML is self-contained.
//!
//! Both directions are bounded: the decoded XML, the stream data, the nesting and the number of
//! values. Anything unreadable makes the whole appearance unusable; the caller then records it and
//! draws the comment from its other entries.

use base64::Engine as _;

use super::common::{check_form_xml_depth, esc, parse_form_xml};
use super::{Dict, Document, ObjRef, Object, PdfString};

/// At most this much decoded XML in one appearance (and at most this much XML is written for one).
pub(crate) const MAX_APPEARANCE_BYTES: usize = 16 << 20;
/// At most this much appearance data (decoded XML) in one import, over all of its comments.
pub(crate) const MAX_IMPORT_APPEARANCE_BYTES: usize = 64 << 20;
/// How deeply objects may nest inside one appearance.
const MAX_NESTING: usize = 64;
/// How many PDF values one appearance may hold.
const MAX_VALUES: usize = 100_000;
/// XFDF 3.0 "Stream encoding": data shorter than this that is all printable ASCII is written as it
/// is (`filtered`/`ascii`); everything else as hexadecimal (`raw`/`hex`).
const ASCII_DATA_LIMIT: usize = 4096;

/// The base 64 `<appearance>` text for annotation `d`'s appearance dictionary, or `None` when it
/// has no normal appearance stream or is past the limits (the comment is then exported without
/// one).
pub(crate) fn encode(doc: &Document, d: &Dict) -> Option<String> {
    if !has_appearance(doc, d) {
        return None;
    }
    let ap = d.get(b"AP").map(|a| doc.resolve(a))?;
    let mut writer = Writer { doc, out: String::new(), values: MAX_VALUES, path: Vec::new() };
    writer.dict(Some(b"AP"), ap.as_dict()?, 0)?;
    let b64 = base64::engine::general_purpose::STANDARD.encode(writer.out.as_bytes());
    // Line breaks every 76 characters, as base 64 text usually has them; the reader skips them.
    let mut wrapped = String::with_capacity(b64.len() + b64.len() / 76 + 1);
    for (i, c) in b64.chars().enumerate() {
        if i > 0 && i % 76 == 0 {
            wrapped.push('\n');
        }
        wrapped.push(c);
    }
    Some(wrapped)
}

struct Writer<'a> {
    doc: &'a Document,
    out: String,
    values: usize,
    /// The references being written, so a reference back to one of them ends the cycle.
    path: Vec<ObjRef>,
}

impl Writer<'_> {
    fn key_attr(key: Option<&[u8]>) -> String {
        key.map(|k| format!(" KEY=\"{}\"", esc(&pdfcraft_forms::name_text(k)))).unwrap_or_default()
    }

    fn spend(&mut self, depth: usize) -> Option<()> {
        if depth > MAX_NESTING || self.out.len() > MAX_APPEARANCE_BYTES {
            return None;
        }
        self.values = self.values.checked_sub(1)?;
        Some(())
    }

    fn dict(&mut self, key: Option<&[u8]>, d: &Dict, depth: usize) -> Option<()> {
        self.spend(depth)?;
        self.out.push_str(&format!("<DICT{}>\n", Self::key_attr(key)));
        for (k, v) in d.iter() {
            self.value(Some(k), v, depth + 1)?;
        }
        self.out.push_str("</DICT>\n");
        Some(())
    }

    fn value(&mut self, key: Option<&[u8]>, o: &Object, depth: usize) -> Option<()> {
        self.spend(depth)?;
        let k = Self::key_attr(key);
        match o {
            Object::Ref(r) => {
                if self.path.contains(r) {
                    // A cycle: the object is already being written further up.
                    self.out.push_str(&format!("<NULL{k}/>\n"));
                    return Some(());
                }
                self.path.push(*r);
                let target = self.doc.get(*r);
                let written = self.value(key, &target, depth + 1);
                self.path.pop();
                written?;
            }
            Object::Null => self.out.push_str(&format!("<NULL{k}/>\n")),
            Object::Bool(b) => self.out.push_str(&format!("<BOOL{k} VAL=\"{b}\"/>\n")),
            Object::Int(i) => self.out.push_str(&format!("<INT{k} VAL=\"{i}\"/>\n")),
            // Rust prints the shortest decimal that reads back as the same number, never with an
            // exponent; a non-finite number (which PDF can't hold) is written as 0.
            Object::Real(r) => self.out.push_str(&format!("<FIXED{k} VAL=\"{}\"/>\n", if r.is_finite() { *r } else { 0.0 })),
            Object::Name(n) => self.out.push_str(&format!("<NAME{k} VAL=\"{}\"/>\n", esc(&pdfcraft_forms::name_text(n)))),
            Object::String(s) => match std::str::from_utf8(&s.bytes) {
                // Text XML can hold as it is (`esc` would drop other control characters).
                Ok(t) if t.chars().all(|c| !(c < ' ' && !matches!(c, '\t' | '\n' | '\r')) && c != '\u{FFFE}' && c != '\u{FFFF}') => {
                    self.out.push_str(&format!("<STRING{k} VAL=\"{}\"/>\n", esc(t)));
                }
                _ => self.out.push_str(&format!("<STRING{k} ENCODING=\"HEX\" VAL=\"{}\"/>\n", hex_of(&s.bytes, false))),
            },
            Object::Array(a) => {
                self.out.push_str(&format!("<ARRAY{k}>\n"));
                for v in a {
                    self.value(None, v, depth + 1)?;
                }
                self.out.push_str("</ARRAY>\n");
            }
            // A page (or the page tree) reached from the appearance is the source document's, not
            // the comment's: written in place it would carry every page along.
            Object::Dict(d) if matches!(d.name(b"Type"), Some(b"Page" | b"Pages")) => self.out.push_str(&format!(
                "<NULL{k}/>
"
            )),
            Object::Dict(d) => self.dict(key, d, depth)?,
            Object::Stream(s) => {
                // The data plus its hexadecimal form must fit what is left of the budget.
                if s.raw.len().saturating_mul(2).saturating_add(self.out.len()) > MAX_APPEARANCE_BYTES {
                    return None;
                }
                self.out.push_str(&format!("<STREAM{k}>\n"));
                // `Length` follows from the data.
                for (dk, dv) in s.dict.iter().filter(|(dk, _)| dk.as_slice() != b"Length") {
                    self.value(Some(dk), dv, depth + 1)?;
                }
                let printable = s.raw.iter().all(|&b| (b > 32 && b < 127) || b == b'\r' || b == b'\n');
                if s.raw.len() < ASCII_DATA_LIMIT && printable && !s.dict.contains(b"Filter") {
                    // Printable ASCII is valid UTF-8.
                    let text = String::from_utf8_lossy(&s.raw);
                    self.out.push_str(&format!("<DATA MODE=\"FILTERED\" ENCODING=\"ASCII\">{}</DATA>\n", esc_data(&text)));
                } else {
                    self.out.push_str(&format!("<DATA MODE=\"RAW\" ENCODING=\"HEX\">{}</DATA>\n", hex_of(&s.raw, true)));
                }
                self.out.push_str("</STREAM>\n");
            }
        }
        Some(())
    }
}

/// XFDF 3.0 "Stream encoding", method 1: the XML control characters as entities; line breaks stay.
fn esc_data(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;")
}

/// Bytes as upper-case hexadecimal, with a line feed after every 80 characters when `wrap`.
fn hex_of(bytes: &[u8], wrap: bool) -> String {
    const DIGITS: &[u8; 16] = b"0123456789ABCDEF";
    let mut out = String::with_capacity(bytes.len().saturating_mul(2).saturating_add(bytes.len() / 40));
    for (i, b) in bytes.iter().enumerate() {
        if wrap && i > 0 && i % 40 == 0 {
            out.push('\n');
        }
        out.push(char::from(DIGITS[usize::from(b >> 4)]));
        out.push(char::from(DIGITS[usize::from(b & 15)]));
    }
    out
}

/// Hexadecimal digits back to bytes, ignoring white space; `None` for any other character or an
/// odd number of digits.
fn bytes_of_hex(text: &str) -> Option<Vec<u8>> {
    let digits: Vec<u8> = text.bytes().filter(|b| !b.is_ascii_whitespace()).collect();
    if !digits.len().is_multiple_of(2) {
        return None;
    }
    let value = |c: u8| char::from(c).to_digit(16).and_then(|d| u8::try_from(d).ok());
    digits.as_chunks::<2>().0.iter().map(|[h, l]| Some(value(*h)? << 4 | value(*l)?)).collect()
}

/// An appearance read from XFDF, before it is put into a document.
#[derive(Debug, PartialEq)]
pub(crate) enum Node {
    Value(Object),
    Array(Vec<Node>),
    Dict(Vec<(Vec<u8>, Node)>),
    Stream(Vec<(Vec<u8>, Node)>, Vec<u8>),
}

/// Read the text of an `<appearance>` element: base 64, then the XML it holds. `budget` is what
/// the import may still read (decoded bytes) and is charged. The result is the `/AP` dictionary,
/// with a normal appearance (`/N`) that is a stream or a dictionary of streams.
pub(crate) fn decode(text: &str, budget: &mut usize) -> Result<Node, String> {
    let limit = MAX_APPEARANCE_BYTES.min(*budget);
    // Base 64 is 4 characters for 3 bytes: refuse a text that decodes past the limit before
    // decoding it.
    let mut compact = String::new();
    for c in text.chars().filter(|c| !c.is_whitespace()) {
        if compact.len() / 4 * 3 > limit {
            return Err(format!("the appearance is larger than {} MiB", limit >> 20));
        }
        compact.push(c);
    }
    let engine = base64::engine::GeneralPurpose::new(
        &base64::alphabet::STANDARD,
        base64::engine::GeneralPurposeConfig::new().with_decode_padding_mode(base64::engine::DecodePaddingMode::Indifferent),
    );
    let bytes = engine.decode(compact.as_bytes()).map_err(|e| format!("the appearance is not base 64: {e}"))?;
    if bytes.len() > limit {
        return Err(format!("the appearance is larger than {} MiB", limit >> 20));
    }
    *budget = budget.saturating_sub(bytes.len());
    let xml = std::str::from_utf8(&bytes).map_err(|_| "the appearance is not UTF-8 text".to_string())?;
    let xml = xml.trim_start_matches('\u{feff}');
    check_form_xml_depth(xml).map_err(|e| e.to_string())?;
    let parsed = parse_form_xml(xml).map_err(|e| format!("the appearance is not XML: {e}"))?;
    let root = parsed.root_element();
    if !root.tag_name().name().eq_ignore_ascii_case("DICT") {
        return Err("the appearance is not a dictionary".into());
    }
    let mut values = MAX_VALUES;
    let node = read(root, 0, &mut values)?;
    let usable = match &node {
        Node::Dict(entries) => entries.iter().any(|(k, v)| {
            k.as_slice() == b"N"
                && match v {
                    Node::Stream(..) => true,
                    Node::Dict(states) => !states.is_empty() && states.iter().all(|(_, s)| matches!(s, Node::Stream(..))),
                    _ => false,
                }
        }),
        _ => false,
    };
    if !usable {
        return Err("the appearance has no normal appearance stream".into());
    }
    Ok(node)
}

fn attr<'a>(node: roxmltree::Node<'a, '_>, name: &str) -> Option<&'a str> {
    node.attributes().find(|a| a.name().eq_ignore_ascii_case(name)).map(|a| a.value())
}

fn read(node: roxmltree::Node, depth: usize, values: &mut usize) -> Result<Node, String> {
    if depth > MAX_NESTING {
        return Err(format!("the appearance nests objects more than {MAX_NESTING} deep"));
    }
    *values = values.checked_sub(1).ok_or_else(|| format!("the appearance holds more than {MAX_VALUES} values"))?;
    let tag = node.tag_name().name().to_ascii_uppercase();
    let val = || attr(node, "VAL").ok_or_else(|| format!("{tag} has no VAL"));
    let entries = |values: &mut usize| -> Result<Vec<(Vec<u8>, Node)>, String> {
        let mut out = Vec::new();
        for child in node.children().filter(|c| c.is_element() && !c.tag_name().name().eq_ignore_ascii_case("DATA")) {
            let key = attr(child, "KEY").ok_or_else(|| format!("an entry of {tag} has no KEY"))?;
            out.push((pdfcraft_forms::name_bytes(key), read(child, depth + 1, values)?));
        }
        Ok(out)
    };
    Ok(match tag.as_str() {
        "DICT" => Node::Dict(entries(values)?),
        "ARRAY" => {
            let mut items = Vec::new();
            for child in node.children().filter(|c| c.is_element()) {
                items.push(read(child, depth + 1, values)?);
            }
            Node::Array(items)
        }
        "STREAM" => {
            let mut dict = entries(values)?;
            let data = node.children().find(|c| c.is_element() && c.tag_name().name().eq_ignore_ascii_case("DATA")).ok_or("a stream has no DATA")?;
            let text: String = data.children().filter(|c| c.is_text()).filter_map(|c| c.text()).collect();
            let bytes = match attr(data, "ENCODING").map(str::to_ascii_lowercase).as_deref() {
                Some("hex") => bytes_of_hex(&text).ok_or("stream data is not hexadecimal")?,
                Some("ascii") | None => text.into_bytes(),
                Some(other) => return Err(format!("unknown stream encoding {other}")),
            };
            // `filtered` data is already decoded: the filters no longer apply to it.
            if attr(data, "MODE").is_some_and(|m| m.eq_ignore_ascii_case("filtered")) {
                dict.retain(|(k, _)| !matches!(k.as_slice(), b"Filter" | b"DecodeParms" | b"FFilter" | b"FDecodeParms"));
            }
            dict.retain(|(k, _)| k.as_slice() != b"Length");
            Node::Stream(dict, bytes)
        }
        "NAME" => Node::Value(Object::Name(pdfcraft_forms::name_bytes(val()?))),
        "INT" => Node::Value(Object::Int(val()?.trim().parse::<i64>().map_err(|_| "INT is not an integer")?)),
        "FIXED" => {
            let v = val()?.trim().parse::<f64>().ok().filter(|v| v.is_finite()).ok_or("FIXED is not a number")?;
            Node::Value(Object::Real(v))
        }
        "BOOL" => Node::Value(Object::Bool(match val()?.trim() {
            "true" => true,
            "false" => false,
            _ => return Err("BOOL is neither true nor false".into()),
        })),
        "STRING" => {
            let v = val()?;
            let bytes = if attr(node, "ENCODING").is_some_and(|e| e.eq_ignore_ascii_case("hex")) {
                bytes_of_hex(v).ok_or("STRING is not hexadecimal")?
            } else {
                v.as_bytes().to_vec()
            };
            Node::Value(Object::String(PdfString::literal(bytes)))
        }
        "NULL" => Node::Value(Object::Null),
        other => return Err(format!("unknown appearance element {other}")),
    })
}

/// Put a read appearance into `doc`: each stream becomes an object of its own. Returns the `/AP`
/// value. Null dictionary entries are left out (ISO 32000-2 §7.3.7: they mean the entry is absent).
pub(crate) fn materialize(doc: &mut Document, node: Node) -> Object {
    let dict = |doc: &mut Document, entries: Vec<(Vec<u8>, Node)>| {
        let mut d = Dict::new();
        for (k, v) in entries {
            let v = materialize(doc, v);
            if !matches!(v, Object::Null) {
                d.set(k, v);
            }
        }
        d
    };
    match node {
        Node::Value(o) => o,
        Node::Array(items) => Object::Array(items.into_iter().map(|n| materialize(doc, n)).collect()),
        Node::Dict(entries) => Object::Dict(dict(doc, entries)),
        Node::Stream(entries, raw) => {
            let d = dict(doc, entries);
            Object::Ref(doc.add(Object::Stream(pdfcraft_cos::Stream { dict: d, raw: raw.into() })))
        }
    }
}

/// Whether an annotation dictionary carries an appearance the document can draw: `/AP` with a
/// normal appearance that is a stream, or a dictionary of streams (one per state).
pub(crate) fn has_appearance(doc: &Document, d: &Dict) -> bool {
    let Some(ap) = d.get(b"AP").map(|a| doc.resolve(a)) else { return false };
    let Some(normal) = ap.as_dict().and_then(|ap| ap.get(b"N")).map(|n| doc.resolve(n)) else { return false };
    match normal.as_ref() {
        Object::Stream(_) => true,
        Object::Dict(states) => !states.is_empty() && states.iter().all(|(_, s)| matches!(doc.resolve(s).as_ref(), Object::Stream(_))),
        _ => false,
    }
}
