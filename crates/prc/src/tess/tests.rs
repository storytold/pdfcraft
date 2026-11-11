#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use super::triangulate::MAX_TEXTURE_SETS;
use super::*;
use crate::Schema;
use crate::testw::W;

const SQUARE: [f64; 12] = [0., 0., 0., 1., 0., 0., 1., 1., 0., 0., 1., 0.];
const ATTRIBUTE: u32 = 201;
const NORMAL_SINGLE: u32 = 0x4000_0000;

/// A 305 section holding `body` (already-written entities, `n` of them).
fn section(n: u32, body: &W) -> W {
    let mut w = W::default();
    w.uint(305).uint(0).bit(true).uint(n).append(body).uint(0);
    w
}

fn uints(w: &mut W, v: &[u32]) {
    w.uint(u32::try_from(v.len()).unwrap());
    for &x in v {
        w.uint(x);
    }
}

fn square(w: &mut W, entity: u32) {
    coordinates(w, entity, &SQUARE);
}

fn coordinates(w: &mut W, entity: u32, coords: &[f64]) {
    w.uint(entity).bit(false).uint(u32::try_from(coords.len()).unwrap());
    for &c in coords {
        w.double(c);
    }
}

#[derive(Default)]
struct Mesh<'a> {
    recalc: bool,
    indices: &'a [u32],
    faces: &'a [(u32, u32, &'a [u32])],
    textures: u32,
    uvs: &'a [f64],
    /// The coordinates; [`SQUARE`] when `None`.
    coords: Option<&'a [f64]>,
}

/// A TESS_3D over [`SQUARE`] at authoring version `v`.
fn tess_3d(w: &mut W, v: u32, m: &Mesh<'_>) {
    coordinates(w, 172, m.coords.unwrap_or(&SQUARE));
    w.bit(true);
    if v >= 7039 {
        w.bit(false);
    }
    if v >= 7047 {
        w.bit(m.recalc);
        if m.recalc {
            w.put(0, 8).double(0.5);
        }
    }
    if m.recalc {
        w.uint(0);
    } else {
        // Two normals: slot 0 is +z, slot 3 is -z.
        w.uint(6);
        for c in [0., 0., 1., 0., 0., -1.] {
            w.double(c);
        }
    }
    w.uint(0);
    uints(w, m.indices);
    w.uint(u32::try_from(m.faces.len()).unwrap());
    for &(flags, start, data) in m.faces {
        w.uint(174).uint(0).uint(0).uint(0).uint(flags).uint(start);
        uints(w, data);
        w.uint(m.textures).bit(false);
    }
    w.uint(u32::try_from(m.uvs.len()).unwrap());
    for &c in m.uvs {
        w.double(c);
    }
}

fn decode(w: &W, schema: &Schema, version: u32) -> Result<Vec<Tessellation>, PrcError> {
    let bytes = w.bytes();
    Ctx::new(BitReader::new(&bytes), schema, version).file_structure_tessellation()
}

fn mesh(t: &Tessellation) -> &TriangleMesh {
    match t {
        Tessellation::Mesh(m) => m,
        other => panic!("not a mesh: {other:?}"),
    }
}

fn one(indices: &[u32], faces: &[(u32, u32, &[u32])], textures: u32) -> TriangleMesh {
    let mut b = W::default();
    tess_3d(&mut b, 8137, &Mesh { indices, faces, textures, ..Mesh::default() });
    mesh(&decode(&section(1, &b), &Schema::default(), 8137).unwrap()[0]).clone()
}

#[test]
fn triangle_and_fan_with_normals() {
    // One triangle (n,p x3), then a fan of 4 vertices (n,p each).
    let m = one(&[3, 0, 0, 3, 3, 6, 0, 0, 3, 3, 0, 6, 3, 9], &[(0x2 | 0x4, 0, &[1, 1, 4])], 0);
    assert_eq!(m.positions.len(), 4);
    assert_eq!(m.positions[2], [1., 1., 0.]);
    assert_eq!(m.triangles, [[0, 1, 2], [0, 1, 2], [0, 2, 3]]);
    assert_eq!(m.faces, vec![0..3]);
    assert!(!m.normals_recalculated);
    assert_eq!(m.normals, [[0., 0., 1.], [0., 0., -1.]]);
    // Each corner keeps the normal stored beside its point.
    assert_eq!(m.triangle_normals, [[1, 0, 1], [0, 1, 0], [0, 0, 1]]);
}

#[test]
fn strip_normals_follow_the_alternating_winding() {
    let m = one(&[3, 0, 0, 3, 3, 6, 0, 9], &[(0x8, 0, &[1, 4])], 0);
    assert_eq!(m.triangles, [[0, 1, 2], [2, 1, 3]]);
    assert_eq!(m.triangle_normals, [[1, 0, 1], [1, 0, 0]]);
}

#[test]
fn a_normal_index_past_the_array_drops_every_normal_but_no_triangle() {
    let m = one(&[0, 0, 6, 3, 0, 6], &[(0x2, 0, &[1])], 0);
    assert_eq!(m.triangles, [[0, 1, 2]]);
    assert!(m.normals.is_empty());
    assert!(m.triangle_normals.is_empty());
}

#[test]
fn recalculated_normals_drop_the_normal_slots_and_strips_alternate() {
    let mut b = W::default();
    tess_3d(&mut b, 8137, &Mesh { recalc: true, indices: &[0, 3, 6, 9], faces: &[(0x8, 0, &[1, 4])], ..Mesh::default() });
    let t = decode(&section(1, &b), &Schema::default(), 8137).unwrap();
    let m = mesh(&t[0]);
    assert!(m.normals_recalculated);
    assert_eq!(m.triangles, [[0, 1, 2], [2, 1, 3]]);
    assert!(m.normals.is_empty() && m.triangle_normals.is_empty());
}

#[test]
fn version_gates_the_loop_and_recalculation_flags() {
    for v in [7038, 7039, 7046, 7047, 8137] {
        let mut b = W::default();
        tess_3d(&mut b, v, &Mesh { indices: &[0, 0, 0, 3, 0, 6], faces: &[(0x2, 0, &[1])], ..Mesh::default() });
        // A trailing wire: a misread flag desyncs it.
        wire(&mut b, false);
        let t = decode(&section(2, &b), &Schema::default(), v).unwrap();
        assert_eq!(mesh(&t[0]).triangles, [[0, 1, 2]], "version {v}");
        assert!(matches!(t[1], Tessellation::Wire(_)), "version {v}");
    }
}

#[test]
fn one_normal_blocks_and_textures() {
    // 0x20: n,p,p,p. 0x40 with NORMAL_Single: n then p per vertex.
    // 0x2000 with one texture set: n,t,p,t,p,t,p.
    let idx = [
        0, 0, 3, 6, // 0x20
        3, 0, 6, 9, // 0x40 single, 3 vertices
        0, 0, 9, 2, 0, 4, 3, // 0x2000
    ];
    let m = one(&idx, &[(0x20 | 0x40 | 0x2000, 0, &[1, 1, 3 | NORMAL_SINGLE, 1])], 1);
    assert_eq!(m.triangles, [[0, 1, 2], [0, 2, 3], [3, 0, 1]]);
    // A one-normal entity gives every corner its single normal.
    assert_eq!(m.triangle_normals, [[0, 0, 0], [1, 1, 1], [0, 0, 0]]);
}

#[test]
fn one_normal_fan_without_single_keeps_a_normal_per_vertex() {
    let m = one(&[0, 0, 0, 3, 0, 6, 0, 9], &[(0x40, 0, &[1, 4])], 0);
    assert_eq!(m.triangles, [[0, 1, 2], [0, 2, 3]]);
}

#[test]
fn textured_fan_and_strip_carry_texture_slots_per_vertex() {
    // 0x400 then 0x800, two texture sets: n,t,t,p per vertex.
    let mut idx = Vec::new();
    for p in [0, 3, 6, 0, 3, 6, 9] {
        idx.extend([0, 0, 2, p]);
    }
    let m = one(&idx, &[(0x400 | 0x800, 0, &[1, 3, 1, 4])], 2);
    assert_eq!(m.triangles, [[0, 1, 2], [0, 1, 2], [2, 1, 3]]);
}

/// The unit square as two textured triangles (one set; per vertex a normal, a texture slot,
/// a point), its corners at the picture's corners.
fn textured_square() -> W {
    let mut body = W::default();
    let indices = [0, 0, 0, 0, 2, 3, 0, 4, 6, 0, 0, 0, 0, 4, 6, 0, 6, 9];
    tess_3d(
        &mut body,
        8137,
        &Mesh { indices: &indices, faces: &[(0x200, 0, &[2])], textures: 1, uvs: &[0., 0., 1., 0., 1., 1., 0., 1.], ..Mesh::default() },
    );
    section(1, &body)
}

#[test]
fn a_textured_mesh_keeps_each_corners_coordinates() {
    let t = decode(&textured_square(), &Schema::default(), 8137).unwrap();
    let m = mesh(&t[0]);
    assert_eq!(m.triangles, [[0, 1, 2], [0, 2, 3]]);
    assert_eq!(m.uvs, [[0.0, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 1.0]]);
    assert_eq!(m.triangle_uvs, [vec![Some([0, 1, 2]), Some([0, 2, 3])]]);
    // An odd slot names no pair: that triangle has no coordinates.
    let mut b = W::default();
    tess_3d(
        &mut b,
        8137,
        &Mesh { indices: &[0, 1, 0, 0, 2, 3, 0, 4, 6], faces: &[(0x200, 0, &[1])], textures: 1, uvs: &[0.0; 6], ..Mesh::default() },
    );
    let t = decode(&section(1, &b), &Schema::default(), 8137).unwrap();
    let m = mesh(&t[0]);
    assert_eq!(m.triangles, [[0, 1, 2]]);
    assert_eq!(m.triangle_uvs, [vec![None]]);
}

#[test]
fn too_many_texture_sets_are_refused_before_anything_is_allocated() {
    // An empty fan on a face claiming 2^32 - 1 texture sets: no slot bounds the count.
    let mut b = W::default();
    tess_3d(&mut b, 8137, &Mesh { indices: &[0], faces: &[(0x400, 0, &[1, 0])], textures: u32::MAX, ..Mesh::default() });
    assert!(
        matches!(decode(&section(1, &b), &Schema::default(), 8137), Err(PrcError::Malformed(m)) if m == format!("{} texture-coordinate sets on a face", u32::MAX))
    );
    let sets = u32::try_from(MAX_TEXTURE_SETS).unwrap();
    let mut ok = W::default();
    tess_3d(&mut ok, 8137, &Mesh { indices: &[0], faces: &[(0x400, 0, &[1, 0])], textures: sets, ..Mesh::default() });
    assert!(decode(&section(1, &ok), &Schema::default(), 8137).is_ok());
}

/// A tessellation section holding one TESS_3D: [`SQUARE`] as two faces of one triangle each.
pub(crate) fn two_face_square() -> W {
    let mut b = W::default();
    tess_3d(&mut b, 8137, &Mesh { indices: &[0, 0, 0, 3, 0, 6, 0, 0, 0, 6, 0, 9], faces: &[(0x2, 0, &[1]), (0x2, 6, &[1])], ..Mesh::default() });
    section(1, &b)
}

#[test]
fn two_faces_partition_the_triangles() {
    let m = one(&[0, 0, 0, 3, 0, 6, 0, 0, 0, 6, 0, 9], &[(0x2, 0, &[1]), (0x2, 6, &[1])], 0);
    assert_eq!(m.triangles, [[0, 1, 2], [0, 2, 3]]);
    assert_eq!(m.faces, [0..1, 1..2]);
}

/// A wire over [`SQUARE`]: an open two-point wire continued by a closing two-point wire,
/// then optionally per-segment RGB colours.
fn wire(w: &mut W, colours: bool) {
    square(w, 175);
    uints(w, &[2, 0, 3, WIRE_IS_CONTINUOUS | WIRE_IS_CLOSING | 2, 6, 9]);
    w.bit(colours);
    if colours {
        // 5 points (4 + the implicit closing one) -> 2 segment colours.
        w.bit(false).bit(true).bit(false).put(0xff0000, 24);
        w.bit(true);
    }
}

#[test]
fn wires_close_and_continue_and_colours_are_counted() {
    for colours in [false, true] {
        let mut b = W::default();
        wire(&mut b, colours);
        // A second entity: a wrong colour count would desync it.
        wire(&mut b, false);
        let t = decode(&section(2, &b), &Schema::default(), 8137).unwrap();
        for e in &t {
            let Tessellation::Wire(w) = e else { panic!("not a wire") };
            assert_eq!(w, &[vec![[0., 0., 0.], [1., 0., 0.], [1., 1., 0.], [0., 1., 0.], [1., 1., 0.]]]);
        }
    }
}

#[test]
fn a_closing_wire_colours_its_implicit_point() {
    // Per-point RGB: 4 indexed points + 1 implicit = 5 colours.
    let mut b = W::default();
    square(&mut b, 175);
    uints(&mut b, &[2, 0, 3, WIRE_IS_CONTINUOUS | WIRE_IS_CLOSING | 2, 6, 9]);
    b.bit(true).bit(false).bit(false).bit(false).put(1, 24);
    for _ in 1..5 {
        b.bit(true);
    }
    wire(&mut b, false);
    let t = decode(&section(2, &b), &Schema::default(), 8137).unwrap();
    assert!(matches!(t[1], Tessellation::Wire(_)));
    assert_eq!(t[0], t[1]);
}

#[test]
fn a_wire_index_must_name_a_point() {
    let mut b = W::default();
    square(&mut b, 175);
    uints(&mut b, &[2, 0, 12]);
    b.bit(false);
    assert!(matches!(decode(&section(1, &b), &Schema::default(), 8137), Err(PrcError::Malformed(_))));
}

#[test]
fn a_wire_without_indices_is_the_coordinate_polyline() {
    let mut b = W::default();
    square(&mut b, 175);
    b.uint(0).bit(true);
    // 4 implicit points -> 4 RGB colours, all "same" after the first.
    b.bit(false).bit(false).bit(false).put(1, 24);
    for _ in 1..4 {
        b.bit(true);
    }
    wire(&mut b, false);
    let t = decode(&section(2, &b), &Schema::default(), 8137).unwrap();
    let Tessellation::Wire(w) = &t[0] else { panic!("not a wire") };
    assert_eq!(w.len(), 1);
    assert_eq!(w[0].len(), 4);
    assert!(matches!(t[1], Tessellation::Wire(_)));
}

/// Reused colours cost one bit each, so a long run of them may end the section.
#[test]
fn reused_vertex_colours_may_be_the_last_data() {
    let n = 40;
    let mut b = W::default();
    b.uint(175).bit(false).uint(3 * n);
    for i in 0..3 * n {
        b.double(f64::from(i));
    }
    // No indices: the polyline of all `n` points, one RGB colour then `n - 1` reuses.
    b.uint(0).bit(true).bit(false).bit(false).bit(false).put(1, 24);
    for _ in 1..n {
        b.bit(true);
    }
    let t = decode(&section(1, &b), &Schema::default(), 8137).unwrap();
    let Tessellation::Wire(w) = &t[0] else { panic!("not a wire") };
    assert_eq!(w[0].len(), n as usize);
}

/// A square as one triangle plus one 4-vertex fan (two entities, three triangles), the face
/// carrying `attrs` and, when it has any, behaviour 0x30; then `colours` per-point colours
/// and a trailing wire.
fn styled_face(attrs: &[u32], colours: usize) -> Vec<Tessellation> {
    decode(&styled_face_section(attrs, colours, 0x30), &Schema::default(), 8137).unwrap()
}

/// [`styled_face`]'s section, the face's behaviour `behaviour`.
fn styled_face_section(attrs: &[u32], colours: usize, behaviour: u32) -> W {
    let mut b = W::default();
    square(&mut b, 172);
    b.bit(true).bit(false).bit(false).uint(0).uint(0);
    uints(&mut b, &[0, 0, 0, 3, 0, 6, 0, 0, 0, 3, 0, 6, 0, 9]);
    b.uint(1).uint(174);
    uints(&mut b, attrs);
    b.uint(0).uint(0).uint(0x2 | 0x4).uint(0);
    uints(&mut b, &[1, 1, 4]);
    b.uint(0).bit(colours > 0);
    if colours > 0 {
        b.bit(false).bit(false).put(0x10_2030, 24);
        for _ in 1..colours {
            b.bit(true);
        }
    }
    if !attrs.is_empty() {
        b.uint(behaviour);
    }
    b.uint(0);
    wire(&mut b, false);
    section(2, &b)
}

#[test]
fn face_vertex_colours_count_points_not_data() {
    // A triangle plus a fan of 4 vertices = 7 points -> 7 colours.
    let t = styled_face(&[], 7);
    assert_eq!(mesh(&t[0]).triangles.len(), 3);
    assert!(matches!(t[1], Tessellation::Wire(_)));
}

fn styles(attrs: &[u32]) -> Vec<(u32, u16)> {
    let t = styled_face(attrs, 0);
    assert!(matches!(t[1], Tessellation::Wire(_)));
    mesh(&t[0]).triangle_styles.iter().map(|g| (g.style, g.behaviour)).collect()
}

#[test]
fn face_line_attributes_style_its_triangles() {
    assert!(styles(&[]).is_empty());
    assert_eq!(styles(&[5]), [(5, 0x30); 3]);
    // One per entity: the triangle, then both triangles of the fan.
    assert_eq!(styles(&[4, 7]), [(4, 0x30), (7, 0x30), (7, 0x30)]);
    // An entity styled 0 inherits, as do entities past the list.
    assert_eq!(styles(&[4, 0, 9]), [(4, 0x30), (0, 0x30), (0, 0x30)]);
}

#[test]
fn markup_is_read_past() {
    let mut b = W::default();
    b.uint(176).bit(false).uint(3);
    for c in [1., 2., 3.] {
        b.double(c);
    }
    uints(&mut b, &[6, 0]);
    b.uint(1).string(Some("R1")).string(Some("label")).put(7, 8);
    wire(&mut b, false);
    let t = decode(&section(2, &b), &Schema::default(), 8137).unwrap();
    assert_eq!(t[0], Tessellation::Markup);
    assert!(matches!(t[1], Tessellation::Wire(_)));
}

#[test]
fn schema_fields_are_skipped_after_each_entity() {
    // A newer producer added one UnsignedInteger to TESS_3D and one to TESS_Face.
    let mut s = W::default();
    s.uint(2).uint(172).uint(1).uint(3).uint(174).uint(1).uint(3);
    let sb = s.bytes();
    let schema = Schema::read(&mut BitReader::new(&sb)).unwrap();

    let mut b = W::default();
    square(&mut b, 172);
    b.bit(true).bit(false).bit(false).uint(0).uint(0);
    uints(&mut b, &[0, 0, 0, 3, 0, 6]);
    b.uint(1).uint(174).uint(0).uint(0).uint(0).uint(0x2).uint(0);
    uints(&mut b, &[1]);
    b.uint(0).bit(false).uint(99); // the face's added field
    b.uint(0).uint(77); // texture count, then TESS_3D's added field
    wire(&mut b, false);
    let t = decode(&section(2, &b), &schema, 8137).unwrap();
    assert_eq!(mesh(&t[0]).triangles, [[0, 1, 2]]);
    assert!(matches!(t[1], Tessellation::Wire(_)));
    // Without the schema the same bytes do not decode to the same thing.
    assert_ne!(decode(&section(2, &b), &Schema::default(), 8137).ok(), Some(t));
}

#[test]
fn attributes_names_and_user_data_are_read_past() {
    let mut w = W::default();
    w.uint(305).uint(1).uint(ATTRIBUTE);
    w.bit(true).uint(2); // predefined title
    w.uint(2);
    w.bit(false).string(Some("k")).uint(4).string(Some("v"));
    w.bit(true).uint(1).uint(5).int(-1).uint(9);
    w.uint(42); // the attribute's schema-added field
    w.bit(false).string(Some("tess")).uint(1);
    wire(&mut w, false);
    w.uint(5).put(0b10110, 5);
    let mut sw = W::default();
    sw.uint(1).uint(ATTRIBUTE).uint(1).uint(3);
    let sb = sw.bytes();
    let schema = Schema::read(&mut BitReader::new(&sb)).unwrap();
    let bytes = w.bytes();
    let mut ctx = Ctx::new(BitReader::new(&bytes), &schema, 8137);
    let t = ctx.file_structure_tessellation().unwrap();
    assert!(matches!(t[..], [Tessellation::Wire(_)]));
    assert_eq!(ctx.name.as_deref(), Some("tess"));
    assert_eq!(ctx.r.position(), w.len());
}

/// A one-triangle TESS_3D_Compressed. `full` takes every optional branch: stored normals,
/// colours, line attributes, texture, behaviours. Otherwise `three_t` picks the 3T edge form.
fn compressed(w: &mut W, v: u32, full: bool, three_t: bool) {
    compressed_with_angles(w, v, full, three_t, 10);
}

/// [`compressed`] whose `full` form stores `angle_bits`-bit normal angles.
fn compressed_with_angles(w: &mut W, v: u32, full: bool, three_t: bool, angle_bits: u8) {
    w.uint(173).bit(false).bit(true).double(0.001);
    if v >= 7031 {
        w.put(0, 32).put(0, 32).put(0, 32);
    }
    // point_array: nine 2-bit values.
    w.bit(false).uint(9);
    for _ in 0..9 {
        w.put(2, 8);
    }
    for _ in 0..9 {
        w.bit(false).bit(true);
    }
    // edge_status_array: the 3T form (Huffman) or the T form.
    if full {
        w.bit(true).huffman(2, 2, &[(0, 0b10, 2)], &[0, 0, 0]);
    } else if three_t {
        w.bit(false).uint(3).put(0, 8).put(0, 8).put(0, 8);
    } else {
        w.bit(false).uint(1).put(0, 8);
    }
    // triangle_face_array [0]. `full`: four references, so the flagless reference array is
    // Huffman; else none.
    w.bit(false).uint(1).put(1, 8).bit(false);
    if full {
        w.uint(4).put(0b1111, 4);
        w.huffman(6, 2, &[(1, 0b10, 2), (0, 0b11, 2)], &[1, 0, 0, 0]);
        w.put(0, 4);
    } else {
        w.uint(3).put(0, 3).uint(0);
    }
    if full {
        w.bit(false).put(u64::from(angle_bits), 8).uint(3).put(0b101, 3);
        w.bit(true).huffman(10, 2, &[(5, 0b10, 2)], &[5, 5]);
        w.bit(true); // is_face_planar
        w.bit(true).bit(true).bit(false).uint(5);
        for c in [1, 255, 0, 0, 128] {
            w.put(c, 8);
        }
        w.bit(true).bit(false);
    } else {
        w.bit(true).bit(false).double(0.5).put(0, 8);
        w.bit(false).bit(false);
    }
    w.bit(false).uint(1).put(1, 8).put(0, 8); // line_attribute_array
    if full {
        w.bit(false).uint(1).put(0, 32).uint(32);
        w.uint(1).put(1, 5).bit(true).double(0.5).uint(2);
        w.put(0, 32).put(0, 32).bit(false).bit(true);
        w.bit(true).bit(false).uint(1).put(1, 8);
    } else {
        w.bit(true).bit(false);
    }
}

#[test]
fn a_compressed_mesh_is_read_to_its_end() {
    for (full, three_t) in [(false, false), (false, true), (true, true)] {
        for v in [7030, 7031, 8137] {
            let mut b = W::default();
            compressed(&mut b, v, full, three_t);
            let indices = [0, 0, 0, 3, 0, 6, 0, 0, 0, 3, 0, 6, 0, 9];
            tess_3d(&mut b, v, &Mesh { indices: &indices, faces: &[(0x2 | 0x4, 0, &[1, 1, 4])], ..Mesh::default() });
            let w = section(2, &b);
            let bytes = w.bytes();
            let schema = Schema::default();
            let mut ctx = Ctx::new(BitReader::new(&bytes), &schema, v);
            let t = ctx.file_structure_tessellation().unwrap();
            assert_eq!(ctx.r.position(), w.len(), "full={full} v={v}");
            assert!(matches!(t[1], Tessellation::Mesh(_)), "full={full} v={v}: {t:?}");
            // Stored normals and the one-status-per-triangle form are not rebuilt; the
            // recalculated 3T seed is.
            if full || !three_t {
                assert!(matches!(t[0], Tessellation::Compressed { triangles: 1, .. }), "full={full} v={v}: {t:?}");
            } else {
                let m = mesh(&t[0]);
                assert_eq!((m.triangles.as_slice(), m.faces.as_slice()), ([[0, 1, 2]].as_slice(), std::slice::from_ref(&(0..1))), "v={v}");
                assert!(m.normals_recalculated && m.normals.is_empty());
                assert_eq!(m.triangle_styles, [TriangleStyle { style: 1, behaviour: 0 }]);
                // Each point component is 1, in units of the 0.001 tolerance, from the origin 0.
                let want = [0.001, 0.002, 0.0025];
                assert!(m.positions.iter().zip(want).all(|(p, c)| p.iter().all(|x| (x - c).abs() < 1e-12)), "{:?}", m.positions);
            }
        }
    }
}

/// A `TESS_3D_Compressed` (authoring version 8137, origin 0) whose normals are recalculated,
/// with `points` in units of `tolerance`, the 3T `status`, each triangle's face, the slots'
/// reference flags, the references and one line attribute per face; every other optional
/// branch absent.
struct Packed<'a> {
    tolerance: f64,
    points: &'a [i64],
    status: &'a [u8],
    face_of: &'a [u32],
    /// The width of each face index; 8 unless an index needs more.
    face_bits: u32,
    is_ref: &'a [bool],
    refs: &'a [u32],
    line_attributes: &'a [u16],
}

/// A `CompressedIndiceArray` [WD 11.9] of `bits`-wide deltas, `implicit` with no compressed
/// flag.
fn indices(w: &mut W, v: &[u32], implicit: bool, bits: u32) {
    if !implicit {
        w.bit(false);
    }
    w.uint(u32::try_from(v.len()).unwrap());
    for i in 0..v.len() {
        w.put(if i == 0 { u64::from(bits) } else { 0 }, 8);
    }
    let mut prev = 0i64;
    for &x in v {
        let d = i64::from(x) - prev;
        w.bit(d < 0).put(d.unsigned_abs(), bits - 1);
        prev = i64::from(x);
    }
}

fn packed(w: &mut W, c: &Packed<'_>) {
    w.uint(173).bit(false).bit(true).double(c.tolerance);
    w.put(0, 32).put(0, 32).put(0, 32);
    // point_array: 8-bit sign-magnitude integers.
    w.bit(false).uint(u32::try_from(c.points.len()).unwrap());
    for _ in c.points {
        w.put(8, 8);
    }
    for &p in c.points {
        w.bit(p < 0).put(p.unsigned_abs(), 7);
    }
    w.bit(false).uint(u32::try_from(c.status.len()).unwrap());
    for &e in c.status {
        w.put(u64::from(e), 8);
    }
    indices(w, c.face_of, false, c.face_bits);
    w.uint(u32::try_from(c.is_ref.len()).unwrap());
    for &b in c.is_ref {
        w.bit(b);
    }
    assert!(c.refs.len() <= 3, "more than three references are Huffman-coded");
    indices(w, c.refs, true, 8);
    // must_recalculate_normals, normal_is_reversed, crease angle, flags; no colours, no
    // multiple line attributes.
    w.bit(true);
    for _ in c.face_of {
        w.bit(false);
    }
    w.double(0.5).put(0, 8).bit(false).bit(false);
    w.bit(false).uint(u32::try_from(c.line_attributes.len()).unwrap());
    for &a in c.line_attributes {
        w.put(u64::from(a & 0xff), 8).put(u64::from(a >> 8), 8);
    }
    // No texture, no behaviours.
    w.bit(true).bit(false);
}

/// A double-sided panel of two faces packed in walk order, its fold apex and its last two
/// apexes references: rebuilt into a mesh grouped by face, each triangle styled by its face.
#[test]
fn a_recalculated_compressed_mesh_is_rebuilt_by_face() {
    let mut b = W::default();
    packed(
        &mut b,
        &Packed {
            tolerance: 0.5,
            points: &[0, 0, 0, 4, 0, 0, -2, 4, 0, 1, 3, 0],
            status: &[3, 3, 2, 0, 0, 0, 0, 0, 0, 0, 0, 0],
            face_of: &[1, 0, 1, 0],
            face_bits: 8,
            is_ref: &[false, false, false, false, true, true],
            refs: &[0, 1],
            line_attributes: &[3, 0],
        },
    );
    let w = section(1, &b);
    let bytes = w.bytes();
    let schema = Schema::default();
    let mut ctx = Ctx::new(BitReader::new(&bytes), &schema, 8137);
    let t = ctx.file_structure_tessellation().unwrap();
    assert_eq!(ctx.r.position(), w.len());
    let m = mesh(&t[0]);
    // Walk order [0 1 2] [2 1 3] [2 3 0] [0 3 1]; faces 1, 0, 1, 0.
    assert_eq!(m.triangles, [[2, 1, 3], [0, 3, 1], [0, 1, 2], [2, 3, 0]]);
    assert_eq!(m.faces, [0..2, 2..4]);
    assert_eq!(m.positions[..3], [[0.0, 0.0, 0.0], [2.0, 0.0, 0.0], [0.0, 2.0, 0.0]]);
    let style = |style| TriangleStyle { style, behaviour: 0 };
    assert_eq!(m.triangle_styles, [style(3), style(3), style(0), style(0)]);
    assert!(m.normals_recalculated);
}

/// Arrays the walk cannot fit stay a [`Tessellation::Compressed`], read to their end.
#[test]
fn a_compressed_mesh_that_does_not_fit_is_left_compressed() {
    let mut b = W::default();
    // One point too many for a lone seed.
    let c = Packed {
        tolerance: 0.5,
        points: &[0; 12],
        status: &[0, 0, 0],
        face_of: &[0],
        face_bits: 8,
        is_ref: &[false; 3],
        refs: &[],
        line_attributes: &[0],
    };
    packed(&mut b, &c);
    let w = section(1, &b);
    let t = decode(&w, &Schema::default(), 8137).unwrap();
    assert!(matches!(t[..], [Tessellation::Compressed { triangles: 1, .. }]), "{t:?}");
}

/// A face index near `u32::MAX` claims four billion faces: once the triangles rebuild, they are
/// charged before any face range is made, so the decode stops at the element allowance instead
/// of allocating them.
#[test]
fn a_huge_face_index_is_charged_not_allocated() {
    let mut b = W::default();
    let c = Packed {
        tolerance: 0.5,
        points: &[0; 9],
        status: &[0, 0, 0],
        face_of: &[u32::MAX - 1],
        face_bits: 33,
        is_ref: &[false; 3],
        refs: &[],
        line_attributes: &[],
    };
    packed(&mut b, &c);
    let r = decode(&section(1, &b), &Schema::default(), 8137);
    assert!(matches!(r, Err(PrcError::TooManyElements { .. })), "{r:?}");
}

/// The same huge face index on arrays the walk cannot fit charges nothing for triangles or
/// faces: the tessellation stays compressed and the file reads on.
#[test]
fn arrays_that_do_not_fit_spend_no_triangles_or_faces() {
    let mut b = W::default();
    let c = Packed {
        tolerance: 0.5,
        points: &[0; 12],
        status: &[0, 0, 0],
        face_of: &[u32::MAX - 1],
        face_bits: 33,
        is_ref: &[false; 3],
        refs: &[],
        line_attributes: &[],
    };
    packed(&mut b, &c);
    let t = decode(&section(1, &b), &Schema::default(), 8137).unwrap();
    assert!(matches!(t[..], [Tessellation::Compressed { triangles: 1, .. }]), "{t:?}");
}

/// The two-face panel of [`a_recalculated_compressed_mesh_is_rebuilt_by_face`], its second
/// reference `last_ref`.
fn panel(last_ref: u32) -> W {
    let mut b = W::default();
    packed(
        &mut b,
        &Packed {
            tolerance: 0.5,
            points: &[0, 0, 0, 4, 0, 0, -2, 4, 0, 1, 3, 0],
            status: &[3, 3, 2, 0, 0, 0, 0, 0, 0, 0, 0, 0],
            face_of: &[1, 0, 1, 0],
            face_bits: 8,
            is_ref: &[false, false, false, false, true, true],
            refs: &[0, last_ref],
            line_attributes: &[3, 0],
        },
    );
    section(1, &b)
}

/// A rebuilt mesh is charged its triangles, each with its style, and each face range at its
/// size; the same arrays left compressed (a reference to a vertex not yet made) are charged
/// neither.
#[test]
fn a_rebuilt_compressed_mesh_is_charged_its_triangles_and_faces() {
    let fits = panel(1);
    let t = decode(&fits, &Schema::default(), 8137).unwrap();
    assert_eq!(mesh(&t[0]).triangles.len(), 4);
    let left = panel(9);
    let t = decode(&left, &Schema::default(), 8137).unwrap();
    assert!(matches!(t[..], [Tessellation::Compressed { triangles: 4, .. }]), "{t:?}");
    assert_eq!(spent(&fits) - spent(&left), 4 * triangle_elements(false, 0, true) + 2 * RANGE_ELEMENTS);
    assert_eq!(triangle_elements(false, 0, true), 3);
    #[cfg(target_pointer_width = "64")]
    assert_eq!(RANGE_ELEMENTS, 2);
}

/// A rebuild whose walk would outgrow the allowance left keeps the arrays compressed, even
/// when the rebuilt mesh alone would have fit, and the stream reads on.
#[test]
fn a_compressed_rebuild_is_bounded_by_the_allowance_left() {
    let fits = panel(1);
    let bytes = fits.bytes();
    let schema = Schema::default();
    let read = |n| Ctx::new(BitReader::with_elements(&bytes, n), &schema, 8137).file_structure_tessellation();
    let arrays = spent(&panel(9));
    let mesh_alone = spent(&fits) - arrays;
    let t = read(arrays + mesh_alone).unwrap();
    assert!(matches!(t[..], [Tessellation::Compressed { triangles: 4, .. }]), "{t:?}");
    assert!(matches!(read(crate::MAX_DECODED_ELEMENTS).unwrap()[..], [Tessellation::Mesh(_)]));
}

/// A face's nonzero behaviour styles its triangles even when its line attribute is 0 (the
/// owner's graphics); all zero, the faces give no styles.
#[test]
fn a_compressed_face_behaviour_alone_styles_its_triangles() {
    let attributes = |behaviours: Vec<i32>| FaceAttributes { multi: Vec::new(), line_attributes: vec![0], behaviours };
    let a = attributes(vec![0x30]);
    assert!(a.styled(1));
    assert_eq!(a.styles(&[0, 0], 1), [TriangleStyle { style: 0, behaviour: 0x30 }; 2]);
    assert!(!attributes(vec![0]).styled(1));
    assert!(!attributes(Vec::new()).styled(1));
}

fn rebuilt(triangles: usize) -> compressed::Rebuilt {
    compressed::Rebuilt { positions: vec![[0.0; 3]; 3], triangles: (0..triangles).map(|i| [0, 1, u32::try_from(i).unwrap()]).collect() }
}

/// Triangles are grouped by face in walk order within each, faces no triangle names (between
/// and after) get empty ranges, and each style moves with its triangle.
#[test]
fn rebuilt_triangles_are_grouped_by_face_with_their_styles() {
    let face_of = [2, 0, 2, 1, 0];
    let m = by_face(rebuilt(5), &face_of, 5, &[]);
    let apex: Vec<u32> = m.triangles.iter().map(|t| t[2]).collect();
    assert_eq!(apex, [1, 4, 3, 0, 2]);
    assert_eq!(m.faces, [0..2, 2..3, 3..5, 5..5, 5..5]);
    assert!(m.triangle_styles.is_empty());
    assert!(m.normals_recalculated);

    let style = |style| TriangleStyle { style, behaviour: u16::try_from(style).unwrap() };
    let styles: Vec<TriangleStyle> = face_of.iter().map(|&f| style(f + 1)).collect();
    let m = by_face(rebuilt(5), &[2, 0, 2, 3, 0], 4, &styles);
    let apex: Vec<u32> = m.triangles.iter().map(|t| t[2]).collect();
    assert_eq!(apex, [1, 4, 0, 2, 3]);
    assert_eq!(m.faces, [0..2, 2..2, 2..4, 4..5]);
    assert_eq!(m.triangle_styles, [style(1), style(1), style(3), style(3), style(2)]);
}

#[test]
fn refusals_and_damage_are_errors() {
    // Edge statuses neither T nor 3T long.
    let mut c = W::default();
    c.uint(173).bit(false).bit(true).double(0.001);
    c.put(0, 32).put(0, 32).put(0, 32);
    c.bit(false).uint(0).bit(false).uint(2).put(0, 16);
    c.bit(false).uint(1).put(1, 8).bit(false);
    assert!(matches!(decode(&section(1, &c), &Schema::default(), 8137), Err(PrcError::Malformed(_))));
    // Optimised vertex colours.
    let mut o = W::default();
    square(&mut o, 175);
    o.uint(0).bit(true).bit(false).bit(false).bit(true);
    assert!(matches!(decode(&section(1, &o), &Schema::default(), 8137), Err(PrcError::Unsupported(_))));
    // A point index that is not a multiple of 3, and one past the end.
    for bad in [4, 12] {
        let mut b = W::default();
        tess_3d(&mut b, 8137, &Mesh { indices: &[0, 0, 0, bad, 0, 6], faces: &[(0x2, 0, &[1])], ..Mesh::default() });
        assert!(matches!(decode(&section(1, &b), &Schema::default(), 8137), Err(PrcError::Malformed(_))));
    }
    // A fan whose count claims a billion points.
    let mut b = W::default();
    tess_3d(&mut b, 8137, &Mesh { indices: &[0, 0], faces: &[(0x4, 0, &[1, 0x3FFF_FFFF])], ..Mesh::default() });
    assert!(decode(&section(1, &b), &Schema::default(), 8137).is_err());
    // A tessellation count far past the data.
    let mut w = W::default();
    w.uint(305).uint(0).bit(true).uint(u32::MAX);
    assert!(matches!(decode(&w, &Schema::default(), 8137), Err(PrcError::Truncated(_))));
    // A wrong entity type where the section's own belongs.
    let mut w = W::default();
    w.uint(306);
    assert!(matches!(decode(&w, &Schema::default(), 8137), Err(PrcError::Malformed(_))));
}

#[test]
fn every_truncation_of_a_section_is_an_error_not_a_panic() {
    let mut b = W::default();
    compressed(&mut b, 8137, true, true);
    wire(&mut b, true);
    let bytes = section(2, &b).bytes();
    for end in 0..bytes.len() {
        let cut = bytes.get(..end).unwrap();
        // The zero padding of the last byte may still decode; anything shorter must not.
        let _ = Ctx::new(BitReader::new(cut), &Schema::default(), 8137).file_structure_tessellation();
    }
    let cut = bytes.get(..bytes.len() / 2).unwrap();
    assert!(Ctx::new(BitReader::new(cut), &Schema::default(), 8137).file_structure_tessellation().is_err());
}

/// A one-file-structure PRC stream whose tessellation section is `tess`, with an empty
/// globals section schema.
fn prc_file(tess: &[u8]) -> Vec<u8> {
    let mut globals = W::default();
    globals.uint(0);
    prc_file_with(&globals.bytes(), tess)
}

/// As [`prc_file`] with the globals section `globals`.
fn prc_file_with(globals: &[u8], tess: &[u8]) -> Vec<u8> {
    use std::io::Write as _;
    const HEADER_LEN: usize = 107;
    let zlib = |data: &[u8]| {
        let mut e = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::default());
        e.write_all(data).unwrap();
        e.finish().unwrap()
    };
    let u32s = |out: &mut Vec<u8>, vs: &[u32]| vs.iter().for_each(|v| out.extend_from_slice(&v.to_le_bytes()));
    let sections: [&[u8]; 5] = [globals, b"", tess, b"", b""];
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

#[test]
fn a_prc_file_yields_its_tessellations() {
    let f = crate::PrcFile::parse(&prc_file(&textured_square().bytes())).unwrap();
    let t = f.file_structures[0].tessellations().unwrap();
    assert_eq!(mesh(&t[0]).triangles, [[0, 1, 2], [0, 2, 3]]);
}

#[test]
fn normal_angles_wider_than_16_bits_are_refused_by_name() {
    let mut b = W::default();
    compressed_with_angles(&mut b, 8137, true, true, 17);
    assert!(matches!(decode(&section(1, &b), &Schema::default(), 8137), Err(PrcError::Malformed(m)) if m == "17-bit normal angles"));
}

/// An entity's counted array is charged against the reader's element allowance.
#[test]
fn counted_entity_arrays_are_charged_against_the_allowance() {
    let mut w = W::default();
    w.uint(3).uint(1).uint(2).uint(3);
    let bytes = w.bytes();
    let schema = Schema::default();
    let read = |n| Ctx::new(BitReader::with_elements(&bytes, n), &schema, 8137).uints("indices");
    assert!(matches!(read(2), Err(PrcError::TooManyElements { .. })));
    assert_eq!(read(3).unwrap(), [1, 2, 3]);
}

#[test]
fn a_face_behaviour_wider_than_16_bits_is_malformed() {
    let r = decode(&styled_face_section(&[1], 0, 0x1_0030), &Schema::default(), 8137);
    assert!(matches!(r, Err(PrcError::Malformed(m)) if m == "face behaviour 65584 is wider than 16 bits"));
}

#[test]
fn coordinates_that_are_not_whole_points_are_malformed() {
    let mut partial = SQUARE.to_vec();
    partial.push(7.0);
    let mut b = W::default();
    let faces: &[(u32, u32, &[u32])] = &[(0x2, 0, &[1])];
    tess_3d(&mut b, 8137, &Mesh { indices: &[0, 0, 0, 3, 0, 6], faces, coords: Some(&partial), ..Mesh::default() });
    let r = decode(&section(1, &b), &Schema::default(), 8137);
    assert!(matches!(r, Err(PrcError::Malformed(m)) if m == "13 coordinates are not whole points"));
    // A partial last normal drops the stored normals, as a slot naming none does.
    assert_eq!(stored_normals(&[0.0, 0.0, 1.0, 0.0], &[[0, 0, 0]], 1), (Vec::new(), Vec::new()));
}

/// Elements a tessellation section spends from a full allowance.
fn spent(section: &W) -> usize {
    let bytes = section.bytes();
    let schema = Schema::default();
    let mut ctx = Ctx::new(BitReader::new(&bytes), &schema, 8137);
    ctx.file_structure_tessellation().unwrap();
    crate::MAX_DECODED_ELEMENTS - ctx.r.elements_left()
}

#[test]
fn faces_reusing_one_index_range_are_charged_for_each_triangle_they_make() {
    const INDICES: [u32; 12] = [0, 0, 0, 3, 0, 6, 0, 0, 0, 6, 0, 9];
    let square = |faces: &[(u32, u32, &[u32])]| {
        let mut b = W::default();
        tess_3d(&mut b, 8137, &Mesh { indices: &INDICES, faces, ..Mesh::default() });
        spent(&section(1, &b))
    };
    let one = square(&[(0x2, 0, &[2])]);
    let three = square(&[(0x2, 0, &[2]), (0x2, 0, &[2]), (0x2, 0, &[2])]);
    // Per extra face: its count entry, its one data element and its two triangles, each
    // 12 bytes of corners and 12 of normal slots.
    assert_eq!(three - one, 2 * (2 + 2 * 4));
}

#[test]
fn reused_textured_faces_are_charged_for_every_corner_array() {
    // The textured square, then one untextured triangle's normal and point slots.
    const INDICES: [u32; 24] = [0, 0, 0, 0, 2, 3, 0, 4, 6, 0, 0, 0, 0, 4, 6, 0, 6, 9, 0, 0, 0, 3, 0, 6];
    let square = |faces: &[(u32, u32, &[u32])], sets: u32| {
        let mut b = W::default();
        tess_3d(&mut b, 8137, &Mesh { indices: &INDICES, faces, textures: sets, uvs: &[0.0; 8], ..Mesh::default() });
        spent(&section(1, &b))
    };
    let face: (u32, u32, &[u32]) = (0x200, 0, &[2]);
    let untextured: (u32, u32, &[u32]) = (0x2, 18, &[1]);
    let one = square(&[face], 1);
    let three = square(&[face, face, face], 1);
    // Per extra face: its count, its data element and two triangles of corners, normal slots
    // and one texture set, 16 bytes a corner triple.
    assert_eq!(three - one, 2 * (2 + 2 * 6));
    // One textured face gives every triangle of the mesh a texture triple.
    let mixed = square(&[face, untextured], 1);
    assert_eq!(mixed - one, 2 + 6);
}

#[test]
fn a_face_is_charged_before_it_is_triangulated() {
    // One face claiming a million triangles over a 12-entry index array.
    const INDICES: [u32; 12] = [0, 0, 0, 3, 0, 6, 0, 0, 0, 6, 0, 9];
    let mut b = W::default();
    tess_3d(&mut b, 8137, &Mesh { indices: &INDICES, faces: &[(0x2, 0, &[1_000_000])], ..Mesh::default() });
    let bytes = section(1, &b).bytes();
    let schema = Schema::default();
    let r = Ctx::new(BitReader::with_elements(&bytes, 1000), &schema, 8137).file_structure_tessellation();
    assert!(matches!(r, Err(PrcError::TooManyElements { .. })), "{r:?}");
}

#[test]
fn the_globals_schema_and_the_tessellations_share_one_allowance() {
    // A schema of one entry with three tokens: four elements.
    let mut globals = W::default();
    globals.uint(1).uint(999).uint(3).uint(0).uint(0).uint(0);
    let tess = textured_square();
    let f = crate::PrcFile::parse(&prc_file_with(&globals.bytes(), &tess.bytes())).unwrap();
    let need = spent(&tess) + 4;
    let within = |elements| f.file_structures[0].tessellations_within(crate::bits::Budget { elements, ..crate::bits::Budget::FULL });
    assert!(within(need).is_ok());
    assert!(matches!(within(need - 1), Err(PrcError::TooManyElements { .. })));
}
