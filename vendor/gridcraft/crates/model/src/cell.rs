//! Cells.

use std::sync::Arc;

use gridcraft_core::{RangeRef, Value};
use gridcraft_formula::Expr;
use serde::{Deserialize, Serialize};

use crate::python::PyCode;
use crate::style::StyleId;

/// A parsed formula. `text` is the canonical formula text without the leading `=`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(from = "SavedFormula")]
pub struct Formula {
    pub text: String,
    #[serde(skip)]
    pub expr: Option<Arc<Expr>>,
    /// Legacy array formula (Ctrl+Shift+Enter) covering this range; the formula lives in the
    /// top-left cell.
    pub array: Option<RangeRef>,
    /// Read from a file that marks this formula as a dynamic-array formula (cell metadata `cm`).
    /// The writer keeps the mark, so a formula Excel evaluates as an array is never saved as a
    /// legacy formula (which Excel evaluates with implicit intersection). Formulas entered or
    /// rewritten in GridCraft start without it, and the writer decides from their shape.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub dynamic: bool,
}

impl Formula {
    /// Parses `text` (with or without `=`). Unparseable formulas keep their text and evaluate to
    /// `#NAME?`.
    pub fn new(text: &str) -> Formula {
        let body = text.strip_prefix('=').unwrap_or(text);
        match gridcraft_formula::parse(body) {
            Ok(e) => Formula { text: gridcraft_formula::print(&e), expr: Some(Arc::new(e)), array: None, dynamic: false },
            Err(_) => Formula { text: body.to_string(), expr: None, array: None, dynamic: false },
        }
    }
    pub fn from_expr(e: Expr) -> Formula {
        Formula { text: gridcraft_formula::print(&e), expr: Some(Arc::new(e)), array: None, dynamic: false }
    }
    /// The parsed expression, re-parsing when no tree is cached.
    pub fn expr(&self) -> Option<Expr> {
        self.expr.as_deref().cloned().or_else(|| gridcraft_formula::parse(&self.text).ok())
    }
    /// The parsed expression without copying it (re-parses when no tree is cached).
    pub fn expr_arc(&self) -> Option<Arc<Expr>> {
        self.expr.clone().or_else(|| gridcraft_formula::parse(&self.text).ok().map(Arc::new))
    }
    /// Runs `f` on the parsed expression, borrowing the cached tree or, when none is cached,
    /// a freshly parsed one that is dropped afterwards. Unlike [`expr`](Self::expr) it never copies
    /// the tree, and unlike [`expr_arc`](Self::expr_arc) it needs no reference count or `Arc`
    /// allocation. `None` when the text does not parse.
    pub fn with_expr<R>(&self, f: impl FnOnce(&Expr) -> R) -> Option<R> {
        match &self.expr {
            Some(e) => Some(f(e)),
            None => gridcraft_formula::parse(&self.text).ok().map(|e| f(&e)),
        }
    }
}

/// A formula as a saved workbook holds it: the tree is not saved. Reading one parses its text
/// once and caches the tree, so a workbook opened from JSON does not parse every formula again on
/// each recalculation. The text is kept as saved, and text that doesn't parse caches nothing, as
/// [`Formula::new`] does.
#[derive(Deserialize)]
struct SavedFormula {
    text: String,
    array: Option<RangeRef>,
    #[serde(default)]
    dynamic: bool,
}

impl From<SavedFormula> for Formula {
    fn from(f: SavedFormula) -> Formula {
        let expr = gridcraft_formula::parse(&f.text).ok().map(Arc::new);
        Formula { text: f.text, expr, array: f.array, dynamic: f.dynamic }
    }
}

/// A static picture stored as cell content, independent of floating drawing anchors.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CellPicture {
    /// Encoded PNG/JPEG bytes, base64 in JSON. Sheet entries share the payload through Arc.
    #[serde(with = "crate::b64")]
    pub data: Vec<u8>,
    pub mime: String,
    pub alt: String,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Cell {
    /// Constant value, or the cached result of the formula or Python code.
    pub value: Value,
    pub formula: Option<Arc<Formula>>,
    pub style: StyleId,
    /// Python code, for a Python cell (which has no formula).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub py: Option<Arc<PyCode>>,
}

impl Cell {
    pub fn value(v: Value) -> Cell {
        Cell { value: v, formula: None, style: StyleId::DEFAULT, py: None }
    }
    pub fn formula(f: Formula) -> Cell {
        Cell { value: Value::Empty, formula: Some(Arc::new(f)), style: StyleId::DEFAULT, py: None }
    }
    /// A value cell with this style.
    pub fn styled(v: Value, style: StyleId) -> Cell {
        Cell { value: v, formula: None, style, py: None }
    }
    /// No value, no formula, no Python code and the default style: storing it is pointless.
    pub fn is_blank(&self) -> bool {
        self.value.is_empty() && self.formula.is_none() && self.style == StyleId::DEFAULT && self.py.is_none()
    }
    /// Whether the cell holds content (a value, a formula or Python code), formats aside.
    pub fn has_content(&self) -> bool {
        !self.value.is_empty() || self.formula.is_some() || self.py.is_some()
    }
    /// Removes the content (value, formula and Python code), keeping the style.
    pub fn clear_content(&mut self) {
        self.value = Value::Empty;
        self.formula = None;
        self.py = None;
    }
    /// What the formula bar shows: a formula, a Python cell's code, or the value.
    pub fn input_text(&self) -> String {
        if let Some(py) = &self.py {
            return py.code.clone();
        }
        if let Some(f) = &self.formula {
            return format!("={}", f.text);
        }
        match &self.value {
            Value::Text(t) => {
                // Text that would parse as something else is shown with its apostrophe by Excel
                // only when entered that way; we show it plainly.
                t.to_string()
            }
            v => v.display(),
        }
    }
    /// What the formula bar shows when not editing: [`input_text`](Self::input_text), with a
    /// legacy array formula in braces (`{=A1:A2*2}`).
    pub fn bar_text(&self) -> String {
        match &self.formula {
            Some(f) if f.array.is_some() => format!("{{={}}}", f.text),
            _ => self.input_text(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gridcraft_core::CellRef;

    #[test]
    fn formulas_read_from_json_keep_their_text_and_cache_the_tree() -> Result<(), serde_json::Error> {
        let mut array = Formula::new("=A1:B2*2");
        array.array = Some(RangeRef::new(CellRef::new(0, 2), CellRef::new(1, 3)));
        array.dynamic = true;
        for f in [Formula::new("=SUM(A1:A3)+'My Sheet'!B2"), Formula::new("=LET(x,A1*2,x+1)"), array] {
            let back: Formula = serde_json::from_str(&serde_json::to_string(&f)?)?;
            assert!(back.expr.is_some(), "{}: the tree is cached after reading", f.text);
            assert_eq!(back, f);
        }
        // Saved text is kept as written, even when the canonical print differs, and its tree is
        // the one the text parses to.
        let back: Formula = serde_json::from_str(r#"{"text": "sum( a1 , 2 )", "array": null}"#)?;
        assert_eq!(back.text, "sum( a1 , 2 )");
        assert!(!back.dynamic);
        assert_eq!(back.expr.as_deref(), gridcraft_formula::parse("sum( a1 , 2 )").ok().as_ref());
        assert!(back.expr.is_some());
        // Text that doesn't parse caches nothing, as `Formula::new` does.
        let bad = Formula::new("=SUM(");
        let back: Formula = serde_json::from_str(&serde_json::to_string(&bad)?)?;
        assert_eq!(back, bad);
        assert!(back.expr.is_none());
        Ok(())
    }

    #[test]
    fn a_workbook_read_from_json_has_every_formula_tree() -> Result<(), Box<dyn std::error::Error>> {
        let mut wb = crate::Workbook::new();
        let mut array = Formula::new("=A1:A3*2");
        array.array = Some(RangeRef::new(CellRef::new(0, 3), CellRef::new(2, 3)));
        let sheet = wb.sheet_mut(0).ok_or("no sheet")?;
        sheet.cells.set(CellRef::new(0, 0), Cell::value(Value::Number(1.0)));
        sheet.cells.set(CellRef::new(0, 1), Cell::formula(Formula::new("=SUM(A1:A3)")));
        sheet.cells.set(CellRef::new(1, 1), Cell::formula(Formula::new("=IF(A1>0,\"yes\",\"no\")")));
        sheet.cells.set(CellRef::new(2, 1), Cell::formula(Formula::new("=SUM(")));
        sheet.cells.set(CellRef::new(0, 3), Cell::formula(array));
        let json = serde_json::to_string(&wb)?;
        let back: crate::Workbook = serde_json::from_str(&json)?;
        let (before, after) = (wb.sheet(0).ok_or("no sheet")?, back.sheet(0).ok_or("no sheet")?);
        let mut formulas = 0;
        for (c, cell) in before.cells.iter() {
            let (Some(f), Some(read)) = (&cell.formula, after.cell(c).and_then(|cell| cell.formula.as_ref())) else { continue };
            assert_eq!(read, f, "{c:?}: text, tree and flags as saved");
            assert_eq!(read.expr.is_some(), gridcraft_formula::parse(&f.text).is_ok(), "{c:?}: every formula that parses has its tree");
            formulas += 1;
        }
        assert_eq!(formulas, 4);
        Ok(())
    }
}
