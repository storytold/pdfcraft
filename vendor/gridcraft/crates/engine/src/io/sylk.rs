//! SYLK (`.slk`) import. `craft_csv::sylk` reads the file (values, formulas rewritten from R1C1 to
//! A1, number formats, column widths); this module makes it a one-sheet workbook: formulas parsed
//! (a formula GridCraft can't parse is skipped and counted), error values recognised, formats
//! interned as styles and widths converted from characters.

use std::collections::HashMap;
use std::sync::Arc;

use craft_csv::sylk;
use gridcraft_core::{CellError, CellRef, Value};
use gridcraft_model::{Cell, Formula, NumFmt, Sheet, Style, Workbook};

use crate::{EngineError, Result};

/// Reads a SYLK file into a one-sheet workbook. Returns the workbook and its warnings.
pub fn read(name: &str, bytes: &[u8]) -> Result<(Workbook, Vec<String>)> {
    let file = sylk::read(bytes).map_err(|e| match e {
        sylk::Error::TooLarge => EngineError::Other(format!("We can't open '{name}': the SYLK file is larger than {} MB.", sylk::MAX_BYTES >> 20)),
        _ => EngineError::Other(format!("We can't open '{name}': it isn't a SYLK file (it doesn't start with an ID record).")),
    })?;
    let mut wb = Workbook::new();
    let mut sheet = Sheet::new("Sheet1");
    let mut skipped = file.skipped;
    let mut styles = HashMap::new();
    for (&(row, col), c) in &file.cells {
        let value = match &c.value {
            sylk::Value::Number(n) => Value::Number(*n),
            sylk::Value::Text(s) => Value::Text(Arc::from(s.as_str())),
            sylk::Value::Bool(b) => Value::Bool(*b),
            sylk::Value::Error(s) => CellError::parse(s).map_or_else(|| Value::Text(Arc::from(s.as_str())), Value::Error),
            _ => Value::Empty,
        };
        let mut cell = Cell::value(value);
        if let Some(a1) = &c.formula {
            let f = Formula::new(a1);
            if f.expr.is_some() {
                cell.formula = Some(Arc::new(f));
            } else {
                skipped += 1;
            }
        }
        if let Some(code) = &c.format {
            cell.style = *styles
                .entry(code.as_str())
                .or_insert_with(|| wb.styles.intern(Style { num_fmt: NumFmt(Arc::from(code.as_str())), ..Style::default() }));
        }
        sheet.set_cell(CellRef::new(row, col), cell);
    }
    for w in &file.column_widths {
        let size = crate::cmd::format::chars_to_points(w.width) as f32;
        for c in w.first..=w.last {
            sheet.cols.entry(c).or_default().size = Some(size);
        }
    }
    if let Some(first) = wb.sheet_mut(0) {
        sheet.name = std::mem::take(&mut first.name);
        *first = sheet;
    }
    let mut warnings = Vec::new();
    if skipped > 0 {
        warnings.push(format!("{name}: {skipped} SYLK records GridCraft couldn't read were skipped"));
    }
    Ok((wb, warnings))
}
