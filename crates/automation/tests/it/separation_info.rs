//! Preseparated pages (ISO 32000-1 §14.11.4) through the tool table: after `page_extract` and
//! `page_delete`, each plate's `/SeparationInfo /Pages` names only pages of the saved file that
//! list it back.

use std::sync::Arc;

use pdfcraft_automation::{Automation, Content};
use pdfcraft_cos::{Document, ObjRef};
use serde_json::{Value, json};

fn tool(a: &mut Automation, name: &str, args: Value) -> Value {
    match a.call(name, &args).unwrap().remove(0) {
        Content::Json(value) => value,
        other => panic!("expected JSON: {other:?}"),
    }
}

/// Three plates of one preseparated page (pages 1–3) sharing one separation dictionary, then an
/// ordinary page.
fn plates_pdf() -> Vec<u8> {
    let objs: Vec<String> = vec![
        "<< /Type /Catalog /Pages 2 0 R >>".into(),
        "<< /Type /Pages /Kids [3 0 R 4 0 R 5 0 R 6 0 R] /Count 4 /MediaBox [0 0 200 200] >>".into(),
        "<< /Type /Page /Parent 2 0 R /SeparationInfo 7 0 R >>".into(),
        "<< /Type /Page /Parent 2 0 R /SeparationInfo 7 0 R >>".into(),
        "<< /Type /Page /Parent 2 0 R /SeparationInfo 7 0 R >>".into(),
        "<< /Type /Page /Parent 2 0 R >>".into(),
        "<< /Pages [3 0 R 4 0 R 5 0 R] /DeviceColorant /Cyan >>".into(),
    ];
    let mut out = b"%PDF-1.7\n".to_vec();
    let mut offsets = Vec::new();
    for (i, o) in objs.iter().enumerate() {
        offsets.push(out.len());
        out.extend_from_slice(format!("{} 0 obj\n{o}\nendobj\n", i + 1).as_bytes());
    }
    let xref = out.len();
    out.extend_from_slice(format!("xref\n0 {}\n0000000000 65535 f \n", objs.len() + 1).as_bytes());
    for o in offsets {
        out.extend_from_slice(format!("{o:010} 00000 n \n").as_bytes());
    }
    out.extend_from_slice(format!("trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n", objs.len() + 1).as_bytes());
    out
}

/// The saved file's page objects, in the page tree or not, and for each listed page what its set
/// names, as indices into the page tree (`None` for a page that is not in it).
fn read_sets(path: &std::path::Path) -> (usize, Vec<Option<Vec<Option<usize>>>>) {
    let doc = Document::open(Arc::new(std::fs::read(path).unwrap())).unwrap();
    let objects = doc.scan_objects().filter(|(_, o)| o.as_dict().is_some_and(|d| d.name(b"Type") == Some(b"Page"))).count();
    let tree: Vec<ObjRef> = pdfcraft_model::pages(&doc).iter().map(|p| p.obj).collect();
    let sets = tree
        .iter()
        .map(|page| {
            let info = doc.get(*page).as_dict()?.get(b"SeparationInfo").cloned()?;
            let list = doc.dict(&info)?.get(b"Pages").and_then(|p| doc.resolve(p).as_array().cloned())?;
            Some(list.iter().map(|e| e.as_ref().and_then(|r| tree.iter().position(|p| *p == r))).collect())
        })
        .collect();
    (objects, sets)
}

#[test]
fn extracted_and_remaining_plates_list_only_pages_of_their_own_file() {
    let dir = std::env::temp_dir().join(format!("pdfcraft-separation-info-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("plates.pdf"), plates_pdf()).unwrap();
    let mut a = Automation::new().with_root(&dir).unwrap();
    let doc = tool(&mut a, "doc_open", json!({ "path": "plates.pdf" }))["doc"].as_u64().unwrap();

    let ex = tool(&mut a, "page_extract", json!({ "doc": doc, "pages": [2] }))["document"]["doc"].as_u64().unwrap();
    tool(&mut a, "doc_save", json!({ "doc": ex, "path": "one.pdf" }));
    let (objects, sets) = read_sets(&dir.join("one.pdf"));
    assert_eq!(objects, 1, "the other plates stay behind");
    assert_eq!(sets, [Some(vec![Some(0)])]);

    tool(&mut a, "page_delete", json!({ "doc": doc, "pages": [1] }));
    tool(&mut a, "doc_save", json!({ "doc": doc, "path": "rest.pdf" }));
    let (_, sets) = read_sets(&dir.join("rest.pdf"));
    assert_eq!(sets, [Some(vec![Some(0), Some(1)]), Some(vec![Some(0), Some(1)]), None]);
    let _ = std::fs::remove_dir_all(&dir);
}
