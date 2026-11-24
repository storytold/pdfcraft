//! XLSX save timing on a generated workbook of realistic shape (release build):
//! `cargo run --release -p gridcraft-xlsx --example write_perf -- [rows] [runs] [out.xlsx]`
//!
//! The workbook mixes what a large business sheet holds: repeated and unique text, integers,
//! decimals, dates, booleans, formulas with cached values, a few styles, a header row and a
//! second summary sheet. Prints each run and the median / minimum of `write_xlsx`.
#![allow(clippy::disallowed_methods)] // a native-only benchmark: wasm never runs it

use std::time::Instant;

use gridcraft_core::{CellRef, Value};
use gridcraft_model::{Cell, Formula, NumFmt, Sheet, StyleId, Workbook};

fn build(rows: u32) -> Workbook {
    let mut wb = Workbook::new();
    let bold = wb.styles.derive(StyleId::DEFAULT, |s| s.font.bold = true);
    let money = wb.styles.derive(StyleId::DEFAULT, |s| s.num_fmt = NumFmt::new("#,##0.00"));
    let date = wb.styles.derive(StyleId::DEFAULT, |s| s.num_fmt = NumFmt::new("yyyy-mm-dd"));
    let pct = wb.styles.derive(StyleId::DEFAULT, |s| s.num_fmt = NumFmt::new("0.0%"));
    let regions = ["North", "South", "East", "West", "Central"];
    let products = ["Widget", "Gadget", "Gizmo", "Doohickey", "Sprocket", "Thingamajig", "Whatsit"];
    let mut s = Sheet::new("Sales");
    let set = |s: &mut Sheet, row: u32, col: u32, value: Value, style: StyleId| {
        s.cells.set(CellRef::new(row, col), Cell { value, formula: None, style, py: None });
    };
    for (col, title) in ["Order", "Date", "Region", "Product", "Units", "Price", "Revenue", "Share", "Paid", "Note"].iter().enumerate() {
        set(&mut s, 0, col as u32, Value::text(*title), bold);
    }
    for r in 1..=rows {
        let i = r as usize;
        set(&mut s, r, 0, Value::text(format!("ORD-{:07}", r)), StyleId::DEFAULT);
        set(&mut s, r, 1, Value::Number(45000.0 + (i % 730) as f64), date);
        set(&mut s, r, 2, Value::text(regions[i % regions.len()]), StyleId::DEFAULT);
        set(&mut s, r, 3, Value::text(products[i % products.len()]), StyleId::DEFAULT);
        let units = (i * 7 % 250 + 1) as f64;
        let price = ((i * 37 % 10_000) as f64) / 100.0 + 0.99;
        set(&mut s, r, 4, Value::Number(units), StyleId::DEFAULT);
        set(&mut s, r, 5, Value::Number(price), money);
        let mut rev = Cell::formula(Formula::new(&format!("=E{0}*F{0}", r + 1)));
        rev.value = Value::Number(units * price);
        rev.style = money;
        s.cells.set(CellRef::new(r, 6), rev);
        let mut share = Cell::formula(Formula::new(&format!("=G{}/Summary!$B$1", r + 1)));
        share.value = Value::Number(units * price / 1.0e7);
        share.style = pct;
        s.cells.set(CellRef::new(r, 7), share);
        set(&mut s, r, 8, Value::Bool(!i.is_multiple_of(3)), StyleId::DEFAULT);
        if i.is_multiple_of(10) {
            set(&mut s, r, 9, Value::text(format!("Follow up with customer #{} & check <terms>", i % 977)), StyleId::DEFAULT);
        }
    }
    let mut summary = Sheet::new("Summary");
    let mut total = Cell::formula(Formula::new(&format!("=SUM(Sales!G2:G{})", rows + 1)));
    total.value = Value::Number(1.0e7);
    summary.cells.set(CellRef::new(0, 1), total);
    summary.cells.set(CellRef::new(0, 0), Cell { value: Value::text("Total"), formula: None, style: bold, py: None });
    for (k, region) in regions.iter().enumerate() {
        let row = k as u32 + 2;
        summary.cells.set(CellRef::new(row, 0), Cell { value: Value::text(*region), formula: None, style: StyleId::DEFAULT, py: None });
        let mut f = Cell::formula(Formula::new(&format!("=SUMIFS(Sales!G:G,Sales!C:C,A{})", row + 1)));
        f.value = Value::Number(2.0e6);
        f.style = money;
        summary.cells.set(CellRef::new(row, 1), f);
    }
    wb.sheets = vec![std::sync::Arc::new(s), std::sync::Arc::new(summary)];
    wb
}

fn main() {
    let mut args = std::env::args().skip(1);
    let rows: u32 = args.next().and_then(|s| s.parse().ok()).unwrap_or(200_000);
    let runs: usize = args.next().and_then(|s| s.parse().ok()).unwrap_or(7).max(1);
    let out = args.next();
    let wb = build(rows);
    let mut times = vec![];
    let mut bytes = vec![];
    for i in 0..runs {
        let t = Instant::now();
        bytes = gridcraft_xlsx::write_xlsx(&wb).expect("write");
        let dt = t.elapsed().as_secs_f64() * 1000.0;
        println!("run {i}: {dt:.1} ms ({} bytes)", bytes.len());
        times.push(dt);
    }
    times.sort_by(f64::total_cmp);
    println!("write_xlsx {rows} rows: median {:.1} ms, min {:.1} ms", times[times.len() / 2], times[0]);
    if let Some(path) = out {
        std::fs::write(&path, &bytes).expect("write output");
    }
}
