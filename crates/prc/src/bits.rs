//! The bit-level reader for PRC compressed sections [WD 10.3, 11.1-11.17].
//!
//! Values are consumed MSB-first from each inflated byte and straddle byte boundaries freely
//! [WD 10.3.1, 11.3]. Multi-byte integers are groups of eight bits, low group first. Two sign
//! conventions coexist: `Integer` is two's complement, the `*WithVariableBitNumber` types are
//! sign-magnitude.

use crate::PrcError;
use crate::acof::{self, Entry};

/// The most array elements the readers of one PRC stream decode together (2^24): at most 8
/// bytes each, so 128 MiB.
pub const MAX_DECODED_ELEMENTS: usize = 1 << 24;

/// The most schema-program steps (tokens executed, loop iterations included) the readers of
/// one PRC stream run together.
pub const MAX_SCHEMA_STEPS: u64 = 1 << 22;

/// What the readers of one stream may still spend, carried from each reader to the next.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Budget {
    /// Array elements still to decode.
    pub(crate) elements: usize,
    /// Schema-program steps still to run.
    pub(crate) steps: u64,
}

impl Budget {
    /// A whole stream's allowance.
    pub(crate) const FULL: Self = Self { elements: MAX_DECODED_ELEMENTS, steps: MAX_SCHEMA_STEPS };
}

/// A cursor over one inflated section.
#[derive(Debug, Clone)]
pub struct BitReader<'a> {
    data: &'a [u8],
    pos: usize,
    budget: Budget,
}

impl<'a> BitReader<'a> {
    /// A reader positioned at the first bit of `data`, allowed [`MAX_DECODED_ELEMENTS`] and
    /// [`MAX_SCHEMA_STEPS`].
    pub fn new(data: &'a [u8]) -> Self {
        Self::with_budget(data, Budget::FULL)
    }

    /// A reader of `data` allowed `budget`: what an earlier reader of the same stream left.
    pub(crate) fn with_budget(data: &'a [u8], budget: Budget) -> Self {
        Self { data, pos: 0, budget }
    }

    /// A reader of `data` allowed `elements` array elements and a full step allowance.
    #[cfg(test)]
    pub(crate) fn with_elements(data: &'a [u8], elements: usize) -> Self {
        Self::with_budget(data, Budget { elements, ..Budget::FULL })
    }

    /// This reader, at its position, allowed `budget` instead: what readers of the same
    /// stream left since it stopped.
    pub(crate) fn with_remaining(mut self, budget: Budget) -> Self {
        self.budget = budget;
        self
    }

    /// What this reader may still spend.
    pub(crate) fn budget(&self) -> Budget {
        self.budget
    }

    /// Array elements this reader may still decode.
    #[cfg(test)]
    pub(crate) fn elements_left(&self) -> usize {
        self.budget.elements
    }

    /// Charge `n` decoded array elements against the allowance.
    ///
    /// # Errors
    /// [`PrcError::TooManyElements`] when fewer than `n` are left.
    pub(crate) fn take_elements(&mut self, n: usize) -> Result<(), PrcError> {
        self.budget.elements = self.budget.elements.checked_sub(n).ok_or(PrcError::TooManyElements { limit: MAX_DECODED_ELEMENTS })?;
        Ok(())
    }

    /// Charge one schema-program step; false when none is left.
    pub(crate) fn take_step(&mut self) -> bool {
        match self.budget.steps.checked_sub(1) {
            Some(left) => {
                self.budget.steps = left;
                true
            }
            None => false,
        }
    }

    /// Bits consumed so far.
    pub fn position(&self) -> usize {
        self.pos
    }

    /// Bits left before the end of the data.
    pub fn remaining(&self) -> usize {
        self.data.len().saturating_mul(8).saturating_sub(self.pos)
    }

    /// Step over `n` bits of opaque data (`UserData` [WD 8.6]).
    ///
    /// # Errors
    /// [`PrcError::Truncated`] when fewer than `n` bits remain; the cursor does not move.
    pub fn skip_bits(&mut self, n: usize) -> Result<(), PrcError> {
        if n > self.remaining() {
            return Err(PrcError::Truncated("user data"));
        }
        self.pos = self.pos.saturating_add(n);
        Ok(())
    }

    /// One bit; a PRC `Boolean` [WD 10.3.3].
    ///
    /// # Errors
    /// [`PrcError::Truncated`] at the end of the data.
    pub fn bit(&mut self) -> Result<bool, PrcError> {
        let byte = self.data.get(self.pos / 8).ok_or(PrcError::Truncated("a bit"))?;
        let b = (byte >> (7 - (self.pos % 8))) & 1 == 1;
        // `pos / 8` indexed the data, so `pos + 1` is at most `8 * len`.
        self.pos = self.pos.saturating_add(1);
        Ok(b)
    }

    /// `n` bits (at most 32), MSB-first; `UnsignedIntegerWithVariableBitNumber` [WD 11.13].
    ///
    /// # Errors
    /// [`PrcError::Truncated`] at the end of the data; [`PrcError::Malformed`] for `n > 32`.
    pub fn bits(&mut self, n: u32) -> Result<u32, PrcError> {
        if n > 32 {
            return Err(PrcError::Malformed(format!("{n}-bit field")));
        }
        if self.remaining() < n as usize {
            return Err(PrcError::Truncated("a bit field"));
        }
        let mut v: u64 = 0;
        for _ in 0..n {
            v = (v << 1) | u64::from(self.bit()?);
        }
        // n <= 32, so v fits.
        Ok(u32::try_from(v).unwrap_or(u32::MAX))
    }

    fn byte(&mut self) -> Result<u8, PrcError> {
        Ok(u8::try_from(self.bits(8)?).unwrap_or(u8::MAX))
    }

    /// A `Character`: one byte [WD 10.3.4].
    ///
    /// # Errors
    /// [`PrcError::Truncated`].
    pub fn character(&mut self) -> Result<u8, PrcError> {
        self.byte()
    }

    /// An `UnsignedInteger` [WD 11.10]: while a 1 bit precedes it, one more eight-bit group,
    /// low group first.
    ///
    /// # Errors
    /// [`PrcError::Truncated`]; [`PrcError::Malformed`] past four groups.
    pub fn unsigned_integer(&mut self) -> Result<u32, PrcError> {
        let mut v: u32 = 0;
        let mut shift = 0;
        while self.bit()? {
            if shift >= 32 {
                return Err(PrcError::Malformed("UnsignedInteger over 32 bits".into()));
            }
            v |= u32::from(self.byte()?) << shift;
            shift += 8;
        }
        Ok(v)
    }

    /// An `Integer` [WD 11.11]: a 0 bit is zero; otherwise eight-bit groups, low group first,
    /// each followed by a continue bit, sign-extended from the last group's top bit.
    ///
    /// # Errors
    /// [`PrcError::Truncated`]; [`PrcError::Malformed`] past four groups.
    pub fn integer(&mut self) -> Result<i32, PrcError> {
        if !self.bit()? {
            return Ok(0);
        }
        let mut v: u32 = 0;
        let mut shift = 0;
        loop {
            if shift >= 32 {
                return Err(PrcError::Malformed("Integer over 32 bits".into()));
            }
            let group = self.byte()?;
            v |= u32::from(group) << shift;
            shift += 8;
            if !self.bit()? {
                if group & 0x80 != 0 && shift < 32 {
                    v |= u32::MAX << shift;
                }
                return Ok(v.cast_signed());
            }
        }
    }

    /// An `IntegerWithVariableBitNumber(n)` [WD 11.12]: a negative flag (1 = negative; the WD
    /// prose says the opposite, its sample code and real files agree on this), then the
    /// magnitude in `n - 1` bits.
    ///
    /// # Errors
    /// [`PrcError::Truncated`]; [`PrcError::Malformed`] for `n > 33`.
    pub fn integer_vbn(&mut self, n: u32) -> Result<i64, PrcError> {
        let Some(magnitude_bits) = n.checked_sub(1) else {
            return Ok(0);
        };
        let negative = self.bit()?;
        let m = i64::from(self.bits(magnitude_bits)?);
        Ok(if negative { -m } else { m })
    }

    /// A `DoubleWithVariableBitNumber(tolerance, n)` [WD 11.14]: a sign-magnitude quantum
    /// count times `tolerance`.
    ///
    /// # Errors
    /// As [`Self::integer_vbn`].
    pub fn double_vbn(&mut self, tolerance: f64, n: u32) -> Result<f64, PrcError> {
        Ok(self.integer_vbn(n)? as f64 * tolerance)
    }

    /// `NumberOfBitsThenUnsignedInteger` [WD 11.15]: a 5-bit width, then the value in that
    /// many bits.
    ///
    /// # Errors
    /// [`PrcError::Truncated`].
    pub fn nbits_then_unsigned(&mut self) -> Result<u32, PrcError> {
        let n = self.bits(5)?;
        self.bits(n)
    }

    /// A `String` [WD 10.3.7, 11.4]: `None` for the null string, else UTF-8 (invalid
    /// sequences replaced).
    ///
    /// # Errors
    /// [`PrcError::Truncated`] when the byte count runs past the data.
    pub fn string(&mut self) -> Result<Option<String>, PrcError> {
        if !self.bit()? {
            return Ok(None);
        }
        let len = self.unsigned_integer()? as usize;
        if len.saturating_mul(8) > self.remaining() {
            return Err(PrcError::Truncated("a String"));
        }
        let bytes = (0..len).map(|_| self.byte()).collect::<Result<Vec<u8>, _>>()?;
        Ok(Some(String::from_utf8_lossy(&bytes).into_owned()))
    }

    /// `FloatAsBytes` [WD 11.5]: an IEEE binary32, least-significant byte first.
    ///
    /// # Errors
    /// [`PrcError::Truncated`].
    pub fn float_as_bytes(&mut self) -> Result<f32, PrcError> {
        let b = [self.byte()?, self.byte()?, self.byte()?, self.byte()?];
        Ok(f32::from_le_bytes(b))
    }

    /// A `Double` [WD 11.17]: a prefix code from the Double table, then a sign bit and, for
    /// an exponent code, a byte-wise mantissa with back-references.
    ///
    /// The two-bit zero code has no sign bit. Back-reference offsets 1 to 5 are all accepted,
    /// distance 1 included (PDF Association pdf-issues #769).
    ///
    /// # Errors
    /// [`PrcError::UnknownDoubleCode`], [`PrcError::Truncated`], or [`PrcError::Malformed`]
    /// for a back-reference outside the value.
    pub fn double(&mut self) -> Result<f64, PrcError> {
        // prc-rs: the decoding steps (code lookup, sign, exponent, byte-wise mantissa with
        // back-references) follow prc-rs's.
        let mut code = 0u32;
        let mut entry = None;
        for n in 1..=acof::MAX_CODE_BITS {
            code = (code << 1) | u32::from(self.bit()?);
            if let Some(e) = acof::lookup(n, code) {
                if n == 2 && code == 0b01 {
                    return Ok(0.0);
                }
                entry = Some(e);
                break;
            }
        }
        let entry = entry.ok_or(PrcError::UnknownDoubleCode)?;
        let negative = self.bit()?;
        let upper = match entry {
            Entry::Double(v) => return Ok(if negative { -v } else { v }),
            Entry::Exponent(upper) => upper,
        };
        let mut b = (u64::from(upper) << 32).to_le_bytes();
        if self.bit()? {
            self.mantissa(&mut b)?;
        }
        let sign = if negative { 1u64 << 63 } else { 0 };
        Ok(f64::from_bits(u64::from_le_bytes(b) | sign))
    }

    /// Mantissa bits 51..0 into `b` (little-endian byte order): a nibble, then bytes 5 down
    /// to 0, each a literal or a back-reference to a byte already set.
    fn mantissa(&mut self, b: &mut [u8; 8]) -> Result<(), PrcError> {
        let nibble = u8::try_from(self.bits(4)?).unwrap_or(0);
        let [.., b6, _] = b;
        *b6 |= nibble;
        let at = |b: &[u8; 8], i: usize| b.get(i).copied().ok_or_else(|| PrcError::Malformed("Double back-reference past the value".into()));
        for cbi in (0..=5usize).rev() {
            let prev = at(b, cbi + 1)?;
            let value = if self.bit()? {
                self.byte()?
            } else {
                match self.bits(3)? {
                    // Fill this byte and every lower one with the previous byte.
                    0 => {
                        b.iter_mut().take(cbi + 1).for_each(|x| *x = prev);
                        return Ok(());
                    }
                    // As 0, but the lowest byte is a literal.
                    6 => {
                        b.iter_mut().take(cbi + 1).skip(1).for_each(|x| *x = prev);
                        let low = self.byte()?;
                        let [b0, ..] = b;
                        *b0 = low;
                        return Ok(());
                    }
                    off @ 1..=5 => at(b, cbi + off as usize)?,
                    _ => return Err(PrcError::Malformed("Double mantissa offset 7".into())),
                }
            };
            if let Some(x) = b.get_mut(cbi) {
                *x = value;
            }
        }
        Ok(())
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use crate::testw::W;

    /// The code for IEEE exponent `e`.
    fn exponent_code(e: u32) -> (u32, u32) {
        acof::exponent_code(e).expect("the table has a row per exponent")
    }

    fn read_double(w: &W) -> f64 {
        BitReader::new(&w.bytes()).double().unwrap()
    }

    #[test]
    fn zero_is_the_two_bit_code_with_no_sign() {
        let mut w = W::default();
        w.put(0b01, 2).put(0b1111_1111, 8);
        let bytes = w.bytes();
        let mut r = BitReader::new(&bytes);
        assert_eq!(r.double().unwrap(), 0.0);
        assert_eq!(r.position(), 2);
    }

    #[test]
    fn exponent_with_literal_mantissa() {
        let v = -1234.5678f64;
        let le = v.to_bits().to_le_bytes();
        let e = u32::try_from((v.to_bits() >> 52) & 0x7ff).unwrap();
        let (n, c) = exponent_code(e);
        let mut w = W::default();
        w.put(u64::from(c), n).put(1, 1).put(1, 1).put(u64::from(le[6] & 0x0f), 4);
        for byte in le[..6].iter().rev() {
            w.put(1, 1).put(u64::from(*byte), 8);
        }
        assert_eq!(read_double(&w), v);
    }

    #[test]
    fn exponent_without_mantissa_is_a_power_of_two() {
        let (n, c) = exponent_code(1023 + 3);
        let mut w = W::default();
        w.put(u64::from(c), n).put(0, 1).put(0, 1);
        assert_eq!(read_double(&w), 8.0);
    }

    #[test]
    fn fill_back_reference_and_literal_low_byte() {
        // Mantissa bytes 6..0 = 0x05, 0xAA, 0xAA (offset 1), 0xAA (offset 2), fill 0xAA to byte 1, byte 0 = 0x11.
        let (n, c) = exponent_code(1023);
        let mut w = W::default();
        w.put(u64::from(c), n).put(0, 1).put(1, 1).put(0x5, 4);
        w.put(1, 1).put(0xAA, 8);
        w.put(0, 1).put(1, 3);
        w.put(0, 1).put(2, 3);
        w.put(0, 1).put(6, 3).put(0x11, 8);
        assert_eq!(read_double(&w), f64::from_le_bytes([0x11, 0xAA, 0xAA, 0xAA, 0xAA, 0xAA, 0xF5, 0x3F]));

        // Byte 5 literal, then fill 4..0.
        let mut w = W::default();
        w.put(u64::from(c), n).put(0, 1).put(1, 1).put(0x5, 4);
        w.put(1, 1).put(0x77, 8).put(0, 1).put(0, 3);
        assert_eq!(read_double(&w), f64::from_le_bytes([0x77, 0x77, 0x77, 0x77, 0x77, 0x77, 0xF5, 0x3F]));
    }

    #[test]
    fn a_back_reference_past_the_value_is_refused() {
        let (n, c) = exponent_code(1023);
        let mut w = W::default();
        w.put(u64::from(c), n).put(0, 1).put(1, 1).put(0, 4);
        w.put(0, 1).put(5, 3).put(0, 16); // byte 5 + offset 5 = byte 10
        assert!(matches!(BitReader::new(&w.bytes()).double(), Err(PrcError::Malformed(_))));
    }

    #[test]
    fn a_truncated_double_is_an_error_not_a_panic() {
        // No code is 8 bits or shorter and a prefix of 0x17, so one byte ends inside a code.
        assert!(matches!(BitReader::new(&[0x17]).double(), Err(PrcError::Truncated(_))));
        assert!(matches!(BitReader::new(&[]).double(), Err(PrcError::Truncated(_))));
    }

    #[test]
    fn integers_and_strings() {
        let mut w = W::default();
        // UnsignedInteger 300 = groups 0x2C, 0x01.
        w.put(1, 1).put(0x2C, 8).put(1, 1).put(0x01, 8).put(0, 1);
        // Integer -2 = group 0xFE, stop.
        w.put(1, 1).put(0xFE, 8).put(0, 1);
        // Integer 200 = 0xC8 then 0x00 (top bit clear), stop.
        w.put(1, 1).put(0xC8, 8).put(1, 1).put(0x00, 8).put(0, 1);
        // IntegerWithVariableBitNumber(4) = -5.
        w.put(1, 1).put(5, 3);
        // String "hé".
        w.put(1, 1).put(1, 1).put(3, 8).put(0, 1).put(0x68, 8).put(0xC3, 8).put(0xA9, 8);
        // Null string.
        w.put(0, 1);
        let bytes = w.bytes();
        let mut r = BitReader::new(&bytes);
        assert_eq!(r.unsigned_integer().unwrap(), 300);
        assert_eq!(r.integer().unwrap(), -2);
        assert_eq!(r.integer().unwrap(), 200);
        assert_eq!(r.integer_vbn(4).unwrap(), -5);
        assert_eq!(r.string().unwrap().as_deref(), Some("hé"));
        assert_eq!(r.string().unwrap(), None);
    }

    #[test]
    fn the_writer_round_trips_through_the_reader() {
        let mut w = W::default();
        w.uint(0).uint(u32::MAX).int(-129).int(i32::MIN).int(i32::MAX).string(Some("glass")).string(None);
        let bytes = w.bytes();
        let mut r = BitReader::new(&bytes);
        assert_eq!(r.unsigned_integer().unwrap(), 0);
        assert_eq!(r.unsigned_integer().unwrap(), u32::MAX);
        assert_eq!(r.integer().unwrap(), -129);
        assert_eq!(r.integer().unwrap(), i32::MIN);
        assert_eq!(r.integer().unwrap(), i32::MAX);
        assert_eq!(r.string().unwrap().as_deref(), Some("glass"));
        assert_eq!(r.string().unwrap(), None);
    }

    #[test]
    fn oversized_groups_and_widths_are_malformed() {
        // Five UnsignedInteger groups.
        let mut w = W::default();
        (0..5).for_each(|_| {
            w.put(1, 1).put(0xFF, 8);
        });
        w.put(0, 1);
        assert!(matches!(BitReader::new(&w.bytes()).unsigned_integer(), Err(PrcError::Malformed(_))));
        assert!(matches!(BitReader::new(&[0; 8]).bits(33), Err(PrcError::Malformed(_))));
        assert!(matches!(BitReader::new(&[0; 8]).integer_vbn(34), Err(PrcError::Malformed(_))));
        assert_eq!(BitReader::new(&[]).integer_vbn(0), Ok(0));
    }

    #[test]
    fn truncation_is_an_error_not_a_panic() {
        let mut r = BitReader::new(&[0xFF]);
        assert!(r.unsigned_integer().is_err());
        let mut r = BitReader::new(&[0xC0, 0x40]);
        assert!(matches!(r.string(), Err(PrcError::Truncated(_))));
        let mut r = BitReader::new(&[0xAB]);
        assert!(matches!(r.skip_bits(9), Err(PrcError::Truncated(_))));
        assert_eq!(r.position(), 0, "a refused skip does not move the cursor");
        r.skip_bits(8).unwrap();
        assert_eq!(r.remaining(), 0);
    }
}
