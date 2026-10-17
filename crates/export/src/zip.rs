//! A minimal zip writer (deflate or store) for Office files and comic book archives.
//!
//! [`ZipWriter`] streams entries to any [`Write`], so a large archive (a comic of a thousand
//! rendered pages) never has to fit in memory; only the central directory is kept. Archives,
//! entries or offsets past the classic format's limits (65,535 entries, 4 GiB) are written as
//! zip64 (APPNOTE 4.5.3); smaller ones stay classic, which every reader opens. [`Zip`] is the
//! in-memory form the Office writers use.

use std::io::{self, Write};

fn crc32(data: &[u8]) -> u32 {
    static TABLE: std::sync::OnceLock<[u32; 256]> = std::sync::OnceLock::new();
    let table = TABLE.get_or_init(|| {
        let mut t = [0u32; 256];
        for (i, e) in t.iter_mut().enumerate() {
            let mut c = i as u32;
            for _ in 0..8 {
                c = if c & 1 != 0 { 0xEDB8_8320 ^ (c >> 1) } else { c >> 1 };
            }
            *e = c;
        }
        t
    });
    let mut c = 0xFFFF_FFFFu32;
    for b in data {
        c = table[((c ^ *b as u32) & 0xFF) as usize] ^ (c >> 8);
    }
    c ^ 0xFFFF_FFFF
}

/// The largest values the classic format's 16- and 32-bit fields hold; these values themselves
/// mean "see the zip64 record".
const MAX16: u64 = 0xFFFF;
const MAX32: u64 = 0xFFFF_FFFF;

/// A `usize` as a `u64`: lossless on every target Rust supports (pointers are at most 64 bits).
fn len64(n: usize) -> u64 {
    n as u64
}

/// The value of a 32-bit field: the value itself, or the zip64 marker when it doesn't fit.
fn field32(v: u64) -> u32 {
    u32::try_from(v).ok().filter(|v| u64::from(*v) < MAX32).unwrap_or(u32::MAX)
}

/// The size at which a field moves to the zip64 record; lowered in the tests, which can't write
/// 4 GiB.
#[derive(Clone, Copy)]
struct Limits {
    entries: u64,
    bytes: u64,
}

const CLASSIC: Limits = Limits { entries: MAX16, bytes: MAX32 };

/// Streams a zip archive to `W`.
pub struct ZipWriter<W: Write> {
    out: W,
    /// Bytes written so far: the next local header's offset.
    offset: u64,
    central: Vec<u8>,
    count: u64,
    limits: Limits,
}

impl<W: Write> ZipWriter<W> {
    pub fn new(out: W) -> Self {
        Self { out, offset: 0, central: Vec::new(), count: 0, limits: CLASSIC }
    }

    fn write(&mut self, bytes: &[u8]) -> io::Result<()> {
        self.out.write_all(bytes)?;
        self.offset = self.offset.checked_add(len64(bytes.len())).ok_or_else(|| io::Error::other("the archive is too large"))?;
        Ok(())
    }

    /// Add a file named `name` (UTF-8, `/` between folders). Already-compressed data (images) is
    /// stored; the rest is deflated when `compress` is set.
    pub fn add(&mut self, name: &str, data: &[u8], compress: bool) -> io::Result<()> {
        let name_len =
            u16::try_from(name.len()).map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "a file name in the archive is too long"))?;
        let crc = crc32(data);
        let deflated;
        let (method, body) = if compress {
            let mut e = flate2::write::DeflateEncoder::new(Vec::new(), flate2::Compression::default());
            e.write_all(data)?;
            deflated = e.finish()?;
            (8u16, deflated.as_slice())
        } else {
            (0u16, data)
        };
        let (size, packed, offset) = (len64(data.len()), len64(body.len()), self.offset);
        let Limits { bytes: big, .. } = self.limits;
        // Fields shared by the local and the central header, from "version needed" on.
        let common = |h: &mut Vec<u8>, zip64: bool, sizes: [u32; 2], extra: u16| {
            h.extend(if zip64 { 45u16 } else { 20u16 }.to_le_bytes()); // version needed
            h.extend(0x0800u16.to_le_bytes()); // UTF-8 names
            h.extend(method.to_le_bytes());
            h.extend([0, 0, 0x21, 0]); // 00:00, 1980-01-01 (a zero date is not a valid DOS date)
            h.extend(crc.to_le_bytes());
            h.extend(sizes[1].to_le_bytes()); // compressed
            h.extend(sizes[0].to_le_bytes()); // uncompressed
            h.extend(name_len.to_le_bytes());
            h.extend(extra.to_le_bytes());
        };
        // Local header: a zip64 extra field holds both sizes, or neither (4.5.3).
        let local64 = size >= big || packed >= big;
        let mut local = Vec::with_capacity(30 + name.len() + 20);
        local.extend(0x0403_4b50u32.to_le_bytes());
        let sizes = if local64 { [u32::MAX; 2] } else { [field32(size), field32(packed)] };
        common(&mut local, local64, sizes, if local64 { 20 } else { 0 });
        local.extend(name.as_bytes());
        if local64 {
            local.extend(1u16.to_le_bytes());
            local.extend(16u16.to_le_bytes());
            local.extend(size.to_le_bytes());
            local.extend(packed.to_le_bytes());
        }
        // Central header: the zip64 extra field holds, in this order, just the values that
        // don't fit.
        let mut extra = Vec::new();
        for v in [size, packed, offset] {
            if v >= big {
                extra.extend(v.to_le_bytes());
            }
        }
        let central64 = !extra.is_empty();
        let field = |v: u64| if v >= big { u32::MAX } else { field32(v) };
        let c = &mut self.central;
        c.extend(0x0201_4b50u32.to_le_bytes());
        c.extend(if central64 { 45u16 } else { 20u16 }.to_le_bytes()); // made by
        common(c, central64, [field(size), field(packed)], if central64 { 4 + extra.len() as u16 } else { 0 });
        c.extend(0u16.to_le_bytes()); // comment
        c.extend(0u16.to_le_bytes()); // disk
        c.extend(0u16.to_le_bytes()); // internal attributes
        c.extend(0u32.to_le_bytes()); // external attributes
        c.extend(field(offset).to_le_bytes());
        c.extend(name.as_bytes());
        if central64 {
            c.extend(1u16.to_le_bytes());
            c.extend((extra.len() as u16).to_le_bytes()); // at most 24
            c.extend(extra);
        }
        self.write(&local)?;
        self.write(body)?;
        self.count = self.count.saturating_add(1);
        Ok(())
    }

    /// Write the central directory and return the writer.
    pub fn finish(mut self) -> io::Result<W> {
        let start = self.offset;
        let size = len64(self.central.len());
        let central = std::mem::take(&mut self.central);
        self.write(&central)?;
        let Limits { entries, bytes } = self.limits;
        if self.count >= entries || size >= bytes || start >= bytes {
            let record = self.offset;
            let mut z = Vec::with_capacity(76);
            z.extend(0x0606_4b50u32.to_le_bytes());
            z.extend(44u64.to_le_bytes()); // the size of the rest of this record
            z.extend(45u16.to_le_bytes()); // made by
            z.extend(45u16.to_le_bytes()); // version needed
            z.extend(0u32.to_le_bytes()); // this disk
            z.extend(0u32.to_le_bytes()); // the central directory's disk
            z.extend(self.count.to_le_bytes()); // entries on this disk
            z.extend(self.count.to_le_bytes()); // entries
            z.extend(size.to_le_bytes());
            z.extend(start.to_le_bytes());
            // Locator.
            z.extend(0x0706_4b50u32.to_le_bytes());
            z.extend(0u32.to_le_bytes()); // the record's disk
            z.extend(record.to_le_bytes());
            z.extend(1u32.to_le_bytes()); // disks
            self.write(&z)?;
        }
        let count = if self.count >= entries { u16::MAX } else { u16::try_from(self.count).unwrap_or(u16::MAX) };
        let field = |v: u64| if v >= bytes { u32::MAX } else { field32(v) };
        let mut e = Vec::with_capacity(22);
        e.extend(0x0605_4b50u32.to_le_bytes());
        e.extend([0u8; 4]); // disks
        e.extend(count.to_le_bytes());
        e.extend(count.to_le_bytes());
        e.extend(field(size).to_le_bytes());
        e.extend(field(start).to_le_bytes());
        e.extend(0u16.to_le_bytes()); // comment
        self.write(&e)?;
        self.out.flush()?;
        Ok(self.out)
    }
}

/// Builds a zip archive in memory (the Office writers).
pub struct Zip(ZipWriter<Vec<u8>>);

impl Default for Zip {
    fn default() -> Self {
        Self(ZipWriter::new(Vec::new()))
    }
}

impl Zip {
    /// Add a file. Already-compressed data (images) is stored; the rest is deflated.
    ///
    /// Writing to memory can't fail, and the Office writers name their parts themselves (well
    /// under the 64 KiB a name may take), so there is no error to report.
    pub fn add(&mut self, name: &str, data: &[u8], compress: bool) {
        let _ = self.0.add(name, data, compress);
    }

    pub fn finish(self) -> Vec<u8> {
        self.0.finish().unwrap_or_default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn crc_matches_the_reference() {
        assert_eq!(crc32(b"123456789"), 0xCBF4_3926);
    }

    #[test]
    fn archive_has_both_directories() {
        let mut z = Zip::default();
        z.add("a.txt", b"hello hello hello", true);
        z.add("b.png", b"\x89PNG", false);
        let b = z.finish();
        assert!(b.starts_with(b"PK\x03\x04"));
        assert_eq!(b.windows(4).filter(|w| *w == b"PK\x01\x02").count(), 2);
        assert_eq!(&b[b.len() - 22..b.len() - 18], b"PK\x05\x06");
        assert!(!b.windows(4).any(|w| w == b"PK\x06\x06"), "a small archive stays classic");
    }

    fn u16_at(b: &[u8], at: usize) -> u16 {
        u16::from_le_bytes(b[at..at + 2].try_into().unwrap())
    }
    fn u32_at(b: &[u8], at: usize) -> u32 {
        u32::from_le_bytes(b[at..at + 4].try_into().unwrap())
    }
    fn u64_at(b: &[u8], at: usize) -> u64 {
        u64::from_le_bytes(b[at..at + 8].try_into().unwrap())
    }

    /// A reader for what these tests write: each entry's (name, data) through the central
    /// directory, following the zip64 records when the classic fields say so.
    fn read(b: &[u8]) -> Vec<(String, Vec<u8>)> {
        let eocd = b.len() - 22;
        assert_eq!(u32_at(b, eocd), 0x0605_4b50);
        let (mut count, mut start) = (u64::from(u16_at(b, eocd + 10)), u64::from(u32_at(b, eocd + 16)));
        if count == MAX16 || start == MAX32 {
            let locator = eocd - 20;
            assert_eq!(u32_at(b, locator), 0x0706_4b50);
            let record = u64_at(b, locator + 8) as usize;
            assert_eq!(u32_at(b, record), 0x0606_4b50);
            count = u64_at(b, record + 32);
            start = u64_at(b, record + 48);
        }
        let mut at = start as usize;
        let mut out = Vec::new();
        for _ in 0..count {
            assert_eq!(u32_at(b, at), 0x0201_4b50);
            let method = u16_at(b, at + 10);
            let crc = u32_at(b, at + 16);
            let (mut packed, mut size) = (u64::from(u32_at(b, at + 20)), u64::from(u32_at(b, at + 24)));
            let (n, x, k) = (u16_at(b, at + 28) as usize, u16_at(b, at + 30) as usize, u16_at(b, at + 32) as usize);
            let mut offset = u64::from(u32_at(b, at + 42));
            let name = String::from_utf8(b[at + 46..at + 46 + n].to_vec()).unwrap();
            let mut e = at + 46 + n;
            if x > 0 {
                assert_eq!(u16_at(b, e), 1);
                e += 4;
                for v in [&mut size, &mut packed, &mut offset] {
                    if *v == MAX32 {
                        *v = u64_at(b, e);
                        e += 8;
                    }
                }
            }
            let local = offset as usize;
            assert_eq!(u32_at(b, local), 0x0403_4b50);
            let body = local + 30 + u16_at(b, local + 26) as usize + u16_at(b, local + 28) as usize;
            let raw = &b[body..body + packed as usize];
            let data = if method == 8 {
                let mut d = Vec::new();
                std::io::Read::read_to_end(&mut flate2::read::DeflateDecoder::new(raw), &mut d).unwrap();
                d
            } else {
                raw.to_vec()
            };
            assert_eq!(len64(data.len()), size);
            assert_eq!(crc32(&data), crc);
            out.push((name, data));
            at += 46 + n + x + k;
        }
        out
    }

    #[test]
    fn classic_archive_reads_back() {
        let mut z = ZipWriter::new(Vec::new());
        z.add("ComicInfo.xml", b"<ComicInfo/> <ComicInfo/>", true).unwrap();
        z.add("001.jpg", b"\xFF\xD8jpeg", false).unwrap();
        let entries = read(&z.finish().unwrap());
        assert_eq!(entries, vec![("ComicInfo.xml".into(), b"<ComicInfo/> <ComicInfo/>".to_vec()), ("001.jpg".into(), b"\xFF\xD8jpeg".to_vec())]);
    }

    /// Sizes, offsets and the entry count past the limits go to the zip64 records, and a reader
    /// following them finds every entry. The limits are lowered: the tests can't write 4 GiB.
    #[test]
    fn past_the_limits_the_archive_is_zip64() {
        let mut z = ZipWriter::new(Vec::new());
        z.limits = Limits { entries: 3, bytes: 40 };
        let big = vec![7u8; 100];
        z.add("small.txt", b"hi", false).unwrap();
        z.add("big.bin", &big, false).unwrap(); // its size and (later) offsets don't fit
        z.add("packed.txt", &big, true).unwrap(); // uncompressed doesn't fit, compressed does
        z.add("late.txt", b"after", false).unwrap(); // only its offset doesn't fit
        let b = z.finish().unwrap();
        assert!(b.windows(4).any(|w| w == b"PK\x06\x06"));
        assert!(b.windows(4).any(|w| w == b"PK\x06\x07"));
        let entries = read(&b);
        let names: Vec<&str> = entries.iter().map(|(n, _)| n.as_str()).collect();
        assert_eq!(names, ["small.txt", "big.bin", "packed.txt", "late.txt"]);
        assert_eq!(entries[1].1, big);
        assert_eq!(entries[2].1, big);
        assert_eq!(entries[3].1, b"after");
    }

    #[test]
    fn an_overlong_name_is_refused_not_truncated() {
        let mut z = ZipWriter::new(Vec::new());
        let err = z.add(&"a".repeat(70_000), b"x", false).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::InvalidInput);
        // Nothing was written for it.
        assert!(z.finish().unwrap().starts_with(b"PK\x05\x06"));
    }

    /// A write that fails reaches the caller.
    #[test]
    fn write_errors_are_reported() {
        struct Full;
        impl Write for Full {
            fn write(&mut self, _: &[u8]) -> io::Result<usize> {
                Err(io::Error::other("disk full"))
            }
            fn flush(&mut self) -> io::Result<()> {
                Ok(())
            }
        }
        let mut z = ZipWriter::new(Full);
        assert!(z.add("a", b"x", false).is_err());
    }
}
