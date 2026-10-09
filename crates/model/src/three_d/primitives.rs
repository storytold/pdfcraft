//! ECMA-363 point and line generators. Their topology stays distinct from a surface.
use super::{Declaration, Decoder, Element, MeshState, PrimitiveKind, Result, count, invalid};
use std::collections::{BTreeMap, BTreeSet};
use std::rc::Rc;

fn indices<const N: usize>(state: &MeshState, elements: &[Element<N>], position: u32, channel: usize, layer: usize) -> Result<BTreeSet<u32>> {
    let mut result = BTreeSet::new();
    if state.positions.is_empty() {
        return Ok(result);
    }
    let adjacency = state.adjacency.get(position as usize).ok_or_else(|| invalid("U3D primitive split position is missing"))?;
    state.budget.spend(adjacency.len().saturating_mul(N))?;
    for index in adjacency {
        let element = elements.get(*index).ok_or_else(|| invalid("U3D primitive adjacency is out of bounds"))?;
        for (corner, p) in element.positions.iter().enumerate() {
            if *p != position {
                continue;
            }
            let index = match channel {
                0 if element.has_normals => element.normal.get(corner),
                1 if element.uses[0] => element.diffuse.get(corner),
                2 if element.uses[1] => element.specular.get(corner),
                3 => element.texture.get(layer).and_then(|indices| indices.get(corner)),
                _ => None,
            };
            if let Some(index) = index {
                result.insert(*index);
            }
        }
    }
    Ok(result)
}
fn used_indices(state: &MeshState, kind: PrimitiveKind, position: u32, channel: usize, layer: usize, reference: bool) -> Result<BTreeSet<u32>> {
    if reference && kind == PrimitiveKind::Points {
        let Some(element) = state.points.get(position as usize) else {
            return Ok(BTreeSet::new());
        };
        let value = match channel {
            0 if element.has_normals => Some(element.normal[0]),
            1 if element.uses[0] => Some(element.diffuse[0]),
            2 if element.uses[1] => Some(element.specular[0]),
            3 => element.texture.get(layer).map(|indices| indices[0]),
            _ => None,
        };
        return Ok(value.into_iter().collect());
    }
    match kind {
        PrimitiveKind::Points => indices(state, &state.points, position, channel, layer),
        PrimitiveKind::Lines => indices(state, &state.lines, position, channel, layer),
        PrimitiveKind::Triangles => Err(invalid("U3D point/line prediction cannot use triangle topology")),
    }
}
fn normal_prediction(state: &MeshState, kind: PrimitiveKind, split: u32, reference: bool) -> Result<super::super::Point> {
    let indices = used_indices(state, kind, split, 0, 0, reference)?;
    let mut result = [0.0; 3];
    for index in &indices {
        let normal = state.normals.get(*index as usize).ok_or_else(|| invalid("U3D primitive normal index is out of bounds"))?;
        for (value, normal) in result.iter_mut().zip(normal) {
            *value += normal / indices.len() as f64;
        }
    }
    let length = result.iter().map(|v| v * v).sum::<f64>().sqrt();
    if length > 1e-15 {
        result = result.map(|v| v / length);
    }
    Ok(result)
}
fn attribute_prediction(state: &MeshState, kind: PrimitiveKind, split: u32, channel: usize, layer: usize, reference: bool) -> Result<[f64; 4]> {
    let indices = used_indices(state, kind, split, channel + 1, layer, reference)?;
    let mut result = [0.0; 4];
    for index in &indices {
        let value = state
            .pools
            .get(channel)
            .and_then(|pool| pool.get(*index as usize))
            .ok_or_else(|| invalid("U3D primitive attribute index is out of bounds"))?;
        for (result, value) in result.iter_mut().zip(value) {
            *result += value / indices.len() as f64;
        }
    }
    if reference && !indices.is_empty() {
        if kind == PrimitiveKind::Lines {
            result = result.map(|v| v * (indices.len() * indices.len()) as f64);
        } else {
            let length = result.iter().take(3).map(|v| v * v).sum::<f64>().sqrt();
            if length > 1e-15 {
                for value in result.iter_mut().take(3) {
                    *value /= length;
                }
            }
        }
    }
    Ok(result)
}
fn attribute(decoder: &mut Decoder<'_>, state: &mut MeshState, channel: usize, predicted: [f64; 4], quant: f64) -> Result<u32> {
    let (duplicate, sign_context, differences) = match channel {
        0 => (210, 26, [214, 215, 216, 217]),
        1 => (212, 26, [214, 215, 216, 217]),
        2 => (218, 26, [220, 221, 222, 223]),
        _ => return Err(invalid("U3D primitive attribute channel is out of bounds")),
    };
    let pool = state.pools.get_mut(channel).ok_or_else(|| invalid("U3D primitive attribute pool is missing"))?;
    match decoder.compressed_u8(duplicate)? {
        1 | 2 => {
            pool.len().checked_sub(1).map(|index| index as u32).ok_or_else(|| invalid("U3D primitive duplicate attribute has no preceding value"))
        }
        0 => {
            let signs = decoder.compressed_u8(sign_context)?;
            if signs & !15 != 0 {
                return Err(invalid("U3D primitive attribute has invalid signs"));
            }
            if pool.len() >= super::super::MAX_POSITIONS {
                return Err(invalid("U3D primitive attribute pool exceeds one million entries"));
            }
            state.budget.reserve(super::super::RecordKind::Attribute, 1, 32)?;
            let mut value = predicted;
            for (component, (v, context)) in value.iter_mut().zip(differences).enumerate() {
                let delta = f64::from(decoder.compressed(context)?) * quant;
                *v += if signs & (1 << component) != 0 { -delta } else { delta };
                if !v.is_finite() || v.abs() > 1e12 {
                    return Err(invalid("U3D primitive attribute reconstruction overflows"));
                }
            }
            let index = pool.len() as u32;
            pool.push(value);
            Ok(index)
        }
        _ => Err(invalid("U3D primitive attribute duplicate flag is invalid")),
    }
}
struct ElementContext<'a> {
    declaration: &'a Declaration,
    resolution: usize,
    base: usize,
    normal_count: usize,
    colors: [[f64; 4]; 2],
    textures: &'a [[f64; 4]],
}
fn element<const N: usize>(decoder: &mut Decoder<'_>, state: &mut MeshState, context: &ElementContext<'_>) -> Result<Element<N>> {
    let ElementContext { declaration, resolution, base, normal_count, colors, textures } = context;
    let (resolution, base, normal_count) = (*resolution, *base, *normal_count);
    let shader = decoder.compressed(201)?;
    let shading = declaration.shading.get(shader as usize).ok_or_else(|| invalid("U3D primitive refers to missing shading"))?;
    let mut element = Element {
        positions: [resolution as u32; N],
        shader,
        normal: [0; N],
        has_normals: declaration.counts[2] != 0,
        uses: [shading.attributes & 1 != 0, shading.attributes & 2 != 0],
        diffuse: [0; N],
        specular: [0; N],
        texture: vec![[0; N]; shading.textures],
    };
    if N == 2 {
        if resolution == 0 {
            return Err(invalid("U3D first line endpoint has no earlier position"));
        }
        *element.positions.first_mut().ok_or_else(|| invalid("U3D line endpoint is missing"))? = decoder.index(resolution)?;
    }
    for corner in 0..N {
        let local_normal = decoder.compressed(55)?;
        if element.has_normals {
            if local_normal as usize >= normal_count {
                return Err(invalid("U3D primitive local normal index is out of bounds"));
            }
            *element.normal.get_mut(corner).ok_or_else(|| invalid("U3D primitive normal corner is missing"))? = local_normal + base as u32;
        }
        if element.uses[0] {
            *element.diffuse.get_mut(corner).ok_or_else(|| invalid("U3D primitive diffuse corner is missing"))? =
                attribute(decoder, state, 0, colors[0], declaration.quant[3])?;
        }
        if element.uses[1] {
            *element.specular.get_mut(corner).ok_or_else(|| invalid("U3D primitive specular corner is missing"))? =
                attribute(decoder, state, 1, colors[1], declaration.quant[4])?;
        }
        for (layer, indices) in element.texture.iter_mut().enumerate() {
            let predicted = *textures.get(layer).ok_or_else(|| invalid("U3D primitive texture prediction is missing"))?;
            *indices.get_mut(corner).ok_or_else(|| invalid("U3D primitive texture corner is missing"))? =
                attribute(decoder, state, 2, predicted, declaration.quant[2])?;
        }
    }
    Ok(element)
}

pub(super) fn continuation(
    decoder: &mut Decoder<'_>,
    declarations: &BTreeMap<String, Declaration>,
    meshes: &mut BTreeMap<String, MeshState>,
    kind: PrimitiveKind,
    reference: bool,
    budget: Rc<super::super::DecodeBudget>,
) -> Result<()> {
    let name = decoder.string()?;
    if decoder.u32()? != 0 {
        return Err(invalid("U3D point/line continuation has an invalid chain index"));
    }
    let declaration = declarations.get(&name).ok_or_else(|| invalid("U3D primitive continuation lacks its declaration"))?;
    if declaration.kind != kind {
        return Err(invalid("U3D primitive continuation disagrees with its generator type"));
    }
    let start = count(decoder, declaration.maximum)?;
    let end = count(decoder, declaration.maximum)?;
    if start > end {
        return Err(invalid("U3D primitive resolution range is reversed"));
    }
    if start == 0 && !meshes.contains_key(&name) {
        meshes.insert(name.clone(), MeshState::empty(name.clone(), budget));
    }
    let state = meshes.get_mut(&name).ok_or_else(|| invalid("U3D primitive continuation lacks its earlier positions"))?;
    if state.positions.len() != start {
        return Err(invalid("U3D primitive resolution is out of sequence"));
    }
    for resolution in start..end {
        let split = decoder.index(resolution.max(1))?;
        let signs = decoder.compressed_u8(21)?;
        if signs & !7 != 0 {
            return Err(invalid("U3D primitive position difference has invalid signs"));
        }
        let mut position = if resolution == 0 {
            [0.0; 3]
        } else {
            *state.positions.get(split as usize).ok_or_else(|| invalid("U3D primitive split position is missing"))?
        };
        for (component, (value, context)) in position.iter_mut().zip([22, 23, 24]).enumerate() {
            let delta = f64::from(decoder.compressed(context)?) * declaration.quant[0];
            *value += if signs & (1 << component) != 0 { -delta } else { delta };
        }
        super::super::checked_point(position)?;
        let predicted = normal_prediction(state, kind, split, reference)?;
        let colors = [attribute_prediction(state, kind, split, 0, 0, reference)?, attribute_prediction(state, kind, split, 1, 0, reference)?];
        let max_layers = declaration.shading.iter().map(|s| s.textures).max().unwrap_or(0);
        let mut textures = Vec::new();
        for layer in 0..max_layers {
            textures.push(attribute_prediction(state, kind, split, 2, layer, reference)?);
        }
        state.budget.reserve(super::super::RecordKind::Position, 1, 24)?;
        state.positions.push(position);
        state.adjacency.push(BTreeSet::new());
        let normal_count = decoder.compressed(25)? as usize;
        let base = state.normals.len();
        if base.saturating_add(normal_count) > super::super::MAX_POSITIONS {
            return Err(invalid("U3D primitive normal pool exceeds one million entries"));
        }
        state.budget.reserve(super::super::RecordKind::Normal, normal_count, 24)?;
        for _ in 0..normal_count {
            let signs = decoder.compressed_u8(26)?;
            if signs & !7 != 0 {
                return Err(invalid("U3D primitive normal difference has invalid signs"));
            }
            let mut normal = predicted;
            for (component, (value, context)) in normal.iter_mut().zip([27, 28, 29]).enumerate() {
                let delta = f64::from(decoder.compressed(context)?) * declaration.quant[1];
                *value += if signs & (1 << component) != 0 { -delta } else { delta };
            }
            state.normals.push(super::super::checked_point(normal)?);
        }
        // The public reference uses one context for both primitive counts and
        // shading IDs, so these must share their adaptive histogram.
        let added = decoder.compressed(201)? as usize;
        let current = state.points.len() + state.lines.len();
        if added > declaration.counts[0].saturating_sub(current) {
            return Err(invalid("U3D primitive count exceeds its declaration"));
        }
        let context = ElementContext { declaration, resolution, base, normal_count, colors, textures: &textures };
        for _ in 0..added {
            match kind {
                PrimitiveKind::Points => {
                    let element = element::<1>(decoder, state, &context)?;
                    state.register_element(state.points.len(), &element)?;
                    state.points.push(element);
                }
                PrimitiveKind::Lines => {
                    let element = element::<2>(decoder, state, &context)?;
                    state.register_element(state.lines.len(), &element)?;
                    state.lines.push(element);
                }
                PrimitiveKind::Triangles => return Err(invalid("U3D point/line continuation uses a triangle generator")),
            }
        }
    }
    Ok(())
}
