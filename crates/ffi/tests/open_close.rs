use pdfcraft_ffi::{Engine, FfiError};

fn synthetic_pdf(pages: usize) -> Vec<u8> {
    let mut out = Vec::new();
    out.extend_from_slice(b"%PDF-1.7\n");
    let mut offsets = Vec::new();
    let mut objs: Vec<String> = Vec::new();
    objs.push("1 0 obj\n<< /Type /Catalog /Pages 2 0 R >>\nendobj\n".to_string());
    let mut kids = String::new();
    for i in 0..pages {
        kids.push_str(&format!("{} 0 R ", 3 + i * 2));
    }
    objs.push(format!("2 0 obj\n<< /Type /Pages /Kids [{kids}] /Count {pages} >>\nendobj\n"));
    for i in 0..pages {
        let page_obj_num = 3 + i * 2;
        let content_obj_num = page_obj_num + 1;
        objs.push(format!("{page_obj_num} 0 obj\n<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Contents {content_obj_num} 0 R >>\nendobj\n"));
        let stream = format!("BT /F1 24 Tf 100 700 Td (Page {}) Tj ET", i + 1);
        objs.push(format!("{content_obj_num} 0 obj\n<< /Length {} >>\nstream\n{}\nendstream\nendobj\n", stream.len(), stream));
    }
    for obj in &objs {
        offsets.push(out.len());
        out.extend_from_slice(obj.as_bytes());
    }
    let xref = out.len();
    out.extend_from_slice(format!("xref\n0 {}\n0000000000 65535 f \n", objs.len() + 1).as_bytes());
    for o in offsets {
        out.extend_from_slice(format!("{o:010} 00000 n \n").as_bytes());
    }
    out.extend_from_slice(format!("trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n", objs.len() + 1).as_bytes());
    out
}

#[test]
fn test_open_valid_bytes_and_query() {
    let engine = Engine::new();
    let pdf = synthetic_pdf(3);
    let doc_id = engine.open_bytes("test.pdf".into(), pdf, None).expect("open valid PDF");

    assert_eq!(engine.page_count(doc_id).unwrap(), 3);
    let info = engine.doc_info(doc_id).unwrap();
    assert_eq!(info.page_count, 3);
    assert!(!info.encrypted);

    let geom = engine.page_geometry(doc_id, 0).unwrap();
    assert_eq!(geom.width, 612.0);
    assert_eq!(geom.height, 792.0);

    engine.close(doc_id).expect("close document");
    assert_eq!(engine.document_count(), 0);
}

#[test]
fn test_open_empty_bytes_fails() {
    let engine = Engine::new();
    let err = engine.open_bytes("empty.pdf".into(), vec![], None).unwrap_err();
    assert!(matches!(err, FfiError::InvalidPdf { .. } | FfiError::Internal { .. }));
}

#[test]
fn test_open_truncated_bytes_fails() {
    let engine = Engine::new();
    let err = engine.open_bytes("truncated.pdf".into(), b"%PDF-1.7\n".to_vec(), None).unwrap_err();
    assert!(matches!(err, FfiError::InvalidPdf { .. }));
}

#[test]
fn test_open_file_with_sandbox() {
    let engine = Engine::new();
    let dir = tempfile::tempdir().unwrap();
    let valid_path = dir.path().join("inside.pdf");
    std::fs::write(&valid_path, synthetic_pdf(2)).unwrap();

    engine.set_sandbox_root(dir.path().to_string_lossy().into_owned()).unwrap();

    let doc_id = engine.open_file(valid_path.to_string_lossy().into_owned(), None).unwrap();
    assert_eq!(engine.page_count(doc_id).unwrap(), 2);
    engine.close(doc_id).unwrap();

    // Trying to open outside sandbox root must fail
    let outside_dir = tempfile::tempdir().unwrap();
    let outside_path = outside_dir.path().join("outside.pdf");
    std::fs::write(&outside_path, synthetic_pdf(1)).unwrap();

    let err = engine.open_file(outside_path.to_string_lossy().into_owned(), None).unwrap_err();
    assert!(matches!(err, FfiError::Io { .. }));
}

#[test]
fn test_close_nonexistent_doc_fails() {
    let engine = Engine::new();
    let err = engine.close(9999).unwrap_err();
    assert!(matches!(err, FfiError::NotFound { .. }));
}

#[test]
fn test_10k_open_close_cycles_no_leak() {
    let engine = Engine::new();
    let pdf = synthetic_pdf(1);

    for i in 0..10_000 {
        let name = format!("doc_{i}.pdf");
        let doc_id = engine.open_bytes(name, pdf.clone(), None).expect("open doc in leak loop");
        engine.close(doc_id).expect("close doc in leak loop");
    }

    assert_eq!(engine.document_count(), 0);
    assert_eq!(engine.cached_tile_count(), 0);
    assert_eq!(engine.cached_tile_bytes(), 0);
}

#[test]
fn test_boundary_fuzz_random_bytes_never_crash() {
    let engine = Engine::new();

    // 1. Extreme short lengths
    for len in 0..=32 {
        let bytes = vec![0x41; len];
        let res = engine.open_bytes("fuzz_short.pdf".into(), bytes, None);
        assert!(res.is_err(), "empty/short bytes must fail safely");
    }

    // 2. All zeroes, all 0xFF, random patterns
    let patterns = [
        vec![0x00; 1024],
        vec![0xFF; 1024],
        b"%PDF-1.7\n%EOF".to_vec(),
        b"%PDF-1.7\n1 0 obj << >> endobj\nxref\n0 2\n0000000000 65535 f \n0000000009 00000 n \ntrailer << /Size 2 /Root 1 0 R >>\nstartxref\n999999\n%%EOF".to_vec(),
    ];

    for (idx, pat) in patterns.iter().enumerate() {
        let res = engine.open_bytes(format!("fuzz_pattern_{idx}.pdf"), pat.clone(), None);
        let _ = res; // May succeed or fail safely, must NEVER panic
    }

    // 3. Mutated pseudorandom fuzz sequences
    let mut state: u64 = 0x123456789abcdef0;
    for iter in 0..200 {
        let len = ((state % 4096) + 16) as usize;
        let mut buf = Vec::with_capacity(len);
        for _ in 0..len {
            state = state.wrapping_mul(6364136223846793005).wrapping_add(1);
            buf.push((state >> 32) as u8);
        }
        // Force PDF header in half the runs
        if iter % 2 == 0 && buf.len() > 8 {
            buf[..8].copy_from_slice(b"%PDF-1.7");
        }
        let res = engine.open_bytes(format!("fuzz_rnd_{iter}.pdf"), buf, None);
        if let Ok(doc_id) = res {
            let _ = engine.page_count(doc_id);
            let _ = engine.close(doc_id);
        }
    }

    assert_eq!(engine.document_count(), 0);
}
