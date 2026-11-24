//! TRIMRANGE: a range or array without its blank outer rows and columns (Microsoft's
//! documented semantics: trim_rows / trim_cols 0 none, 1 leading, 2 trailing, 3 both).

use gridcraft_core::{Array, CellError, CellRef, RangeRef, Value};
use gridcraft_formula::{Expr, TrimMode};

use crate::eval::{Area, Ev, Evaluator, MAX_CELLS};

/// (leading, trailing) for a trim_rows / trim_cols code.
fn mode(code: f64) -> Result<(bool, bool), CellError> {
    match code.trunc() {
        0.0 => Ok((false, false)),
        1.0 => Ok((true, false)),
        2.0 => Ok((false, true)),
        3.0 => Ok((true, true)),
        _ => Err(CellError::Value),
    }
}

/// Rows and columns (first, last) holding a non-blank value in a row-major grid.
fn bounds(values: &[Value], width: usize) -> Option<((usize, usize), (usize, usize))> {
    let mut b: Option<((usize, usize), (usize, usize))> = None;
    for (i, v) in values.iter().enumerate() {
        if matches!(v, Value::Empty) {
            continue;
        }
        let (r, c) = (i / width.max(1), i % width.max(1));
        b = Some(match b {
            None => ((r, r), (c, c)),
            Some(((r0, r1), (c0, c1))) => ((r0.min(r), r1.max(r)), (c0.min(c), c1.max(c))),
        });
    }
    b
}

/// The kept span of `0..len` given the non-blank span and the trim flags.
fn keep(len: usize, nonblank: (usize, usize), (lead, trail): (bool, bool)) -> (usize, usize) {
    (if lead { nonblank.0 } else { 0 }, if trail { nonblank.1 } else { len.saturating_sub(1) })
}

impl Evaluator<'_> {
    pub(crate) fn trimrange(&mut self, args: &[Expr]) -> Ev {
        match self.trimrange_inner(args) {
            Ok(ev) => ev,
            Err(e) => Ev::V(Value::Error(e)),
        }
    }

    /// A trim reference `A1.:E10` / `A1:.E10` / `A1.:.E10`: `TRIMRANGE(range, code, code)`.
    pub(crate) fn trim_ref(&mut self, mode: TrimMode, x: &Expr) -> Ev {
        let flags = match mode {
            TrimMode::Leading => (true, false),
            TrimMode::Trailing => (false, true),
            TrimMode::All => (true, true),
        };
        let ev = self.eval(x);
        match self.trim_ev(ev, flags, flags) {
            Ok(ev) => ev,
            Err(e) => Ev::V(Value::Error(e)),
        }
    }

    fn trim_mode(&mut self, args: &[Expr], i: usize) -> Result<(bool, bool), CellError> {
        match args.get(i) {
            None | Some(Expr::Missing) => Ok((true, true)),
            Some(e) => match self.value(e).scalar() {
                Value::Error(e) => Err(e),
                v => mode(v.to_number()?),
            },
        }
    }

    fn trimrange_inner(&mut self, args: &[Expr]) -> Result<Ev, CellError> {
        let first = match args {
            [first] | [first, _] | [first, _, _] => first,
            _ => return Err(CellError::Value),
        };
        let rows = self.trim_mode(args, 1)?;
        let cols = self.trim_mode(args, 2)?;
        let ev = self.eval(first);
        self.trim_ev(ev, rows, cols)
    }

    fn trim_ev(&mut self, ev: Ev, rows: (bool, bool), cols: (bool, bool)) -> Result<Ev, CellError> {
        let none = rows == (false, false) && cols == (false, false);
        match ev {
            Ev::R(areas) => {
                let [a] = areas.as_slice() else { return Err(CellError::Value) };
                if none {
                    return Ok(Ev::R(vec![*a]));
                }
                // Cells past the used range are blank: only the used part is read.
                let part = self.trim(a);
                let (h, w) = (part.height() as usize, part.width() as usize);
                if (h as u64) * (w as u64) > MAX_CELLS {
                    return Err(CellError::Num);
                }
                let values = self.host.range_values(a.sheet, part);
                let ((r0, r1), (c0, c1)) = bounds(&values, w).ok_or(CellError::Calc)?;
                let (h_all, w_all) = (a.range.height() as usize, a.range.width() as usize);
                let (kr0, kr1) = keep(h_all, (r0, r1), rows);
                let (kc0, kc1) = keep(w_all, (c0, c1), cols);
                let at = |r: usize, c: usize| CellRef::new(a.range.start.row.saturating_add(r as u32), a.range.start.col.saturating_add(c as u32));
                Ok(Ev::R(vec![Area { sheet: a.sheet, range: RangeRef::new(at(kr0, kc0), at(kr1, kc1)) }]))
            }
            Ev::V(Value::Array(arr)) => {
                if none {
                    return Ok(Ev::V(Value::Array(arr)));
                }
                let ((r0, r1), (c0, c1)) = bounds(&arr.data, arr.cols).ok_or(CellError::Calc)?;
                let (kr0, kr1) = keep(arr.rows, (r0, r1), rows);
                let (kc0, kc1) = keep(arr.cols, (c0, c1), cols);
                let mut data = Vec::with_capacity((kr1 + 1 - kr0) * (kc1 + 1 - kc0));
                for r in kr0..=kr1 {
                    for c in kc0..=kc1 {
                        data.push(arr.get(r, c).cloned().unwrap_or(Value::Empty));
                    }
                }
                Array::new(kr1 + 1 - kr0, kc1 + 1 - kc0, data).map(|a| Ev::V(Value::from(a))).ok_or(CellError::Value)
            }
            Ev::V(Value::Error(e)) => Err(e),
            Ev::V(Value::Empty) if !none => Err(CellError::Calc),
            Ev::V(v) => Ok(Ev::V(v)),
            Ev::L(_) => Err(CellError::Value),
        }
    }
}
