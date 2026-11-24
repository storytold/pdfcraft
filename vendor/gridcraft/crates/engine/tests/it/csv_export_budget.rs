//! CSV and TSV export is capped: two cells far apart in a sheet would otherwise make a file as
//! big as the whole rectangle between them. A refused export is an error and writes nothing.

use gridcraft_engine::Session;
use serde_json::json;

fn two_far_cells() -> Session {
    let mut s = Session::new();
    s.new_workbook();
    s.execute("cell.set", json!({"cell": "A1", "input": "a"})).unwrap();
    s.execute("cell.set", json!({"cell": "XFD1048576", "input": "b"})).unwrap();
    s
}

#[test]
fn a_sparse_sheet_spanning_the_grid_is_refused_not_expanded() {
    let mut s = two_far_cells();
    for format in ["csv", "tsv"] {
        let err = s.execute("file.saveBytes", json!({"format": format})).unwrap_err().to_string();
        assert!(err.contains("1048576 rows by 16384 columns") && err.contains("MiB"), "{format}: {err}");
    }
    // Save As and Export write no file, and the document stays as it was.
    let dir = std::env::temp_dir().join(format!("gridcraft-csv-budget-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("big.csv");
    let err = s.execute("file.exportCsv", json!({"path": path.to_str().unwrap()})).unwrap_err().to_string();
    assert!(err.contains("over the limit"), "{err}");
    assert!(!path.exists(), "nothing is written for a refused export");
    std::fs::remove_dir_all(&dir).unwrap();
    // Other formats are unaffected, and so is a small export from the same session.
    assert!(s.execute("file.saveBytes", json!({"format": "xlsx"})).is_ok());
    s.execute("cell.set", json!({"cell": "XFD1048576", "input": ""})).unwrap();
    assert_eq!(s.execute("file.saveBytes", json!({"format": "csv"})).unwrap()["bytes"], 3);
}

#[test]
fn exports_below_the_cap_keep_their_positions() {
    for (corner, bytes) in [("C3", 14), ("Z10", 272), ("XFD8", 131_082)] {
        let mut s = Session::new();
        s.new_workbook();
        s.execute("cell.set", json!({"cell": "A1", "input": "a"})).unwrap();
        s.execute("cell.set", json!({"cell": corner, "input": "b"})).unwrap();
        assert_eq!(s.execute("file.saveBytes", json!({"format": "csv"})).unwrap()["bytes"], bytes, "{corner}");
    }
}
