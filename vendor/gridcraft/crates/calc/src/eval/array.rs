use super::{Array, BinOp, CellError, MAX_CELLS, Value, compare};

/// The aggregates that can fold a reference without building an array.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Agg {
    Sum,
    Count,
    Average,
}

impl Agg {
    pub(super) fn of(name: &str) -> Option<Agg> {
        match name {
            "SUM" => Some(Agg::Sum),
            "COUNT" => Some(Agg::Count),
            "AVERAGE" => Some(Agg::Average),
            _ => None,
        }
    }
}

/// What a folded aggregate carries from one value to the next.
#[derive(Default)]
pub(super) struct Acc {
    pub(super) sum: f64,
    pub(super) count: u64,
}

impl Acc {
    /// Takes a value found inside a reference: only numbers count. `Err` is an error value that
    /// ends `SUM` and `AVERAGE`; `COUNT` skips errors.
    pub(super) fn add_cell(&mut self, kind: Agg, v: &Value) -> Result<(), CellError> {
        match v {
            Value::Number(n) => {
                self.sum += n;
                self.count = self.count.saturating_add(1);
                Ok(())
            }
            Value::Error(e) if kind != Agg::Count => Err(*e),
            _ => Ok(()),
        }
    }

    pub(super) fn finish(&self, kind: Agg) -> Value {
        match kind {
            Agg::Sum => Value::number(self.sum),
            Agg::Count => Value::number(self.count as f64),
            Agg::Average if self.count == 0 => Value::Error(CellError::Div0),
            Agg::Average => Value::number(self.sum / self.count as f64),
        }
    }
}

// ---------------------------------------------------------------- operators

pub(super) fn map_unary(v: &Value, f: &dyn Fn(f64) -> Value, plus: bool, sys: gridcraft_core::DateSystem) -> Value {
    match v {
        Value::Array(a) => Value::from(Array { rows: a.rows, cols: a.cols, data: a.data.iter().map(|x| map_unary(x, f, plus, sys)).collect() }),
        Value::Error(e) => Value::Error(*e),
        Value::Text(_) if plus => v.clone(),
        other => match other.to_number_in(sys) {
            Ok(n) => f(n),
            Err(e) => Value::Error(e),
        },
    }
}

/// Element-wise combination with Excel's broadcasting.
pub fn zip2(a: &Value, b: &Value, f: impl Fn(&Value, &Value) -> Value) -> Value {
    match (a, b) {
        (Value::Array(x), Value::Array(y)) => {
            let rows = broadcast_dim(x.rows, y.rows);
            let cols = broadcast_dim(x.cols, y.cols);
            let Some(n) = broadcast_len(rows, cols) else { return Value::Error(CellError::Num) };
            let mut data = Vec::with_capacity(n);
            for r in 0..rows {
                for c in 0..cols {
                    data.push(f(&x.get_broadcast(r, c), &y.get_broadcast(r, c)));
                }
            }
            Value::from(Array { rows, cols, data })
        }
        (Value::Array(x), s) => Value::from(Array { rows: x.rows, cols: x.cols, data: x.data.iter().map(|v| f(v, s)).collect() }),
        (s, Value::Array(y)) => Value::from(Array { rows: y.rows, cols: y.cols, data: y.data.iter().map(|v| f(s, v)).collect() }),
        (x, y) => f(x, y),
    }
}

/// [`zip2`] that stops at the first cell `f` refuses (`None`), before the rest is built: the
/// result grows as cells are made rather than being reserved for the whole broadcast up front.
fn try_zip2(a: &Value, b: &Value, mut f: impl FnMut(&Value, &Value) -> Option<Value>) -> Option<Value> {
    Some(match (a, b) {
        (Value::Array(x), Value::Array(y)) => {
            let rows = broadcast_dim(x.rows, y.rows);
            let cols = broadcast_dim(x.cols, y.cols);
            if broadcast_len(rows, cols).is_none() {
                return Some(Value::Error(CellError::Num));
            }
            let mut data = Vec::new();
            for r in 0..rows {
                for c in 0..cols {
                    data.push(f(&x.get_broadcast(r, c), &y.get_broadcast(r, c))?);
                }
            }
            Value::from(Array { rows, cols, data })
        }
        (Value::Array(x), s) => Value::from(Array { rows: x.rows, cols: x.cols, data: x.data.iter().map(|v| f(v, s)).collect::<Option<_>>()? }),
        (s, Value::Array(y)) => Value::from(Array { rows: y.rows, cols: y.cols, data: y.data.iter().map(|v| f(s, v)).collect::<Option<_>>()? }),
        (x, y) => f(x, y)?,
    })
}

/// Cell count of a `rows`×`cols` broadcast result, or `None` when it would exceed [`MAX_CELLS`].
/// Each operand passes its own size cap, but their broadcast product can be far larger
/// (`SEQUENCE(1048576)*SEQUENCE(1,16384)` is 17 billion cells), so it is checked before allocating.
/// The product is formed in `u64`: on wasm32 `usize` is 32 bits, where a `usize` product above
/// `u32::MAX` would wrap (2^32 cells → 0) and slip past the cap.
pub(super) fn broadcast_len(rows: usize, cols: usize) -> Option<usize> {
    let cells = u64::try_from(rows).ok()?.checked_mul(u64::try_from(cols).ok()?)?;
    if cells > MAX_CELLS {
        return None;
    }
    usize::try_from(cells).ok()
}

fn broadcast_dim(a: usize, b: usize) -> usize {
    if a == 1 {
        b
    } else if b == 1 {
        a
    } else {
        a.max(b)
    }
}

pub(super) fn zip3(c: &Array, t: &Value, f: &Value, g: impl Fn(&Value, &Value, &Value) -> Value) -> Value {
    let dims = |v: &Value| if let Value::Array(a) = v { (a.rows, a.cols) } else { (1, 1) };
    let (tr, tc) = dims(t);
    let (fr, fc) = dims(f);
    let rows = broadcast_dim(broadcast_dim(c.rows, tr), fr);
    let cols = broadcast_dim(broadcast_dim(c.cols, tc), fc);
    let Some(n) = broadcast_len(rows, cols) else { return Value::Error(CellError::Num) };
    let get = |v: &Value, r: usize, col: usize| if let Value::Array(a) = v { a.get_broadcast(r, col) } else { v.clone() };
    let mut data = Vec::with_capacity(n);
    for r in 0..rows {
        for col in 0..cols {
            data.push(g(&c.get_broadcast(r, col), &get(t, r, col), &get(f, r, col)));
        }
    }
    Value::from(Array { rows, cols, data })
}

/// One cell of a LAMBDA helper's result, or `None` for an array of more than one value: arrays
/// can't nest, so the whole helper gives `#CALC!`, as in Excel.
pub(super) fn single(v: Value) -> Option<Value> {
    match v {
        Value::Array(a) if a.data.len() > 1 => None,
        v => Some(v.scalar()),
    }
}

/// Text bytes that one `&` over arrays may produce in all. Each piece is capped on its own, but a
/// compact formula can broadcast a long text across millions of cells
/// (`SEQUENCE(4000,4000)&REPT("x",30000)` is 480 GB): past this the whole result is `#NUM!`, the
/// error for a result too large to hold, before the rest is built.
pub(super) const MAX_CONCAT_ARRAY_BYTES: usize = 512 * 1024 * 1024;

pub fn binary_values(op: BinOp, x: &Value, y: &Value, sys: gridcraft_core::DateSystem) -> Value {
    if op == BinOp::Concat && (matches!(x, Value::Array(_)) || matches!(y, Value::Array(_))) {
        return concat_arrays(x, y, sys, MAX_CONCAT_ARRAY_BYTES);
    }
    zip2(x, y, |a, b| binary_scalar(op, a, b, sys))
}

/// `x & y` with at least one array operand, giving `#NUM!` once the texts built so far pass `budget`
/// bytes.
fn concat_arrays(x: &Value, y: &Value, sys: gridcraft_core::DateSystem, budget: usize) -> Value {
    let mut used = 0usize;
    try_zip2(x, y, |a, b| {
        let v = binary_scalar(BinOp::Concat, a, b, sys);
        if let Value::Text(t) = &v {
            used = used.saturating_add(t.len());
            if used > budget {
                return None;
            }
        }
        Some(v)
    })
    .unwrap_or(Value::Error(CellError::Num))
}

fn binary_scalar(op: BinOp, a: &Value, b: &Value, sys: gridcraft_core::DateSystem) -> Value {
    if let Value::Error(e) = a {
        return Value::Error(*e);
    }
    if let Value::Error(e) = b {
        return Value::Error(*e);
    }
    let num = |f: fn(f64, f64) -> Value| match (a.to_number_in(sys), b.to_number_in(sys)) {
        (Ok(x), Ok(y)) => f(x, y),
        (Err(e), _) | (_, Err(e)) => Value::Error(e),
    };
    use std::cmp::Ordering::*;
    match op {
        BinOp::Add => num(|x, y| Value::number(x + y)),
        BinOp::Sub => num(|x, y| Value::number(x - y)),
        BinOp::Mul => num(|x, y| Value::number(x * y)),
        BinOp::Div => num(|x, y| if y == 0.0 { Value::Error(CellError::Div0) } else { Value::number(x / y) }),
        BinOp::Pow => num(|x, y| {
            if x == 0.0 && y == 0.0 {
                Value::Error(CellError::Num)
            } else if x == 0.0 && y < 0.0 {
                Value::Error(CellError::Div0)
            } else {
                Value::number(x.powf(y))
            }
        }),
        BinOp::Concat => match (a.to_text(), b.to_text()) {
            (Ok(x), Ok(y)) => {
                if x.len() + y.len() > 32767 * 4 {
                    Value::Error(CellError::Value)
                } else {
                    Value::text(x + &y)
                }
            }
            (Err(e), _) | (_, Err(e)) => Value::Error(e),
        },
        BinOp::Eq => Value::Bool(compare(a, b) == Equal),
        BinOp::Ne => Value::Bool(compare(a, b) != Equal),
        BinOp::Lt => Value::Bool(compare(a, b) == Less),
        BinOp::Gt => Value::Bool(compare(a, b) == Greater),
        BinOp::Le => Value::Bool(compare(a, b) != Greater),
        BinOp::Ge => Value::Bool(compare(a, b) != Less),
        BinOp::Range | BinOp::Union | BinOp::Intersect => Value::Error(CellError::Value),
    }
}

#[cfg(test)]
mod tests {
    use super::{Array, BinOp, CellError, MAX_CELLS, Value, broadcast_len, concat_arrays, try_zip2};

    /// The broadcast stops at the first refused cell: nothing after it is visited.
    #[test]
    fn a_refused_cell_stops_the_broadcast() {
        let col = Value::from(Array::column((0..4_000).map(|i| Value::number(i as f64)).collect()));
        let row = Value::from(Array { rows: 1, cols: 4_000, data: (0..4_000).map(|i| Value::number(i as f64)).collect() });
        let mut calls = 0;
        let r = try_zip2(&col, &row, |a, _| {
            calls += 1;
            (calls <= 10).then(|| a.clone())
        });
        assert_eq!((r, calls), (None, 11));
    }

    #[test]
    fn concatenating_arrays_stops_at_the_byte_budget() {
        let sys = gridcraft_core::DateSystem::default();
        let col = Value::from(Array::column((0..100).map(|i| Value::number(i as f64)).collect()));
        let long = Value::text("x".repeat(100));
        // 100 cells of 101 or 102 bytes each: about 10 KB in all.
        let r = concat_arrays(&col, &long, sys, 20_000);
        assert!(matches!(&r, Value::Array(a) if a.data.len() == 100), "{r:?}");
        assert_eq!(concat_arrays(&col, &long, sys, 5_000), Value::Error(CellError::Num));
        assert_eq!(concat_arrays(&long, &col, sys, 5_000), Value::Error(CellError::Num));
        // Through the operator, a small result is unaffected.
        let r = super::binary_values(BinOp::Concat, &col, &Value::text("a"), sys);
        assert!(matches!(&r, Value::Array(a) if a.data.first() == Some(&Value::text("0a"))), "{r:?}");
    }

    #[test]
    fn broadcast_len_caps_at_max_cells() {
        assert_eq!(broadcast_len(4_000, 4_000), Some(MAX_CELLS as usize));
        assert_eq!(broadcast_len(4_000, 4_001), None);
        assert_eq!(broadcast_len(1, MAX_CELLS as usize), Some(MAX_CELLS as usize));
    }

    /// `65_536 * 65_536` is 2^32. Multiplied as `usize` on wasm32 it wraps to 0, which a
    /// `> MAX_CELLS` test would let through; the guard must reject the true product.
    #[test]
    fn broadcast_len_rejects_a_product_that_wraps_to_zero_on_32_bits() {
        assert_eq!(65_536u32.wrapping_mul(65_536), 0);
        assert_eq!(broadcast_len(65_536, 65_536), None);
        assert_eq!(broadcast_len(1_048_576, 16_384), None);
    }
}
