//! A synthetic CMYK output (printer) ICC profile for tests: a naive press whose cyan, magenta and
//! yellow are the complements of sRGB's red, green and blue, with no black (C = 1 − R, M = 1 − G,
//! Y = 1 − B, K = 0; K darkens all three on the way back). It describes no real printing condition:
//! it gives conversion tests a profile with real tables (version 2, Lab connection space, `lut16`
//! tables for every intent) whose answers are known, without shipping anyone's press profile.

/// The sRGB primaries adapted to D50 (the columns are R, G and B), as PdfCraft's own sRGB profile
/// writes them.
const RGB_TO_XYZ: [[f64; 3]; 3] = [[0.4361, 0.3851, 0.1431], [0.2225, 0.7169, 0.0606], [0.0139, 0.0971, 0.7141]];
/// The D50 white.
const WHITE: [f64; 3] = [0.9642, 1.0, 0.8249];

fn invert(m: [[f64; 3]; 3]) -> [[f64; 3]; 3] {
    let [[a, b, c], [d, e, f], [g, h, i]] = m;
    let det = a * (e * i - f * h) - b * (d * i - f * g) + c * (d * h - e * g);
    [
        [(e * i - f * h) / det, (c * h - b * i) / det, (b * f - c * e) / det],
        [(f * g - d * i) / det, (a * i - c * g) / det, (c * d - a * f) / det],
        [(d * h - e * g) / det, (b * g - a * h) / det, (a * e - b * d) / det],
    ]
}

fn times(m: [[f64; 3]; 3], v: [f64; 3]) -> [f64; 3] {
    [0, 1, 2].map(|r| m[r][0] * v[0] + m[r][1] * v[1] + m[r][2] * v[2])
}

fn linear(v: f64) -> f64 {
    if v <= 0.04045 { v / 12.92 } else { ((v + 0.055) / 1.055).powf(2.4) }
}

fn encoded(v: f64) -> f64 {
    if v <= 0.003_130_8 { v * 12.92 } else { 1.055 * v.powf(1.0 / 2.4) - 0.055 }
}

fn lab_of(xyz: [f64; 3]) -> [f64; 3] {
    let f = |t: f64| if t > (6.0f64 / 29.0).powi(3) { t.cbrt() } else { t / (3.0 * (6.0f64 / 29.0).powi(2)) + 4.0 / 29.0 };
    let [fx, fy, fz] = [0, 1, 2].map(|i| f(xyz[i] / WHITE[i]));
    [116.0 * fy - 16.0, 500.0 * (fx - fy), 200.0 * (fy - fz)]
}

fn xyz_of(lab: [f64; 3]) -> [f64; 3] {
    let fy = (lab[0] + 16.0) / 116.0;
    let fx = fy + lab[1] / 500.0;
    let fz = fy - lab[2] / 200.0;
    let g = |t: f64| if t > 6.0 / 29.0 { t.powi(3) } else { 3.0 * (6.0f64 / 29.0).powi(2) * (t - 4.0 / 29.0) };
    [g(fx) * WHITE[0], g(fy) * WHITE[1], g(fz) * WHITE[2]]
}

/// Version 2 16-bit Lab: L* 0–100 is 0–0xFF00, a* and b* −128–127.996 are 0–0xFFFF.
fn lab_from_u16(v: [u16; 3]) -> [f64; 3] {
    [f64::from(v[0]) * 100.0 / 65280.0, f64::from(v[1]) / 256.0 - 128.0, f64::from(v[2]) / 256.0 - 128.0]
}

fn lab_to_u16(lab: [f64; 3]) -> [u16; 3] {
    let q = |v: f64| v.round().clamp(0.0, 65535.0) as u16;
    [q(lab[0] * 65280.0 / 100.0), q((lab[1] + 128.0) * 256.0), q((lab[2] + 128.0) * 256.0)]
}

fn unit(v: f64) -> u16 {
    (v.clamp(0.0, 1.0) * 65535.0).round() as u16
}

/// A `lut16Type` tag: identity input and output curves, a grid of `grid` points per input.
fn lut16(inputs: usize, outputs: usize, grid: usize, clut: &[u16]) -> Vec<u8> {
    let mut t = b"mft2\0\0\0\0".to_vec();
    t.extend([inputs as u8, outputs as u8, grid as u8, 0]);
    // The identity matrix (used only with an XYZ connection space).
    for v in [1, 0, 0, 0, 1, 0, 0, 0, 1] {
        t.extend((v * 65536i32).to_be_bytes());
    }
    t.extend(2u16.to_be_bytes());
    t.extend(2u16.to_be_bytes());
    for _ in 0..inputs {
        t.extend(0u16.to_be_bytes());
        t.extend(65535u16.to_be_bytes());
    }
    for v in clut {
        t.extend(v.to_be_bytes());
    }
    for _ in 0..outputs {
        t.extend(0u16.to_be_bytes());
        t.extend(65535u16.to_be_bytes());
    }
    t
}

/// Lab → CMYK on a grid of `grid` points per channel.
fn to_device(grid: usize) -> Vec<u8> {
    let to_rgb = invert(RGB_TO_XYZ);
    let step = |i: usize| (i as f64 * 65535.0 / (grid - 1) as f64).round() as u16;
    let mut clut = Vec::with_capacity(grid * grid * grid * 4);
    for l in 0..grid {
        for a in 0..grid {
            for b in 0..grid {
                let lab = lab_from_u16([step(l), step(a), step(b)]);
                let rgb = times(to_rgb, xyz_of(lab)).map(|v| encoded(v.clamp(0.0, 1.0)));
                clut.extend([unit(1.0 - rgb[0]), unit(1.0 - rgb[1]), unit(1.0 - rgb[2]), 0]);
            }
        }
    }
    lut16(3, 4, grid, &clut)
}

/// CMYK → Lab on a grid of `grid` points per channel.
fn to_pcs(grid: usize) -> Vec<u8> {
    let at = |i: usize| i as f64 / (grid - 1) as f64;
    let mut clut = Vec::with_capacity(grid.pow(4) * 3);
    for c in 0..grid {
        for m in 0..grid {
            for y in 0..grid {
                for k in 0..grid {
                    let rgb = [c, m, y].map(|v| linear((1.0 - at(v)) * (1.0 - at(k))));
                    clut.extend(lab_to_u16(lab_of(times(RGB_TO_XYZ, rgb))));
                }
            }
        }
    }
    lut16(4, 3, grid, &clut)
}

fn desc(text: &str) -> Vec<u8> {
    let mut t = b"desc\0\0\0\0".to_vec();
    t.extend(((text.len() + 1) as u32).to_be_bytes());
    t.extend(text.as_bytes());
    t.push(0);
    t.extend([0u8; 8]);
    t.extend([0u8; 70]);
    t
}

fn xyz(v: [f64; 3]) -> Vec<u8> {
    let mut t = b"XYZ \0\0\0\0".to_vec();
    for c in v {
        t.extend(((c * 65536.0).round() as i32).to_be_bytes());
    }
    t
}

/// The profile's bytes, with `description` as its `desc` tag.
pub fn cmyk_printer_profile(description: &str) -> Vec<u8> {
    let a2b = to_pcs(5);
    let b2a = to_device(17);
    let mut cprt = b"text\0\0\0\0".to_vec();
    cprt.extend(b"No copyright, use freely\0");
    let tags: Vec<(&[u8; 4], Vec<u8>)> = vec![(b"desc", desc(description)), (b"cprt", cprt), (b"wtpt", xyz(WHITE)), (b"A2B0", a2b), (b"B2A0", b2a)];
    // Every intent shares the perceptual tables.
    let aliases: [(&[u8; 4], &[u8; 4]); 4] = [(b"A2B1", b"A2B0"), (b"A2B2", b"A2B0"), (b"B2A1", b"B2A0"), (b"B2A2", b"B2A0")];
    let count = tags.len() + aliases.len();
    let mut offset = 128 + 4 + count * 12;
    let mut table = (count as u32).to_be_bytes().to_vec();
    let mut data = Vec::new();
    let mut placed = Vec::new();
    for (sig, body) in &tags {
        table.extend(*sig);
        table.extend((offset as u32).to_be_bytes());
        table.extend((body.len() as u32).to_be_bytes());
        placed.push((*sig, offset, body.len()));
        data.extend(body);
        let pad = (4 - body.len() % 4) % 4;
        data.extend(std::iter::repeat_n(0u8, pad));
        offset += body.len() + pad;
    }
    for (alias, of) in aliases {
        let (_, at, len) = placed.iter().find(|(sig, _, _)| *sig == of).copied().unwrap_or((of, 0, 0));
        table.extend(alias);
        table.extend((at as u32).to_be_bytes());
        table.extend((len as u32).to_be_bytes());
    }
    let size = 128 + table.len() + data.len();
    let mut h = Vec::with_capacity(size);
    h.extend((size as u32).to_be_bytes());
    h.extend([0u8; 4]);
    h.extend([2, 0x10, 0, 0]);
    h.extend(b"prtr");
    h.extend(b"CMYK");
    h.extend(b"Lab ");
    h.extend([0u8; 12]);
    h.extend(b"acsp");
    h.extend([0u8; 24]);
    h.extend([0, 0, 0, 1]); // relative colorimetric
    h.extend(xyz(WHITE).get(8..).unwrap_or_default());
    h.resize(128, 0);
    h.extend(table);
    h.extend(data);
    h
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lab_round_trips() {
        for lab in [[50.0, 20.0, -30.0], [100.0, 0.0, 0.0], [0.0, 0.0, 0.0]] {
            let back = lab_from_u16(lab_to_u16(lab));
            assert!(lab.iter().zip(back).all(|(a, b)| (a - b).abs() < 0.01), "{lab:?} {back:?}");
            let again = lab_of(xyz_of(lab));
            assert!(lab.iter().zip(again).all(|(a, b)| (a - b).abs() < 1e-6), "{lab:?} {again:?}");
        }
    }

    #[test]
    fn profile_is_well_formed() {
        let p = cmyk_printer_profile("Test press");
        assert_eq!(u32::from_be_bytes(p[0..4].try_into().unwrap()) as usize, p.len());
        assert_eq!(&p[12..24], b"prtrCMYKLab ");
        assert_eq!(&p[36..40], b"acsp");
    }
}
