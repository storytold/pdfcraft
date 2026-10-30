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

    /// The bits packed MSB-first, the last byte zero-padded.
    pub(crate) fn bytes(&self) -> Vec<u8> {
        self.bits.chunks(8).map(|c| c.iter().enumerate().fold(0u8, |a, (i, &b)| a | (u8::from(b) << (7 - i)))).collect()
    }
}
