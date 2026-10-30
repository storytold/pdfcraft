//! A bounded ZIP reader for EPUB containers: stored and deflated entries only, read from the
//! central directory. An untrusted book can't make it allocate without bound or reach outside the
//! book: entry count and sizes are capped (a zip bomb stops at the cap, not at the end of memory),
//! and names that are absolute or climb out with `..` are refused.

use std::io::Read;

use super::EpubError;

/// At most this many entries.
pub const MAX_ENTRIES: usize = 10_000;
/// At most this many bytes in one entry, uncompressed.
pub const MAX_ENTRY: u64 = 256 << 20;
/// At most this many bytes in all entries together, uncompressed.
pub const MAX_TOTAL: u64 = 1 << 30;

/// One file in the container.
#[derive(Clone, Debug)]
struct Entry {
    name: String,
    method: u16,
    flags: u16,
    compressed: u64,
    size: u64,
    offset: u64,
}

/// The container: its entries, read on demand.
pub struct Zip<'a> {
    bytes: &'a [u8],
    entries: Vec<Entry>,
    /// Bytes handed out so far (counted against [`MAX_TOTAL`]).
    read: std::cell::Cell<u64>,
}

fn u16_at(b: &[u8], at: usize) -> Option<u16> {
    Some(u16::from_le_bytes(b.get(at..at + 2)?.try_into().ok()?))
}

fn u32_at(b: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_le_bytes(b.get(at..at + 4)?.try_into().ok()?))
}

fn damaged(what: &str) -> EpubError {
    EpubError::Damaged(what.to_string())
}

/// Whether `name` stays inside the book: relative, no `..` step, no drive or backslash root.
pub fn safe_name(name: &str) -> bool {
    !name.is_empty()
        && !name.starts_with('/')
        && !name.starts_with('\\')
        && !name.contains('\0')
        && !(name.len() >= 2 && name.as_bytes()[1] == b':')
        && !name.split(['/', '\\']).any(|part| part == "..")
}

impl<'a> Zip<'a> {
    /// The container's directory. Refuses a file that isn't a ZIP, a ZIP64 or split archive, more
    /// than [`MAX_ENTRIES`] entries, and entries whose names leave the book.
    pub fn open(bytes: &'a [u8]) -> Result<Self, EpubError> {
        // The end of central directory record: 22 bytes plus a comment of up to 64 KiB.
        let from = bytes.len().saturating_sub(22 + 0xFFFF);
        let eocd = (from..=bytes.len().saturating_sub(22))
            .rev()
            .find(|&i| bytes.get(i..i + 4) == Some(b"PK\x05\x06"))
            .ok_or_else(|| damaged("not a ZIP container"))?;
        let disk = u16_at(bytes, eocd + 4).ok_or_else(|| damaged("the ZIP directory is cut short"))?;
        let count = u16_at(bytes, eocd + 10).ok_or_else(|| damaged("the ZIP directory is cut short"))?;
        let size = u32_at(bytes, eocd + 12).ok_or_else(|| damaged("the ZIP directory is cut short"))?;
        let start = u32_at(bytes, eocd + 16).ok_or_else(|| damaged("the ZIP directory is cut short"))?;
        if disk != 0 {
            return Err(damaged("a split ZIP archive"));
        }
        if count == 0xFFFF || size == 0xFFFF_FFFF || start == 0xFFFF_FFFF {
            return Err(damaged("a ZIP64 archive, which EPUB readers don't expect"));
        }
        if usize::from(count) > MAX_ENTRIES {
            return Err(EpubError::TooLarge(format!("more than {MAX_ENTRIES} files")));
        }
        let mut entries = Vec::with_capacity(usize::from(count));
        let mut at = start as usize;
        for _ in 0..count {
            if bytes.get(at..at + 4) != Some(b"PK\x01\x02") {
                return Err(damaged("the ZIP directory is damaged"));
            }
            let field = |o: usize| u16_at(bytes, at + o).ok_or_else(|| damaged("the ZIP directory is cut short"));
            let long = |o: usize| u32_at(bytes, at + o).ok_or_else(|| damaged("the ZIP directory is cut short"));
            let (flags, method) = (field(8)?, field(10)?);
            let (compressed, size) = (long(20)?, long(24)?);
            let (name_len, extra_len, comment_len) = (usize::from(field(28)?), usize::from(field(30)?), usize::from(field(32)?));
            let offset = long(42)?;
            let raw = bytes.get(at + 46..at + 46 + name_len).ok_or_else(|| damaged("the ZIP directory is cut short"))?;
            // Bit 11: the name is UTF-8; EPUB requires UTF-8 names, so read them that way regardless.
            let name = String::from_utf8_lossy(raw).into_owned();
            if compressed == 0xFFFF_FFFF || size == 0xFFFF_FFFF || offset == 0xFFFF_FFFF {
                return Err(damaged("a ZIP64 archive, which EPUB readers don't expect"));
            }
            if !safe_name(&name) {
                return Err(EpubError::Damaged(format!("the file name \"{name}\" points outside the book")));
            }
            entries.push(Entry { name, method, flags, compressed: u64::from(compressed), size: u64::from(size), offset: u64::from(offset) });
            at += 46 + name_len + extra_len + comment_len;
        }
        Ok(Self { bytes, entries, read: std::cell::Cell::new(0) })
    }

    /// Whether the container holds `name` (directory names end in `/`).
    pub fn has(&self, name: &str) -> bool {
        self.entries.iter().any(|e| e.name == name)
    }

    /// The first entry's name and whether it is stored (EPUB's `mimetype` rule).
    #[cfg(test)]
    pub fn first(&self) -> Option<(&str, bool)> {
        self.entries.first().map(|e| (e.name.as_str(), e.method == 0))
    }

    /// Entry `name`, uncompressed. `Ok(None)` when there is no such entry.
    pub fn read(&self, name: &str) -> Result<Option<Vec<u8>>, EpubError> {
        let Some(e) = self.entries.iter().find(|e| e.name == name) else { return Ok(None) };
        if e.flags & 1 != 0 {
            return Err(EpubError::Protected);
        }
        if e.size > MAX_ENTRY {
            return Err(EpubError::TooLarge(format!("{name} is larger than {} MiB", MAX_ENTRY >> 20)));
        }
        if self.read.get().saturating_add(e.size) > MAX_TOTAL {
            return Err(EpubError::TooLarge(format!("more than {} MiB uncompressed", MAX_TOTAL >> 20)));
        }
        let at = e.offset as usize;
        if self.bytes.get(at..at + 4) != Some(b"PK\x03\x04") {
            return Err(EpubError::Damaged(format!("{name} is missing from the container")));
        }
        let name_len = usize::from(u16_at(self.bytes, at + 26).ok_or_else(|| damaged("a file header is cut short"))?);
        let extra_len = usize::from(u16_at(self.bytes, at + 28).ok_or_else(|| damaged("a file header is cut short"))?);
        let data = at + 30 + name_len + extra_len;
        let raw = self.bytes.get(data..data + e.compressed as usize).ok_or_else(|| EpubError::Damaged(format!("{name} is cut short")))?;
        let out = match e.method {
            // Stored: its bytes are its size, so a directory that understates them can't slip
            // an entry past the caps above.
            0 if e.compressed != e.size => return Err(EpubError::Damaged(format!("{name} is larger than the container says"))),
            0 => raw.to_vec(),
            8 => {
                // Never more than the directory said, and never more than the cap: a bomb whose
                // directory lies stops here.
                let limit = e.size.min(MAX_ENTRY);
                let mut out = Vec::with_capacity(limit.min(16 << 20) as usize);
                flate2::read::DeflateDecoder::new(raw)
                    .take(limit + 1)
                    .read_to_end(&mut out)
                    .map_err(|_| EpubError::Damaged(format!("{name} can't be decompressed")))?;
                if out.len() as u64 > e.size {
                    return Err(EpubError::Damaged(format!("{name} is larger than the container says")));
                }
                out
            }
            m => return Err(EpubError::Damaged(format!("{name} uses ZIP compression method {m}, which EPUB doesn't allow"))),
        };
        self.read.set(self.read.get().saturating_add(out.len() as u64));
        Ok(Some(out))
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// A ZIP of `(name, bytes, deflate)` entries (the test books are built with this).
    pub(crate) fn zip(entries: &[(&str, &[u8], bool)]) -> Vec<u8> {
        let mut out = Vec::new();
        let mut dir = Vec::new();
        for (name, data, deflate) in entries {
            let body = if *deflate {
                let mut e = flate2::write::DeflateEncoder::new(Vec::new(), flate2::Compression::default());
                std::io::Write::write_all(&mut e, data).unwrap();
                e.finish().unwrap()
            } else {
                data.to_vec()
            };
            let crc = crc32(data);
            let method: u16 = if *deflate { 8 } else { 0 };
            let offset = out.len() as u32;
            out.extend_from_slice(b"PK\x03\x04");
            out.extend_from_slice(&20u16.to_le_bytes());
            out.extend_from_slice(&0x0800u16.to_le_bytes());
            out.extend_from_slice(&method.to_le_bytes());
            out.extend_from_slice(&[0; 4]);
            out.extend_from_slice(&crc.to_le_bytes());
            out.extend_from_slice(&(body.len() as u32).to_le_bytes());
            out.extend_from_slice(&(data.len() as u32).to_le_bytes());
            out.extend_from_slice(&(name.len() as u16).to_le_bytes());
            out.extend_from_slice(&0u16.to_le_bytes());
            out.extend_from_slice(name.as_bytes());
            out.extend_from_slice(&body);
            dir.extend_from_slice(b"PK\x01\x02");
            dir.extend_from_slice(&20u16.to_le_bytes());
            dir.extend_from_slice(&20u16.to_le_bytes());
            dir.extend_from_slice(&0x0800u16.to_le_bytes());
            dir.extend_from_slice(&method.to_le_bytes());
            dir.extend_from_slice(&[0; 4]);
            dir.extend_from_slice(&crc.to_le_bytes());
            dir.extend_from_slice(&(body.len() as u32).to_le_bytes());
            dir.extend_from_slice(&(data.len() as u32).to_le_bytes());
            dir.extend_from_slice(&(name.len() as u16).to_le_bytes());
            dir.extend_from_slice(&[0; 12]);
            dir.extend_from_slice(&offset.to_le_bytes());
            dir.extend_from_slice(name.as_bytes());
        }
        let start = out.len() as u32;
        out.extend_from_slice(&dir);
        out.extend_from_slice(b"PK\x05\x06\0\0\0\0");
        out.extend_from_slice(&(entries.len() as u16).to_le_bytes());
        out.extend_from_slice(&(entries.len() as u16).to_le_bytes());
        out.extend_from_slice(&(dir.len() as u32).to_le_bytes());
        out.extend_from_slice(&start.to_le_bytes());
        out.extend_from_slice(&0u16.to_le_bytes());
        out
    }

    fn crc32(data: &[u8]) -> u32 {
        let mut crc = 0xFFFF_FFFFu32;
        for &b in data {
            crc ^= u32::from(b);
            for _ in 0..8 {
                crc = if crc & 1 != 0 { (crc >> 1) ^ 0xEDB8_8320 } else { crc >> 1 };
            }
        }
        !crc
    }

    #[test]
    fn stored_and_deflated_entries_are_read() {
        let bytes = zip(&[("mimetype", b"application/epub+zip", false), ("OEBPS/a.xhtml", "<p>héllo</p>".repeat(50).as_bytes(), true)]);
        let z = Zip::open(&bytes).unwrap();
        assert_eq!(z.first(), Some(("mimetype", true)));
        assert_eq!(z.read("mimetype").unwrap().unwrap(), b"application/epub+zip");
        assert_eq!(z.read("OEBPS/a.xhtml").unwrap().unwrap(), "<p>héllo</p>".repeat(50).as_bytes());
        assert!(z.read("missing").unwrap().is_none());
    }

    #[test]
    fn names_that_leave_the_book_are_refused() {
        for bad in ["../evil.xhtml", "/etc/passwd", "OEBPS/../../x", "C:/windows/x", "\\\\server\\x", "a\\..\\..\\b"] {
            let bytes = zip(&[(bad, b"x", false)]);
            assert!(matches!(Zip::open(&bytes), Err(EpubError::Damaged(m)) if m.contains("outside the book")), "{bad}");
        }
        for good in ["OEBPS/a..b.xhtml", "a/./b", "..hidden"] {
            assert!(safe_name(good), "{good}");
        }
    }

    #[test]
    fn a_zip_bomb_stops_at_the_cap() {
        // 300 MiB of zeros, deflated to a few hundred KiB: refused by its declared size.
        let big = vec![0u8; (MAX_ENTRY + 1) as usize];
        let bytes = zip(&[("bomb.xhtml", &big, true)]);
        let z = Zip::open(&bytes).unwrap();
        assert!(matches!(z.read("bomb.xhtml"), Err(EpubError::TooLarge(_))));
        // A directory that understates the size is caught while inflating.
        let mut lying = zip(&[("bomb.xhtml", &vec![0u8; 1 << 20], true)]);
        let dir = lying.windows(4).rposition(|w| w == b"PK\x01\x02").unwrap();
        lying[dir + 24..dir + 28].copy_from_slice(&1000u32.to_le_bytes());
        let z = Zip::open(&lying).unwrap();
        assert!(matches!(z.read("bomb.xhtml"), Err(EpubError::Damaged(m)) if m.contains("larger than the container says")));
    }

    #[test]
    fn not_a_zip_and_too_many_entries_are_refused() {
        assert!(matches!(Zip::open(b"%PDF-1.7"), Err(EpubError::Damaged(_))));
        let mut many = zip(&[("a", b"", false)]);
        let n = many.len();
        many[n - 12..n - 10].copy_from_slice(&((MAX_ENTRIES + 1) as u16).to_le_bytes());
        assert!(matches!(Zip::open(&many), Err(EpubError::TooLarge(_))));
    }
}
