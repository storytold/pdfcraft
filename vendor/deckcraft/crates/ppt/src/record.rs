//! The record framing shared by [MS-PPT] and [MS-ODRAW]: an 8-byte header (version, instance,
//! type, length) followed by `len` bytes, which for a container (version `0xF`) are more records.

use crate::cfb::{u16_at, u32_at};

/// Containers nest at most this deep; deeper input is treated as damaged and skipped.
pub const MAX_DEPTH: usize = 32;

/// One record over borrowed bytes.
#[derive(Clone, Copy, Debug)]
pub struct Rec<'a> {
    pub ver: u8,
    pub instance: u16,
    pub kind: u16,
    /// The record's body (after the header), clipped to the input.
    pub data: &'a [u8],
    /// Offset of the header in the slice it was read from.
    pub offset: usize,
}

impl<'a> Rec<'a> {
    /// The record whose header starts at `at`, its body clipped to `bytes` (a truncated last record
    /// keeps what is there).
    pub fn at(bytes: &'a [u8], at: usize) -> Option<Rec<'a>> {
        let vi = u16_at(bytes, at)?;
        let kind = u16_at(bytes, at.checked_add(2)?)?;
        let len = u32_at(bytes, at.checked_add(4)?)? as usize;
        let start = at.checked_add(8)?;
        let end = start.saturating_add(len).min(bytes.len());
        Some(Rec { ver: (vi & 0xF) as u8, instance: vi >> 4, kind, data: bytes.get(start..end)?, offset: at })
    }

    pub fn is_container(&self) -> bool {
        self.ver == 0xF
    }

    /// The records inside this one; none for an atom.
    pub fn children(&self) -> Records<'a> {
        Records::new(if self.is_container() { self.data } else { &[] })
    }

    /// The first child of type `kind`.
    pub fn child(&self, kind: u16) -> Option<Rec<'a>> {
        self.children().find(|r| r.kind == kind)
    }

    /// Every child of type `kind`.
    pub fn children_of(&self, kind: u16) -> impl Iterator<Item = Rec<'a>> {
        self.children().filter(move |r| r.kind == kind)
    }

    pub fn u8(&self, at: usize) -> Option<u8> {
        self.data.get(at).copied()
    }
    pub fn u16(&self, at: usize) -> Option<u16> {
        u16_at(self.data, at)
    }
    pub fn i16(&self, at: usize) -> Option<i16> {
        self.u16(at).map(|v| v as i16)
    }
    pub fn u32(&self, at: usize) -> Option<u32> {
        u32_at(self.data, at)
    }
    pub fn i32(&self, at: usize) -> Option<i32> {
        self.u32(at).map(|v| v as i32)
    }
}

/// Iterates the records laid end to end in a byte slice.
#[derive(Clone, Debug)]
pub struct Records<'a> {
    bytes: &'a [u8],
    at: usize,
}

impl<'a> Records<'a> {
    pub fn new(bytes: &'a [u8]) -> Self {
        Records { bytes, at: 0 }
    }
}

impl<'a> Iterator for Records<'a> {
    type Item = Rec<'a>;
    fn next(&mut self) -> Option<Rec<'a>> {
        let r = Rec::at(self.bytes, self.at)?;
        // Always advance by at least the header, so a zero-length record can't stall the walk.
        self.at = self.at.checked_add(8)?.checked_add(r.data.len())?;
        Some(r)
    }
}

/// A little-endian reader over a record body for the variable-length structures (text
/// exceptions, style runs): every read is checked and a short body ends the walk.
pub struct Cursor<'a> {
    pub b: &'a [u8],
    pub at: usize,
}

impl<'a> Cursor<'a> {
    pub fn new(b: &'a [u8]) -> Self {
        Cursor { b, at: 0 }
    }
    /// Bytes left to read.
    pub fn remaining(&self) -> usize {
        self.b.len().saturating_sub(self.at)
    }
    pub fn u8(&mut self) -> Option<u8> {
        let v = *self.b.get(self.at)?;
        self.at += 1;
        Some(v)
    }
    pub fn u16(&mut self) -> Option<u16> {
        let v = u16_at(self.b, self.at)?;
        self.at += 2;
        Some(v)
    }
    pub fn i16(&mut self) -> Option<i16> {
        self.u16().map(|v| v as i16)
    }
    pub fn u32(&mut self) -> Option<u32> {
        let v = u32_at(self.b, self.at)?;
        self.at += 4;
        Some(v)
    }
}

#[cfg(test)]
pub(crate) mod build {
    //! Writing records, for tests that build `.ppt` files.

    pub fn atom(kind: u16, instance: u16, body: &[u8]) -> Vec<u8> {
        head(0, kind, instance, body)
    }

    pub fn container(kind: u16, instance: u16, children: &[Vec<u8>]) -> Vec<u8> {
        head(0xF, kind, instance, &children.concat())
    }

    fn head(ver: u16, kind: u16, instance: u16, body: &[u8]) -> Vec<u8> {
        let mut v = Vec::with_capacity(body.len() + 8);
        v.extend_from_slice(&(ver | (instance << 4)).to_le_bytes());
        v.extend_from_slice(&kind.to_le_bytes());
        v.extend_from_slice(&(body.len() as u32).to_le_bytes());
        v.extend_from_slice(body);
        v
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::build::*;
    use super::*;

    #[test]
    fn walks_nested_records() {
        let c = container(0x03E8, 0, &[atom(0x03E9, 0, &[1, 2, 3, 4]), atom(0x0FA0, 2, &[9; 6])]);
        let r = Rec::at(&c, 0).unwrap();
        assert!(r.is_container());
        let kids: Vec<_> = r.children().collect();
        assert_eq!(kids.len(), 2);
        assert_eq!(kids[1].instance, 2);
        assert_eq!(r.child(0x03E9).unwrap().u32(0), Some(0x0403_0201));
    }

    #[test]
    fn truncated_and_zero_length_records_end_the_walk() {
        let mut c = atom(0x1, 0, &[]).repeat(3);
        c.extend_from_slice(&[0xF, 0, 0, 0, 0xFF, 0xFF, 0xFF, 0x7F, 1, 2]);
        let all: Vec<_> = Records::new(&c).collect();
        assert_eq!(all.len(), 4);
        assert_eq!(all[3].data, &[1, 2]);
        assert!(Records::new(&[1, 2, 3]).next().is_none());
    }
}
