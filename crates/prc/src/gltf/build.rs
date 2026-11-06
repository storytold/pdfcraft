//! glTF materials, meshes, nodes and the scene as a [`MeshScene`]
//! [glTF 2.0 §3.5, §3.7, §3.9].

use super::json::{Json, JsonExt};

use super::accessor::{self, Refusal};
use super::textures::{Images, TextureUse, Textures};
use super::{GltfColours, GltfImport, GltfOptions, GltfUpAxis, member};
use crate::{MeshScene, SceneMaterial, SceneMesh, SceneNode, ScenePrimitive};

fn name(v: &Json) -> Option<String> {
    v.get("name").and_then(Json::as_str).map(str::to_owned)
}

fn index(v: &Json, key: &str) -> Option<usize> {
    v.get(key).and_then(Json::as_u32).map(|n| n as usize)
}

fn indices(v: &Json, key: &str) -> Vec<usize> {
    member(v, key).iter().filter_map(|c| c.as_u32().map(|c| c as usize)).collect()
}

fn numbers<const N: usize>(v: Option<&Json>) -> Option<[f64; N]> {
    let a = v?.as_array()?;
    if a.len() != N {
        return None;
    }
    let mut out = [0.0; N];
    for (o, j) in out.iter_mut().zip(a) {
        *o = j.as_f64()?;
    }
    Some(out)
}

/// IEC 61966-2-1 sRGB encoding of a linear component.
fn srgb(c: f64) -> f64 {
    let c = c.clamp(0.0, 1.0);
    if c <= 0.003_130_8 { 12.92 * c } else { 1.055 * c.powf(1.0 / 2.4) - 0.055 }
}

/// The scene the document names, in metres.
pub(super) fn scene(doc: &Json, buffers: &accessor::Buffers, images: &Images, options: GltfOptions) -> GltfImport {
    let mut out = GltfImport { scene: MeshScene { units_in_mm: 1000.0, ..MeshScene::default() }, notes: Vec::new(), inferred: Vec::new() };
    let uses = materials(doc, images, options, &mut out);
    meshes(doc, buffers, &uses, &mut out);
    nodes(doc, &mut out);
    roots(doc, options, &mut out);
    if !member(doc, "animations").is_empty() {
        out.notes.push("animations not imported; the scene is drawn still".into());
    }
    out
}

/// The materials, and how each draws its texture.
fn materials(doc: &Json, images: &Images, options: GltfOptions, out: &mut GltfImport) -> Vec<Option<TextureUse>> {
    let mut textures = Textures::default();
    let mut uses = Vec::new();
    for (i, m) in member(doc, "materials").iter().enumerate() {
        let pbr = m.get("pbrMetallicRoughness");
        let mut rgba = numbers::<4>(pbr.and_then(|p| p.get("baseColorFactor"))).unwrap_or([1.0; 4]);
        if options.colours == GltfColours::Linear {
            for c in rgba.iter_mut().take(3) {
                *c = srgb(*c);
            }
        }
        let label = name(m).unwrap_or_else(|| format!("material {i}"));
        let unread: Vec<&str> = ["normalTexture", "occlusionTexture", "emissiveTexture"]
            .into_iter()
            .filter(|k| m.get(k).is_some())
            .chain(pbr.and_then(|p| p.get("metallicRoughnessTexture")).map(|_| "metallicRoughnessTexture"))
            .collect();
        if !unread.is_empty() {
            out.notes.push(format!("{label}: {} not drawn; only the base colour is", unread.join(", ")));
        }
        let mode = m.get("alphaMode").and_then(Json::as_str).unwrap_or("OPAQUE");
        match mode {
            "BLEND" => {}
            "MASK" => {
                let cutoff = m.get("alphaCutoff").and_then(Json::as_f64).unwrap_or(0.5);
                let shown = rgba[3] >= cutoff;
                rgba[3] = if shown { 1.0 } else { 0.0 };
                out.inferred.push(format!(
                    "{label}: alpha mask drawn wholly {} (base alpha against cutoff {cutoff})",
                    if shown { "opaque" } else { "invisible" }
                ));
            }
            _ => rgba[3] = 1.0,
        }
        let texture = pbr.and_then(|p| p.get("baseColorTexture")).and_then(|info| {
            let used = (label.as_str(), mode == "BLEND");
            textures.base_colour(doc, images, info, used, out)
        });
        if texture.is_some() && mode == "MASK" {
            out.inferred.push(format!("{label}: texture alpha not cut out; the texture drawn opaque"));
        }
        uses.push(texture.map(|(_, u)| u));
        out.scene.materials.push(SceneMaterial { name: name(m), rgba, texture: texture.map(|(k, _)| k) });
    }
    uses
}

fn meshes(doc: &Json, buffers: &accessor::Buffers, uses: &[Option<TextureUse>], out: &mut GltfImport) {
    let default_material = out.scene.materials.len();
    let mut used_default = false;
    let mut coloured = 0usize;
    for (m, mesh) in member(doc, "meshes").iter().enumerate() {
        let mut primitives = Vec::new();
        for (p, prim) in member(mesh, "primitives").iter().enumerate() {
            match primitive(doc, buffers, prim) {
                Ok(mut sp) => {
                    if sp.material.is_none() && prim.get("material").is_some() {
                        out.notes.push(format!("mesh {m} primitive {p}: its material does not exist; drawn in the default white"));
                    }
                    coloured += usize::from(prim.get("attributes").and_then(|a| a.get("COLOR_0")).is_some());
                    texture_coordinates(doc, buffers, prim, uses, &mut sp, (m, p), out);
                    if sp.material.is_none() {
                        sp.material = Some(default_material);
                        used_default = true;
                    }
                    primitives.push(sp);
                }
                Err(why) => out.notes.push(format!("mesh {m} primitive {p} skipped: {why}")),
            }
        }
        out.scene.meshes.push(SceneMesh { name: name(mesh), primitives });
    }
    if coloured > 0 {
        out.notes.push(format!("the vertex colours (COLOR_0) of {coloured} primitives not drawn"));
    }
    if used_default {
        // The glTF default material: opaque white [§3.9.6].
        out.scene.materials.push(SceneMaterial { name: None, rgba: [1.0; 4], texture: None });
    }
}

/// `sp`'s coordinates for its material's texture, if it has one; a
/// primitive without them is drawn in the base colour, with a note.
fn texture_coordinates(
    doc: &Json,
    buffers: &accessor::Buffers,
    prim: &Json,
    uses: &[Option<TextureUse>],
    sp: &mut ScenePrimitive,
    (m, p): (usize, usize),
    out: &mut GltfImport,
) {
    let Some(&Some(using)) = sp.material.and_then(|k| uses.get(k)) else {
        return;
    };
    let Some(attrs) = prim.get("attributes") else {
        return;
    };
    match super::textures::coordinates(doc, buffers, attrs, using, sp.positions.len()) {
        Ok(uvs) => sp.uvs = uvs,
        Err(why) => out.notes.push(format!("mesh {m} primitive {p}: texture not drawn ({why}); drawn in its base colour")),
    }
}

fn vec3s(flat: &[f64]) -> Vec<[f64; 3]> {
    flat.as_chunks::<3>().0.to_vec()
}

/// One primitive as triangles; strips and fans are unrolled [§3.7.2.1].
fn primitive(doc: &Json, buffers: &accessor::Buffers, prim: &Json) -> Result<ScenePrimitive, Refusal> {
    let mode = prim.get("mode").and_then(Json::as_u32).unwrap_or(4);
    if !(4..=6).contains(&mode) {
        return Err(format!("mode {mode} draws points or lines, not triangles"));
    }
    let attrs = prim.get("attributes");
    let at = |k| attrs.and_then(|a| index(a, k));
    let position = at("POSITION").ok_or("it has no POSITION")?;
    let positions = vec3s(&accessor::read(doc, buffers, position, 3)?);
    let normals = match at("NORMAL") {
        Some(n) => vec3s(&accessor::read(doc, buffers, n, 3)?),
        None => Vec::new(),
    };
    if !normals.is_empty() && normals.len() != positions.len() {
        return Err("its NORMAL and POSITION counts differ".into());
    }
    let corners = match index(prim, "indices") {
        Some(i) => accessor::indices(doc, buffers, i)?,
        None => (0..u32::try_from(positions.len()).map_err(|_| "it is too large")?).collect(),
    };
    if let Some(&v) = corners.iter().find(|&&v| v as usize >= positions.len()) {
        return Err(format!("index {v} names no vertex"));
    }
    Ok(ScenePrimitive {
        positions,
        normals,
        triangles: triangles(&corners, mode),
        material: index(prim, "material").filter(|&m| m < member(doc, "materials").len()),
        uvs: Vec::new(),
    })
}

fn triangles(c: &[u32], mode: u32) -> Vec<[u32; 3]> {
    let get = |i: usize| c.get(i).copied().unwrap_or(0);
    match mode {
        5 => (0..c.len().saturating_sub(2))
            .map(|i| match i % 2 {
                0 => [get(i), get(i + 1), get(i + 2)],
                _ => [get(i + 1), get(i), get(i + 2)],
            })
            .collect(),
        6 => (1..c.len().saturating_sub(1)).map(|i| [get(0), get(i), get(i + 1)]).collect(),
        _ => c.as_chunks::<3>().0.to_vec(),
    }
}

/// `T × R × S` as a column-major matrix; `q` is the unit quaternion
/// (x, y, z, w) [§3.5.3].
fn trs(t: [f64; 3], q: [f64; 4], s: [f64; 3]) -> [f64; 16] {
    let [x, y, z, w] = q;
    // The rotation's columns.
    let r = [
        [1.0 - 2.0 * (y * y + z * z), 2.0 * (x * y + z * w), 2.0 * (x * z - y * w)],
        [2.0 * (x * y - z * w), 1.0 - 2.0 * (x * x + z * z), 2.0 * (y * z + x * w)],
        [2.0 * (x * z + y * w), 2.0 * (y * z - x * w), 1.0 - 2.0 * (x * x + y * y)],
    ];
    let mut m = IDENTITY;
    for ((col, axis), k) in m.as_chunks_mut::<4>().0.iter_mut().zip(r).zip(s) {
        for (d, v) in col.iter_mut().zip(axis) {
            *d = v * k;
        }
    }
    for (d, v) in m.iter_mut().skip(12).zip(t) {
        *d = v;
    }
    m
}

fn nodes(doc: &Json, out: &mut GltfImport) {
    let meshes = member(doc, "meshes");
    let morphed = |k: usize| meshes.get(k).is_some_and(|mesh| member(mesh, "primitives").iter().any(|p| p.get("targets").is_some()));
    let (mut skinned, mut missing) = (0, 0);
    for n in member(doc, "nodes") {
        let matrix = numbers::<16>(n.get("matrix")).or_else(|| {
            let (t, r, s) = (n.get("translation"), n.get("rotation"), n.get("scale"));
            (t.is_some() || r.is_some() || s.is_some())
                .then(|| trs(numbers(t).unwrap_or([0.0; 3]), numbers(r).unwrap_or([0.0, 0.0, 0.0, 1.0]), numbers(s).unwrap_or([1.0; 3])))
        });
        let mesh = index(n, "mesh").filter(|&m| m < out.scene.meshes.len());
        skinned += usize::from(n.get("skin").is_some() || n.get("weights").is_some() || mesh.is_some_and(morphed));
        missing += usize::from(mesh.is_none() && n.get("mesh").is_some());
        out.scene.nodes.push(SceneNode { name: name(n), matrix, mesh, children: indices(n, "children") });
    }
    if skinned > 0 {
        out.notes.push(format!("{skinned} skinned or morphed nodes drawn in their rest pose"));
    }
    if missing > 0 {
        out.notes.push(format!("{missing} nodes name a mesh that does not exist; drawn without one"));
    }
}

/// The default scene's roots, or every parentless node when the file
/// names no scene [§3.4]; turned Z-up when asked.
fn roots(doc: &Json, options: GltfOptions, out: &mut GltfImport) {
    let scenes = member(doc, "scenes");
    let chosen = index(doc, "scene").unwrap_or(0);
    out.scene.roots = match scenes.get(chosen) {
        Some(s) => indices(s, "nodes"),
        None => {
            let mut parented = vec![false; out.scene.nodes.len()];
            for &c in out.scene.nodes.iter().flat_map(|n| &n.children) {
                if let Some(p) = parented.get_mut(c) {
                    *p = true;
                }
            }
            if !out.scene.nodes.is_empty() {
                out.notes.push("the file names no scene; every top-level node is drawn".into());
            }
            (0..parented.len()).filter(|&i| parented.get(i) == Some(&false)).collect()
        }
    };
    let mut seen = vec![false; out.scene.nodes.len()];
    let listed = out.scene.roots.len();
    out.scene.roots.retain(|&r| seen.get_mut(r).is_none_or(|s| !std::mem::replace(s, true)));
    if out.scene.roots.len() < listed {
        out.notes.push(format!("{} repeated scene roots drawn once", listed - out.scene.roots.len()));
    }
    if scenes.len() > 1 {
        out.notes.push(format!("scene {chosen} of {} drawn", scenes.len()));
    }
    if options.up_axis == GltfUpAxis::ZUp {
        for &r in &out.scene.roots {
            if let Some(n) = out.scene.nodes.get_mut(r) {
                n.matrix = Some(z_up(n.matrix.unwrap_or(IDENTITY)));
            }
        }
    }
}

const IDENTITY: [f64; 16] = [1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0];

/// `C × m` where `C` turns glTF's +Y up into +Z up: (x, y, z) → (x, −z, y).
fn z_up(mut m: [f64; 16]) -> [f64; 16] {
    for [_, y, z, _] in m.as_chunks_mut::<4>().0.iter_mut() {
        (*y, *z) = (-*z, *y);
    }
    m
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::indexing_slicing)]
mod tests {
    use super::*;

    #[test]
    fn srgb_encoding_matches_the_standard() {
        assert_eq!(srgb(0.0), 0.0);
        assert!((srgb(1.0) - 1.0).abs() < 1e-12);
        assert!((srgb(0.214_041_14) - 0.5).abs() < 1e-6);
        assert!((srgb(0.002) - 0.025_84).abs() < 1e-12);
    }

    #[test]
    fn strips_and_fans_unroll_with_consistent_winding() {
        assert_eq!(triangles(&[0, 1, 2, 3], 5), [[0, 1, 2], [2, 1, 3]]);
        assert_eq!(triangles(&[0, 1, 2, 3], 6), [[0, 1, 2], [0, 2, 3]]);
        assert_eq!(triangles(&[0, 1, 2, 3], 4), [[0, 1, 2]]);
    }

    #[test]
    fn a_quarter_turn_quaternion_is_a_rotation_matrix() {
        let h = std::f64::consts::FRAC_1_SQRT_2;
        // 90° about z (x → y), x scaled by 2.
        let m = trs([1.0, 2.0, 3.0], [0.0, 0.0, h, h], [2.0, 1.0, 1.0]);
        assert!(m[0].abs() < 1e-12 && (m[1] - 2.0).abs() < 1e-12 && m[2].abs() < 1e-12);
        assert!((m[4] + 1.0).abs() < 1e-12 && m[5].abs() < 1e-12);
        assert_eq!(&m[12..16], &[1.0, 2.0, 3.0, 1.0]);
        let up = z_up(IDENTITY);
        assert_eq!(&up[4..7], &[0.0, 0.0, 1.0]);
        assert_eq!(&up[8..11], &[0.0, -1.0, 0.0]);
    }
}
