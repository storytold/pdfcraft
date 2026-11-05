#![allow(clippy::unwrap_used)]

use super::*;

fn triangle() -> TriangleMesh {
    let mut m = TriangleMesh::default();
    m.positions = vec![[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]];
    m.triangles = vec![[0, 1, 2]];
    m
}

#[test]
fn a_placement_moves_its_mesh_into_the_world() {
    let mut shift = Matrix::IDENTITY;
    shift.0[0][3] = 5.0;
    shift.0[2][3] = -2.0;
    let placed = place(&triangle(), &shift, Some([1, 2, 3, 255]), None);
    assert_eq!(placed.mesh.positions, [[5.0, 0.0, -2.0], [6.0, 0.0, -2.0], [5.0, 1.0, -2.0]]);
    assert_eq!(placed.colour, Some([1, 2, 3, 255]));
    // A mirror keeps the triangle facing the same way round.
    let mut mirror = Matrix::IDENTITY;
    mirror.0[0][0] = -1.0;
    assert_eq!(place(&triangle(), &mirror, None, None).mesh.triangles, [[0, 2, 1]]);
}

#[test]
fn a_triangle_colour_wins_over_the_mesh_colour() {
    let mut m = place(&triangle(), &Matrix::IDENTITY, Some([9, 9, 9, 255]), Some(vec![Some([1, 1, 1, 255]), None]));
    m.mesh.triangles.push([0, 2, 1]);
    m.mesh.triangles.push([1, 2, 0]);
    assert_eq!(m.colour_of(0), Some([1, 1, 1, 255]));
    assert_eq!(m.colour_of(1), Some([9, 9, 9, 255]), "an unstyled face takes the mesh's colour");
    assert_eq!(m.colour_of(2), Some([9, 9, 9, 255]), "past the list too");
    assert_eq!(WorldMesh::new(triangle()).colour_of(0), None);
}

#[test]
fn colours_are_clamped_into_bytes() {
    let [r, g, b, a] = [0.0, 1.0, 0.5, 2.0];
    let mut c = Rgba::default();
    (c.r, c.g, c.b, c.a) = (r, g, b, a);
    assert_eq!(bytes(c), [0, 255, 128, 255]);
    c.r = f64::NAN;
    c.g = -3.0;
    assert_eq!(bytes(c)[..2], [0, 0]);
}

#[test]
fn the_triangle_budget_is_shared_and_refuses_past_the_ceiling() {
    let mut b = Budget { triangles: MAX_ASSEMBLED_TRIANGLES - 1, vertices: 0 };
    assert!(b.spend(&triangle()).is_ok());
    assert_eq!(b.spend(&triangle()), Err(RenderError::TooManyTriangles));
    let mut b = Budget { triangles: usize::MAX, vertices: 0 };
    assert_eq!(b.spend(&triangle()), Err(RenderError::TooManyTriangles));
}

#[test]
fn the_vertex_budget_counts_positions_normals_and_uvs() {
    let mut heavy = triangle();
    heavy.normals = vec![[0.0, 0.0, 1.0]; 2];
    heavy.uvs = vec![[0.0; 2]; 4];
    let mut b = Budget::default();
    b.spend(&heavy).unwrap();
    assert_eq!((b.triangles, b.vertices), (1, 9));
    // One triangle over many vertices: the triangle ceiling alone would let it through.
    let mut b = Budget { triangles: 0, vertices: MAX_ASSEMBLED_VERTICES - 8 };
    assert_eq!(b.spend(&heavy), Err(RenderError::TooManyVertices));
    assert_eq!((b.triangles, b.vertices), (0, MAX_ASSEMBLED_VERTICES - 8), "a refused mesh spends nothing");
    let mut b = Budget { triangles: 0, vertices: usize::MAX };
    assert_eq!(b.spend(&triangle()), Err(RenderError::TooManyVertices));
}

fn item<'a>(fs: usize, tess: usize, x: f64, colour: Option<Rgba>) -> Item<'a> {
    let mut matrix = Matrix::IDENTITY;
    matrix.0[0][3] = x;
    Item { file_structure: fs, tessellation: tess, matrix, colour, triangle_colours: Box::new(|_| None) }
}

#[test]
fn each_placement_draws_its_tessellation_where_and_in_the_colour_the_tree_says() {
    let tessellations = vec![vec![Tessellation::Mesh(triangle())]];
    let mut red = Rgba::default();
    red.r = 1.0;
    red.a = 1.0;
    // Two occurrences of one tessellation at different places, one coloured.
    let items = [item(0, 0, 3.0, Some(red)), item(0, 0, -4.0, None)];
    let (mut budget, mut out) = (Budget::default(), Vec::new());
    place_items(&tessellations, items, &mut budget, &mut out).unwrap();
    assert_eq!(out.len(), 2);
    assert_eq!(out[0].mesh.positions, [[3.0, 0.0, 0.0], [4.0, 0.0, 0.0], [3.0, 1.0, 0.0]]);
    assert_eq!(out[0].colour, Some([255, 0, 0, 255]));
    assert_eq!(out[1].mesh.positions, [[-4.0, 0.0, 0.0], [-3.0, 0.0, 0.0], [-4.0, 1.0, 0.0]]);
    assert_eq!(out[1].colour, None);
    assert_eq!(budget.triangles, 2, "each occurrence spends its triangles");
}

#[test]
fn a_placement_naming_a_missing_tessellation_is_malformed() {
    let tessellations = vec![vec![Tessellation::Mesh(triangle())]];
    for (fs, tess) in [(1, 0), (0, 7)] {
        let (mut budget, mut out) = (Budget::default(), Vec::new());
        let r = place_items(&tessellations, [item(fs, tess, 0.0, None)], &mut budget, &mut out);
        assert!(matches!(r, Err(RenderError::Model(pdfcraft_prc::PrcError::Malformed(_)))), "{fs}/{tess}: {r:?}");
    }
}

#[test]
fn a_placement_past_the_triangle_budget_is_an_error() {
    let tessellations = vec![vec![Tessellation::Mesh(triangle())]];
    let (mut budget, mut out) = (Budget { triangles: MAX_ASSEMBLED_TRIANGLES, vertices: 0 }, Vec::new());
    let r = place_items(&tessellations, [item(0, 0, 0.0, None)], &mut budget, &mut out);
    assert!(matches!(r, Err(RenderError::TooManyTriangles)));
    assert!(out.is_empty());
}

#[test]
fn decoding_stops_at_the_structure_that_passes_the_ceiling() {
    let pulled = std::cell::Cell::new(0);
    let structures = std::iter::repeat_with(|| {
        pulled.set(pulled.get() + 1);
        Ok(vec![Tessellation::Mesh(triangle())])
    })
    .take(3);
    let nearly_full = Budget { triangles: MAX_ASSEMBLED_TRIANGLES - 1, vertices: 0 };
    assert_eq!(decode(structures, nearly_full).err(), Some(RenderError::TooManyTriangles));
    assert_eq!(pulled.get(), 2, "the third structure is never decoded");
}
