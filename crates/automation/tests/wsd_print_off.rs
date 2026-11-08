//! Without the `network-print` feature the WS-Print tools refuse and nothing can reach a printer.
#![cfg(not(feature = "network-print"))]

use pdfcraft_automation::{Automation, ToolError};
use serde_json::json;

#[test]
fn the_tools_refuse_without_the_feature() {
    let mut a = Automation::new();
    let url = "http://127.0.0.1:5357/print";
    for (tool, args) in
        [("wsd_printer_info", json!({ "printer_url": url })), ("print_wsd", json!({ "printer_url": url, "user_confirmed": true, "path": "x.pdf" }))]
    {
        let e = a.call(tool, &args).unwrap_err();
        assert!(matches!(&e, ToolError::Failed(m) if m.contains("network-print")), "{tool}: {e:?}");
    }
}
