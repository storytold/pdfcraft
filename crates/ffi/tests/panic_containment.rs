use pdfcraft_ffi::{Engine, FfiError, catch_panic};

#[test]
fn test_catch_panic_boundary() {
    let result: Result<(), FfiError> = catch_panic(|| {
        panic!("simulated panic across FFI boundary");
    });

    match result {
        Err(FfiError::Internal { reason }) => {
            assert!(reason.contains("simulated panic across FFI boundary"));
        }
        _ => panic!("expected FfiError::Internal from panic"),
    }
}

#[test]
fn test_engine_remains_usable_after_recovered_panic() {
    let engine = Engine::new();

    // Trigger an error on an invalid doc_id
    let err = engine.doc_info(9999).unwrap_err();
    assert!(matches!(err, FfiError::NotFound { .. }));

    // Verify engine can still open documents and function normally
    let mut out = Vec::new();
    out.extend_from_slice(b"%PDF-1.7\n");
    let obj1 = "1 0 obj\n<< /Type /Catalog /Pages 2 0 R >>\nendobj\n";
    let obj2 = "2 0 obj\n<< /Type /Pages /Kids [3 0 R] /Count 1 >>\nendobj\n";
    let obj3 = "3 0 obj\n<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] >>\nendobj\n";
    let o1 = out.len();
    out.extend_from_slice(obj1.as_bytes());
    let o2 = out.len();
    out.extend_from_slice(obj2.as_bytes());
    let o3 = out.len();
    out.extend_from_slice(obj3.as_bytes());
    let xref = out.len();
    out.extend_from_slice(format!("xref\n0 4\n0000000000 65535 f \n{o1:010} 00000 n \n{o2:010} 00000 n \n{o3:010} 00000 n \n").as_bytes());
    out.extend_from_slice(format!("trailer\n<< /Size 4 /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n").as_bytes());

    let doc_id = engine.open_bytes("test.pdf".into(), out, None).expect("engine is still usable");
    assert_eq!(engine.page_count(doc_id).unwrap(), 1);
    engine.close(doc_id).unwrap();
}
