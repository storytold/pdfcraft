//! FlateDecode (ISO 32000-2 §7.4.4): zlib (RFC 1950) wrapping deflate (RFC 1951).
//!
//! Real-world quirks handled:
//! - the Adler-32 trailer is not required (some producers omit or corrupt it), because the
//!   deflate stream is inflated raw after the 2-byte zlib header;
//! - streams without a valid zlib header are inflated as raw deflate;
//! - data after the end of the deflate stream is ignored.
//!
//! A back-reference to before the start of the output points at nothing, and is damage, as zlib
//! has it: read strictly the stream is refused, and read tolerantly what came before it is kept.
//! (miniz_oxide, flate2's backend here before zlib-rs, copied zeros and went on.) So is a dynamic
//! block whose code has no end-of-block symbol, and nothing of that block is kept.

use std::io::Write;

use flate2::{Compression, Decompress, FlushDecompress, Status};

use crate::{Failure, Step};

const NAME: &str = "FlateDecode";

/// Checks the RFC 1950 header: CM = 8, CINFO ≤ 7 and the FCHECK multiple-of-31 rule.
fn zlib_header(data: &[u8]) -> Option<(u8, u8)> {
    let (&cmf, &flg) = (data.first()?, data.get(1)?);
    let ok = cmf & 0x0f == 8 && cmf >> 4 <= 7 && (u16::from(cmf) << 8 | u16::from(flg)) % 31 == 0;
    ok.then_some((cmf, flg))
}

/// `data` inflated within `max`, and whether it was read twice: a zlib header that only looked
/// valid is read again as raw deflate, within `again`, what a caller paying for what is read has
/// left for it once it has paid for the second reading. `None`, that reading can't be paid for,
/// and is [`FilterError::Spent`](crate::FilterError::Spent).
pub(crate) fn decode(data: &[u8], max: usize, again: Option<usize>) -> (Step, bool) {
    match zlib_header(data) {
        Some((_, flg)) if flg & 0x20 != 0 => (Err(Failure::corrupt(NAME, "zlib preset dictionary is not allowed in PDF", Vec::new())), false),
        Some(_) => match inflate_raw(&data[2..], max) {
            // A header that only looked valid: retry the whole input as raw deflate.
            Err(f) if f.partial.is_empty() && matches!(f.error, crate::FilterError::Corrupt { .. }) => {
                let Some(again) = again else {
                    return (Err(Failure { error: crate::FilterError::Spent, partial: Vec::new(), discarded: 0 }), false);
                };
                let step = match inflate_raw(data, again) {
                    // Keep the original corruption diagnostic only if the retry is corrupt
                    // too. A limit error must remain fatal even for tolerant decoding. What the
                    // retry decoded isn't kept, but a caller charging for work has to hear of it.
                    Err(retry) if matches!(retry.error, crate::FilterError::Corrupt { .. }) => Err(Failure { discarded: retry.partial.len(), ..f }),
                    retry => retry,
                };
                (step, true)
            }
            r => (r, false),
        },
        None => (inflate_raw(data, max), false),
    }
}

#[cfg(test)]
thread_local! {
    /// Bytes inflating gave back on this thread, its partial output included.
    pub(crate) static INFLATED: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

fn inflate_raw(input: &[u8], max: usize) -> Step {
    let step = inflate(input, max);
    #[cfg(test)]
    INFLATED.with(|c| c.set(c.get() + step.as_ref().map_or_else(|f| f.partial.len(), Vec::len)));
    step
}

fn inflate(input: &[u8], max: usize) -> Step {
    let mut d = Decompress::new(false);
    let mut out: Vec<u8> = Vec::new();
    // One byte beyond the limit lets us tell "exactly max" from "more than max".
    let cap = max.saturating_add(1);
    loop {
        if out.len() == out.capacity() {
            if out.len() >= cap {
                return Err(Failure::limit(max));
            }
            let grow = out.capacity().max(input.len().saturating_mul(2)).max(4096).min(cap - out.len());
            out.reserve_exact(grow);
        }
        let consumed = d.total_in() as usize;
        let produced = out.len();
        let res = d.decompress_vec(&input[consumed.min(input.len())..], &mut out, FlushDecompress::None);
        if out.len() > max {
            return Err(Failure::limit(max));
        }
        match res {
            Err(e) => return Err(Failure::corrupt(NAME, e.to_string(), out)),
            Ok(Status::StreamEnd) => return Ok(out),
            Ok(Status::Ok | Status::BufError) => {
                let progressed = d.total_in() as usize != consumed || out.len() != produced;
                let room_left = out.len() < out.capacity();
                if room_left && (d.total_in() as usize >= input.len() || !progressed) {
                    return Err(Failure::corrupt(NAME, "unexpected end of deflate data", out));
                }
            }
        }
    }
}

/// zlib at level 6.
// The documented never-crash exception: writing into a `Vec` cannot fail.
#[allow(clippy::expect_used)]
pub(crate) fn encode(data: &[u8]) -> Vec<u8> {
    let mut e = flate2::write::ZlibEncoder::new(Vec::with_capacity(data.len() / 2 + 64), Compression::new(6));
    e.write_all(data).expect("in-memory write");
    e.finish().expect("in-memory write")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::FilterError;

    fn dec(d: &[u8]) -> Result<Vec<u8>, FilterError> {
        decode(d, 1 << 24, Some(1 << 24)).0.map_err(|f| f.error)
    }

    #[test]
    fn known_zlib_vector() {
        // zlib("a"): header 78 9C, fixed-Huffman block, Adler-32 0x00620062.
        assert_eq!(dec(&[0x78, 0x9c, 0x4b, 0x04, 0x00, 0x00, 0x62, 0x00, 0x62]).unwrap(), b"a");
    }

    #[test]
    fn hand_built_stored_block() {
        // RFC 1951 §3.2.4 stored block: BFINAL=1 BTYPE=00, LEN=5, NLEN=!5, "hello",
        // then Adler-32("hello") = 0x062C0215.
        let v = [0x78, 0x01, 0x01, 0x05, 0x00, 0xfa, 0xff, b'h', b'e', b'l', b'l', b'o', 0x06, 0x2c, 0x02, 0x15];
        assert_eq!(dec(&v).unwrap(), b"hello");
        // Missing Adler-32 is tolerated.
        assert_eq!(dec(&v[..12]).unwrap(), b"hello");
        // Wrong Adler-32 is tolerated (as in other readers).
        let mut bad = v;
        bad[15] ^= 0xff;
        assert_eq!(dec(&bad).unwrap(), b"hello");
        // Trailing garbage after the stream is ignored.
        let mut trailing = v.to_vec();
        trailing.extend_from_slice(b"\r\nendstream");
        assert_eq!(dec(&trailing).unwrap(), b"hello");
    }

    #[test]
    fn raw_deflate_fallback() {
        let data = b"raw deflate without any zlib header, repeated repeated repeated".repeat(4);
        let mut e = flate2::write::DeflateEncoder::new(Vec::new(), Compression::new(6));
        e.write_all(&data).unwrap();
        let raw = e.finish().unwrap();
        assert!(zlib_header(&raw).is_none());
        assert_eq!(dec(&raw).unwrap(), data);
    }

    #[test]
    fn round_trip_and_missing_checksum() {
        let data: Vec<u8> = (0..50_000u32).map(|i| (i * i % 251) as u8).collect();
        let z = encode(&data);
        assert_eq!(&z[..2], &[0x78, 0x9c]);
        assert_eq!(dec(&z).unwrap(), data);
        assert_eq!(dec(&z[..z.len() - 4]).unwrap(), data);
        assert_eq!(dec(&z[..z.len() - 2]).unwrap(), data);
    }

    #[test]
    fn empty_input() {
        assert_eq!(dec(&encode(b"")).unwrap(), b"");
        assert!(matches!(dec(b""), Err(FilterError::Corrupt { .. })));
    }

    #[test]
    fn corrupt_and_truncated() {
        let data: Vec<u8> = (0..10_000u32).map(|i| (i % 13) as u8).collect();
        let z = encode(&data);
        match decode(&z[..z.len() / 2], 1 << 24, Some(1 << 24)).0 {
            Err(f) => {
                assert!(matches!(f.error, FilterError::Corrupt { .. }));
                assert_eq!(&data[..f.partial.len()], &f.partial[..]);
            }
            Ok(_) => panic!("truncated stream decoded"),
        }
        // BTYPE = 11 is reserved (RFC 1951 §3.2.3).
        assert!(matches!(dec(&[0x78, 0x9c, 0x07, 0, 0, 0]), Err(FilterError::Corrupt { .. })));
        // Preset dictionary.
        assert!(matches!(dec(&[0x78, 0x20, 0, 0, 0, 0]), Err(FilterError::Corrupt { .. })));
        // One whose header is followed by data that inflates: refused all the same.
        assert!(matches!(dec(&[0x78, 0xBB, 0x4b, 0x04, 0x00]), Err(FilterError::Corrupt { .. })));
    }

    #[test]
    fn bomb_is_bounded() {
        let z = encode(&vec![0u8; 16 << 20]);
        assert!(z.len() < 32 * 1024);
        assert!(matches!(dec_limit(&z, 1 << 20), Err(FilterError::LimitExceeded(n)) if n == 1 << 20));
        assert_eq!(dec_limit(&z, 16 << 20).unwrap().len(), 16 << 20);
        assert!(matches!(dec_limit(&z, (16 << 20) - 1), Err(FilterError::LimitExceeded(_))));
        assert!(matches!(dec_limit(&z, 0), Err(FilterError::LimitExceeded(0))));
    }

    fn dec_limit(d: &[u8], max: usize) -> Result<Vec<u8>, FilterError> {
        decode(d, max, Some(max)).0.map_err(|f| f.error)
    }

    /// Deflate's bits: header fields and extra bits least significant first, Huffman codes most
    /// significant first (RFC 1951 §3.1.1).
    #[derive(Default)]
    struct Bits {
        out: Vec<u8>,
        acc: u32,
        n: u32,
    }

    impl Bits {
        fn put(&mut self, value: u32, len: u32) {
            for i in 0..len {
                self.acc |= ((value >> i) & 1) << self.n;
                self.n += 1;
                if self.n == 8 {
                    self.out.push(self.acc as u8);
                    (self.acc, self.n) = (0, 0);
                }
            }
        }

        fn code(&mut self, code: u32, len: u32) {
            for i in (0..len).rev() {
                self.put((code >> i) & 1, 1);
            }
        }

        fn finish(mut self) -> Vec<u8> {
            if self.n > 0 {
                self.out.push(self.acc as u8);
            }
            self.out
        }
    }

    /// A dynamic block whose literal and length code has no end-of-block symbol can't end (RFC 1951
    /// §3.2.5): it is refused, and nothing of it is kept.
    #[test]
    fn a_dynamic_code_without_an_end_is_damage() {
        let mut bits = Bits::default();
        bits.put(1, 1); // the final block,
        bits.put(2, 2); // dynamic Huffman codes (§3.2.7)
        bits.put(0, 5); // 257 literal and length codes
        bits.put(0, 5); // 1 distance code
        bits.put(14, 4); // 18 code length codes, in their order up to the one for 1
        for symbol in [16, 17, 18, 0, 8, 7, 9, 6, 10, 5, 11, 4, 12, 3, 13, 2, 14, 1] {
            bits.put(u32::from(symbol == 18 || symbol == 1), 3);
        }
        // Code length codes: 1 is 0, 18 is 1. 'A' and 'B' get a bit each, 256 none.
        let zeros = |bits: &mut Bits, n: u32| {
            bits.code(1, 1);
            bits.put(n - 11, 7);
        };
        zeros(&mut bits, 65);
        bits.code(0, 1);
        bits.code(0, 1);
        zeros(&mut bits, 138);
        zeros(&mut bits, 52);
        bits.code(0, 1); // the distance code: a bit
        for _ in 0..4 {
            bits.code(0, 1); // 'A'
        }
        let data = bits.finish();
        assert!(zlib_header(&data).is_none(), "read as raw deflate");
        match decode(&data, 1 << 20, Some(1 << 20)).0 {
            Err(f) => {
                assert!(matches!(f.error, FilterError::Corrupt { .. }), "{}", f.error);
                assert_eq!(f.partial, b"");
            }
            Ok(out) => panic!("read as {out:?}"),
        }
    }

    /// A back-reference to before the start of the output points at nothing: read strictly the
    /// stream is refused, and what came before it is what a tolerant reading keeps. miniz_oxide,
    /// flate2's backend here before zlib-rs, copied zeros and went on, so this also shows which
    /// backend inflates (the price of reading Flate, `FLATE_READ_WORK`, is set for zlib-rs).
    #[test]
    fn a_copy_from_before_the_start_is_damage() {
        let literal = |bits: &mut Bits, b: u8| bits.code(0x30 + u32::from(b), 8);
        let mut bits = Bits::default();
        bits.put(1, 1); // the final block,
        bits.put(1, 2); // fixed Huffman codes (§3.2.6)
        literal(&mut bits, b'A');
        literal(&mut bits, b'B');
        bits.code(1, 7); // length 3
        bits.code(4, 5); // distance 5 (code 4 and an extra bit of 0), three bytes before the start
        bits.put(0, 1);
        literal(&mut bits, b'C');
        bits.code(0, 7); // end of block
        let data = bits.finish();
        assert!(zlib_header(&data).is_none(), "read as raw deflate");
        match decode(&data, 1 << 20, Some(1 << 20)).0 {
            Err(f) => {
                assert!(matches!(f.error, FilterError::Corrupt { .. }), "{}", f.error);
                assert_eq!(f.partial, b"AB");
            }
            Ok(out) => panic!("read as {out:?}"),
        }
    }
}
