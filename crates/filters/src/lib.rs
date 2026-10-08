//! PDF stream filters (ISO 32000-2 §7.4).
//!
//! Layer L0, standalone: this crate depends on no other PdfCraft crate.
//!
//! Decoding covers the general-purpose filters (`FlateDecode`, `LZWDecode` with
//! TIFF/PNG predictors, `ASCIIHexDecode`, `ASCII85Decode`, `RunLengthDecode`).
//! Image codecs (`DCTDecode`, `JPXDecode`, `JBIG2Decode`, `CCITTFaxDecode`),
//! `Crypt` and unknown filters are reported as [`FilterError::Unsupported`] so
//! callers keep such data encoded.
//!
//! Every decoder is bounded by a caller-supplied `max_output` (decompression-bomb
//! defence) and never panics on malformed input.
#![forbid(unsafe_code)]
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

mod ascii85;
mod asciihex;
mod flate;
mod lzw;
mod predictor;
mod runlength;

use std::borrow::Cow;

/// A PDF stream filter, identified by its `/Filter` name.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum Filter {
    Flate,
    Lzw,
    AsciiHex,
    Ascii85,
    RunLength,
    Dct,
    Jpx,
    Jbig2,
    CcittFax,
    Crypt,
    /// Any other name (kept verbatim, lossily converted to UTF-8).
    Unknown(String),
}

impl Filter {
    /// Parses a filter name. Accepts the full names (ISO 32000-2 Table 6) and the
    /// inline-image abbreviations (Table 92): `Fl`, `LZW`, `AHx`, `A85`, `RL`, `DCT`, `CCF`.
    /// A leading `/` is ignored.
    pub fn from_name(name: &[u8]) -> Filter {
        let n = name.strip_prefix(b"/").unwrap_or(name);
        match n {
            b"FlateDecode" | b"Fl" => Filter::Flate,
            b"LZWDecode" | b"LZW" => Filter::Lzw,
            b"ASCIIHexDecode" | b"AHx" => Filter::AsciiHex,
            b"ASCII85Decode" | b"A85" => Filter::Ascii85,
            b"RunLengthDecode" | b"RL" => Filter::RunLength,
            b"DCTDecode" | b"DCT" => Filter::Dct,
            b"JPXDecode" => Filter::Jpx,
            b"JBIG2Decode" => Filter::Jbig2,
            b"CCITTFaxDecode" | b"CCF" => Filter::CcittFax,
            b"Crypt" => Filter::Crypt,
            other => Filter::Unknown(String::from_utf8_lossy(other).into_owned()),
        }
    }

    /// The full PDF name (without the leading `/`).
    pub fn name(&self) -> &str {
        match self {
            Filter::Flate => "FlateDecode",
            Filter::Lzw => "LZWDecode",
            Filter::AsciiHex => "ASCIIHexDecode",
            Filter::Ascii85 => "ASCII85Decode",
            Filter::RunLength => "RunLengthDecode",
            Filter::Dct => "DCTDecode",
            Filter::Jpx => "JPXDecode",
            Filter::Jbig2 => "JBIG2Decode",
            Filter::CcittFax => "CCITTFaxDecode",
            Filter::Crypt => "Crypt",
            Filter::Unknown(s) => s,
        }
    }

    /// True for the image codecs (DCT, JPX, JBIG2, CCITT fax).
    pub fn is_image_codec(&self) -> bool {
        matches!(self, Filter::Dct | Filter::Jpx | Filter::Jbig2 | Filter::CcittFax)
    }

    fn is_decodable(&self) -> bool {
        matches!(self, Filter::Flate | Filter::Lzw | Filter::AsciiHex | Filter::Ascii85 | Filter::RunLength)
    }
}

/// `/DecodeParms` values relevant to the general-purpose filters (ISO 32000-2 Tables 8–9).
#[derive(Clone, Debug, PartialEq)]
pub struct Params {
    /// 1 = none, 2 = TIFF predictor 2, 10–15 = PNG predictors.
    pub predictor: i64,
    pub colors: i64,
    pub bits_per_component: i64,
    pub columns: i64,
    /// LZW only: 1 (default) switches code width one code early, 0 does not.
    pub early_change: i64,
}

impl Default for Params {
    fn default() -> Self {
        Params { predictor: 1, colors: 1, bits_per_component: 8, columns: 1, early_change: 1 }
    }
}

/// Errors from decoding or encoding.
#[derive(Debug, thiserror::Error)]
pub enum FilterError {
    #[error("{filter}: corrupt data: {detail}")]
    Corrupt { filter: &'static str, detail: String },
    #[error("decoded output exceeds the limit of {0} bytes")]
    LimitExceeded(usize),
    #[error("unsupported filter: {0}")]
    Unsupported(String),
}

/// A decoder failure carrying whatever was decoded before the error.
pub(crate) struct Failure {
    pub(crate) error: FilterError,
    pub(crate) partial: Vec<u8>,
}

impl Failure {
    pub(crate) fn corrupt(filter: &'static str, detail: impl Into<String>, partial: Vec<u8>) -> Self {
        Failure { error: FilterError::Corrupt { filter, detail: detail.into() }, partial }
    }

    pub(crate) fn limit(max: usize) -> Self {
        Failure { error: FilterError::LimitExceeded(max), partial: Vec::new() }
    }
}

pub(crate) type Step = Result<Vec<u8>, Failure>;

/// Decodes `data` through `chain` (first element applied first).
///
/// Image codecs, `Crypt` and unknown filters anywhere in the chain give
/// [`FilterError::Unsupported`] (callers keep such data encoded). `max_output` bounds
/// every intermediate buffer.
pub fn decode(chain: &[(Filter, Params)], data: &[u8], max_output: usize) -> Result<Vec<u8>, FilterError> {
    run(chain, data, max_output, true).map(|(v, _)| v)
}

/// Like [`decode`], but on corrupt data returns what was decoded before the error and
/// reports `partial = true` (the behaviour viewers expect). The remaining filters of the
/// chain still run on the partial data. Limit and unsupported errors are still errors.
pub fn decode_tolerant(chain: &[(Filter, Params)], data: &[u8], max_output: usize) -> Result<(Vec<u8>, bool), FilterError> {
    run(chain, data, max_output, false)
}

fn run(chain: &[(Filter, Params)], data: &[u8], max: usize, strict: bool) -> Result<(Vec<u8>, bool), FilterError> {
    if let Some((f, _)) = chain.iter().find(|(f, _)| !f.is_decodable()) {
        return Err(FilterError::Unsupported(f.name().to_owned()));
    }
    let mut cur: Cow<[u8]> = Cow::Borrowed(data);
    let mut partial = false;
    for (filter, params) in chain {
        cur = Cow::Owned(match decode_one(filter, params, &cur, max) {
            Ok(v) => v,
            Err(Failure { error: FilterError::Corrupt { .. }, partial: out }) if !strict => {
                partial = true;
                out
            }
            Err(f) => return Err(f.error),
        });
    }
    Ok((cur.into_owned(), partial))
}

fn decode_one(filter: &Filter, params: &Params, data: &[u8], max: usize) -> Step {
    match filter {
        Filter::Flate => with_predictor(flate::decode(data, max), params, "FlateDecode"),
        Filter::Lzw => with_predictor(lzw::decode(data, params.early_change != 0, max), params, "LZWDecode"),
        Filter::AsciiHex => asciihex::decode(data, max),
        Filter::Ascii85 => ascii85::decode(data, max),
        Filter::RunLength => runlength::decode(data, max),
        other => Err(Failure { error: FilterError::Unsupported(other.name().to_owned()), partial: Vec::new() }),
    }
}

/// Applies the predictor to a decoder's result, including to partial output on failure.
fn with_predictor(step: Step, params: &Params, name: &'static str) -> Step {
    match step {
        Ok(v) => predictor::decode(params, v, name),
        Err(Failure { error, partial }) => {
            let partial = match predictor::decode(params, partial, name) {
                Ok(v) => v,
                Err(f) => f.partial,
            };
            Err(Failure { error, partial })
        }
    }
}

/// Encodes `data` with `filter`. Supported: Flate (zlib, level 6), LZW, ASCIIHex,
/// ASCII85, RunLength. For Flate and LZW, `params.predictor >= 2` applies the
/// predictor first (TIFF 2, PNG 10–15; 15 chooses the per-row optimum).
pub fn encode(filter: &Filter, params: &Params, data: &[u8]) -> Result<Vec<u8>, FilterError> {
    match filter {
        Filter::Flate => Ok(flate::encode(&predictor::encode(params, data)?)),
        Filter::Lzw => Ok(lzw::encode(&predictor::encode(params, data)?, params.early_change != 0)),
        Filter::AsciiHex => Ok(asciihex::encode(data)),
        Filter::Ascii85 => Ok(ascii85::encode(data)),
        Filter::RunLength => Ok(runlength::encode(data)),
        other => Err(FilterError::Unsupported(format!("encoding with {}", other.name()))),
    }
}

/// zlib (RFC 1950) at compression level 6.
pub fn encode_flate(data: &[u8]) -> Vec<u8> {
    flate::encode(data)
}

/// PDF white-space characters (ISO 32000-2 Table 1).
pub(crate) fn is_pdf_whitespace(b: u8) -> bool {
    matches!(b, 0 | 9 | 10 | 12 | 13 | 32)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_and_abbreviations() {
        for (full, abbr, f) in [
            (&b"FlateDecode"[..], &b"Fl"[..], Filter::Flate),
            (b"LZWDecode", b"LZW", Filter::Lzw),
            (b"ASCIIHexDecode", b"AHx", Filter::AsciiHex),
            (b"ASCII85Decode", b"A85", Filter::Ascii85),
            (b"RunLengthDecode", b"RL", Filter::RunLength),
            (b"DCTDecode", b"DCT", Filter::Dct),
            (b"CCITTFaxDecode", b"CCF", Filter::CcittFax),
        ] {
            assert_eq!(Filter::from_name(full), f);
            assert_eq!(Filter::from_name(abbr), f);
            assert_eq!(f.name().as_bytes(), full);
        }
        assert_eq!(Filter::from_name(b"/JPXDecode"), Filter::Jpx);
        assert_eq!(Filter::from_name(b"JBIG2Decode"), Filter::Jbig2);
        assert_eq!(Filter::from_name(b"Crypt"), Filter::Crypt);
        assert_eq!(Filter::from_name(b"Foo"), Filter::Unknown("Foo".into()));
        assert_eq!(Filter::Unknown("Foo".into()).name(), "Foo");
        assert!(Filter::Dct.is_image_codec() && Filter::CcittFax.is_image_codec());
        assert!(!Filter::Flate.is_image_codec() && !Filter::Crypt.is_image_codec());
    }

    #[test]
    fn default_params() {
        let p = Params::default();
        assert_eq!((p.predictor, p.colors, p.bits_per_component, p.columns, p.early_change), (1, 1, 8, 1, 1));
    }

    #[test]
    fn unsupported_filters() {
        for f in [Filter::Dct, Filter::Jpx, Filter::Jbig2, Filter::CcittFax, Filter::Crypt, Filter::Unknown("X".into())] {
            let chain = [(Filter::AsciiHex, Params::default()), (f.clone(), Params::default())];
            assert!(matches!(decode(&chain, b"00>", 100), Err(FilterError::Unsupported(_))));
            assert!(matches!(decode_tolerant(&chain, b"00>", 100), Err(FilterError::Unsupported(_))));
            assert!(matches!(encode(&f, &Params::default(), b"x"), Err(FilterError::Unsupported(_))));
        }
    }

    #[test]
    fn empty_chain_is_identity() {
        assert_eq!(decode(&[], b"abc", 1).unwrap(), b"abc");
    }

    #[test]
    fn chain_ascii85_flate() {
        let data = b"Hello, chained filters! Hello, chained filters!".repeat(10);
        let z = encode_flate(&data);
        let a = encode(&Filter::Ascii85, &Params::default(), &z).unwrap();
        let chain = [(Filter::Ascii85, Params::default()), (Filter::Flate, Params::default())];
        assert_eq!(decode(&chain, &a, 1 << 20).unwrap(), data);
    }

    #[test]
    fn chain_three_filters_with_predictor() {
        let p = Params { predictor: 12, colors: 3, columns: 5, ..Params::default() };
        let data: Vec<u8> = (0..300u32).map(|i| (i * 7 % 251) as u8).collect();
        let lzw = encode(&Filter::Lzw, &p, &data).unwrap();
        let rl = encode(&Filter::RunLength, &Params::default(), &lzw).unwrap();
        let hex = encode(&Filter::AsciiHex, &Params::default(), &rl).unwrap();
        let chain = [(Filter::AsciiHex, Params::default()), (Filter::RunLength, Params::default()), (Filter::Lzw, p)];
        assert_eq!(decode(&chain, &hex, 1 << 20).unwrap(), data);
    }

    #[test]
    fn limit_applies_to_intermediate_buffers() {
        let hex = encode(&Filter::AsciiHex, &Params::default(), &[7u8; 100]).unwrap();
        let chain = [(Filter::AsciiHex, Params::default())];
        assert!(matches!(decode(&chain, &hex, 99), Err(FilterError::LimitExceeded(99))));
        assert_eq!(decode(&chain, &hex, 100).unwrap().len(), 100);
    }

    #[test]
    fn tolerant_flate_returns_partial() {
        let data: Vec<u8> = (0..20_000u32).map(|i| (i % 97) as u8 ^ (i / 300) as u8).collect();
        let mut z = encode_flate(&data);
        let cut = z.len() / 2;
        z.truncate(cut);
        let chain = [(Filter::Flate, Params::default())];
        assert!(matches!(decode(&chain, &z, 1 << 20), Err(FilterError::Corrupt { .. })));
        let (out, partial) = decode_tolerant(&chain, &z, 1 << 20).unwrap();
        assert!(partial);
        assert!(!out.is_empty() && out.len() < data.len());
        assert_eq!(&data[..out.len()], &out[..]);
        // Complete data is not partial.
        let (out, partial) = decode_tolerant(&chain, &encode_flate(&data), 1 << 20).unwrap();
        assert!(!partial);
        assert_eq!(out, data);
    }

    #[test]
    fn tolerant_partial_flows_through_predictor() {
        let p = Params { predictor: 12, columns: 4, ..Params::default() };
        let data: Vec<u8> = (0..4000u32).map(|i| (i * 31 % 256) as u8).collect();
        let mut z = encode(&Filter::Flate, &p, &data).unwrap();
        z.truncate(z.len() - 20);
        let (out, partial) = decode_tolerant(&[(Filter::Flate, p)], &z, 1 << 20).unwrap();
        assert!(partial);
        assert_eq!(&data[..out.len()], &out[..]);
    }
}
