use pdfcraft_ffi::{Engine, FfiToolClassification, invoke_tool, list_tools, parse_tool_schema};

fn sample_pdf() -> Vec<u8> {
    b"%PDF-1.7
1 0 obj
<< /Type /Catalog /Pages 2 0 R >>
endobj
2 0 obj
<< /Type /Pages /Kids [3 0 R] /Count 1 >>
endobj
3 0 obj
<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] >>
endobj
xref
0 4
0000000000 65535 f 
0000000009 00000 n 
000000058 00000 n 
000000115 00000 n 
trailer
<< /Size 4 /Root 1 0 R >>
startxref
193
%%EOF
"
    .to_vec()
}

#[test]
fn test_hard_gate_all_tools_classified() {
    let tools = list_tools();
    assert!(!tools.is_empty(), "tool catalogue must not be empty");

    let mut schema_driven_count = 0;
    let mut bespoke_count = 0;
    let mut not_on_mobile_count = 0;

    for tool in &tools {
        match tool.classification {
            FfiToolClassification::SchemaDriven => {
                schema_driven_count += 1;
                // Schema must be valid and parseable
                let fields = parse_tool_schema(tool.input_schema_json.clone())
                    .unwrap_or_else(|e| panic!("tool {} marked SchemaDriven failed schema parse: {:?}", tool.name, e));
                assert!(!fields.is_empty() || tool.input_schema_json.contains("{}"), "tool {} had 0 fields", tool.name);
            }
            FfiToolClassification::BespokeScreen => {
                bespoke_count += 1;
                assert!(tool.mobile_reason.is_none(), "bespoke screen tool {} should not have mobile_reason", tool.name);
            }
            FfiToolClassification::NotOnMobile => {
                not_on_mobile_count += 1;
                assert!(
                    tool.mobile_reason.is_some() && !tool.mobile_reason.as_ref().unwrap().is_empty(),
                    "tool {} marked NotOnMobile must provide an explicit reason",
                    tool.name
                );
            }
        }
    }

    assert!(schema_driven_count > 0, "should have schema-driven tools");
    assert!(bespoke_count > 0, "should have bespoke screen tools");
    assert!(not_on_mobile_count > 0, "should have documented not-on-mobile tools");
}

#[test]
fn test_schema_parsing_subset_types() {
    let custom_schema = r#"{
        "type": "object",
        "properties": {
            "title": { "type": "string", "description": "Document title" },
            "count": { "type": "integer", "minimum": 1, "maximum": 100 },
            "ratio": { "type": "number" },
            "enabled": { "type": "boolean", "default": true },
            "mode": { "type": "string", "enum": ["fast", "quality", "balanced"] },
            "pages": { "type": "string", "description": "1-based page range e.g. 1-3, 5" }
        },
        "required": ["title", "count"]
    }"#;

    let fields = parse_tool_schema(custom_schema.into()).expect("parse schema");
    assert_eq!(fields.len(), 6);

    let title_field = fields.iter().find(|f| f.name == "title").expect("title field");
    assert_eq!(title_field.field_type, pdfcraft_ffi::FfiSchemaFieldType::String);
    assert!(title_field.required);

    let count_field = fields.iter().find(|f| f.name == "count").expect("count field");
    assert_eq!(count_field.field_type, pdfcraft_ffi::FfiSchemaFieldType::Integer);
    assert_eq!(count_field.minimum, Some(1.0));
    assert_eq!(count_field.maximum, Some(100.0));
    assert!(count_field.required);

    let enabled_field = fields.iter().find(|f| f.name == "enabled").expect("enabled field");
    assert_eq!(enabled_field.field_type, pdfcraft_ffi::FfiSchemaFieldType::Boolean);
    assert!(!enabled_field.required);
    assert_eq!(enabled_field.default_value, Some("true".into()));

    let mode_field = fields.iter().find(|f| f.name == "mode").expect("mode field");
    assert_eq!(mode_field.field_type, pdfcraft_ffi::FfiSchemaFieldType::Enum);
    assert_eq!(mode_field.enum_options, vec!["fast", "quality", "balanced"]);

    let pages_field = fields.iter().find(|f| f.name == "pages").expect("pages field");
    assert_eq!(pages_field.field_type, pdfcraft_ffi::FfiSchemaFieldType::PageRange);
}

#[test]
fn test_engine_tool_invocation_equality_and_state_sync() {
    let engine = Engine::new();
    let bytes = sample_pdf();
    let doc_id = engine.open_bytes("test.pdf".into(), bytes, None).expect("open bytes");

    // 1. Inspect properties via tool invocation on the engine session
    let info_args = serde_json::json!({ "doc": doc_id }).to_string();
    let res = engine.invoke_tool("doc_info".into(), info_args).expect("invoke doc_info");
    assert!(res.success);
    assert!(res.output_json.contains("\"pdf_version\""));
    assert!(res.output_json.contains("\"pages\":"));

    // 2. Rotate page via tool invocation
    let rotate_args = serde_json::json!({
        "doc": doc_id,
        "degrees": 90
    })
    .to_string();
    let rotate_res = engine.invoke_tool("page_rotate".into(), rotate_args).expect("invoke page_rotate");
    assert!(rotate_res.success);
    assert!(rotate_res.output_json.contains("\"rotated\":1"));

    // Verify engine's page_geometry picked up the tool mutation
    let geom = engine.page_geometry(doc_id, 0).expect("page geometry");
    assert_eq!(geom.rotation, 90);

    // 3. Destructive tool flag check
    let tools = list_tools();
    let rotate_tool = tools.iter().find(|t| t.name == "page_rotate").expect("rotate tool");
    assert!(!rotate_tool.read_only);

    let doc_save_tool = tools.iter().find(|t| t.name == "doc_save").expect("save tool");
    assert!(doc_save_tool.destructive);

    let doc_info_tool = tools.iter().find(|t| t.name == "doc_info").expect("doc_info tool");
    assert!(doc_info_tool.read_only);
    assert!(!doc_info_tool.destructive);

    // 4. Standalone tool call
    let list_res = invoke_tool("doc_list".into(), "{}".into()).expect("invoke doc_list");
    assert!(list_res.success);
}

#[test]
fn test_heavy_features_classification_and_gating() {
    let tools = list_tools();

    // Verify heavy features tools exist in catalogue and have valid classifications
    let heavy_tool_names = [
        ("ocr_recognize", cfg!(feature = "ocr")),
        ("ocr_status", cfg!(feature = "ocr")),
        ("sign_list", cfg!(feature = "sign")),
        ("sign_document", cfg!(feature = "sign")),
        ("doc_export_images", cfg!(feature = "export")),
        ("doc_export_text", cfg!(feature = "export")),
    ];

    for (name, enabled) in heavy_tool_names {
        let tool = tools.iter().find(|t| t.name == name);
        assert!(tool.is_some(), "tool '{name}' must exist in catalogue");
        let tool = tool.unwrap();
        if enabled {
            assert!(tool.classification == FfiToolClassification::SchemaDriven || tool.classification == FfiToolClassification::BespokeScreen);
        } else {
            assert_eq!(tool.classification, FfiToolClassification::NotOnMobile);
            assert!(tool.mobile_reason.as_ref().unwrap().contains("not compiled into pdfcraft-ffi"));
        }
    }
}
