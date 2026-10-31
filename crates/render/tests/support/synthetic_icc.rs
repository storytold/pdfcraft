//! Synthetic ICC profiles for the Output Preview tests, written byte by byte from ICC.1 (version
//! 2.1 profiles with `lut16Type` tables and the legacy 16-bit PCS L*a*b* encoding). They are
//! contributor-original and generated when a test runs; no profile file is committed.
//!
//! The CMYK printing condition is made up so that its conversions can be worked out by hand:
//!
//! - AToB (CMYK → PCS L*a*b*): L* = 100 · (1 − (c + m + y + k) / 4), a* = 60 · (m − c),
//!   b* = 60 · (y − (c + m) / 2). It is affine, so a two-point grid holds it exactly.
//! - BToA (PCS → CMYK), its inverse with equal inks along the neutral axis: with
//!   t = 1 − L* / 100, c = t − a* / 120, m = t + a* / 120, y = t + b* / 60, k = t − b* / 60, each
//!   clamped to 0..1. A neutral of L* 50 prints as 50% of each ink, where the ISO 32000-2
//!   §10.4.2.4 device formulas would print it on black alone.
//!
//! Shared by the render, automation and UI tests through `#[path]`.

#![allow(dead_code)]

/// Grid points per input of the BToA table: fine enough that colours a little off the neutral
/// axis interpolate between unclamped grid points.
const BTOA_GRID: usize = 17;

/// What the synthetic printing condition prints for PCS L*a*b*, each ink `0.0..=1.0`.
pub fn btoa(l: f64, a: f64, b: f64) -> [f64; 4] {
    let t = 1.0 - l / 100.0;
    [t - a / 120.0, t + a / 120.0, t + b / 60.0, t - b / 60.0].map(|v| v.clamp(0.0, 1.0))
}

/// The PCS L*a*b* the synthetic printing condition gives an ink combination.
pub fn atob([c, m, y, k]: [f64; 4]) -> [f64; 3] {
    [100.0 * (1.0 - (c + m + y + k) / 4.0), 60.0 * (m - c), 60.0 * (y - (c + m) / 2.0)]
}

/// The CIE L* of an sRGB gray `v` (`0.0..=1.0`); a neutral's a* and b* are 0.
pub fn srgb_gray_lightness(v: f64) -> f64 {
    let linear = if v <= 0.04045 { v / 12.92 } else { ((v + 0.055) / 1.055).powf(2.4) };
    let f = if linear > 216.0 / 24389.0 { linear.cbrt() } else { (24389.0 / 27.0 * linear + 16.0) / 116.0 };
    116.0 * f - 16.0
}

/// A CMYK output profile (`prtr`) of the synthetic printing condition, described as
/// `description`. With `btoa` false it has no PCS-to-device tables (and so can't be printed to).
pub fn cmyk_profile(description: &str, btoa_tables: bool) -> Vec<u8> {
    let atob_table = lut16(4, 3, 2, false, |node| {
        let cmyk = [node[0], node[1], node[2], node[3]];
        encode_lab(atob(cmyk))
    });
    let mut tags: Vec<(&[u8; 4], Vec<u8>)> = vec![
        (b"desc", text_description(description)),
        (b"cprt", text(b"No copyright, synthetic test data")),
        (b"wtpt", xyz(D50)),
        (b"A2B0", atob_table.clone()),
        (b"A2B1", atob_table.clone()),
        (b"A2B2", atob_table),
    ];
    if btoa_tables {
        let btoa_table = lut16(3, 4, BTOA_GRID, true, |node| {
            let [l, a, b] = grid_lab([node[0], node[1], node[2]]);
            btoa(l, a, b).map(|v| v * 65535.0).to_vec()
        });
        tags.push((b"B2A0", btoa_table.clone()));
        tags.push((b"B2A1", btoa_table.clone()));
        tags.push((b"B2A2", btoa_table));
        // Everything is in gamut.
        tags.push((b"gamt", lut16(3, 1, 2, false, |_| vec![0.0])));
    }
    profile(b"prtr", b"CMYK", b"Lab ", &tags)
}

/// A gray display profile (gamma 2.2), described as `description`: an output intent that is
/// not CMYK.
pub fn gray_profile(description: &str) -> Vec<u8> {
    // curveType with one entry: a gamma as u8Fixed8Number (2.2 ≈ 0x0233).
    let mut curve = b"curv\0\0\0\0".to_vec();
    curve.extend_from_slice(&1_u32.to_be_bytes());
    curve.extend_from_slice(&0x0233_u16.to_be_bytes());
    let tags: Vec<(&[u8; 4], Vec<u8>)> =
        vec![(b"desc", text_description(description)), (b"cprt", text(b"No copyright, synthetic test data")), (b"wtpt", xyz(D50)), (b"kTRC", curve)];
    profile(b"mntr", b"GRAY", b"XYZ ", &tags)
}

/// The D50 illuminant of the PCS.
const D50: [f64; 3] = [0.9642, 1.0, 0.8249];

/// PCS L*a*b* in the legacy 16-bit encoding (ICC.1 Annex A), as `0.0..=65535.0` table values:
/// L* 100 is 0xFF00 and a* or b* 0 is 0x8000.
fn encode_lab([l, a, b]: [f64; 3]) -> Vec<f64> {
    vec![l * 65280.0 / 100.0, (a + 128.0) * 256.0, (b + 128.0) * 256.0].into_iter().map(|v| v.clamp(0.0, 65535.0)).collect()
}

/// A BToA grid point's PCS L*a*b*: `node` holds its inputs as `0.0..=1.0`. The L* input
/// table has stretched L* 100 (0xFF00) onto the last grid point, so white prints no ink; a*
/// and b* are in the legacy encoding.
fn grid_lab(node: [f64; 3]) -> [f64; 3] {
    let [l, a, b] = node;
    [l * 100.0, a * 65535.0 / 256.0 - 128.0, b * 65535.0 / 256.0 - 128.0]
}

/// A `lut16Type` with `inputs` × `outputs` channels, `grid` points per input, identity
/// matrix and output tables, and the grid filled by `value` (each grid point's inputs as
/// `0.0..=1.0`, returning its outputs as `0.0..=65535.0`). The input tables are the identity,
/// except that `stretch_lightness` scales the first input by 0xFFFF / 0xFF00 (see [`grid_lab`]).
fn lut16(inputs: usize, outputs: usize, grid: usize, stretch_lightness: bool, value: impl Fn(&[f64]) -> Vec<f64>) -> Vec<u8> {
    const ENTRIES: usize = 257;
    let mut out = b"mft2\0\0\0\0".to_vec();
    out.extend_from_slice(&[inputs as u8, outputs as u8, grid as u8, 0]);
    for k in 0_usize..9 {
        let one = if k.is_multiple_of(4) { 0x0001_0000_i32 } else { 0 };
        out.extend_from_slice(&one.to_be_bytes());
    }
    out.extend_from_slice(&(ENTRIES as u16).to_be_bytes());
    out.extend_from_slice(&2_u16.to_be_bytes());
    for channel in 0..inputs {
        for e in 0..ENTRIES {
            let x = e as f64 / (ENTRIES - 1) as f64;
            let y = if stretch_lightness && channel == 0 { (x * 65535.0 / 65280.0).min(1.0) } else { x };
            out.extend_from_slice(&((y * 65535.0).round() as u16).to_be_bytes());
        }
    }
    // The first input varies slowest.
    let points = grid.pow(inputs as u32);
    for index in 0..points {
        let mut node = vec![0.0; inputs];
        let mut rest = index;
        for i in (0..inputs).rev() {
            node[i] = (rest % grid) as f64 / (grid - 1) as f64;
            rest /= grid;
        }
        let values = value(&node);
        assert_eq!(values.len(), outputs);
        for v in values {
            out.extend_from_slice(&(v.round().clamp(0.0, 65535.0) as u16).to_be_bytes());
        }
    }
    for _ in 0..outputs {
        out.extend_from_slice(&[0, 0, 0xFF, 0xFF]);
    }
    out
}

/// A `textDescriptionType` (ICC.1:2001, the version 2 `desc` tag) with an ASCII description.
fn text_description(description: &str) -> Vec<u8> {
    let ascii: Vec<u8> = description.bytes().filter(u8::is_ascii).collect();
    let mut out = b"desc\0\0\0\0".to_vec();
    out.extend_from_slice(&(ascii.len() as u32 + 1).to_be_bytes());
    out.extend_from_slice(&ascii);
    out.push(0);
    out.extend_from_slice(&[0; 4]); // Unicode language code
    out.extend_from_slice(&[0; 4]); // Unicode count
    out.extend_from_slice(&[0; 2]); // ScriptCode code
    out.push(0); // ScriptCode count
    out.extend_from_slice(&[0; 67]);
    out
}

/// A `textType`.
fn text(ascii: &[u8]) -> Vec<u8> {
    let mut out = b"text\0\0\0\0".to_vec();
    out.extend_from_slice(ascii);
    out.push(0);
    out
}

fn s15_fixed16(v: f64) -> [u8; 4] {
    ((v * 65536.0).round() as i32).to_be_bytes()
}

/// An `XYZType` with one value.
fn xyz(v: [f64; 3]) -> Vec<u8> {
    let mut out = b"XYZ \0\0\0\0".to_vec();
    for c in v {
        out.extend_from_slice(&s15_fixed16(c));
    }
    out
}

/// A whole version 2.1 profile: the 128-byte header, the tag table, then each tag's data,
/// 4-byte aligned. Tags with equal data share it.
fn profile(class: &[u8; 4], space: &[u8; 4], pcs: &[u8; 4], tags: &[(&[u8; 4], Vec<u8>)]) -> Vec<u8> {
    let table_end = 128 + 4 + 12 * tags.len();
    let mut data: Vec<u8> = Vec::new();
    let mut entries: Vec<(&[u8; 4], usize, usize)> = Vec::new();
    for (signature, body) in tags {
        let shared = entries.iter().zip(tags).find(|(_, (_, b))| b == body).map(|((_, offset, len), _)| (*offset, *len));
        let (offset, len) = match shared {
            Some(place) => place,
            None => {
                while !data.len().is_multiple_of(4) {
                    data.push(0);
                }
                let offset = table_end + data.len();
                data.extend_from_slice(body);
                (offset, body.len())
            }
        };
        entries.push((signature, offset, len));
    }
    while !data.len().is_multiple_of(4) {
        data.push(0);
    }
    let size = table_end + data.len();
    let mut out = Vec::with_capacity(size);
    out.extend_from_slice(&(size as u32).to_be_bytes());
    out.extend_from_slice(&[0; 4]); // preferred CMM
    out.extend_from_slice(&0x0210_0000_u32.to_be_bytes());
    out.extend_from_slice(class);
    out.extend_from_slice(space);
    out.extend_from_slice(pcs);
    for v in [2026_u16, 10, 16, 0, 0, 0] {
        out.extend_from_slice(&v.to_be_bytes());
    }
    out.extend_from_slice(b"acsp");
    out.extend_from_slice(&[0; 4 + 4 + 4 + 4 + 8 + 4]); // platform, flags, manufacturer, model, attributes, intent
    for c in D50 {
        out.extend_from_slice(&s15_fixed16(c));
    }
    out.extend_from_slice(&[0; 4 + 16 + 28]); // creator, ID, reserved
    out.extend_from_slice(&(tags.len() as u32).to_be_bytes());
    for (signature, offset, len) in &entries {
        out.extend_from_slice(*signature);
        out.extend_from_slice(&(*offset as u32).to_be_bytes());
        out.extend_from_slice(&(*len as u32).to_be_bytes());
    }
    out.extend_from_slice(&data);
    out
}
