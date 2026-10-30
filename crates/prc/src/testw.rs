//! Test-only PRC bit writer: the inverse of [`crate::BitReader`].

#![allow(clippy::expect_used)]

use crate::acof;

/// An MSB-first bit sink for building test streams.
#[derive(Debug, Default, Clone)]
pub(crate) struct W {
    bits: Vec<bool>,
}

impl W {
    /// The low `n` bits of `v` (`n` ≤ 64), most significant first.
    pub(crate) fn put(&mut self, v: u64, n: u32) -> &mut Self {
        for i in (0..n).rev() {
            self.bits.push((v >> i) & 1 == 1);
        }
        self
    }

    /// One bit; a PRC `Boolean`.
    pub(crate) fn bit(&mut self, b: bool) -> &mut Self {
        self.bits.push(b);
        self
    }

    /// Bits written so far.
    pub(crate) fn len(&self) -> usize {
        self.bits.len()
    }

    /// Every bit of `other`, unaligned.
    pub(crate) fn append(&mut self, other: &Self) -> &mut Self {
        self.bits.extend_from_slice(&other.bits);
        self
    }

    /// An `UnsignedInteger` [WD 11.10].
    pub(crate) fn uint(&mut self, mut v: u32) -> &mut Self {
        while v != 0 {
            self.bit(true).put(u64::from(v & 0xff), 8);
            v >>= 8;
        }
        self.bit(false)
    }

    /// An `Integer` [WD 11.11]: two's-complement groups, low first, each followed by a
    /// continue bit.
    pub(crate) fn int(&mut self, v: i32) -> &mut Self {
        if v == 0 {
            return self.bit(false);
        }
        self.bit(true);
        let mut v = i64::from(v);
        loop {
            let group = v & 0xff;
            self.put(group.cast_unsigned(), 8);
            v >>= 8;
            let done = (v == 0 && group & 0x80 == 0) || (v == -1 && group & 0x80 != 0);
            self.bit(!done);
            if done {
                return self;
            }
        }
    }

    /// A `String` [WD 11.4]: presence bit, byte count, raw UTF-8 bytes.
    pub(crate) fn string(&mut self, s: Option<&str>) -> &mut Self {
        let Some(s) = s else {
            return self.bit(false);
        };
        self.bit(true).uint(u32::try_from(s.len()).expect("a short test string"));
        for b in s.bytes() {
            self.put(u64::from(b), 8);
        }
        self
    }

    /// A finite `Double` [WD 11.17]: zero as the two-bit zero code; anything else as its
    /// exponent row, the sign, then the mantissa as literal bytes.
    pub(crate) fn double(&mut self, v: f64) -> &mut Self {
        if v == 0.0 {
            return self.put(0b01, 2);
        }
        let bits = v.to_bits();
        let exponent = u32::try_from((bits >> 52) & 0x7ff).expect("11 bits");
        let (n, c) = acof::exponent_code(exponent).expect("the Double table has a row per exponent");
        let le = bits.to_le_bytes();
        self.put(u64::from(c), n).bit(v < 0.0).bit(true).put(u64::from(le[6] & 0x0f), 4);
        for b in le[..6].iter().rev() {
            self.bit(true).put(u64::from(*b), 8);
        }
        self
    }

    /// A Huffman container [WD 12.2]: `leaves` are `(symbol, stored code, length)`, each value
    /// is emitted as its leaf's code.
    pub(crate) fn huffman(&mut self, nbits: u32, len_width: u32, leaves: &[(u32, u32, u32)], values: &[i32]) -> &mut Self {
        let mask = (1u32 << nbits) - 1;
        let codes: Vec<(u32, u32)> = values
            .iter()
            .map(|&v| {
                let l = leaves.iter().find(|l| l.0 & mask == v.cast_unsigned() & mask).expect("a leaf per value");
                (l.1, l.2)
            })
            .collect();
        self.huffman_raw(nbits, len_width, leaves, &codes)
    }

    /// As [`Self::huffman`] with each element's `(code, length)` given.
    pub(crate) fn huffman_raw(&mut self, nbits: u32, len_width: u32, leaves: &[(u32, u32, u32)], codes: &[(u32, u32)]) -> &mut Self {
        let mut blob = Self::huffman_head(nbits, len_width, leaves);
        lsb(&mut blob, u32::try_from(codes.len()).expect("a short test array"), 32);
        for &(c, l) in codes {
            for i in (0..l).rev() {
                blob.push((c >> i) & 1 == 1);
            }
        }
        self.huffman_words(blob)
    }

    /// A Huffman container whose element count is `count` with no elements written.
    pub(crate) fn huffman_count(&mut self, nbits: u32, len_width: u32, leaves: &[(u32, u32, u32)], count: u32) -> &mut Self {
        let mut blob = Self::huffman_head(nbits, len_width, leaves);
        lsb(&mut blob, count, 32);
        self.huffman_words(blob)
    }

    fn huffman_head(nbits: u32, len_width: u32, leaves: &[(u32, u32, u32)]) -> Vec<bool> {
        let mut blob = Vec::new();
        lsb(&mut blob, u32::try_from(leaves.len()).expect("a short test table"), nbits + 1);
        lsb(&mut blob, len_width, 8);
        for &(s, c, l) in leaves {
            lsb(&mut blob, s, nbits);
            lsb(&mut blob, l, len_width);
            lsb(&mut blob, c, l);
        }
        blob
    }

    /// The payload as whole 32-bit words of LSB-first bytes, then the last word's used bits.
    fn huffman_words(&mut self, mut blob: Vec<bool>) -> &mut Self {
        let bits = blob.len();
        let words = bits.div_ceil(32);
        blob.resize(words * 32, false);
        self.uint(u32::try_from(words).expect("a short test payload"));
        for byte in blob.chunks(8) {
            let b = byte.iter().enumerate().fold(0u64, |a, (i, &x)| a | (u64::from(x) << i));
            self.put(b, 8);
        }
        self.uint(u32::try_from(bits - (words - 1) * 32).expect("at most 32"))
    }

    /// The bits packed MSB-first, the last byte zero-padded.
    pub(crate) fn bytes(&self) -> Vec<u8> {
        self.bits.chunks(8).map(|c| c.iter().enumerate().fold(0u8, |a, (i, &b)| a | (u8::from(b) << (7 - i)))).collect()
    }
}

/// The low `n` bits of `v`, least significant first.
fn lsb(blob: &mut Vec<bool>, v: u32, n: u32) {
    for i in 0..n {
        blob.push((v >> i) & 1 == 1);
    }
}
