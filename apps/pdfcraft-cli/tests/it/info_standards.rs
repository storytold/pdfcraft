//! `info` reports the standard a document declares, as the Standards panel does (#3319). A file
//! carrying `pdfaid:part` and `pdfaid:conformance` in its XMP metadata was summarised with no
//! mention of PDF/A at all, though the app reads the same declaration and shows it.

use std::process::Command;

const BIN: &str = env!("CARGO_BIN_EXE_pdfcraft-cli");

/// A one-page PDF, with `metadata` as the catalog's XMP stream when given.
fn fixture(metadata: Option<&str>) -> Vec<u8> {
    let content = b"0 0 0 rg 10 10 40 20 re f";
    let catalog = match metadata {
        Some(_) => "<< /Type /Catalog /Pages 2 0 R /Metadata 5 0 R >>".to_string(),
        None => "<< /Type /Catalog /Pages 2 0 R >>".to_string(),
    };
    let mut objs: Vec<Vec<u8>> = vec![
        catalog.into_bytes(),
        b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_vec(),
        b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 120 60] /Contents 4 0 R >>".to_vec(),
        format!("<< /Length {} >> stream\n", content.len()).into_bytes(),
    ];
    objs[3].extend_from_slice(content);
    objs[3].extend_from_slice(b"\nendstream");
    if let Some(xmp) = metadata {
        let mut stream = format!("<< /Type /Metadata /Subtype /XML /Length {} >> stream\n", xmp.len()).into_bytes();
        stream.extend_from_slice(xmp.as_bytes());
        stream.extend_from_slice(b"\nendstream");
        objs.push(stream);
    }
    let mut out = b"%PDF-1.7\n".to_vec();
    let mut offsets = Vec::new();
    for (i, o) in objs.iter().enumerate() {
        offsets.push(out.len());
        out.extend_from_slice(format!("{} 0 obj\n", i + 1).as_bytes());
        out.extend_from_slice(o);
        out.extend_from_slice(b"\nendobj\n");
    }
    let xref = out.len();
    out.extend_from_slice(format!("xref\n0 {}\n0000000000 65535 f \n", objs.len() + 1).as_bytes());
    for off in &offsets {
        out.extend_from_slice(format!("{off:010} 00000 n \n").as_bytes());
    }
    out.extend_from_slice(format!("trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n", objs.len() + 1).as_bytes());
    out
}

fn xmp(part: &str, conformance: &str) -> String {
    format!(
        "<?xpacket begin=\"\" id=\"W5M0MpCehiHzreSzNTczkc9d\"?>\n\
<x:xmpmeta xmlns:x=\"adobe:ns:meta/\">\n <rdf:RDF xmlns:rdf=\"http://www.w3.org/1999/02/22-rdf-syntax-ns#\">\n\
  <rdf:Description rdf:about=\"\" xmlns:pdfaid=\"http://www.aiim.org/pdfa/ns/id/\">\n\
   <pdfaid:part>{part}</pdfaid:part>\n   <pdfaid:conformance>{conformance}</pdfaid:conformance>\n\
  </rdf:Description>\n </rdf:RDF>\n</x:xmpmeta>\n<?xpacket end=\"w\"?>"
    )
}

fn info_of(bytes: &[u8], name: &str) -> serde_json::Value {
    let dir = std::env::temp_dir().join(format!("pdfcraft-info-standards-{}-{name}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("f.pdf");
    std::fs::write(&path, bytes).unwrap();
    let out = Command::new(BIN).args(["info", path.to_str().unwrap()]).output().unwrap();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let json = serde_json::from_slice(&out.stdout).expect("info prints JSON");
    let _ = std::fs::remove_dir_all(&dir);
    json
}

#[test]
fn info_names_the_declared_pdfa_level() {
    let declared = info_of(&fixture(Some(&xmp("2", "B"))), "pdfa2b");
    assert_eq!(declared["pdfa"], serde_json::json!("PDF/A-2b"), "{declared}");
    assert_eq!(declared["pdfua"], serde_json::Value::Null);
    assert_eq!(declared["output_intents"], serde_json::json!([]));
    // Lower-case conformance in the file still reads as the level's own spelling.
    assert_eq!(info_of(&fixture(Some(&xmp("3", "a"))), "pdfa3a")["pdfa"], serde_json::json!("PDF/A-3a"));
}

#[test]
fn info_declares_nothing_for_a_plain_pdf() {
    let plain = info_of(&fixture(None), "plain");
    assert_eq!(plain["pdfa"], serde_json::Value::Null, "{plain}");
    assert_eq!(plain["pdfua"], serde_json::Value::Null);
    // The rest of the summary is unaffected.
    assert_eq!(plain["pages"], serde_json::json!(1));
    assert_eq!(plain["pdf_version"], serde_json::json!("1.7"));
}
