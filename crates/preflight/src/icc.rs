//! Small ICC profiles (version 2, display class) written here rather than shipped: sRGB
//! (matrix/TRC), from the published sRGB primaries (IEC 61966-2-1) adapted to D50, and a gray
//! profile with the same tone curve, both sampled at 1024 points. sRGB is the PDF/A output
//! intent's destination profile and what untagged RGB is taken to be; the gray profile maps
//! device gray (`DefaultGray`) under an RGB printing condition.

fn s15f16(v: f64) -> [u8; 4] {
    ((v * 65536.0).round() as i32).to_be_bytes()
}

fn xyz(x: f64, y: f64, z: f64) -> Vec<u8> {
    let mut t = b"XYZ \0\0\0\0".to_vec();
    t.extend(s15f16(x));
    t.extend(s15f16(y));
    t.extend(s15f16(z));
    t
}

fn desc(text: &str) -> Vec<u8> {
    let mut t = b"desc\0\0\0\0".to_vec();
    let ascii: Vec<u8> = text.bytes().chain(std::iter::once(0)).collect();
    t.extend((ascii.len() as u32).to_be_bytes());
    t.extend(&ascii);
    // No Unicode or ScriptCode descriptions.
    t.extend([0u8; 4 + 4]);
    t.extend([0u8; 2 + 1 + 67]);
    t
}

fn text(text: &str) -> Vec<u8> {
    let mut t = b"text\0\0\0\0".to_vec();
    t.extend(text.bytes());
    t.push(0);
    t
}

fn curve() -> Vec<u8> {
    const N: usize = 1024;
    let mut t = b"curv\0\0\0\0".to_vec();
    t.extend((N as u32).to_be_bytes());
    for i in 0..N {
        let v = i as f64 / (N - 1) as f64;
        let lin = if v <= 0.04045 { v / 12.92 } else { ((v + 0.055) / 1.055).powf(2.4) };
        t.extend(((lin * 65535.0).round() as u16).to_be_bytes());
    }
    t
}

/// The sRGB profile's bytes.
pub fn srgb() -> Vec<u8> {
    // sRGB primaries, Bradford-adapted to the D50 profile connection space.
    let tags: Vec<(&[u8; 4], Vec<u8>)> = vec![
        (b"desc", desc("sRGB (PdfCraft)")),
        (b"cprt", text("No copyright, use freely")),
        (b"wtpt", xyz(0.9642, 1.0, 0.8249)),
        (b"rXYZ", xyz(0.4361, 0.2225, 0.0139)),
        (b"gXYZ", xyz(0.3851, 0.7169, 0.0971)),
        (b"bXYZ", xyz(0.1431, 0.0606, 0.7141)),
        (b"rTRC", curve()),
    ];
    // gTRC and bTRC share rTRC's data.
    build(b"RGB ", &tags, &[(b"gTRC", b"rTRC"), (b"bTRC", b"rTRC")])
}

/// The gray profile's bytes: the D50 white and the sRGB tone curve.
pub fn gray() -> Vec<u8> {
    let tags: Vec<(&[u8; 4], Vec<u8>)> = vec![
        (b"desc", desc("Gray, sRGB tone curve (PdfCraft)")),
        (b"cprt", text("No copyright, use freely")),
        (b"wtpt", xyz(0.9642, 1.0, 0.8249)),
        (b"kTRC", curve()),
    ];
    build(b"GRAY", &tags, &[])
}

/// A display profile of colour space `space` holding `tags`, then `aliases`: tags that share an
/// earlier tag's data.
fn build(space: &[u8; 4], tags: &[(&[u8; 4], Vec<u8>)], aliases: &[(&[u8; 4], &[u8; 4])]) -> Vec<u8> {
    let count = tags.len() + aliases.len();
    let table_len = 4 + count * 12;
    let mut offset = 128 + table_len;
    let mut table = (count as u32).to_be_bytes().to_vec();
    let mut data = Vec::new();
    let mut placed: Vec<(&[u8; 4], usize, usize)> = Vec::new();
    for (sig, body) in tags {
        let len = body.len();
        table.extend(*sig);
        table.extend((offset as u32).to_be_bytes());
        table.extend((len as u32).to_be_bytes());
        placed.push((sig, offset, len));
        data.extend(body);
        // Tags start on 4-byte boundaries.
        let pad = (4 - len % 4) % 4;
        data.extend(std::iter::repeat_n(0u8, pad));
        offset += len + pad;
    }
    for (alias, of) in aliases {
        let (at, len) = placed.iter().find(|(sig, _, _)| sig == of).map_or((0, 0), |(_, at, len)| (*at, *len));
        table.extend(*alias);
        table.extend((at as u32).to_be_bytes());
        table.extend((len as u32).to_be_bytes());
    }
    let size = 128 + table.len() + data.len();
    let mut h = Vec::with_capacity(size);
    h.extend((size as u32).to_be_bytes());
    h.extend(b"\0\0\0\0"); // preferred CMM
    h.extend([2, 0x10, 0, 0]); // version 2.1
    h.extend(b"mntr");
    h.extend(space);
    h.extend(b"XYZ ");
    h.extend([0u8; 12]); // date
    h.extend(b"acsp");
    h.extend(b"\0\0\0\0"); // platform
    h.extend([0u8; 4]); // flags
    h.extend([0u8; 4]); // manufacturer
    h.extend([0u8; 4]); // model
    h.extend([0u8; 8]); // attributes
    h.extend([0, 0, 0, 0]); // perceptual intent
    h.extend(s15f16(0.9642));
    h.extend(s15f16(1.0));
    h.extend(s15f16(0.8249));
    h.extend([0u8; 4]); // creator
    h.resize(128, 0);
    h.extend(table);
    h.extend(data);
    h
}

#[cfg(test)]
mod tests {
    #[test]
    fn gray_profile_is_well_formed() {
        let p = super::gray();
        assert_eq!(u32::from_be_bytes(p[0..4].try_into().unwrap()) as usize, p.len());
        assert_eq!(&p[12..20], b"mntrGRAY");
        assert_eq!(u32::from_be_bytes(p[128..132].try_into().unwrap()), 4);
        let tag = |i: usize| &p[132 + i * 12..132 + i * 12 + 4];
        assert_eq!(tag(3), b"kTRC");
    }

    #[test]
    fn profile_is_well_formed() {
        let p = super::srgb();
        assert_eq!(u32::from_be_bytes(p[0..4].try_into().unwrap()) as usize, p.len());
        assert_eq!(&p[36..40], b"acsp");
        assert_eq!(&p[12..20], b"mntrRGB ");
        let n = u32::from_be_bytes(p[128..132].try_into().unwrap()) as usize;
        assert_eq!(n, 9);
        for i in 0..n {
            let e = 132 + i * 12;
            let off = u32::from_be_bytes(p[e + 4..e + 8].try_into().unwrap()) as usize;
            let len = u32::from_be_bytes(p[e + 8..e + 12].try_into().unwrap()) as usize;
            assert!(off.is_multiple_of(4) && off + len <= p.len(), "tag {i}");
        }
    }
}
