//! The foundation is usable through the real script entry point, without a PDF.

use std::process::Command;

use serde_json::{Value, json};

/// Run the documented draft through the real CLI and require exact totals without output files.
#[test]
fn invoice_script_returns_exact_totals_and_explicit_validation_scope() {
    let directory = std::env::temp_dir().join(format!("pdfcraft-cli-invoice-{}", std::process::id()));
    std::fs::create_dir_all(&directory).unwrap();
    let script = directory.join("steps.json");
    let steps = json!([{"tool":"invoice_calculate", "args":{"draft":{
        "number":"RE-2026-001", "issue_date":"2026-10-23", "currency":"EUR",
        "seller":{"name":"Example Seller", "country_code":"DE"},
        "buyer":{"name":"Example Buyer", "country_code":"DE"},
        "lines":[{"description":"Service", "quantity":"2.5", "unit_price":"12.3456", "vat_rate":"19"}]
    }}}]);
    std::fs::write(&script, steps.to_string()).unwrap();
    let output =
        Command::new(env!("CARGO_BIN_EXE_pdfcraft-cli")).args(["run", "--script"]).arg(&script).arg("--root").arg(&directory).output().unwrap();
    assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
    let report: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(report["standards_validation"], "not_performed");
    assert_eq!(report["totals"]["net_amount"], "30.86");
    assert_eq!(report["totals"]["payable_amount"], "36.72");
    assert_eq!(std::fs::read_dir(&directory).unwrap().count(), 1, "the tool must create no output files");
    std::fs::remove_file(script).unwrap();
    std::fs::remove_dir(directory).unwrap();
}
