//! The PDF object model (ISO 32000-2 §7.3).
//!
//! Dictionaries keep their key order so rewritten objects stay recognisable in diffs and
//! byte-stable when re-serialized. Streams keep their *encoded* bytes; decoding is on demand.

use std::sync::Arc;

use pdfcraft_filters::{Filter, Params};

use crate::CosError;

/// An indirect reference: object number and generation.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ObjRef {
    pub num: u32,
    pub generation: u16,
}

impl ObjRef {
    pub const fn new(num: u32, generation: u16) -> Self {
        Self { num, generation }
    }
}

impl std::fmt::Display for ObjRef {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{} {} R", self.num, self.generation)
    }
}

/// A string object. `hex` records how it was written, so it is re-emitted the same way.
#[derive(Clone, Debug, PartialEq)]
pub struct PdfString {
    pub bytes: Vec<u8>,
    pub hex: bool,
}

impl PdfString {
    pub fn literal(bytes: impl Into<Vec<u8>>) -> Self {
        Self { bytes: bytes.into(), hex: false }
    }

    /// A text string (ISO 32000-2 §7.9.2.2): PDFDocEncoding when possible, else UTF-16BE with BOM.
    pub fn text(s: &str) -> Self {
        if s.chars().all(|c| (' '..='~').contains(&c) || c == '\n' || c == '\r' || c == '\t') {
            return Self::literal(s.as_bytes().to_vec());
        }
        let mut bytes = vec![0xFE, 0xFF];
        for u in s.encode_utf16() {
            bytes.extend_from_slice(&u.to_be_bytes());
        }
        Self { bytes, hex: false }
    }

    /// Decode a text string: UTF-16BE (BOM), UTF-8 (BOM, PDF 2.0) or PDFDocEncoding.
    pub fn to_text(&self) -> String {
        let b = &self.bytes;
        if b.len() >= 2 && b[0] == 0xFE && b[1] == 0xFF {
            let units: Vec<u16> = b[2..].as_chunks::<2>().0.iter().map(|c| u16::from_be_bytes(*c)).collect();
            return String::from_utf16_lossy(&units);
        }
        if b.len() >= 3 && b[..3] == [0xEF, 0xBB, 0xBF] {
            return String::from_utf8_lossy(&b[3..]).into_owned();
        }
        b.iter().map(|&c| pdfdoc_char(c)).collect()
    }
}

/// PDFDocEncoding → Unicode (ISO 32000-2 Annex D). Bytes 0x18–0x1F and 0x80–0xA0 differ from Latin-1.
fn pdfdoc_char(c: u8) -> char {
    const HIGH: [char; 33] = [
        '•', '†', '‡', '…', '—', '–', 'ƒ', '⁄', '‹', '›', '−', '‰', '„', '“', '”', '‘', '’', '‚', '™', 'ﬁ', 'ﬂ', 'Ł', 'Œ', 'Š', 'Ÿ', 'Ž', 'ı', 'ł',
        'œ', 'š', 'ž', '\u{FFFD}', '€',
    ];
    const LOW: [char; 8] = ['˘', 'ˇ', 'ˆ', '˙', '˝', '˛', '˚', '˜'];
    match c {
        0x18..=0x1F => LOW[(c - 0x18) as usize],
        0x80..=0xA0 => HIGH[(c - 0x80) as usize],
        _ => c as char,
    }
}

/// A name object (without the leading `/`, with `#xx` escapes already decoded).
pub type Name = Vec<u8>;

/// An ordered dictionary.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Dict {
    entries: Vec<(Name, Object)>,
}

impl Dict {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn get(&self, key: &[u8]) -> Option<&Object> {
        self.entries.iter().find(|(k, _)| k == key).map(|(_, v)| v)
    }

    pub fn get_mut(&mut self, key: &[u8]) -> Option<&mut Object> {
        self.entries.iter_mut().find(|(k, _)| k == key).map(|(_, v)| v)
    }

    /// Insert or replace, keeping the position of an existing key.
    pub fn set(&mut self, key: impl Into<Name>, value: impl Into<Object>) {
        let key = key.into();
        let value = value.into();
        match self.entries.iter_mut().find(|(k, _)| *k == key) {
            Some((_, v)) => *v = value,
            None => self.entries.push((key, value)),
        }
    }

    pub fn remove(&mut self, key: &[u8]) -> Option<Object> {
        let i = self.entries.iter().position(|(k, _)| k == key)?;
        Some(self.entries.remove(i).1)
    }

    pub fn contains(&self, key: &[u8]) -> bool {
        self.get(key).is_some()
    }

    pub fn iter(&self) -> impl Iterator<Item = (&Name, &Object)> {
        self.entries.iter().map(|(k, v)| (k, v))
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub fn name(&self, key: &[u8]) -> Option<&[u8]> {
        self.get(key).and_then(Object::as_name)
    }

    pub fn int(&self, key: &[u8]) -> Option<i64> {
        self.get(key).and_then(Object::as_int)
    }

    pub fn reference(&self, key: &[u8]) -> Option<ObjRef> {
        self.get(key).and_then(Object::as_ref)
    }
}

impl FromIterator<(Name, Object)> for Dict {
    fn from_iter<T: IntoIterator<Item = (Name, Object)>>(iter: T) -> Self {
        let mut d = Dict::new();
        for (k, v) in iter {
            d.set(k, v);
        }
        d
    }
}

/// A stream: dictionary plus its data *as stored* (still filtered).
#[derive(Clone, Debug, PartialEq)]
pub struct Stream {
    pub dict: Dict,
    pub raw: Arc<Vec<u8>>,
}

/// Upper bound for a single decoded stream (decompression-bomb defence).
pub const MAX_DECODED: usize = 1 << 30;

impl Stream {
    /// A new stream from already-encoded bytes (`/Length` is set at write time).
    pub fn from_raw(dict: Dict, raw: Vec<u8>) -> Self {
        Self { dict, raw: Arc::new(raw) }
    }

    /// A new stream holding `data` compressed with Flate.
    pub fn flate(mut dict: Dict, data: &[u8]) -> Self {
        dict.set(b"Filter".to_vec(), Object::Name(b"FlateDecode".to_vec()));
        dict.remove(b"DecodeParms");
        Self { dict, raw: Arc::new(pdfcraft_filters::encode_flate(data)) }
    }

    /// The filter chain declared in the dictionary.
    pub fn filters(&self) -> Vec<(Filter, Params)> {
        let names: Vec<&[u8]> = match self.dict.get(b"Filter") {
            Some(Object::Name(n)) => vec![n],
            Some(Object::Array(a)) => a.iter().filter_map(Object::as_name).collect(),
            _ => Vec::new(),
        };
        let parms: Vec<Option<&Dict>> = match self.dict.get(b"DecodeParms") {
            Some(Object::Dict(d)) => vec![Some(d)],
            Some(Object::Array(a)) => a.iter().map(Object::as_dict).collect(),
            _ => Vec::new(),
        };
        names
            .into_iter()
            .enumerate()
            // `/Crypt` is applied by the security handler when the object is loaded (§7.4.10).
            .filter(|(_, n)| *n != b"Crypt")
            .map(|(i, n)| {
                let p = parms.get(i).copied().flatten();
                let get = |k: &[u8], d: i64| p.and_then(|p| p.int(k)).unwrap_or(d);
                let params = Params {
                    predictor: get(b"Predictor", 1),
                    colors: get(b"Colors", 1),
                    bits_per_component: get(b"BitsPerComponent", 8),
                    columns: get(b"Columns", 1),
                    early_change: get(b"EarlyChange", 1),
                };
                (Filter::from_name(n), params)
            })
            .collect()
    }

    /// Decoded data, tolerating truncated or corrupt encodings the way viewers do (the data
    /// decoded before the damage is returned). Streams with image codecs fail with
    /// `CosError::Filter` (keep them encoded). Use `decoded_strict` to detect damage.
    pub fn decoded(&self) -> Result<Vec<u8>, CosError> {
        self.decoded_within(MAX_DECODED)
    }

    /// Like [`Stream::decoded`], for a stream whose decoded size has a tighter bound than
    /// [`MAX_DECODED`]: decoding past `max` bytes is an error.
    pub fn decoded_within(&self, max: usize) -> Result<Vec<u8>, CosError> {
        let chain = self.filters();
        if chain.is_empty() {
            return Ok(self.raw.as_ref().clone());
        }
        pdfcraft_filters::decode_tolerant(&chain, &self.raw, max.min(MAX_DECODED)).map(|(v, _)| v).map_err(|e| CosError::Filter(e.to_string()))
    }

    /// Decoded data; any corruption is an error.
    pub fn decoded_strict(&self) -> Result<Vec<u8>, CosError> {
        let chain = self.filters();
        if chain.is_empty() {
            return Ok(self.raw.as_ref().clone());
        }
        pdfcraft_filters::decode(&chain, &self.raw, MAX_DECODED).map_err(|e| CosError::Filter(e.to_string()))
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum Object {
    Null,
    Bool(bool),
    Int(i64),
    Real(f64),
    String(PdfString),
    Name(Name),
    Array(Vec<Object>),
    Dict(Dict),
    Stream(Stream),
    Ref(ObjRef),
}

impl Object {
    pub fn name(n: &str) -> Self {
        Object::Name(n.as_bytes().to_vec())
    }

    pub fn as_int(&self) -> Option<i64> {
        match self {
            Object::Int(i) => Some(*i),
            Object::Real(r) if r.fract() == 0.0 && r.is_finite() => Some(*r as i64),
            _ => None,
        }
    }

    pub fn as_f64(&self) -> Option<f64> {
        match self {
            Object::Int(i) => Some(*i as f64),
            Object::Real(r) => Some(*r),
            _ => None,
        }
    }

    pub fn as_name(&self) -> Option<&[u8]> {
        match self {
            Object::Name(n) => Some(n),
            _ => None,
        }
    }

    pub fn as_ref(&self) -> Option<ObjRef> {
        match self {
            Object::Ref(r) => Some(*r),
            _ => None,
        }
    }

    pub fn as_dict(&self) -> Option<&Dict> {
        match self {
            Object::Dict(d) => Some(d),
            Object::Stream(s) => Some(&s.dict),
            _ => None,
        }
    }

    pub fn as_dict_mut(&mut self) -> Option<&mut Dict> {
        match self {
            Object::Dict(d) => Some(d),
            Object::Stream(s) => Some(&mut s.dict),
            _ => None,
        }
    }

    pub fn as_array(&self) -> Option<&Vec<Object>> {
        match self {
            Object::Array(a) => Some(a),
            _ => None,
        }
    }

    pub fn as_string(&self) -> Option<&PdfString> {
        match self {
            Object::String(s) => Some(s),
            _ => None,
        }
    }
}

impl From<i64> for Object {
    fn from(v: i64) -> Self {
        Object::Int(v)
    }
}
impl From<i32> for Object {
    fn from(v: i32) -> Self {
        Object::Int(v as i64)
    }
}
impl From<f64> for Object {
    fn from(v: f64) -> Self {
        Object::Real(v)
    }
}
impl From<bool> for Object {
    fn from(v: bool) -> Self {
        Object::Bool(v)
    }
}
impl From<ObjRef> for Object {
    fn from(v: ObjRef) -> Self {
        Object::Ref(v)
    }
}
impl From<Dict> for Object {
    fn from(v: Dict) -> Self {
        Object::Dict(v)
    }
}
impl From<Vec<Object>> for Object {
    fn from(v: Vec<Object>) -> Self {
        Object::Array(v)
    }
}
impl From<PdfString> for Object {
    fn from(v: PdfString) -> Self {
        Object::String(v)
    }
}
impl From<Stream> for Object {
    fn from(v: Stream) -> Self {
        Object::Stream(v)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn text_strings_round_trip() {
        for s in ["Plain ASCII", "Café — naïve", "日本語", "emoji 📄"] {
            assert_eq!(PdfString::text(s).to_text(), s);
        }
        assert!(!PdfString::text("abc").bytes.starts_with(&[0xFE, 0xFF]));
    }

    #[test]
    fn pdfdoc_encoding_high_range() {
        assert_eq!(PdfString::literal(vec![0x84, 0x92, 0xA0, b'A']).to_text(), "—™€A");
    }

    #[test]
    fn dict_preserves_order_and_replaces_in_place() {
        let mut d = Dict::new();
        d.set(b"B".to_vec(), 1);
        d.set(b"A".to_vec(), 2);
        d.set(b"B".to_vec(), 3);
        let keys: Vec<_> = d.iter().map(|(k, _)| k.clone()).collect();
        assert_eq!(keys, vec![b"B".to_vec(), b"A".to_vec()]);
        assert_eq!(d.int(b"B"), Some(3));
    }
}
