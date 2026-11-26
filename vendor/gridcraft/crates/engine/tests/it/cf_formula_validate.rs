//! #1999: a conditional-format rule whose formula doesn't parse must be rejected at
//! creation (Excel's object model refuses it too), not saved and poison the file.

use gridcraft_engine::Session;
use serde_json::json;

fn session() -> Session {
    let mut s = Session::new();
    s.new_workbook();
    s
}

#[test]
fn invalid_cf_formulas_are_rejected() {
    for rule in [
        // exact repros from the issue's measured table
        json!({"type": "expression", "formula": "=A1>", "preset": "lightRedFill"}),
        json!({"type": "cellIs", "operator": "greaterThan", "value": "=1+", "preset": "lightRedFill"}),
        json!({"type": "dataBar", "min": {"type": "formula", "value": "1+"}}),
        // more garbage that the parser rejects
        json!({"type": "expression", "formula": "=IF(A1", "preset": "lightRedFill"}),
        json!({"type": "cellIs", "operator": "equal", "value": "=((A1", "preset": "lightRedFill"}),
        // multiple leading '=' on a stored-as-typed threshold: the writer strips one,
        // the rest must still parse (#1999 review)
        json!({"type": "dataBar", "min": {"type": "formula", "value": "===A1"}}),
        json!({"type": "dataBar", "max": {"type": "formula", "value": "==1+"}}),
    ] {
        let err = session()
            .execute("home.conditionalFormat", json!({"range": "A1:A5", "rule": rule.clone()}))
            .expect_err("invalid formula {rule} must be rejected");
        let msg = err.to_string();
        assert!(msg.to_lowercase().contains("formula"), "error should mention the formula: {msg}");
    }
}

#[test]
fn valid_cf_formulas_still_work() {
    let mut s = session();
    // bare comparison without '=' (the issue: this one saves fine today)
    s.execute("home.conditionalFormat", json!({"range": "A1:A5", "rule": {"type": "expression", "formula": "A1>5", "preset": "lightRedFill"}}))
        .unwrap();
    // leading '=' is accepted and stripped
    s.execute("home.conditionalFormat", json!({"range": "B1:B5", "rule": {"type": "expression", "formula": "=B1>5", "preset": "lightRedFill"}}))
        .unwrap();
    // cellIs numeric and text values are not formulas, must pass untouched
    s.execute(
        "home.conditionalFormat",
        json!({"range": "C1:C5", "rule": {"type": "cellIs", "operator": "greaterThan", "value": "5", "preset": "lightRedFill"}}),
    )
    .unwrap();
    s.execute(
        "home.conditionalFormat",
        json!({"range": "D1:D5", "rule": {"type": "cellIs", "operator": "equal", "value": "hello there", "preset": "lightRedFill"}}),
    )
    .unwrap();
    // dataBar formula thresholds that parse
    s.execute("home.conditionalFormat", json!({"range": "E1:E5", "rule": {"type": "dataBar", "min": {"type": "formula", "value": "MIN(E1:E5)"}, "max": {"type": "formula", "value": "MAX(E1:E5)"}}}))
        .unwrap();
    // non-formula rule types are untouched
    s.execute("home.conditionalFormat", json!({"range": "F1:F5", "rule": {"type": "duplicate", "preset": "lightRedFill"}})).unwrap();
}
