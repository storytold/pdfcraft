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
    w.uint(entity).bit(false).uint(12);
    for c in SQUARE {
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
}

/// A TESS_3D over [`SQUARE`] at authoring version `v`.
fn tess_3d(w: &mut W, v: u32, m: &Mesh<'_>) {
    square(w, 172);
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
    assert!(matches!(decode(&section(1, &b), &Schema::default(), 8137), Err(PrcError::Malformed(_))));
    let sets = u32::try_from(MAX_TEXTURE_SETS).unwrap();
    let mut ok = W::default();
    tess_3d(&mut ok, 8137, &Mesh { indices: &[0], faces: &[(0x400, 0, &[1, 0])], textures: sets, ..Mesh::default() });
    assert!(decode(&section(1, &ok), &Schema::default(), 8137).is_ok());
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

/// A square as one triangle plus one 4-vertex fan (two entities, three triangles), the face
/// carrying `attrs` and, when it has any, behaviour 0x30; then `colours` per-point colours
/// and a trailing wire.
fn styled_face(attrs: &[u32], colours: usize) -> Vec<Tessellation> {
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
        b.uint(0x30);
    }
    b.uint(0);
    wire(&mut b, false);
    decode(&section(2, &b), &Schema::default(), 8137).unwrap()
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
        w.bit(false).put(10, 8).uint(3).put(0b101, 3);
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
            assert!(matches!(t[..], [Tessellation::Compressed { triangles: 1, .. }, Tessellation::Mesh(_)]), "full={full} v={v}: {t:?}");
            assert_eq!(ctx.r.position(), w.len(), "full={full} v={v}");
        }
    }
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
    use std::io::Write as _;
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

#[test]
fn a_prc_file_yields_its_tessellations() {
    let f = crate::PrcFile::parse(&prc_file(&textured_square().bytes())).unwrap();
    let t = f.file_structures[0].tessellations().unwrap();
    assert_eq!(mesh(&t[0]).triangles, [[0, 1, 2], [0, 2, 3]]);
}
