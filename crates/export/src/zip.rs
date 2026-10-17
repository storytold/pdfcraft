//! A minimal zip writer (deflate or store) for Office files. Archives that outgrow the classic
//! end-of-central-directory record's 16-bit entry count or 32-bit sizes are written with the
//! ZIP64 records (APPNOTE 4.4), which every reader understands; small ones are unchanged.

use std::io::Write;

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

/// The largest entry count the classic end-of-central-directory record holds (APPNOTE 4.3.12).
const MAX_CLASSIC_COUNT: u64 = u16::MAX as u64;
/// The largest size or offset the classic records hold: past it the field is a sentinel and the
/// ZIP64 record carries the real value (APPNOTE 4.4.1).
const MAX_CLASSIC_U32: u64 = u32::MAX as u64;

/// The ZIP64 extended-information extra field (APPNOTE 4.5.3) holding `fields` — the values that
/// no longer fit their classic fields — in specification order. Empty when everything fits.
fn zip64_extra(fields: &[u64]) -> Vec<u8> {
    let mut e = Vec::with_capacity(4 + 8 * fields.len());
    if !fields.is_empty() {
        e.extend(0x0001u16.to_le_bytes());
        e.extend((8 * fields.len() as u16).to_le_bytes());
        for &f in fields {
            e.extend(f.to_le_bytes());
        }
    }
    e
}

/// The classic 32-bit field value for `value`: the value itself, or the 0xFFFFFFFF sentinel when it
/// no longer fits — including when it equals the sentinel, which a reader cannot tell from a real
/// value — in which case the ZIP64 extra field carries the real value (APPNOTE 4.4.3).
fn classic_u32(value: u64) -> u32 {
    if value >= MAX_CLASSIC_U32 { u32::MAX } else { value as u32 }
}

/// Builds one member's local file header and central directory entry. The sizes and `offset` are
/// parameters, not read from the body, so tests can inject values past the classic fields without
/// allocating the data.
fn entry_headers(name: &str, crc: u32, method: u16, compressed: u64, uncompressed: u64, offset: u64) -> (Vec<u8>, Vec<u8>) {
    // A value at or above the sentinel cannot be held by a classic field, so it is offloaded.
    let size_is_zip64 = uncompressed >= MAX_CLASSIC_U32 || compressed >= MAX_CLASSIC_U32;
    // A local header that carries ZIP64 sizes carries both, original then compressed, even when only
    // one overflowed (APPNOTE 4.5.3). A central entry carries one value for each field it set to its
    // sentinel, and only those, so a reader walking the fields by their classic ones stays in step.
    let local_extra = if size_is_zip64 { zip64_extra(&[uncompressed, compressed]) } else { Vec::new() };
    let mut central_fields = Vec::new();
    if uncompressed >= MAX_CLASSIC_U32 {
        central_fields.push(uncompressed);
    }
    if compressed >= MAX_CLASSIC_U32 {
        central_fields.push(compressed);
    }
    if offset >= MAX_CLASSIC_U32 {
        central_fields.push(offset);
    }
    let central_extra = zip64_extra(&central_fields);
    // An entry that carries ZIP64 fields needs version 4.5 to extract (APPNOTE 4.4.3); the version
    // in the local header and the central entry agree.
    let version_needed = if size_is_zip64 || offset >= MAX_CLASSIC_U32 { 45u16 } else { 20u16 };
    let header = |sig: u32, central: bool| {
        let mut h = Vec::new();
        h.extend(sig.to_le_bytes());
        if central {
            h.extend(20u16.to_le_bytes()); // made by
        }
        h.extend(version_needed.to_le_bytes()); // version needed
        h.extend(0x0800u16.to_le_bytes()); // UTF-8 names
        h.extend(method.to_le_bytes());
        h.extend([0, 0, 0x21, 0]); // 00:00, 1980-01-01 (a zero date is not a valid DOS date)
        h.extend(crc.to_le_bytes());
        h.extend(classic_u32(compressed).to_le_bytes());
        h.extend(classic_u32(uncompressed).to_le_bytes());
        h.extend((name.len() as u16).to_le_bytes());
        let extra = if central { &central_extra } else { &local_extra };
        h.extend((extra.len() as u16).to_le_bytes()); // extra
        if central {
            h.extend(0u16.to_le_bytes()); // comment
            h.extend(0u16.to_le_bytes()); // disk
            h.extend(0u16.to_le_bytes()); // internal attributes
            h.extend(0u32.to_le_bytes()); // external attributes
            h.extend(classic_u32(offset).to_le_bytes());
        }
        h.extend(name.as_bytes());
        h.extend(extra);
        h
    };
    (header(0x0403_4b50, false), header(0x0201_4b50, true))
}

/// Builds a zip archive in memory. An archive past the classic record's 16-bit entry count or
/// 32-bit sizes and offsets is written with a ZIP64 end-of-central-directory record: a
/// PowerPoint export writes two entries per page plus ten fixed parts, so a document with
/// 32 763 pages already passes 65 535 entries, and without the ZIP64 record the count wraps
/// and readers see a central directory that does not describe the archive.
#[derive(Default)]
pub struct Zip {
    out: Vec<u8>,
    central: Vec<u8>,
    count: usize,
}

impl Zip {
    /// Add a file. Already-compressed data (images) is stored; the rest is deflated.
    pub fn add(&mut self, name: &str, data: &[u8], compress: bool) {
        let crc = crc32(data);
        let (method, body) = if compress {
            let mut e = flate2::write::DeflateEncoder::new(Vec::new(), flate2::Compression::default());
            let _ = e.write_all(data);
            (8u16, e.finish().unwrap_or_default())
        } else {
            (0u16, data.to_vec())
        };
        let offset = self.out.len() as u64;
        let (local, central) = entry_headers(name, crc, method, body.len() as u64, data.len() as u64, offset);
        self.out.extend(local);
        self.out.extend(&body);
        self.central.extend(central);
        self.count += 1;
    }

    pub fn finish(mut self) -> Vec<u8> {
        let start = self.out.len() as u64;
        let size = self.central.len() as u64;
        let count = self.count as u64;
        self.out.append(&mut self.central);
        let classic_count = if count > MAX_CLASSIC_COUNT { u16::MAX } else { count as u16 };
        if count > MAX_CLASSIC_COUNT || size >= MAX_CLASSIC_U32 || start >= MAX_CLASSIC_U32 {
            let eocd64_at = self.out.len() as u64;
            self.out.extend(0x0606_4b50u32.to_le_bytes()); // ZIP64 end of central directory record
            self.out.extend(44u64.to_le_bytes()); // size of the rest of this record
            self.out.extend(45u16.to_le_bytes()); // version made by (4.5, DOS)
            self.out.extend(45u16.to_le_bytes()); // version needed to extract
            self.out.extend(0u32.to_le_bytes()); // number of this disk
            self.out.extend(0u32.to_le_bytes()); // disk with the start of the central directory
            self.out.extend(count.to_le_bytes()); // entries on this disk
            self.out.extend(count.to_le_bytes()); // entries in total
            self.out.extend(size.to_le_bytes()); // size of the central directory
            self.out.extend(start.to_le_bytes()); // offset of the central directory
            self.out.extend(0x0706_4b50u32.to_le_bytes()); // ZIP64 end of central directory locator
            self.out.extend(0u32.to_le_bytes()); // disk with the ZIP64 record
            self.out.extend(eocd64_at.to_le_bytes()); // offset of the ZIP64 record
            self.out.extend(1u32.to_le_bytes()); // total number of disks
        }
        self.out.extend(0x0605_4b50u32.to_le_bytes());
        self.out.extend([0u8; 4]); // number of this disk, disk with the central directory
        self.out.extend(classic_count.to_le_bytes()); // entries on this disk
        self.out.extend(classic_count.to_le_bytes()); // entries in total
        self.out.extend(classic_u32(size).to_le_bytes());
        self.out.extend(classic_u32(start).to_le_bytes());
        self.out.extend(0u16.to_le_bytes()); // comment length
        self.out
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
    }

    #[test]
    fn more_entries_than_the_classic_record_holds_writes_zip64() {
        // A PowerPoint export writes two entries per page plus ten fixed parts, so a
        // document with 32 763 pages already passes the classic record's 16-bit entry
        // count. The count must not wrap: it overflows in a debug build, and in a release
        // one it silently writes a central directory readers cannot match to the archive.
        let mut z = Zip::default();
        for i in 0..u16::MAX as usize + 1 {
            z.add(&format!("ppt/slides/slide{i}.xml"), b"x", true);
            z.add(&format!("ppt/slides/_rels/slide{i}.xml.rels"), b"y", true);
        }
        let b = z.finish();
        let entries = 2 * (u16::MAX as usize + 1);
        // The classic end-of-central-directory record holds sentinels…
        let eocd = &b[b.len() - 22..];
        assert_eq!(&eocd[..4], &0x0605_4b50u32.to_le_bytes());
        assert_eq!(&eocd[8..10], &u16::MAX.to_le_bytes(), "entries on this disk");
        assert_eq!(&eocd[10..12], &u16::MAX.to_le_bytes(), "entries in total");
        // …and the ZIP64 locator and record carry the truth.
        let locator = &b[b.len() - 42..b.len() - 22];
        assert_eq!(&locator[..4], &0x0706_4b50u32.to_le_bytes());
        let at = u64::from_le_bytes(locator[8..16].try_into().unwrap_or([0; 8])) as usize;
        assert_eq!(&b[at..at + 4], &0x0606_4b50u32.to_le_bytes());
        assert_eq!(u64::from_le_bytes(b[at + 24..at + 32].try_into().unwrap_or([0; 8])), entries as u64, "entries on this disk");
        assert_eq!(u64::from_le_bytes(b[at + 32..at + 40].try_into().unwrap_or([0; 8])), entries as u64, "entries in total");
    }

    /// The extra field of a local file header (APPNOTE 4.3.7), which follows the file name.
    fn local_extra(h: &[u8]) -> &[u8] {
        let le = |i: usize, n: usize| h[i..i + n].iter().rev().fold(0usize, |v, b| v << 8 | *b as usize);
        let (name_len, extra_len) = (le(26, 2), le(28, 2));
        &h[30 + name_len..30 + name_len + extra_len]
    }

    /// The extra field of a central directory entry (APPNOTE 4.3.12), which follows the file name.
    fn central_extra(h: &[u8]) -> &[u8] {
        let le = |i: usize, n: usize| h[i..i + n].iter().rev().fold(0usize, |v, b| v << 8 | *b as usize);
        let (name_len, extra_len) = (le(28, 2), le(30, 2));
        &h[46 + name_len..46 + name_len + extra_len]
    }

    #[test]
    fn local_zip64_extra_carries_both_sizes() {
        // When only one size overflows the u32 limit, the local header still carries both sizes —
        // original then compressed — because a local ZIP64 extra that carries either must carry
        // both (APPNOTE 4.5.3). Either size may be the one past the limit.
        for (compressed, uncompressed) in [(100u64, u32::MAX as u64 + 1), (u32::MAX as u64 + 1, 100)] {
            let (local, _) = entry_headers("a", 0, 0, compressed, uncompressed, 0);
            let extra = local_extra(&local);
            assert_eq!(u16::from_le_bytes(extra[..2].try_into().unwrap_or([0; 2])), 0x0001, "ZIP64 extra id");
            assert_eq!(u16::from_le_bytes(extra[2..4].try_into().unwrap_or([0; 2])), 16, "both sizes are present");
            assert_eq!(u64::from_le_bytes(extra[4..12].try_into().unwrap_or([0; 8])), uncompressed, "original size first");
            assert_eq!(u64::from_le_bytes(extra[12..20].try_into().unwrap_or([0; 8])), compressed, "compressed size second");
        }
    }

    #[test]
    fn zip64_entries_need_version_4_5() {
        // An entry that carries ZIP64 fields must advertise version 4.5 to extract, in both the
        // local file header and the central directory entry (APPNOTE 4.4.3). An entry that fits
        // every classic field stays at 2.0, so small archives are unchanged.
        let (local, central) = entry_headers("a", 0, 0, u32::MAX as u64 + 1, u32::MAX as u64 + 1, 0);
        assert_eq!(u16::from_le_bytes(local[4..6].try_into().unwrap_or([0; 2])), 45, "local version needed");
        assert_eq!(u16::from_le_bytes(central[6..8].try_into().unwrap_or([0; 2])), 45, "central version needed");
        let (local, central) = entry_headers("a", 0, 0, 4, 8, 0);
        assert_eq!(u16::from_le_bytes(local[4..6].try_into().unwrap_or([0; 2])), 20, "local version needed");
        assert_eq!(u16::from_le_bytes(central[6..8].try_into().unwrap_or([0; 2])), 20, "central version needed");
    }

    #[test]
    fn size_equal_to_the_sentinel_is_offloaded() {
        // A real size of exactly u32::MAX is indistinguishable from the 0xFFFFFFFF sentinel, so it
        // too must be offloaded: every classic field set to the sentinel needs its 8-byte value in
        // the central extra, or a reader such as Python's zipfile reports "Compress size not
        // found". Both orders of which size is the MAX+1 one must work.
        for (compressed, uncompressed) in [(u32::MAX as u64, u32::MAX as u64 + 1), (u32::MAX as u64 + 1, u32::MAX as u64)] {
            let (local, central) = entry_headers("a", 0, 0, compressed, uncompressed, 0);
            assert_eq!(u32::from_le_bytes(local[18..22].try_into().unwrap_or([0; 4])), u32::MAX, "local compressed sentinel");
            assert_eq!(u32::from_le_bytes(local[22..26].try_into().unwrap_or([0; 4])), u32::MAX, "local original sentinel");
            assert_eq!(u32::from_le_bytes(central[20..24].try_into().unwrap_or([0; 4])), u32::MAX, "central compressed sentinel");
            assert_eq!(u32::from_le_bytes(central[24..28].try_into().unwrap_or([0; 4])), u32::MAX, "central original sentinel");
            let extra = central_extra(&central);
            assert_eq!(u16::from_le_bytes(extra[2..4].try_into().unwrap_or([0; 2])), 16, "a value for each sentinel field");
            assert_eq!(u64::from_le_bytes(extra[4..12].try_into().unwrap_or([0; 8])), uncompressed, "original size first");
            assert_eq!(u64::from_le_bytes(extra[12..20].try_into().unwrap_or([0; 8])), compressed, "compressed size second");
        }
    }
}
