//! Invoice arithmetic is available headlessly without opening or changing a PDF.

use pdfcraft_automation::{Automation, Content, ToolError, tools};
use serde_json::{Value, json};

/// Build supported JSON input without relying on PDF state or external fixtures.
fn draft() -> Value {
    json!({
        "number":"RE-2026-001", "issue_date":"2026-10-23", "currency":"EUR",
        "seller":{"name":"Example Seller", "country_code":"DE"},
        "buyer":{"name":"Example Buyer", "country_code":"DE"},
        "lines":[{"description":"Service", "quantity":"2.5", "unit_price":"12.3456", "vat_rate":"19"}]
    })
}

/// Invoke the public tool dispatcher and require its structured JSON report.
fn calculate(automation: &mut Automation, draft: Value) -> Value {
    let result = automation.call("invoice_calculate", &json!({"draft":draft})).unwrap();
    match result.into_iter().next().unwrap() {
        Content::Json(value) => value,
        other => panic!("expected JSON: {other:?}"),
    }
}

/// Verify exact totals, truthful validation scope and unchanged open-document state.
#[test]
fn invoice_tool_calculates_without_claiming_conformance_or_opening_documents() {
    let mut a = Automation::new();
    let before = a.call("doc_list", &json!({})).unwrap();
    let report = calculate(&mut a, draft());
    assert_eq!(report["scope"], "draft_arithmetic_only");
    assert_eq!(report["standards_validation"], "not_performed");
    assert_eq!(report["issues"], json!([]));
    assert_eq!(report["totals"]["net_amount"], "30.86");
    assert_eq!(report["totals"]["tax_amount"], "5.86");
    assert_eq!(report["totals"]["payable_amount"], "36.72");
    assert_eq!(a.call("doc_list", &json!({})).unwrap(), before);
    let id = match a.call("doc_create", &json!({"from":"blank"})).unwrap().remove(0) {
        Content::Json(value) => value["doc"].as_u64().unwrap(),
        other => panic!("expected JSON: {other:?}"),
    };
    let bytes = a.session().save_bytes(pdfcraft_engine::DocId(id)).unwrap();
    let before = a.call("doc_list", &json!({})).unwrap();
    calculate(&mut a, draft());
    assert_eq!(a.call("doc_list", &json!({})).unwrap(), before);
    assert_eq!(a.session().save_bytes(pdfcraft_engine::DocId(id)).unwrap(), bytes);
    let tool = tools().into_iter().find(|t| t.name == "invoice_calculate").unwrap();
    assert!(tool.read_only);
    assert!(!tool.destructive);
}

/// Separate field-level semantic issues from malformed or unsupported argument errors.
#[test]
fn invoice_tool_returns_semantic_issues_but_rejects_unsupported_fields_and_numbers() {
    let mut a = Automation::new();
    let mut input = draft();
    input["lines"][0]["vat_rate"] = json!("0");
    let report = calculate(&mut a, input);
    assert!(report["totals"].is_null());
    assert_eq!(report["issues"][0]["path"], "lines[0].vat_rate");
    for location in ["draft", "seller", "line"] {
        let mut input = draft();
        let target = match location {
            "seller" => &mut input["seller"],
            "line" => &mut input["lines"][0],
            _ => &mut input,
        };
        target["discount"] = json!("5");
        assert!(matches!(a.call("invoice_calculate", &json!({"draft":input})), Err(ToolError::InvalidArgs(_))));
    }
    for value in [json!(0.1), json!("-1"), json!("1,23"), json!("1e10"), json!("9999999999999"), json!("NaN")] {
        let mut input = draft();
        input["lines"][0]["unit_price"] = value;
        assert!(a.call("invoice_calculate", &json!({"draft":input})).is_err());
    }
}

/// Exercise handler limits before JSON cloning and reject an unexpected output path.
#[test]
fn invoice_tool_refuses_oversized_or_deep_inputs_before_copying_them() {
    let mut a = Automation::new();
    let mut large = draft();
    large["lines"] = json!(vec![large["lines"][0].clone(); 1001]);
    assert!(a.call("invoice_calculate", &json!({"draft":large})).is_err());
    let mut input = draft();
    input["seller"]["name"] = json!("x".repeat(1025));
    assert!(a.call("invoice_calculate", &json!({"draft":input})).is_err());
    let mut deep = json!({});
    for _ in 0..20 {
        deep = json!({"nested":deep});
    }
    assert!(a.call("invoice_calculate", &json!({"draft":deep})).is_err());
    assert!(a.call("invoice_calculate", &json!({"draft":draft(), "path":"../invoice.xml"})).is_err());
}
