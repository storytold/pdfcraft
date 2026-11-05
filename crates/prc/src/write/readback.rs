//! Test-only: a written stream read back through this crate's reader.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use crate::{PrcFile, Tessellation, TriangleMesh};

/// What a PRC stream draws, every placement in world coordinates.
#[derive(Debug, Default)]
pub(crate) struct Model {
    /// Triangles over every placement.
    pub(crate) triangles: usize,
    /// Each placement's colour as straight 8-bit RGBA.
    pub(crate) colours: Vec<Option<[u8; 4]>>,
    /// Each placement's mesh, placed.
    pub(crate) meshes: Vec<TriangleMesh>,
}

/// `prc` parsed and its placements resolved to placed meshes.
pub(crate) fn assemble(prc: &[u8]) -> Model {
    let file = PrcFile::parse(prc).unwrap();
    let tessellations: Vec<Vec<Tessellation>> = file.file_structures.iter().map(|fs| fs.tessellations().unwrap()).collect();
    let mut out = Model::default();
    for p in file.placements().unwrap() {
        let Some(Tessellation::Mesh(mesh)) = tessellations.get(p.file_structure).and_then(|t| t.get(p.tessellation)) else {
            panic!("placement {p:?} names no mesh");
        };
        out.triangles += mesh.triangles.len();
        out.colours.push(p.colour.map(|c| [c.r, c.g, c.b, c.a].map(|v| (v.clamp(0.0, 1.0) * 255.0).round() as u8)));
        out.meshes.push(mesh.transformed(&p.matrix));
    }
    out
}
