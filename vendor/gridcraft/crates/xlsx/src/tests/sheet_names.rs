//! Sheet names XLSX cannot hold are refused on export instead of breaking formulas on reopen.

use std::sync::Arc;

use gridcraft_model::{Sheet, Workbook};

use crate::{IoError, write_xlsx};

fn book(names: &[&str]) -> Workbook {
    let mut wb = Workbook::new();
    wb.sheets = names.iter().map(|n| Arc::new(Sheet::new(*n))).collect();
    wb
}

#[test]
fn unrepresentable_sheet_names_are_refused() {
    let long_a = format!("{}X", "A".repeat(31));
    let long_b = format!("{}Y", "A".repeat(31));
    for names in [vec!["Data", "a/b"], vec![long_a.as_str(), long_b.as_str()], vec!["Data", "data"], vec!["Data", ""]] {
        assert!(matches!(write_xlsx(&book(&names)), Err(IoError::Format(_))), "{names:?}");
    }
    assert!(write_xlsx(&book(&["Data", "Other"])).is_ok());
}

/// A saved sheet keeps its `sheetId`; a copied or new sheet gets one no other sheet asks for.
#[test]
fn sheet_ids_survive_a_save() {
    let mut wb = book(&["A", "B", "C", "D"]);
    for (sheet, id) in wb.sheets.iter_mut().zip([3, 7, 0, 3]) {
        Arc::make_mut(sheet).file_id = id;
    }
    let (read, _) = crate::read_xlsx(&write_xlsx(&wb).unwrap()).unwrap();
    let ids: Vec<u32> = read.sheets.iter().map(|s| s.file_id).collect();
    assert_eq!(ids, [3, 7, 1, 2]);
    let (again, _) = crate::read_xlsx(&write_xlsx(&read).unwrap()).unwrap();
    assert_eq!(again.sheets.iter().map(|s| s.file_id).collect::<Vec<_>>(), ids);
}

/// Repairing repeated names takes one set lookup per candidate, so thousands of sheets sharing
/// a name are renamed without rescanning the workbook for every suffix tried.
#[test]
fn repeated_sheet_names_are_repaired_without_rescans() {
    let mut names = crate::read::SheetNames::default();
    assert_eq!(names.unique("Data", 0), "Data");
    assert_eq!(names.unique("data", 1), "data (2)");
    assert_eq!(names.unique("", 2), "Sheet3");
    assert_eq!(names.unique("Data (3)", 3), "Data (3)");
    for i in 4..100_000 {
        assert_eq!(names.unique("Data", i), format!("Data ({i})"));
    }
}
