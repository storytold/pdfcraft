//! The bit-level writer for PRC compressed sections: the inverse of
//! [`crate::bits::BitReader`] for the types the writer emits [WD 10.3,
//! 11.1-11.17].

use crate::acof;

/// An MSB-first bit sink.
///
/// Every value the writer can be handed is encodable except a non-finite
/// `Double`; one of those is recorded and reported by [`Self::finish`]
/// rather than written.
#[derive(Debug, Default, Clone)]
pub(crate) struct BitWriter {
    bytes: Vec<u8>,
    len: usize,
    non_finite: bool,
}

impl BitWriter {
    /// The low `n` bits of `v` (`n` ≤ 64), most significant first.
    pub(crate) fn put(&mut self, v: u64, n: u32) -> &mut Self {
        for i in (0..n).rev() {
            self.bit((v >> i) & 1 == 1);
        }
        self
    }

    /// One bit; a PRC `Boolean`.
    pub(crate) fn bit(&mut self, b: bool) -> &mut Self {
        if self.len.is_multiple_of(8) {
            self.bytes.push(0);
        }
        if b && let Some(last) = self.bytes.last_mut() {
            *last |= 0x80 >> (self.len % 8);
        }
        self.len += 1;
        self
    }

    /// Every bit of `other`, unaligned.
    #[cfg(test)]
    pub(crate) fn append(&mut self, other: &Self) -> &mut Self {
        for i in 0..other.len {
            let byte = other.bytes.get(i / 8).copied().unwrap_or(0);
            self.bit((byte >> (7 - i % 8)) & 1 == 1);
        }
        self.non_finite |= other.non_finite;
        self
    }

    /// An `UnsignedInteger` [WD 11.2]: per byte, a continuation bit then
    /// eight bits, low byte first; a final zero bit.
    pub(crate) fn uint(&mut self, mut v: u32) -> &mut Self {
        while v != 0 {
            self.bit(true).put(u64::from(v & 0xff), 8);
            v >>= 8;
        }
        self.bit(false)
    }

    /// An `Integer` [WD 11.3]: two's-complement bytes, low first, each
    /// followed by a continue bit.
    pub(crate) fn int(&mut self, v: i32) -> &mut Self {
        if v == 0 {
            return self.bit(false);
        }
        self.bit(true);
        let mut v = i64::from(v);
        loop {
            let byte = (v & 0xff) as u8;
            self.put(u64::from(byte), 8);
            v >>= 8;
            let done = (v == 0 && byte & 0x80 == 0) || (v == -1 && byte & 0x80 != 0);
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
        self.bit(true).uint(u32::try_from(s.len()).unwrap_or(u32::MAX));
        for b in s.bytes() {
            self.put(u64::from(b), 8);
        }
        self
    }

    /// A `Double` [WD 11.17]: ±0 as the two-bit zero code; any other finite
    /// value as its exponent row, the sign, then the mantissa as literal
    /// bytes, never as a back-reference to a previous double (the
    /// simplest form a reader can take; the writer never needs the others).
    pub(crate) fn double(&mut self, v: f64) -> &mut Self {
        if v == 0.0 {
            return self.put(0b01, 2);
        }
        let bits = v.to_bits();
        let Some((n, c)) = v.is_finite().then(|| acof::exponent_code(((bits >> 52) & 0x7ff) as u32)).flatten() else {
            self.non_finite = true;
            return self.put(0b01, 2);
        };
        let le = bits.to_le_bytes();
        self.put(u64::from(c), n).bit(v < 0.0).bit(true).put(u64::from(le[6] & 0x0f), 4);
        for &b in le[..6].iter().rev() {
            self.bit(true).put(u64::from(b), 8);
        }
        self
    }

    /// `n` doubles.
    pub(crate) fn doubles(&mut self, vs: impl IntoIterator<Item = f64>) -> &mut Self {
        for v in vs {
            self.double(v);
        }
        self
    }

    /// The packed bytes, or `None` when a non-finite double was written.
    pub(crate) fn finish(self) -> Option<Vec<u8>> {
        (!self.non_finite).then_some(self.bytes)
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use crate::bits::BitReader;

    #[test]
    fn every_type_reads_back() {
        let values = [1.0, -0.5, 1e-300, 123_456.789, -6.02e23, 0.0, 2000.0];
        let mut w = BitWriter::default();
        w.uint(0).uint(300).int(-129).int(4).string(Some("glass"));
        w.string(None).doubles(values);
        let bytes = w.finish().unwrap();
        let mut r = BitReader::new(&bytes);
        assert_eq!(r.unsigned_integer().unwrap(), 0);
        assert_eq!(r.unsigned_integer().unwrap(), 300);
        assert_eq!(r.integer().unwrap(), -129);
        assert_eq!(r.integer().unwrap(), 4);
        assert_eq!(r.string().unwrap().as_deref(), Some("glass"));
        assert_eq!(r.string().unwrap(), None);
        for v in values {
            assert_eq!(r.double().unwrap(), v);
        }
    }

    #[test]
    fn a_non_finite_double_fails_the_section() {
        let mut w = BitWriter::default();
        w.double(f64::NAN);
        assert!(w.finish().is_none());
        let mut inner = BitWriter::default();
        inner.double(f64::INFINITY);
        let mut w = BitWriter::default();
        w.append(&inner);
        assert!(w.finish().is_none());
    }
}
