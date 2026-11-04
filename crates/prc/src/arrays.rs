//! The array primitives of compressed tessellation [WD 11.6-11.9] and the Huffman payload
//! they may carry [WD 12.2].
//!
//! The WD gives only the Huffman writer. The reader decodes what producers write: each
//! leaf's code length is read in the width the payload's 8-bit field states (the WD writes a
//! fixed 8 bits), stored codes carry a leading root bit, and the payload is read LSB-first
//! although the stream around it is MSB-first.

use std::collections::HashMap;

use crate::PrcError;
use crate::bits::BitReader;

/// Longest Huffman code [WD 12.2].
const MAX_CODE_BITS: u32 = 32;

/// Widest Huffman symbol a caller asks for: the arrays hold 2- to 16-bit values.
const MAX_SYMBOL_BITS: u32 = 16;

fn malformed(what: &str) -> PrcError {
    PrcError::Malformed(what.to_owned())
}

/// An LSB-first cursor over a Huffman payload.
struct Lsb<'a> {
    data: &'a [u8],
    pos: usize,
    end: usize,
}

impl Lsb<'_> {
    fn bit(&mut self) -> Result<u32, PrcError> {
        if self.pos >= self.end {
            return Err(PrcError::Truncated("a Huffman payload"));
        }
        let byte = self.data.get(self.pos / 8).ok_or(PrcError::Truncated("a Huffman payload"))?;
        let b = u32::from((byte >> (self.pos % 8)) & 1);
        self.pos += 1;
        Ok(b)
    }

    /// `n` ≤ 32 bits, the first read being the least significant.
    fn bits(&mut self, n: u32) -> Result<u32, PrcError> {
        let mut v = 0u32;
        for i in 0..n.min(32) {
            v |= self.bit()? << i;
        }
        Ok(v)
    }

    fn remaining(&self) -> usize {
        self.end.saturating_sub(self.pos)
    }
}

/// The low `n` (1..=16) bits of `v` as a symbol, two's complement when `signed`.
fn symbol(v: u32, n: u32, signed: bool) -> i32 {
    let v = i64::from(v);
    let wide = if signed && n > 0 && v & (1 << (n - 1)) != 0 { v - (1i64 << n) } else { v };
    // n <= 16, so `wide` is within ±2^16.
    i32::try_from(wide).unwrap_or(0)
}

/// The Huffman container of a compressed Character/Short array [WD 11.6, 11.7, 12.2]:
/// symbols are `nbits` wide, sign-extended when `signed`.
fn huffman(r: &mut BitReader<'_>, nbits: u32, signed: bool) -> Result<Vec<i32>, PrcError> {
    if nbits == 0 || nbits > MAX_SYMBOL_BITS {
        return Err(malformed("Huffman symbol width"));
    }
    let words = r.unsigned_integer()? as usize;
    if words == 0 {
        return Ok(Vec::new());
    }
    if words.saturating_mul(32) > r.remaining() {
        return Err(PrcError::Truncated("a Huffman payload"));
    }
    // words * 32 bits remain, so words * 4 bytes cannot overflow.
    let blob = (0..words * 4).map(|_| r.character()).collect::<Result<Vec<u8>, _>>()?;
    let last = r.unsigned_integer()?;
    if last > 32 {
        return Err(malformed("Huffman last-word bit count above 32"));
    }
    let end = (words - 1) * 32 + last as usize;
    let mut b = Lsb { data: &blob, pos: 0, end };
    let codes = huffman_leaves(&mut b, nbits, signed)?;
    if codes.is_empty() {
        return Ok(Vec::new());
    }
    let count = b.bits(32)? as usize;
    // Every element is at least one bit.
    if count > b.remaining() {
        return Err(PrcError::Truncated("a Huffman element count"));
    }
    r.take_elements(count)?;
    let mut out = Vec::with_capacity(count.min(1024));
    for _ in 0..count {
        out.push(huffman_symbol(&mut b, &codes)?);
    }
    Ok(out)
}

/// The leaf table: `(code length, stored code)` to symbol.
fn huffman_leaves(b: &mut Lsb<'_>, nbits: u32, signed: bool) -> Result<HashMap<(u32, u32), i32>, PrcError> {
    let leaves = b.bits(nbits + 1)?;
    if leaves == 0 {
        return Ok(HashMap::new());
    }
    // prc-rs: the code-length width is read from the payload, not fixed at 8 bits.
    let len_width = b.bits(8)?;
    if len_width == 0 || len_width > MAX_CODE_BITS {
        return Err(malformed("Huffman code-length width"));
    }
    // Every leaf is at least nbits + 2 bits, so the payload bounds the table.
    if (leaves as usize).saturating_mul((nbits + 2) as usize) > b.remaining() {
        return Err(PrcError::Truncated("a Huffman payload"));
    }
    let mut codes = HashMap::new();
    for _ in 0..leaves {
        let s = symbol(b.bits(nbits)?, nbits, signed);
        let len = b.bits(len_width)?;
        if len == 0 || len > MAX_CODE_BITS {
            return Err(malformed("Huffman code length"));
        }
        let code = b.bits(len)?;
        if codes.insert((len, code), s).is_some() {
            return Err(malformed("duplicate Huffman code"));
        }
    }
    // A leaf whose code begins another's would end that code early, leaving its unread bits
    // in front of the next element.
    let shadowed = codes.keys().any(|&(len, code)| (1..len).any(|l| codes.contains_key(&(l, code >> (len - l)))));
    if shadowed {
        return Err(malformed("a Huffman code that is a prefix of another"));
    }
    Ok(codes)
}

/// One element: code bits, most significant first, until a leaf matches.
fn huffman_symbol(b: &mut Lsb<'_>, codes: &HashMap<(u32, u32), i32>) -> Result<i32, PrcError> {
    // prc-rs: the stored code's first bit read is its most significant (the root bit).
    let (mut code, mut len) = (0u32, 0u32);
    while len < MAX_CODE_BITS {
        code = (code << 1) | b.bit()?;
        len += 1;
        if let Some(&s) = codes.get(&(len, code)) {
            return Ok(s);
        }
    }
    Err(malformed("a Huffman code matching no leaf"))
}

/// A `CharacterArray(nbits)` [WD 11.6]. `compressed` is `None` when the flag is written,
/// else the caller's implicit value. Uncompressed elements are whole bytes, sign-extended
/// from 8 bits when `signed`.
pub(crate) fn character_array(r: &mut BitReader<'_>, nbits: u32, compressed: Option<bool>, signed: bool) -> Result<Vec<i32>, PrcError> {
    let compressed = match compressed {
        Some(c) => c,
        None => r.bit()?,
    };
    if compressed {
        return huffman(r, nbits, signed);
    }
    let n = r.unsigned_integer()? as usize;
    if n.saturating_mul(8) > r.remaining() {
        return Err(PrcError::Truncated("a character array"));
    }
    r.take_elements(n)?;
    (0..n)
        .map(|_| {
            let c = r.character()?;
            Ok(if signed { i32::from(c.cast_signed()) } else { i32::from(c) })
        })
        .collect()
}

/// A `ShortArray(nbits)` [WD 11.7]: uncompressed elements are a low then a high byte, read
/// unsigned.
pub(crate) fn short_array(r: &mut BitReader<'_>, nbits: u32) -> Result<Vec<i32>, PrcError> {
    if r.bit()? {
        return huffman(r, nbits, false);
    }
    let n = r.unsigned_integer()? as usize;
    if n.saturating_mul(16) > r.remaining() {
        return Err(PrcError::Truncated("a short array"));
    }
    r.take_elements(n)?;
    (0..n)
        .map(|_| {
            let lo = r.character()?;
            let hi = r.character()?;
            Ok(i32::from(u16::from_le_bytes([lo, hi])))
        })
        .collect()
}

/// A `CompressedIntegerArray` [WD 11.8]: a CharacterArray(6) of widths, then one
/// `IntegerWithVariableBitNumber` per width.
pub(crate) fn compressed_integer_array(r: &mut BitReader<'_>) -> Result<Vec<i64>, PrcError> {
    let widths = character_array(r, 6, None, false)?;
    widths
        .into_iter()
        .map(|w| {
            let w = u32::try_from(w).map_err(|_| malformed("integer width"))?;
            r.integer_vbn(w)
        })
        .collect()
}

/// A `CompressedIndiceArray` [WD 11.9], read as producers write it: the stored signed 6-bit
/// values are width differences summed into each element's width, and each element is a
/// delta from the previous (the WD's own reading desyncs from the third element). Indices
/// are never negative.
pub(crate) fn compressed_indice_array(r: &mut BitReader<'_>, compressed: Option<bool>) -> Result<Vec<u32>, PrcError> {
    let diffs = character_array(r, 6, compressed, true)?;
    let mut out = Vec::with_capacity(diffs.len());
    let (mut width, mut prev) = (0i64, 0i64);
    for d in diffs {
        // prc-rs: widths are running sums of the stored differences, values running deltas.
        width = width.checked_add(i64::from(d)).ok_or_else(|| malformed("indice width"))?;
        let w = u32::try_from(width).map_err(|_| malformed("indice width"))?;
        let v = prev.checked_add(r.integer_vbn(w)?).ok_or_else(|| malformed("indice delta"))?;
        out.push(u32::try_from(v).map_err(|_| malformed("an index outside 0..2^32"))?);
        prev = v;
    }
    Ok(out)
}

/// An `UncompressedBoolArray` of `n` raw bits (no length prefix).
pub(crate) fn bool_array(r: &mut BitReader<'_>, n: usize) -> Result<Vec<bool>, PrcError> {
    if n > r.remaining() {
        return Err(PrcError::Truncated("a boolean array"));
    }
    r.take_elements(n)?;
    (0..n).map(|_| r.bit()).collect()
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;
    use crate::testw::W;

    fn read<T>(w: &W, f: impl FnOnce(&mut BitReader<'_>) -> Result<T, PrcError>) -> T {
        let bytes = w.bytes();
        let mut r = BitReader::new(&bytes);
        let v = f(&mut r).unwrap();
        assert_eq!(r.position(), w.len(), "bits consumed");
        v
    }

    #[test]
    fn huffman_decodes_the_wd_example_with_root_bits() {
        // WD 12.2's input {1,3,5,7,7,9,5,3,3}; codes carry the root bit.
        let leaves = [(3, 0b100, 3), (7, 0b101, 3), (5, 0b110, 3), (1, 0b1110, 4), (9, 0b1111, 4)];
        let values = [1, 3, 5, 7, 7, 9, 5, 3, 3];
        let mut w = W::default();
        w.bit(true).huffman(4, 3, &leaves, &values);
        assert_eq!(read(&w, |r| character_array(r, 4, None, false)), values);
    }

    #[test]
    fn huffman_symbols_sign_extend_only_when_signed() {
        let leaves = [(0b111110, 0b10, 2), (1, 0b11, 2)];
        let mut w = W::default();
        w.huffman(6, 2, &leaves, &[62, 1, 62]);
        assert_eq!(read(&w, |r| huffman(r, 6, true)), [-2, 1, -2]);
        assert_eq!(read(&w, |r| huffman(r, 6, false)), [62, 1, 62]);
    }

    #[test]
    fn huffman_rejects_codes_that_match_no_leaf_and_empty_payloads_are_empty() {
        let mut bad = W::default();
        bad.huffman_raw(2, 2, &[(1, 0b10, 2)], &[(0b11, 2)]);
        let bytes = bad.bytes();
        assert!(matches!(huffman(&mut BitReader::new(&bytes), 2, false), Err(PrcError::Malformed(_) | PrcError::Truncated(_))));
        let mut empty = W::default();
        empty.uint(0);
        assert!(read(&empty, |r| huffman(r, 2, false)).is_empty());
    }

    #[test]
    fn a_huffman_element_count_past_the_payload_is_truncated_not_allocated() {
        // One leaf, then an element count of 2^32 - 1 with no elements behind it.
        let mut w = W::default();
        w.huffman_count(2, 2, &[(1, 0b10, 2)], u32::MAX);
        let bytes = w.bytes();
        assert!(matches!(huffman(&mut BitReader::new(&bytes), 2, false), Err(PrcError::Truncated("a Huffman element count"))));
    }

    #[test]
    fn a_huffman_code_that_begins_another_is_malformed() {
        // `10` would end every `100` one bit early.
        let mut w = W::default();
        w.huffman(2, 2, &[(1, 0b10, 2), (2, 0b100, 3)], &[2, 1]);
        let bytes = w.bytes();
        let r = huffman(&mut BitReader::new(&bytes), 2, false);
        assert!(malformed_by(r, "a Huffman code that is a prefix of another"));
    }

    #[test]
    fn huffman_widths_out_of_range_are_errors() {
        // Well-formed payloads at each width, which only the width check refuses.
        for nbits in [0, 17] {
            let mut w = W::default();
            w.huffman(nbits, 2, &[(1, 0b10, 2)], &[1]);
            let bytes = w.bytes();
            assert!(matches!(huffman(&mut BitReader::new(&bytes), nbits, false), Err(PrcError::Malformed(_))), "{nbits}");
        }
    }

    #[test]
    fn uncompressed_arrays() {
        let mut w = W::default();
        w.bit(false).uint(2).put(0xfe, 8).put(3, 8);
        assert_eq!(read(&w, |r| character_array(r, 6, None, true)), [-2, 3]);
        let mut s = W::default();
        s.bit(false).uint(1).put(0x34, 8).put(0x12, 8);
        assert_eq!(read(&s, |r| short_array(r, 16)), [0x1234]);
    }

    #[test]
    fn integer_array_reads_one_width_per_value() {
        let mut w = W::default();
        w.bit(false).uint(3).put(3, 8).put(0, 8).put(4, 8);
        w.bit(true).put(3, 2); // -3 in 3 bits
        w.bit(false).put(5, 3); // 5 in 4 bits
        assert_eq!(read(&w, compressed_integer_array), [-3, 0, 5]);
    }

    #[test]
    fn indice_array_sums_widths_and_deltas() {
        // Values 5, 6, 4: widths 4, 2, 3 stored as diffs 4, -2, +1.
        let mut w = W::default();
        w.bit(false).uint(3).put(4, 8).put(0xfe, 8).put(1, 8);
        w.bit(false).put(5, 3); // 5
        w.bit(false).put(1, 1); // +1
        w.bit(true).put(2, 2); // -2
        assert_eq!(read(&w, |r| compressed_indice_array(r, None)), [5, 6, 4]);
        // The implicit-flag form reads no flag bit.
        let mut i = W::default();
        i.uint(1).put(2, 8).bit(false).put(1, 1);
        assert_eq!(read(&i, |r| compressed_indice_array(r, Some(false))), [1]);
        // A negative running index is damage.
        let mut n = W::default();
        n.bit(false).uint(1).put(2, 8).bit(true).put(1, 1);
        let bytes = n.bytes();
        assert!(compressed_indice_array(&mut BitReader::new(&bytes), None).is_err());
    }

    #[test]
    fn a_bool_array_longer_than_the_data_is_truncated() {
        assert!(matches!(bool_array(&mut BitReader::new(&[0]), 9), Err(PrcError::Truncated(_))));
        assert_eq!(bool_array(&mut BitReader::new(&[0x80]), 2).unwrap(), [true, false]);
    }

    #[test]
    fn decoded_elements_are_charged_against_one_allowance() {
        // One bit can decode to a 4-byte element, so the byte cap alone does not bound memory.
        let leaves = [(0b111110, 0b10, 2), (1, 0b11, 2)];
        let mut w = W::default();
        w.huffman(6, 2, &leaves, &[62, 1, 62]);
        let bytes = w.bytes();
        let mut r = BitReader::with_elements(&bytes, 2);
        assert!(matches!(huffman(&mut r, 6, false), Err(PrcError::TooManyElements { .. })));
        let mut r = BitReader::with_elements(&bytes, 4);
        assert_eq!(huffman(&mut r, 6, false).unwrap(), [62, 1, 62]);
        assert_eq!(r.elements_left(), 1);
        assert!(matches!(bool_array(&mut BitReader::with_elements(&[0], 1), 2), Err(PrcError::TooManyElements { .. })));
    }

    /// Uncompressed character and short arrays are charged against the allowance too, and a
    /// character count past the data is refused by name before anything is read.
    #[test]
    fn uncompressed_arrays_are_charged_and_their_counts_checked() {
        let mut w = W::default();
        w.uint(3).put(0, 8).put(1, 8).put(2, 8);
        let chars = w.bytes();
        let read = |n| character_array(&mut BitReader::with_elements(&chars, n), 8, Some(false), false);
        assert!(matches!(read(2), Err(PrcError::TooManyElements { .. })));
        assert_eq!(read(3).unwrap(), [0, 1, 2]);
        let mut w = W::default();
        w.bit(false).uint(2).put(1, 8).put(0, 8).put(2, 8).put(0, 8);
        let shorts = w.bytes();
        assert!(matches!(short_array(&mut BitReader::with_elements(&shorts, 1), 8), Err(PrcError::TooManyElements { .. })));
        assert_eq!(short_array(&mut BitReader::with_elements(&shorts, 2), 8).unwrap(), [1, 2]);
        let mut w = W::default();
        w.uint(1000).put(7, 8);
        let short_of_data = w.bytes();
        let res = character_array(&mut BitReader::new(&short_of_data), 8, Some(false), false);
        assert!(matches!(res, Err(PrcError::Truncated("a character array"))), "{res:?}");
    }

    fn malformed_by(res: Result<Vec<i32>, PrcError>, what: &str) -> bool {
        matches!(res, Err(PrcError::Malformed(m)) if m == what)
    }

    #[test]
    fn a_zero_code_length_width_or_length_is_refused_by_name() {
        let mut w = W::default();
        w.huffman_raw(2, 0, &[(1, 0, 0)], &[]);
        let bytes = w.bytes();
        assert!(malformed_by(huffman(&mut BitReader::new(&bytes), 2, false), "Huffman code-length width"));
        let mut w = W::default();
        w.huffman_raw(2, 2, &[(1, 0, 0)], &[]);
        let bytes = w.bytes();
        assert!(malformed_by(huffman(&mut BitReader::new(&bytes), 2, false), "Huffman code length"));
    }

    #[test]
    fn a_boolean_array_past_the_data_is_refused_before_it_is_read() {
        let mut w = W::default();
        w.put(0b1011, 4);
        let bytes = w.bytes();
        let mut r = BitReader::new(&bytes);
        let n = r.remaining() + 1;
        assert_eq!(bool_array(&mut r, n), Err(PrcError::Truncated("a boolean array")));
        assert_eq!(r.position(), 0, "nothing read");
    }
}
