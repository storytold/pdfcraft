//! ECMA-363 third edition: U3D block framing, mesh declarations and base meshes.
#[path = "primitives.rs"]
mod primitives;
use super::bitstream::Decoder;
use super::{Error, IDENTITY, Instance, MAX_INSTANCES, MAX_POSITIONS, MAX_TRIANGLES, Matrix, Mesh, NormalEncoding, Result, Scene, invalid, multiply};
use std::collections::{BTreeMap, BTreeSet};
const MAX_BYTES: usize = 64 * 1024 * 1024;
const MAX_BLOCKS: usize = 65536;
#[derive(Clone)]
struct Shading {
    attributes: u32,
    textures: usize,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum PrimitiveKind {
    Triangles,
    Points,
    Lines,
}
struct Declaration {
    kind: PrimitiveKind,
    counts: [usize; 7],
    attributes: u32,
    shading: Vec<Shading>,
    minimum: usize,
    maximum: usize,
    quant: [f64; 5],
}
struct Node {
    parents: Vec<(String, Matrix)>,
    resource: Option<String>,
    visibility: u32,
}
struct Block<'a> {
    kind: u32,
    data: &'a [u8],
    next: usize,
}
fn integer(bytes: &[u8], offset: usize) -> Result<u32> {
    let range = offset..offset.checked_add(4).ok_or_else(|| invalid("U3D offset overflows"))?;
    let bytes: [u8; 4] =
        bytes.get(range).ok_or_else(|| invalid("truncated U3D block header"))?.try_into().map_err(|_| invalid("invalid U3D block header"))?;
    Ok(u32::from_le_bytes(bytes))
}
fn align(value: usize) -> Result<usize> {
    value.checked_add(3).map(|n| n & !3).ok_or_else(|| invalid("U3D padding offset overflows"))
}
fn block(bytes: &[u8], offset: usize) -> Result<Block<'_>> {
    let kind = integer(bytes, offset)?;
    let data_size = integer(bytes, offset.checked_add(4).ok_or_else(|| invalid("U3D offset overflows"))?)? as usize;
    let metadata_size = integer(bytes, offset.checked_add(8).ok_or_else(|| invalid("U3D offset overflows"))?)? as usize;
    let start = offset.checked_add(12).ok_or_else(|| invalid("U3D offset overflows"))?;
    let end = start.checked_add(data_size).ok_or_else(|| invalid("U3D data length overflows"))?;
    let next = align(end)?.checked_add(align(metadata_size)?).ok_or_else(|| invalid("U3D metadata length overflows"))?;
    if next > bytes.len() {
        return Err(invalid("U3D block lengths exceed the enclosing block"));
    }
    Ok(Block { kind, data: bytes.get(start..end).ok_or_else(|| invalid("truncated U3D data"))?, next })
}
fn count(decoder: &mut Decoder<'_>, limit: usize) -> Result<usize> {
    let value = decoder.u32()? as usize;
    if value > limit {
        return Err(invalid("U3D object count exceeds its resource limit"));
    }
    Ok(value)
}
fn declaration(decoder: &mut Decoder<'_>, kind: PrimitiveKind) -> Result<(String, Declaration)> {
    let name = decoder.string()?;
    if name.is_empty() || decoder.u32()? != 0 {
        return Err(invalid("U3D mesh declaration has an invalid name or chain index"));
    }
    let attributes = decoder.u32()?;
    if attributes & !1 != 0 || (kind != PrimitiveKind::Triangles && attributes != 0) {
        return Err(invalid("U3D mesh has unknown geometry attributes"));
    }
    let mut counts = [0usize; 7];
    for c in &mut counts {
        *c = count(decoder, MAX_POSITIONS)?;
    }
    if counts[0] > MAX_TRIANGLES || counts[6] > 4096 {
        return Err(invalid("U3D mesh exceeds face or shading limits"));
    }
    let mut shading = Vec::with_capacity(counts[6]);
    for _ in 0..counts[6] {
        let attributes = decoder.u32()?;
        if attributes & !3 != 0 {
            return Err(invalid("U3D shading has unknown vertex attributes"));
        }
        let textures = count(decoder, 8)?;
        for _ in 0..textures {
            if !(1..=4).contains(&decoder.u32()?) {
                return Err(invalid("U3D texture coordinate dimension must be 1 to 4"));
            }
        }
        decoder.u32()?; // original shading order
        shading.push(Shading { attributes, textures });
    }
    let (minimum, maximum) =
        if kind == PrimitiveKind::Triangles { (count(decoder, MAX_POSITIONS)?, count(decoder, MAX_POSITIONS)?) } else { (0, counts[1]) };
    if minimum > maximum || maximum != counts[1] {
        return Err(invalid("U3D mesh resolution bounds disagree with its positions"));
    }
    for _ in 0..3 {
        decoder.u32()?;
    } // informational quality settings
    let mut quant = [0.0; 5];
    for value in &mut quant {
        *value = decoder.f32()?;
    }
    for _ in 0..3 {
        decoder.f32()?;
    } // normal resource parameters
    if decoder.u32()? != 0 {
        return Err(invalid("U3D bone deformation must be decoded before measuring this mesh"));
    }
    Ok((name, Declaration { kind, counts, attributes, shading, minimum, maximum, quant }))
}
#[derive(Clone)]
struct Element<const N: usize> {
    has_normals: bool,
    positions: [u32; N],
    shader: u32,
    normal: [u32; N],
    uses: [bool; 2],
    diffuse: [u32; N],
    specular: [u32; N],
    texture: Vec<[u32; N]>,
}
type Face = Element<3>;
fn element_attributes<const N: usize>(element: &Element<N>) -> super::PrimitiveAttributes<N> {
    super::PrimitiveAttributes {
        normals: element.has_normals.then_some(element.normal),
        diffuse: element.uses[0].then_some(element.diffuse),
        specular: element.uses[1].then_some(element.specular),
        textures: element.texture.clone(),
    }
}
struct MeshState {
    name: String,
    positions: Vec<super::Point>,
    normals: Vec<super::Point>,
    pools: [Vec<[f64; 4]>; 3],
    faces: Vec<Face>,
    points: Vec<Element<1>>,
    lines: Vec<Element<2>>,
    adjacency: Vec<BTreeSet<usize>>,
    last_colors: [Option<[u32; 3]>; 2],
    last_texture: Option<[u32; 3]>,
    budget: std::rc::Rc<super::DecodeBudget>,
}
impl MeshState {
    fn empty(name: String, budget: std::rc::Rc<super::DecodeBudget>) -> Self {
        Self {
            name,
            positions: Vec::new(),
            normals: Vec::new(),
            pools: Default::default(),
            faces: Vec::new(),
            points: Vec::new(),
            lines: Vec::new(),
            adjacency: Vec::new(),
            last_colors: [None; 2],
            last_texture: None,
            budget,
        }
    }
    fn spend(&mut self, work: usize) -> Result<()> {
        self.budget.spend(work)
    }
    fn add_face(&mut self, face: Face) -> Result<()> {
        if self.faces.len() >= MAX_TRIANGLES {
            return Err(invalid("U3D mesh exceeds one million triangles"));
        }
        self.budget.reserve(super::RecordKind::Face, 1, std::mem::size_of::<Face>().saturating_add(face.texture.len().saturating_mul(12)))?;
        let index = self.faces.len();
        for p in face.positions {
            self.adjacency.get_mut(p as usize).ok_or_else(|| invalid("U3D face position is out of bounds"))?.insert(index);
        }
        self.faces.push(face);
        Ok(())
    }
    fn indices_at(&mut self, position: u32, channel: usize, layer: usize) -> Result<Vec<u32>> {
        let faces = self.adjacency.get(position as usize).ok_or_else(|| invalid("U3D position is out of bounds"))?;
        let mut indices = BTreeSet::new();
        for index in faces {
            let face = self.faces.get(*index).ok_or_else(|| invalid("U3D face is out of bounds"))?;
            for (corner, p) in face.positions.iter().enumerate() {
                if *p == position {
                    let value = match channel {
                        0 => face.uses[0].then(|| face.diffuse.get(corner)).flatten(),
                        1 => face.uses[1].then(|| face.specular.get(corner)).flatten(),
                        _ => face.texture.get(layer).and_then(|a| a.get(corner)),
                    };
                    if let Some(value) = value {
                        indices.insert(*value);
                    }
                }
            }
        }
        let work = faces.len();
        self.spend(work)?;
        Ok(indices.into_iter().rev().collect())
    }
    fn move_position(&mut self, face_index: usize, split: u32, new: u32) -> Result<()> {
        let face = self.faces.get_mut(face_index).ok_or_else(|| invalid("U3D moving face is out of bounds"))?;
        for p in &mut face.positions {
            if *p == split {
                *p = new;
            }
        }
        self.adjacency.get_mut(split as usize).ok_or_else(|| invalid("U3D split position is out of bounds"))?.remove(&face_index);
        self.adjacency.get_mut(new as usize).ok_or_else(|| invalid("U3D new position is out of bounds"))?.insert(face_index);
        Ok(())
    }
    fn register_element<const N: usize>(&mut self, index: usize, element: &Element<N>) -> Result<()> {
        self.budget.reserve(
            super::RecordKind::Face,
            1,
            std::mem::size_of::<Element<N>>().saturating_add(element.texture.len().saturating_mul(N * 4)),
        )?;
        for p in element.positions {
            self.adjacency.get_mut(p as usize).ok_or_else(|| invalid("U3D primitive position is out of bounds"))?.insert(index);
        }
        Ok(())
    }
    fn geometry(self) -> Result<Mesh> {
        let texture_bytes = self
            .faces
            .iter()
            .map(|f| f.texture.len() * 12)
            .sum::<usize>()
            .saturating_add(self.points.iter().map(|f| f.texture.len() * 4).sum::<usize>())
            .saturating_add(self.lines.iter().map(|f| f.texture.len() * 8).sum::<usize>());
        let records = self.faces.len().saturating_add(self.points.len()).saturating_add(self.lines.len());
        self.budget.allocate(records.saturating_mul(std::mem::size_of::<super::FaceAttributes>() + 16).saturating_add(texture_bytes))?;
        let shading = self.faces.iter().map(|f| f.shader).collect();
        let attributes = self.faces.iter().map(element_attributes).collect();
        let point_shading = self.points.iter().map(|f| f.shader).collect();
        let point_attributes = self.points.iter().map(element_attributes).collect();
        let line_shading = self.lines.iter().map(|f| f.shader).collect();
        let line_attributes = self.lines.iter().map(element_attributes).collect();
        let [diffuse, specular, texture_coordinates] = self.pools;
        Ok(Mesh {
            name: self.name,
            positions: self.positions,
            triangles: self.faces.into_iter().map(|f| f.positions).collect(),
            points: self.points.into_iter().map(|f| f.positions[0]).collect(),
            lines: self.lines.into_iter().map(|f| f.positions).collect(),
            shading,
            attributes,
            point_shading,
            point_attributes,
            line_shading,
            line_attributes,
            normals: self.normals,
            diffuse,
            specular,
            texture_coordinates,
        })
    }
}
fn base_mesh(decoder: &mut Decoder<'_>, declarations: &BTreeMap<String, Declaration>, budget: std::rc::Rc<super::DecodeBudget>) -> Result<MeshState> {
    let name = decoder.string()?;
    if decoder.u32()? != 0 {
        return Err(invalid("U3D mesh continuation has an invalid chain index"));
    }
    let declaration = declarations.get(&name).ok_or_else(|| invalid("U3D mesh continuation precedes its declaration"))?;
    if declaration.kind != PrimitiveKind::Triangles {
        return Err(invalid("U3D triangle continuation disagrees with its generator type"));
    }
    let mut counts = [0usize; 6];
    for (value, maximum) in counts.iter_mut().zip(declaration.counts) {
        *value = count(decoder, maximum)?;
    }
    if counts[1] != declaration.minimum {
        return Err(invalid("U3D base mesh position count disagrees with its minimum resolution"));
    }
    let mut state = MeshState::empty(name, budget);
    state.budget.reserve(super::RecordKind::Position, counts[1], 24)?;
    state.budget.reserve(super::RecordKind::Normal, counts[2], 24)?;
    state.budget.reserve(super::RecordKind::Attribute, counts.iter().skip(3).sum(), 32)?;
    for _ in 0..counts[1] {
        state.positions.push(decoder.vector()?);
        state.adjacency.push(BTreeSet::new());
    }
    for _ in 0..counts[2] {
        state.normals.push(decoder.vector()?);
    }
    for (pool, n) in state.pools.iter_mut().zip(counts.iter().skip(3)) {
        for _ in 0..*n {
            pool.push([decoder.f32()?, decoder.f32()?, decoder.f32()?, decoder.f32()?]);
        }
    }
    for _ in 0..counts[0] {
        let shader = decoder.compressed(1)?;
        let shading = declaration.shading.get(shader as usize).ok_or_else(|| invalid("U3D face refers to a missing shading description"))?;
        let mut face = Face {
            has_normals: declaration.attributes & 1 == 0,
            positions: [0; 3],
            shader,
            normal: [0; 3],
            uses: [shading.attributes & 1 != 0, shading.attributes & 2 != 0],
            diffuse: [0; 3],
            specular: [0; 3],
            texture: vec![[0; 3]; shading.textures],
        };
        for corner in 0..3 {
            face.positions[corner] = decoder.index(counts[1])?;
            if declaration.attributes & 1 == 0 {
                face.normal[corner] = decoder.index(counts[2])?;
            }
            if shading.attributes & 1 != 0 {
                face.diffuse[corner] = decoder.index(counts[3])?;
            }
            if shading.attributes & 2 != 0 {
                face.specular[corner] = decoder.index(counts[4])?;
            }
            for indices in &mut face.texture {
                indices[corner] = decoder.index(counts[5])?;
            }
        }
        state.add_face(face)?;
    }
    Ok(state)
}

fn dynamic_index(decoder: &mut Decoder<'_>, context: u16, count: usize) -> Result<u32> {
    let index = decoder.compressed(context)?;
    if index as usize >= count {
        Err(Error(format!("U3D progressive index {index} is outside {count} entries in context {context}")))
    } else {
        Ok(index)
    }
}
fn new_attributes(decoder: &mut Decoder<'_>, state: &mut MeshState, split: u32, declaration: &Declaration) -> Result<[(usize, Vec<u32>); 3]> {
    let mut out: [(usize, Vec<u32>); 3] = Default::default();
    for (channel, result) in out.iter_mut().enumerate() {
        let local = if state.positions.is_empty() { Vec::new() } else { state.indices_at(split, channel, 0)? };
        let mut prediction = [0.0; 4];
        for index in &local {
            let value =
                state.pools.get(channel).and_then(|p| p.get(*index as usize)).ok_or_else(|| invalid("U3D local attribute is out of bounds"))?;
            for (p, v) in prediction.iter_mut().zip(value) {
                *p += v / local.len() as f64;
            }
        }
        let old = state.pools.get(channel).map(Vec::len).unwrap_or(0);
        let (count_context, sign_context, diff_contexts, quant) = match channel {
            0 => (3, 4, [7, 8, 9, 10], declaration.quant[3]),
            1 => (5, 6, [7, 8, 9, 10], declaration.quant[4]),
            _ => (11, 12, [13, 14, 15, 16], declaration.quant[2]),
        };
        let added = usize::from(decoder.compressed_u16(count_context)?);
        if old.saturating_add(added) > declaration.counts.get(channel + 3).copied().unwrap_or(0) {
            return Err(invalid("U3D attribute pool exceeds its declaration"));
        }
        state.budget.reserve(super::RecordKind::Attribute, added, 32)?;
        for _ in 0..added {
            let sign = decoder.compressed_u8(sign_context)?;
            if sign & !15 != 0 {
                return Err(invalid("U3D attribute difference has invalid signs"));
            }
            let mut value = prediction;
            for (component, (v, context)) in value.iter_mut().zip(diff_contexts).enumerate() {
                let difference = f64::from(decoder.compressed(context)?) * quant;
                *v += if sign & (1 << component) != 0 { -difference } else { difference };
                if !v.is_finite() || v.abs() > 1e12 {
                    return Err(invalid("U3D attribute reconstruction overflows"));
                }
            }
            state.pools.get_mut(channel).ok_or_else(|| invalid("invalid U3D attribute channel"))?.push(value);
        }
        *result = (old, local);
    }
    Ok(out)
}
fn attribute_change(decoder: &mut Decoder<'_>, state: &MeshState, channel: usize, old: usize, local: &[u32], current: u32) -> Result<u32> {
    let offset = 30 + channel as u16 * 5;
    match decoder.compressed_u8(offset)? {
        2 => Ok(current),
        1 => match decoder.compressed_u8(offset + 1)? {
            1 => {
                let count = state.pools.get(channel).map(Vec::len).unwrap_or(0).saturating_sub(old);
                Ok(dynamic_index(decoder, offset + 2, count)? + old as u32)
            }
            2 => local
                .get(dynamic_index(decoder, offset + 3, local.len())? as usize)
                .copied()
                .ok_or_else(|| invalid("U3D local change index is out of bounds")),
            3 => dynamic_index(decoder, offset + 4, state.pools.get(channel).map(Vec::len).unwrap_or(0)),
            _ => Err(invalid("U3D attribute change type must be new, local or global")),
        },
        _ => Err(invalid("U3D attribute update must keep or change")),
    }
}
fn face_attributes(
    decoder: &mut Decoder<'_>,
    state: &mut MeshState,
    split: u32,
    third: u32,
    channel: usize,
    split_local: &[u32],
) -> Result<[u32; 3]> {
    let (duplicate, index_type, index_local, index_global) = if channel == 2 { (51, 52, 53, 54) } else { (41, 42, 43, 44) };
    let flags = decoder.compressed_u8(duplicate)?;
    if flags & !7 != 0 {
        return Err(invalid("U3D attribute duplicate flags are invalid"));
    }
    let previous = if channel == 2 { state.last_texture } else { state.last_colors.get(channel).copied().flatten() };
    let mut out = [0u32; 3];
    for (corner, value) in out.iter_mut().enumerate() {
        if flags & (1 << corner) != 0 {
            *value = previous.and_then(|a| a.get(corner).copied()).ok_or_else(|| invalid("U3D attribute duplicates a missing prior face"))?;
        } else {
            let local = if corner < 2 { split_local.to_vec() } else { state.indices_at(third, channel, 0)? };
            *value = match decoder.compressed_u8(index_type)? {
                2 => *local
                    .get(dynamic_index(decoder, index_local, local.len())? as usize)
                    .ok_or_else(|| invalid("U3D local face attribute is out of bounds"))?,
                3 => dynamic_index(decoder, index_global, state.pools.get(channel).map(Vec::len).unwrap_or(0))?,
                _ => return Err(invalid("U3D face attribute type must be local or global")),
            };
        }
    }
    let _ = split;
    if channel == 2 {
        state.last_texture = Some(out);
    } else if let Some(last) = state.last_colors.get_mut(channel) {
        *last = Some(out);
    }
    Ok(out)
}
fn stay_prediction(face: &Face, split: u32, new: &[(Face, u8, u32)], prior: &BTreeMap<usize, u8>, faces: &[Face]) -> u16 {
    let Some(corner) = face.positions.iter().position(|p| *p == split) else { return 0 };
    let next = face.positions[(corner + 1) % 3];
    let previous = face.positions[(corner + 2) % 3];
    let mut direct = None;
    for (_, orientation, third) in new {
        if next == *third {
            direct = Some(if *orientation == 1 { 2 } else { 1 });
        }
        if previous == *third {
            direct = Some(if *orientation == 1 { 1 } else { 2 });
        }
    }
    if let Some(prediction) = direct {
        return prediction;
    }
    let mut adjacent = BTreeSet::new();
    for (index, prediction) in prior {
        let Some(other) = faces.get(*index) else { continue };
        let Some(corner) = other.positions.iter().position(|p| *p == split) else { continue };
        // Neighbors share an oppositely directed edge, rather than merely the
        // split vertex. This also distinguishes overlapping/nonmanifold faces.
        if (next == other.positions[(corner + 2) % 3] || previous == other.positions[(corner + 1) % 3]) && *prediction != 0 {
            adjacent.insert(if *prediction == 1 || *prediction == 3 { 3 } else { 4 });
        }
    }
    if adjacent.len() == 1 { adjacent.first().copied().unwrap_or(0) as u16 } else { 0 }
}

fn normalize(point: super::Point) -> super::Point {
    let norm = super::dot(point, point).sqrt();
    if norm < 1e-15 { [0.0; 3] } else { point.map(|v| v / norm) }
}
fn normal_predictions(state: &mut MeshState, faces: &[usize], count: usize, encoding: NormalEncoding) -> Result<Vec<super::Point>> {
    if count > faces.len() {
        return Err(invalid("U3D new normal count exceeds its incident faces"));
    }
    let mut predictions = Vec::new();
    for index in faces {
        let face = state.faces.get(*index).ok_or_else(|| invalid("U3D normal face is out of bounds"))?;
        let mut p = [[0.0; 3]; 3];
        for (p, index) in p.iter_mut().zip(face.positions) {
            *p = *state.positions.get(index as usize).ok_or_else(|| invalid("U3D normal position is out of bounds"))?;
        }
        predictions.push((normalize(super::cross(super::sub(p[1], p[0]), super::sub(p[2], p[0]))), 1.0_f64));
    }
    if encoding == NormalEncoding::ReferenceRotations {
        return reference_normal_predictions(state, &predictions, count);
    }
    while predictions.len() > count && count > 0 {
        let mut closest = (0usize, 1usize, f64::NEG_INFINITY);
        state.spend(predictions.len().saturating_mul(predictions.len()))?;
        for (i, (a, _)) in predictions.iter().enumerate() {
            for (j, (b, _)) in predictions.iter().enumerate().skip(i + 1) {
                let dot = super::dot(*a, *b);
                if dot > closest.2 {
                    closest = (i, j, dot);
                }
            }
        }
        let (a, wa) = *predictions.get(closest.0).ok_or_else(|| invalid("U3D predicted normal is missing"))?;
        let (b, wb) = *predictions.get(closest.1).ok_or_else(|| invalid("U3D predicted normal is missing"))?;
        let angle = super::dot(a, b).clamp(-1.0, 1.0).acos();
        let t = wb / (wa + wb);
        let normal = if angle.sin().abs() < 1e-12 {
            normalize([a[0] * (1.0 - t) + b[0] * t, a[1] * (1.0 - t) + b[1] * t, a[2] * (1.0 - t) + b[2] * t])
        } else {
            let u = ((1.0 - t) * angle).sin() / angle.sin();
            let v = (t * angle).sin() / angle.sin();
            [a[0] * u + b[0] * v, a[1] * u + b[1] * v, a[2] * u + b[2] * v]
        };
        if let Some(p) = predictions.get_mut(closest.0) {
            *p = (normal, wa + wb);
        }
        predictions.remove(closest.1);
    }
    Ok(predictions.into_iter().take(count).map(|p| p.0).collect())
}
/// The reference producer selects widely separated initial directions, then
/// visits face normals in order and updates the nearest cluster's weighted
/// spherical average. Keeping a seed's initial weight at zero matters: a seed
/// can move to a different cluster before its own face is visited.
/// This convention was checked through independent writer and reader APIs on
/// contributor-original meshes. The published vector-difference convention
/// remains separate above.
fn reference_normal_predictions(state: &mut MeshState, normals: &[(super::Point, f64)], count: usize) -> Result<Vec<super::Point>> {
    if count == 0 {
        return Ok(Vec::new());
    }
    let first = normals.first().ok_or_else(|| invalid("U3D normal seed is missing"))?.0;
    let mut seeds = vec![0usize];
    let mut selected = vec![false; normals.len()];
    if let Some(value) = selected.first_mut() {
        *value = true;
    }
    state.spend(normals.len())?;
    let mut similarity: Vec<_> = normals.iter().map(|(normal, _)| super::dot(*normal, first)).collect();
    while seeds.len() < count {
        state.spend(normals.len().saturating_mul(2))?;
        let mut farthest = None;
        for (index, (closest, selected)) in similarity.iter().zip(&selected).enumerate() {
            if !selected && farthest.is_none_or(|(_, best)| *closest < best) {
                farthest = Some((index, *closest));
            }
        }
        let index = farthest.ok_or_else(|| invalid("U3D normal seed count exceeds its face normals"))?.0;
        seeds.push(index);
        *selected.get_mut(index).ok_or_else(|| invalid("U3D selected normal seed is missing"))? = true;
        let direction = normals.get(index).ok_or_else(|| invalid("U3D normal seed is missing"))?.0;
        for ((closest, selected), (normal, _)) in similarity.iter_mut().zip(&selected).zip(normals) {
            if !selected {
                *closest = closest.max(super::dot(*normal, direction));
            }
        }
    }
    let mut clusters: Vec<_> = seeds
        .iter()
        .map(|index| normals.get(*index).map(|normal| (normal.0, 0.0_f64)).ok_or_else(|| invalid("U3D normal seed is missing")))
        .collect::<Result<_>>()?;
    for (normal, weight) in normals {
        state.spend(clusters.len())?;
        let mut nearest = None;
        for (index, (direction, _)) in clusters.iter().enumerate() {
            let similarity = super::dot(*normal, *direction);
            if nearest.is_none_or(|(_, best)| similarity > best) {
                nearest = Some((index, similarity));
            }
        }
        let (direction, total) = clusters
            .get_mut(nearest.ok_or_else(|| invalid("U3D normal cluster is missing"))?.0)
            .ok_or_else(|| invalid("U3D normal cluster is out of bounds"))?;
        *direction = if *total == 0.0 { *normal } else { reference_spherical_average(*direction, *normal, *weight / (*total + *weight)) };
        *total += *weight;
    }
    Ok(clusters.into_iter().map(|(direction, _)| direction).collect())
}
fn reference_spherical_average(a: super::Point, mut b: super::Point, t: f64) -> super::Point {
    let mut dot = super::dot(a, b);
    // The reference interpolation follows the shorter quaternion arc.
    if dot < 0.0 {
        b = b.map(|v| -v);
        dot = -dot;
    }
    let angle = dot.clamp(-1.0, 1.0).acos();
    let sine = angle.sin();
    if sine.abs() < 1e-12 {
        return normalize([a[0] * (1.0 - t) + b[0] * t, a[1] * (1.0 - t) + b[1] * t, a[2] * (1.0 - t) + b[2] * t]);
    }
    let u = ((1.0 - t) * angle).sin() / sine;
    let v = (t * angle).sin() / sine;
    [a[0] * u + b[0] * v, a[1] * u + b[1] * v, a[2] * u + b[2] * v]
}
fn rotate_normal(predicted: super::Point, mut vector: super::Point, signs: u8) -> Result<super::Point> {
    for (component, value) in vector.iter_mut().enumerate() {
        if signs & (2 << component) != 0 {
            *value = -*value;
        }
    }
    let squared = super::dot(vector, vector);
    if !squared.is_finite() || squared > 1.000001 {
        return Err(invalid("U3D normal rotation sine exceeds unit length"));
    }
    // The stored vector is the rotation axis multiplied by sin(angle).
    // Bit zero selects the other cosine hemisphere, including a half turn
    // with a zero stored vector. Do not treat these values as quaternion XYZ.
    let cosine = (1.0 - squared).max(0.0).sqrt() * if signs & 1 != 0 { -1.0 } else { 1.0 };
    let cross = super::cross(vector, predicted);
    super::checked_point([predicted[0] * cosine - cross[0], predicted[1] * cosine - cross[1], predicted[2] * cosine - cross[2]])
}

fn progressive_mesh(
    decoder: &mut Decoder<'_>,
    declarations: &BTreeMap<String, Declaration>,
    meshes: &mut BTreeMap<String, MeshState>,
    encoding: NormalEncoding,
    budget: std::rc::Rc<super::DecodeBudget>,
) -> Result<()> {
    let name = decoder.string()?;
    if decoder.u32()? != 0 {
        return Err(invalid("U3D progressive chain index must be zero"));
    }
    let declaration = declarations.get(&name).ok_or_else(|| invalid("U3D progressive mesh lacks a declaration"))?;
    if declaration.kind != PrimitiveKind::Triangles {
        return Err(invalid("U3D triangle continuation disagrees with its generator type"));
    }
    let start = count(decoder, declaration.maximum)?;
    let end = count(decoder, declaration.maximum)?;
    if start > end {
        return Err(invalid("U3D progressive range is reversed"));
    }
    if start == 0 && declaration.minimum == 0 && !meshes.contains_key(&name) {
        meshes.insert(name.clone(), MeshState::empty(name.clone(), budget));
    }
    let state = meshes.get_mut(&name).ok_or_else(|| invalid("U3D progressive mesh lacks its base continuation"))?;
    if state.positions.len() != start {
        return Err(invalid("U3D progressive resolution is out of sequence"));
    }
    for resolution in start..end {
        let split = if resolution == 0 {
            let value = decoder.compressed(2)?;
            if value != 0 {
                return Err(invalid("U3D initial split must be zero"));
            }
            value
        } else {
            decoder.index(resolution)?
        };
        let split_faces: Vec<usize> = state.adjacency.get(split as usize).map(|s| s.iter().rev().copied().collect()).unwrap_or_default();
        if split_faces.len() > 4096 {
            return Err(invalid("U3D split position exceeds 4096 incident faces"));
        }
        let mut neighbors = BTreeSet::new();
        for index in &split_faces {
            if let Some(face) = state.faces.get(*index) {
                neighbors.extend(face.positions);
            }
        }
        // A third corner cannot be the split corner itself; local positions are
        // the other vertices in the split neighborhood, in descending order.
        neighbors.remove(&split);
        let attributes = new_attributes(decoder, state, split, declaration)?;
        let new_count = decoder.compressed(17)? as usize;
        if new_count > declaration.counts[0].saturating_sub(state.faces.len()) {
            return Err(invalid("U3D face updates exceed their declaration"));
        }
        let mut new_faces = Vec::with_capacity(new_count);
        for _ in 0..new_count {
            let shader = decoder.compressed(1)?;
            let shading = declaration.shading.get(shader as usize).ok_or_else(|| invalid("U3D progressive face shading is out of bounds"))?;
            let orientation = decoder.compressed_u8(18)?;
            if orientation != 1 && orientation != 2 {
                return Err(invalid("U3D face orientation must be left or right"));
            }
            let third = match decoder.compressed_u8(19)? {
                1 => neighbors
                    .iter()
                    .rev()
                    .nth(dynamic_index(decoder, 20, neighbors.len())? as usize)
                    .copied()
                    .ok_or_else(|| invalid("U3D local third position is out of bounds"))?,
                2 => decoder.index(resolution)?,
                _ => return Err(invalid("U3D third position must be local or global")),
            };
            // Each new face expands the local split neighborhood before the next face.
            neighbors.insert(third);
            let positions = if orientation == 1 { [split, resolution as u32, third] } else { [resolution as u32, split, third] };
            new_faces.push((
                Face {
                    has_normals: declaration.attributes & 1 == 0,
                    positions,
                    shader,
                    normal: [0; 3],
                    uses: [shading.attributes & 1 != 0, shading.attributes & 2 != 0],
                    diffuse: [0; 3],
                    specular: [0; 3],
                    texture: vec![[0; 3]; shading.textures],
                },
                orientation,
                third,
            ));
        }
        let mut movements = BTreeMap::new();
        let mut moved = Vec::new();
        for index in &split_faces {
            state.spend(split_faces.len().saturating_add(new_faces.len()))?;
            let face = state.faces.get(*index).ok_or_else(|| invalid("U3D split face is out of bounds"))?;
            let prediction = stay_prediction(face, split, &new_faces, &movements, &state.faces);

            let movement = decoder.compressed_u8(100 + prediction)?;
            movements.insert(*index, if movement == 1 { 1 } else { 2 });
            match movement {
                0 => {}
                1 => moved.push(*index),
                value => {
                    return Err(Error(format!(
                        "U3D face stay/move value {value} is invalid at resolution {resolution}, prediction {prediction}, face {index}"
                    )));
                }
            }
        }
        let predicted = state.positions.get(split as usize).copied().unwrap_or([0.0; 3]);
        state.budget.reserve(super::RecordKind::Position, 1, 24)?;
        state.positions.push(predicted);
        state.adjacency.push(BTreeSet::new());
        for index in moved {
            let mut face = state.faces.get(index).cloned().ok_or_else(|| invalid("U3D moving face is out of bounds"))?;
            let corner = face.positions.iter().position(|p| *p == split).ok_or_else(|| invalid("U3D moving face lacks its split position"))?;
            for (channel, (old, local)) in attributes.iter().enumerate() {
                match channel {
                    0 if face.uses[0] => face.diffuse[corner] = attribute_change(decoder, state, channel, *old, local, face.diffuse[corner])?,
                    1 if face.uses[1] => face.specular[corner] = attribute_change(decoder, state, channel, *old, local, face.specular[corner])?,
                    2 => {
                        for indices in &mut face.texture {
                            indices[corner] = attribute_change(decoder, state, channel, *old, local, indices[corner])?;
                        }
                    }
                    _ => {}
                }
            }
            if let Some(original) = state.faces.get_mut(index) {
                *original = face;
            }
            state.move_position(index, split, resolution as u32)?;
        }
        for (mut face, orientation, third) in new_faces {
            for (channel, (_, local)) in attributes.iter().enumerate() {
                match channel {
                    0 if face.uses[0] => {
                        let mut values = face_attributes(decoder, state, split, third, channel, local)?;
                        if orientation == 2 {
                            values.swap(0, 1);
                        }
                        face.diffuse = values;
                    }
                    1 if face.uses[1] => {
                        let mut values = face_attributes(decoder, state, split, third, channel, local)?;
                        if orientation == 2 {
                            values.swap(0, 1);
                        }
                        face.specular = values;
                    }
                    2 => {
                        for indices in &mut face.texture {
                            let mut values = face_attributes(decoder, state, split, third, channel, local)?;
                            if orientation == 2 {
                                values.swap(0, 1);
                            }
                            *indices = values;
                        }
                    }
                    _ => {}
                }
            }
            state.add_face(face)?;
        }
        let sign = decoder.compressed_u8(21)?;
        if sign & !7 != 0 {
            return Err(invalid("U3D position difference has invalid signs"));
        }
        let mut point = predicted;
        for (component, context) in [22, 23, 24].into_iter().enumerate() {
            let delta = f64::from(decoder.compressed(context)?) * declaration.quant[0];
            point[component] += if sign & (1 << component) != 0 { -delta } else { delta };
        }
        super::checked_point(point)?;
        if let Some(p) = state.positions.get_mut(resolution) {
            *p = point;
        }
        if declaration.attributes & 1 == 0 {
            let mut neighborhood = BTreeSet::from([resolution as u32]);
            for index in state.adjacency.get(resolution).ok_or_else(|| invalid("U3D new position is missing"))? {
                if let Some(face) = state.faces.get(*index) {
                    neighborhood.extend(face.positions);
                }
            }
            for position in neighborhood.into_iter().rev() {
                let faces: Vec<usize> = state
                    .adjacency
                    .get(position as usize)
                    .ok_or_else(|| invalid("U3D normal neighborhood is invalid"))?
                    .iter()
                    .rev()
                    .copied()
                    .collect();
                // An isolated vertex has no face-normal neighborhood and no normal payload.
                // Verified against independently encoded compressed ECMA-363 fixtures.
                if faces.is_empty() {
                    continue;
                }
                let count = decoder.compressed(25)? as usize;
                if count > faces.len() {
                    return Err(Error(format!(
                        "U3D normal count {count} exceeds {} incident faces at resolution {resolution}, position {position}",
                        faces.len()
                    )));
                }
                let predictions = normal_predictions(state, &faces, count, encoding)?;
                let base = state.normals.len();
                if base.saturating_add(count) > MAX_POSITIONS {
                    return Err(invalid("U3D normal pool exceeds one million entries"));
                }
                state.budget.reserve(super::RecordKind::Normal, count, 24)?;
                for predicted in predictions {
                    let sign = decoder.compressed_u8(26)?;
                    if sign & !15 != 0 {
                        return Err(invalid("U3D normal difference has invalid signs"));
                    }
                    if encoding == NormalEncoding::VectorDifferences && sign & 8 != 0 {
                        return Err(invalid("U3D normals require reference rotation decoding"));
                    }
                    let mut difference = [0.0; 3];
                    for (delta, context) in difference.iter_mut().zip([27, 28, 29]) {
                        *delta = f64::from(decoder.compressed(context)?) * declaration.quant[1];
                    }
                    let normal = match encoding {
                        NormalEncoding::VectorDifferences => {
                            let mut normal = predicted;
                            for (component, delta) in difference.into_iter().enumerate() {
                                normal[component] += if sign & (1 << component) != 0 { -delta } else { delta };
                            }
                            normal
                        }
                        NormalEncoding::ReferenceRotations => rotate_normal(predicted, difference, sign)?,
                    };
                    super::checked_point(normal)?;
                    state.normals.push(normal);
                }
                for index in faces {
                    let selected = dynamic_index(decoder, 55, count)? + base as u32;
                    let face = state.faces.get_mut(index).ok_or_else(|| invalid("U3D normal face is missing"))?;
                    for (corner, p) in face.positions.iter().enumerate() {
                        if *p == position
                            && let Some(normal) = face.normal.get_mut(corner)
                        {
                            *normal = selected;
                        }
                    }
                }
            }
        }
    }
    Ok(())
}

fn node(decoder: &mut Decoder<'_>, model: bool) -> Result<(String, Node)> {
    let name = decoder.string()?;
    if name.is_empty() {
        return Err(invalid("U3D node name cannot be empty"));
    }
    let count = count(decoder, 64)?;
    let mut parents = Vec::with_capacity(count);
    for _ in 0..count {
        let parent = decoder.string()?;
        let mut matrix = [0.0; 16];
        for value in &mut matrix {
            *value = decoder.f32()?;
        }
        if matrix[3] != 0.0 || matrix[7] != 0.0 || matrix[11] != 0.0 || matrix[15] != 1.0 {
            return Err(invalid("U3D node transform is not affine"));
        }
        parents.push((parent, matrix));
    }
    let resource = if model { Some(decoder.string()?) } else { None };
    let visibility = if model { decoder.u32()? } else { 0 };
    if visibility > 3 {
        return Err(invalid("U3D model visibility must be 0 to 3"));
    }
    Ok((name, Node { parents, resource, visibility }))
}
fn instances(nodes: &BTreeMap<String, Node>, meshes: &[Mesh]) -> Result<Vec<Instance>> {
    let mesh_index: BTreeMap<&str, usize> = meshes.iter().enumerate().map(|(i, m)| (m.name.as_str(), i)).collect();
    let mut out = Vec::new();
    let mut expanded = 0usize;
    for (name, node) in nodes {
        let Some(resource) = &node.resource else { continue };
        let mesh = if resource.is_empty() {
            continue;
        } else {
            *mesh_index.get(resource.as_str()).ok_or_else(|| invalid("U3D model refers to a missing resource"))?
        };
        let mut stack = vec![(name.as_str(), IDENTITY, Vec::<String>::new())];
        while let Some((current, matrix, path)) = stack.pop() {
            expanded += 1;
            if expanded > 100_000 || path.len() > 64 {
                return Err(invalid("U3D assembly expansion exceeds its limit"));
            }
            if current.is_empty() {
                if out.len() >= MAX_INSTANCES {
                    return Err(invalid("U3D scene exceeds 4096 instances"));
                }
                out.push(Instance { name: name.clone(), mesh, transform: matrix, visibility: node.visibility });
                continue;
            }
            if path.iter().any(|p| p == current) {
                return Err(invalid("U3D scene has a cyclic parent hierarchy"));
            }
            let parent_node = nodes.get(current).ok_or_else(|| invalid("U3D node has a missing parent"))?;
            let mut path = path;
            path.push(current.into());
            for (parent, local) in &parent_node.parents {
                stack.push((parent, multiply(*local, matrix)?, path.clone()));
            }
        }
    }
    Ok(out)
}
pub(super) fn decode(bytes: &[u8], encoding: NormalEncoding) -> Result<Scene> {
    if bytes.len() > MAX_BYTES {
        return Err(invalid("U3D stream exceeds 64 MiB"));
    }
    let header = block(bytes, 0)?;
    if header.kind != 0x00443355 {
        return Err(invalid("3D stream does not start with a U3D file header"));
    }
    let budget = std::rc::Rc::new(super::DecodeBudget::default());
    let mut decoder = Decoder::with_budget(header.data, false, budget.clone());
    let version = decoder.u16()? as i16;
    decoder.u16()?;
    if version > 0 {
        return Err(invalid("unknown U3D major version"));
    }
    let profile = decoder.u32()?;
    if profile & !14 != 0 {
        return Err(invalid("unknown U3D profile bits"));
    }
    let declaration_size = decoder.u32()? as usize;
    let file_size = decoder.u64()?;
    if file_size != bytes.len() as u64 || declaration_size > bytes.len() || declaration_size < header.next {
        return Err(invalid("U3D file or declaration size is inconsistent"));
    }
    if decoder.u32()? != 106 {
        return Err(invalid("U3D character encoding must be UTF-8"));
    }
    let meters_per_unit = if profile & 8 != 0 {
        let scale = decoder.f64()?;
        if scale <= 0.0 {
            return Err(invalid("U3D units must be positive"));
        }
        Some(scale)
    } else {
        None
    };
    let mut declarations = BTreeMap::new();
    let mut nodes = BTreeMap::new();
    let mut meshes: BTreeMap<String, MeshState> = BTreeMap::new();
    let mut stack = vec![(bytes.get(header.next..).ok_or_else(|| invalid("truncated U3D header"))?, 0usize, None::<usize>)];
    let mut continuations = Vec::new();
    let mut total_blocks = 0usize;
    let mut positions = 0usize;
    let mut triangles = 0usize;
    while let Some((contents, depth, expected)) = stack.pop() {
        if depth > 32 {
            return Err(invalid("U3D modifier chain nesting exceeds 32"));
        }
        let mut offset = 0usize;
        let mut local_blocks = 0usize;
        while offset < contents.len() {
            total_blocks += 1;
            local_blocks += 1;
            if total_blocks > MAX_BLOCKS {
                return Err(invalid("U3D stream exceeds 65536 blocks"));
            }
            let b = block(contents, offset)?;
            offset = b.next;
            let mut decoder = Decoder::with_budget(b.data, profile & 4 != 0, budget.clone());
            match b.kind {
                0xFFFFFF14 => {
                    decoder.string()?;
                    if decoder.u32()? > 2 {
                        return Err(invalid("U3D modifier chain has an unknown type"));
                    }
                    let attributes = decoder.u32()?;
                    if attributes & !3 != 0 {
                        return Err(invalid("U3D modifier chain has unknown bounds"));
                    }
                    if attributes & 1 != 0 {
                        for _ in 0..4 {
                            decoder.f32()?;
                        }
                    }
                    if attributes & 2 != 0 {
                        for _ in 0..6 {
                            decoder.f32()?;
                        }
                    }
                    decoder.align()?;
                    let count = count(&mut decoder, MAX_BLOCKS)?;
                    let start = decoder.align()?;
                    stack.push((b.data.get(start..).ok_or_else(|| invalid("truncated U3D modifier chain"))?, depth + 1, Some(count)));
                }
                0xFFFFFF21 | 0xFFFFFF22 => {
                    let (name, node) = node(&mut decoder, b.kind == 0xFFFFFF22)?;
                    nodes.insert(name, node);
                    if nodes.len() > MAX_INSTANCES {
                        return Err(invalid("U3D scene exceeds 4096 nodes"));
                    }
                }
                0xFFFFFF31 | 0xFFFFFF36 | 0xFFFFFF37 => {
                    let kind = match b.kind {
                        0xFFFFFF36 => PrimitiveKind::Points,
                        0xFFFFFF37 => PrimitiveKind::Lines,
                        _ => PrimitiveKind::Triangles,
                    };
                    let (name, decl) = declaration(&mut decoder, kind)?;
                    declarations.insert(name, decl);
                    if declarations.len() > MAX_INSTANCES {
                        return Err(invalid("U3D scene exceeds 4096 mesh declarations"));
                    }
                }
                0xFFFFFF3B | 0xFFFFFF3C | 0xFFFFFF3E | 0xFFFFFF3F => continuations.push((b.kind, b.data)),
                0xFFFFFF12 => return Err(invalid("external U3D model references are not loaded automatically")),
                0xFFFFFF32 | 0xFFFFFF33 | 0xFFFFFF41 | 0xFFFFFF42 | 0xFFFFFF43 | 0xFFFFFF44 | 0xFFFFFF46 => {
                    return Err(invalid("U3D geometry modifier must be decoded before measuring this model"));
                }
                // These blocks affect presentation, not model-space vertex positions.
                0xFFFFFF15 | 0xFFFFFF23 | 0xFFFFFF24 | 0xFFFFFF45 | 0xFFFFFF51 | 0xFFFFFF52 | 0xFFFFFF53 | 0xFFFFFF54 | 0xFFFFFF55 | 0xFFFFFF56
                | 0xFFFFFF5C => {}
                _ => return Err(Error(format!("U3D block {:#010x} has no verified geometry interpretation", b.kind))),
            }
        }
        if expected.is_some_and(|count| count != local_blocks) {
            return Err(invalid("U3D modifier count disagrees with its blocks"));
        }
    }
    for (kind, data) in continuations {
        let mut decoder = Decoder::with_budget(data, profile & 4 != 0, budget.clone());
        if kind == 0xFFFFFF3B {
            let mesh = base_mesh(&mut decoder, &declarations, budget.clone())?;
            if meshes.insert(mesh.name.clone(), mesh).is_some() {
                return Err(invalid("U3D mesh has duplicate base continuations"));
            }
        } else if kind == 0xFFFFFF3C {
            progressive_mesh(&mut decoder, &declarations, &mut meshes, encoding, budget.clone())?;
        } else {
            let primitive = if kind == 0xFFFFFF3E { PrimitiveKind::Points } else { PrimitiveKind::Lines };
            primitives::continuation(
                &mut decoder,
                &declarations,
                &mut meshes,
                primitive,
                encoding == NormalEncoding::ReferenceRotations,
                budget.clone(),
            )?;
        }
    }
    let mut geometry = Vec::new();
    for mesh in meshes.into_values() {
        let declared = declarations.get(&mesh.name).ok_or_else(|| invalid("U3D mesh lacks a declaration"))?;
        if mesh.positions.len() != declared.maximum || mesh.faces.len() + mesh.points.len() + mesh.lines.len() != declared.counts[0] {
            return Err(invalid("U3D mesh has not reached its declared full resolution"));
        }
        positions = positions.saturating_add(mesh.positions.len());
        triangles = triangles.saturating_add(mesh.faces.len()).saturating_add(mesh.points.len()).saturating_add(mesh.lines.len());
        if positions > MAX_POSITIONS || triangles > MAX_TRIANGLES {
            return Err(invalid("U3D scene exceeds one million vertices or faces"));
        }
        if encoding == NormalEncoding::VectorDifferences {
            for index in mesh.faces.iter().flat_map(|f| f.normal) {
                if let Some(normal) = mesh.normals.get(index as usize) {
                    let length = super::dot(*normal, *normal).sqrt();
                    if (length - 1.0).abs() > 0.001_f64.max(declared.quant[1].abs() * 4.0) {
                        return Err(invalid("U3D normals require reference rotation decoding"));
                    }
                }
            }
        }
        geometry.push(mesh.geometry()?);
    }
    let meshes = geometry;
    let instances = instances(&nodes, &meshes)?;
    Ok(Scene { meshes, instances, meters_per_unit, normal_encoding: encoding })
}
#[cfg(test)]
mod tests {
    use super::*;
    fn string(bytes: &mut Vec<u8>, value: &str) {
        bytes.extend((value.len() as u16).to_le_bytes());
        bytes.extend(value.as_bytes());
    }
    fn block_bytes(kind: u32, data: Vec<u8>) -> Vec<u8> {
        let mut bytes = Vec::new();
        bytes.extend(kind.to_le_bytes());
        bytes.extend((data.len() as u32).to_le_bytes());
        bytes.extend(0u32.to_le_bytes());
        bytes.extend(data);
        while !bytes.len().is_multiple_of(4) {
            bytes.push(0);
        }
        bytes
    }
    fn model() -> Vec<u8> {
        let mut d = Vec::new();
        string(&mut d, "part");
        d.extend(0u32.to_le_bytes());
        d.extend(1u32.to_le_bytes());
        for c in [1u32, 3, 0, 0, 0, 0, 1, 0, 0, 0, 3, 3, 0, 0, 0] {
            d.extend(c.to_le_bytes());
        }
        for _ in 0..8 {
            d.extend(1.0f32.to_le_bytes());
        }
        d.extend(0u32.to_le_bytes());
        let mut payload = block_bytes(0xFFFFFF31, d);
        let mut n = Vec::new();
        string(&mut n, "visible-part");
        n.extend(1u32.to_le_bytes());
        string(&mut n, "");
        for value in IDENTITY {
            n.extend((value as f32).to_le_bytes());
        }
        string(&mut n, "part");
        n.extend(3u32.to_le_bytes());
        payload.extend(block_bytes(0xFFFFFF22, n));
        let declaration_size = payload.len() + 44;
        let mut b = Vec::new();
        string(&mut b, "part");
        b.extend(0u32.to_le_bytes());
        for c in [1u32, 3, 0, 0, 0, 0] {
            b.extend(c.to_le_bytes());
        }
        for p in [[0.0f32, 0.0, 0.0], [3.0, 0.0, 0.0], [0.0, 4.0, 0.0]] {
            for v in p {
                b.extend(v.to_le_bytes());
            }
        }
        for v in [0u32, 0, 1, 2] {
            b.extend(v.to_le_bytes());
        }
        payload.extend(block_bytes(0xFFFFFF3B, b));
        let mut header = Vec::new();
        header.extend(0u16.to_le_bytes());
        header.extend(0u16.to_le_bytes());
        header.extend(12u32.to_le_bytes());
        header.extend((declaration_size as u32).to_le_bytes());
        header.extend((payload.len() as u64 + 44).to_le_bytes());
        header.extend(106u32.to_le_bytes());
        header.extend(0.001f64.to_le_bytes());
        let mut bytes = block_bytes(0x00443355, header);
        bytes.extend(payload);
        bytes
    }
    #[test]
    fn standard_base_mesh_instances_units_and_surface_pick() {
        let bytes = model();
        let scene = decode(&bytes, NormalEncoding::VectorDifferences).unwrap();
        assert_eq!(scene.meters_per_unit, Some(0.001));
        assert_eq!(scene.meshes[0].positions[2], [0.0, 4.0, 0.0]);
        assert_eq!(scene.instances[0].name, "visible-part");
        assert_eq!(scene.pick([1.0, 1.0, 10.0], [0.0, 0.0, -1.0]).unwrap().unwrap().point, [1.0, 1.0, 0.0]);
        for end in 0..bytes.len() {
            assert!(decode(&bytes[..end], NormalEncoding::VectorDifferences).is_err());
        }
    }
    #[test]
    fn cycles_and_missing_parents_return_errors() {
        let mesh = Mesh { name: "part".into(), positions: vec![], triangles: vec![], shading: vec![], ..Mesh::default() };
        let mut nodes = BTreeMap::new();
        nodes.insert("a".into(), Node { parents: vec![("a".into(), IDENTITY)], resource: Some("part".into()), visibility: 3 });
        assert!(instances(&nodes, std::slice::from_ref(&mesh)).is_err());
        nodes.get_mut("a").unwrap().parents[0].0 = "missing".into();
        assert!(instances(&nodes, &[mesh]).is_err());
    }
}

#[cfg(test)]
mod normal_prediction_tests {
    use super::*;
    #[test]
    fn rotation_uses_sine_axis_and_signed_cosine() {
        let n = [0.0, 0.0, 1.0];
        assert_eq!(rotate_normal(n, [0.0; 3], 1).unwrap(), [0.0, 0.0, -1.0]);
        assert_eq!(rotate_normal(n, [1.0, 0.0, 0.0], 0).unwrap(), [0.0, 1.0, 0.0]);
        let sine = 3.0_f64.sqrt() / 2.0;
        let actual = rotate_normal(n, [sine, 0.0, 0.0], 0).unwrap();
        for (a, b) in actual.into_iter().zip([0.0, sine, 0.5]) {
            assert!((a - b).abs() < 1e-12);
        }
        assert!(rotate_normal(n, [1.1, 0.0, 0.0], 0).is_err());
        assert!(rotate_normal(n, [f64::NAN, 0.0, 0.0], 0).is_err());
    }
    #[test]
    fn reference_prediction_has_a_work_bound_and_preserves_coincident_seeds() {
        let mut state = MeshState::empty("bounded".into(), std::rc::Rc::new(super::super::DecodeBudget::default()));
        let directions = vec![([0.0, 0.0, 1.0], 1.0); 3];
        assert_eq!(reference_normal_predictions(&mut state, &directions, 0).unwrap(), Vec::<super::super::Point>::new());
        assert_eq!(reference_normal_predictions(&mut state, &directions, 3).unwrap(), vec![[0.0, 0.0, 1.0]; 3]);
        assert!(reference_normal_predictions(&mut state, &directions, 4).is_err());
        state.budget.work.set(19_999_999);
        assert!(reference_normal_predictions(&mut state, &directions, 2).is_err());
    }
}
