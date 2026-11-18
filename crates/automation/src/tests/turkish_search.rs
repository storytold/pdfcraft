use super::*;

const SOURCE: &str = "I ı İ i IĞDIR ığdır I\u{307}STANBUL istanbul S\u{327}is\u{327}li";

/// An imported PDF whose marked text preserves its original decomposed spelling.
/// Each word owns one painted glyph, just as a PDF can map one ligature to several letters.
fn imported_pdf() -> Vec<u8> {
    let mut body = String::new();
    for (i, word) in SOURCE.split(' ').enumerate() {
        let utf16: String = word.encode_utf16().map(|c| format!("{c:04X}")).collect();
        body.push_str(&format!("BT /F1 12 Tf 1 0 0 1 {} 150 Tm /Span <</ActualText <FEFF{utf16}>>> BDC (x) Tj EMC ET\n", 20 + i * 70));
    }
    let objects = [
        "<< /Type /Catalog /Pages 2 0 R >>".to_owned(),
        "<< /Type /Pages /Kids [3 0 R] /Count 1 /MediaBox [0 0 800 300] >>".into(),
        "<< /Type /Page /Parent 2 0 R /Resources << /Font << /F1 4 0 R >> >> /Contents 5 0 R >>".into(),
        "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>".into(),
        format!("<< /Length {} >>\nstream\n{body}\nendstream", body.len()),
    ];
    let mut out = b"%PDF-1.7\n".to_vec();
    let mut offsets = Vec::new();
    for (i, object) in objects.iter().enumerate() {
        offsets.push(out.len());
        out.extend(format!("{} 0 obj\n{object}\nendobj\n", i + 1).bytes());
    }
    let xref = out.len();
    out.extend(b"xref\n0 6\n0000000000 65535 f \n");
    for offset in offsets {
        out.extend(format!("{offset:010} 00000 n \n").bytes());
    }
    out.extend(format!("trailer\n<< /Size 6 /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n").bytes());
    out
}

#[test]
fn turkish_search_tools_keep_original_text_rectangles_bytes_and_pagination() {
    let dir = tempfile::tempdir().unwrap();
    let bytes = imported_pdf();
    std::fs::write(dir.path().join("Ayrışık Türkçe.pdf"), &bytes).unwrap();
    let mut a = Automation::new().with_root(dir.path()).unwrap();
    let id = json(&mut a, "doc_open", json!({"path":"Ayrışık Türkçe.pdf"}))["doc"].as_u64().unwrap();
    let extract = |a: &mut Automation| json(a, "text_extract", json!({"doc":id}))["pages"][0]["text"].as_str().unwrap().to_string();
    let original = extract(&mut a);
    assert_eq!(original.split_whitespace().collect::<Vec<_>>(), SOURCE.split_whitespace().collect::<Vec<_>>());
    let general = json(&mut a, "text_find", json!({"doc":id,"query":"i","whole_words":true}));
    // The legacy matcher treats a decomposed dot as a word boundary. Keep that
    // existing behavior in General; Turkish matching owns the canonical cluster.
    assert_eq!(general["count"], 5);
    assert!(general.get("language").is_none(), "legacy response shape stays unchanged");
    let mut args = json!({"doc":id,"query":"i","language":"tr","whole_words":true,"limit":1});
    let first = json(&mut a, "text_find", args.clone());
    assert_eq!(first["count"], 1);
    assert_eq!(first["complete"], false);
    assert_eq!(first["matches"][0]["text"], "İ");
    assert_eq!(first["language"], "tr");
    args["start_page"] = first["next"]["start_page"].clone();
    args["skip"] = first["next"]["skip"].clone();
    let second = json(&mut a, "text_find", args);
    assert_eq!(second["matches"][0]["text"], "i");
    assert_eq!(second["complete"], true);
    for (query, expected) in [("ığdır", 2), ("istanbul", 2), ("Şişli", 1)] {
        let found = json(&mut a, "text_find", json!({"doc":id,"query":query,"language":"tr","whole_words":true}));
        assert_eq!(found["count"], expected, "{query}: {found}");
        assert_eq!(found["warning_count"], 0);
        assert!(found["matches"][0]["rects"].as_array().is_some_and(|r| !r.is_empty()), "{found}");
    }
    let exact = json(&mut a, "text_find", json!({"doc":id,"query":"İSTANBUL","language":"tr","case_sensitive":true,"whole_words":true}));
    assert_eq!(exact["count"], 1);
    assert_eq!(exact["matches"][0]["text"], "I\u{307}STANBUL");
    assert!(a.call("text_find", &json!({"doc":id,"query":"i","language":"unknown"})).is_err());
    assert_eq!(extract(&mut a), original);
    assert_eq!(std::fs::read(dir.path().join("Ayrışık Türkçe.pdf")).unwrap(), bytes);
    assert_eq!(json(&mut a, "doc_info", json!({"doc":id}))["document"]["dirty"], false);
}
