//! Mesh shadings without a function (types 4–7, ISO 32000-2 8.7.4.5.5–8.7.4.5.8), converted to
//! CMYK corner by corner.
//!
//! A mesh's colour is blended between its corners in its own colour space and then shown; once
//! converted, it blends between converted corners. The two agree at the corners, and in between
//! only as far as the conversion is linear, so every triangle and patch is checked where it
//! strays most (the middles of its edges and its centre) against colour blended in the original
//! space and then converted. A piece that strays more than [`TOLERANCE`] is split in four (a
//! triangle at the middles of its edges, a patch at the middle of each parameter, by de
//! Casteljau), and its pieces checked again, up to [`MAX_SPLITS`] times.
//!
//! When nothing is split, the mesh keeps its records as they were (flags and coordinates as
//! stored) with only the colour rewritten. When something is, it is written out again piece by
//! piece: triangles as a free-form mesh (type 4), patches as tensor-product patches (type 7; a
//! Coons patch is the tensor-product patch whose inner control points ISO 32000-2 8.7.4.5.8 gives).
//!
//! Every record must end on a byte boundary. Readers that start each vertex or patch on a byte
//! and readers that don't then read it alike; a mesh whose records don't is refused.

use pdfcraft_cos::{Dict, Document, Object};

use super::TOLERANCE;

/// The most records read.
const MAX_RECORDS: usize = 1 << 20;
/// The most pieces a mesh is split into.
const MAX_PIECES: usize = 1 << 17;
/// How many times a triangle or patch is split in four.
const MAX_SPLITS: u8 = 8;

/// Why a mesh wasn't converted.
#[derive(Debug, PartialEq)]
pub(super) enum Failed {
    /// Its data can't be read: unusual bit widths, a decode array of the wrong length, a flag
    /// that continues nothing.
    Unreadable,
    /// Splitting [`MAX_SPLITS`] times, or into [`MAX_PIECES`], doesn't bring it within
    /// [`TOLERANCE`].
    Inexact,
    /// The conversion failed, and a blocker already says why.
    Reported,
}

/// One record as stored: its flag, its coordinates (x, y, x, y…) as raw values, and its colours
/// decoded.
#[derive(Clone, Debug)]
struct Record {
    flag: u32,
    coords: Vec<u64>,
    colours: Vec<Vec<f64>>,
}

/// A mesh shading's records, read.
#[derive(Debug)]
pub(super) struct Mesh {
    kind: i64,
    flag_bits: u32,
    coord_bits: u32,
    /// x, y, then each colour component.
    decode: Vec<(f64, f64)>,
    per_row: usize,
    records: Vec<Record>,
}

/// A triangle or a patch to check.
#[derive(Clone, Debug)]
enum Shape {
    Triangle([[f64; 2]; 3]),
    /// The 4 × 4 control points `p[i][j]`, `i` along u and `j` along v.
    Patch(Box<[[[f64; 2]; 4]; 4]>),
}

/// A piece with its corners' colours in the original space: a triangle's three, a patch's at
/// (u, v) = (0, 0), (0, 1), (1, 1) and (1, 0).
#[derive(Clone, Debug)]
struct Piece {
    shape: Shape,
    colours: Vec<Vec<f64>>,
}

/// Where a triangle is checked: each point's weights on its corners.
const TRIANGLE_CHECKS: [&[f64]; 4] = [&[0.5, 0.5, 0.0], &[0.0, 0.5, 0.5], &[0.5, 0.0, 0.5], &[1.0 / 3.0, 1.0 / 3.0, 1.0 / 3.0]];
/// Where a patch is checked: the middles of its edges and its centre, weighted on its corners
/// (0, 0), (0, 1), (1, 1), (1, 0) as bilinear blending weights them.
const PATCH_CHECKS: [&[f64]; 5] =
    [&[0.5, 0.0, 0.0, 0.5], &[0.5, 0.5, 0.0, 0.0], &[0.0, 0.0, 0.5, 0.5], &[0.0, 0.5, 0.5, 0.0], &[0.25, 0.25, 0.25, 0.25]];

/// The boundary of a patch in the order a record lists it, as `(i, j)` of `p[i][j]`.
const BOUNDARY: [(usize, usize); 12] = [(0, 0), (0, 1), (0, 2), (0, 3), (1, 3), (2, 3), (3, 3), (3, 2), (3, 1), (3, 0), (2, 0), (1, 0)];
/// A tensor-product patch's inner points, in the order a record lists them.
const INNER: [(usize, usize); 4] = [(1, 1), (1, 2), (2, 2), (2, 1)];

/// A patch's boundary, in the order a record lists it, and its corner colours c1–c4.
type Boundary = ([[f64; 2]; 12], [Vec<f64>; 4]);

/// The entries a converted mesh's dictionary gets (`None`: removed), and its data.
pub(super) type Converted = (Vec<(&'static [u8], Option<Object>)>, Vec<u8>);

impl Mesh {
    /// The records of a mesh shading of type `kind` with `n` colour components. `None` when its
    /// dictionary doesn't describe records it can read.
    pub(super) fn read(doc: &Document, kind: i64, dict: &Dict, data: &[u8], n: usize) -> Option<Mesh> {
        let int = |key: &[u8]| dict.get(key).map(|v| doc.resolve(v)).and_then(|v| v.as_int()).and_then(|v| u32::try_from(v).ok());
        let coord_bits = int(b"BitsPerCoordinate").filter(|b| [1, 2, 4, 8, 12, 16, 24, 32].contains(b))?;
        let comp_bits = int(b"BitsPerComponent").filter(|b| [1, 2, 4, 8, 12, 16].contains(b))?;
        let flag_bits = if kind == 5 { 0 } else { int(b"BitsPerFlag").filter(|b| [2, 4, 8].contains(b))? };
        let per_row = if kind == 5 { usize::try_from(int(b"VerticesPerRow")?).ok().filter(|v| *v >= 2)? } else { 0 };
        let decode: Vec<f64> = match &*doc.resolve(dict.get(b"Decode")?) {
            Object::Array(a) => a.iter().map(|v| doc.resolve(v).as_f64().filter(|v| v.is_finite())).collect::<Option<_>>()?,
            _ => return None,
        };
        let decode: Vec<(f64, f64)> = decode.as_chunks::<2>().0.iter().map(|&[a, b]| (a, b)).collect();
        if decode.len() != n.checked_add(2)? {
            return None;
        }
        // Points and colours in a record that starts a piece, and in one that continues the last.
        let (full, more) = match kind {
            4 | 5 => ((1, 1), (1, 1)),
            6 => ((12, 4), (8, 2)),
            7 => ((16, 4), (12, 2)),
            _ => return None,
        };
        let bits = |(points, colours): (usize, usize)| -> Option<usize> {
            (flag_bits as usize)
                .checked_add(points.checked_mul(2)?.checked_mul(coord_bits as usize)?)?
                .checked_add(colours.checked_mul(n)?.checked_mul(comp_bits as usize)?)
        };
        let (full_bits, more_bits) = (bits(full)?, bits(more)?);
        if full_bits % 8 != 0 || more_bits % 8 != 0 || full_bits == 0 {
            return None;
        }
        let colour_max = ((1u64 << comp_bits) - 1) as f64;
        let mut records = Vec::new();
        let mut pos = 0usize;
        while pos < data.len() {
            let flag = if flag_bits > 0 { Bits::new(data.get(pos..)?).read(flag_bits)? } else { 0 };
            let ((points, colours), size) = if flag == 0 || kind < 6 { (full, full_bits / 8) } else { (more, more_bits / 8) };
            let Some(bytes) = data.get(pos..pos.checked_add(size)?) else { break };
            let mut r = Bits::new(bytes);
            if flag_bits > 0 {
                r.read(flag_bits)?;
            }
            let coords = (0..points * 2).map(|_| r.read(coord_bits).map(u64::from)).collect::<Option<Vec<_>>>()?;
            let mut cs = Vec::with_capacity(colours);
            for _ in 0..colours {
                let mut c = Vec::with_capacity(n);
                for (lo, hi) in decode.get(2..)? {
                    c.push(lo + f64::from(r.read(comp_bits)?) * (hi - lo) / colour_max);
                }
                cs.push(c);
            }
            records.push(Record { flag, coords, colours: cs });
            if records.len() > MAX_RECORDS {
                return None;
            }
            pos = pos.checked_add(size)?;
        }
        Some(Mesh { kind, flag_bits, coord_bits, decode, per_row, records })
    }

    fn point(&self, coords: &[u64], i: usize) -> Option<[f64; 2]> {
        let max = ((1u64 << self.coord_bits) - 1) as f64;
        let ((x0, x1), (y0, y1)) = (*self.decode.first()?, *self.decode.get(1)?);
        let (x, y) = (*coords.get(i.checked_mul(2)?)? as f64, *coords.get(i.checked_mul(2)?.checked_add(1)?)? as f64);
        Some([x0 + x * (x1 - x0) / max, y0 + y * (y1 - y0) / max])
    }

    /// The triangles or patches the records make.
    fn pieces(&self) -> Option<Vec<Piece>> {
        let mut out = Vec::new();
        let vertex = |r: &Record| -> Option<([f64; 2], Vec<f64>)> { Some((self.point(&r.coords, 0)?, r.colours.first()?.clone())) };
        match self.kind {
            4 => {
                // The last triangle's vertices a, b, c: flag 1 shares its edge bc, flag 2 its
                // edge ac.
                let mut last: Option<[([f64; 2], Vec<f64>); 3]> = None;
                let mut i = 0;
                while let Some(r) = self.records.get(i) {
                    let tri = match r.flag {
                        0 => {
                            let (Some(b), Some(c)) = (self.records.get(i + 1), self.records.get(i + 2)) else { break };
                            i += 3;
                            [vertex(r)?, vertex(b)?, vertex(c)?]
                        }
                        1 | 2 => {
                            let [a, b, c] = last.take()?;
                            i += 1;
                            if r.flag == 1 { [b, c, vertex(r)?] } else { [a, c, vertex(r)?] }
                        }
                        _ => return None,
                    };
                    out.push(Piece { shape: Shape::Triangle([tri[0].0, tri[1].0, tri[2].0]), colours: tri.iter().map(|v| v.1.clone()).collect() });
                    last = Some(tri);
                }
            }
            5 => {
                let rows: Vec<&[Record]> = self.records.chunks_exact(self.per_row).collect();
                for pair in rows.windows(2) {
                    let [top, bottom] = pair else { continue };
                    for c in 0..self.per_row.saturating_sub(1) {
                        let v = |row: &[Record], i: usize| row.get(i).and_then(vertex);
                        let (a, b, d, e) = (v(top, c)?, v(top, c + 1)?, v(bottom, c)?, v(bottom, c + 1)?);
                        for tri in [[a.clone(), b.clone(), d.clone()], [b, e, d]] {
                            out.push(Piece {
                                shape: Shape::Triangle([tri[0].0, tri[1].0, tri[2].0]),
                                colours: tri.iter().map(|v| v.1.clone()).collect(),
                            });
                        }
                    }
                }
            }
            _ => {
                // The last patch's boundary and corner colours c1–c4.
                let mut last: Option<Boundary> = None;
                for r in &self.records {
                    let mut boundary = [[0.0; 2]; 12];
                    let mut colours: [Vec<f64>; 4] = Default::default();
                    let (from_record, first_colour) = if r.flag == 0 {
                        (0, 0)
                    } else {
                        let (b, c) = last.as_ref()?;
                        // The edge shared with the last patch: its points 4–7, 7–10 or 10–1.
                        let (start, colour) = match r.flag {
                            1 => (3, 1),
                            2 => (6, 2),
                            3 => (9, 3),
                            _ => return None,
                        };
                        for (k, slot) in boundary.iter_mut().take(4).enumerate() {
                            *slot = *b.get((start + k) % 12)?;
                        }
                        colours[0] = c.get(colour)?.clone();
                        colours[1] = c.get((colour + 1) % 4)?.clone();
                        (4, 2)
                    };
                    let mut at = 0;
                    for slot in boundary.iter_mut().skip(from_record) {
                        *slot = self.point(&r.coords, at)?;
                        at += 1;
                    }
                    let mut net = [[[0.0; 2]; 4]; 4];
                    for ((i, j), p) in BOUNDARY.iter().zip(boundary) {
                        net[*i][*j] = p;
                    }
                    if self.kind == 7 {
                        for (i, j) in INNER {
                            net[i][j] = self.point(&r.coords, at)?;
                            at += 1;
                        }
                    } else {
                        coons_inner(&mut net);
                    }
                    for (slot, c) in colours.iter_mut().skip(first_colour).zip(&r.colours) {
                        *slot = c.clone();
                    }
                    out.push(Piece { shape: Shape::Patch(Box::new(net)), colours: colours.to_vec() });
                    last = Some((boundary, colours));
                }
            }
        }
        Some(out)
    }

    /// The mesh with its colour converted by `convert` (whole colours in, 4 values each out).
    pub(super) fn into_cmyk(self, convert: &mut dyn FnMut(&[f64]) -> Option<Vec<f32>>) -> Result<Converted, Failed> {
        let pieces = self.pieces().ok_or(Failed::Unreadable)?;
        let count = pieces.len();
        let settled = settle(pieces, convert)?;
        // The records can keep their flags and coordinates if those still end on a byte.
        let aligned =
            self.records.iter().all(|r| (self.flag_bits as usize).saturating_add(r.coords.len().saturating_mul(self.coord_bits as usize)) % 8 == 0);
        if settled.len() == count && settled.iter().all(|(_, split)| !split) && aligned {
            return self.recoloured(convert);
        }
        let pieces: Vec<Piece> = settled.into_iter().map(|(p, _)| p).collect();
        written(self.kind, &pieces, convert)
    }

    /// The records as they are, with their colours converted and written at 16 bits.
    fn recoloured(&self, convert: &mut dyn FnMut(&[f64]) -> Option<Vec<f32>>) -> Result<Converted, Failed> {
        let values: Vec<f64> = self.records.iter().flat_map(|r| r.colours.iter().flatten().copied()).collect();
        let cmyk = convert(&values).ok_or(Failed::Reported)?;
        let mut w = BitWriter::default();
        let mut colour = cmyk.as_chunks::<4>().0.iter();
        for r in &self.records {
            if self.flag_bits > 0 {
                w.write(u64::from(r.flag), self.flag_bits);
            }
            for c in &r.coords {
                w.write(*c, self.coord_bits);
            }
            for _ in &r.colours {
                for v in colour.next().ok_or(Failed::Reported)? {
                    w.write(u64::from(unit16(f64::from(*v))), 16);
                }
            }
        }
        let mut decode: Vec<Object> = self.decode.iter().take(2).flat_map(|(a, b)| [Object::Real(*a), Object::Real(*b)]).collect();
        decode.extend((0..4).flat_map(|_| [Object::Int(0), Object::Int(1)]));
        Ok((vec![(b"BitsPerComponent", Some(Object::Int(16))), (b"Decode", Some(Object::Array(decode)))], w.finish()))
    }
}

/// A Coons patch's inner control points, from its boundary (ISO 32000-2 8.7.4.5.8): each is a
/// weighting of eight boundary points `(i, j, weight)`, over 9.
fn coons_inner(p: &mut Net) {
    let q = *p;
    let inner = |w: [(usize, usize, f64); 8]| -> [f64; 2] { [0, 1].map(|k| w.iter().map(|(i, j, c)| c * q[*i][*j][k]).sum::<f64>() / 9.0) };
    p[1][1] = inner([(0, 0, -4.0), (0, 1, 6.0), (1, 0, 6.0), (0, 3, -2.0), (3, 0, -2.0), (3, 1, 3.0), (1, 3, 3.0), (3, 3, -1.0)]);
    p[1][2] = inner([(0, 3, -4.0), (0, 2, 6.0), (1, 3, 6.0), (0, 0, -2.0), (3, 3, -2.0), (3, 2, 3.0), (1, 0, 3.0), (3, 0, -1.0)]);
    p[2][1] = inner([(3, 0, -4.0), (3, 1, 6.0), (2, 0, 6.0), (3, 3, -2.0), (0, 0, -2.0), (0, 1, 3.0), (2, 3, 3.0), (0, 3, -1.0)]);
    p[2][2] = inner([(3, 3, -4.0), (3, 2, 6.0), (2, 3, 6.0), (3, 0, -2.0), (0, 3, -2.0), (0, 2, 3.0), (2, 0, 3.0), (0, 0, -1.0)]);
}

/// `weights` over `colours`, component by component.
fn blend(colours: &[Vec<f64>], weights: &[f64]) -> Vec<f64> {
    let n = colours.first().map_or(0, Vec::len);
    (0..n).map(|k| colours.iter().zip(weights).map(|(c, w)| c.get(k).copied().unwrap_or(0.0) * w).sum()).collect()
}

impl Piece {
    fn checks(&self) -> &'static [&'static [f64]] {
        match self.shape {
            Shape::Triangle(_) => &TRIANGLE_CHECKS,
            Shape::Patch(_) => &PATCH_CHECKS,
        }
    }

    /// The piece in four, in the order they paint.
    fn split(&self) -> Vec<Piece> {
        match &self.shape {
            Shape::Triangle([a, b, c]) => {
                let mid = |p: &[f64; 2], q: &[f64; 2]| [(p[0] + q[0]) / 2.0, (p[1] + q[1]) / 2.0];
                let (ab, bc, ca) = (mid(a, b), mid(b, c), mid(c, a));
                let col = |w: &[f64]| blend(&self.colours, w);
                let (ca_c, cb_c, cc_c) = (col(&[1.0, 0.0, 0.0]), col(&[0.0, 1.0, 0.0]), col(&[0.0, 0.0, 1.0]));
                let (cab, cbc, cca) = (col(&[0.5, 0.5, 0.0]), col(&[0.0, 0.5, 0.5]), col(&[0.5, 0.0, 0.5]));
                vec![
                    Piece { shape: Shape::Triangle([*a, ab, ca]), colours: vec![ca_c, cab.clone(), cca.clone()] },
                    Piece { shape: Shape::Triangle([ab, *b, bc]), colours: vec![cab.clone(), cb_c, cbc.clone()] },
                    Piece { shape: Shape::Triangle([ca, bc, *c]), colours: vec![cca.clone(), cbc.clone(), cc_c] },
                    Piece { shape: Shape::Triangle([ab, bc, ca]), colours: vec![cab, cbc, cca] },
                ]
            }
            Shape::Patch(net) => {
                // Along u (i), then each half along v (j).
                let (left, right) = split_u(net);
                let colour_at = |u: f64, v: f64| blend(&self.colours, &[(1.0 - u) * (1.0 - v), (1.0 - u) * v, u * v, u * (1.0 - v)]);
                let mut out = Vec::with_capacity(4);
                let halves = [(split_v(&left), 0.0), (split_v(&right), 0.5)];
                for (b, v0) in [(0usize, 0.0), (1, 0.5)] {
                    for ((lower, upper), u0) in &halves {
                        let net = if b == 0 { *lower } else { *upper };
                        let (u1, v1) = (u0 + 0.5, v0 + 0.5);
                        out.push(Piece {
                            shape: Shape::Patch(Box::new(net)),
                            colours: vec![colour_at(*u0, v0), colour_at(*u0, v1), colour_at(u1, v1), colour_at(u1, v0)],
                        });
                    }
                }
                out
            }
        }
    }

    /// Every control point, for the bounds of what's written.
    fn points(&self) -> Vec<[f64; 2]> {
        match &self.shape {
            Shape::Triangle(t) => t.to_vec(),
            Shape::Patch(net) => net.iter().flatten().copied().collect(),
        }
    }
}

/// A cubic Bézier `(p0, p1, p2, p3)` split at its middle.
fn halve(p: [[f64; 2]; 4]) -> ([[f64; 2]; 4], [[f64; 2]; 4]) {
    let mid = |a: [f64; 2], b: [f64; 2]| [(a[0] + b[0]) / 2.0, (a[1] + b[1]) / 2.0];
    let (a, b, c) = (mid(p[0], p[1]), mid(p[1], p[2]), mid(p[2], p[3]));
    let (d, e) = (mid(a, b), mid(b, c));
    let m = mid(d, e);
    ([p[0], a, d, m], [m, e, c, p[3]])
}

type Net = [[[f64; 2]; 4]; 4];

fn split_u(net: &Net) -> (Net, Net) {
    let (mut l, mut r) = (*net, *net);
    for j in 0..4 {
        let (a, b) = halve([net[0][j], net[1][j], net[2][j], net[3][j]]);
        for i in 0..4 {
            l[i][j] = a[i];
            r[i][j] = b[i];
        }
    }
    (l, r)
}

fn split_v(net: &Net) -> (Net, Net) {
    let (mut lower, mut upper) = (*net, *net);
    for i in 0..4 {
        let (a, b) = halve(net[i]);
        lower[i] = a;
        upper[i] = b;
    }
    (lower, upper)
}

/// Check `pieces` and split those that stray, keeping the order they paint in. Each comes back
/// with whether it was split from a larger one.
fn settle(pieces: Vec<Piece>, convert: &mut dyn FnMut(&[f64]) -> Option<Vec<f32>>) -> Result<Vec<(Piece, bool)>, Failed> {
    let mut slots: Vec<(Piece, bool, bool)> = pieces.into_iter().map(|p| (p, false, false)).collect();
    for level in 0..=MAX_SPLITS {
        if slots.iter().all(|(_, _, done)| *done) {
            break;
        }
        // The corners and the checked points of every piece still to check, in one conversion.
        let mut values = Vec::new();
        for (p, _, done) in &slots {
            if *done {
                continue;
            }
            values.extend(p.colours.iter().flatten().copied());
            for w in p.checks() {
                values.extend(blend(&p.colours, w));
            }
        }
        let cmyk = convert(&values).ok_or(Failed::Reported)?;
        let mut colour = cmyk.as_chunks::<4>().0.iter();
        let mut next = Vec::with_capacity(slots.len());
        for (p, split, done) in slots {
            if done {
                next.push((p, split, done));
                continue;
            }
            let corners: Vec<&[f32; 4]> = (0..p.colours.len()).map(|_| colour.next().ok_or(Failed::Reported)).collect::<Result<_, _>>()?;
            let mut fine = true;
            for w in p.checks() {
                let exact = colour.next().ok_or(Failed::Reported)?;
                for (k, e) in exact.iter().enumerate() {
                    let blended: f64 = corners.iter().zip(*w).map(|(c, w)| f64::from(c.get(k).copied().unwrap_or(0.0)) * w).sum();
                    fine &= (blended - f64::from(*e)).abs() <= TOLERANCE;
                }
            }
            if fine {
                next.push((p, split, true));
            } else if level == MAX_SPLITS {
                return Err(Failed::Inexact);
            } else {
                next.extend(p.split().into_iter().map(|c| (c, true, false)));
            }
        }
        if next.len() > MAX_PIECES {
            return Err(Failed::Inexact);
        }
        slots = next;
    }
    Ok(slots.into_iter().map(|(p, split, _)| (p, split)).collect())
}

/// `pieces` written out again: triangles as a free-form mesh, patches as tensor-product patches,
/// with 8-bit flags (each piece starts afresh), 32-bit coordinates over the pieces' bounds and
/// 16-bit CMYK.
fn written(kind: i64, pieces: &[Piece], convert: &mut dyn FnMut(&[f64]) -> Option<Vec<f32>>) -> Result<Converted, Failed> {
    let mut lo = [f64::INFINITY; 2];
    let mut hi = [f64::NEG_INFINITY; 2];
    for p in pieces.iter().flat_map(Piece::points) {
        for k in 0..2 {
            lo[k] = lo[k].min(p[k]);
            hi[k] = hi[k].max(p[k]);
        }
    }
    for k in 0..2 {
        if !lo[k].is_finite() || !hi[k].is_finite() {
            (lo[k], hi[k]) = (0.0, 1.0);
        } else if hi[k] <= lo[k] {
            hi[k] = lo[k] + 1.0;
        }
    }
    let values: Vec<f64> = pieces.iter().flat_map(|p| p.colours.iter().flatten().copied()).collect();
    let cmyk = convert(&values).ok_or(Failed::Reported)?;
    let mut colour = cmyk.as_chunks::<4>().0.iter();
    let mut w = BitWriter::default();
    let coord = |w: &mut BitWriter, p: [f64; 2]| {
        for k in 0..2 {
            let t = ((p[k] - lo[k]) / (hi[k] - lo[k])).clamp(0.0, 1.0);
            w.write((t * f64::from(u32::MAX)).round() as u64, 32);
        }
    };
    let mut ink = |w: &mut BitWriter| -> Result<(), Failed> {
        for v in colour.next().ok_or(Failed::Reported)? {
            w.write(u64::from(unit16(f64::from(*v))), 16);
        }
        Ok(())
    };
    for p in pieces {
        match &p.shape {
            Shape::Triangle(t) => {
                for v in t {
                    w.write(0, 8);
                    coord(&mut w, *v);
                    ink(&mut w)?;
                }
            }
            Shape::Patch(net) => {
                w.write(0, 8);
                for (i, j) in BOUNDARY.iter().chain(&INNER) {
                    coord(&mut w, net[*i][*j]);
                }
                for _ in 0..4 {
                    ink(&mut w)?;
                }
            }
        }
    }
    let mut decode = vec![Object::Real(lo[0]), Object::Real(hi[0]), Object::Real(lo[1]), Object::Real(hi[1])];
    decode.extend((0..4).flat_map(|_| [Object::Int(0), Object::Int(1)]));
    let entries: Vec<(&'static [u8], Option<Object>)> = vec![
        (b"ShadingType", Some(Object::Int(if matches!(kind, 4 | 5) { 4 } else { 7 }))),
        (b"BitsPerCoordinate", Some(Object::Int(32))),
        (b"BitsPerComponent", Some(Object::Int(16))),
        (b"BitsPerFlag", Some(Object::Int(8))),
        (b"Decode", Some(Object::Array(decode))),
        (b"VerticesPerRow", None),
    ];
    Ok((entries, w.finish()))
}

#[derive(Default)]
struct BitWriter {
    out: Vec<u8>,
    acc: u64,
    bits: u32,
}

impl BitWriter {
    /// The low `bits` bits of `v` (at most 32), most significant first.
    fn write(&mut self, v: u64, bits: u32) {
        for b in (0..bits.min(32)).rev() {
            self.acc = (self.acc << 1) | ((v >> b) & 1);
            self.bits += 1;
            if self.bits == 8 {
                self.out.push(self.acc as u8);
                self.acc = 0;
                self.bits = 0;
            }
        }
    }

    fn finish(mut self) -> Vec<u8> {
        if self.bits > 0 {
            self.out.push((self.acc << (8 - self.bits)) as u8);
        }
        self.out
    }
}

/// Reads values of up to 32 bits, most significant first.
struct Bits<'a> {
    data: &'a [u8],
    bit: usize,
}

impl<'a> Bits<'a> {
    fn new(data: &'a [u8]) -> Self {
        Bits { data, bit: 0 }
    }

    fn read(&mut self, bits: u32) -> Option<u32> {
        let mut v = 0u64;
        for _ in 0..bits.min(32) {
            let byte = *self.data.get(self.bit / 8)?;
            v = (v << 1) | u64::from((byte >> (7 - self.bit % 8)) & 1);
            self.bit += 1;
        }
        u32::try_from(v).ok()
    }
}

/// A colour component 0–1 at 16 bits.
fn unit16(v: f64) -> u16 {
    if v.is_finite() { (v.clamp(0.0, 1.0) * 65535.0).round() as u16 } else { 0 }
}

#[cfg(test)]
mod tests;
