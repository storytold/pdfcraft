//! Which options a list box has selected, through the tool table (ISO 32000-2 Table 234, `/I`).

use pdfcraft_automation::{Automation, Content};
use serde_json::{Value, json};

fn tool(a: &mut Automation, name: &str, args: Value) -> Value {
    match a.call(name, &args).unwrap().remove(0) {
        Content::Json(value) => value,
        other => panic!("expected JSON: {other:?}"),
    }
}

/// A PDF of the given objects (numbered from 1).
fn pdf_of(objs: &[&str]) -> Vec<u8> {
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

fn open(test: &str) -> (Automation, u64, std::path::PathBuf) {
    let dir = std::env::temp_dir().join(format!("pdfcraft-list-selection-{test}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let bytes = pdf_of(&[
        "<< /Type /Catalog /Pages 2 0 R /AcroForm << /Fields [4 0 R 6 0 R] /NeedAppearances true /DA (/Helv 10 Tf 0 g) /DR << /Font << /Helv 5 0 R >> >> >> >>",
        "<< /Type /Pages /Kids [3 0 R] /Count 1 /MediaBox [0 0 400 400] >>",
        "<< /Type /Page /Parent 2 0 R /Annots [4 0 R 6 0 R] >>",
        "<< /Type /Annot /Subtype /Widget /FT /Ch /T (D_dup) /Opt [[(1) (One-A)] [(1) (One-B)] [(2) (Two)]] /V (1) /I [1] /Rect [50 250 200 320] /P 3 0 R /F 4 >>",
        "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica /Encoding /WinAnsiEncoding >>",
        "<< /Type /Annot /Subtype /Widget /FT /Ch /T (H_twovals) /Opt [[(TW) (Taiwan)] [(JP) (Japan)] [(US) (United States)]] /V [(TW) (JP)] /Rect [50 150 200 220] /P 3 0 R /F 4 >>",
    ]);
    std::fs::write(dir.join("choice.pdf"), bytes).unwrap();
    let mut a = Automation::new().with_root(&dir).unwrap();
    let id = tool(&mut a, "doc_open", json!({ "path": "choice.pdf" }))["doc"].as_u64().unwrap();
    (a, id, dir)
}

fn field(a: &mut Automation, doc: u64, name: &str) -> Value {
    let all = tool(a, "form_fields", json!({ "doc": doc }));
    all["fields"].as_array().unwrap().iter().find(|f| f["name"] == name).unwrap().clone()
}

/// Two options share the export value `1`: `selected` says which one `/I` picks, and filling the
/// field with an option's label picks that option.
#[test]
fn form_fields_reports_which_of_two_options_with_one_export_value_is_selected() {
    let (mut a, doc, dir) = open("dup");
    let d = field(&mut a, doc, "D_dup");
    assert_eq!(d["value"], "1");
    assert_eq!(d["selected"], json!([2]), "One-B, the second option: {d}");
    tool(&mut a, "form_fill", json!({ "doc": doc, "values": { "D_dup": "One-A" } }));
    let d = field(&mut a, doc, "D_dup");
    assert_eq!((d["value"].clone(), d["selected"].clone()), (json!("1"), json!([1])), "{d}");
    tool(&mut a, "form_fill", json!({ "doc": doc, "values": { "D_dup": "One-B" } }));
    let d = field(&mut a, doc, "D_dup");
    assert_eq!(d["selected"], json!([2]));
    let _ = std::fs::remove_dir_all(&dir);
}

/// A list box without MultiSelect given `/V [(TW) (JP)]` reports Taiwan alone (ISO 32000-2
/// Table 233: "at most one item shall be selected"), and filling it writes one value.
#[test]
fn form_fields_reports_one_value_for_a_single_select_list_given_two() {
    let (mut a, doc, dir) = open("twovals");
    let h = field(&mut a, doc, "H_twovals");
    assert_eq!((h["value"].clone(), h["selected"].clone()), (json!("TW"), json!([1])), "{h}");
    tool(&mut a, "form_fill", json!({ "doc": doc, "values": { "H_twovals": "United States" } }));
    let h = field(&mut a, doc, "H_twovals");
    assert_eq!((h["value"].clone(), h["selected"].clone()), (json!("US"), json!([3])), "{h}");
    let _ = std::fs::remove_dir_all(&dir);
}
