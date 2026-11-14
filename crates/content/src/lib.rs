//! Content streams (ISO 32000-2 §7.8.2, §8, §9): parse a stream into operators that keep their
//! source spans, write operators back, and the matrix arithmetic content interpretation needs.
//!
//! Layer L2. The parser is tolerant: bytes it cannot read are skipped (and counted in
//! [`Parsed::skipped`]) instead of failing the whole stream, and it never panics. Inline images
//! (`BI … ID … EI`) are kept whole, so a parse → serialize round trip preserves them.

#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

use pdfcraft_cos::{Dict, Lexer, Object, PdfString, serialize};

#[cfg(test)]
mod tests;

/// One operator with its operands.
#[derive(Clone, Debug, PartialEq)]
pub struct Op {
    /// The operator keyword (`Tj`, `re`, `BI` for inline images…).
    pub op: Vec<u8>,
    pub operands: Vec<Object>,
    /// Inline images: the image dictionary (`operands` is empty) and the data between `ID` and
    /// `EI`.
    pub inline: Option<(Dict, Vec<u8>)>,
    /// The bytes of the source stream this operator came from (operands included).
    pub span: std::ops::Range<usize>,
}

impl Op {
    pub fn new(op: &str, operands: Vec<Object>) -> Self {
        Op { op: op.as_bytes().to_vec(), operands, inline: None, span: 0..0 }
    }

    pub fn is(&self, op: &str) -> bool {
        self.op == op.as_bytes()
    }

    /// Operand `i` as a number.
    pub fn num(&self, i: usize) -> Option<f64> {
        self.operands.get(i).and_then(Object::as_f64)
    }

    /// The last `n` operands as numbers (operators take their operands from the end).
    pub fn nums<const N: usize>(&self) -> Option<[f64; N]> {
        let k = self.operands.len().checked_sub(N)?;
        let mut out = [0.0; N];
        for (o, v) in out.iter_mut().zip(&self.operands[k..]) {
            *o = v.as_f64()?;
        }
        Some(out)
    }

    pub fn name(&self, i: usize) -> Option<&[u8]> {
        self.operands.get(i).and_then(Object::as_name)
    }

    /// The last operand as a name: the one `Do` and the other one-operand operators act on, since
    /// the renderer takes operands from the end and ignores any surplus before them.
    pub fn last_name(&self) -> Option<&[u8]> {
        self.operands.last().and_then(Object::as_name)
    }

    /// Operand `k` counted from the end (0 is the last), as the renderer reads an operator's
    /// operands: `(x) /OC /MC0 BDC` marks with `/OC /MC0`, and `(x) /F1 10 Tf` selects `/F1`.
    pub fn back(&self, k: usize) -> Option<&Object> {
        let i = self.operands.len().checked_sub(k)?.checked_sub(1)?;
        self.operands.get(i)
    }

    /// [`Op::back`] as a name.
    pub fn name_back(&self, k: usize) -> Option<&[u8]> {
        self.back(k).and_then(Object::as_name)
    }

    /// [`Op::back`] as a number.
    pub fn num_back(&self, k: usize) -> Option<f64> {
        self.back(k).and_then(Object::as_f64)
    }
}

/// A parsed stream.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Parsed {
    pub ops: Vec<Op>,
    /// Bytes that could not be read as operands or operators.
    pub skipped: usize,
    /// Inline images the renderer's tokenizer reads no image for (it stops, or finds no `EI` it
    /// accepts), so their end here is a guess and the operators after them may not be the ones
    /// the page draws; and images it reads whose parameters this parser can't, which are missing
    /// from `ops` although the page draws them.
    pub ambiguous_images: usize,
    /// For [`Pieces::parse`]: per piece, whether a comment runs into it from the piece before
    /// and whether one runs out of it into the next. Only boundaries are recorded, so the size
    /// depends on the number of pieces, not of comments.
    comment_edges: Vec<(bool, bool)>,
}

fn is_ws(b: u8) -> bool {
    matches!(b, b'\0' | b'\t' | b'\n' | b'\x0C' | b'\r' | b' ')
}

fn is_delim(b: u8) -> bool {
    matches!(b, b'(' | b')' | b'<' | b'>' | b'[' | b']' | b'{' | b'}' | b'/' | b'%')
}

/// Longest unreadable number token retried from its next byte. Real numbers are far shorter.
const MAX_RETRIED_TOKEN: usize = 64;

/// Parse a (decoded) content stream. Its inline images aren't decoded: one behind ASCIIHex or
/// ASCII85 and a second filter whose text holds an `EI` before the renderer's counts in
/// [`Parsed::ambiguous_images`], where [`parse_within`] can tell more.
pub fn parse(data: &[u8]) -> Parsed {
    parse_pieces(data, &[], &mut ImageDecoding::none())
}

/// [`parse`], its inline images behind ASCIIHex or ASCII85 and a second filter decoded within
/// `decoding` to tell where a reader that decodes them stops, for a caller that reads
/// [`Parsed::ambiguous_images`]. A caller shares one between the parses of one operation: content
/// parsed again, as a form drawn again is, then decodes its images again within what is left, not
/// within an allowance of its own.
pub fn parse_within(data: &[u8], decoding: &mut ImageDecoding) -> Parsed {
    parse_pieces(data, &[], decoding)
}

/// What decoding inline images may still give, to tell where a reader that decodes them stops
/// ([`MAX_IMAGE_DECODING`] to begin with): their ASCIIHex or ASCII85 text and what inflating that
/// gives. Past it, where such a reader stops isn't told.
pub struct ImageDecoding {
    left: usize,
}

impl Default for ImageDecoding {
    fn default() -> Self {
        ImageDecoding { left: MAX_IMAGE_DECODING }
    }
}

impl ImageDecoding {
    /// Nothing to decode with, for the parses that don't read the count.
    fn none() -> Self {
        ImageDecoding { left: 0 }
    }
}

/// The operators of a (decoded) content stream, read one at a time, as [`parse`] reads them,
/// decoding no inline image. A caller that stops early or keeps only a summary never holds every
/// operator of the stream.
pub fn ops(data: &[u8]) -> Ops<'_> {
    Ops::new(data, &[])
}

/// Parse `data`, recording which of the piece boundaries `ends` (as in [`Pieces`]) a comment
/// crosses, its inline images decoded within `decoding`.
fn parse_pieces(data: &[u8], ends: &[usize], decoding: &mut ImageDecoding) -> Parsed {
    let mut reader = Ops::new(data, ends);
    reader.image_ends.decoding = std::mem::take(decoding);
    let ops = reader.by_ref().collect();
    *decoding = std::mem::take(&mut reader.image_ends.decoding);
    Parsed { ops, skipped: reader.skipped, ambiguous_images: reader.ambiguous_images, comment_edges: reader.comment_edges }
}

/// The iterator [`ops`] returns.
pub struct Ops<'a> {
    data: &'a [u8],
    lx: Lexer<'a>,
    operands: Vec<Object>,
    start: Option<usize>,
    skipped: usize,
    ambiguous_images: usize,
    ends: &'a [usize],
    boundary: usize,
    comment_edges: Vec<(bool, bool)>,
    /// What the inline images so far found ahead of them, for the ones after.
    image_ends: ImageEnds,
}

impl<'a> Ops<'a> {
    fn new(data: &'a [u8], ends: &'a [usize]) -> Self {
        Ops {
            data,
            lx: Lexer::new(data, 0),
            operands: Vec::new(),
            start: None,
            skipped: 0,
            ambiguous_images: 0,
            ends,
            boundary: 0,
            comment_edges: vec![(false, false); ends.len()],
            image_ends: ImageEnds { decoding: ImageDecoding::none(), ..ImageEnds::default() },
        }
    }

    /// Bytes read so far that could not be read as operands or operators.
    pub fn skipped(&self) -> usize {
        self.skipped
    }

    /// Inline images read so far that are ambiguous (as in [`Parsed::ambiguous_images`]): either
    /// their end is this parser's guess, or the renderer reads them and this parser cannot.
    pub fn ambiguous_images(&self) -> usize {
        self.ambiguous_images
    }
}

impl Iterator for Ops<'_> {
    type Item = Op;

    fn next(&mut self) -> Option<Op> {
        let Ops { data, lx, operands, start, skipped, ambiguous_images, ends, boundary, comment_edges: edges, image_ends } = self;
        let (data, ends): (&[u8], &[usize]) = (data, ends);
        loop {
            let trivia = lx.pos;
            lx.skip_ws();
            // Only the whitespace the lexer skips here holds top-level comments: a `%` inside a
            // string, a name, an array or dictionary operand or inline image data is not one.
            if *boundary < ends.len() {
                comment_edges(data, trivia..lx.pos, ends, boundary, edges);
            }
            let Some(&b) = data.get(lx.pos) else {
                // Operands left without an operator.
                *skipped += operands.len();
                operands.clear();
                return None;
            };
            let at = lx.pos;
            let operand_start = matches!(b, b'/' | b'(' | b'<' | b'[' | b'+' | b'-' | b'.') || b.is_ascii_digit();
            if operand_start {
                match lx.object() {
                    Ok(o) => {
                        start.get_or_insert(at);
                        operands.push(o);
                    }
                    Err(_) if b == b'(' => {
                        // An unterminated string runs to the end of the stream.
                        *skipped += data.len() - at;
                        lx.pos = data.len();
                    }
                    Err(_) => {
                        let read = lx.pos.saturating_sub(at);
                        #[cfg(test)]
                        tests::RESCANNED.with(|c| c.set(c.get() + read));
                        let number = matches!(b, b'+' | b'-' | b'.') || b.is_ascii_digit();
                        if number && read > MAX_RETRIED_TOKEN {
                            // A long unreadable number is skipped whole: retrying from each of its
                            // bytes would read the rest of it again every time.
                            *skipped += read;
                        } else {
                            // Skip the offending byte and carry on.
                            lx.pos = at + 1;
                            *skipped += 1;
                        }
                    }
                }
                continue;
            }
            if is_delim(b) {
                // A stray `)`, `>`, `]`, `{`, `}`.
                lx.pos += 1;
                *skipped += 1;
                continue;
            }
            let kw = lx.token();
            if kw.is_empty() {
                lx.pos = at + 1;
                *skipped += 1;
                continue;
            }
            match kw {
                b"true" => {
                    start.get_or_insert(at);
                    operands.push(Object::Bool(true));
                }
                b"false" => {
                    start.get_or_insert(at);
                    operands.push(Object::Bool(false));
                }
                b"null" => {
                    start.get_or_insert(at);
                    operands.push(Object::Null);
                }
                b"BI" => {
                    let image = inline_image(data, at, lx.pos, image_ends);
                    *ambiguous_images += usize::from(image.ambiguous);
                    let end = image.end;
                    lx.pos = end;
                    operands.clear();
                    match image.image {
                        Some((dict, img)) => {
                            return Some(Op {
                                op: b"BI".to_vec(),
                                operands: Vec::new(),
                                inline: Some((dict, img)),
                                span: start.take().unwrap_or(at)..end,
                            });
                        }
                        None => {
                            *skipped += end.saturating_sub(at);
                            *start = None;
                        }
                    }
                }
                _ => {
                    let s = start.take().unwrap_or(at);
                    return Some(Op { op: kw.to_vec(), operands: std::mem::take(operands), inline: None, span: s..lx.pos });
                }
            }
        }
    }
}

/// Record the piece boundaries crossed by the comments in `trivia` (skipped whitespace and
/// comments). `boundary` is the first boundary not yet passed; boundaries only move forward.
fn comment_edges(data: &[u8], trivia: std::ops::Range<usize>, ends: &[usize], boundary: &mut usize, edges: &mut [(bool, bool)]) {
    let mut at = trivia.start;
    while at < trivia.end {
        if data.get(at) != Some(&b'%') {
            at += 1;
            continue;
        }
        let comment = at;
        while at < trivia.end && !matches!(data.get(at), Some(b'\r' | b'\n')) {
            at += 1;
        }
        // The separator of piece `boundary` is the last byte before its end.
        while let Some(&end) = ends.get(*boundary) {
            let separator = end.saturating_sub(1);
            if separator >= at {
                break;
            }
            if separator >= comment {
                if let Some(edge) = edges.get_mut(*boundary) {
                    edge.1 = true;
                }
                // Includes a comment whose end of line is the next piece's first byte.
                if let Some(edge) = edges.get_mut(boundary.saturating_add(1)) {
                    edge.0 = true;
                }
            }
            *boundary += 1;
        }
    }
}

/// An inline image as [`inline_image`] reads it.
struct Inline {
    /// Its parameters and data; `None` when the parameters can't be read.
    image: Option<(Dict, Vec<u8>)>,
    /// The position after its `EI` (or the end of the stream).
    end: usize,
    /// The renderer's tokenizer reads no image here, so `end` is this parser's guess; or it reads
    /// one whose parameters this parser can't, so `image` is `None` for an image the page draws.
    ambiguous: bool,
}

/// Read the inline image whose `BI` keyword starts at `bi` (its parameters start at `pos`).
///
/// Filtered image data has no length, and `EI` can occur inside it, so where it ends is a
/// heuristic choice among `EI` candidates. The renderer's choice decides which operators the
/// page draws after the image, and redaction must see the same ones, so the end is taken from
/// the renderer's tokenizer (hayro-syntax) rather than worked out here by another rule. Only
/// when that tokenizer reads no image does this parser fall back to its own rules, and the
/// image is marked ambiguous. It is marked so too when another reader may end it elsewhere
/// ([`ends_elsewhere`]), or when a key its end goes by is given twice, differently ([`END_KEYS`]).
fn inline_image(data: &[u8], bi: usize, pos: usize, scan: &mut ImageEnds) -> Inline {
    let mut lx = Lexer::new(data, pos);
    let mut dict = Dict::new();
    let mut readable = true;
    // A key the image's end goes by, given twice with different values (`END_KEYS`).
    let mut repeated = false;
    let mut stop = pos;
    loop {
        lx.skip_ws();
        if lx.eat_keyword(b"ID") {
            break;
        }
        let at = lx.pos;
        let entry = match lx.object() {
            Ok(Object::Name(k)) => lx.object().ok().map(|v| (k, v)),
            _ => None,
        };
        let Some((k, v)) = entry else {
            // A stream that ends inside the parameters is cut short, not unreadable: keep what was read.
            if at >= data.len() {
                return Inline { image: Some((dict, Vec::new())), end: data.len(), ambiguous: false };
            }
            readable = false;
            stop = at;
            break;
        };
        repeated |= END_KEYS.iter().any(|(long, short)| k == *long || k == *short) && dict.get(&k).is_some_and(|old| *old != v);
        dict.set(k, v);
    }
    if let Some((start, end_data, end)) = renderer_image_end(data, bi) {
        // Where the renderer ends the data at an `EI` found by its heuristic, one whitespace byte
        // before that `EI` separates it from the data.
        let mut samples = data.get(start..end_data).unwrap_or_default();
        if data.get(end_data..).is_some_and(|rest| rest.starts_with(b"EI"))
            && let Some((last, rest)) = samples.split_last()
            && is_ws(*last)
        {
            samples = rest;
        }
        // The renderer draws an image whose parameters this parser can't read: it is not
        // represented here, so a redaction can't inspect or remove it and must not trust the page.
        // Nor when a reader that decodes the data may end the image at another `EI`: it reads as
        // operators what the renderer reads as data, or the other way round. Nor when a key its end
        // goes by is given twice, differently: readers keep either value.
        let ambiguous = !readable || repeated || ends_elsewhere(data, (lx.pos + 1).min(data.len()), end.saturating_sub(2), &dict, scan);
        return Inline { image: readable.then(|| (dict, samples.to_vec())), end, ambiguous };
    }
    if !readable {
        // Without an `EI` after it, the image runs to the end of the stream.
        return Inline { image: None, end: end_of_image(data, stop, b"").map_or(data.len(), |(_, end)| end), ambiguous: true };
    }
    // One whitespace byte separates ID from the data.
    let begin = (lx.pos + 1).min(data.len());
    let end = unfiltered_image_end(data, begin, &dict)
        .or_else(|| declared_image_end(data, begin, &dict))
        .or_else(|| image_data_end(data, begin, filter_eod(&dict)));
    let (image, end) = match end {
        Some((end_data, end)) => (data.get(begin..end_data).unwrap_or_default().to_vec(), end),
        None => (data.get(begin..).unwrap_or_default().to_vec(), data.len()),
    };
    Inline { image: Some((dict, image)), end, ambiguous: true }
}

/// The first `needle` at or after a position, remembered. Inline images come in order, so a later
/// query can often use an earlier answer, and each needle's search reads the stream about once
/// however many images it holds.
#[derive(Default)]
struct NextOf {
    /// The last query and its answer: where, or none at or after it.
    last: Option<(usize, Option<usize>)>,
}

impl NextOf {
    fn at(&mut self, data: &[u8], needle: &[u8], from: usize) -> Option<usize> {
        if let Some((queried, found)) = self.last
            && queried <= from
        {
            match found {
                None => return None,
                Some(at) if at >= from => return Some(at),
                Some(_) => {}
            }
        }
        let rest = data.get(from..).unwrap_or_default();
        let found = rest.windows(needle.len()).position(|w| w == needle).and_then(|i| from.checked_add(i));
        #[cfg(test)]
        tests::EOD_SEARCHED.with(|c| c.set(c.get() + found.map_or(rest.len(), |at| at - from + needle.len())));
        self.last = Some((from, found));
        found
    }
}

/// The searches a stream's inline images share: for `EI`, and for the ASCIIHex and ASCII85
/// end-of-data markers; and what decoding their data may still give.
#[derive(Default)]
pub(crate) struct ImageEnds {
    ei: NextOf,
    hex_end: NextOf,
    a85_end: NextOf,
    decoding: ImageDecoding,
}

/// What decoding the data of inline images may give in all, their ASCIIHex or ASCII85 text and what
/// inflating that gives, to tell where a reader that decodes them stops ([`chain_stop`]): for a
/// stream, or for all those parsed with one [`ImageDecoding`]. Past it, where it stops isn't told.
pub const MAX_IMAGE_DECODING: usize = 64 << 20;

/// An inline image's outer filter, as it decides where a decoding reader stops reading the data.
#[derive(Clone, Copy, PartialEq)]
enum Outer {
    None,
    Hex,
    A85,
    Other,
    /// A filter no reader decodes: not a name, or not one of an inline image's filters.
    Unknown,
}

/// The filters an inline image may name, by their names and abbreviations (ISO 32000-2 §8.9.7).
fn inline_filter(name: &[u8]) -> bool {
    matches!(
        name,
        b"ASCIIHexDecode"
            | b"AHx"
            | b"ASCII85Decode"
            | b"A85"
            | b"LZWDecode"
            | b"LZW"
            | b"FlateDecode"
            | b"Fl"
            | b"RunLengthDecode"
            | b"RL"
            | b"CCITTFaxDecode"
            | b"CCF"
            | b"DCTDecode"
            | b"DCT"
    )
}

/// The outer filter of an inline image, and whether it is the only one.
fn outer_filter(dict: &Dict) -> (Outer, bool) {
    let filters = match dict.get(b"Filter").or_else(|| dict.get(b"F")) {
        None | Some(Object::Null) => return (Outer::None, true),
        Some(Object::Array(filters)) => filters.as_slice(),
        Some(other) => std::slice::from_ref(other),
    };
    let Some(first) = filters.first() else { return (Outer::None, true) };
    let sole = filters.len() == 1;
    if !filters.iter().all(|f| f.as_name().is_some_and(inline_filter)) {
        return (Outer::Unknown, sole);
    }
    let kind = match first.as_name() {
        Some(b"ASCIIHexDecode" | b"AHx") => Outer::Hex,
        Some(b"ASCII85Decode" | b"A85") => Outer::A85,
        _ => Outer::Other,
    };
    (kind, sole)
}

/// The keys of an inline image that this parser reads where it ends by, in both their spellings:
/// its filters and their parameters, and its layout. Of a key in both spellings readers may take
/// either, and of one given twice either value (Ghostscript keeps the first, the renderer and
/// Poppler the last). A `/Decode` a reader refuses ends the image elsewhere too, and isn't read.
const END_KEYS: [(&[u8], &[u8]); 7] = [
    (b"Filter", b"F"),
    (b"DecodeParms", b"DP"),
    (b"Width", b"W"),
    (b"Height", b"H"),
    (b"ColorSpace", b"CS"),
    (b"BitsPerComponent", b"BPC"),
    (b"ImageMask", b"IM"),
];

/// Whether an inline image gives a key of [`END_KEYS`] in both its spellings, saying different
/// things. Readers may take either.
fn spellings_disagree(dict: &Dict) -> bool {
    END_KEYS.into_iter().any(|(long, short)| matches!((dict.get(long), dict.get(short)), (Some(long), Some(short)) if long != short))
}

/// Whether a reader that decodes an inline image (Poppler does) may end it at another `EI` than the
/// renderer's, at `ei`. Such a reader reads the data through its filters until it has the samples
/// the image needs, or until the filter's end-of-data marker, and ends the image at the first `EI`
/// after that, whatever the bytes around it; an image it can't decode it ends at the first `EI`
/// after `ID`. Where it stops is worked out exactly with no filter and a layout this parser knows
/// ([`layout_bytes`]), or with one ASCIIHex or ASCII85 filter and such a layout, by
/// counting its characters up to `ei` (a reader that needs more ends elsewhere). Otherwise the
/// image counts unless no `EI` comes before the renderer's and, behind ASCIIHex or ASCII85, the
/// marker does; behind them and then only Flate or RunLength, an `EI` before the renderer's counts only where
/// the reader may stop before it, told by decoding ([`chain_stop`]). A colour space named as a
/// resource has a layout only the page's resources tell, which this parser can't look up: a reader
/// that can reads by it, possibly past the renderer's `EI`, and that is missed here, as is a colour
/// space given as an array other than Indexed, and a device name the page's resources redefine.
/// Behind another filter first (Flate, DCT and the rest) the end isn't told, and the renderer's
/// stands; behind one no reader decodes, the first `EI` after `ID` is the end. An image that gives
/// a key in both spellings, differently, counts.
fn ends_elsewhere(data: &[u8], begin: usize, ei: usize, dict: &Dict, scan: &mut ImageEnds) -> bool {
    // Readers may take either spelling (the renderer's decoder takes `/F`, `outer_filter`
    // `/Filter`), so where one ends the image can't be told.
    if spellings_disagree(dict) {
        return true;
    }
    let (outer, sole) = outer_filter(dict);
    let stop = match (outer, layout_bytes(dict)) {
        (Outer::Other, _) => return false,
        (Outer::Unknown, _) => return scan.ei.at(data, b"EI", begin).is_some_and(|at| at < ei),
        (Outer::None, Some(bytes)) => begin.checked_add(bytes).filter(|stop| *stop <= ei),
        (Outer::Hex | Outer::A85, Some(bytes)) if sole => decoder_stop(data, begin, ei, bytes, outer),
        _ => {
            let marker_first = match outer {
                Outer::Hex => scan.hex_end.at(data, b">", begin).is_some_and(|at| at < ei),
                Outer::A85 => scan.a85_end.at(data, b"~", begin).is_some_and(|at| at < ei),
                _ => true,
            };
            if !marker_first {
                return true;
            }
            if !scan.ei.at(data, b"EI", begin).is_some_and(|at| at < ei) {
                return false;
            }
            // An `EI` before the renderer's counts where a decoding reader may stop before it: it
            // reads whole groups, so one whose `E` ends the last group it reads it doesn't see.
            return match chain_stop(data, begin, ei, dict, outer, sole, scan) {
                Some(stop) => scan.ei.at(data, b"EI", stop) != Some(ei),
                None => true,
            };
        }
    };
    stop.is_none_or(|stop| scan.ei.at(data, b"EI", stop) != Some(ei))
}

/// Where a reader decoding an image's data through one ASCIIHex or ASCII85 filter stops reading it,
/// when the image needs `bytes` bytes: after the characters that give them, or after the filter's
/// end-of-data marker if that comes first. ASCIIHex takes two characters a byte, any but white
/// space (readers take an unknown one as 0); ASCII85 a group of five for four bytes (`z` for a
/// group), read whole. `None` when that is past `ei`, the renderer's `EI`, or the ASCII85 data
/// holds a character outside its alphabet, which readers may take otherwise. Counted up to `ei`
/// only, so the images of a stream are counted once in all.
fn decoder_stop(data: &[u8], begin: usize, ei: usize, bytes: usize, outer: Outer) -> Option<usize> {
    let needed = match outer {
        Outer::Hex => bytes.checked_mul(2)?,
        _ => bytes.div_ceil(4).checked_mul(5)?,
    };
    let mut count = 0usize;
    let mut at = begin;
    let stop = loop {
        if count >= needed {
            break Some(at);
        }
        if at >= ei {
            break None;
        }
        let Some(&b) = data.get(at) else { break None };
        at += 1;
        match (outer, b) {
            (_, b) if is_ws(b) => {}
            (Outer::Hex, b'>') | (Outer::A85, b'~') => break Some(at),
            (Outer::Hex, _) => count += 1,
            (Outer::A85, b'z') if count.is_multiple_of(5) => count += 5,
            (Outer::A85, b'!'..=b'u') => count += 1,
            _ => break None,
        }
    };
    #[cfg(test)]
    tests::EOD_SEARCHED.with(|c| c.set(c.get() + at.saturating_sub(begin)));
    stop
}

/// Where a reader decoding an inline image's data through ASCIIHex or ASCII85 (`outer`, its
/// marker before `ei`) and one filter more stops reading it: after the text that gives the least
/// the inner filter takes in to have the samples the image's layout needs, or to reach its own
/// end. Told for Flate (zlib) and RunLength, the text decoded and inflated within what the
/// parse's [`ImageDecoding`] has left, which a decode that fails is charged for too.
/// `None` for a filter alone or another one (DCT among them: a JPEG decoder may stop before its
/// end-of-image marker, at rows it doesn't need or a marker it refuses, where reading the
/// markers doesn't tell), for an image with decode parameters or without a layout
/// [`layout_bytes`] tells, and where the data doesn't decode.
fn chain_stop(data: &[u8], begin: usize, ei: usize, dict: &Dict, outer: Outer, sole: bool, scan: &mut ImageEnds) -> Option<usize> {
    if sole || !matches!(outer, Outer::Hex | Outer::A85) {
        return None;
    }
    // Parameters that say nothing are none: null, an empty dictionary, or an array of those.
    let none = |o: &Object| matches!(o, Object::Null) || o.as_dict().is_some_and(|d| d.iter().next().is_none());
    match dict.get(b"DecodeParms").or_else(|| dict.get(b"DP")) {
        None => {}
        Some(Object::Array(each)) if each.iter().all(none) => {}
        Some(params) if none(params) => {}
        Some(_) => return None,
    }
    let Some(Object::Array(filters)) = dict.get(b"Filter").or_else(|| dict.get(b"F")) else { return None };
    let [_, inner] = filters.as_slice() else { return None };
    // Whether the inner filter is Flate or RunLength, before anything is decoded for it.
    let flate = match inner.as_name()? {
        b"FlateDecode" | b"Fl" => true,
        b"RunLengthDecode" | b"RL" => false,
        _ => return None,
    };
    let samples = layout_bytes(dict)?;
    let left = &mut scan.decoding.left;
    let decoded = match outer_decoded(data.get(begin..ei)?, outer, *left) {
        Ok(decoded) => decoded,
        Err(spent) => {
            *left = left.saturating_sub(spent);
            return None;
        }
    };
    *left = left.saturating_sub(decoded.len());
    let taken = if flate {
        let (taken, made) = pdfcraft_filters::flate_input_for(&decoded, samples, *left);
        *left = left.saturating_sub(made);
        taken?
    } else {
        run_length_input(&decoded, samples)
    };
    outer_text_for(data.get(begin..ei)?, outer, taken).and_then(|at| begin.checked_add(at))
}

/// One step of ASCIIHex or ASCII85 text: the bytes a group (ASCII85's five characters or `z`,
/// ASCIIHex's two digits) gives, the end-of-data marker with what the group before it gives, or
/// none. `None` for text no reader agrees on: a character outside the filter's alphabet, ASCII85's
/// `z` inside a group or a group past 2^32 - 1, a last group of one character.
enum Group {
    Bytes(usize),
    End(usize),
}

/// The groups of ASCIIHex or ASCII85 `text`, each with the position after it.
fn outer_groups(text: &[u8], outer: Outer) -> impl Iterator<Item = Option<(Group, usize, [u8; 4])>> + '_ {
    let mut at = 0usize;
    let mut done = false;
    std::iter::from_fn(move || {
        if done {
            return None;
        }
        let mut digits = [0u32; 5];
        let mut count = 0usize;
        let width = if outer == Outer::Hex { 2 } else { 5 };
        loop {
            let Some(&b) = text.get(at) else {
                done = true;
                return (count > 0).then_some(None);
            };
            at += 1;
            #[cfg(test)]
            tests::EOD_SEARCHED.with(|c| c.set(c.get() + 1));
            let digit = match (outer, b) {
                (_, b) if is_ws(b) => continue,
                (Outer::Hex, b'>') | (Outer::A85, b'~') => {
                    done = true;
                    return Some(group(outer, &digits, count, true).map(|(n, bytes)| (Group::End(n), at, bytes)));
                }
                (Outer::A85, b'z') if count == 0 => return Some(Some((Group::Bytes(4), at, [0; 4]))),
                (Outer::Hex, b) => (b as char).to_digit(16),
                (Outer::A85, b'!'..=b'u') => Some(u32::from(b - b'!')),
                _ => None,
            };
            let Some(digit) = digit else {
                done = true;
                return Some(None);
            };
            if let Some(slot) = digits.get_mut(count) {
                *slot = digit;
            }
            count += 1;
            if count == width {
                return Some(group(outer, &digits, count, false).map(|(n, bytes)| (Group::Bytes(n), at, bytes)));
            }
        }
    })
}

/// The bytes `count` digits of a group give, and how many: a whole group, or the last one before
/// the marker (`last`), padded as the filter pads it.
fn group(outer: Outer, digits: &[u32; 5], count: usize, last: bool) -> Option<(usize, [u8; 4])> {
    if outer == Outer::Hex {
        let [high, low, ..] = *digits;
        let byte = u8::try_from(high << 4 | if count == 2 { low } else { 0 }).ok()?;
        return Some((usize::from(count > 0), [byte, 0, 0, 0]));
    }
    if last && count == 0 {
        return Some((0, [0; 4]));
    }
    if count == 1 {
        return None;
    }
    let value = (0..5).try_fold(0u64, |v, i| Some(v * 85 + u64::from(if i < count { *digits.get(i)? } else { 84 })))?;
    let bytes = u32::try_from(value).ok()?.to_be_bytes();
    Some((if count == 5 { 4 } else { count - 1 }, bytes))
}

/// The bytes ASCIIHex or ASCII85 `text` gives up to its end-of-data marker; or, where it holds a
/// group no reader agrees on, or no marker, or gives more than `max` bytes (ASCII85 gives four a
/// character for `z`), how many it gave before that.
fn outer_decoded(text: &[u8], outer: Outer, max: usize) -> Result<Vec<u8>, usize> {
    let mut out = Vec::new();
    for step in outer_groups(text, outer) {
        let Some((step, _, bytes)) = step else { return Err(out.len()) };
        let (Group::Bytes(n) | Group::End(n)) = step;
        if out.len().saturating_add(n) > max {
            return Err(out.len());
        }
        let Some(group) = bytes.get(..n) else { return Err(out.len()) };
        out.extend_from_slice(group);
        if let Group::End(_) = step {
            return Ok(out);
        }
    }
    Err(out.len())
}

/// Where in ASCIIHex or ASCII85 `text` a reader that has taken in `taken` bytes of what it gives
/// stops: after the group that gives the last of them, read whole.
fn outer_text_for(text: &[u8], outer: Outer, taken: usize) -> Option<usize> {
    if taken == 0 {
        return Some(0);
    }
    let mut given = 0usize;
    for step in outer_groups(text, outer) {
        let (Group::Bytes(n) | Group::End(n), at, _) = step?;
        given = given.saturating_add(n);
        if given >= taken {
            return Some(at);
        }
    }
    None
}

/// How many bytes of RunLength data a reader takes in to have `samples` bytes, or to its
/// end-of-data byte (ISO 32000-2 §7.4.5); all of it where it ends before either.
fn run_length_input(data: &[u8], samples: usize) -> usize {
    let (mut at, mut made) = (0usize, 0usize);
    while made < samples {
        let Some(&length) = data.get(at) else { return data.len() };
        match length {
            128 => return at + 1,
            0..=127 => {
                at = at.saturating_add(usize::from(length) + 2);
                made = made.saturating_add(usize::from(length) + 1);
            }
            _ => {
                at = at.saturating_add(2);
                made = made.saturating_add(257 - usize::from(length));
            }
        }
    }
    at.min(data.len())
}

/// Where the renderer's tokenizer ends the inline image whose `BI` starts at `bi`: the start and
/// end of its data and the position after its `EI`. `None` when it reads no image there (its
/// parameters don't read, or no `EI` passes its checks).
fn renderer_image_end(data: &[u8], bi: usize) -> Option<(usize, usize, usize)> {
    use hayro_syntax::content::TypedIter;
    use hayro_syntax::content::ops::{InlineImage, TypedInstruction};
    let tail = data.get(bi..)?;
    let mut iter = TypedIter::new(tail);
    let (start, len) = match iter.next()? {
        TypedInstruction::InlineImage(InlineImage(stream)) => match stream.raw_data() {
            // The data is a slice of `tail`: its offset is where it starts.
            std::borrow::Cow::Borrowed(samples) => ((samples.as_ptr() as usize).checked_sub(tail.as_ptr() as usize)?, samples.len()),
            std::borrow::Cow::Owned(_) => return None,
        },
        _ => return None,
    };
    let end_data = start.checked_add(len)?;
    // `committed` is past the whitespace after `EI`; the image ends at the `EI`.
    let mut end = iter.committed();
    while end > end_data && tail.get(end - 1).is_some_and(|b| is_ws(*b)) {
        end -= 1;
    }
    if end < end_data.checked_add(2)? || tail.get(end - 2..end) != Some(b"EI") {
        return None;
    }
    Some((bi.checked_add(start)?, bi.checked_add(end_data)?, bi.checked_add(end)?))
}

/// The end-of-data marker (`>` for ASCIIHex, `~>` for ASCII85) of the outer stream filter, if any.
fn filter_eod(dict: &Dict) -> &'static [u8] {
    for key in [b"Filter".as_slice(), b"F"] {
        if let Some(filter) = dict.get(key) {
            let name = match filter {
                Object::Name(name) => Some(name.as_slice()),
                Object::Array(filters) => filters.first().and_then(Object::as_name),
                _ => None,
            };
            return match name {
                Some(b"ASCIIHexDecode" | b"AHx") => b">",
                Some(b"ASCII85Decode" | b"A85") => b"~>",
                _ => b"",
            };
        }
    }
    b""
}

/// The bytes an inline image's samples take as stored, unfiltered: its rows, each padded to a whole
/// byte. `None` unless its layout is one of a device colour space (or a short name of one), an
/// Indexed array written in the image, or an image mask, and its keys agree where both spellings
/// are given: a colour space named as a resource has a layout only the page's resources tell.
fn layout_bytes(dict: &Dict) -> Option<usize> {
    for (full, short) in
        [(b"Width".as_slice(), b"W".as_slice()), (b"Height", b"H"), (b"ColorSpace", b"CS"), (b"BitsPerComponent", b"BPC"), (b"ImageMask", b"IM")]
    {
        if let (Some(full), Some(short)) = (dict.get(full), dict.get(short))
            && full != short
        {
            return None;
        }
    }
    let get = |full: &[u8], short: &[u8]| dict.get(full).or_else(|| dict.get(short));
    let width = usize::try_from(get(b"Width", b"W")?.as_int()?).ok().filter(|n| *n > 0)?;
    let height = usize::try_from(get(b"Height", b"H")?.as_int()?).ok().filter(|n| *n > 0)?;
    let mask = match get(b"ImageMask", b"IM") {
        None | Some(Object::Null | Object::Bool(false)) => false,
        Some(Object::Bool(true)) => true,
        _ => return None,
    };
    let bits = match get(b"BitsPerComponent", b"BPC") {
        None | Some(Object::Null) => None,
        Some(value) => Some(value.as_int()?),
    };
    let (components, bits) = if mask {
        if bits.is_some_and(|bits| bits != 1) {
            return None;
        }
        (1usize, 1usize)
    } else {
        let (components, max_bits) = match get(b"ColorSpace", b"CS")? {
            Object::Name(name) => match name.as_slice() {
                b"DeviceGray" | b"G" => (1, 16),
                b"DeviceRGB" | b"RGB" => (3, 16),
                b"DeviceCMYK" | b"CMYK" => (4, 16),
                _ => return None,
            },
            Object::Array(space) if space.len() == 4 && matches!(space.first().and_then(Object::as_name), Some(b"Indexed" | b"I")) => (1, 8),
            _ => return None,
        };
        let bits = match bits? {
            bits @ (1 | 2 | 4 | 8 | 16) => bits as usize,
            _ => return None,
        };
        if bits > max_bits {
            return None;
        }
        (components, bits)
    };
    let row = width.checked_mul(components)?.checked_mul(bits)?.div_ceil(8);
    row.checked_mul(height)
}

/// An unfiltered image's samples determine its length, including byte padding per row.
/// Binary samples may themselves contain whitespace + `EI` + whitespace, so use the known
/// length when the actual terminator follows it. Unknown or damaged layouts keep the heuristic.
fn unfiltered_image_end(data: &[u8], begin: usize, dict: &Dict) -> Option<(usize, usize)> {
    for key in [b"Filter".as_slice(), b"F"] {
        if let Some(filter) = dict.get(key)
            && !matches!(filter, Object::Null)
            && !matches!(filter, Object::Array(filters) if filters.is_empty())
        {
            return None;
        }
    }
    let end_data = begin.checked_add(layout_bytes(dict)?)?;
    let after = data.get(end_data..)?;
    let separator = after.iter().position(|b| !is_ws(*b))?;
    if separator == 0 && data.get(end_data.checked_sub(1)?).is_some_and(|b| is_ws(*b)) {
        return None;
    }
    let ei = end_data.checked_add(separator)?;
    let end = ei.checked_add(2)?;
    if data.get(ei..end) == Some(b"EI") && data.get(end).is_none_or(|b| is_ws(*b) || is_delim(*b)) { Some((end_data, end)) } else { None }
}

/// The length declared by `/L` (or `/Length`, ISO 32000-2 §8.9.7), used when `EI` follows the
/// declared bytes after optional whitespace. A declared length without that `EI` is ignored.
fn declared_image_end(data: &[u8], begin: usize, dict: &Dict) -> Option<(usize, usize)> {
    let length = dict.get(b"L").or_else(|| dict.get(b"Length"))?.as_int()?;
    let end_data = begin.checked_add(usize::try_from(length).ok()?)?;
    let after = data.get(end_data..)?;
    let ei = end_data.checked_add(after.iter().position(|b| !is_ws(*b))?)?;
    let end = ei.checked_add(2)?;
    if data.get(ei..end) == Some(b"EI") && data.get(end).is_none_or(|b| is_ws(*b) || is_delim(*b)) { Some((end_data, end)) } else { None }
}

/// Where inline image data of unknown length ends. A candidate `EI` followed by binary bytes before
/// the next `EI` lies inside the (compressed) data, unless another inline image starts first.
fn image_data_end(data: &[u8], begin: usize, eod: &[u8]) -> Option<(usize, usize)> {
    let mut candidate = end_of_image(data, begin, eod)?;
    loop {
        let (end_data, end) = candidate;
        let Some(next) = end_of_image(data, end, eod) else {
            return Some(candidate);
        };
        let between = data.get(end..next.0).unwrap_or_default();
        if !between.iter().any(|b| !b.is_ascii()) || starts_inline_image(between) {
            return Some((end_data, end));
        }
        candidate = next;
    }
}

/// Whether `bytes` hold a whitespace-delimited `BI` operator.
fn starts_inline_image(bytes: &[u8]) -> bool {
    bytes
        .windows(2)
        .enumerate()
        .any(|(i, pair)| pair == b"BI" && (i == 0 || is_ws(bytes[i - 1])) && bytes.get(i + 2).is_none_or(|b| is_ws(*b) || is_delim(*b)))
}

/// Where inline image data starting at `begin` ends: (end of the data, position after `EI`). The
/// data ends at whitespace (or `eod`: `>` for ASCIIHex, `~>` for ASCII85) + `EI` + (whitespace,
/// a delimiter or the end).
fn end_of_image(data: &[u8], begin: usize, eod: &[u8]) -> Option<(usize, usize)> {
    let mut i = begin;
    while i + 2 <= data.len() {
        if data[i] == b'E'
            && data.get(i + 1) == Some(&b'I')
            && (i == begin || is_ws(data[i - 1]) || (!eod.is_empty() && data[begin..i].ends_with(eod)))
            && data.get(i + 2).is_none_or(|b| is_ws(*b) || is_delim(*b))
        {
            let end_data = if i > begin && is_ws(data[i - 1]) { i - 1 } else { i };
            return Some((end_data, i + 2));
        }
        i += 1;
    }
    None
}

/// Write one operator (with a trailing newline).
pub fn write_op(op: &Op, out: &mut Vec<u8>) {
    if let Some((dict, img)) = &op.inline {
        out.extend_from_slice(b"BI");
        for (k, v) in dict.iter() {
            out.push(b' ');
            serialize(&Object::Name(k.clone()), out);
            out.push(b' ');
            serialize(v, out);
        }
        out.extend_from_slice(b" ID ");
        out.extend_from_slice(img);
        out.extend_from_slice(b"\nEI\n");
        return;
    }
    for o in &op.operands {
        serialize(o, out);
        out.push(b' ');
    }
    out.extend_from_slice(&op.op);
    out.push(b'\n');
}

/// Write operators back to a content stream.
pub fn serialize_ops(ops: &[Op]) -> Vec<u8> {
    let mut out = Vec::new();
    for op in ops {
        write_op(op, &mut out);
    }
    out
}

/// A page's content streams joined into the one stream they are. Writers may split it between
/// any two tokens (ISO 32000-2 §7.8.2), so an operator's operands can end one piece and its
/// keyword start the next: parse the pieces joined ([`Pieces::parse`]) and map positions back
/// with [`Pieces::piece_of`]. A space separates the pieces, as renderers join them (hayro,
/// MuPDF and Poppler alike): tokens never run together, and a comment at the end of a piece
/// runs on into the next one, up to its first end of line.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Pieces {
    data: Vec<u8>,
    /// Where each piece ends in `data`, after its separator.
    ends: Vec<usize>,
}

impl Pieces {
    pub fn join<D: AsRef<[u8]>>(pieces: &[D]) -> Self {
        let mut data = Vec::new();
        let mut ends = Vec::with_capacity(pieces.len());
        for p in pieces {
            data.extend_from_slice(p.as_ref());
            data.push(b' ');
            ends.push(data.len());
        }
        Pieces { data, ends }
    }

    /// The joined bytes (the spans of [`Pieces::parse`]'s operators index into them).
    pub fn data(&self) -> &[u8] {
        &self.data
    }

    /// How many pieces there are.
    pub fn len(&self) -> usize {
        self.ends.len()
    }

    pub fn is_empty(&self) -> bool {
        self.ends.is_empty()
    }

    /// Parse the pieces as one stream, as [`parse`] does.
    pub fn parse(&self) -> Parsed {
        parse_pieces(&self.data, &self.ends, &mut ImageDecoding::none())
    }

    /// [`Pieces::parse`], its inline images decoded within `decoding` ([`parse_within`]).
    pub fn parse_within(&self, decoding: &mut ImageDecoding) -> Parsed {
        parse_pieces(&self.data, &self.ends, decoding)
    }

    /// Per piece, from `parsed` (this piece set's [`Pieces::parse`]): whether a comment runs
    /// into it from the piece before, and whether one runs out of it into the next. A writer
    /// that rewrites a piece without its comments must keep both: end the incoming comment
    /// before its first operator, and leave a comment open at its end.
    pub fn comment_edges(&self, parsed: &Parsed) -> Vec<(bool, bool)> {
        let mut edges = parsed.comment_edges.clone();
        edges.resize(self.len(), (false, false));
        edges
    }

    /// The piece holding joined position `at` (the last piece for positions past the end).
    pub fn piece_of(&self, at: usize) -> usize {
        self.ends.partition_point(|&e| e <= at).min(self.ends.len().saturating_sub(1))
    }

    /// The pieces an operator spans: (the one its first token is in, the one its keyword is in).
    pub fn pieces_of(&self, op: &Op) -> (usize, usize) {
        (self.piece_of(op.span.start), self.piece_of(op.span.end.saturating_sub(1)))
    }

    /// The joined range of piece `i`, without its separator.
    fn range(&self, i: usize) -> std::ops::Range<usize> {
        let start = match i.checked_sub(1) {
            None => 0,
            Some(p) => self.ends.get(p).copied().unwrap_or(self.data.len()),
        };
        let end = self.ends.get(i).map_or(self.data.len(), |e| e.saturating_sub(1)).max(start);
        start..end
    }

    /// Rebuild every piece after replacing joined byte ranges: `edits` are (range, bytes) in
    /// ascending, non-overlapping order. A replacement goes into the piece its range starts in,
    /// and the rest of a range that runs on into later pieces is removed from them, so an
    /// operator split across pieces is replaced or removed whole. Everything else is copied byte
    /// for byte. Returns every piece's new bytes (unchanged pieces come back equal).
    pub fn splice(&self, edits: impl IntoIterator<Item = (std::ops::Range<usize>, Vec<u8>)>) -> Vec<Vec<u8>> {
        let mut out: Vec<Vec<u8>> = (0..self.len()).map(|i| Vec::with_capacity(self.range(i).len())).collect();
        // Only the pieces from the one holding `from` up to `to`: one call per edit must not
        // visit every piece.
        let copy = |out: &mut Vec<Vec<u8>>, from: usize, to: usize| {
            if from >= to {
                return;
            }
            let first = self.piece_of(from);
            for (i, piece) in out.iter_mut().enumerate().skip(first) {
                #[cfg(test)]
                tests::PIECE_VISITS.with(|c| c.set(c.get() + 1));
                let r = self.range(i);
                if r.start >= to {
                    break;
                }
                let (a, b) = (from.max(r.start), to.min(r.end));
                if a < b {
                    piece.extend_from_slice(self.data.get(a..b).unwrap_or_default());
                }
            }
        };
        let mut at = 0;
        for (range, bytes) in edits {
            let start = range.start.clamp(at, self.data.len());
            copy(&mut out, at, start);
            if !bytes.is_empty()
                && let Some(piece) = out.get_mut(self.piece_of(start))
            {
                piece.extend_from_slice(&bytes);
            }
            at = range.end.clamp(start, self.data.len());
        }
        copy(&mut out, at, self.data.len());
        out
    }
}

/// A number operand, written as an integer when it is one.
pub fn num(v: f64) -> Object {
    if v.fract() == 0.0 && v.abs() < 1e15 { Object::Int(v as i64) } else { Object::Real((v * 10_000.0).round() / 10_000.0) }
}

pub fn string(bytes: Vec<u8>) -> Object {
    Object::String(PdfString::literal(bytes))
}

/// An affine matrix `[a b c d e f]` (PDF order: a point maps to `(a·x + c·y + e, b·x + d·y + f)`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Matrix(pub [f64; 6]);

impl Default for Matrix {
    fn default() -> Self {
        Matrix::IDENTITY
    }
}

impl Matrix {
    pub const IDENTITY: Matrix = Matrix([1.0, 0.0, 0.0, 1.0, 0.0, 0.0]);

    pub fn translate(x: f64, y: f64) -> Self {
        Matrix([1.0, 0.0, 0.0, 1.0, x, y])
    }

    pub fn from_operands(v: &[Object]) -> Option<Self> {
        if v.len() != 6 {
            return None;
        }
        let mut m = [0.0; 6];
        for (o, x) in m.iter_mut().zip(v) {
            *o = x.as_f64()?;
        }
        Some(Matrix(m))
    }

    /// `self` then `then` (`self × then` in PDF's row-vector convention).
    pub fn then(&self, then: &Matrix) -> Matrix {
        let [a, b, c, d, e, f] = self.0;
        let [a2, b2, c2, d2, e2, f2] = then.0;
        Matrix([a * a2 + b * c2, a * b2 + b * d2, c * a2 + d * c2, c * b2 + d * d2, e * a2 + f * c2 + e2, e * b2 + f * d2 + f2])
    }

    pub fn apply(&self, x: f64, y: f64) -> (f64, f64) {
        let [a, b, c, d, e, f] = self.0;
        (a * x + c * y + e, b * x + d * y + f)
    }

    pub fn invert(&self) -> Option<Matrix> {
        let [a, b, c, d, e, f] = self.0;
        let det = a * d - b * c;
        if det == 0.0 || !det.is_finite() || det.abs() <= 1e-12 * ((a * d).abs() + (b * c).abs()) {
            return None;
        }
        let (ia, ib, ic, id) = (d / det, -b / det, -c / det, a / det);
        let inverse = [ia, ib, ic, id, -(e * ia + f * ic), -(e * ib + f * id)];
        // A determinant that underflows toward zero can still pass the relative check above,
        // and dividing by it overflows: such a matrix has no usable inverse.
        inverse.iter().all(|v| v.is_finite()).then_some(Matrix(inverse))
    }

    /// The bounding box of a rectangle `[x0 y0 x1 y1]` after this transform.
    pub fn bbox(&self, r: [f64; 4]) -> [f64; 4] {
        let pts = [self.apply(r[0], r[1]), self.apply(r[2], r[1]), self.apply(r[0], r[3]), self.apply(r[2], r[3])];
        let mut b = [f64::MAX, f64::MAX, f64::MIN, f64::MIN];
        for (x, y) in pts {
            b = [b[0].min(x), b[1].min(y), b[2].max(x), b[3].max(y)];
        }
        b
    }
}

/// Do two rectangles overlap by more than `eps` in both directions?
pub fn overlaps(a: [f64; 4], b: [f64; 4], eps: f64) -> bool {
    a[0].max(b[0]) + eps < a[2].min(b[2]) && a[1].max(b[1]) + eps < a[3].min(b[3])
}

/// Is `inner` inside `outer` (within `eps`)?
pub fn contains(outer: [f64; 4], inner: [f64; 4], eps: f64) -> bool {
    inner[0] >= outer[0] - eps && inner[1] >= outer[1] - eps && inner[2] <= outer[2] + eps && inner[3] <= outer[3] + eps
}
