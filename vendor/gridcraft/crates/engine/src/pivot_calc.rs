//! Calculated PivotTable fields: a formula over other fields (`=Sales*0.1`,
//! `='Unit Price'*Units`, `=IF(Units>0,Sales/Units,0)`).
//!
//! As Microsoft documents for Excel, the formula operates on the *sums* of the fields it names
//! for each report cell (items, subtotals and grand totals alike), not record by record: for
//! `=Sales*1.2` a cell shows 1.2 × the sum of Sales of that cell's records. Fields are written
//! bare (`Sales`) or in single quotes (`'Unit Price'`); cell references and defined names are not
//! allowed. A calculated field may use other calculated fields; a cycle evaluates to `#REF!`.

use gridcraft_core::{CellError, CellRef, Value};
use gridcraft_formula::Expr;
use gridcraft_model::{PivotTable, Workbook};

/// Longest formula accepted.
pub const MAX_FORMULA_LEN: usize = 8_192;
/// Most calculated fields per PivotTable.
pub const MAX_CALC_FIELDS: usize = 256;
/// Deepest chain of calculated fields using calculated fields.
const MAX_DEPTH: usize = 64;
const PREFIX: &str = "__pcf";

fn placeholder(i: usize) -> String {
    format!("{PREFIX}{i}")
}

/// A parsed formula: field names replaced by placeholder names `__pcf0`, `__pcf1`, … in the
/// order of `refs`.
#[derive(Clone, Debug)]
pub struct Compiled {
    pub body: Expr,
    /// Canonical names of the fields used.
    pub refs: Vec<String>,
}

/// Parses a calculated-field formula. `field` maps a written name to the canonical name of a
/// source or calculated field.
pub fn compile(formula: &str, field: &dyn Fn(&str) -> Option<String>) -> Result<Compiled, String> {
    let text = formula.trim();
    let text = text.strip_prefix('=').unwrap_or(text).trim();
    if text.is_empty() {
        return Err("The formula is empty.".into());
    }
    if text.len() > MAX_FORMULA_LEN {
        return Err("The formula is too long.".into());
    }
    let chars: Vec<char> = text.chars().collect();
    let at = |i: usize| chars.get(i).copied();
    let mut out = String::with_capacity(text.len() + 16);
    let mut refs: Vec<String> = Vec::new();
    let slot = |name: String, refs: &mut Vec<String>| -> String {
        let i = refs.iter().position(|r| r.eq_ignore_ascii_case(&name)).unwrap_or_else(|| {
            refs.push(name);
            refs.len() - 1
        });
        placeholder(i)
    };
    let mut i = 0;
    while let Some(c) = at(i) {
        match c {
            '"' => {
                out.push('"');
                i += 1;
                loop {
                    match at(i) {
                        None => return Err("The formula has an unterminated text value.".into()),
                        Some('"') if at(i + 1) == Some('"') => {
                            out.push_str("\"\"");
                            i += 2;
                        }
                        Some('"') => {
                            out.push('"');
                            i += 1;
                            break;
                        }
                        Some(x) => {
                            out.push(x);
                            i += 1;
                        }
                    }
                }
            }
            '\'' => {
                let mut name = String::new();
                i += 1;
                loop {
                    match at(i) {
                        None => return Err("The formula has an unterminated field name.".into()),
                        Some('\'') if at(i + 1) == Some('\'') => {
                            name.push('\'');
                            i += 2;
                        }
                        Some('\'') => {
                            i += 1;
                            break;
                        }
                        Some(x) => {
                            name.push(x);
                            i += 1;
                        }
                    }
                }
                let canon = field(&name).ok_or_else(|| format!("There's no field named '{name}'."))?;
                out.push_str(&slot(canon, &mut refs));
            }
            c if c.is_ascii_digit() || (c == '.' && at(i + 1).is_some_and(|d| d.is_ascii_digit())) => {
                while at(i).is_some_and(|d| d.is_ascii_digit() || d == '.') {
                    out.push(at(i).unwrap_or('0'));
                    i += 1;
                }
                if matches!(at(i), Some('e' | 'E')) {
                    let sign = usize::from(matches!(at(i + 1), Some('+' | '-')));
                    if at(i + 1 + sign).is_some_and(|d| d.is_ascii_digit()) {
                        for _ in 0..=sign {
                            out.push(at(i).unwrap_or('0'));
                            i += 1;
                        }
                        while at(i).is_some_and(|d| d.is_ascii_digit()) {
                            out.push(at(i).unwrap_or('0'));
                            i += 1;
                        }
                    }
                }
            }
            c if c.is_alphabetic() || c == '_' => {
                let start = i;
                while at(i).is_some_and(|d| d.is_alphanumeric() || d == '_' || d == '.') {
                    i += 1;
                }
                let word: String = chars.get(start..i).map(|w| w.iter().collect()).unwrap_or_default();
                let mut j = i;
                while at(j).is_some_and(char::is_whitespace) {
                    j += 1;
                }
                let is_call = at(j) == Some('(');
                match field(&word) {
                    Some(canon) if !is_call => out.push_str(&slot(canon, &mut refs)),
                    _ => out.push_str(&word),
                }
            }
            c => {
                out.push(c);
                i += 1;
            }
        }
    }
    let body = gridcraft_formula::parse(&out).map_err(|_| "The formula contains an error.".to_string())?;
    let mut problem: Option<String> = None;
    body.walk(&mut |e| {
        if problem.is_some() {
            return;
        }
        match e {
            Expr::Name(n) if n.starts_with(PREFIX) => {}
            Expr::Name(n) => problem = Some(format!("There's no field named '{n}'.")),
            Expr::Ref(_) | Expr::Struct(_) => problem = Some("A calculated field can't refer to cells or tables; use field names.".into()),
            // Evaluated once per group and per report, with only the first value kept: Excel does not
            // allow functions that need arrays in a calculated field, and these build one from scalars.
            Expr::Call(f, _) if matches!(f.as_str(), "SEQUENCE" | "RANDARRAY" | "MAKEARRAY" | "EXPAND" | "MUNIT") => {
                problem = Some(format!("A calculated field can't use {f}: it builds an array."))
            }
            _ => {}
        }
    });
    match problem {
        Some(p) => Err(p),
        None => Ok(Compiled { body, refs }),
    }
}

fn literal(v: &Value) -> Expr {
    match v {
        Value::Number(n) => Expr::Number(*n),
        Value::Error(e) => Expr::Error(*e),
        Value::Text(t) => Expr::Text(t.clone()),
        Value::Bool(b) => Expr::Bool(*b),
        Value::Empty => Expr::Number(0.0),
        Value::Array(_) => literal(&v.scalar()),
    }
}

/// Evaluates a compiled formula with the given field values (in `refs` order).
pub fn eval(wb: &Workbook, c: &Compiled, args: &[Value]) -> Value {
    let expr = if c.refs.is_empty() {
        c.body.clone()
    } else {
        let mut call = Vec::with_capacity(c.refs.len() * 2 + 1);
        for i in 0..c.refs.len() {
            call.push(Expr::Name(placeholder(i)));
            call.push(literal(args.get(i).unwrap_or(&Value::Empty)));
        }
        call.push(c.body.clone());
        Expr::Call("LET".into(), call)
    };
    match gridcraft_calc::recalc::evaluate_expr(wb, 0, CellRef::default(), &expr).scalar() {
        Value::Empty => Value::Number(0.0),
        v => v,
    }
}

/// A PivotTable's calculated fields, compiled against its source headers.
pub struct CalcSet {
    names: Vec<String>,
    compiled: Vec<Result<Compiled, String>>,
}

impl CalcSet {
    pub fn new(pt: &PivotTable, headers: &[String]) -> CalcSet {
        let names: Vec<String> = pt.calculated_fields.iter().take(MAX_CALC_FIELDS).map(|c| c.name.clone()).collect();
        let lookup = |n: &str| -> Option<String> {
            let n = n.trim();
            headers.iter().chain(names.iter()).find(|h| h.trim().eq_ignore_ascii_case(n)).cloned()
        };
        let compiled = pt.calculated_fields.iter().take(MAX_CALC_FIELDS).map(|c| compile(&c.formula, &lookup)).collect();
        CalcSet { names, compiled }
    }

    pub fn index(&self, name: &str) -> Option<usize> {
        self.names.iter().position(|n| n.eq_ignore_ascii_case(name.trim()))
    }

    /// The compile error of a calculated field, if any.
    pub fn error(&self, i: usize) -> Option<&str> {
        self.compiled.get(i).and_then(|c| c.as_ref().err()).map(String::as_str)
    }

    /// Source fields a calculated field needs, directly or through other calculated fields.
    pub fn source_refs(&self, i: usize) -> Vec<String> {
        let mut out: Vec<String> = Vec::new();
        let mut seen = vec![false; self.names.len()];
        let mut stack = vec![i];
        while let Some(j) = stack.pop() {
            if !seen.get(j).copied().unwrap_or(true) {
                if let Some(s) = seen.get_mut(j) {
                    *s = true;
                }
                if let Some(Ok(c)) = self.compiled.get(j) {
                    for r in &c.refs {
                        match self.index(r) {
                            Some(k) => stack.push(k),
                            None => {
                                if !out.iter().any(|o| o.eq_ignore_ascii_case(r)) {
                                    out.push(r.clone());
                                }
                            }
                        }
                    }
                }
            }
        }
        out
    }

    /// Whether a calculated field reaches itself through the fields it uses.
    pub fn is_cyclic(&self, i: usize) -> bool {
        let mut seen = vec![false; self.names.len()];
        let mut stack: Vec<usize> = match self.compiled.get(i) {
            Some(Ok(c)) => c.refs.iter().filter_map(|r| self.index(r)).collect(),
            _ => vec![],
        };
        while let Some(j) = stack.pop() {
            if j == i {
                return true;
            }
            if seen.get(j).copied().unwrap_or(true) {
                continue;
            }
            if let Some(s) = seen.get_mut(j) {
                *s = true;
            }
            if let Some(Ok(c)) = self.compiled.get(j) {
                stack.extend(c.refs.iter().filter_map(|r| self.index(r)));
            }
        }
        false
    }

    /// The value of calculated field `i` for one report cell; `sum` gives a source field's sum
    /// over that cell's records.
    pub fn value(&self, wb: &Workbook, i: usize, sum: &dyn Fn(&str) -> Value) -> Value {
        let mut memo: Vec<Option<Value>> = vec![None; self.names.len()];
        let mut active = vec![false; self.names.len()];
        self.value_in(wb, i, sum, &mut memo, &mut active, 0)
    }

    fn value_in(&self, wb: &Workbook, i: usize, sum: &dyn Fn(&str) -> Value, memo: &mut [Option<Value>], active: &mut [bool], depth: usize) -> Value {
        if let Some(Some(v)) = memo.get(i) {
            return v.clone();
        }
        if depth > MAX_DEPTH || active.get(i).copied().unwrap_or(true) {
            return Value::Error(CellError::Ref);
        }
        let Some(Ok(c)) = self.compiled.get(i) else { return Value::Error(CellError::Name) };
        if let Some(a) = active.get_mut(i) {
            *a = true;
        }
        let args: Vec<Value> = c
            .refs
            .iter()
            .map(|r| match self.index(r) {
                Some(j) => self.value_in(wb, j, sum, memo, active, depth + 1),
                None => sum(r),
            })
            .collect();
        let v = eval(wb, c, &args);
        if let Some(a) = active.get_mut(i) {
            *a = false;
        }
        if let Some(m) = memo.get_mut(i) {
            *m = Some(v.clone());
        }
        v
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fields(n: &str) -> Option<String> {
        ["Sales", "Units", "Unit Price", "Q1"].iter().find(|f| f.eq_ignore_ascii_case(n)).map(|f| f.to_string())
    }

    #[test]
    fn compiles_field_names() {
        let c = compile("= Sales*0.1 + 'Unit Price'/units - Sales", &fields).unwrap();
        assert_eq!(c.refs, vec!["Sales", "Unit Price", "Units"]);
        // A field named like a cell reference is still a field; functions stay functions.
        let c = compile("=ROUND(Q1*1.5E2,0)", &fields).unwrap();
        assert_eq!(c.refs, vec!["Q1"]);
        let wb = Workbook::new();
        assert_eq!(eval(&wb, &c, &[Value::Number(2.0)]), Value::Number(300.0));
        assert!(compile("=Sales*Bogus", &fields).unwrap_err().contains("Bogus"));
        assert!(compile("=A1*2", &fields).is_err());
        assert!(compile("='Nope'*2", &fields).is_err());
        assert!(compile("=Sales*(", &fields).is_err());
        assert!(compile("=", &fields).is_err());
        assert!(compile(&format!("={}", "1+".repeat(MAX_FORMULA_LEN)), &fields).is_err());
    }

    #[test]
    fn a_calculated_field_cannot_build_arrays_from_scalars() {
        for f in
            ["=SEQUENCE(5000,1000)", "=SUM(RANDARRAY(1000,1000))", "=MAKEARRAY(9,9,LAMBDA(r,c,r*c))+Sales", "=expand(Sales,1000,1000)", "=MUNIT(500)"]
        {
            assert!(compile(f, &fields).unwrap_err().contains("builds an array"), "{f}");
        }
        assert!(compile("=SUM({1,2,3})*Sales", &fields).is_ok(), "an array constant is bounded by the formula's own length");
    }

    #[test]
    fn deep_and_cyclic_chains_are_bounded() {
        // 300 calculated fields, each using the previous one, and a two-field cycle.
        let mut pt = PivotTable::default();
        pt.calculated_fields.push(gridcraft_model::PivotCalcField { name: "C0".into(), formula: "=Sales+1".into() });
        for i in 1..300 {
            pt.calculated_fields.push(gridcraft_model::PivotCalcField { name: format!("C{i}"), formula: format!("=C{}+1", i - 1) });
        }
        pt.calculated_fields.push(gridcraft_model::PivotCalcField { name: "P".into(), formula: "=Q".into() });
        pt.calculated_fields.push(gridcraft_model::PivotCalcField { name: "Q".into(), formula: "=P*2".into() });
        let set = CalcSet::new(&pt, &["Sales".to_string()]);
        let wb = Workbook::new();
        let sum = |_: &str| Value::Number(10.0);
        assert_eq!(set.value(&wb, 0, &sum), Value::Number(11.0));
        assert_eq!(set.value(&wb, 10, &sum), Value::Number(21.0));
        // Past the depth bound (the list itself is capped at MAX_CALC_FIELDS).
        assert_eq!(set.value(&wb, 200, &sum), Value::Error(CellError::Ref));
        assert_eq!(set.index("P"), None, "P is past the cap of {MAX_CALC_FIELDS} fields");
        // A small cycle within the cap.
        let mut small = PivotTable::default();
        small.calculated_fields.push(gridcraft_model::PivotCalcField { name: "P".into(), formula: "=Q".into() });
        small.calculated_fields.push(gridcraft_model::PivotCalcField { name: "Q".into(), formula: "=P*2".into() });
        let set = CalcSet::new(&small, &["Sales".to_string()]);
        assert!(set.is_cyclic(0));
        assert_eq!(set.value(&wb, 0, &sum), Value::Error(CellError::Ref));
        assert_eq!(set.source_refs(0), Vec::<String>::new());
    }
}
