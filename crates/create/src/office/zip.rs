//! A bounded reader for the zip packages Office files are (ECMA-376 Part 2, Open Packaging
//! Conventions). Only what OOXML writers produce is read: stored or deflated entries, no zip64,
//! no encryption. Every size comes from the file, so each is checked before it is trusted, and
//! inflating stops at a cap so a small entry can't expand without bound.

use std::io::Read;

/// Most entries a package may list.
const MAX_ENTRIES: usize = 20_000;
/// Largest single part, decompressed.
pub(crate) const MAX_PART: usize = 128 << 20;
/// Largest total read from one package, decompressed.
const MAX_TOTAL: usize = 512 << 20;

#[derive(Debug, Clone)]
struct Entry {
    name: String,
    method: u16,
    flags: u16,
    compressed: usize,
    size: usize,
    offset: usize,
}

#[derive(Debug)]
pub(crate) struct Zip<'a> {
    bytes: &'a [u8],
    entries: Vec<Entry>,
    read: std::cell::Cell<usize>,
}

fn u16_at(b: &[u8], at: usize) -> Option<u16> {
    Some(u16::from_le_bytes([*b.get(at)?, *b.get(at.checked_add(1)?)?]))
}

fn u32_at(b: &[u8], at: usize) -> Option<u32> {
    let s = b.get(at..at.checked_add(4)?)?;
    Some(u32::from_le_bytes([*s.first()?, *s.get(1)?, *s.get(2)?, *s.get(3)?]))
}

impl<'a> Zip<'a> {
    /// The package's central directory. `Err` says, in words a person can act on, why the bytes
    /// are not a package this reader accepts.
    pub(crate) fn open(bytes: &'a [u8]) -> Result<Self, String> {
        if bytes.starts_with(&[0xD0, 0xCF, 0x11, 0xE0, 0xA1, 0xB1, 0x1A, 0xE1]) {
            // Encrypted OOXML is stored in an OLE compound file, as are the legacy binary formats.
            return Err(
                "the file is password-protected or in the older binary Office format (.doc, .ppt, .xls); save it as .docx, .pptx or .xlsx".into()
            );
        }
        if !bytes.starts_with(b"PK\x03\x04") {
            return Err("the file is not an Office Open XML package".into());
        }
        // The end-of-central-directory record is in the last 22 + 65 535 bytes.
        let from = bytes.len().saturating_sub(22 + 0xFFFF);
        let tail = bytes.get(from..).unwrap_or_default();
        let eocd = tail
            .windows(4)
            .rposition(|w| w == b"PK\x05\x06")
            .map(|p| from.saturating_add(p))
            .ok_or("the package's directory is missing; the file may be truncated")?;
        let count = usize::from(u16_at(bytes, eocd.saturating_add(10)).ok_or("truncated package directory")?);
        let cd_offset = u32_at(bytes, eocd.saturating_add(16)).ok_or("truncated package directory")?;
        if cd_offset == u32::MAX || count == usize::from(u16::MAX) {
            return Err("the package uses zip64, which Office files of this size don't need".into());
        }
        if count > MAX_ENTRIES {
            return Err(format!("the package lists {count} parts; at most {MAX_ENTRIES} are read"));
        }
        let mut entries = Vec::with_capacity(count);
        let mut at = cd_offset as usize;
        for _ in 0..count {
            if u32_at(bytes, at) != Some(0x0201_4b50) {
                return Err("the package's directory is damaged".into());
            }
            let field = |o: usize| u16_at(bytes, at.saturating_add(o)).ok_or("the package's directory is damaged");
            let field32 = |o: usize| u32_at(bytes, at.saturating_add(o)).ok_or("the package's directory is damaged");
            let flags = field(8)?;
            let method = field(10)?;
            let compressed = field32(20)? as usize;
            let size = field32(24)? as usize;
            let name_len = usize::from(field(28)?);
            let extra_len = usize::from(field(30)?);
            let comment_len = usize::from(field(32)?);
            let offset = field32(42)? as usize;
            let name_at = at.saturating_add(46);
            let name = bytes.get(name_at..name_at.saturating_add(name_len)).ok_or("the package's directory is damaged")?;
            entries.push(Entry { name: String::from_utf8_lossy(name).replace('\\', "/"), method, flags, compressed, size, offset });
            at = name_at.saturating_add(name_len).saturating_add(extra_len).saturating_add(comment_len);
        }
        Ok(Zip { bytes, entries, read: std::cell::Cell::new(0) })
    }

    /// Whether a part exists (names compare case-insensitively, as OPC part names do).
    pub(crate) fn has(&self, name: &str) -> bool {
        self.find(name).is_some()
    }

    fn find(&self, name: &str) -> Option<&Entry> {
        let name = name.trim_start_matches('/');
        self.entries.iter().find(|e| e.name.eq_ignore_ascii_case(name))
    }

    /// The decompressed bytes of a part, `Ok(None)` when it is absent.
    pub(crate) fn read(&self, name: &str) -> Result<Option<Vec<u8>>, String> {
        let Some(e) = self.find(name) else { return Ok(None) };
        if e.flags & 1 != 0 {
            return Err(format!("{}: the part is encrypted", e.name));
        }
        if e.size > MAX_PART {
            return Err(format!("{}: the part is larger than {} MiB", e.name, MAX_PART >> 20));
        }
        let budget = MAX_TOTAL.saturating_sub(self.read.get());
        if e.size > budget {
            return Err(format!("the package expands to more than {} MiB", MAX_TOTAL >> 20));
        }
        if u32_at(self.bytes, e.offset) != Some(0x0403_4b50) {
            return Err(format!("{}: the part's header is damaged", e.name));
        }
        let name_len = usize::from(u16_at(self.bytes, e.offset.saturating_add(26)).ok_or("truncated part header")?);
        let extra_len = usize::from(u16_at(self.bytes, e.offset.saturating_add(28)).ok_or("truncated part header")?);
        let start = e.offset.saturating_add(30).saturating_add(name_len).saturating_add(extra_len);
        let data = self.bytes.get(start..start.saturating_add(e.compressed)).ok_or_else(|| format!("{}: the part is truncated", e.name))?;
        // The directory's size is a hint, not a promise: read at most one byte past it, and the cap.
        let cap = e.size.min(budget);
        let out = match e.method {
            0 => data.get(..cap.min(data.len())).unwrap_or_default().to_vec(),
            8 => {
                let mut out = Vec::new();
                flate2::read::DeflateDecoder::new(data)
                    .take(cap as u64 + 1)
                    .read_to_end(&mut out)
                    .map_err(|err| format!("{}: the part can't be decompressed ({err})", e.name))?;
                out
            }
            m => return Err(format!("{}: compression method {m} is not supported", e.name)),
        };
        if out.len() > cap {
            return Err(format!("{}: the part is larger than its directory says", e.name));
        }
        self.read.set(self.read.get().saturating_add(out.len()));
        Ok(Some(out))
    }

    /// A part as UTF-8 text (a byte order mark is dropped), `Ok(None)` when absent.
    pub(crate) fn text(&self, name: &str) -> Result<Option<String>, String> {
        let Some(bytes) = self.read(name)? else { return Ok(None) };
        let bytes = bytes.strip_prefix(&[0xEF, 0xBB, 0xBF]).unwrap_or(&bytes);
        // OOXML parts are UTF-8 or UTF-16 XML; UTF-16 is rare enough to decode leniently.
        if let Some((enc, bom)) = encoding_rs::Encoding::for_bom(bytes) {
            return Ok(Some(enc.decode_without_bom_handling(bytes.get(bom..).unwrap_or_default()).0.into_owned()));
        }
        Ok(Some(String::from_utf8_lossy(bytes).into_owned()))
    }
}

/// Resolve a relationship target against the part that owns it (OPC §9.3: targets are relative
/// URIs; a leading `/` starts at the package root). `%20` and friends are decoded.
pub(crate) fn resolve(source_part: &str, target: &str) -> String {
    let target = percent_decode(target);
    let mut parts: Vec<&str> = if target.starts_with('/') {
        Vec::new()
    } else {
        let dir = source_part.trim_start_matches('/').rsplit_once('/').map_or("", |(d, _)| d);
        dir.split('/').filter(|s| !s.is_empty()).collect()
    };
    for seg in target.split('/') {
        match seg {
            "" | "." => {}
            ".." => {
                parts.pop();
            }
            s => parts.push(s),
        }
    }
    parts.join("/")
}

fn percent_decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0usize;
    while let Some(&c) = b.get(i) {
        if c == b'%'
            && let (Some(h), Some(l)) = (b.get(i + 1).and_then(|c| (*c as char).to_digit(16)), b.get(i + 2).and_then(|c| (*c as char).to_digit(16)))
        {
            out.push((h * 16 + l) as u8);
            i += 3;
            continue;
        }
        out.push(c);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// A stored-or-deflated zip, for tests.
    pub(crate) fn build(parts: &[(&str, &[u8])]) -> Vec<u8> {
        use std::io::Write;
        let mut out = Vec::new();
        let mut central = Vec::new();
        for (i, (name, data)) in parts.iter().enumerate() {
            let deflate = i % 2 == 0;
            let body = if deflate {
                let mut e = flate2::write::DeflateEncoder::new(Vec::new(), flate2::Compression::default());
                e.write_all(data).unwrap();
                e.finish().unwrap()
            } else {
                data.to_vec()
            };
            let method: u16 = if deflate { 8 } else { 0 };
            let offset = out.len() as u32;
            out.extend_from_slice(b"PK\x03\x04");
            out.extend_from_slice(&[20, 0, 0, 0]);
            out.extend_from_slice(&method.to_le_bytes());
            out.extend_from_slice(&[0; 8]); // time, date, crc (not checked by the reader)
            out.extend_from_slice(&(body.len() as u32).to_le_bytes());
            out.extend_from_slice(&(data.len() as u32).to_le_bytes());
            out.extend_from_slice(&(name.len() as u16).to_le_bytes());
            out.extend_from_slice(&[0, 0]);
            out.extend_from_slice(name.as_bytes());
            out.extend_from_slice(&body);
            central.extend_from_slice(b"PK\x01\x02");
            central.extend_from_slice(&[20, 0, 20, 0, 0, 0]);
            central.extend_from_slice(&method.to_le_bytes());
            central.extend_from_slice(&[0; 8]);
            central.extend_from_slice(&(body.len() as u32).to_le_bytes());
            central.extend_from_slice(&(data.len() as u32).to_le_bytes());
            central.extend_from_slice(&(name.len() as u16).to_le_bytes());
            central.extend_from_slice(&[0; 12]);
            central.extend_from_slice(&offset.to_le_bytes());
            central.extend_from_slice(name.as_bytes());
        }
        let cd = out.len() as u32;
        out.extend_from_slice(&central);
        out.extend_from_slice(b"PK\x05\x06");
        out.extend_from_slice(&[0; 4]);
        out.extend_from_slice(&(parts.len() as u16).to_le_bytes());
        out.extend_from_slice(&(parts.len() as u16).to_le_bytes());
        out.extend_from_slice(&(central.len() as u32).to_le_bytes());
        out.extend_from_slice(&cd.to_le_bytes());
        out.extend_from_slice(&[0, 0]);
        out
    }

    #[test]
    fn reads_stored_and_deflated_parts() {
        let z = build(&[("a/b.xml", b"<x>deflated</x>"), ("c.txt", b"stored")]);
        let zip = Zip::open(&z).unwrap();
        assert_eq!(zip.read("a/b.xml").unwrap().unwrap(), b"<x>deflated</x>");
        assert_eq!(zip.read("/C.TXT").unwrap().unwrap(), b"stored");
        assert_eq!(zip.read("missing").unwrap(), None);
    }

    #[test]
    fn hostile_packages_are_refused_not_trusted() {
        assert!(Zip::open(b"not a zip").is_err());
        assert!(Zip::open(&[0xD0, 0xCF, 0x11, 0xE0, 0xA1, 0xB1, 0x1A, 0xE1, 0]).unwrap_err().contains("password"));
        let mut z = build(&[("a.xml", b"0123456789")]);
        let len = z.len();
        // Truncate inside the central directory: refused, never a panic.
        for cut in [len - 1, len - 30, 10, 4] {
            let _ = Zip::open(&z[..cut]).and_then(|zip| zip.read("a.xml"));
        }
        // A directory that understates the size can't make the reader inflate past it.
        let at = z.windows(4).position(|w| w == b"PK\x01\x02").unwrap() + 24;
        z[at..at + 4].copy_from_slice(&3u32.to_le_bytes());
        assert!(Zip::open(&z).unwrap().read("a.xml").unwrap_err().contains("larger than its directory"));
    }

    #[test]
    fn relationship_targets_resolve_like_uris() {
        assert_eq!(resolve("word/document.xml", "media/image1.png"), "word/media/image1.png");
        assert_eq!(resolve("ppt/slides/slide1.xml", "../media/a%20b.png"), "ppt/media/a b.png");
        assert_eq!(resolve("ppt/slides/slide1.xml", "/ppt/media/x.png"), "ppt/media/x.png");
        assert_eq!(resolve("x.xml", "../../../../etc/passwd"), "etc/passwd");
    }
}
