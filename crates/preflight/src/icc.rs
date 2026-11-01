//! Small ICC profiles (version 2, display class) written here rather than shipped: sRGB
//! (matrix/TRC), from the published sRGB primaries (IEC 61966-2-1) adapted to D50, and a gray
//! profile with the same tone curve, both sampled at 1024 points. sRGB is the PDF/A output
//! intent's destination profile and what untagged RGB is taken to be; the gray profile maps
//! device gray (`DefaultGray`) under an RGB printing condition.
//!
//! Converting colour to CMYK also writes profiles for PDF's own device-independent spaces (ISO
//! 32000-2 8.6.5): CalGray and CalRGB are gamma curves (and a matrix) under a white point, which
//! is what a matrix/TRC profile is once the white is adapted to D50 (Bradford, as ICC profiles
//! adapt); Lab goes through a version 4 Lab profile that passes its values to the connection space
//! unchanged, after [`lab_to_d50`] has adapted them.

/// The D50 white of the profile connection space.
pub(crate) const D50: [f64; 3] = [0.9642, 1.0, 0.8249];

fn s15f16(v: f64) -> [u8; 4] {
    ((v * 65536.0).round() as i32).to_be_bytes()
}

/// The Bradford cone response matrix.
const BRADFORD: [[f64; 3]; 3] = [[0.8951, 0.2664, -0.1614], [-0.7502, 1.7135, 0.0367], [0.0389, -0.0685, 1.0296]];

fn times(m: &[[f64; 3]; 3], v: [f64; 3]) -> [f64; 3] {
    [0, 1, 2].map(|r| m[r][0] * v[0] + m[r][1] * v[1] + m[r][2] * v[2])
}

fn product(a: &[[f64; 3]; 3], b: &[[f64; 3]; 3]) -> [[f64; 3]; 3] {
    [0, 1, 2].map(|r| [0, 1, 2].map(|c| a[r][0] * b[0][c] + a[r][1] * b[1][c] + a[r][2] * b[2][c]))
}

fn invert(m: &[[f64; 3]; 3]) -> Option<[[f64; 3]; 3]> {
    let [[a, b, c], [d, e, f], [g, h, i]] = *m;
    let det = a * (e * i - f * h) - b * (d * i - f * g) + c * (d * h - e * g);
    if det.abs() < 1e-12 || !det.is_finite() {
        return None;
    }
    Some([
        [(e * i - f * h) / det, (c * h - b * i) / det, (b * f - c * e) / det],
        [(f * g - d * i) / det, (a * i - c * g) / det, (c * d - a * f) / det],
        [(d * h - e * g) / det, (b * g - a * h) / det, (a * e - b * d) / det],
    ])
}

/// The Bradford transform taking XYZ under the white `from` to XYZ under `to`. `None` for a white
/// with no cone response (it must have positive Y, X and Z).
pub(crate) fn bradford(from: [f64; 3], to: [f64; 3]) -> Option<[[f64; 3]; 3]> {
    let (s, d) = (times(&BRADFORD, from), times(&BRADFORD, to));
    if s.iter().chain(&d).any(|v| v.abs() < 1e-9 || !v.is_finite()) {
        return None;
    }
    let scale = [[d[0] / s[0], 0.0, 0.0], [0.0, d[1] / s[1], 0.0], [0.0, 0.0, d[2] / s[2]]];
    Some(product(&invert(&BRADFORD)?, &product(&scale, &BRADFORD)))
}

/// CIE L*a*b* under the white `white` (ISO 32000-2 8.6.5.4) as L*a*b* under D50, Bradford-adapted.
pub(crate) fn lab_to_d50(lab: [f64; 3], white: [f64; 3], adapt: &[[f64; 3]; 3]) -> [f64; 3] {
    let g = |x: f64| if x >= 6.0 / 29.0 { x * x * x } else { 108.0 / 841.0 * (x - 4.0 / 29.0) };
    let m = (lab[0] + 16.0) / 116.0;
    let xyz = [white[0] * g(m + lab[1] / 500.0), white[1] * g(m), white[2] * g(m - lab[2] / 200.0)];
    let [x, y, z] = times(adapt, xyz);
    let f = |t: f64| if t > (6.0f64 / 29.0).powi(3) { t.cbrt() } else { t * 841.0 / 108.0 + 4.0 / 29.0 };
    let (fx, fy, fz) = (f(x / D50[0]), f(y / D50[1]), f(z / D50[2]));
    [116.0 * fy - 16.0, 500.0 * (fx - fy), 200.0 * (fy - fz)]
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

/// A pure gamma curve (`parametricCurveType`, function 0).
fn gamma(g: f64) -> Vec<u8> {
    let mut t = b"para\0\0\0\0".to_vec();
    t.extend([0u8; 4]);
    t.extend(s15f16(g));
    t
}

/// A CalRGB space's profile: the columns of `matrix` (the XYZ of each component at full strength,
/// under `white`) adapted to D50, and each component's `gamma`. `None` for a white that can't be
/// adapted.
pub(crate) fn calibrated_rgb(white: [f64; 3], gamma_rgb: [f64; 3], matrix: [f64; 9]) -> Option<Vec<u8>> {
    let adapt = bradford(white, D50)?;
    let column = |i: usize| {
        let [x, y, z] = times(&adapt, [matrix[i], matrix[i + 1], matrix[i + 2]]);
        xyz(x, y, z)
    };
    let tags: Vec<(&[u8; 4], Vec<u8>)> = vec![
        (b"desc", desc("CalRGB (PdfCraft)")),
        (b"cprt", text("No copyright, use freely")),
        (b"wtpt", xyz(D50[0], D50[1], D50[2])),
        (b"rXYZ", column(0)),
        (b"gXYZ", column(3)),
        (b"bXYZ", column(6)),
        (b"rTRC", gamma(gamma_rgb[0])),
        (b"gTRC", gamma(gamma_rgb[1])),
        (b"bTRC", gamma(gamma_rgb[2])),
    ];
    Some(build(b"RGB ", &tags, &[]))
}

/// A CalGray space's profile: its gamma, with its white adapted to D50 (relative colorimetry maps
/// a white to the connection space's).
pub(crate) fn calibrated_gray(g: f64) -> Vec<u8> {
    let tags: Vec<(&[u8; 4], Vec<u8>)> = vec![
        (b"desc", desc("CalGray (PdfCraft)")),
        (b"cprt", text("No copyright, use freely")),
        (b"wtpt", xyz(D50[0], D50[1], D50[2])),
        (b"kTRC", gamma(g)),
    ];
    build(b"GRAY", &tags, &[])
}

/// A version 4 multiLocalizedUnicode tag holding `text` in US English.
fn mluc(text: &str) -> Vec<u8> {
    let utf16: Vec<u8> = text.encode_utf16().flat_map(u16::to_be_bytes).collect();
    let mut t = b"mluc\0\0\0\0".to_vec();
    t.extend(1u32.to_be_bytes());
    t.extend(12u32.to_be_bytes());
    t.extend(b"enUS");
    t.extend((utf16.len() as u32).to_be_bytes());
    t.extend(28u32.to_be_bytes());
    t.extend(utf16);
    t
}

/// An identity `lutAToBType` or `lutBToAType` (`sig`) on three channels: B curves only, each the
/// identity.
fn identity_lut(sig: &[u8; 4]) -> Vec<u8> {
    let mut t = sig.to_vec();
    t.extend([0u8; 4]);
    t.extend([3, 3, 0, 0]);
    // Offsets to the B curves, matrix, M curves, CLUT and A curves: only the B curves.
    t.extend(32u32.to_be_bytes());
    t.extend([0u8; 16]);
    for _ in 0..3 {
        t.extend(b"curv\0\0\0\0");
        t.extend(0u32.to_be_bytes());
    }
    t
}

/// A version 4 colour space profile for L*a*b* under D50 that passes values to the connection
/// space (also L*a*b*) unchanged, so converting from it is converting the connection space.
pub(crate) fn lab() -> Vec<u8> {
    let tags: Vec<(&[u8; 4], Vec<u8>)> = vec![
        (b"desc", mluc("L*a*b* D50 (PdfCraft)")),
        (b"cprt", mluc("No copyright, use freely")),
        (b"wtpt", xyz(D50[0], D50[1], D50[2])),
        (b"A2B0", identity_lut(b"mAB ")),
        (b"B2A0", identity_lut(b"mBA ")),
    ];
    build_with(Header { version: [4, 0x30, 0, 0], class: b"spac", space: b"Lab ", pcs: b"Lab " }, &tags, &[])
}

/// What a profile's header says about it.
struct Header {
    version: [u8; 4],
    class: &'static [u8; 4],
    space: &'static [u8; 4],
    pcs: &'static [u8; 4],
}

/// A display profile of colour space `space` holding `tags`, then `aliases`: tags that share an
/// earlier tag's data.
fn build(space: &'static [u8; 4], tags: &[(&[u8; 4], Vec<u8>)], aliases: &[(&[u8; 4], &[u8; 4])]) -> Vec<u8> {
    build_with(Header { version: [2, 0x10, 0, 0], class: b"mntr", space, pcs: b"XYZ " }, tags, aliases)
}

fn build_with(header: Header, tags: &[(&[u8; 4], Vec<u8>)], aliases: &[(&[u8; 4], &[u8; 4])]) -> Vec<u8> {
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
    h.extend(header.version);
    h.extend(header.class);
    h.extend(header.space);
    h.extend(header.pcs);
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
