//! PDF stream filters (ISO 32000-2 §7.4).
//!
//! Layer L0, standalone: this crate depends on no other PdfCraft crate.
//!
//! Decoding covers the general-purpose filters (`FlateDecode`, `LZWDecode` with
//! TIFF/PNG predictors, `ASCIIHexDecode`, `ASCII85Decode`, `RunLengthDecode`).
//! Image codecs (`DCTDecode`, `JPXDecode`, `JBIG2Decode`, `CCITTFaxDecode`),
//! `Crypt` and unknown filters are reported as [`FilterError::Unsupported`] so
//! callers keep such data encoded. CCITT fax and JBIG2 images decode on their own
//! ([`decode_ccitt_within`], [`decode_jbig2_within`]), each spending from an allowance its caller
//! shares across a run of images, as `pdfcraft_cos::ImageBudget::decode` does after the filters
//! before them. Neither decodes without one:
//!
//! ```compile_fail
//! let _ = pdfcraft_filters::decode_jbig2(&[], None, 1, 1, 1 << 20);
//! ```
//!
//! ```compile_fail
//! let _ = pdfcraft_filters::decode_ccitt(&[], &pdfcraft_filters::CcittParams::default(), 1);
//! ```
//!
//! ```
//! let mut work = 1 << 20;
//! assert!(pdfcraft_filters::decode_jbig2_within(&[], None, 1, 1, 1 << 20, &mut work).is_err());
//! let p = pdfcraft_filters::CcittParams { columns: 8, ..Default::default() };
//! assert!(pdfcraft_filters::decode_ccitt_within(&[], &p, 1, &mut work).is_ok());
//! ```
//!
//! 1-bit samples can be encoded as CCITT Group 4 ([`encode_g4`]) or lossless JBIG2
//! ([`encode_jbig2`]), a group of images sharing their marks through `/JBIG2Globals`
//! ([`encode_jbig2_shared`]).
//!
//! Every decoder is bounded by a caller-supplied `max_output` (decompression-bomb
//! defence) and never panics on malformed input.
#![forbid(unsafe_code)]
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

mod ascii85;
mod asciihex;
mod ccitt;
mod flate;
mod jbig2;
mod lzw;
mod mq;
mod predictor;
mod runlength;

use std::borrow::Cow;

pub use ccitt::{CcittParams, MAX_CCITT_COLUMNS, MAX_G4_COLUMNS, decode_ccitt_within, encode_g4};
pub use jbig2::{MAX_JBIG2_COLUMNS, MAX_JBIG2_PIXELS, SharedJbig2, decode_jbig2_within, encode_jbig2, encode_jbig2_generic, encode_jbig2_shared};

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

    /// True for the general-purpose filters, which a chain can hold ([`decode`]).
    pub fn is_decodable(&self) -> bool {
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
    /// A run of images has nothing left to spend ([`decode_tolerant_within`]).
    #[error("more image decoding than one operation may do")]
    Spent,
}

/// A decoder failure carrying whatever was decoded before the error.
pub(crate) struct Failure {
    pub(crate) error: FilterError,
    pub(crate) partial: Vec<u8>,
    /// Bytes decoded on the way that `partial` doesn't hold: they took as long.
    pub(crate) discarded: usize,
}

impl Failure {
    pub(crate) fn corrupt(filter: &'static str, detail: impl Into<String>, partial: Vec<u8>) -> Self {
        Failure { error: FilterError::Corrupt { filter, detail: detail.into() }, partial, discarded: 0 }
    }

    pub(crate) fn limit(max: usize) -> Self {
        Failure { error: FilterError::LimitExceeded(max), partial: Vec::new(), discarded: 0 }
    }
}

pub(crate) type Step = Result<Vec<u8>, Failure>;

/// Takes `steps` from `work`, what a run of images may still cost. What it can't pay for spends
/// all of it.
pub(crate) fn pay(work: &mut u64, steps: u64) -> Result<(), FilterError> {
    match work.checked_sub(steps) {
        Some(left) => {
            *work = left;
            Ok(())
        }
        None => {
            *work = 0;
            Err(FilterError::Spent)
        }
    }
}

/// Decodes `data` through `chain` (first element applied first).
///
/// Image codecs, `Crypt` and unknown filters anywhere in the chain give
/// [`FilterError::Unsupported`] (callers keep such data encoded), as does a chain longer than
/// [`MAX_CHAIN`]. `max_output` bounds every intermediate buffer.
pub fn decode(chain: &[(Filter, Params)], data: &[u8], max_output: usize) -> Result<Vec<u8>, FilterError> {
    run(chain, data, max_output, None, true, false, None).map(|(v, _, _)| v).map_err(|(e, _)| e)
}

/// Like [`decode`] for a caller charging an allowance: `max_output` bounds the bytes all stages
/// decode together, each stage getting what the earlier ones left, and the result gives those
/// bytes, on success and on failure. A stage's output counts even when a later stage or its
/// predictor shrinks it.
pub fn decode_counted(chain: &[(Filter, Params)], data: &[u8], max_output: usize) -> Result<(Vec<u8>, usize), (FilterError, usize)> {
    run(chain, data, max_output, Some(max_output), true, false, None).map(|(v, _, spent)| (v, spent))
}

/// Like [`decode`], but on corrupt data returns what was decoded before the error and
/// reports `partial = true` (the behaviour viewers expect). The remaining filters of the
/// chain still run on the partial data. Limit and unsupported errors are still errors.
pub fn decode_tolerant(chain: &[(Filter, Params)], data: &[u8], max_output: usize) -> Result<(Vec<u8>, bool), FilterError> {
    run(chain, data, max_output, None, false, false, None).map(|(v, partial, _)| (v, partial)).map_err(|(e, _)| e)
}

/// [`decode_tolerant`] bounded and counted as [`decode_counted`] is.
pub fn decode_tolerant_counted(chain: &[(Filter, Params)], data: &[u8], max_output: usize) -> Result<(Vec<u8>, bool, usize), (FilterError, usize)> {
    run(chain, data, max_output, Some(max_output), false, false, None)
}

/// [`decode_tolerant`], spending from `work`, what decoding may still cost across a run of images:
/// a step for each byte of `data`, paid before anything is decoded, [`FLATE_READ_WORK`] for each
/// byte a Flate stage reads, paid before it runs (again if it reads them twice), and one for each
/// byte a stage decodes, so a chain that fails, or a stage that goes past `max_output`, has paid for
/// what it did. No stage decodes past what is left: a chain that `work` can't pay for spends all of
/// it and stops there, so with nothing left nothing is read. A chain that can't be decoded at all
/// costs nothing, whatever is left, so what an image's filters are is known from the error.
///
/// Unlike [`decode_tolerant`], no filters at all gives `data` itself, within `max_output`.
pub fn decode_tolerant_within(chain: &[(Filter, Params)], data: &[u8], max_output: usize, work: &mut u64) -> Result<Vec<u8>, FilterError> {
    spending(chain, data, max_output, work, false, None)
}

/// [`decode_tolerant_within`] for the samples of an image `rows` high (its `/Height`), which the
/// last stage gives. When that stage is Flate with a PNG predictor and gives exactly `rows` rows
/// without the filter-type byte each predicted row starts with (ISO 32000-2 §7.4.4.4), its writer
/// declared the predictor without applying it: the output is taken as the samples, as other
/// readers take it, and reported as the damage it is, like any other corruption tolerated here.
/// Any other output has its predictor undone as [`decode_tolerant_within`] does.
pub fn decode_image_tolerant_within(
    chain: &[(Filter, Params)],
    data: &[u8],
    max_output: usize,
    rows: u32,
    work: &mut u64,
) -> Result<Vec<u8>, FilterError> {
    spending(chain, data, max_output, work, false, usize::try_from(rows).ok())
}

/// [`decode`], spending from `work` as [`decode_tolerant_within`] does: corrupt data is an error,
/// having paid for what was decoded before it.
pub fn decode_within(chain: &[(Filter, Params)], data: &[u8], max_output: usize, work: &mut u64) -> Result<Vec<u8>, FilterError> {
    spending(chain, data, max_output, work, true, None)
}

fn spending(
    chain: &[(Filter, Params)],
    data: &[u8],
    max_output: usize,
    work: &mut u64,
    strict: bool,
    rows: Option<usize>,
) -> Result<Vec<u8>, FilterError> {
    decodable(chain)?;
    pay(work, data.len() as u64)?;
    if chain.is_empty() {
        return if data.len() > max_output { Err(FilterError::LimitExceeded(max_output)) } else { Ok(data.to_vec()) };
    }
    let left = usize::try_from(*work).unwrap_or(usize::MAX);
    match run(chain, data, max_output, Some(left), strict, true, rows) {
        Ok((out, _, decoded)) => pay(work, decoded as u64).map(|()| out),
        // The stage stopped at the lower of the two, which took as much to find out.
        Err((FilterError::LimitExceeded(_), decoded)) if left.saturating_sub(decoded) > max_output => {
            pay(work, (decoded as u64).saturating_add(max_output as u64))?;
            Err(FilterError::LimitExceeded(max_output))
        }
        // Past what is left, a stage stops, or a Flate stage doesn't start.
        Err((FilterError::LimitExceeded(_) | FilterError::Spent, _)) => {
            *work = 0;
            Err(FilterError::Spent)
        }
        Err((e, decoded)) => pay(work, decoded as u64).and(Err(e)),
    }
}

/// Longest filter chain decoded. Real streams use one to three filters; every stage may produce
/// up to the output limit, so the chain length multiplies the work.
pub const MAX_CHAIN: usize = 8;

/// The steps each byte Flate reads costs where decoding is paid for ([`decode_tolerant_within`],
/// [`decode_within`]), besides the step it was paid as input and a step for each byte it makes.
/// Inflating does work for every block it reads, whether or not the block makes anything: an empty
/// fixed-Huffman block is ten bits (RFC 1951 §3.2.6). With flate2's zlib-rs backend (zlib-rs
/// 0.6.8, release build) the slowest blocks measured, empty dynamic ones, took 21 ns a byte read
/// to make nothing, empty fixed ones 9 and fixed ones of a literal each 6, so at this price the
/// worst per byte read comes to about 7 ns a step, inside what a step of JBIG2 decoding takes.
/// (miniz_oxide 0.9.1, flate2's backend here before, took about 1.7 µs for every fixed-Huffman
/// block, empty or not: 1,383 ns a byte read on empty ones.) A stage's start isn't priced, about
/// 0.25 µs: every way to repeat one takes bytes of the file, so what starts cost is bounded by the
/// file's length. Data a real encoder makes took 0.2 to 2.3 ns a byte made, its reading included, so
/// an image pays twice its stored length more than it would unpriced.
pub const FLATE_READ_WORK: u64 = 2;

/// Whether a chain is one [`run`] takes, and if not, the error.
fn decodable(chain: &[(Filter, Params)]) -> Result<(), FilterError> {
    if chain.len() > MAX_CHAIN {
        return Err(FilterError::Unsupported(format!("a chain of {} filters", chain.len())));
    }
    match chain.iter().find(|(f, _)| !f.is_decodable()) {
        Some((f, _)) => Err(FilterError::Unsupported(f.name().to_owned())),
        None => Ok(()),
    }
}

/// The decoded data, whether it is partial and the bytes all stages decoded, or the error and
/// the bytes decoded before it. `max` bounds each stage, and `total` all of them together. With
/// `priced`, the count is steps: each Flate stage also pays [`FLATE_READ_WORK`] for each byte it
/// reads, before it runs, so a stage `total` can't pay for doesn't run
/// ([`FilterError::Spent`]). `rows` is the height of the image whose samples the last stage
/// produces, if it does ([`decode_image_tolerant_within`]).
fn run(
    chain: &[(Filter, Params)],
    data: &[u8],
    max: usize,
    total: Option<usize>,
    strict: bool,
    priced: bool,
    rows: Option<usize>,
) -> Result<(Vec<u8>, bool, usize), (FilterError, usize)> {
    decodable(chain).map_err(|e| (e, 0))?;
    let mut cur: Cow<[u8]> = Cow::Borrowed(data);
    let mut partial = false;
    let mut spent = 0usize;
    let price = usize::try_from(FLATE_READ_WORK).unwrap_or(usize::MAX);
    for (i, (filter, params)) in chain.iter().enumerate() {
        let read = if priced && matches!(filter, Filter::Flate) { price.saturating_mul(cur.len()) } else { 0 };
        let room = total.map_or(usize::MAX, |total| total.saturating_sub(spent));
        if read > room {
            return Err((FilterError::Spent, spent));
        }
        spent = spent.saturating_add(read);
        #[cfg(test)]
        STAGE_BYTES.with(|c| c.set(c.get() + cur.len()));
        let limit = total.map_or(max, |total| max.min(total.saturating_sub(spent)));
        // A second reading of the same bytes (Flate's raw retry) is paid for as the first was, and
        // makes no more than is left once it has been.
        let rows = rows.filter(|_| i + 1 == chain.len());
        let again = room.checked_sub(read.saturating_mul(2)).map(|left| max.min(left));
        let (step, work, again) = decode_one(filter, params, &cur, limit, again, rows);
        if again {
            spent = spent.saturating_add(read);
        }
        spent = spent.saturating_add(work);
        cur = Cow::Owned(match step {
            Ok(v) => v,
            Err(Failure { error: FilterError::Corrupt { .. }, partial: out, .. }) if !strict => {
                partial = true;
                out
            }
            Err(Failure { error: FilterError::LimitExceeded(_), .. }) => return Err((FilterError::LimitExceeded(max), spent)),
            Err(f) => return Err((f.error, spent)),
        });
    }
    Ok((cur.into_owned(), partial, spent))
}

/// One stage's result, the bytes its decoder produced (before any predictor), and whether it
/// read its input twice, which it does only within `again` ([`flate::decode`]); `rows` as for
/// [`run`].
fn decode_one(filter: &Filter, params: &Params, data: &[u8], max: usize, again: Option<usize>, rows: Option<usize>) -> (Step, usize, bool) {
    let once = |(step, n): (Step, usize)| (step, n, false);
    match filter {
        Filter::Flate => {
            let (step, twice) = flate::decode(data, max, again);
            let (step, n) = with_predictor(step, params, "FlateDecode", rows);
            (step, n, twice)
        }
        // LZW rows are predicted whatever their length, as the renderer reads them.
        Filter::Lzw => once(with_predictor(lzw::decode(data, params.early_change != 0, max), params, "LZWDecode", None)),
        Filter::AsciiHex => once(counted(asciihex::decode(data, max))),
        Filter::Ascii85 => once(counted(ascii85::decode(data, max))),
        Filter::RunLength => once(counted(runlength::decode(data, max))),
        other => (Err(Failure { error: FilterError::Unsupported(other.name().to_owned()), partial: Vec::new(), discarded: 0 }), 0, false),
    }
}

/// A step with the bytes it produced: its output, or its partial output and what it threw away.
fn counted(step: Step) -> (Step, usize) {
    let n = match &step {
        Ok(v) => v.len(),
        Err(f) => f.partial.len().saturating_add(f.discarded),
    };
    (step, n)
}

/// Applies the predictor to a decoder's result, including to partial output on failure. The
/// count is the decoder's output: a PNG predictor drops a tag byte per row, but the decoder
/// produced it. `rows` (an image's height, for its last stage) applies only to data that decoded
/// whole: data cut short by damage has its predictor undone as it always had.
fn with_predictor(step: Step, params: &Params, name: &'static str, rows: Option<usize>) -> (Step, usize) {
    let (step, n) = counted(step);
    let step = match step {
        Ok(v) => predictor::decode(params, v, name, rows),
        Err(Failure { error, partial, discarded }) => {
            let partial = match predictor::decode(params, partial, name, None) {
                Ok(v) => v,
                Err(f) => f.partial,
            };
            Err(Failure { error, partial, discarded })
        }
    };
    (step, n)
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
thread_local! {
    /// Bytes handed to decoding stages on this thread.
    static STAGE_BYTES: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

#[cfg(test)]
mod tests {
    #[test]
    fn a_long_filter_chain_is_refused_before_decoding() {
        // `FF FF` run-length decodes to itself, so every stage of the chain repeats the work.
        let data = vec![0xFF; 4096];
        let chain = |n: usize| vec![(super::Filter::RunLength, super::Params::default()); n];
        super::STAGE_BYTES.with(|c| c.set(0));
        let long = super::decode(&chain(1000), &data, 1 << 20);
        let read = super::STAGE_BYTES.with(|c| c.get());
        assert!(read <= 16 * data.len(), "{read} bytes decoded through the chain");
        assert!(matches!(long, Err(super::FilterError::Unsupported(_))), "{long:?}");
        assert_eq!(super::decode(&chain(super::MAX_CHAIN), &data, 1 << 20).ok(), Some(data));
    }

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
    fn raw_flate_fallback_preserves_output_limits() {
        // RFC 1951 stored blocks: the first is nonfinal with LEN=257 and
        // NLEN=!257, followed by a final empty block. Its ignored alignment
        // bits make the first two bytes also look like a valid zlib header.
        let data = vec![b'A'; 257];
        let mut raw = vec![0x78, 0x01, 0x01, 0xfe, 0xfe];
        raw.extend_from_slice(&data);
        raw.extend_from_slice(&[0x01, 0x00, 0x00, 0xff, 0xff]);
        let chain = [(Filter::Flate, Params::default())];
        for max in [0, 1, 128, 256] {
            assert!(matches!(decode(&chain, &raw, max), Err(FilterError::LimitExceeded(m)) if m == max));
            assert!(matches!(decode_tolerant(&chain, &raw, max), Err(FilterError::LimitExceeded(m)) if m == max));
        }
        for max in [257, 258] {
            assert_eq!(decode(&chain, &raw, max).unwrap(), data);
            assert_eq!(decode_tolerant(&chain, &raw, max).unwrap(), (data.clone(), false));
        }
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

    /// A counted decode bounds what all stages decode together: Flate inflating 1 MiB of hex
    /// that ASCIIHex halves is 1.5 MiB of work, past a 1.25 MiB allowance. A plain decode keeps
    /// the limit per stage.
    #[test]
    fn a_counted_decode_bounds_all_stages_together() {
        let hex: Vec<u8> = std::iter::repeat_n(b"ab".as_slice(), 1 << 19).flatten().copied().collect();
        let data = encode_flate(&hex);
        let chain = [(Filter::Flate, Params::default()), (Filter::AsciiHex, Params::default())];
        let max = (1 << 20) + (1 << 18);
        assert_eq!(decode(&chain, &data, max).map(|d| d.len()).ok(), Some(1 << 19));
        assert!(matches!(decode_counted(&chain, &data, max), Err((FilterError::LimitExceeded(m), _)) if m == max));
        assert_eq!(decode_counted(&chain, &data, 2 << 20).map(|(_, spent)| spent).ok(), Some((1 << 20) + (1 << 19)));
        let tolerant = decode_tolerant_counted(&chain, &data, 2 << 20).map(|(out, partial, spent)| (out.len(), partial, spent));
        assert_eq!(tolerant.ok(), Some((1 << 19, false, (1 << 20) + (1 << 19))));
        assert!(matches!(decode_tolerant_counted(&chain, &data, max), Err((FilterError::LimitExceeded(m), _)) if m == max));
    }

    /// A PNG predictor drops a tag byte per row, but Flate produced it: the count includes it,
    /// also when the predictor fails on a bad tag.
    #[test]
    fn a_counted_decode_counts_output_before_the_predictor() {
        let params = Params { predictor: 12, ..Params::default() };
        let rows = vec![7u8; 1 << 16];
        let data = encode(&Filter::Flate, &params, &rows).unwrap();
        let chain = [(Filter::Flate, params.clone())];
        let (out, spent) = decode_counted(&chain, &data, usize::MAX).unwrap();
        assert!(out == rows);
        assert_eq!(spent, 2 << 16);
        let bad: Vec<u8> = std::iter::repeat_n([0u8, 7], 1 << 15).flatten().chain([9, 7]).collect();
        let (e, spent) = decode_counted(&chain, &encode_flate(&bad), usize::MAX).unwrap_err();
        assert!(matches!(e, FilterError::Corrupt { .. }), "{e}");
        assert_eq!(spent, bad.len());
    }

    fn refused(e: &FilterError) -> bool {
        matches!(e, FilterError::Spent)
    }

    /// A chain pays for what it reads and what each stage decodes, down to the byte: Flate inflates
    /// hex that ASCIIHex halves, so a stage counts when a later one shrinks it.
    #[test]
    fn a_chain_spends_down_to_the_byte() {
        let hex = b"ab".repeat(1 << 10);
        let data = encode_flate(&hex);
        let chain = [(Filter::Flate, Params::default()), (Filter::AsciiHex, Params::default())];
        let whole = decode(&chain, &data, 1 << 20).unwrap();
        let cost = (data.len() + hex.len() + whole.len()) as u64 + FLATE_READ_WORK * data.len() as u64;
        let mut work = u64::MAX;
        assert_eq!(decode_tolerant_within(&chain, &data, 1 << 20, &mut work).unwrap(), whole);
        assert_eq!(u64::MAX - work, cost);
        let mut work = cost;
        assert_eq!(decode_tolerant_within(&chain, &data, 1 << 20, &mut work).unwrap(), whole, "exactly enough");
        assert_eq!(work, 0);
        let mut work = cost + 7;
        decode_tolerant_within(&chain, &data, 1 << 20, &mut work).unwrap();
        assert_eq!(work, 7, "only what it took");
        let mut work = cost - 1;
        assert!(refused(&decode_tolerant_within(&chain, &data, 1 << 20, &mut work).unwrap_err()), "a step short");
        assert_eq!(work, 0, "spends all that was left");
        // Each stage has `max_output` to itself, as in a plain decode: together they are past it.
        let mut work = u64::MAX;
        assert_eq!(decode_tolerant_within(&chain, &data, hex.len(), &mut work).unwrap(), whole);
    }

    /// No stage runs past what is left: the first stage of this bomb stops there, so the second
    /// isn't given anything to read, and with too little to read it nothing runs at all.
    #[test]
    fn a_chain_stops_at_what_is_left() {
        let inner = encode_flate(&vec![0; 1 << 20]);
        let bomb = encode_flate(&inner);
        let chain = [(Filter::Flate, Params::default()), (Filter::Flate, Params::default())];
        let read = |work: u64| {
            super::STAGE_BYTES.with(|c| c.set(0));
            let mut work = work;
            assert!(refused(&decode_tolerant_within(&chain, &bomb, usize::MAX, &mut work).unwrap_err()));
            assert_eq!(work, 0);
            super::STAGE_BYTES.with(|c| c.get())
        };
        let paid = |n: usize| (1 + FLATE_READ_WORK) * n as u64;
        assert_eq!(read(paid(bomb.len()) + paid(inner.len()) + 10), bomb.len() + inner.len(), "the second stage stops");
        assert_eq!(read(paid(bomb.len()) + FLATE_READ_WORK * inner.len() as u64 - 1), bomb.len(), "the second can't pay to read");
        assert_eq!(read(paid(bomb.len()) + 10), bomb.len(), "the first stops, and the second never runs");
        assert_eq!(read(paid(bomb.len()) - 1), 0, "the first can't pay to read");
        assert_eq!(read(bomb.len() as u64 - 1), 0, "it can't pay to be read");
    }

    /// Past `max_output` the chain has failed, and hasn't run out of anything: it pays what it took
    /// to find out, and what is left stays. Whichever limit a stage stops at is the one reported.
    #[test]
    fn a_stage_past_its_limit_pays_for_finding_out() {
        let data = encode_flate(&vec![0; 1 << 16]);
        let chain = [(Filter::Flate, Params::default())];
        let mut work = u64::MAX;
        assert!(matches!(decode_tolerant_within(&chain, &data, 1000, &mut work), Err(FilterError::LimitExceeded(1000))));
        let read = |n: usize| FLATE_READ_WORK * n as u64;
        assert_eq!(u64::MAX - work, data.len() as u64 + read(data.len()) + 1000);
        // The later stage of two, which pays for the first as well.
        let bomb = encode_flate(&data);
        let two = [(Filter::Flate, Params::default()), (Filter::Flate, Params::default())];
        let mut work = u64::MAX;
        assert!(matches!(decode_tolerant_within(&two, &bomb, data.len(), &mut work), Err(FilterError::LimitExceeded(_))));
        assert_eq!(u64::MAX - work, (bomb.len() + 2 * data.len()) as u64 + read(bomb.len()) + read(data.len()));
        // Both limits at once: a run that can't go on is what a caller needs to hear.
        let mut work = data.len() as u64 + read(data.len()) + 1000;
        assert!(refused(&decode_tolerant_within(&chain, &data, 1000, &mut work).unwrap_err()));
        assert_eq!(work, 0);
        let mut work = data.len() as u64 + read(data.len()) + 1001;
        assert!(matches!(decode_tolerant_within(&chain, &data, 1000, &mut work), Err(FilterError::LimitExceeded(1000))));
        assert_eq!(work, 1, "the image's limit, with a step to spare");
    }

    /// What a damaged stream decoded is paid for, however the decode ends.
    #[test]
    fn a_chain_that_fails_has_paid_for_what_it_did() {
        let data: Vec<u8> = (0..20_000u32).map(|i| (i % 97) as u8 ^ (i / 300) as u8).collect();
        let mut cut = encode_flate(&data);
        cut.truncate(cut.len() / 2);
        let chain = [(Filter::Flate, Params::default())];
        let (partial, _) = decode_tolerant(&chain, &cut, 1 << 20).unwrap();
        assert!(!partial.is_empty());
        let mut work = u64::MAX;
        assert_eq!(decode_tolerant_within(&chain, &cut, 1 << 20, &mut work).unwrap(), partial);
        let read = |n: usize| FLATE_READ_WORK * n as u64;
        assert_eq!(u64::MAX - work, (cut.len() + partial.len()) as u64 + read(cut.len()));
        // Read strictly it is an error, for the same work.
        let mut work = u64::MAX;
        assert!(matches!(decode_within(&chain, &cut, 1 << 20, &mut work), Err(FilterError::Corrupt { .. })));
        assert_eq!(u64::MAX - work, (cut.len() + partial.len()) as u64 + read(cut.len()));
        let whole = encode_flate(&data);
        let mut work = u64::MAX;
        assert_eq!(decode_within(&chain, &whole, 1 << 20, &mut work).unwrap(), data);
        assert_eq!(u64::MAX - work, (whole.len() + data.len()) as u64 + read(whole.len()));
        // A predictor that comes to nothing, after all of this was inflated for it.
        let p = Params { predictor: 99, ..Params::default() };
        let data = encode_flate(&vec![0; 1 << 16]);
        let mut work = u64::MAX;
        assert!(decode_tolerant_within(&[(Filter::Flate, p)], &data, 1 << 20, &mut work).unwrap().is_empty());
        assert_eq!(u64::MAX - work, data.len() as u64 + read(data.len()) + (1 << 16));
    }

    /// A stream that reads as zlib only until it fails, and as raw deflate until that fails too,
    /// gives back no data, and has still decoded everything the retry did.
    #[test]
    fn a_retry_that_fails_pays_for_what_it_decoded() {
        // Stored as raw deflate: this many bytes, then a block of a reserved type. As zlib: the
        // same reserved type at once, so the first reading fails with nothing.
        let mut data = vec![0x78, 0x01, 0x06, 0xFE, 0xF9];
        data.extend(vec![0u8; 0x0601]);
        data.push(0x07);
        let chain = [(Filter::Flate, Params::default())];
        let (out, partial) = decode_tolerant(&chain, &data, 1 << 20).unwrap();
        assert!(out.is_empty() && partial, "what is read stays as it was");
        let (_, _, spent) = decode_tolerant_counted(&chain, &data, 1 << 20).unwrap();
        assert_eq!(spent, 0x0601);
        let (e, spent) = decode_counted(&chain, &data, 1 << 20).unwrap_err();
        assert!(matches!(e, FilterError::Corrupt { .. }), "{e}");
        assert_eq!(spent, 0x0601);
        let mut work = u64::MAX;
        assert!(decode_tolerant_within(&chain, &data, 1 << 20, &mut work).unwrap().is_empty());
        // Read twice: as zlib, then as raw deflate.
        assert_eq!(u64::MAX - work, (data.len() + 0x0601) as u64 + 2 * FLATE_READ_WORK * data.len() as u64);
        // Through a predictor, which would otherwise lose the count.
        let p = Params { predictor: 12, columns: 4, ..Params::default() };
        let (_, _, spent) = decode_tolerant_counted(&[(Filter::Flate, p)], &data, 1 << 20).unwrap();
        assert_eq!(spent, 0x0601);
    }

    /// Inflating does work for every block it reads, whether or not the block makes anything: empty
    /// fixed-Huffman blocks, ten bits each, make nothing however many there are. Each byte of them
    /// is paid for at Flate's price before any is read.
    #[test]
    fn a_flate_stage_pays_for_what_it_reads() {
        // Four empty blocks in five bytes, as raw deflate, then an empty final block.
        let mut hollow = [0x02, 0x08, 0x20, 0x80, 0x00].repeat(1 << 12);
        hollow.extend([0x03, 0x00]);
        let chain = [(Filter::Flate, Params::default())];
        assert_eq!(decode(&chain, &hollow, 1 << 20).unwrap(), b"");
        let cost = (1 + FLATE_READ_WORK) * hollow.len() as u64;
        let mut work = u64::MAX;
        assert_eq!(decode_tolerant_within(&chain, &hollow, 1 << 20, &mut work).unwrap(), b"");
        assert_eq!(u64::MAX - work, cost);
        let mut work = cost;
        assert!(decode_tolerant_within(&chain, &hollow, 1 << 20, &mut work).is_ok(), "exactly enough");
        assert_eq!(work, 0);
        super::STAGE_BYTES.with(|c| c.set(0));
        let mut work = cost - 1;
        assert!(refused(&decode_tolerant_within(&chain, &hollow, 1 << 20, &mut work).unwrap_err()));
        assert_eq!((work, super::STAGE_BYTES.with(|c| c.get())), (0, 0), "nothing was inflated");
        // Counted for a byte allowance, as a page's content is, reading costs nothing.
        assert_eq!(decode_counted(&chain, &hollow, 1 << 20).unwrap(), (Vec::new(), 0));
    }

    /// A zlib header that only looked valid is read again as raw deflate, and the second reading
    /// is paid for as the first was. One that can't be paid for is refused, not read as damaged.
    #[test]
    fn a_second_reading_is_paid_for() {
        // As zlib: a block of a reserved type at once. As raw deflate: a stored block of this many
        // zero bytes, then an empty final block.
        let mut data = vec![0x78, 0x01, 0x06, 0xFE, 0xF9];
        data.extend(vec![0u8; 0x0601]);
        data.extend([0x03, 0x00]);
        let chain = [(Filter::Flate, Params::default())];
        assert_eq!(decode(&chain, &data, 1 << 20).unwrap(), vec![0u8; 0x0601]);
        let read = FLATE_READ_WORK * data.len() as u64;
        let mut work = (data.len() + 0x0601) as u64 + 2 * read;
        assert_eq!(decode_tolerant_within(&chain, &data, 1 << 20, &mut work).unwrap(), vec![0u8; 0x0601]);
        assert_eq!(work, 0, "exactly enough");
        // Enough to read it once and not twice: it isn't read again.
        super::flate::INFLATED.with(|c| c.set(0));
        let mut work = data.len() as u64 + 2 * read - 1;
        assert!(refused(&decode_tolerant_within(&chain, &data, 1 << 20, &mut work).unwrap_err()));
        assert_eq!((work, super::flate::INFLATED.with(|c| c.get())), (0, 0));
        // Enough to read it twice, and a byte short of what it makes: the second reading makes no
        // more than is left once it has been paid for.
        super::flate::INFLATED.with(|c| c.set(0));
        let mut work = (data.len() + 0x0601) as u64 + 2 * read - 1;
        assert!(refused(&decode_tolerant_within(&chain, &data, 1 << 20, &mut work).unwrap_err()));
        assert!(super::flate::INFLATED.with(|c| c.get()) < 0x0601, "it stopped short");
    }

    /// With nothing left nothing is read. What can't be decoded at all never costs anything, and
    /// says so whatever is left. Without filters, what is read is the data.
    #[test]
    fn what_a_chain_is_shows_whatever_is_left() {
        let flate = [(Filter::Flate, Params::default())];
        let mut work = 0;
        assert!(refused(&decode_tolerant_within(&flate, b"garbage", 1 << 20, &mut work).unwrap_err()));
        let long = vec![(Filter::Flate, Params::default()); MAX_CHAIN + 1];
        for chain in [vec![(Filter::Dct, Params::default())], vec![(Filter::Jbig2, Params::default())], long] {
            for left in [0, 100] {
                let mut work = left;
                let e = decode_tolerant_within(&chain, b"data", 1 << 20, &mut work).unwrap_err();
                assert!(matches!(e, FilterError::Unsupported(_)) && !refused(&e), "{e}");
                assert_eq!(work, left);
            }
        }
        let mut work = 100;
        assert_eq!(decode_tolerant_within(&[], b"data", 4, &mut work).unwrap(), b"data");
        assert_eq!(work, 96);
        assert!(matches!(decode_tolerant_within(&[], b"data", 3, &mut work), Err(FilterError::LimitExceeded(3))));
        assert_eq!(work, 92);
        let mut work = 3;
        assert!(refused(&decode_tolerant_within(&[], b"data", 4, &mut work).unwrap_err()));
        assert_eq!(work, 0);
    }

    /// Four rows of three RGB pixels, every sample above 4 (none reads as a PNG filter type).
    fn rgb_rows() -> (Params, Vec<u8>) {
        let p = Params { predictor: 15, colors: 3, columns: 3, ..Params::default() };
        (p, (0..36u8).map(|i| 200 - i).collect())
    }

    #[test]
    fn an_image_whose_rows_carry_no_png_tags_is_read_as_its_samples_and_reported() {
        let (p, samples) = rgb_rows();
        let chain = [(Filter::Flate, p.clone())];
        let data = encode_flate(&samples);
        let mut work = u64::MAX;
        assert_eq!(decode_image_tolerant_within(&chain, &data, 1 << 20, 4, &mut work).unwrap(), samples);
        // The repair is reported as the damage it is: tolerant decoding says so, and strict refuses.
        let (out, partial, _) = run(&chain, &data, 1 << 20, None, false, false, Some(4)).map_err(|(e, _)| e).unwrap();
        assert_eq!((out, partial), (samples.clone(), true));
        let strict = run(&chain, &data, 1 << 20, None, true, false, Some(4)).map(|_| ()).map_err(|(e, _)| e.to_string());
        assert!(strict.as_ref().is_err_and(|e| e.contains("carry no filter-type bytes")), "{strict:?}");
        // The same data that isn't known to be an image's rows still goes through the predictor.
        let mut work = u64::MAX;
        assert_ne!(decode_tolerant_within(&chain, &data, 1 << 20, &mut work).unwrap(), samples);
        // LZW is read as before, as the renderer reads it.
        let lzw = [(Filter::Lzw, p)];
        let data = encode(&Filter::Lzw, &Params::default(), &samples).unwrap();
        let mut plain = u64::MAX;
        assert_eq!(
            decode_image_tolerant_within(&lzw, &data, 1 << 20, 4, &mut work).ok(),
            decode_tolerant_within(&lzw, &data, 1 << 20, &mut plain).ok()
        );
    }

    #[test]
    fn an_image_whose_rows_carry_png_tags_has_its_predictor_undone() {
        let (p, samples) = rgb_rows();
        let chain = [(Filter::Flate, p.clone())];
        let data = encode(&Filter::Flate, &p, &samples).unwrap();
        let (mut a, mut b) = (u64::MAX, u64::MAX);
        assert_eq!(decode_image_tolerant_within(&chain, &data, 1 << 20, 4, &mut a).unwrap(), samples);
        assert_eq!(decode_tolerant_within(&chain, &data, 1 << 20, &mut b).unwrap(), samples);
        assert_eq!(a, b, "the same work");
        let (_, partial, _) = run(&chain, &data, 1 << 20, None, false, false, Some(4)).map_err(|(e, _)| e).unwrap();
        assert!(!partial);
    }

    #[test]
    fn image_data_of_neither_length_decodes_as_it_did() {
        let (p, _) = rgb_rows();
        let chain = [(Filter::Flate, p.clone())];
        for len in [0, 1, 9, 27, 35, 37, 39, 40, 41, 72] {
            let data = encode_flate(&(0..len).map(|i| (i * 7 % 5) as u8).collect::<Vec<_>>());
            let (mut a, mut b) = (u64::MAX, u64::MAX);
            let image = decode_image_tolerant_within(&chain, &data, 1 << 20, 4, &mut a).map_err(|e| e.to_string());
            let plain = decode_tolerant_within(&chain, &data, 1 << 20, &mut b).map_err(|e| e.to_string());
            assert_eq!(image, plain, "{len} bytes");
        }
        // Rows of the untagged length for another height, or no height.
        let samples = encode_flate(&[9; 36]);
        for rows in [0, 3, 5, u32::MAX] {
            let (mut a, mut b) = (u64::MAX, u64::MAX);
            assert_eq!(
                decode_image_tolerant_within(&chain, &samples, 1 << 20, rows, &mut a).ok(),
                decode_tolerant_within(&chain, &samples, 1 << 20, &mut b).ok(),
                "{rows} rows"
            );
        }
        // Only the last stage produces the rows: an earlier one is predicted as before.
        let hex: Vec<u8> = [9u8; 36].iter().flat_map(|b| format!("{b:02x}").into_bytes()).collect();
        let earlier = [(Filter::Flate, p.clone()), (Filter::AsciiHex, Params::default())];
        let data = encode_flate(&hex[..36]);
        let (mut a, mut b) = (u64::MAX, u64::MAX);
        assert_eq!(
            decode_image_tolerant_within(&earlier, &data, 1 << 20, 4, &mut a).ok(),
            decode_tolerant_within(&earlier, &data, 1 << 20, &mut b).ok()
        );
        // TIFF predictor 2 and no predictor are left alone.
        for predictor in [1, 2] {
            let chain = [(Filter::Flate, Params { predictor, ..p.clone() })];
            let (mut a, mut b) = (u64::MAX, u64::MAX);
            assert_eq!(
                decode_image_tolerant_within(&chain, &samples, 1 << 20, 4, &mut a).ok(),
                decode_tolerant_within(&chain, &samples, 1 << 20, &mut b).ok()
            );
        }
    }

    #[test]
    fn image_rows_of_no_bytes_or_past_memory_do_not_panic() {
        for (colors, bits_per_component, columns) in [(0, 8, 3), (3, 8, 0), (3, 8, -1), (255, 16, i64::MAX), (1, 1, i64::MAX), (3, 3, 3)] {
            let chain = [(Filter::Flate, Params { predictor: 15, colors, bits_per_component, columns, early_change: 1 })];
            for len in [0, 1, 36] {
                let data = encode_flate(&vec![7; len]);
                for rows in [0, 1, 4, u32::MAX] {
                    let mut work = u64::MAX;
                    let _ = decode_image_tolerant_within(&chain, &data, 1 << 20, rows, &mut work);
                }
            }
        }
    }
}
