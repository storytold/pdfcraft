//! Performance scenarios on large workbooks (release build):
//! `cargo run --release -p gridcraft-engine --example perf -- [rows]`

#![allow(clippy::disallowed_methods)] // a native-only benchmark: wasm never runs it

use std::time::Instant;

use gridcraft_engine::Session;
use serde_json::json;

fn main() {
    let rows: usize = std::env::args().nth(1).and_then(|s| s.parse().ok()).unwrap_or(100_000);
    let mut s = Session::new();
    s.new_workbook();
    // Data: rows × 6 numeric columns, built through range.setValues in chunks.
    let t = Instant::now();
    let chunk = 10_000;
    let mut r = 0;
    while r < rows {
        let n = chunk.min(rows - r);
        let vals: Vec<serde_json::Value> = (0..n)
            .map(|i| json!([format!("Item {}", r + i), (r + i) as f64, ((r + i) % 97) as f64, ((r + i) % 13) as f64 * 1.5, 1.0, 2.0]))
            .collect();
        s.execute("range.setValues", json!({"range": format!("A{}", r + 1), "values": vals})).expect("set");
        r += n;
    }
    println!("fill {rows}x6 values: {:?}", t.elapsed());
    // Formulas: G = B*C+D, H = running sum of G (chain).
    let t = Instant::now();
    s.execute("cell.set", json!({"cell": "G1", "input": "=B1*C1+D1"})).expect("g1");
    s.execute("edit.autoFill", json!({"source": "G1", "target": format!("G1:G{rows}")})).expect("fill g");
    println!("fill {rows} formulas G=B*C+D: {:?}", t.elapsed());
    let t = Instant::now();
    s.execute("cell.set", json!({"cell": "H1", "input": "=G1"})).expect("h1");
    s.execute("cell.set", json!({"cell": "H2", "input": "=H1+G2"})).expect("h2");
    s.execute("edit.autoFill", json!({"source": "H2", "target": format!("H2:H{rows}")})).expect("fill h");
    println!("fill {rows} chained running-sum formulas: {:?}", t.elapsed());
    let t = Instant::now();
    s.execute("cell.set", json!({"cell": "J1", "input": format!("=SUM(G1:G{rows})")})).expect("sum");
    s.execute("cell.set", json!({"cell": "J2", "input": "=SUMIFS(B:B,C:C,\">50\")"})).expect("sumifs");
    s.execute("cell.set", json!({"cell": "J3", "input": "=XLOOKUP(\"Item 77777\",A:A,B:B)"})).expect("xlookup");
    println!("aggregate formulas: {:?}", t.elapsed());
    // Edit at the top: everything below recalculates.
    let t = Instant::now();
    s.execute("cell.set", json!({"cell": "B1", "input": "5"})).expect("edit");
    println!("edit B1 → recalc {} cells: {:?}", s.doc().map(|d| d.calc.last_recalc_cells).unwrap_or(0), t.elapsed());
    let t = Instant::now();
    s.execute("data.sortDescending", json!({"range": format!("A1:F{rows}"), "column": "C"})).expect("sort");
    println!("sort {rows} rows: {:?}", t.elapsed());
    let t = Instant::now();
    let r = s.execute("file.saveBytes", json!({"format": "xlsx"})).expect("save");
    let b64 = r["base64"].as_str().unwrap_or("").to_string();
    println!("save xlsx ({} bytes): {:?}", r["bytes"], t.elapsed());
    let t = Instant::now();
    s.execute("file.open", json!({"name": "big.xlsx", "base64": b64})).expect("open");
    println!("open xlsx + full recalc: {:?}", t.elapsed());
    let t = Instant::now();
    s.execute("edit.undo", json!({})).ok();
    println!("undo: {:?}", t.elapsed());
}
