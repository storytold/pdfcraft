//! Flat operator chains as long as Excel allows (#916). `=1+1+…` with 4,096 operands is
//! exactly 8,192 characters, Excel's formula limit, and builds a tree 4,095 levels deep on its
//! left. Entering, editing, saving and reopening such a formula must neither overflow the stack
//! nor fall back to an error: Excel computes it.

use gridcraft_core::{CellRef, Value};
use gridcraft_engine::{DocState, Session, io};
use serde_json::json;

/// Excel's longest flat chain of literals: 4,096 operands, 8,192 characters with the `=`.
fn literal_chain(op: &str) -> String {
    format!("={}", vec!["1"; 4096].join(op))
}

/// `=A1+A2+…+A1500`: 7,893 characters, inside Excel's limit.
fn reference_chain() -> String {
    format!("={}", (1..=1500).map(|r| format!("A{r}")).collect::<Vec<_>>().join("+"))
}

fn value(session: &Session, cell: &str) -> Value {
    let at = CellRef::parse(cell).expect("cell");
    session.doc().expect("doc").wb.sheet(0).expect("sheet").value(at)
}

/// Runs `f` on a 1 MiB stack, the size of the Windows main thread the desktop app runs on.
fn on_small_stack(f: impl FnOnce() + Send + 'static) {
    std::thread::Builder::new().stack_size(1 << 20).spawn(f).expect("spawn").join().expect("long chains must not overflow the stack");
}

#[test]
fn longest_flat_chains_survive_editing_saving_and_reopening() {
    on_small_stack(|| {
        let mut session = Session::new();
        session.new_workbook();
        session.execute("range.setValues", json!({"range": "A1:A1500", "values": vec![[1]; 1500]})).expect("values");
        session.execute("cell.set", json!({"cell": "C1", "input": literal_chain("+")})).expect("plus chain");
        session.execute("cell.set", json!({"cell": "C2", "input": literal_chain("-")})).expect("minus chain");
        session.execute("cell.set", json!({"cell": "C3", "input": literal_chain("&")})).expect("concat chain");
        session.execute("cell.set", json!({"cell": "C4", "input": reference_chain()})).expect("reference chain");
        assert_eq!(value(&session, "C1"), Value::Number(4096.0));
        assert_eq!(value(&session, "C2"), Value::Number(-4094.0));
        assert_eq!(value(&session, "C3"), Value::text("1".repeat(4096)));
        assert_eq!(value(&session, "C4"), Value::Number(1500.0));

        // Precedents of a long reference chain still drive recalculation.
        session.execute("cell.set", json!({"cell": "A750", "input": "101"})).expect("edit precedent");
        assert_eq!(value(&session, "C4"), Value::Number(1600.0));

        // Evaluate Formula walks every operator of the chain.
        let steps = session.execute("formulas.evaluateFormula", json!({"cell": "C4"})).expect("evaluate formula");
        assert_eq!(steps["steps"].as_array().map(Vec::len), Some(200));
        assert_eq!(steps["result"], json!(1600.0));
        assert_eq!(session.execute("formulas.evaluate", json!({"formula": literal_chain("+")})).expect("evaluate"), json!(4096.0));

        // Copy and paste move every reference (Excel: the pasted C4 sums A2:A1501).
        session.execute("edit.copy", json!({"range": "C4"})).expect("copy");
        session.execute("edit.paste", json!({"at": "C5"})).expect("paste");
        assert_eq!(value(&session, "C5"), Value::Number(1599.0));
        session.execute("edit.undo", json!({})).expect("undo paste");

        // Fill and structural edits rewrite every reference in the chain (Excel: an inserted
        // row above shifts A1:A1500 to A2:A1501; the sum is unchanged).
        session.execute("edit.fillRight", json!({"range": "C1:D4"})).expect("fill right");
        assert_eq!(value(&session, "D1"), Value::Number(4096.0));
        assert_eq!(value(&session, "D4"), Value::Number(0.0));
        session.execute("home.insertRows", json!({"rows": "1:1"})).expect("insert row");
        assert_eq!(value(&session, "C5"), Value::Number(1600.0));
        session.execute("home.insertColumns", json!({"cols": "A:A"})).expect("insert column");
        assert_eq!(value(&session, "D5"), Value::Number(1600.0));
        session.execute("edit.undo", json!({})).expect("undo");
        session.execute("edit.undo", json!({})).expect("undo");
        assert_eq!(value(&session, "C4"), Value::Number(1600.0));
        session.execute("edit.redo", json!({})).expect("redo");
        assert_eq!(value(&session, "C5"), Value::Number(1600.0));

        // XLSX round trip keeps the formulas and their values.
        let wb = &session.doc().expect("doc").wb;
        let bytes = io::save_bytes(wb, "chains.xlsx").expect("save");
        let (reopened, _) = io::open_bytes("chains.xlsx", &bytes).expect("open");
        let mut session = Session::new();
        session.add_document(DocState::new(reopened, None, "chains".into(), gridcraft_engine::Trust::FromFile));
        session.execute("formulas.calculateNow", json!({})).expect("recalc");
        assert_eq!(value(&session, "C2"), Value::Number(4096.0));
        assert_eq!(value(&session, "C3"), Value::Number(-4094.0));
        assert_eq!(value(&session, "C5"), Value::Number(1600.0));
    });
}
