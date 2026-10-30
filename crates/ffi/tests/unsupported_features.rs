#[allow(unused_imports)]
use pdfcraft_ffi::{Engine, FfiError};

#[test]
fn test_unsupported_features_return_unsupported_error() {
    let engine = Engine::new();
    let empty_pdf = b"%PDF-1.4\n1 0 obj\n<< /Type /Catalog /Pages 2 0 R >>\nendobj\n2 0 obj\n<< /Type /Pages /Kids [3 0 R] /Count 1 >>\nendobj\n3 0 obj\n<< /Type /Page /Parent 2 0 R /MediaBox [0 0 100 100] >>\nendobj\nxref\n0 4\n0000000000 65535 f \n0000000009 00000 n \n0000000058 00000 n \n0000000115 00000 n \ntrailer\n<< /Size 4 /Root 1 0 R >>\nstartxref\n185\n%%EOF\n".to_vec();
    let doc_id = engine.open_bytes("test.pdf".into(), empty_pdf, None).expect("open test document");

    #[cfg(not(feature = "ocr"))]
    {
        assert!(matches!(engine.ocr_status(), Err(FfiError::Unsupported { .. })));
        assert!(matches!(engine.recognize_page_text(doc_id, 0, None, false), Err(FfiError::Unsupported { .. })));
        assert!(matches!(engine.apply_ocr_text_layer(doc_id, 0, Vec::new()), Err(FfiError::Unsupported { .. })));
        assert!(matches!(engine.set_ocr_models_dir("/tmp".into()), Err(FfiError::Unsupported { .. })));
    }

    #[cfg(not(feature = "sign"))]
    {
        assert!(matches!(engine.inspect_digital_id(vec![0; 10], None), Err(FfiError::Unsupported { .. })));
        assert!(matches!(engine.list_digital_signatures(doc_id), Err(FfiError::Unsupported { .. })));
    }

    #[cfg(not(feature = "js"))]
    {
        assert!(matches!(engine.set_form_javascript_enabled(true), Err(FfiError::Unsupported { .. })));
        assert!(matches!(engine.is_form_javascript_enabled(), Err(FfiError::Unsupported { .. })));
        assert!(matches!(engine.run_form_javascript(doc_id, "1+1".into(), None), Err(FfiError::Unsupported { .. })));
        assert!(matches!(engine.take_form_javascript_output(doc_id), Err(FfiError::Unsupported { .. })));
    }

    #[cfg(not(feature = "xfa"))]
    {
        assert!(matches!(engine.get_xfa_status(doc_id), Err(FfiError::Unsupported { .. })));
        assert!(matches!(engine.export_xfa_datasets(doc_id), Err(FfiError::Unsupported { .. })));
        assert!(matches!(engine.import_xfa_datasets(doc_id, "<data/>".into()), Err(FfiError::Unsupported { .. })));
    }

    #[cfg(not(feature = "preflight"))]
    {
        assert!(matches!(engine.verify_pdfa(doc_id, "pdfa2b".into()), Err(FfiError::Unsupported { .. })));
        assert!(matches!(engine.declared_standards(doc_id), Err(FfiError::Unsupported { .. })));
        assert!(matches!(engine.convert_to_pdfa(doc_id, "pdfa2b".into()), Err(FfiError::Unsupported { .. })));
    }

    engine.close(doc_id).expect("close");
}
