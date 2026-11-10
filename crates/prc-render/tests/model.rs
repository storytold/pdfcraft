//! End to end: a synthetic PRC stream parsed, assembled and rendered to pixels.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use pdfcraft_prc::PrcFile;
use pdfcraft_prc_render::{Camera, Image, RenderOptions, assemble, render};
use std::io::Write as _;

/// An MSB-first bit sink: the inverse of the PRC bit reader, for the few values these tests write.
#[derive(Default)]
struct W {
    bits: Vec<bool>,
}

impl W {
    fn put(&mut self, v: u64, n: u32) -> &mut Self {
        for i in (0..n).rev() {
            self.bits.push((v >> i) & 1 == 1);
        }
        self
    }

    fn bit(&mut self, b: bool) -> &mut Self {
        self.bits.push(b);
        self
    }

    /// An `UnsignedInteger`: bytes low first, each after a 1 bit, then a 0 bit.
    fn uint(&mut self, mut v: u32) -> &mut Self {
        while v != 0 {
            self.bit(true).put(u64::from(v & 0xff), 8);
            v >>= 8;
        }
        self.bit(false)
    }

    fn uints(&mut self, v: &[u32]) -> &mut Self {
        self.uint(u32::try_from(v.len()).unwrap());
        for &x in v {
            self.uint(x);
        }
        self
    }

    /// A `Double` from the few the code table stores whole: 0, ±0.5, ±1, ±2.
    fn double(&mut self, v: f64) -> &mut Self {
        let negative = v < 0.0;
        match v.abs() {
            0.0 => self.put(0b01, 2),
            0.5 => self.put(0x777, 11).bit(negative),
            1.0 => self.put(0b0000, 4).bit(negative),
            // The exponent row of [2, 4) with no mantissa bytes.
            2.0 => self.put(0x1b, 5).bit(negative).bit(false),
            _ => panic!("{v} has no short code here"),
        }
    }

    fn bytes(&self) -> Vec<u8> {
        self.bits.chunks(8).map(|c| c.iter().enumerate().fold(0u8, |a, (i, &b)| a | (u8::from(b) << (7 - i)))).collect()
    }
}

/// A tessellation section of one `TESS_3D` (authoring version 8137) with explicit normals and
/// triangles of `(normal, point)` coordinate indices.
fn tess_section(points: &[[f64; 3]], normals: &[[f64; 3]], corners: &[(u32, u32)]) -> Vec<u8> {
    let mut w = W::default();
    w.uint(305).uint(0).bit(true).uint(1);
    w.uint(172).bit(false).uint(u32::try_from(points.len() * 3).unwrap());
    points.iter().flatten().for_each(|&c| _ = w.double(c));
    // Faces present; no loop flag; normals not recalculated.
    w.bit(true).bit(false).bit(false);
    w.uint(u32::try_from(normals.len() * 3).unwrap());
    normals.iter().flatten().for_each(|&c| _ = w.double(c));
    w.uint(0).uints(&corners.iter().flat_map(|&(n, p)| [n, p]).collect::<Vec<_>>());
    // One face of triangles with a normal per corner.
    w.uint(1).uint(174).uint(0).uint(0).uint(0).uint(0x2).uint(0).uints(&[u32::try_from(corners.len() / 3).unwrap()]);
    w.uint(0).bit(false);
    w.uint(0).uint(0);
    w.bytes()
}

/// A one-file-structure PRC stream whose tessellation section is `tess`, with no product tree.
fn prc_file(tess: &[u8]) -> Vec<u8> {
    const HEADER_LEN: usize = 107;
    let zlib = |data: &[u8]| {
        let mut e = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::default());
        e.write_all(data).unwrap();
        e.finish().unwrap()
    };
    let u32s = |out: &mut Vec<u8>, vs: &[u32]| vs.iter().for_each(|v| out.extend_from_slice(&v.to_le_bytes()));
    let mut globals = W::default();
    globals.uint(0);
    let sections: [&[u8]; 5] = [&globals.bytes(), b"", tess, b"", b""];
    let mut body = b"PRC".to_vec();
    u32s(&mut body, &[8137, 8137, 5, 6, 7, 8, 0, 0, 0, 0, 0]);
    let mut offs = vec![HEADER_LEN];
    for s in sections {
        offs.push(HEADER_LEN + body.len());
        body.extend(zlib(s));
    }
    let mf_start = HEADER_LEN + body.len();
    body.extend(zlib(b""));
    let mf_end = HEADER_LEN + body.len();
    let mut out = b"PRC".to_vec();
    u32s(&mut out, &[8137, 8137, 1, 2, 3, 4, 0, 0, 0, 0, 1]);
    u32s(&mut out, &[5, 6, 7, 8, 0, 6]);
    u32s(&mut out, &offs.iter().map(|&o| u32::try_from(o).unwrap()).collect::<Vec<_>>());
    u32s(&mut out, &[u32::try_from(mf_start).unwrap(), u32::try_from(mf_end).unwrap(), 0]);
    out.extend(body);
    out
}

fn pixel(image: &Image, x: u32, y: u32) -> [u8; 4] {
    let i = (y * image.width + x) as usize * 4;
    image.rgba[i..i + 4].try_into().unwrap()
}

#[test]
fn a_synthetic_prc_renders_its_nearer_triangle_over_the_farther() {
    // Listed first, a small triangle at z = 1 facing the viewer; behind it at z = -1, a larger
    // one whose stored normal lies across the view, so the headlight shades it darkest.
    let points = [[-1., -1., 1.], [1., -1., 1.], [0., 1., 1.], [-2., -2., -1.], [2., -2., -1.], [0., 2., -1.]];
    let normals = [[0., 0., 1.], [1., 0., 0.]];
    let corners = [(0, 0), (0, 3), (0, 6), (3, 9), (3, 12), (3, 15)];
    let prc = PrcFile::parse(&prc_file(&tess_section(&points, &normals, &corners))).unwrap();
    let meshes = assemble(&prc).unwrap();
    assert_eq!(meshes.len(), 1);
    assert_eq!(meshes[0].mesh.triangles.len(), 2);
    let camera = Camera::fit_meshes(&meshes, [0.0, 0.0, -1.0], [0.0, 1.0, 0.0], false, 1.0).unwrap();
    let image = render(&meshes, &camera, &RenderOptions { width: 40, height: 40, ..RenderOptions::default() }).unwrap();
    assert_eq!(pixel(&image, 1, 1), [255, 255, 255, 255], "background corner");
    assert_eq!(pixel(&image, 20, 20), [190, 192, 200, 255], "the nearer triangle, lit head on");
    // Below the near triangle only the far one: 0.3 of the surface colour.
    assert_eq!(pixel(&image, 20, 34), [57, 58, 60, 255], "the farther triangle, lit edge on");
}

#[test]
fn damaged_streams_are_errors_not_panics() {
    let points = [[0., 0., 0.], [1., 0., 0.], [0., 1., 0.]];
    let bytes = prc_file(&tess_section(&points, &[[0., 0., 1.]], &[(0, 0), (0, 3), (0, 6)]));
    for end in 0..bytes.len() {
        if let Ok(prc) = PrcFile::parse(&bytes[..end]) {
            let _ = assemble(&prc);
        }
    }
}
