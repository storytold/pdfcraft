//! Comprehensive Integration Tests for Bundle 3: Annotations, Appearances, and Scripted Forms.

use pdfcraft_appearances::{AnnotationMarkup, AppearanceSynthesizer, LineEndingStyle};
use pdfcraft_forms_engine::{ButtonActionDispatcher, FieldAppearanceGenerator, FormAction, FormField, FormFieldType, FLAG_COMB, FLAG_PASSWORD};
use pdfcraft_js_sandbox::{AcrobatEvent, DocObject, SandboxedJsContext};
use std::collections::HashMap;

#[test]
fn test_appearance_stream_generation_all_markup_types() {
    // 1. Highlight
    let highlight = AnnotationMarkup::Highlight { quad_points: vec![10.0, 10.0, 80.0, 10.0, 10.0, 30.0, 80.0, 30.0] };
    let app1 = AppearanceSynthesizer::synthesize(&highlight, [10.0, 10.0, 80.0, 30.0], [1.0, 1.0, 0.0], 1.0);
    assert!(!app1.stream_bytes.is_empty());

    // 2. Line with arrow endings
    let line = AnnotationMarkup::Line {
        start: (10.0, 10.0),
        end: (100.0, 100.0),
        start_style: LineEndingStyle::ClosedArrow,
        end_style: LineEndingStyle::Circle,
    };
    let app2 = AppearanceSynthesizer::synthesize(&line, [10.0, 10.0, 100.0, 100.0], [0.0, 0.0, 0.0], 2.0);
    assert!(!app2.stream_bytes.is_empty());

    // 3. Redaction mark
    let redact = AnnotationMarkup::Redact { overlay_text: Some("EXEMPT (b)(4)".into()) };
    let app3 = AppearanceSynthesizer::synthesize(&redact, [50.0, 50.0, 200.0, 70.0], [0.0, 0.0, 0.0], 1.0);
    let redact_str = String::from_utf8_lossy(&app3.stream_bytes);
    assert!(redact_str.contains("EXEMPT (b)(4)"));
}

#[test]
fn test_acroform_comb_and_password_appearance_regeneration() {
    // 1. Comb field (e.g. SSN / Tax ID with fixed cells)
    let ssn_field = FormField {
        name: "SSN".into(),
        field_type: FormFieldType::Text,
        flags: FLAG_COMB,
        value: "987654321".into(),
        default_value: None,
        max_len: Some(9),
        bbox: [20.0, 20.0, 200.0, 40.0],
        default_appearance: "/Helv 0 Tf 0 g".into(),
    };
    let ssn_ap = FieldAppearanceGenerator::generate_normal_appearance(&ssn_field);
    let ssn_str = String::from_utf8_lossy(&ssn_ap);
    assert!(ssn_str.contains("Tm (9) Tj"));
    assert!(ssn_str.contains("Tm (1) Tj"));

    // 2. Password field masking
    let pass_field = FormField {
        name: "PIN".into(),
        field_type: FormFieldType::Text,
        flags: FLAG_PASSWORD,
        value: "4321".into(),
        default_value: None,
        max_len: None,
        bbox: [20.0, 50.0, 100.0, 70.0],
        default_appearance: "/Helv 12 Tf 0 g".into(),
    };
    let pass_ap = FieldAppearanceGenerator::generate_normal_appearance(&pass_field);
    let pass_str = String::from_utf8_lossy(&pass_ap);
    assert!(!pass_str.contains("4321"));
    assert!(pass_str.contains("\u{2022}\u{2022}\u{2022}\u{2022}"));
}

#[test]
fn test_interactive_button_and_sandboxed_js_execution() {
    let mut fields = HashMap::new();
    fields.insert("Subtotal".into(), "150.00".into());
    fields.insert("TaxRate".into(), "0.08".into());

    let defaults = fields.clone();

    // 1. Reset button execution
    ButtonActionDispatcher::dispatch(&FormAction::ResetForm { fields: None }, &mut fields, &defaults);
    assert_eq!(fields.get("Subtotal").unwrap(), "150.00");

    // 2. JavaScript sandbox calculation
    let doc = DocObject::new(1, fields);
    let mut js_ctx = SandboxedJsContext::new(doc, 5000);
    js_ctx.event = Some(AcrobatEvent {
        name: "calculate".into(),
        target_field: "Total".into(),
        value: "".into(),
        rc: true,
        will_commit: true,
    });

    let res = js_ctx.execute("AFSimple_Calculate(\"SUM\", new Array(\"Subtotal\", \"TaxRate\"));").unwrap();
    assert_eq!(res, Some("150.08".into()));
    assert_eq!(js_ctx.event.unwrap().value, "150.08");
}
