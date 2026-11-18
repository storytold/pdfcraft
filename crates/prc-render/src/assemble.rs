//! A parsed PRC file's tessellations placed in the world, ready for [`crate::render`].

use crate::RenderError;
use crate::raster::channel;
use craft_prc::{Matrix, PrcFile, Rgba, Tessellation, TriangleMesh};

/// The most triangles [`assemble`] places, over every placement: a tessellation many
/// occurrences draw is copied once per occurrence.
pub const MAX_ASSEMBLED_TRIANGLES: usize = 1 << 24;

/// The most positions, normals and texture coordinates [`assemble`] copies, over every placement.
pub const MAX_ASSEMBLED_VERTICES: usize = 1 << 24;

/// A triangle mesh in world coordinates with the colours it draws in.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct WorldMesh {
    /// The triangles, already placed.
    pub mesh: TriangleMesh,
    /// The mesh's straight-RGBA colour; `None` takes [`crate::RenderOptions::colour`], opaque.
    pub colour: Option<[u8; 4]>,
    /// Per triangle, its own colour when its face is styled; `None` (the whole list or an entry)
    /// takes [`Self::colour`].
    pub triangle_colours: Option<Vec<Option<[u8; 4]>>>,
}

impl WorldMesh {
    /// `mesh`, uncoloured.
    #[must_use]
    pub fn new(mesh: TriangleMesh) -> Self {
        WorldMesh { mesh, colour: None, triangle_colours: None }
    }

    /// The colour triangle `t` draws in, before the render options' default.
    pub(crate) fn colour_of(&self, t: usize) -> Option<[u8; 4]> {
        self.triangle_colours.as_ref().and_then(|c| c.get(t).copied().flatten()).or(self.colour)
    }
}

/// `mesh` moved into the world by `matrix`, drawn in `colour` and, per triangle, `triangle_colours`.
#[must_use]
pub fn place(mesh: &TriangleMesh, matrix: &Matrix, colour: Option<[u8; 4]>, triangle_colours: Option<Vec<Option<[u8; 4]>>>) -> WorldMesh {
    WorldMesh { mesh: mesh.transformed(matrix), colour, triangle_colours }
}

/// Every triangle mesh `prc` draws, in world coordinates and coloured as its product tree says
/// ([`PrcFile::placements`]). A file with no tree draws each file structure's meshes where they
/// are, uncoloured. Wires, markups and compressed tessellations are not drawn.
///
/// # Errors
/// [`RenderError::Model`] when a tessellation section or the tree cannot be read, or the tree
/// places a tessellation its file structure does not have; [`RenderError::TooManyTriangles`] past
/// [`MAX_ASSEMBLED_TRIANGLES`] and [`RenderError::TooManyVertices`] past [`MAX_ASSEMBLED_VERTICES`],
/// counting both what is decoded and what is placed.
pub fn assemble(prc: &PrcFile) -> Result<Vec<WorldMesh>, RenderError> {
    let tessellations = decode(prc.file_structures.iter().map(|fs| fs.tessellations()), Budget::default())?;
    let mut budget = Budget::default();
    let mut out = Vec::new();
    if !prc.stores_tree() {
        for mesh in tessellations.iter().flatten().filter_map(as_mesh) {
            budget.spend(mesh)?;
            out.push(WorldMesh::new(mesh.clone()));
        }
        return Ok(out);
    }
    let items = prc.placements()?.into_iter().map(|p| Item {
        file_structure: p.file_structure,
        tessellation: p.tessellation,
        matrix: p.matrix,
        colour: p.colour,
        triangle_colours: Box::new(move |mesh: &TriangleMesh| p.triangle_colours(mesh)),
    });
    place_items(&tessellations, items, &mut budget, &mut out)?;
    Ok(out)
}

/// Each file structure's tessellations, decoded one structure at a time and charged to `budget`
/// as they are, so decoding stops at the structure that passes a ceiling.
pub(crate) fn decode(
    structures: impl IntoIterator<Item = Result<Vec<Tessellation>, craft_prc::PrcError>>,
    mut budget: Budget,
) -> Result<Vec<Vec<Tessellation>>, RenderError> {
    let mut out = Vec::new();
    for tessellations in structures {
        let tessellations = tessellations?;
        for mesh in tessellations.iter().filter_map(as_mesh) {
            budget.spend(mesh)?;
        }
        out.push(tessellations);
    }
    Ok(out)
}

/// One placement as [`place_items`] reads it: a tessellation by file structure and index, where
/// it goes and how it is coloured.
pub(crate) struct Item<'a> {
    pub(crate) file_structure: usize,
    pub(crate) tessellation: usize,
    pub(crate) matrix: Matrix,
    pub(crate) colour: Option<Rgba>,
    pub(crate) triangle_colours: Box<FaceColours<'a>>,
}

/// Per triangle of a mesh, the colour its face's style resolves to.
type FaceColours<'a> = dyn Fn(&TriangleMesh) -> Option<Vec<Option<Rgba>>> + 'a;

/// Each item's mesh moved into the world and coloured, appended to `out`. Items naming a wire,
/// markup or compressed tessellation are skipped; an item naming no tessellation at all is
/// [`PrcError::Malformed`](craft_prc::PrcError::Malformed).
pub(crate) fn place_items<'a>(
    tessellations: &[Vec<Tessellation>],
    items: impl IntoIterator<Item = Item<'a>>,
    budget: &mut Budget,
    out: &mut Vec<WorldMesh>,
) -> Result<(), RenderError> {
    for p in items {
        let Some(tessellation) = tessellations.get(p.file_structure).and_then(|t| t.get(p.tessellation)) else {
            let what = format!("tessellation {} of file structure {}, which has none by that index", p.tessellation, p.file_structure);
            return Err(craft_prc::PrcError::Malformed(what).into());
        };
        let Some(mesh) = as_mesh(tessellation) else {
            continue;
        };
        budget.spend(mesh)?;
        let triangle_colours = (p.triangle_colours)(mesh).map(|c| c.into_iter().map(|c| c.map(bytes)).collect());
        out.push(place(mesh, &p.matrix, p.colour.map(bytes), triangle_colours));
    }
    Ok(())
}

/// Triangles and vertex entries (positions, normals, texture coordinates) placed so far.
#[derive(Debug, Default)]
pub(crate) struct Budget {
    pub(crate) triangles: usize,
    pub(crate) vertices: usize,
}

impl Budget {
    fn spend(&mut self, mesh: &TriangleMesh) -> Result<(), RenderError> {
        let triangles =
            self.triangles.checked_add(mesh.triangles.len()).filter(|&n| n <= MAX_ASSEMBLED_TRIANGLES).ok_or(RenderError::TooManyTriangles)?;
        let vertices = [mesh.positions.len(), mesh.normals.len(), mesh.uvs.len()]
            .into_iter()
            .try_fold(self.vertices, usize::checked_add)
            .filter(|&n| n <= MAX_ASSEMBLED_VERTICES)
            .ok_or(RenderError::TooManyVertices)?;
        (self.triangles, self.vertices) = (triangles, vertices);
        Ok(())
    }
}

fn as_mesh(t: &Tessellation) -> Option<&TriangleMesh> {
    match t {
        Tessellation::Mesh(m) => Some(m),
        _ => None,
    }
}

/// A 0–1 colour as straight-RGBA bytes, each channel clamped.
fn bytes(c: Rgba) -> [u8; 4] {
    [c.r, c.g, c.b, c.a].map(|v| channel(v.clamp(0.0, 1.0) * 255.0))
}

#[cfg(test)]
mod tests;
