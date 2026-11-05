//! `model_add` embeds a U3D or PRC model, or a glTF scene converted to PRC, as a 3D annotation
//! (ISO 32000-2 §13.6.2) with a poster, end to end: the tool, the saved file, undo, and the refusals.

use pdfcraft_automation::{Automation, Content, ToolError};
use pdfcraft_cos::{Dict, Document, Object, Stream};
use serde_json::{Value, json};

fn tool(a: &mut Automation, name: &str, args: Value) -> Value {
    match a.call(name, &args).unwrap().remove(0) {
        Content::Json(value) => value,
        other => panic!("expected JSON: {other:?}"),
    }
}

fn png(w: u32, h: u32) -> Vec<u8> {
    let mut out = Vec::new();
    let mut encoder = png::Encoder::new(&mut out, w, h);
    encoder.set_color(png::ColorType::Rgb);
    encoder.set_depth(png::BitDepth::Eight);
    encoder.write_header().unwrap().write_image_data(&vec![90; (w * h * 3) as usize]).unwrap();
    out
}

fn stream(doc: &Document, obj: &Object) -> Stream {
    match doc.resolve(obj).as_ref() {
        Object::Stream(s) => s.clone(),
        other => panic!("expected a stream: {other:?}"),
    }
}

/// The 3D annotations on the first page of the saved file.
fn saved_3d(dir: &std::path::Path) -> (Document, Vec<Dict>) {
    let doc = Document::open(std::sync::Arc::new(std::fs::read(dir.join("out.pdf")).unwrap())).unwrap();
    let page = &pdfcraft_model::pages(&doc)[0];
    let list = page.dict.get(b"Annots").map(|a| doc.resolve(a).as_array().cloned().unwrap()).unwrap_or_default();
    let found = list.iter().map(|o| doc.resolve(o).as_dict().unwrap().clone()).filter(|d| d.name(b"Subtype") == Some(&b"3D"[..])).collect();
    (doc, found)
}

#[test]
fn model_add_embeds_the_model_with_a_fitted_poster() {
    let dir = tempfile::tempdir().unwrap();
    let mut prc = b"PRC".to_vec();
    prc.extend_from_slice(&[7; 200]);
    std::fs::write(dir.path().join("bracket.prc"), &prc).unwrap();
    std::fs::write(dir.path().join("poster.png"), png(80, 40)).unwrap();
    let mut a = Automation::new().with_root(dir.path()).unwrap();
    let doc = tool(&mut a, "doc_create", json!({ "from": "blank", "width": 600, "height": 800 }))["doc"].as_u64().unwrap();
    // A 300 × 300 box from the top-left of the page: user space [100, 400, 400, 700].
    let call =
        json!({ "doc": doc, "page": 1, "path": "bracket.prc", "rect": [100, 100, 400, 400], "poster": "poster.png", "activation": "page_open" });
    tool(&mut a, "model_add", call);
    tool(&mut a, "doc_save", json!({ "doc": doc, "path": "out.pdf" }));

    let (saved, found) = saved_3d(dir.path());
    let [annot] = found.as_slice() else { panic!("one 3D annotation: {found:?}") };
    let rect: Vec<f64> = annot.get(b"Rect").unwrap().as_array().unwrap().iter().filter_map(Object::as_f64).collect();
    assert_eq!(rect, [100.0, 400.0, 400.0, 700.0]);
    assert_eq!(annot.get(b"Contents").and_then(Object::as_string).map(|s| s.to_text()).as_deref(), Some("bracket"));
    let model = stream(&saved, annot.get(b"3DD").unwrap());
    assert_eq!(model.dict.name(b"Type"), Some(&b"3D"[..]));
    assert_eq!(model.dict.name(b"Subtype"), Some(&b"PRC"[..]));
    assert_eq!(model.dict.name(b"Filter"), Some(&b"FlateDecode"[..]));
    assert_eq!(model.decoded().unwrap().to_vec(), prc, "the model is stored unchanged");
    let activation = saved.resolve(annot.get(b"3DA").unwrap()).as_dict().unwrap().clone();
    assert_eq!(activation.name(b"A"), Some(&b"PO"[..]));
    // The 80 × 40 picture fitted in the 300 × 300 box: 300 × 150, centred top to bottom.
    let ap = saved.resolve(annot.get(b"AP").unwrap()).as_dict().unwrap().clone();
    let poster = stream(&saved, ap.get(b"N").unwrap());
    let content = String::from_utf8(poster.decoded().unwrap().to_vec()).unwrap();
    assert!(content.contains("300 0 0 150 100 475 cm /Pic Do"), "{content}");

    assert_eq!(tool(&mut a, "edit_undo", json!({ "doc": doc }))["undone"], "Add 3D model");
    tool(&mut a, "doc_save", json!({ "doc": doc, "path": "out.pdf" }));
    assert!(saved_3d(dir.path()).1.is_empty(), "undo removes it");
}

#[test]
fn model_add_without_a_poster_draws_a_labelled_frame_and_activates_on_click() {
    let dir = tempfile::tempdir().unwrap();
    let mut u3d = b"U3D\0".to_vec();
    u3d.extend_from_slice(&[0; 60]);
    std::fs::write(dir.path().join("part.u3d"), &u3d).unwrap();
    let mut a = Automation::new().with_root(dir.path()).unwrap();
    let doc = tool(&mut a, "doc_create", json!({ "from": "blank", "width": 600, "height": 800 }))["doc"].as_u64().unwrap();
    tool(&mut a, "model_add", json!({ "doc": doc, "page": 1, "path": "part.u3d", "rect": [50, 50, 250, 200], "name": "Part" }));
    tool(&mut a, "doc_save", json!({ "doc": doc, "path": "out.pdf" }));

    let (saved, found) = saved_3d(dir.path());
    let [annot] = found.as_slice() else { panic!("one 3D annotation: {found:?}") };
    assert_eq!(stream(&saved, annot.get(b"3DD").unwrap()).dict.name(b"Subtype"), Some(&b"U3D"[..]));
    let activation = saved.resolve(annot.get(b"3DA").unwrap()).as_dict().unwrap().clone();
    assert_eq!(activation.name(b"A"), Some(&b"XA"[..]), "the standard's default, stated");
    let ap = saved.resolve(annot.get(b"AP").unwrap()).as_dict().unwrap().clone();
    let content = String::from_utf8(stream(&saved, ap.get(b"N").unwrap()).decoded().unwrap().to_vec()).unwrap();
    assert!(content.contains("re S") && content.contains("(3D model) Tj"), "{content}");
}

#[test]
fn model_add_refuses_what_is_not_a_model_and_adds_nothing() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("notes.txt"), b"PR not a model").unwrap();
    std::fs::write(dir.path().join("part.prc"), b"PRC....").unwrap();
    let mut a = Automation::new().with_root(dir.path()).unwrap();
    let doc = tool(&mut a, "doc_create", json!({ "from": "blank", "width": 600, "height": 800 }))["doc"].as_u64().unwrap();
    let not_a_model = a.call("model_add", &json!({ "doc": doc, "page": 1, "path": "notes.txt", "rect": [10, 10, 100, 100] }));
    assert!(matches!(&not_a_model, Err(ToolError::Failed(m)) if m.contains("U3D") && m.contains("PRC")), "{not_a_model:?}");
    for (args, why) in [
        (json!({ "rect": [10, 10, 10, 100] }), "zero width"),
        (json!({ "rect": [0, 0, 20000, 100] }), "wider than a PDF page can be"),
        (json!({}), "no rect"),
        (json!({ "rect": [10, 10, 100, 100], "activation": "now" }), "an unknown activation"),
    ] {
        let mut call = json!({ "doc": doc, "page": 1, "path": "part.prc" });
        call.as_object_mut().unwrap().extend(args.as_object().unwrap().clone());
        assert!(matches!(a.call("model_add", &call), Err(ToolError::InvalidArgs(_))), "{why}");
    }
    tool(&mut a, "doc_save", json!({ "doc": doc, "path": "out.pdf" }));
    assert!(saved_3d(dir.path()).1.is_empty());
}

/// One triangle's three corners as little-endian `f32`s, and a glTF document drawing it from
/// buffer `uri` (`None`: the GLB binary chunk).
fn triangle_gltf(uri: Option<&str>) -> (Vec<u8>, String) {
    let bin: Vec<u8> = [0f32, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0, 0.0].iter().flat_map(|v| v.to_le_bytes()).collect();
    let buffer = match uri {
        Some(uri) => json!({ "byteLength": 36, "uri": uri }),
        None => json!({ "byteLength": 36 }),
    };
    let doc = json!({
        "asset": { "version": "2.0" },
        "buffers": [buffer],
        "bufferViews": [{ "buffer": 0, "byteLength": 36 }],
        "accessors": [{ "bufferView": 0, "componentType": 5126, "count": 3, "type": "VEC3", "min": [0, 0, 0], "max": [1, 1, 0] }],
        "meshes": [{ "name": "tri", "primitives": [{ "attributes": { "POSITION": 0 } }] }],
        "nodes": [{ "mesh": 0 }],
        "scenes": [{ "nodes": [0] }],
        "scene": 0
    });
    (bin, doc.to_string())
}

/// A GLB container holding `json` and `bin`.
fn glb(json: &str, bin: &[u8]) -> Vec<u8> {
    let mut json = json.as_bytes().to_vec();
    json.resize(json.len().next_multiple_of(4), b' ');
    let total = 12 + 8 + json.len() + 8 + bin.len();
    let mut out = b"glTF".to_vec();
    for v in [2, total as u32, json.len() as u32, 0x4E4F_534A] {
        out.extend(u32::to_le_bytes(v));
    }
    out.extend(&json);
    out.extend(u32::to_le_bytes(bin.len() as u32));
    out.extend(u32::to_le_bytes(0x004E_4942));
    out.extend(bin);
    out
}

/// The PRC stream of the one 3D annotation in the saved file, parsed.
fn saved_prc(dir: &std::path::Path) -> pdfcraft_prc::PrcFile {
    let (saved, found) = saved_3d(dir);
    let [annot] = found.as_slice() else { panic!("one 3D annotation: {found:?}") };
    let model = stream(&saved, annot.get(b"3DD").unwrap());
    assert_eq!(model.dict.name(b"Subtype"), Some(&b"PRC"[..]));
    pdfcraft_prc::PrcFile::parse(&model.decoded().unwrap()).unwrap()
}

#[test]
fn model_add_converts_a_glb_to_prc() {
    let dir = tempfile::tempdir().unwrap();
    let (bin, json) = triangle_gltf(None);
    std::fs::write(dir.path().join("tri.glb"), glb(&json, &bin)).unwrap();
    let mut a = Automation::new().with_root(dir.path()).unwrap();
    let doc = tool(&mut a, "doc_create", json!({ "from": "blank", "width": 600, "height": 800 }))["doc"].as_u64().unwrap();
    let out = tool(&mut a, "model_add", json!({ "doc": doc, "page": 1, "path": "tri.glb", "rect": [100, 100, 400, 400] }));
    assert_eq!(out["converted"]["from"], "glTF", "{out}");
    assert_eq!(out["converted"]["triangles"], 1, "{out}");
    tool(&mut a, "doc_save", json!({ "doc": doc, "path": "out.pdf" }));
    let prc = saved_prc(dir.path());
    assert_eq!(prc.placements().unwrap().len(), 1, "the triangle's node is placed");
}

#[test]
fn model_add_reads_a_gltf_buffer_beside_it_and_nothing_outside_the_root() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("root");
    std::fs::create_dir_all(root.join("parts/sub")).unwrap();
    let (bin, beside) = triangle_gltf(Some("sub/tri%20data.bin"));
    std::fs::write(root.join("parts/sub/tri data.bin"), &bin).unwrap();
    std::fs::write(root.join("parts/tri.gltf"), beside).unwrap();
    std::fs::write(dir.path().join("outside.bin"), &bin).unwrap();
    let outside = dir.path().join("outside.bin").to_string_lossy().replace('\\', "/");
    let mut a = Automation::new().with_root(&root).unwrap();
    let doc = tool(&mut a, "doc_create", json!({ "from": "blank", "width": 600, "height": 800 }))["doc"].as_u64().unwrap();
    let out = tool(&mut a, "model_add", json!({ "doc": doc, "page": 1, "path": "parts/tri.gltf", "rect": [10, 10, 200, 200] }));
    assert_eq!(out["converted"]["triangles"], 1, "{out}");

    for (n, uri) in
        ["../../outside.bin", "%2e%2e/%2e%2e/outside.bin", "C:outside.bin", outside.as_str(), "file:///outside.bin"].into_iter().enumerate()
    {
        let name = format!("parts/escape{n}.gltf");
        std::fs::write(root.join(&name), triangle_gltf(Some(uri)).1).unwrap();
        let refused = a.call("model_add", &json!({ "doc": doc, "page": 1, "path": name, "rect": [10, 10, 200, 200] }));
        assert!(matches!(&refused, Err(ToolError::Failed(m)) if m.contains("buffer 0")), "{uri}: {refused:?}");
    }
    tool(&mut a, "doc_save", json!({ "doc": doc, "path": "out.pdf" }));
    assert_eq!(saved_3d(&root).1.len(), 1, "only the model whose buffer is inside the root");
}

#[test]
fn model_add_refuses_a_broken_gltf_and_adds_nothing() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("broken.gltf"), b"{ \"asset\": ").unwrap();
    let (_, json) = triangle_gltf(None);
    std::fs::write(dir.path().join("no-chunk.glb"), glb(&json, &[])).unwrap();
    let mut a = Automation::new().with_root(dir.path()).unwrap();
    let doc = tool(&mut a, "doc_create", json!({ "from": "blank", "width": 600, "height": 800 }))["doc"].as_u64().unwrap();
    for name in ["broken.gltf", "no-chunk.glb"] {
        let refused = a.call("model_add", &json!({ "doc": doc, "page": 1, "path": name, "rect": [10, 10, 200, 200] }));
        assert!(matches!(&refused, Err(ToolError::Failed(m)) if m.contains(name)), "{name}: {refused:?}");
    }
    tool(&mut a, "doc_save", json!({ "doc": doc, "path": "out.pdf" }));
    assert!(saved_3d(dir.path()).1.is_empty());
}
