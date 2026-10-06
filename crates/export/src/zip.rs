//! A minimal zip writer (deflate or store, no zip64) for Office files.

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
        c = table[((c ^ u32::from(*b)) & 0xFF) as usize] ^ (c >> 8);
    }
    c ^ 0xFFFF_FFFF
}

/// Builds a zip archive in memory.
#[derive(Default)]
pub struct Zip {
    out: Vec<u8>,
    central: Vec<u8>,
    count: u16,
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
        let offset = self.out.len() as u32;
        let header = |sig: u32, central: bool| {
            let mut h = Vec::new();
            h.extend(sig.to_le_bytes());
            if central {
                h.extend(20u16.to_le_bytes()); // made by
            }
            h.extend(20u16.to_le_bytes()); // version needed
            h.extend(0x0800u16.to_le_bytes()); // UTF-8 names
            h.extend(method.to_le_bytes());
            h.extend([0u8; 4]); // time, date
            h.extend(crc.to_le_bytes());
            h.extend((body.len() as u32).to_le_bytes());
            h.extend((data.len() as u32).to_le_bytes());
            h.extend((name.len() as u16).to_le_bytes());
            h.extend(0u16.to_le_bytes()); // extra
            if central {
                h.extend(0u16.to_le_bytes()); // comment
                h.extend(0u16.to_le_bytes()); // disk
                h.extend(0u16.to_le_bytes()); // internal attributes
                h.extend(0u32.to_le_bytes()); // external attributes
                h.extend(offset.to_le_bytes());
            }
            h.extend(name.as_bytes());
            h
        };
        self.out.extend(header(0x0403_4b50, false));
        self.out.extend(&body);
        self.central.extend(header(0x0201_4b50, true));
        self.count += 1;
    }

    #[must_use]
    pub fn finish(mut self) -> Vec<u8> {
        let start = self.out.len() as u32;
        let size = self.central.len() as u32;
        self.out.append(&mut self.central);
        self.out.extend(0x0605_4b50u32.to_le_bytes());
        self.out.extend([0u8; 4]);
        self.out.extend(self.count.to_le_bytes());
        self.out.extend(self.count.to_le_bytes());
        self.out.extend(size.to_le_bytes());
        self.out.extend(start.to_le_bytes());
        self.out.extend(0u16.to_le_bytes());
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
}
