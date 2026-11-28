//! `pdfa_verify` checks PDF/A-4 (ISO 19005-4) files against PDF/A-4, by default whenever the file
//! declares it, and `pdfa_convert` refuses to make PDF/A-4 files.

use pdfcraft_automation::{Automation, Content};
use serde_json::{Value, json};

fn tool(a: &mut Automation, name: &str, args: Value) -> Value {
    match a.call(name, &args).unwrap().remove(0) {
        Content::Json(value) => value,
        other => panic!("expected JSON: {other:?}"),
    }
}

/// A one-page PDF 2.0 file whose metadata declares PDF/A-4 with `conformance`, or PDF/A-2b when
/// `part` is 2.
fn declaring(part: u8, conformance: &str) -> Vec<u8> {
    let identification = match part {
        4 => format!("<pdfaid:part>4</pdfaid:part><pdfaid:rev>2020</pdfaid:rev>{conformance}"),
        _ => "<pdfaid:part>2</pdfaid:part><pdfaid:conformance>B</pdfaid:conformance>".to_owned(),
    };
    let packet = format!(
        "<?xpacket begin=\"\u{feff}\" id=\"W5M0MpCehiHzreSzNTczkc9d\"?><x:xmpmeta xmlns:x=\"adobe:ns:meta/\">\
<rdf:RDF xmlns:rdf=\"http://www.w3.org/1999/02/22-rdf-syntax-ns#\"><rdf:Description rdf:about=\"\" \
xmlns:pdfaid=\"http://www.aiim.org/pdfa/ns/id/\">{identification}</rdf:Description></rdf:RDF></x:xmpmeta><?xpacket end=\"w\"?>"
    );
    let objects = [
        "<< /Type /Catalog /Pages 2 0 R /Metadata 3 0 R >>".to_owned(),
        "<< /Type /Pages /Kids [4 0 R] /Count 1 >>".to_owned(),
        format!("<< /Type /Metadata /Subtype /XML /Length {} >>\nstream\n{packet}\nendstream", packet.len()),
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 200 200] /Resources << >> >>".to_owned(),
    ];
    let mut out = b"%PDF-2.0\n%\xE2\xE3\xCF\xD3\n".to_vec();
    let mut offsets = Vec::new();
    for (i, object) in objects.iter().enumerate() {
        offsets.push(out.len());
        out.extend_from_slice(format!("{} 0 obj\n{object}\nendobj\n", i + 1).as_bytes());
    }
    let xref = out.len();
    out.extend_from_slice(format!("xref\n0 {}\n0000000000 65535 f \n", objects.len() + 1).as_bytes());
    for offset in offsets {
        out.extend_from_slice(format!("{offset:010} 00000 n \n").as_bytes());
    }
    let id = "/ID [<00112233445566778899AABBCCDDEEFF> <00112233445566778899AABBCCDDEEFF>]";
    out.extend_from_slice(format!("trailer\n<< /Size {} /Root 1 0 R {id} >>\nstartxref\n{xref}\n%%EOF\n", objects.len() + 1).as_bytes());
    out
}

#[test]
fn pdfa_verify_checks_a_pdfa4_file_against_pdfa4() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("a4.pdf"), declaring(4, "")).unwrap();
    std::fs::write(dir.path().join("a4e.pdf"), declaring(4, "<pdfaid:conformance>E</pdfaid:conformance>")).unwrap();
    std::fs::write(dir.path().join("a2b.pdf"), declaring(2, "")).unwrap();
    let mut a = Automation::new().with_root(dir.path()).unwrap();
    let open = |a: &mut Automation, path: &str| tool(a, "doc_open", json!({ "path": path }))["doc"].as_u64().unwrap();

    // Without a level, the one the file declares.
    let doc = open(&mut a, "a4.pdf");
    let r = tool(&mut a, "pdfa_verify", json!({ "doc": doc }));
    assert_eq!(r["level"], "PDF/A-4", "{r}");
    assert_eq!(r["compliant"], true, "{r}");
    assert_eq!(r["declared"]["pdfa"], "PDF/A-4", "{r}");
    let doc_e = open(&mut a, "a4e.pdf");
    let r = tool(&mut a, "pdfa_verify", json!({ "doc": doc_e }));
    assert_eq!((&r["level"], &r["compliant"]), (&json!("PDF/A-4e"), &json!(true)), "{r}");

    // A level asked for is the one checked: PDF/A-2b rules find the PDF 2.0 header.
    let r = tool(&mut a, "pdfa_verify", json!({ "doc": doc, "level": "2b" }));
    assert_eq!(r["compliant"], false, "{r}");
    assert!(r["issues"].as_array().unwrap().iter().any(|i| i["clause"] == "6.1.2"), "{r}");
    // And a PDF/A-2b file checked as 4f breaks PDF/A-4's rules, none of them fixable here.
    let doc2 = open(&mut a, "a2b.pdf");
    let r = tool(&mut a, "pdfa_verify", json!({ "doc": doc2 }));
    assert_eq!(r["level"], "PDF/A-2b", "{r}");
    let r = tool(&mut a, "pdfa_verify", json!({ "doc": doc2, "level": "4f" }));
    let issues = r["issues"].as_array().unwrap();
    assert!(issues.iter().any(|i| i["clause"] == "6.7.3") && issues.iter().any(|i| i["clause"] == "6.9"), "{r}");
    assert!(issues.iter().all(|i| i["fixable"] == false), "{r}");

    // Save as PDF/A doesn't make PDF/A-4 files.
    assert!(a.call("pdfa_convert", &json!({ "doc": doc, "level": "4" })).is_err());
    assert!(a.call("pdfa_verify", &json!({ "doc": doc, "level": "4b" })).is_err());
}
