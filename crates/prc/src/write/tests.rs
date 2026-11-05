//! write_prc → reader round trips.

#![allow(clippy::unwrap_used, clippy::indexing_slicing)]

use super::readback::assemble;
use super::*;
use crate::PrcFile;

fn tri(material: Option<usize>, dx: f64) -> ScenePrimitive {
    ScenePrimitive {
        positions: vec![[dx, 0.0, 0.0], [dx + 1.0, 0.0, 0.0], [dx, 1.0, 0.0]],
        triangles: vec![[0, 1, 2]],
        material,
        ..ScenePrimitive::default()
    }
}

fn node(name: &str, mesh: Option<usize>, children: Vec<usize>) -> SceneNode {
    SceneNode { name: Some(name.into()), mesh, children, ..SceneNode::default() }
}

/// A frame over a glass pane and a shared bolt placed twice.
fn scene() -> MeshScene {
    let mut bolt2 = node("bolt-2", Some(1), vec![]);
    let mut m = [0.0; 16];
    for (i, v) in [1.0, 1.0, 1.0, 1.0].into_iter().enumerate() {
        m[i * 5] = v;
    }
    m[12] = 10.0;
    bolt2.matrix = Some(m);
    MeshScene {
        nodes: vec![node("frame", None, vec![1, 2, 3]), node("glass", Some(0), vec![]), node("bolt-1", Some(1), vec![]), bolt2],
        roots: vec![0],
        meshes: vec![
            SceneMesh { name: Some("pane".into()), primitives: vec![tri(Some(0), 0.0)] },
            SceneMesh { name: Some("bolt".into()), primitives: vec![tri(Some(1), 5.0), tri(None, 7.0)] },
        ],
        materials: vec![
            SceneMaterial { name: Some("glass".into()), rgba: [0.2, 0.4, 0.6, 0.4], texture: None },
            SceneMaterial { name: Some("steel".into()), rgba: [1.0, 0.0, 0.0, 1.0], texture: None },
        ],
        textures: vec![],
        units_in_mm: 1000.0,
    }
}

#[test]
fn triangles_colours_and_alpha_read_back() {
    let r = write_prc(&scene()).unwrap();
    assert_eq!((r.triangles, r.flat_normals, r.empty_primitives), (3, 3, 0));
    let m = assemble(&r.prc);
    // The glass once, the bolt's two primitives at both placements.
    assert_eq!(m.triangles, 5);
    let mut colours: Vec<_> = m.colours.iter().flatten().copied().collect();
    colours.sort_unstable();
    assert!(colours.contains(&[51, 102, 153, 102]), "{colours:?}");
    assert_eq!(colours.iter().filter(|c| **c == [255, 0, 0, 255]).count(), 2);
}

#[test]
fn node_names_reach_the_model_tree() {
    let r = write_prc(&scene()).unwrap();
    let tree = PrcFile::parse(&r.prc).unwrap().model_tree().unwrap();
    let names: Vec<_> = tree.iter().map(|n| n.name.as_deref()).collect();
    assert_eq!(names, [Some("frame"), Some("glass"), Some("bolt-1"), Some("bolt-2")]);
    assert_eq!(tree[3].parent, Some(0));
}

#[test]
fn a_translation_moves_the_second_placement() {
    let m = assemble(&write_prc(&scene()).unwrap().prc);
    let xs: Vec<f64> = m.meshes.iter().flat_map(|t| t.positions.iter().map(|p| p[0])).collect();
    // bolt-1's first primitive starts at x = 5; bolt-2's at 15.
    assert!(xs.contains(&5.0) && xs.contains(&15.0), "{xs:?}");
}

#[test]
fn a_non_uniform_scale_is_a_general_transformation() {
    let mut s = scene();
    let mut m = [0.0; 16];
    (m[0], m[5], m[10], m[15]) = (2.0, 1.0, 1.0, 1.0);
    s.nodes[3].matrix = Some(m);
    let a = assemble(&write_prc(&s).unwrap().prc);
    let xs: Vec<f64> = a.meshes.iter().flat_map(|t| t.positions.iter().map(|p| p[0])).collect();
    assert!(xs.contains(&10.0) && xs.contains(&12.0), "{xs:?}");
}

#[test]
fn several_roots_get_one_synthetic_root() {
    let mut s = scene();
    s.nodes[0].children = vec![1];
    s.roots = vec![0, 2, 3];
    let tree = PrcFile::parse(&write_prc(&s).unwrap().prc).unwrap().model_tree().unwrap();
    assert_eq!(tree.len(), 5);
    assert_eq!(tree[0].name, None);
    assert!(tree[1..].iter().all(|n| n.parent.is_some()));
}

#[test]
fn writing_is_deterministic() {
    assert_eq!(write_prc(&scene()).unwrap().prc, write_prc(&scene()).unwrap().prc);
}

#[test]
fn invalid_scenes_are_refused() {
    let mut s = scene();
    s.nodes[0].children.push(1);
    s.nodes[2].children.push(1);
    assert!(matches!(write_prc(&s), Err(PrcWriteError::NotATree(_))));
    let mut s = scene();
    s.roots = vec![1];
    s.nodes[0].children.clear();
    s.meshes[0].primitives[0].triangles[0] = [0, 1, 9];
    assert_eq!(write_prc(&s), Err(PrcWriteError::BadIndex { what: "vertex", index: 9 }));
    let mut s = scene();
    s.meshes[0].primitives[0].positions[0][1] = f64::NAN;
    assert!(matches!(write_prc(&s), Err(PrcWriteError::BadNumber(_))));
    let mut s = scene();
    s.units_in_mm = 0.0;
    assert!(matches!(write_prc(&s), Err(PrcWriteError::BadNumber(_))));
    let mut s = scene();
    s.meshes.iter_mut().flat_map(|m| &mut m.primitives).for_each(|p| p.triangles.clear());
    assert_eq!(write_prc(&s), Err(PrcWriteError::Empty));
    assert_eq!(write_prc(&MeshScene::default()), Err(PrcWriteError::BadNumber("units_in_mm")));
}

#[test]
fn a_cycle_is_refused_not_looped() {
    let mut s = scene();
    s.nodes[1].children.push(0);
    assert!(write_prc(&s).is_err());
}

#[test]
fn a_mirrored_rotation_reads_back() {
    // 90° about z, then mirror z, scale 2, translate y by 3: x' = -2y,
    // y' = 2x + 3.
    let mut s = scene();
    let mut m = [0.0; 16];
    (m[1], m[4], m[10], m[13], m[15]) = (2.0, -2.0, -2.0, 3.0, 1.0);
    s.nodes[3].matrix = Some(m);
    let a = assemble(&write_prc(&s).unwrap().prc);
    let pts: Vec<[f64; 3]> = a.meshes.iter().flat_map(|t| t.positions.clone()).collect();
    // bolt's first vertex (5,0,0) → (0,13,0); (5,1,0) → (-2,13,0).
    let near = |q: [f64; 3]| pts.iter().any(|p| (0..3).all(|i| (p[i] - q[i]).abs() < 1e-9));
    assert!(near([0.0, 13.0, 0.0]) && near([-2.0, 13.0, 0.0]), "{pts:?}");
}

/// Acrobat reads occurrences in one forward pass and drops a son written
/// after its father, which shows as an empty grey 3D window.
#[test]
fn every_son_precedes_its_father_and_the_root_is_last() {
    for roots in [vec![0], vec![0, 2, 3]] {
        let mut s = scene();
        if roots.len() > 1 {
            s.nodes[0].children = vec![1];
        }
        s.roots = roots;
        let occ = validate::plan(&s).unwrap().occurrences;
        for (i, o) in occ.iter().enumerate() {
            assert!(o.sons.iter().all(|&son| (son as usize) < i), "{i}");
        }
        let root = occ.len() - 1;
        let fathered = |k: usize| occ.iter().any(|o| o.sons.contains(&(k as u32)));
        assert!(!fathered(root));
        assert!((0..root).all(fathered), "every other occurrence has a father");
    }
}

#[test]
fn textures_are_checked_before_writing() {
    let mut s = scene();
    s.materials[1].texture = Some(0);
    assert_eq!(write_prc(&s), Err(PrcWriteError::BadIndex { what: "texture", index: 0 }));
    s.textures = vec![SceneTexture { image: b"GIF89a".to_vec(), ..SceneTexture::default() }];
    assert_eq!(write_prc(&s), Err(PrcWriteError::BadTexture { index: 0 }));
    let mut s = scene();
    s.meshes[0].primitives[0].uvs = vec![[0.0, 0.0]];
    assert!(matches!(write_prc(&s), Err(PrcWriteError::BadIndex { .. })));
    // Coordinates under an untextured material are not written.
    let mut s = scene();
    s.meshes[0].primitives[0].uvs = vec![[0.0, 0.0]; 3];
    assert_eq!(write_prc(&s).unwrap().textured, 0);
}

#[test]
fn texture_coordinates_read_back() {
    let mut s = scene();
    // A PNG signature and header claiming 1 x 2: enough for the writer.
    let image = b"\x89PNG\r\n\x1a\n\0\0\0\x0dIHDR\0\0\0\x01\0\0\0\x02".to_vec();
    s.textures = vec![SceneTexture { image: image.clone(), wrap: [TextureWrap::Clamp, TextureWrap::MirroredRepeat], alpha: false }];
    s.materials[1].texture = Some(0);
    s.meshes[1].primitives[0].uvs = vec![[0.5, 0.1], [0.5, 0.1], [0.5, 0.9]];
    let r = write_prc(&s).unwrap();
    assert_eq!(r.textured, 1);
    let file = PrcFile::parse(&r.prc).unwrap();
    let pictures: Vec<&Vec<u8>> = file.file_structures.iter().flat_map(|fs| &fs.pictures).collect();
    assert_eq!(pictures, [&image], "the picture is stored as it is");
    let m = assemble(&r.prc);
    let with_uvs: Vec<_> = m.meshes.iter().filter(|t| !t.uvs.is_empty()).collect();
    assert_eq!(with_uvs.len(), 2, "the bolt's primitive at both placements");
    assert!(with_uvs.iter().all(|t| t.uvs == [[0.5, 0.1], [0.5, 0.1], [0.5, 0.9]]));
}
