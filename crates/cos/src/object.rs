//! The PDF object model (ISO 32000-2 §7.3).
//!
//! Dictionaries keep their key order so rewritten objects stay recognisable in diffs and
//! byte-stable when re-serialized. Streams keep their *encoded* bytes; decoding is on demand.

use std::borrow::Borrow;

use pdfcraft_filters::{Filter, Params};

use crate::{Bytes, CosError, Document};

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
    ///
    /// Lenient beyond the spec, as readers are: invalid sequences after a BOM decode lossily
    /// instead of dropping the string, a BOM-less string whose bytes are valid UTF-8 with
    /// non-ASCII bytes (written by some producers) is read as UTF-8 rather than as
    /// PDFDocEncoding mojibake, and stray leading U+FEFF (doubled BOMs) are removed.
    pub fn to_text(&self) -> String {
        let s = match self.bytes.as_slice() {
            [0xFE, 0xFF, rest @ ..] => {
                let units: Vec<u16> = rest.as_chunks::<2>().0.iter().map(|c| u16::from_be_bytes(*c)).collect();
                String::from_utf16_lossy(&units)
            }
            [0xEF, 0xBB, 0xBF, rest @ ..] => String::from_utf8_lossy(rest).into_owned(),
            b => match std::str::from_utf8(b) {
                // Pure ASCII is identical either way; only non-ASCII UTF-8 needs the switch.
                Ok(utf8) if !b.is_ascii() => utf8.to_owned(),
                _ => b.iter().map(|&c| pdfcraft_crypt::pdfdoc_char(c)).collect(),
            },
        };
        if s.starts_with('\u{FEFF}') { s.trim_start_matches('\u{FEFF}').to_owned() } else { s }
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
    pub raw: Bytes,
}

/// Upper bound for a single decoded stream (decompression-bomb defence).
pub const MAX_DECODED: usize = 1 << 30;

impl Stream {
    /// A new stream from already-encoded bytes (`/Length` is set at write time).
    pub fn from_raw(dict: Dict, raw: Vec<u8>) -> Self {
        Self { dict, raw: raw.into() }
    }

    /// A new stream holding `data` compressed with Flate.
    pub fn flate(mut dict: Dict, data: &[u8]) -> Self {
        dict.set(b"Filter".to_vec(), Object::Name(b"FlateDecode".to_vec()));
        dict.remove(b"DecodeParms");
        Self { dict, raw: pdfcraft_filters::encode_flate(data).into() }
    }

    /// The filter chain declared in the dictionary, as written: an indirect `/Filter` or
    /// `/DecodeParms`, or indirect elements of their arrays, are not followed (there is no
    /// document here) and count as absent. See [`Stream::filters_in`] to resolve them.
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
        filter_chain(names.into_iter(), &parms, Dict::int)
    }

    /// The filter chain resolved through `doc`: `/Filter` and `/DecodeParms` may be indirect
    /// references, and so may the elements of their arrays and the parameter values
    /// (ISO 32000-2 §7.3.8.2, §7.4). An error when an entry does not resolve to a name or a
    /// dictionary, or when the dictionary carries `/F` (the data then lives in another file and
    /// the bytes between `stream` and `endstream` are to be ignored, Table 5) or `/DP` (an
    /// inline-image key, Table 92, that a viewer may read before `/DecodeParms`): the stored bytes
    /// are then not the data the document shows.
    pub fn filters_in(&self, doc: &Document) -> Result<Vec<(Filter, Params)>, CosError> {
        let unresolved = |what: &str| CosError::Filter(format!("the stream's {what} does not resolve to a filter name or parameter dictionary"));
        if self.dict.contains(b"F") || self.dict.contains(b"DP") {
            return Err(CosError::Filter("the stream dictionary carries /F or /DP, so its stored bytes are not the data it shows".into()));
        }
        let names: Vec<Name> = match self.dict.get(b"Filter").map(|f| doc.resolve(f)).as_deref() {
            None | Some(Object::Null) => Vec::new(),
            Some(Object::Name(n)) => vec![n.clone()],
            Some(Object::Array(a)) => a
                .iter()
                .map(|f| doc.resolve(f).as_name().map(<[u8]>::to_vec).ok_or_else(|| unresolved("/Filter element")))
                .collect::<Result<_, _>>()?,
            Some(_) => return Err(unresolved("/Filter")),
        };
        let parms: Vec<Option<Dict>> = match self.dict.get(b"DecodeParms").map(|p| doc.resolve(p)).as_deref() {
            None | Some(Object::Null) => Vec::new(),
            Some(Object::Dict(d)) => vec![Some(d.clone())],
            Some(Object::Array(a)) => a
                .iter()
                .map(|p| match &*doc.resolve(p) {
                    Object::Dict(d) => Ok(Some(d.clone())),
                    Object::Null => Ok(None),
                    _ => Err(unresolved("/DecodeParms element")),
                })
                .collect::<Result<_, _>>()?,
            Some(_) => return Err(unresolved("/DecodeParms")),
        };
        Ok(filter_chain(names.iter().map(Vec::as_slice), &parms, |p, k| p.get(k).and_then(|v| doc.resolve(v).as_int())))
    }

    /// Decoded data with the chain resolved through `doc` ([`Stream::filters_in`]) and any
    /// corruption an error: the whole stream decoded with the parameters the document declares,
    /// or nothing. For consumers that must not act on bytes a viewer would read differently
    /// (redaction, sanitising).
    pub fn decoded_strict_in(&self, doc: &Document) -> Result<Vec<u8>, CosError> {
        self.decode_strict(&self.filters_in(doc)?, MAX_DECODED)
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
            return self.raw_within(max);
        }
        pdfcraft_filters::decode_tolerant(&chain, &self.raw, max.min(MAX_DECODED)).map(|(v, _)| v).map_err(|e| CosError::Filter(e.to_string()))
    }

    /// Decoded data, bounded by [`MAX_DECODED`]; any corruption is an error.
    pub fn decoded_strict(&self) -> Result<Vec<u8>, CosError> {
        self.decode_strict(&self.filters(), MAX_DECODED)
    }

    /// Data after the first filter alone, bounded by `max`: the rest of the chain stays encoded.
    /// Any corruption in that first filter is an error.
    pub fn decoded_first_filter_strict(&self, max: usize) -> Result<Vec<u8>, CosError> {
        let mut chain = self.filters();
        chain.truncate(1);
        self.decode_strict(&chain, max)
    }

    /// The stored bytes run through `chain`, any corruption an error. The output is bounded by
    /// `max` and by [`MAX_DECODED`]; an empty chain yields the stored bytes under the same bound.
    fn decode_strict(&self, chain: &[(Filter, Params)], max: usize) -> Result<Vec<u8>, CosError> {
        if chain.is_empty() {
            return self.raw_within(max);
        }
        pdfcraft_filters::decode(chain, &self.raw, max.min(MAX_DECODED)).map_err(|e| CosError::Filter(e.to_string()))
    }

    fn raw_within(&self, max: usize) -> Result<Vec<u8>, CosError> {
        let max = max.min(MAX_DECODED);
        // An empty decoder chain still produces an owned output buffer: check its limit
        // before cloning, including streams whose /Crypt filter was applied on load.
        if self.raw.len() > max {
            return Err(CosError::Filter(pdfcraft_filters::FilterError::LimitExceeded(max).to_string()));
        }
        Ok(self.raw.to_vec())
    }
}

/// The decoder chain for the filter `names`, in order, each with the parameter dictionary at its
/// position in `parms` (none when the list is shorter or holds no dictionary there). `/Crypt` is
/// left out, and a parameter that `int` does not find takes its default. `int` reads one integer
/// entry of a parameter dictionary: whether it follows references is the caller's choice.
fn filter_chain<'a, D: Borrow<Dict>>(
    names: impl Iterator<Item = &'a [u8]>,
    parms: &[Option<D>],
    int: impl Fn(&Dict, &[u8]) -> Option<i64>,
) -> Vec<(Filter, Params)> {
    names
        .enumerate()
        // `/Crypt` is applied by the security handler when the object is loaded (§7.4.10).
        .filter(|(_, n)| *n != b"Crypt")
        .map(|(i, n)| {
            let p = parms.get(i).and_then(Option::as_ref).map(D::borrow);
            let get = |k: &[u8], d: i64| p.and_then(|p| int(p, k)).unwrap_or(d);
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
    fn text_string_encodings() {
        // UTF-16BE with BOM.
        let mut utf16 = vec![0xFE, 0xFF];
        utf16.extend("第一章 概述".encode_utf16().flat_map(u16::to_be_bytes));
        assert_eq!(PdfString::literal(utf16).to_text(), "第一章 概述");
        // UTF-8 with BOM (PDF 2.0): the BOM is not part of the text.
        let mut utf8 = vec![0xEF, 0xBB, 0xBF];
        utf8.extend_from_slice("第一章".as_bytes());
        assert_eq!(PdfString::literal(utf8).to_text(), "第一章");
        // Invalid UTF-8 after the BOM decodes lossily instead of vanishing.
        assert_eq!(PdfString::literal(vec![0xEF, 0xBB, 0xBF, b'A', 0xFF, b'B']).to_text(), "A\u{FFFD}B");
        // BOM-less raw UTF-8 from producers that skip the BOM.
        assert_eq!(PdfString::literal("目录".as_bytes()).to_text(), "目录");
        // PDFDocEncoding Latin-1 (not valid UTF-8) stays PDFDocEncoding.
        assert_eq!(PdfString::literal(vec![b'C', b'a', b'f', 0xE9]).to_text(), "Café");
        // Doubled BOMs leave no U+FEFF behind.
        assert_eq!(PdfString::literal(vec![0xFE, 0xFF, 0xFE, 0xFF, 0x00, b'A']).to_text(), "A");
        assert_eq!(PdfString::literal(vec![0xEF, 0xBB, 0xBF, 0xEF, 0xBB, 0xBF, b'A']).to_text(), "A");
        // Odd trailing byte and empty strings never panic.
        assert_eq!(PdfString::literal(vec![0xFE, 0xFF, 0x00, b'A', 0x00]).to_text(), "A");
        assert_eq!(PdfString::literal(vec![0xFE, 0xFF]).to_text(), "");
        assert_eq!(PdfString::literal(Vec::new()).to_text(), "");
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

    #[test]
    fn raw_streams_refuse_decoding_past_a_caller_limit() {
        let stream = Stream::from_raw(Dict::new(), b"abc".to_vec());
        for max in [0, 2] {
            assert!(matches!(stream.decoded_within(max), Err(CosError::Filter(_))));
        }
    }

    #[test]
    fn empty_filter_arrays_refuse_decoding_past_a_caller_limit() {
        let mut dict = Dict::new();
        dict.set(b"Filter".to_vec(), Object::Array(Vec::new()));
        let stream = Stream::from_raw(dict, b"abc".to_vec());
        assert!(matches!(stream.decoded_within(2), Err(CosError::Filter(_))));
    }

    #[test]
    fn crypt_only_streams_refuse_decoding_past_a_caller_limit() {
        let mut dict = Dict::new();
        dict.set(b"Filter".to_vec(), Object::name("Crypt"));
        let mut parms = Dict::new();
        parms.set(b"Name".to_vec(), Object::name("Identity"));
        dict.set(b"DecodeParms".to_vec(), Object::Dict(parms));
        let stream = Stream::from_raw(dict, b"abc".to_vec());
        assert!(matches!(stream.decoded_within(2), Err(CosError::Filter(_))));
    }

    #[test]
    fn identity_streams_preserve_bytes_within_a_caller_limit() {
        let mut array = Dict::new();
        array.set(b"Filter".to_vec(), Object::Array(Vec::new()));
        let mut crypt = Dict::new();
        crypt.set(b"Filter".to_vec(), Object::name("Crypt"));
        for dict in [Dict::new(), array, crypt] {
            let stream = Stream::from_raw(dict.clone(), b"abc".to_vec());
            assert_eq!(stream.decoded_within(3).unwrap(), b"abc");
            assert_eq!(stream.decoded_within(usize::MAX).unwrap(), b"abc");
            assert_eq!(stream.decoded().unwrap(), b"abc");
            assert_eq!(stream.decoded_strict().unwrap(), b"abc");
            assert!(Stream::from_raw(dict, Vec::new()).decoded_within(0).unwrap().is_empty());
        }
    }

    #[test]
    fn filtered_streams_preserve_the_caller_limit() {
        let stream = Stream::flate(Dict::new(), b"abc");
        assert!(matches!(stream.decoded_within(2), Err(CosError::Filter(_))));
        assert_eq!(stream.decoded_within(3).unwrap(), b"abc");
        assert_eq!(stream.decoded_strict().unwrap(), b"abc");
    }

    #[test]
    fn first_filter_decodes_only_the_outer_wrapper() {
        let data = "0 0 m 100 100 l S\n".repeat(64).into_bytes();
        let inner = Stream::flate(Dict::new(), &data).raw.to_vec();
        for (outer, name, corrupt) in [(Filter::Ascii85, "ASCII85Decode", &b"{~>"[..]), (Filter::AsciiHex, "ASCIIHexDecode", &b"G0>"[..])] {
            let encoded = pdfcraft_filters::encode(&outer, &Params::default(), &inner).unwrap();
            let mut dict = Dict::new();
            dict.set(b"Filter".to_vec(), Object::Array(vec![Object::name(name), Object::name("FlateDecode")]));
            let stream = Stream::from_raw(dict.clone(), encoded);
            assert_eq!(stream.decoded_first_filter_strict(usize::MAX).unwrap(), inner);
            assert_eq!(stream.decoded_strict().unwrap(), data);
            assert!(matches!(stream.decoded_first_filter_strict(inner.len() - 1), Err(CosError::Filter(_))));
            assert!(matches!(Stream::from_raw(dict, corrupt.to_vec()).decoded_first_filter_strict(usize::MAX), Err(CosError::Filter(_))));
        }
    }

    /// An indirect /Filter or /DecodeParms, and indirect elements of their arrays, resolve
    /// through the document; what does not resolve, a dictionary carrying /F or /DP, and any
    /// corruption are errors, where the document-less decode returns the stored bytes as they are.
    #[test]
    fn filters_in_resolves_references_and_refuses_what_it_cannot() {
        let mut doc = Document::new_empty();
        let data = "0 0 m 100 100 l S\n".repeat(64).into_bytes();
        let z = Stream::flate(Dict::new(), &data).raw.to_vec();
        let name = doc.add(Object::name("FlateDecode"));
        let mut dict = Dict::new();
        dict.set(b"Filter".to_vec(), Object::Ref(name));
        let stream = Stream::from_raw(dict, z.clone());
        assert_eq!(stream.decoded().unwrap(), z, "without the document the chain is empty");
        assert_eq!(stream.decoded_strict_in(&doc).unwrap(), data);
        // The filter as an array element and the parameters as a dictionary, both by reference.
        let p = Params { predictor: 12, columns: 18, ..Params::default() };
        let predicted = pdfcraft_filters::encode(&Filter::Flate, &p, &data).unwrap();
        let mut parms = Dict::new();
        parms.set(b"Predictor".to_vec(), 12);
        parms.set(b"Columns".to_vec(), 18);
        let parms = doc.add(Object::Dict(parms));
        let mut dict = Dict::new();
        dict.set(b"Filter".to_vec(), Object::Array(vec![Object::Ref(name)]));
        dict.set(b"DecodeParms".to_vec(), Object::Array(vec![Object::Ref(parms)]));
        let stream = Stream::from_raw(dict, predicted);
        assert_eq!(stream.filters_in(&doc).unwrap(), vec![(Filter::Flate, p)]);
        assert_eq!(stream.decoded_strict_in(&doc).unwrap(), data);
        // Entries that do not resolve, the /F and /DP keys, and a stream cut short.
        let refused = [
            (&b"Filter"[..], Object::Int(1)),
            (&b"DecodeParms"[..], Object::Int(1)),
            (&b"F"[..], Object::name("ASCIIHexDecode")),
            (&b"DP"[..], Object::Dict(Dict::new())),
        ];
        for (key, value) in refused {
            let mut dict = Dict::new();
            dict.set(b"Filter".to_vec(), Object::Ref(name));
            dict.set(key.to_vec(), value);
            let stream = Stream::from_raw(dict, z.clone());
            assert!(matches!(stream.decoded_strict_in(&doc), Err(CosError::Filter(_))), "{}", String::from_utf8_lossy(key));
            assert!(stream.decoded().is_ok(), "the tolerant decode still answers");
        }
        let mut dict = Dict::new();
        dict.set(b"Filter".to_vec(), Object::Ref(name));
        let mut cut = z.clone();
        cut.truncate(z.len() / 2);
        assert!(matches!(Stream::from_raw(dict, cut).decoded_strict_in(&doc), Err(CosError::Filter(_))));
    }
}
