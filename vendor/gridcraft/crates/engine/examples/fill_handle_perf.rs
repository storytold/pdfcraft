//! Times the fill-handle double-click scan against a densely-filled neighbour column: the old
//! row-by-row `value_ref` loop from `grid.rs` vs. `Sheet::last_contiguous_nonempty`.
//! `cargo run --release -p gridcraft-engine --example fill_handle_perf -- [rows]`

#![allow(clippy::disallowed_methods)] // a native-only benchmark: wasm never runs it

use std::time::Instant;

use gridcraft_engine::Session;
use gridcraft_engine::core::CellRef;
use serde_json::json;

fn main() {
    let rows: u32 = std::env::args().nth(1).and_then(|s| s.parse().ok()).unwrap_or(1_000_000);
    let mut s = Session::new();
    s.new_workbook();
    let t = Instant::now();
    let chunk = 10_000u32;
    let mut r = 0u32;
    while r < rows {
        let n = chunk.min(rows - r);
        let vals: Vec<serde_json::Value> = (0..n).map(|i| json!([(r + i) as f64])).collect();
        s.execute("range.setValues", json!({"range": format!("A{}", r + 1), "values": vals})).expect("set");
        r += n;
    }
    println!("fill A1:A{rows}: {:?}", t.elapsed());

    let d = s.doc().expect("doc");
    let sh = d.wb.active().expect("sheet");
    let neighbour = 0u32; // column A
    let cap = (rows - 1).min(999_999);

    let t = Instant::now();
    let mut last = 0u32;
    let mut iters = 0u64;
    while last + 1 < gridcraft_engine::core::MAX_ROWS && sh.value_ref(CellRef::new(last + 1, neighbour)).is_some_and(|v| !v.is_empty()) && last < cap
    {
        last += 1;
        iters += 1;
    }
    let old = t.elapsed();
    println!("old row-by-row scan:          {iters} iterations, {old:?} ({:.1} ns/iter)", old.as_nanos() as f64 / iters.max(1) as f64);

    let t = Instant::now();
    let new_last = sh.last_contiguous_nonempty(neighbour, 1, cap);
    let new_t = t.elapsed();
    println!("new last_contiguous_nonempty: last={new_last}, {new_t:?} ({:.2}x faster)", old.as_secs_f64() / new_t.as_secs_f64().max(1e-12));

    assert_eq!(last, new_last, "old and new scans must agree");
}
