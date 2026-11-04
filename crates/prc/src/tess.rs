//! The tessellation section: `FileStructureTessellation` (305) and its entities, `TESS_3D`
//! (172) with its `TESS_Face`s (174), `TESS_3D_Wire` (175), `TESS_Markup` (176) and
//! `TESS_3D_Compressed` (173) [WD 7.3.7, 7.8].
//!
//! `TESS_3D_Compressed` is read field by field to its end [WD 7.8.9.7] so the entities after
//! it decode; its triangles are not rebuilt.
//!
//! The schema runs after the fields of each concrete type read here (201, 172, 173, 174, 175,
//! 176, 305), not for the abstract levels (`PRCBase`, `TESS`).

mod triangulate;

use crate::arrays;
use crate::bits::{BitReader, Budget};
use crate::entity::{Ctx, malformed};
use crate::{FileStructure, PrcError, SectionKind};
use triangulate::{Corners, Face};

const FILE_STRUCTURE_TESSELLATION: u32 = 305;
const TESS_3D: u32 = 172;
const TESS_3D_COMPRESSED: u32 = 173;
const TESS_FACE: u32 = 174;
const TESS_3D_WIRE: u32 = 175;
const TESS_MARKUP: u32 = 176;

/// prc-rs: `origin` exists in a compressed tessellation from this version (pdf-issues #705).
const ORIGIN_FROM: u32 = 7031;
/// prc-rs: `has_loops` exists from this file-structure authoring version.
const HAS_LOOPS_FROM: u32 = 7039;
/// prc-rs: `must_recalculate_normals` exists from this version.
const RECALCULATE_FROM: u32 = 7047;

/// A wire header's count bits; the top four are flags [WD 7.8.7.5].
const WIRE_COUNT_MASK: u32 = 0x0FFF_FFFF;
const WIRE_IS_CLOSING: u32 = 0x1000_0000;
const WIRE_IS_CONTINUOUS: u32 = 0x2000_0000;

/// Normal angles wider than this are not a `ShortArray` [WD 7.8.9.7].
const MAX_ANGLE_BITS: u32 = 16;

/// One entry of the tessellation array; a representation item's `index_tessellation` selects
/// one by position.
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub enum Tessellation {
    /// A `TESS_3D` triangle mesh.
    Mesh(TriangleMesh),
    /// A `TESS_3D_Wire`: polylines.
    Wire(Vec<Vec<[f64; 3]>>),
    /// A `TESS_Markup` (PMI drawing), read past but not decoded.
    Markup,
    /// A `TESS_3D_Compressed` mesh, read past but not rebuilt.
    #[non_exhaustive]
    Compressed {
        /// The entity's triangle count.
        triangles: usize,
    },
}

/// A `TESS_3D` decoded to indexed triangles, in the entity's own frame (the representation
/// item's placement is not applied).
#[derive(Debug, Clone, Default, PartialEq)]
#[non_exhaustive]
pub struct TriangleMesh {
    /// The coordinate array as points.
    pub positions: Vec<[f64; 3]>,
    /// Counter-clockwise triangles seen from the outside, as indices into
    /// [`Self::positions`] [WD 7.8.5.2].
    pub triangles: Vec<[u32; 3]>,
    /// `triangles` split by `TESS_Face`: face `i` is `triangles[faces[i].clone()]`.
    pub faces: Vec<std::ops::Range<usize>>,
    /// The producer asks the reader to compute normals (none are stored).
    pub normals_recalculated: bool,
    /// The stored vertex normals [WD 7.8.5.1], as written (not renormalised); empty when the
    /// mesh stores none or any of its normal indices is unusable.
    pub normals: Vec<[f64; 3]>,
    /// Per triangle, the index into [`Self::normals`] of each corner's normal, in the corner
    /// order of [`Self::triangles`]; empty exactly when [`Self::normals`] is.
    pub triangle_normals: Vec<[u32; 3]>,
    /// Per triangle, the style its face's line attributes give it [WD 7.8.6]; empty when no
    /// face carries any.
    pub triangle_styles: Vec<TriangleStyle>,
    /// The stored texture coordinates as (u, v) pairs [WD 7.8.5.1]; empty when the mesh
    /// stores none.
    pub uvs: Vec<[f64; 2]>,
    /// Per texture-coordinate set, per triangle, the index into [`Self::uvs`] of each
    /// corner's coordinate, in the corner order of [`Self::triangles`]; each set is as long
    /// as [`Self::triangles`], `None` for a triangle whose face stores no coordinate for the
    /// set. Empty when [`Self::uvs`] is.
    pub triangle_uvs: Vec<Vec<Option<[u32; 3]>>>,
}

/// The graphics a face's `line_attributes` give one triangle [WD 7.8.6].
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
#[non_exhaustive]
pub struct TriangleStyle {
    /// An index into the globals' style array plus one; 0 keeps the owner's graphics.
    pub style: u32,
    /// The face's behaviour bits [WD 5.4].
    pub behaviour: u16,
}

impl FileStructure {
    /// The tessellation section's entities, in the order a representation item's
    /// `index_tessellation` counts them [WD 7.3.7].
    ///
    /// # Errors
    /// As [`crate::Schema::read`] for the globals' schema; [`PrcError::Truncated`] or
    /// [`PrcError::Malformed`] for a damaged section; [`PrcError::Unsupported`] for
    /// optimised vertex colours.
    pub fn tessellations(&self) -> Result<Vec<Tessellation>, PrcError> {
        self.tessellations_within(Budget::FULL)
    }

    /// As [`Self::tessellations`], the globals' schema and the section sharing `budget`.
    pub(crate) fn tessellations_within(&self, budget: Budget) -> Result<Vec<Tessellation>, PrcError> {
        let mut globals = BitReader::with_budget(self.section(SectionKind::Globals), budget);
        let schema = crate::Schema::read(&mut globals)?;
        let r = BitReader::with_budget(self.section(SectionKind::Tessellation), globals.budget());
        Ctx::new(r, &schema, self.authoring_version).file_structure_tessellation()
    }
}

impl Ctx<'_, '_> {
    /// `FileStructureTessellation` (305) [WD 7.3.7].
    pub(crate) fn file_structure_tessellation(&mut self) -> Result<Vec<Tessellation>, PrcError> {
        self.expect_type(FILE_STRUCTURE_TESSELLATION)?;
        self.content_prc_base()?;
        let n = self.count(1, "tessellations")?;
        let mut out = Vec::with_capacity(n.min(1024));
        for _ in 0..n {
            out.push(match self.r.unsigned_integer()? {
                TESS_3D => Tessellation::Mesh(self.tess_3d()?),
                TESS_3D_WIRE => Tessellation::Wire(self.wire()?),
                TESS_MARKUP => {
                    self.markup()?;
                    Tessellation::Markup
                }
                TESS_3D_COMPRESSED => Tessellation::Compressed { triangles: self.tess_3d_compressed()? },
                t => return Err(malformed(format!("entity type {t} in the tessellation array"))),
            });
        }
        self.schema.skip_added_fields(FILE_STRUCTURE_TESSELLATION, &mut self.r)?;
        self.user_data()?;
        Ok(out)
    }

    /// `ContentBaseTessData` [WD 7.8.4]: `is_calculated`, then coordinates.
    fn base_tess_data(&mut self) -> Result<Vec<f64>, PrcError> {
        self.r.bit()?;
        self.doubles("tessellation coordinates")
    }

    /// `VertexColors` [WD 7.8.7.2] for `count` points, read past.
    fn vertex_colors(&mut self, count: usize, in_wire: bool) -> Result<(), PrcError> {
        let rgba = self.r.bit()?;
        // prc-rs: `is_segment_color` exists only inside a wire.
        let per_segment = in_wire && self.r.bit()?;
        if self.r.bit()? {
            return Err(PrcError::Unsupported("optimised vertex colours"));
        }
        let count = if per_segment { count / 2 } else { count };
        let bytes = if rgba { 4 } else { 3 };
        // The first colour is `8 * bytes` bits; every later one is at least its reuse bit.
        if count > 0 && (count - 1).saturating_add(8 * bytes) > self.r.remaining() {
            return Err(PrcError::Truncated("vertex colours"));
        }
        for i in 0..count {
            if i == 0 || !self.r.bit()? {
                for _ in 0..bytes {
                    self.r.character()?;
                }
            }
        }
        Ok(())
    }

    /// `TESS_3D` (172) after its type code [WD 7.8.5].
    fn tess_3d(&mut self) -> Result<TriangleMesh, PrcError> {
        let coords = self.base_tess_data()?;
        let _has_faces = self.r.bit()?;
        if self.version >= HAS_LOOPS_FROM {
            let _has_loops = self.r.bit()?;
        }
        let recalc = self.version >= RECALCULATE_FROM && self.r.bit()?;
        if recalc {
            let _flags = self.r.character()?;
            let _crease_angle = self.r.double()?;
        }
        let normal_coords = self.doubles("normal coordinates")?;
        let _wire_indices = self.uints("wire indices")?;
        let indices = self.uints("triangulated indices")?;
        let n_faces = self.count(1, "faces")?;
        let mut faces = Vec::with_capacity(n_faces.min(1024));
        for _ in 0..n_faces {
            self.expect_type(TESS_FACE)?;
            faces.push(self.face()?);
        }
        let texture = self.doubles("texture coordinates")?;
        self.schema.skip_added_fields(TESS_3D, &mut self.r)?;

        let mut mesh = TriangleMesh { positions: points(&coords)?, normals_recalculated: recalc, ..TriangleMesh::default() };
        let styled = faces.iter().any(|f| !f.styles.is_empty());
        let mut corners = Corners::default();
        for f in &faces {
            let start = mesh.triangles.len();
            let entities = f.triangulate(&indices, !recalc, mesh.positions.len(), &mut mesh.triangles, &mut corners)?;
            // Faces may reuse one index range, so the triangles are charged as they are made.
            self.r.take_elements(mesh.triangles.len().saturating_sub(start))?;
            mesh.faces.push(start..mesh.triangles.len());
            if styled {
                f.styles_into(&entities, &mut mesh.triangle_styles);
            }
        }
        (mesh.normals, mesh.triangle_normals) = stored_normals(&normal_coords, &corners.normals, mesh.triangles.len());
        (mesh.uvs, mesh.triangle_uvs) = stored_uvs(&texture, corners.textures);
        Ok(mesh)
    }

    /// `TESS_Face` (174) after its type code [WD 7.8.6].
    fn face(&mut self) -> Result<Face, PrcError> {
        let styles = self.uints("face line attributes")?;
        let _start_of_wire = self.r.unsigned_integer()?;
        let _sizes_wire = self.uints("face wire sizes")?;
        let flags = self.r.unsigned_integer()?;
        let start = self.r.unsigned_integer()? as usize;
        let data = self.uints("triangulated data")?;
        let textures = self.r.unsigned_integer()? as usize;
        let mut face = Face { flags, start, data, textures, styles, behaviour: 0 };
        if self.r.bit()? {
            let n = face.point_count()?;
            self.vertex_colors(n, false)?;
        }
        if !face.styles.is_empty() {
            let bits = self.r.unsigned_integer()?;
            face.behaviour = u16::try_from(bits).map_err(|_| malformed(format!("face behaviour {bits} is wider than 16 bits")))?;
        }
        self.schema.skip_added_fields(TESS_FACE, &mut self.r)?;
        Ok(face)
    }

    /// `TESS_3D_Wire` (175) after its type code [WD 7.8.7].
    fn wire(&mut self) -> Result<Vec<Vec<[f64; 3]>>, PrcError> {
        let pts = points(&self.base_tess_data()?)?;
        let words = self.uints("wire indices")?;
        let mut wires: Vec<Vec<[f64; 3]>> = Vec::new();
        let mut colours = 0usize;
        if words.is_empty() {
            colours = pts.len();
            if !pts.is_empty() {
                wires.push(pts.clone());
            }
        }
        let mut it = words.iter().copied();
        while let Some(header) = it.next() {
            let n = (header & WIRE_COUNT_MASK) as usize;
            let mut wire = Vec::with_capacity(n.min(words.len()));
            for _ in 0..n {
                let idx = it.next().ok_or_else(|| malformed("wire runs past its indices".into()))?;
                wire.push(point_at(&pts, idx)?);
            }
            colours = colours.saturating_add(n);
            if header & WIRE_IS_CLOSING != 0 {
                colours = colours.saturating_add(1);
                if let Some(&first) = wire.first() {
                    wire.push(first);
                }
            }
            match wires.last_mut() {
                Some(prev) if header & WIRE_IS_CONTINUOUS != 0 => prev.extend(wire),
                _ => wires.push(wire),
            }
        }
        if self.r.bit()? {
            self.vertex_colors(colours, true)?;
        }
        self.schema.skip_added_fields(TESS_3D_WIRE, &mut self.r)?;
        Ok(wires)
    }

    /// `TESS_Markup` (176) after its type code [WD 7.8.8], read past.
    fn markup(&mut self) -> Result<(), PrcError> {
        self.base_tess_data()?;
        self.uints("markup codes")?;
        let n = self.count(1, "markup strings")?;
        for _ in 0..n {
            self.r.string()?;
        }
        self.r.string()?;
        self.r.character()?;
        self.schema.skip_added_fields(TESS_MARKUP, &mut self.r)
    }

    /// `TESS_3D_Compressed` (173) after its type code [WD 7.8.9.7], read past; returns its
    /// triangle count. prc-rs: the field order and version gates follow its schema.
    ///
    /// Sizes the WD leaves unstated: the face count is the largest face index + 1,
    /// `normal_is_reversed` is one bit per triangle, and the point-reference array's
    /// compressed flag is implicit (more than three references).
    fn tess_3d_compressed(&mut self) -> Result<usize, PrcError> {
        let r = &mut self.r;
        r.bit()?; // is_calculated
        r.bit()?; // has_faces
        r.double()?; // tolerance
        if self.version >= ORIGIN_FROM {
            for _ in 0..3 {
                r.float_as_bytes()?;
            }
        }
        arrays::compressed_integer_array(r)?; // point_array
        let edge_status = arrays::character_array(r, 2, None, false)?;
        let face_of = arrays::compressed_indice_array(r, None)?;
        let t = face_of.len();
        if edge_status.len() != t && edge_status.len() != t.saturating_mul(3) {
            return Err(malformed(format!("{} edge statuses for {t} triangles", edge_status.len())));
        }
        let faces = face_of.iter().max().map_or(0, |&m| (m as usize).saturating_add(1));
        let n = r.unsigned_integer()? as usize;
        let references = arrays::bool_array(r, n)?.iter().filter(|&&b| b).count();
        let refs = arrays::compressed_indice_array(r, Some(references > 3))?;
        if refs.len() != references {
            return Err(malformed(format!("{} point references for {references} flagged points", refs.len())));
        }
        self.compressed_normals(t, faces)?;
        let r = &mut self.r;
        if r.bit()? {
            arrays::bool_array(r, faces)?; // is_point_color_on_face
            arrays::character_array(r, 8, None, false)?; // point_color_array
        }
        if r.bit()? {
            arrays::bool_array(r, faces)?; // is_multiple_line_attribute_on_face
        }
        arrays::short_array(r, 16)?; // line_attributes
        if !r.bit()? {
            self.compressed_texture_parameter()?;
            if !self.r.bit()? {
                arrays::bool_array(&mut self.r, faces)?; // face_has_texture
            }
        }
        if self.r.bit()? {
            arrays::character_array(&mut self.r, 8, None, false)?; // behaviours
        }
        self.schema.skip_added_fields(TESS_3D_COMPRESSED, &mut self.r)?;
        Ok(t)
    }

    /// A compressed tessellation's normals, from `must_recalculate_normals` on, read past.
    fn compressed_normals(&mut self, triangles: usize, faces: usize) -> Result<(), PrcError> {
        let r = &mut self.r;
        if r.bit()? {
            arrays::bool_array(r, triangles)?; // normal_is_reversed
            r.double()?; // crease_angle
            r.character()?; // normal_recalculation_flags
            return Ok(());
        }
        let angle_bits = u32::from(r.character()?);
        if angle_bits > MAX_ANGLE_BITS {
            return Err(malformed(format!("{angle_bits}-bit normal angles")));
        }
        let n = r.unsigned_integer()? as usize;
        arrays::bool_array(r, n)?; // is_binary_normal
        arrays::short_array(r, angle_bits)?; // normal angles
        arrays::bool_array(r, faces)?; // is_face_planar
        Ok(())
    }

    /// `CompressedTextureParameter` [WD 7.8.9.8-7.8.9.9; pdf-issues #729, #749], read past.
    fn compressed_texture_parameter(&mut self) -> Result<(), PrcError> {
        let words = self.count(32, "texture data")?;
        // count() bounded words * 32 by the remaining bits.
        self.r.skip_bits(words.saturating_mul(32))?;
        self.r.unsigned_integer()?; // last_integer_used_bit_number
        let n = self.count(5, "texture references")?;
        for _ in 0..n {
            self.r.nbits_then_unsigned()?;
        }
        self.r.double()?; // tolerance
        let n = self.count(32, "texture parameters")?;
        for _ in 0..n {
            self.r.float_as_bytes()?;
        }
        Ok(())
    }
}

/// The normal array and each triangle's corner normals, or both empty when any slot fails to
/// name a normal: a mesh shades from its stored normals wholly or not at all.
fn stored_normals(coords: &[f64], slots: &[[u32; 3]], triangles: usize) -> (Vec<[f64; 3]>, Vec<[u32; 3]>) {
    // Like a slot naming no normal, a partial last normal drops the stored normals.
    let Ok(normals) = points(coords) else {
        return (Vec::new(), Vec::new());
    };
    let named = |k: u32| k.is_multiple_of(3) && (k as usize / 3) < normals.len();
    if normals.is_empty() || slots.len() != triangles || !slots.iter().flatten().all(|&k| named(k)) {
        return (Vec::new(), Vec::new());
    }
    let corners = slots.iter().map(|t| t.map(|k| k / 3)).collect();
    (normals, corners)
}

/// Per texture-coordinate set, each triangle's corner pair indices.
type UvSets = Vec<Vec<Option<[u32; 3]>>>;

/// The texture coordinates as (u, v) pairs, and per set the corner slots turned into pair
/// indices (a slot is a `Double` offset, so even); a triangle with a slot naming no pair is
/// `None`.
fn stored_uvs(coords: &[f64], sets: UvSets) -> (Vec<[f64; 2]>, UvSets) {
    let uvs: Vec<[f64; 2]> = coords.as_chunks::<2>().0.to_vec();
    if uvs.is_empty() {
        return (Vec::new(), Vec::new());
    }
    let pair = |k: u32| (k.is_multiple_of(2) && (k as usize / 2) < uvs.len()).then_some(k / 2);
    let sets =
        sets.into_iter().map(|set| set.into_iter().map(|tri| tri.and_then(|[a, b, c]| Some([pair(a)?, pair(b)?, pair(c)?]))).collect()).collect();
    (uvs, sets)
}

/// `coords` as points; an array that does not divide into whole points is malformed.
fn points(coords: &[f64]) -> Result<Vec<[f64; 3]>, PrcError> {
    let (whole, rest) = coords.as_chunks::<3>();
    if !rest.is_empty() {
        return Err(malformed(format!("{} coordinates are not whole points", coords.len())));
    }
    Ok(whole.to_vec())
}

/// The point a `Double`-offset index names [WD 7.8.5.2: multiples of 3].
fn point_at(pts: &[[f64; 3]], idx: u32) -> Result<[f64; 3], PrcError> {
    if !idx.is_multiple_of(3) {
        return Err(malformed(format!("point index {idx} is not a multiple of 3")));
    }
    pts.get(idx as usize / 3).copied().ok_or_else(|| malformed(format!("point index {idx} past the coordinates")))
}

#[cfg(test)]
pub(crate) mod tests;
