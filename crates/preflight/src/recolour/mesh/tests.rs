use std::sync::Arc;

use pdfcraft_cos::{Dict, Document, Object};
use pdfcraft_testkit::Pdf;

use super::*;

fn doc() -> Document {
    Document::open(Arc::new(Pdf::new().obj("<< /Type /Catalog >>").build())).unwrap()
}

fn dict(entries: &[(&str, Object)]) -> Dict {
    let mut d = Dict::new();
    for (k, v) in entries {
        d.set(k.as_bytes().to_vec(), v.clone());
    }
    d
}

fn numbers(values: &[f64]) -> Object {
    Object::Array(values.iter().map(|v| Object::Real(*v)).collect())
}

/// A point on the tensor-product surface `net` at (u, v).
fn surface(net: &Net, u: f64, v: f64) -> [f64; 2] {
    let b = |t: f64| [(1.0 - t).powi(3), 3.0 * t * (1.0 - t).powi(2), 3.0 * t * t * (1.0 - t), t.powi(3)];
    let (bu, bv) = (b(u), b(v));
    let mut p = [0.0; 2];
    for i in 0..4 {
        for j in 0..4 {
            for k in 0..2 {
                p[k] += net[i][j][k] * bu[i] * bv[j];
            }
        }
    }
    p
}

/// A free-form triangle mesh in RGB: 8-bit flags, coordinates and components, coordinates 0–255.
fn triangles(vertices: &[(u32, [u32; 2], [u32; 3])]) -> Mesh {
    let mut w = BitWriter::default();
    for (flag, p, c) in vertices {
        w.write(u64::from(*flag), 8);
        for v in p.iter().chain(c) {
            w.write(u64::from(*v), 8);
        }
    }
    let d = dict(&[
        ("BitsPerCoordinate", Object::Int(8)),
        ("BitsPerComponent", Object::Int(8)),
        ("BitsPerFlag", Object::Int(8)),
        ("Decode", numbers(&[0.0, 255.0, 0.0, 255.0, 0.0, 1.0, 0.0, 1.0, 0.0, 1.0])),
    ]);
    Mesh::read(&doc(), 4, &d, &w.finish(), 3).unwrap()
}

#[test]
fn bits_written_read_back() {
    let mut w = BitWriter::default();
    for (v, bits) in [(3u64, 2u32), (0x1FF, 9), (0, 5), (0xDEAD_BEEF, 32), (1, 1)] {
        w.write(v, bits);
    }
    let data = w.finish();
    assert_eq!(data.len(), 7, "49 bits take 7 bytes");
    let mut r = Bits::new(&data);
    for (v, bits) in [(3u32, 2u32), (0x1FF, 9), (0, 5), (0xDEAD_BEEF, 32), (1, 1)] {
        assert_eq!(r.read(bits), Some(v));
    }
    assert_eq!(Bits::new(&[0xFF]).read(9), None, "reading past the end has no value");
}

/// A point on the cubic Bézier `p` at `t`.
fn bezier(p: [[f64; 2]; 4], t: f64) -> [f64; 2] {
    let b = [(1.0 - t).powi(3), 3.0 * t * (1.0 - t).powi(2), 3.0 * t * t * (1.0 - t), t.powi(3)];
    [0, 1].map(|k| (0..4).map(|i| p[i][k] * b[i]).sum())
}

#[test]
fn a_coons_patch_is_the_tensor_patch_with_its_inner_points() {
    // A curved boundary away from the origin, so every weight on every boundary point counts.
    let mut net = [[[0.0; 2]; 4]; 4];
    for (n, (i, j)) in BOUNDARY.into_iter().enumerate() {
        let wobble = (n as f64 * 1.7).sin() * 3.0;
        net[i][j] = [20.0 + i as f64 * 10.0 + wobble, 40.0 + j as f64 * 12.0 - wobble * 0.5 + (i * j) as f64];
    }
    coons_inner(&mut net);
    // The Coons surface: the boundary curves blended in each direction, less the corners' blend.
    let (c1, c2) = ([net[0][0], net[1][0], net[2][0], net[3][0]], [net[0][3], net[1][3], net[2][3], net[3][3]]);
    let (d1, d2) = ([net[0][0], net[0][1], net[0][2], net[0][3]], [net[3][0], net[3][1], net[3][2], net[3][3]]);
    for (u, v) in [(0.25, 0.5), (0.5, 0.5), (0.1, 0.9), (0.7, 0.3), (0.9, 0.05)] {
        let (a, b, c, d) = (bezier(c1, u), bezier(c2, u), bezier(d1, v), bezier(d2, v));
        let want: [f64; 2] = [0, 1].map(|k| {
            (1.0 - v) * a[k] + v * b[k] + (1.0 - u) * c[k] + u * d[k]
                - ((1.0 - u) * (1.0 - v) * net[0][0][k] + u * (1.0 - v) * net[3][0][k] + (1.0 - u) * v * net[0][3][k] + u * v * net[3][3][k])
        });
        let got = surface(&net, u, v);
        assert!((got[0] - want[0]).abs() < 1e-9 && (got[1] - want[1]).abs() < 1e-9, "({u}, {v}): {got:?} against {want:?}");
    }
}

#[test]
fn split_patches_draw_the_same_surface() {
    let mut net = [[[0.0; 2]; 4]; 4];
    for (i, row) in net.iter_mut().enumerate() {
        for (j, p) in row.iter_mut().enumerate() {
            *p = [(i as f64).powi(2) + j as f64, (j as f64 * 1.3).sin() * 4.0 + i as f64];
        }
    }
    let piece = Piece { shape: Shape::Patch(Box::new(net)), colours: vec![vec![0.0], vec![1.0], vec![0.5], vec![0.25]] };
    let parts = piece.split();
    assert_eq!(parts.len(), 4);
    // Parts in paint order: (u, v) halves (0, 0), (1, 0), (0, 1), (1, 1).
    for (part, (u0, v0)) in parts.iter().zip([(0.0, 0.0), (0.5, 0.0), (0.0, 0.5), (0.5, 0.5)]) {
        let Shape::Patch(sub) = &part.shape else { panic!() };
        for (s, t) in [(0.0, 0.0), (0.3, 0.7), (1.0, 0.5), (1.0, 1.0)] {
            let (a, b) = (surface(sub, s, t), surface(&net, u0 + s / 2.0, v0 + t / 2.0));
            assert!((a[0] - b[0]).abs() < 1e-9 && (a[1] - b[1]).abs() < 1e-9, "{a:?} {b:?}");
        }
        // Corner colours are the parent's blended at the part's corners: c00 is (u0, v0).
        let want = (1.0 - u0) * (1.0 - v0) * 0.0 + (1.0 - u0) * v0 * 1.0 + u0 * v0 * 0.5 + u0 * (1.0 - v0) * 0.25;
        assert!((part.colours[0][0] - want).abs() < 1e-12);
    }
}

#[test]
fn free_form_flags_share_the_last_triangles_edges() {
    let m = triangles(&[(0, [0, 0], [0; 3]), (0, [10, 0], [0; 3]), (0, [0, 10], [0; 3]), (1, [10, 10], [0; 3]), (2, [20, 20], [0; 3])]);
    let shapes: Vec<[[f64; 2]; 3]> = m
        .pieces()
        .unwrap()
        .into_iter()
        .map(|p| match p.shape {
            Shape::Triangle(t) => t,
            Shape::Patch(_) => panic!(),
        })
        .collect();
    assert_eq!(
        shapes,
        vec![
            [[0.0, 0.0], [10.0, 0.0], [0.0, 10.0]],
            // Flag 1: the edge from b to c.
            [[10.0, 0.0], [0.0, 10.0], [10.0, 10.0]],
            // Flag 2: the edge from a to c of the last triangle.
            [[10.0, 0.0], [10.0, 10.0], [20.0, 20.0]],
        ]
    );
    // A flag that continues no triangle.
    let orphan = triangles(&[(1, [0, 0], [0; 3])]);
    assert!(orphan.pieces().is_none());
}

#[test]
fn records_that_dont_end_on_a_byte_are_refused() {
    let d = dict(&[
        ("BitsPerCoordinate", Object::Int(8)),
        ("BitsPerComponent", Object::Int(8)),
        ("BitsPerFlag", Object::Int(2)),
        ("Decode", numbers(&[0.0, 1.0, 0.0, 1.0, 0.0, 1.0, 0.0, 1.0, 0.0, 1.0])),
    ]);
    assert!(Mesh::read(&doc(), 4, &d, &[0; 16], 3).is_none());
    let short = dict(&[
        ("BitsPerCoordinate", Object::Int(8)),
        ("BitsPerComponent", Object::Int(8)),
        ("BitsPerFlag", Object::Int(8)),
        ("Decode", numbers(&[0.0, 1.0, 0.0, 1.0, 0.0, 1.0])),
    ]);
    assert!(Mesh::read(&doc(), 4, &short, &[0; 16], 3).is_none(), "a decode array for one component, read for three");
}

/// A triangle from black to full red, blue and green at its corners.
fn primaries() -> Mesh {
    triangles(&[(0, [0, 0], [255, 0, 0]), (0, [100, 0], [0, 255, 0]), (0, [0, 100], [0, 0, 255])])
}

#[test]
fn a_linear_conversion_keeps_the_records() {
    let mut linear = |v: &[f64]| Some(v.chunks(3).flat_map(|c| [1.0 - c[0] as f32, 1.0 - c[1] as f32, 1.0 - c[2] as f32, 0.0]).collect());
    let (entries, data) = primaries().into_cmyk(&mut linear).unwrap();
    assert!(entries.iter().all(|(k, _)| *k != b"ShadingType".as_slice()), "{entries:?}");
    // Flag, x, y at 8 bits, then 4 × 16 bits of colour, for each vertex.
    assert_eq!(data.len(), 3 * (3 + 8));
    assert_eq!(&data[..3], &[0, 0, 0]);
    assert_eq!(&data[3..11], &[0, 0, 0xFF, 0xFF, 0xFF, 0xFF, 0, 0]);
}

#[test]
fn a_curved_conversion_splits_triangles_until_they_blend_close_enough() {
    let mut curved = |v: &[f64]| Some(v.chunks(3).flat_map(|c| [(c[0] * c[0]) as f32, (c[1] * c[1]) as f32, (c[2] * c[2]) as f32, 0.0]).collect());
    let (entries, data) = primaries().into_cmyk(&mut curved).unwrap();
    assert!(entries.contains(&(b"ShadingType".as_slice(), Some(Object::Int(4)))));
    // Each vertex: an 8-bit flag, two 32-bit coordinates and four 16-bit inks.
    let vertices = data.len() / 17;
    assert_eq!(data.len() % 17, 0);
    assert!(vertices / 3 >= 4u32.pow(3) as usize, "{} triangles", vertices / 3);
    // A step can't be blended to, however finely it is split.
    let mut step = |v: &[f64]| Some(v.chunks(3).flat_map(|c| [if c[0] > 0.5 { 1.0 } else { 0.0 }, 0.0, 0.0, 0.0]).collect());
    assert_eq!(primaries().into_cmyk(&mut step).unwrap_err(), Failed::Inexact);
}
