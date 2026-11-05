//! read_gltf over synthetic files, then through write_prc and back.

#![allow(clippy::unwrap_used, clippy::indexing_slicing)]

use super::*;
use crate::write::readback::assemble;
use crate::write_prc;

/// Little-endian bytes of `f32` positions then `u16` indices.
fn payload(positions: &[f32], indices: &[u16]) -> Vec<u8> {
    let mut b: Vec<u8> = positions.iter().flat_map(|f| f.to_le_bytes()).collect();
    b.extend(indices.iter().flat_map(|i| i.to_le_bytes()));
    while !b.len().is_multiple_of(4) {
        b.push(0);
    }
    b
}

fn glb(json: &str, bin: &[u8]) -> Vec<u8> {
    let mut j = json.as_bytes().to_vec();
    while !j.len().is_multiple_of(4) {
        j.push(b' ');
    }
    let total = 12 + 8 + j.len() + 8 + bin.len();
    let mut out = b"glTF".to_vec();
    for v in [2, total as u32, j.len() as u32, 0x4E4F_534A] {
        out.extend(v.to_le_bytes());
    }
    out.extend(&j);
    out.extend((bin.len() as u32).to_le_bytes());
    out.extend(0x004E_4942u32.to_le_bytes());
    out.extend(bin);
    out
}

/// A unit quad (two triangles, u16 indices) in buffer 0.
const QUAD: [f32; 12] = [0., 0., 0., 1., 0., 0., 1., 1., 0., 0., 1., 0.];
const QUAD_INDICES: [u16; 6] = [0, 1, 2, 0, 2, 3];

/// glass (BLEND, linear) and a masked grille over one shared quad; a
/// translated root, a strip, and a line primitive.
fn doc(buffer: &str) -> String {
    format!(
        r#"{{"asset":{{"version":"2.0"}}, "scene":0,
  "buffers":[{{"byteLength":60{buffer}}}],
  "bufferViews":[{{"buffer":0,"byteLength":48}},{{"buffer":0,"byteOffset":48,"byteLength":12}}],
  "accessors":[{{"bufferView":0,"componentType":5126,"count":4,"type":"VEC3"}},
    {{"bufferView":1,"componentType":5123,"count":6,"type":"SCALAR"}}],
  "materials":[
    {{"name":"glass","alphaMode":"BLEND","pbrMetallicRoughness":{{"baseColorFactor":[0.214041,0.214041,0.214041,0.4]}}}},
    {{"name":"grille","alphaMode":"MASK","alphaCutoff":0.6,"pbrMetallicRoughness":{{"baseColorFactor":[1,1,1,0.5]}}}}],
  "meshes":[{{"name":"pane","primitives":[{{"attributes":{{"POSITION":0}},"indices":1,"material":0}}]}},
    {{"name":"rest","primitives":[{{"attributes":{{"POSITION":0}},"mode":5,"material":1}},
      {{"attributes":{{"POSITION":0}},"mode":1}}]}}],
  "nodes":[{{"name":"frame","translation":[0,2,0],"children":[1,2]}},
    {{"name":"glass","mesh":0}},{{"name":"grille","mesh":1}}],
  "scenes":[{{"nodes":[0]}}]}}"#
    )
}

fn read(data: &[u8], options: GltfOptions) -> GltfImport {
    read_gltf(data, &mut |_| None, options).unwrap()
}

#[test]
fn a_glb_reads_names_colours_and_placements() {
    let bin = payload(&QUAD, &QUAD_INDICES);
    let import = read(&glb(&doc(""), &bin), GltfOptions::default());
    let s = &import.scene;
    let names: Vec<_> = s.nodes.iter().map(|n| n.name.as_deref().unwrap()).collect();
    assert_eq!(names, ["frame", "glass", "grille"]);
    assert_eq!(s.roots, [0]);
    // Linear 0.214 encodes to sRGB 0.5; BLEND keeps its alpha.
    let glass = s.materials[0].rgba;
    assert!((glass[0] - 0.5).abs() < 1e-5 && glass[3] == 0.4, "{glass:?}");
    // MASK at 0.5 against cutoff 0.6: invisible, disclosed.
    assert_eq!(s.materials[1].rgba[3], 0.0);
    assert_eq!(import.inferred.len(), 1);
    assert!(import.inferred[0].starts_with("grille: alpha mask"));
    // The strip unrolled to two triangles; the line skipped, noted.
    assert_eq!(s.meshes[1].primitives.len(), 1);
    assert_eq!(s.meshes[1].primitives[0].triangles, [[0, 1, 2], [2, 1, 3]]);
    assert!(import.notes.iter().any(|n| n.starts_with("mesh 1 primitive 1 skipped")));
    // Z-up: the root's +Y translation now points along +Z.
    let m = s.nodes[0].matrix.unwrap();
    assert_eq!(&m[12..15], &[0.0, 0.0, 2.0]);
}

#[test]
fn as_stored_leaves_colours_and_axes_alone() {
    let bin = payload(&QUAD, &QUAD_INDICES);
    let options = GltfOptions { colours: GltfColours::AsStored, up_axis: GltfUpAxis::AsStored };
    let s = read(&glb(&doc(""), &bin), options).scene;
    assert!((s.materials[0].rgba[0] - 0.214_041).abs() < 1e-9);
    assert_eq!(&s.nodes[0].matrix.unwrap()[12..15], &[0.0, 2.0, 0.0]);
}

#[test]
fn an_external_buffer_goes_through_the_resolver() {
    let bin = payload(&QUAD, &QUAD_INDICES);
    let text = doc(r#","uri":"mesh%20data.bin""#);
    let mut asked = Vec::new();
    let import = read_gltf(
        text.as_bytes(),
        &mut |uri: &str| {
            asked.push(uri.to_owned());
            Some(bin.clone())
        },
        GltfOptions::default(),
    )
    .unwrap();
    assert_eq!(asked, ["mesh data.bin"]);
    assert_eq!(import.scene.meshes[0].primitives[0].triangles.len(), 2);
    let missing = read_gltf(text.as_bytes(), &mut |_| None, GltfOptions::default());
    assert!(matches!(missing, Err(GltfError::Buffer { index: 0, .. })));
}

#[test]
fn the_scene_round_trips_through_prc() {
    let bin = payload(&QUAD, &QUAD_INDICES);
    let import = read(&glb(&doc(""), &bin), GltfOptions::default());
    let report = write_prc(&import.scene).unwrap();
    // The quad twice and the strip; the invisible grille is still written.
    assert_eq!(report.triangles, 4);
    let model = assemble(&report.prc);
    assert_eq!(model.triangles, 4);
    let colours: Vec<_> = model.colours.iter().flatten().copied().collect();
    assert!(colours.contains(&[127, 127, 127, 102]), "{colours:?}");
}

#[test]
fn bad_files_are_refused() {
    let v1 = br#"{"asset":{"version":"1.0"}}"#;
    assert!(matches!(read_gltf(v1, &mut |_| None, GltfOptions::default()), Err(GltfError::NotGltf(_))));
    let ext = br#"{"asset":{"version":"2.0"},"extensionsRequired":["KHR_mesh_quantization","KHR_draco_mesh_compression"]}"#;
    assert_eq!(read_gltf(ext, &mut |_| None, GltfOptions::default()), Err(GltfError::RequiredExtension("KHR_draco_mesh_compression".into())));
    assert!(matches!(read_gltf(b"\x7b", &mut |_| None, GltfOptions::default()), Err(GltfError::Json(_))));
}

#[test]
fn a_bad_primitive_is_a_note_not_an_error() {
    // Index 9 names no vertex.
    let bin = payload(&QUAD, &[0, 1, 9, 0, 2, 3]);
    let import = read(&glb(&doc(""), &bin), GltfOptions::default());
    assert!(import.scene.meshes[0].primitives.is_empty());
    assert!(import.notes.iter().any(|n| n == "mesh 0 primitive 0 skipped: index 9 names no vertex"), "{:?}", import.notes);
}

#[test]
fn no_scene_draws_every_top_level_node() {
    let text = br#"{"asset":{"version":"2.0"},"nodes":[{"children":[1]},{},{"name":"loose"}]}"#;
    let import = read(text, GltfOptions::default());
    assert_eq!(import.scene.roots, [0, 2]);
    assert!(import.notes.iter().any(|n| n.contains("names no scene")));
}

#[test]
fn a_repeated_scene_root_is_drawn_once_and_turned_once() {
    let bin = payload(&QUAD, &QUAD_INDICES);
    let text = doc("").replace(r#""scenes":[{"nodes":[0]}]"#, r#""scenes":[{"nodes":[0,0,0]}]"#);
    let import = read(&glb(&text, &bin), GltfOptions::default());
    assert_eq!(import.scene.roots, [0]);
    assert!(import.notes.iter().any(|n| n == "2 repeated scene roots drawn once"), "{:?}", import.notes);
    assert_eq!(&import.scene.nodes[0].matrix.unwrap()[12..15], &[0.0, 0.0, 2.0]);
    assert!(write_prc(&import.scene).is_ok());
}

/// A PNG header claiming 1 x 2: enough for import, not for decoding.
const PNG_HEADER: &[u8] = b"\x89PNG\r\n\x1a\n\0\0\0\x0dIHDR\0\0\0\x01\0\0\0\x02";

/// The quad with `TEXCOORD_0` (glTF's top-left origin) at byte 60 and
/// `image` in buffer view 3 at byte 92; material 0 textured through
/// `texture`, material 1 through a WebP-only texture.
fn textured_glb(image: &[u8], material: &str) -> Vec<u8> {
    let uvs: [f32; 8] = [0.5, 0.9, 0.5, 0.9, 0.5, 0.1, 0.5, 0.1];
    let mut bin = payload(&QUAD, &QUAD_INDICES);
    bin.extend(uvs.iter().flat_map(|f| f.to_le_bytes()));
    bin.extend(image);
    let json = format!(
        r#"{{"asset":{{"version":"2.0"}},"extensionsUsed":["KHR_texture_transform"],
  "buffers":[{{"byteLength":{}}}],
  "bufferViews":[{{"buffer":0,"byteLength":48}},{{"buffer":0,"byteOffset":48,"byteLength":12}},
    {{"buffer":0,"byteOffset":60,"byteLength":32}},{{"buffer":0,"byteOffset":92,"byteLength":{}}}],
  "accessors":[{{"bufferView":0,"componentType":5126,"count":4,"type":"VEC3"}},
    {{"bufferView":1,"componentType":5123,"count":6,"type":"SCALAR"}},
    {{"bufferView":2,"componentType":5126,"count":4,"type":"VEC2"}}],
  "images":[{{"bufferView":3,"mimeType":"image/png"}}],
  "samplers":[{{"wrapS":33071,"wrapT":33648}}],
  "textures":[{{"source":0,"sampler":0}},{{"extensions":{{"EXT_texture_webp":{{"source":0}}}}}}],
  "materials":[{material},
    {{"name":"webp","pbrMetallicRoughness":{{"baseColorTexture":{{"index":1}}}}}}],
  "meshes":[{{"primitives":[{{"attributes":{{"POSITION":0,"TEXCOORD_0":2}},"indices":1,"material":0}},
    {{"attributes":{{"POSITION":0}},"indices":1,"material":0}},
    {{"attributes":{{"POSITION":0,"TEXCOORD_0":2}},"indices":1,"material":1}}]}}],
  "nodes":[{{"mesh":0}}],"scenes":[{{"nodes":[0]}}]}}"#,
        bin.len(),
        image.len()
    );
    glb(&json, &bin)
}

const PLAIN: &str = r#"{"name":"decal","pbrMetallicRoughness":{"baseColorTexture":{"index":0}}}"#;

#[test]
fn a_base_colour_texture_and_its_coordinates_are_read() {
    let import = read(&textured_glb(PNG_HEADER, PLAIN), GltfOptions::default());
    let s = &import.scene;
    assert_eq!(s.textures.len(), 1);
    assert_eq!(s.textures[0].image, PNG_HEADER);
    assert_eq!(s.textures[0].wrap, [crate::TextureWrap::Clamp, crate::TextureWrap::MirroredRepeat]);
    assert!(!s.textures[0].alpha, "an opaque material ignores texture alpha");
    assert_eq!((s.materials[0].texture, s.materials[1].texture), (Some(0), None));
    let p = &s.meshes[0].primitives;
    // v flipped to the bottom-left origin.
    assert_eq!(p[0].uvs[0], [0.5, 1.0 - f64::from(0.9f32)]);
    assert!(p[1].uvs.is_empty());
    let noted = |what: &str| import.notes.iter().any(|n| n.contains(what));
    assert!(noted("mesh 0 primitive 1: texture not drawn (it has no TEXCOORD_0)"), "{:?}", import.notes);
    assert!(noted("webp: base colour texture uses an image format this reader does not read"));
}

#[test]
fn a_texture_transform_moves_the_coordinates() {
    let m = r#"{"alphaMode":"BLEND","pbrMetallicRoughness":{"baseColorTexture":{"index":0,
        "extensions":{"KHR_texture_transform":{"offset":[0.25,0],"scale":[2,1]}}}}}"#;
    let s = read(&textured_glb(PNG_HEADER, m), GltfOptions::default()).scene;
    assert!(s.textures[0].alpha, "a blended material keeps texture alpha");
    let uv = s.meshes[0].primitives[0].uvs[0];
    assert!((uv[0] - 1.25).abs() < 1e-12, "{uv:?}");
}

#[test]
fn an_unreadable_image_is_a_note() {
    let import = read(&textured_glb(b"GIF89a", PLAIN), GltfOptions::default());
    assert!(import.scene.textures.is_empty());
    assert_eq!(import.scene.materials[0].texture, None);
    assert!(
        import.notes.iter().any(|n| n == "decal: base colour texture dropped: it is not a PNG or JPEG file; drawn in its base colour"),
        "{:?}",
        import.notes
    );
}
