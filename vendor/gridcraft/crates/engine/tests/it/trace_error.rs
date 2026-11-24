//! Formulas › Formula Auditing › Error Checking › Trace Error: from a cell showing an error, the
//! first reference that shows the same error, back to the cell the error starts in, with arrows
//! along the way and from the source's own references into it. The sheet and the expected
//! sources and arrows are the ones measured in Excel.

use gridcraft_engine::Session;
use serde_json::{Value as Json, json};

/// The measured sheet. A4 is a typed `#N/A`; I1's own formula gives `#REF!`.
fn measured() -> Session {
    let mut s = Session::new();
    s.new_workbook();
    for (cell, input) in [
        ("A1", "=1/0"),
        ("A2", "5"),
        ("B1", "=A1+A2"),
        ("C1", "=B1*2"),
        ("D1", "=SUM(C1,A2)"),
        ("A4", "#N/A"),
        ("B4", "=A4"),
        ("C4", "=B4&\"x\""),
        ("A6", "=SQRT(-1)"),
        ("A7", "3"),
        ("B6", "=A6+A7"),
        ("E1", "10"),
        ("E2", "=E1*2"),
        ("F1", "=A1+A4"),
        ("F2", "=A4+A1"),
        ("G1", "=IFERROR(A1,0)"),
        ("G2", "=G1+B1"),
        ("H1", "0"),
        ("H2", "=10/H1"),
        ("H3", "=H2+E1"),
        ("H4", "=H3*1"),
        ("I1", "=#REF!"),
        ("I2", "=I1+1"),
        ("J1", "=XLOOKUP(99,A2:A3,A2:A3)"),
        ("J2", "=J1"),
        ("A9", "=A10"),
        ("A10", "=1/0"),
        ("B9", "=SUM(A9:A10)"),
        ("C9", "=B9"),
    ] {
        s.execute("cell.set", json!({"cell": cell, "input": input})).unwrap();
    }
    s
}

fn trace(s: &mut Session, cell: &str) -> Json {
    s.execute("formulas.traceError", json!({"cell": cell})).unwrap()
}

/// An end of an arrow as `A1`, or `Sheet2!A1` off Sheet1.
fn end(v: &Json) -> String {
    let at = v["range"].as_str().or(v["cell"].as_str()).unwrap();
    match v["sheet"].as_str().unwrap() {
        "Sheet1" => at.to_string(),
        sheet => format!("{sheet}!{at}"),
    }
}

/// The arrows as `from → to`, with ` (precedent)` after the source's own references.
fn arrows(r: &Json) -> Vec<String> {
    r["arrows"]
        .as_array()
        .unwrap()
        .iter()
        .map(|a| {
            let kind = match a["kind"].as_str().unwrap() {
                "error" => "",
                "precedent" => " (precedent)",
                k => panic!("unknown kind {k}"),
            };
            format!("{} → {}{kind}", end(&a["from"]), end(&a["to"]))
        })
        .collect()
}

#[test]
fn trace_error_finds_the_measured_sources_and_arrows() {
    let mut s = measured();
    for (cell, error, source, expected) in [
        ("C1", "#DIV/0!", "A1", vec!["A1 → B1", "B1 → C1"]),
        ("D1", "#DIV/0!", "A1", vec!["A1 → B1", "B1 → C1", "C1 → D1"]),
        ("B1", "#DIV/0!", "A1", vec!["A1 → B1"]),
        ("A1", "#DIV/0!", "A1", vec![]),
        ("A6", "#NUM!", "A6", vec![]),
        ("C4", "#N/A", "A4", vec!["A4 → B4", "B4 → C4"]),
        // Two errors: the first reference showing the cell's own error.
        ("F1", "#DIV/0!", "A1", vec!["A1 → F1"]),
        ("F2", "#N/A", "A4", vec!["A4 → F2"]),
        // IFERROR in G1 hides the error, so the error comes through B1.
        ("G2", "#DIV/0!", "A1", vec!["A1 → B1", "B1 → G2"]),
        // H1 feeds the source H2 without an error of its own; E1 feeds H3 and has no arrow.
        ("H4", "#DIV/0!", "H2", vec!["H1 → H2 (precedent)", "H2 → H3", "H3 → H4"]),
        ("H3", "#DIV/0!", "H2", vec!["H1 → H2 (precedent)", "H2 → H3"]),
        // XLOOKUP's two A2:A3 arguments make one range arrow into the source.
        ("J2", "#N/A", "J1", vec!["A2:A3 → J1 (precedent)", "J1 → J2"]),
        // In A9:A10 the first cell showing the error is A9, itself fed by A10.
        ("C9", "#DIV/0!", "A10", vec!["A10 → A9", "A9 → B9", "B9 → C9"]),
        ("I2", "#REF!", "I1", vec!["I1 → I2"]),
    ] {
        let r = trace(&mut s, cell);
        assert_eq!((r["sheet"].as_str(), r["cell"].as_str()), (Some("Sheet1"), Some(cell)));
        assert_eq!(r["error"], error, "{cell}");
        assert_eq!(end(&r["source"]), source, "{cell}");
        assert_eq!(arrows(&r), expected, "{cell}");
        assert_eq!(r["truncated"], false, "{cell}");
    }
}

#[test]
fn a_cell_without_an_error_has_no_source_and_no_arrows() {
    let mut s = measured();
    for cell in ["E2", "A2", "Z99", "G1"] {
        let r = trace(&mut s, cell);
        assert_eq!(r, json!({"sheet": "Sheet1", "cell": cell, "error": null, "source": null, "arrows": [], "truncated": false}));
    }
    // Without `cell`, the active cell.
    s.execute("selection.set", json!({"range": "C1"})).unwrap();
    let r = s.execute("formulas.traceError", json!({})).unwrap();
    assert_eq!((r["cell"].as_str(), arrows(&r).len()), (Some("C1"), 2));
}

#[test]
fn tracing_changes_nothing_and_twice_gives_the_same_arrows() {
    let mut s = measured();
    let (wb, undo) = (s.doc().unwrap().wb.clone(), s.doc().unwrap().undo.len());
    let first = trace(&mut s, "C1");
    assert_eq!(trace(&mut s, "C1"), first);
    let d = s.doc().unwrap();
    assert!(std::sync::Arc::ptr_eq(&d.wb, &wb), "the workbook is untouched");
    assert_eq!(d.undo.len(), undo, "no undo step");
}

#[test]
fn a_cell_on_another_sheet_is_refused_and_bad_params_fail() {
    let mut s = measured();
    s.execute("home.insertSheet", json!({"name": "Two"})).unwrap();
    s.execute("sheet.activate", json!({"sheet": "Sheet1"})).unwrap();
    for cell in [json!("Two!A1"), json!("not a cell"), json!(5)] {
        assert!(s.execute("formulas.traceError", json!({"cell": cell})).is_err(), "{cell}");
    }
    assert!(s.execute("formulas.traceError", json!({"cell": "Sheet1!C1"})).is_ok());
}

#[test]
fn the_error_is_followed_across_sheets() {
    let mut s = Session::new();
    s.new_workbook();
    s.execute("home.insertSheet", json!({"name": "Data 2"})).unwrap();
    s.execute("cell.set", json!({"cell": "A1", "input": "=1/B1"})).unwrap();
    s.execute("cell.set", json!({"cell": "B1", "input": "0"})).unwrap();
    s.execute("cell.set", json!({"cell": "C1", "input": "=A1"})).unwrap();
    s.execute("sheet.activate", json!({"sheet": "Sheet1"})).unwrap();
    s.execute("cell.set", json!({"cell": "B2", "input": "='Data 2'!C1*2"})).unwrap();
    s.execute("cell.set", json!({"cell": "C2", "input": "=B2"})).unwrap();
    let r = trace(&mut s, "C2");
    assert_eq!(end(&r["source"]), "Data 2!A1");
    assert_eq!(arrows(&r), ["Data 2!B1 → Data 2!A1 (precedent)", "Data 2!A1 → Data 2!C1", "Data 2!C1 → B2", "B2 → C2"]);
}

#[test]
fn only_the_cells_own_error_is_followed() {
    let mut s = measured();
    // K1 shows #DIV/0!; A4 shows #N/A, another error, so K1 is the source and A4 only feeds it.
    s.execute("cell.set", json!({"cell": "K1", "input": "=IF(ISNA(A4),1/0,0)"})).unwrap();
    s.execute("cell.set", json!({"cell": "K2", "input": "=K1+F2"})).unwrap();
    let r = trace(&mut s, "K2");
    assert_eq!(r["error"], "#DIV/0!");
    assert_eq!(end(&r["source"]), "K1");
    assert_eq!(arrows(&r), ["A4 → K1 (precedent)", "K1 → K2"]);
}

#[test]
fn a_spilled_error_leads_to_the_references_of_its_array_formula() {
    let mut s = Session::new();
    s.new_workbook();
    s.execute("range.setValues", json!({"range": "A1", "values": [[1, 1], [2, 0]]})).unwrap();
    // D1 spills 1 and #DIV/0! into D1:D2.
    s.execute("cell.set", json!({"cell": "D1", "input": "=A1:A2/B1:B2"})).unwrap();
    s.execute("cell.set", json!({"cell": "E1", "input": "=SUM(D1:D2)"})).unwrap();
    let r = trace(&mut s, "E1");
    assert_eq!(end(&r["source"]), "D2", "{r}");
    assert_eq!(arrows(&r), ["A1:A2 → D2 (precedent)", "B1:B2 → D2 (precedent)", "D2 → E1"]);
}

#[test]
fn a_circular_reference_ends_the_walk() {
    let mut s = Session::new();
    s.new_workbook();
    s.execute("cell.set", json!({"cell": "A1", "input": "=B1+1"})).unwrap();
    s.execute("cell.set", json!({"cell": "B1", "input": "=A1+1"})).unwrap();
    s.execute("cell.set", json!({"cell": "C1", "input": "=C1"})).unwrap();
    s.execute("cell.set", json!({"cell": "D1", "input": "=B1"})).unwrap();
    let r = trace(&mut s, "D1");
    assert!(r["error"].is_string(), "{r}");
    assert_eq!(end(&r["source"]), "A1");
    assert_eq!(arrows(&r), ["B1 → A1 (precedent)", "A1 → B1", "B1 → D1"]);
    // A cell that reads itself is its own source.
    let r = trace(&mut s, "C1");
    assert_eq!(end(&r["source"]), "C1");
    assert_eq!(arrows(&r), ["C1 → C1 (precedent)"]);
}

#[test]
fn a_long_chain_is_walked_to_its_source_and_a_longer_one_stops_at_the_limit() {
    // On a 1 MiB stack, as on the desktop app's main thread: the walk must not recurse.
    std::thread::Builder::new()
        .stack_size(1 << 20)
        .spawn(|| {
            let mut s = Session::new();
            s.new_workbook();
            let rows = 10_002;
            let formulas: Vec<[String; 1]> = (1..=rows).map(|r| [if r == 1 { "=1/0".to_string() } else { format!("=A{}", r - 1) }]).collect();
            s.execute("range.setValues", json!({"range": "A1", "values": formulas})).unwrap();
            // 10,000 links, the most Trace Error walks: it reaches A1.
            let r = trace(&mut s, "A10001");
            assert_eq!((end(&r["source"]), r["truncated"].clone()), ("A1".to_string(), json!(false)));
            let list = arrows(&r);
            assert_eq!(list.len(), 10_000);
            assert_eq!((list.first().map(String::as_str), list.last().map(String::as_str)), (Some("A1 → A2"), Some("A10000 → A10001")));
            // One more and the walk stops short of the source.
            let r = trace(&mut s, "A10002");
            assert_eq!((r["source"].clone(), r["truncated"].clone()), (json!(null), json!(true)));
            let list = arrows(&r);
            assert_eq!(list.len(), 10_000);
            assert_eq!(list.first().map(String::as_str), Some("A2 → A3"), "no arrows into a cell that isn't the source");
        })
        .unwrap()
        .join()
        .unwrap();
}
