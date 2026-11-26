//! "Set precision as displayed" (File › Options › Advanced; `fullPrecision="0"` in `.xlsx`,
//! CalcPrecision in `.xls`): with it on, a number is stored as its cell's number format shows it,
//! and formulas read those rounded values. The rounding itself is the number format crate's
//! ([`gridcraft_numfmt::NumberFormat::round_as_displayed`]); this module looks up each cell's
//! format, parsing every style's code once.

use std::collections::HashMap;

use gridcraft_core::{CellRef, Value};
use gridcraft_model::{StyleId, Workbook};
use gridcraft_numfmt::NumberFormat;

/// Rounds values to their cell's displayed precision, remembering each style's parsed format.
/// Styles don't change while it is used (one recalculation pass, one sweep of a workbook).
#[derive(Default)]
pub struct DisplayedPrecision {
    /// Parsed format per style; `None` for General, which keeps the full value.
    formats: HashMap<StyleId, Option<NumberFormat>>,
}

impl DisplayedPrecision {
    pub fn new() -> Self {
        Self::default()
    }

    /// `n` as the format of `c` on sheet `sheet` displays it (the cell's own style, else its
    /// row's, else its column's, as the grid shows it).
    pub fn round(&mut self, wb: &Workbook, sheet: usize, c: CellRef, n: f64) -> f64 {
        let Some(sh) = wb.sheet(sheet) else { return n };
        let id = sh.style_id(c);
        let fmt = self.formats.entry(id).or_insert_with(|| {
            let f = NumberFormat::parse_cached(wb.styles.get(id).num_fmt.as_str());
            (!f.is_general()).then_some(f)
        });
        match fmt {
            Some(f) => f.round_as_displayed(n),
            None => n,
        }
    }

    /// Like [`DisplayedPrecision::round`] for a value: only numbers change.
    pub fn round_value(&mut self, wb: &Workbook, sheet: usize, c: CellRef, v: Value) -> Value {
        match v {
            Value::Number(n) => {
                let r = self.round(wb, sheet, c, n);
                // Bit-equal results keep the value as it was (no needless change for undo/diffs).
                if r.to_bits() == n.to_bits() { Value::Number(n) } else { Value::Number(r) }
            }
            v => v,
        }
    }
}
