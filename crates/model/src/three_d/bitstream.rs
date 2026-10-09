//! ECMA-363 §10: bounded inverse of the normative arithmetic encoding procedure.
use super::{Result, invalid};
use std::collections::BTreeMap;
const MAX_SYMBOLS: usize = super::MAX_DECODE_WORK;
const MAX_HISTOGRAM_CELLS: usize = 1_048_576;
#[derive(Default)]
struct Histogram {
    bins: Vec<u16>,
    tree: Vec<u32>,
    total: u32,
}
impl Histogram {
    fn new() -> Self {
        Self { bins: vec![1], tree: vec![0, 1], total: 1 }
    }
    fn rebuild(&mut self) {
        self.tree = vec![0; self.bins.len() + 1];
        self.total = 0;
        for (index, value) in self.bins.iter().enumerate() {
            self.total += u32::from(*value);
            if let Some(cell) = self.tree.get_mut(index + 1) {
                *cell = u32::from(*value);
            }
        }
        for index in 1..self.tree.len() {
            let value = self.tree.get(index).copied().unwrap_or(0);
            if let Some(parent) = self.tree.get_mut(index + index.isolate_lowest_one()) {
                *parent += value;
            }
        }
    }
    fn select(&self, frequency: u32) -> Result<(u32, u32, u32)> {
        let mut index = 0usize;
        let mut sum = 0u32;
        let mut step = self.bins.len().next_power_of_two();
        while step > 0 {
            let next = index + step;
            if let Some(value) = self.tree.get(next)
                && sum + value <= frequency
            {
                index = next;
                sum += value;
            }
            step >>= 1;
        }
        let value = self.bins.get(index).copied().filter(|v| *v > 0).ok_or_else(|| invalid("invalid U3D arithmetic histogram"))?;
        Ok((index as u32, sum, u32::from(value)))
    }
    fn add(&mut self, symbol: u32, cells: &mut usize, work: &mut usize) -> Result<()> {
        if symbol > 65535 {
            return Ok(());
        }
        let index = symbol as usize;
        if index >= self.bins.len() {
            let size = (index + 1).next_power_of_two();
            *cells = cells.checked_add(size - self.bins.len()).ok_or_else(|| invalid("U3D histogram size overflows"))?;
            if *cells > MAX_HISTOGRAM_CELLS {
                return Err(invalid("U3D histogram memory limit exceeded"));
            }
            *work = work.saturating_add(size.saturating_mul(2));
            if *work > MAX_SYMBOLS {
                return Err(invalid("U3D arithmetic work limit exceeded"));
            }
            self.bins.resize(size, 0);
            self.rebuild();
        }
        if self.total >= 8191 {
            *work = work.saturating_add(self.bins.len().saturating_mul(3));
            if *work > MAX_SYMBOLS {
                return Err(invalid("U3D arithmetic work limit exceeded"));
            }
            for value in &mut self.bins {
                *value >>= 1;
            }
            if let Some(escape) = self.bins.first_mut() {
                *escape += 1;
            }
            self.rebuild();
        }
        let cell = self.bins.get_mut(index).ok_or_else(|| invalid("invalid U3D symbol"))?;
        *cell += 1;
        self.total += 1;
        let mut i = index + 1;
        while let Some(cell) = self.tree.get_mut(i) {
            *cell += 1;
            i += i.isolate_lowest_one();
        }
        Ok(())
    }
}
/// Every U3D block starts a fresh arithmetic state and fresh dynamic contexts.
pub(super) struct Decoder<'a> {
    bytes: &'a [u8],
    bit: usize,
    low: u32,
    high: u32,
    pending: usize,
    histograms: BTreeMap<u16, Histogram>,
    cells: usize,
    work: usize,
    pub no_compression: bool,
    budget: std::rc::Rc<super::DecodeBudget>,
}
impl<'a> Decoder<'a> {
    #[cfg(test)]
    pub fn new(bytes: &'a [u8], no_compression: bool) -> Self {
        Self::with_budget(bytes, no_compression, std::rc::Rc::new(super::DecodeBudget::default()))
    }
    pub fn with_budget(bytes: &'a [u8], no_compression: bool, budget: std::rc::Rc<super::DecodeBudget>) -> Self {
        Self { bytes, bit: 0, low: 0, high: 65535, pending: 0, histograms: BTreeMap::new(), cells: 0, work: 0, no_compression, budget }
    }
    fn bit_at(&self, index: usize) -> Result<u32> {
        // The decoder's 16-bit lookahead is zero-padded. Consumed bits must still
        // lie within the block; callers never invent payload bytes after EOF.
        if index > self.bytes.len().saturating_mul(8).saturating_add(15) {
            return Err(invalid("truncated U3D arithmetic data"));
        }
        Ok(self.bytes.get(index / 8).map(|v| u32::from((v >> (index % 8)) & 1)).unwrap_or(0))
    }
    fn symbol(&mut self, context: Option<u16>, range: u32) -> Result<u32> {
        self.work = self.budget.work.get().saturating_add(1);
        if self.work > MAX_SYMBOLS {
            return Err(invalid("U3D arithmetic work limit exceeded"));
        }
        if self.bit >= self.bytes.len().saturating_mul(8) {
            return Err(invalid("truncated U3D block"));
        }
        let mut code = self.bit_at(self.bit)?;
        let start = self.bit.checked_add(self.pending).and_then(|v| v.checked_add(1)).ok_or_else(|| invalid("U3D bit offset overflows"))?;
        for index in start..start + 15 {
            code = (code << 1) | self.bit_at(index)?;
        }
        if code < self.low || code > self.high {
            return Err(invalid("invalid U3D arithmetic interval"));
        }
        if let Some(context) = context
            && !self.histograms.contains_key(&context)
        {
            self.cells += 1;
            if self.cells > MAX_HISTOGRAM_CELLS {
                return Err(invalid("U3D histogram memory limit exceeded"));
            }
            self.histograms.insert(context, Histogram::new());
        }
        let total = match context {
            Some(key) => self.histograms.get(&key).map(|h| h.total).unwrap_or(1),
            None => range,
        };
        if total == 0 || total > 16382 {
            return Err(invalid("invalid U3D static range"));
        }
        let width = self.high - self.low + 1;
        let frequency = (u64::from(total) * u64::from(code - self.low + 1) - 1) / u64::from(width);
        let (symbol, cumulative, count) = match context {
            Some(key) => self.histograms.get(&key).ok_or_else(|| invalid("missing U3D histogram"))?.select(frequency as u32)?,
            None => (frequency as u32 + 1, frequency as u32, 1),
        };
        let lower = self.low;
        self.high = (lower + (u64::from(width) * u64::from(cumulative + count) / u64::from(total)) as u32)
            .checked_sub(1)
            .ok_or_else(|| invalid("empty U3D arithmetic interval"))?;
        self.low = lower + (u64::from(width) * u64::from(cumulative) / u64::from(total)) as u32;
        if self.low > self.high || self.high > 65535 {
            return Err(invalid("invalid U3D arithmetic interval"));
        }
        if let Some(key) = context {
            self.histograms.get_mut(&key).ok_or_else(|| invalid("missing U3D histogram"))?.add(symbol, &mut self.cells, &mut self.work)?;
        }
        self.budget.work.set(self.work);
        let mut consumed = 0usize;
        while (self.low & 0x8000) == (self.high & 0x8000) {
            self.low = (self.low & 0x7fff) << 1;
            self.high = ((self.high & 0x7fff) << 1) | 1;
            consumed += 1;
        }
        let high_bit_low = self.low & 0x8000;
        let high_bit_high = self.high & 0x8000;
        if consumed > 0 {
            consumed += self.pending;
            self.pending = 0;
        }
        while self.low & 0x4000 != 0 && self.high & 0x4000 == 0 {
            self.low = (self.low & 0x3fff) << 1;
            self.high = ((self.high & 0x3fff) << 1) | 1;
            self.pending += 1;
            if self.pending > 1_048_576 {
                return Err(invalid("U3D arithmetic underflow limit exceeded"));
            }
        }
        self.low |= high_bit_low;
        self.high |= high_bit_high;
        self.bit = self.bit.checked_add(consumed).ok_or_else(|| invalid("U3D bit offset overflows"))?;
        if self.bit > self.bytes.len().saturating_mul(8) {
            return Err(invalid("truncated U3D block"));
        }
        Ok(symbol)
    }
    pub fn u8(&mut self) -> Result<u8> {
        Ok(((self.symbol(None, 256)? - 1) as u8).reverse_bits())
    }
    pub fn u16(&mut self) -> Result<u16> {
        Ok(u16::from_le_bytes([self.u8()?, self.u8()?]))
    }
    pub fn u32(&mut self) -> Result<u32> {
        Ok(u32::from_le_bytes([self.u8()?, self.u8()?, self.u8()?, self.u8()?]))
    }
    pub fn u64(&mut self) -> Result<u64> {
        let low = self.u32()?;
        let high = self.u32()?;
        Ok(u64::from(low) | (u64::from(high) << 32))
    }
    pub fn f32(&mut self) -> Result<f64> {
        let value = f64::from(f32::from_bits(self.u32()?));
        finite(value)
    }
    pub fn f64(&mut self) -> Result<f64> {
        let value = f64::from_bits(self.u64()?);
        finite(value)
    }
    pub fn vector(&mut self) -> Result<[f64; 3]> {
        Ok([self.f32()?, self.f32()?, self.f32()?])
    }
    pub fn string(&mut self) -> Result<String> {
        let count = usize::from(self.u16()?);
        if count > 4096 {
            return Err(invalid("U3D name exceeds 4096 bytes"));
        }
        let mut bytes = Vec::with_capacity(count);
        for _ in 0..count {
            bytes.push(self.u8()?);
        }
        String::from_utf8(bytes).map_err(|_| invalid("U3D name is not UTF-8"))
    }
    pub fn compressed(&mut self, context: u16) -> Result<u32> {
        if self.no_compression {
            return self.u32();
        }
        if context == 0 || context >= 1024 {
            return Err(invalid("invalid U3D dynamic context"));
        }
        let symbol = self.symbol(Some(context), 0)?;
        if symbol != 0 {
            return Ok(symbol - 1);
        }
        let value = self.u32()?;
        if let Some(symbol) = value.checked_add(1) {
            self.histograms.get_mut(&context).ok_or_else(|| invalid("missing U3D histogram"))?.add(symbol, &mut self.cells, &mut self.work)?;
        }
        Ok(value)
    }
    pub fn compressed_u16(&mut self, context: u16) -> Result<u16> {
        if context == 0 || context >= 1024 {
            return Err(invalid("invalid U3D dynamic context"));
        }
        if self.no_compression {
            return self.u16();
        }
        let symbol = self.symbol(Some(context), 0)?;
        if symbol != 0 {
            return u16::try_from(symbol - 1).map_err(|_| invalid("U3D compressed U16 overflows"));
        }
        let value = self.u16()?;
        self.histograms.get_mut(&context).ok_or_else(|| invalid("missing U3D histogram"))?.add(
            u32::from(value) + 1,
            &mut self.cells,
            &mut self.work,
        )?;
        Ok(value)
    }
    pub fn compressed_u8(&mut self, context: u16) -> Result<u8> {
        if context == 0 || context >= 1024 {
            return Err(invalid("invalid U3D dynamic context"));
        }
        if self.no_compression {
            return self.u8();
        }
        let symbol = self.symbol(Some(context), 0)?;
        if symbol != 0 {
            return u8::try_from(symbol - 1).map_err(|_| invalid("U3D compressed U8 overflows"));
        }
        let value = self.u8()?;
        self.histograms.get_mut(&context).ok_or_else(|| invalid("missing U3D histogram"))?.add(
            u32::from(value) + 1,
            &mut self.cells,
            &mut self.work,
        )?;
        Ok(value)
    }
    pub fn index(&mut self, count: usize) -> Result<u32> {
        if count == 0 {
            return Err(invalid("U3D index refers to an empty array"));
        }
        let index = if self.no_compression || count > 16382 { self.u32()? } else { self.symbol(None, count as u32)? - 1 };
        if index as usize >= count {
            return Err(invalid("U3D mesh index is out of bounds"));
        }
        Ok(index)
    }
    pub fn align(&mut self) -> Result<usize> {
        if self.low != 0 || self.high != 65535 || self.pending != 0 {
            return Err(invalid("U3D block alignment follows compressed values"));
        }
        self.bit = self.bit.saturating_add(31) & !31;
        if self.bit > self.bytes.len().saturating_mul(8) {
            return Err(invalid("truncated U3D block padding"));
        }
        Ok(self.bit / 8)
    }
}
fn finite(value: f64) -> Result<f64> {
    if value.is_finite() && value.abs() <= 1e12 { Ok(value) } else { Err(invalid("U3D number is not finite or exceeds 1e12")) }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn uncompressed_bytes_and_numbers_have_standard_order() {
        let mut bytes: Vec<u8> = (0..=255).collect();
        bytes.extend(23.5_f32.to_le_bytes());
        bytes.extend(0.001_f64.to_le_bytes());
        let mut decoder = Decoder::new(&bytes, false);
        for value in 0..=255 {
            assert_eq!(decoder.u8().unwrap(), value);
        }
        assert_eq!(decoder.f32().unwrap(), 23.5);
        assert_eq!(decoder.f64().unwrap(), 0.001);
        assert!(decoder.u8().is_err());
    }
    #[test]
    fn hostile_histograms_and_indices_return_errors() {
        let mut decoder = Decoder::new(&[255; 32], true);
        assert!(decoder.index(3).is_err());
        assert!(decoder.index(0).is_err());
        let mut histogram = Histogram::new();
        let mut cells = MAX_HISTOGRAM_CELLS;
        assert!(histogram.add(65535, &mut cells, &mut 0).is_err());
        assert!(Decoder::new(&[], false).string().is_err());
    }
}

#[cfg(test)]
mod independent_sequences {
    use super::*;
    // Contributor-original values, encoded through the public Apache-2.0 U3D
    // reference API. Includes escapes, reused contexts, static ranges and raw bytes.
    #[test]
    fn independently_encoded_compression_sequence() {
        let bytes: &[u8] = &[
            0, 0, 0, 0, 0, 0, 0, 0, 152, 156, 40, 232, 70, 0, 0, 56, 91, 130, 155, 131, 149, 6, 0, 64, 214, 223, 8, 228, 11, 29, 0, 0, 168, 105, 128,
            11, 0, 91, 21, 0, 32, 229, 109, 237, 172, 196, 148, 0, 0, 164, 63, 100, 207, 60, 247, 27, 0, 0, 85, 88, 183, 147, 48, 219, 5, 0, 64, 194,
            4, 43, 209, 48, 192, 0, 0, 54, 9, 109, 217, 207, 108, 42, 0, 64, 58, 94, 143, 29, 83, 200, 4, 0, 96, 220, 58, 103, 42, 43, 165, 1, 0,
            199, 94, 226, 99, 135, 238, 11, 0, 192, 53, 5, 12, 91, 208, 241, 4, 0, 240, 255, 217, 116, 0, 175, 98, 0, 0, 72, 249, 180, 231, 130, 6,
            13, 0, 160, 19, 186, 252, 144, 105, 17, 5, 0, 136, 159, 130, 182, 31, 118, 123, 1, 0, 236, 247, 216, 158, 165, 156, 87, 0, 160, 65, 23,
            136, 35, 1, 30, 9, 0, 152, 202, 103, 96, 84, 9, 92, 2, 0, 34, 160, 76, 189, 138, 113, 42, 0, 0, 174, 168, 52, 84, 218, 100, 21, 0, 168,
            215, 72, 206, 76, 216, 2, 2, 0, 90, 154, 22, 112, 67, 60, 145, 0, 0, 9, 182, 163, 187, 145, 142, 16, 0, 72, 13, 175, 29, 14, 240, 6, 1,
            0, 34, 41, 36, 118, 152, 150, 199, 0, 0, 239, 240, 9, 99, 161, 45, 1, 0, 0, 118, 142, 74, 12, 156, 72, 1, 0, 92, 211, 211, 83, 58, 228,
            106, 2, 0, 3, 19, 117, 248, 100, 224, 41, 0, 64, 252, 51, 116, 248, 209, 243, 28, 0, 164, 70, 112, 176, 25, 243, 122, 2, 0, 25, 0, 98,
            129, 132, 200, 0, 0, 128, 185, 160, 136, 102, 112, 204, 34, 0, 192, 128, 196, 253, 100, 18, 38, 0, 0, 102, 223, 26, 252, 134, 24, 34, 0,
            128, 1, 187, 64, 98, 54, 217, 0, 0, 64, 47, 141, 165, 80, 148, 103, 0, 0, 178, 160, 116, 105, 10, 117, 27, 0, 0, 15, 154, 84, 4, 225,
            228, 0, 0, 192, 4, 86, 86, 5, 1, 168, 5, 0, 64, 128, 148, 24, 78, 113, 252, 1, 0, 29, 224, 192, 241, 129, 86, 45, 0, 64, 96, 2, 249, 122,
            169, 187, 13, 0, 160, 165, 249, 24, 86, 72, 23, 1, 0, 214, 121, 33, 197, 159, 78, 123, 0, 128, 149, 31, 241, 38, 167, 253, 20, 0, 192,
            126, 214, 113, 25, 180, 101, 12, 0, 32, 230, 52, 233, 19, 24, 142, 1, 0, 87, 178, 182, 227, 39, 50, 80, 0, 192, 178, 25, 16, 251, 24, 59,
            24, 0, 160, 116, 69, 159, 76, 240, 214, 0, 0, 201, 61, 205, 76, 79, 130, 192, 0, 128, 23, 231, 45, 226, 32, 236, 50, 0, 96, 130, 61, 24,
            194, 66, 157, 0, 0, 128, 188, 233, 193, 8, 178, 195, 1, 128, 212, 174, 253, 76, 195, 127, 19, 0, 192, 145, 52, 168, 79, 8, 168, 2, 0,
            144, 93, 17, 61, 1, 57, 171, 0, 0, 37, 140, 110, 237, 14, 176, 9, 0, 64, 182, 213, 186, 246, 208, 253, 20, 0, 96, 114, 8, 164, 190, 216,
            59, 5, 0, 48, 120, 90, 94, 139, 165, 161, 2, 128, 92, 147, 25, 122, 160, 101, 15, 0, 32, 50, 56, 208, 254, 241, 22, 29, 0, 224, 113, 249,
            116, 56, 99, 143, 5, 0, 99, 234, 236, 234, 10, 17, 135, 0, 64, 114, 32, 122, 168, 211, 134, 48, 0, 224, 138, 216, 105, 178, 148, 19, 6,
            0, 160, 134, 36, 19, 27, 91, 82, 0, 128, 17, 103, 162, 213, 98, 35, 8, 0, 192, 183, 38, 173, 30, 192, 11, 1, 0, 80, 20, 206, 50, 61, 105,
            95, 0, 0, 163, 142, 32, 31, 142, 124, 15, 0, 64, 123, 199, 121, 176, 73, 62, 4, 0, 96, 66, 164, 202, 141, 129, 6, 0, 0, 133, 137, 126,
            12, 1, 53, 0, 128, 110, 72, 2, 48, 129, 179, 1, 0, 202, 253, 88, 190, 144, 0, 4, 0, 96, 82, 120, 194, 98, 241, 23, 0, 208, 218, 39, 92,
            51, 25, 75, 3, 0, 196, 213, 93, 220, 60, 183, 206, 0, 0, 182, 33, 89, 15, 0, 40, 1, 0, 128, 151, 111, 206, 113, 13, 26, 0, 72, 90, 209,
            158, 67, 16, 118, 0, 192, 232, 86, 190, 154, 109, 156, 1, 0, 158, 47, 214, 208, 40, 138, 1, 0, 182, 55, 64, 14, 137, 230, 211, 0, 128,
            129, 69, 207, 74, 163, 200, 37, 0, 192, 41, 187, 72, 14, 122, 229, 0, 0, 106, 216, 207, 77, 206, 3, 2, 0, 201, 237, 165, 121, 24, 68, 0,
            0, 56, 233, 252, 40, 213, 211, 20, 0, 192, 216, 114, 17, 1, 250, 69, 0, 192, 13, 3, 2, 233, 49, 92, 7, 0, 144, 222, 179, 230, 80, 40,
            133, 5, 0, 88, 59, 56, 102, 8, 199, 19, 0, 0, 45, 3, 5, 136, 96, 155, 0, 32, 20, 151, 72, 19, 213, 247, 3, 0, 113, 144, 230, 242, 132,
            171, 11, 0, 72, 171, 112, 69, 9, 9, 8, 0, 136, 83, 239, 60, 238, 128, 224, 0, 0, 220, 96, 136, 203, 30, 137, 222, 2, 0, 54, 180, 121, 48,
            226, 72, 12, 0, 224, 35, 60, 40, 137, 131, 0, 0, 240, 79, 209, 241, 12, 80, 0, 0, 64, 253, 218, 42, 55, 227, 11, 0, 0, 172, 80, 114, 108,
            232, 86, 0, 0, 226, 172, 7, 163, 9, 185, 0, 0, 208, 80, 147, 38, 5, 141, 0, 0, 128, 125, 72, 115, 31, 153, 9, 0, 0, 64, 224, 88, 175,
            216, 54, 7, 0, 180, 121, 195, 40, 81, 210, 4, 0, 96, 234, 3, 181, 1, 162, 83, 0, 0, 147, 73, 44, 1, 126, 54, 0, 0, 5, 197, 148, 102, 133,
            225, 7, 0, 216, 85, 253, 166, 130, 160, 8, 0, 192, 18, 121, 48, 147, 249, 104, 0, 0, 226, 119, 13, 58, 4, 55, 7, 0, 90, 16, 188, 177, 24,
            134, 18, 0, 112, 128, 164, 198, 134, 210, 108, 0, 128, 84, 10, 129, 6, 93, 165, 0, 128, 202, 133, 89, 14, 132, 130, 0, 0, 244, 108, 55,
            17, 115, 5, 18, 0, 96, 153, 94, 170, 14, 64, 35, 0, 0, 2, 225, 247, 54, 193, 97, 0, 0, 181, 237, 1, 0, 0, 0,
        ];
        let mut d = Decoder::new(bytes, false);
        for i in 0u32..128 {
            assert_eq!(d.compressed_u8(100 + (i % 5) as u16).unwrap(), ((i * 13 + i / 7) % 17) as u8, "u8 at {i}");
            assert_eq!(d.compressed_u16(3 + (i % 3) as u16).unwrap(), ((i * 23 + i / 11) % 93) as u16, "u16 at {i}");
            assert_eq!(d.compressed(22 + (i % 3) as u16).unwrap(), (i * 37 + i / 13) % 1301, "u32 at {i}");
            assert_eq!(d.index(7).unwrap(), (i * 19) % 7, "static at {i}");
            assert_eq!(d.u8().unwrap(), ((i * 17) % 256) as u8, "raw at {i}");
        }
    }
    #[test]
    fn histogram_rescaling_matches_a_linear_frequency_table() {
        let mut histogram = Histogram::new();
        let mut naive = vec![1u32];
        let mut cells = 1;
        let mut work = 0;
        for i in 0usize..25000 {
            let symbol = if i % 19 == 0 { 4097 } else { (i * 13 + i / 7) % 19 };
            if symbol >= naive.len() {
                naive.resize((symbol + 1).next_power_of_two(), 0);
            }
            if naive.iter().sum::<u32>() >= 8191 {
                for n in &mut naive {
                    *n >>= 1;
                }
                naive[0] += 1;
            }
            naive[symbol] += 1;
            histogram.add(symbol as u32, &mut cells, &mut work).unwrap();
            assert_eq!(histogram.total, naive.iter().sum::<u32>());
            if i % 127 == 0 {
                let mut cumulative = 0;
                for (index, count) in naive.iter().enumerate() {
                    if *count > 0 {
                        assert_eq!(histogram.select(cumulative).unwrap(), (index as u32, cumulative, *count));
                        assert_eq!(histogram.select(cumulative + count - 1).unwrap(), (index as u32, cumulative, *count));
                    }
                    cumulative += count;
                }
            }
        }
    }
}
