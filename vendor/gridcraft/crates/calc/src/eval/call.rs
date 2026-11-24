use super::array::{Acc, Agg, zip2, zip3};
use super::format::same_kind;
use super::special::{ev_shape, resized_sum_range, resizes_sum_range};
use super::{Area, Arg, Array, CellError, Ev, Evaluator, Expr, FnCtx, Lambda, Pipe, Value, compare, err};
use std::sync::Arc;

impl<'h> Evaluator<'h> {
    pub(super) fn call(&mut self, name: &str, args: &[Expr]) -> Ev {
        match name {
            "IF" => self.if_call(args),
            "IFS" => {
                if args.len() < 2 || !args.len().is_multiple_of(2) {
                    return err(CellError::Value);
                }
                for pair in args.chunks(2) {
                    let [c, v] = pair else { break };
                    let cv = self.value(c);
                    match cv.scalar().to_bool() {
                        Ok(true) => return self.eval(v),
                        Ok(false) => {}
                        Err(e) => return err(e),
                    }
                }
                err(CellError::NA)
            }
            "IFERROR" | "IFNA" => self.if_error_call(name, args),
            "CHOOSE" => self.choose_call(args),
            "SWITCH" => self.switch_call(args),
            "ROW" | "COLUMN" => self.row_column_call(name, args),
            "ROWS" | "COLUMNS" => {
                if args.len() != 1 {
                    return err(CellError::Value);
                }
                let is_rows = name == "ROWS";
                match self.eval(&args[0]) {
                    Ev::R(a) => match a.as_slice() {
                        [a] => Ev::V(Value::number(if is_rows { a.range.height() } else { a.range.width() } as f64)),
                        _ => err(CellError::Ref),
                    },
                    Ev::V(Value::Array(arr)) => Ev::V(Value::number(if is_rows { arr.rows } else { arr.cols } as f64)),
                    Ev::V(Value::Error(e)) => err(e),
                    Ev::V(_) => Ev::V(Value::Number(1.0)),
                    Ev::L(_) => err(CellError::Value),
                }
            }
            "AREAS" => match args.first().map(|a| self.eval(a)) {
                Some(Ev::R(a)) => Ev::V(Value::number(a.len() as f64)),
                Some(Ev::V(Value::Error(e))) => err(e),
                _ => err(CellError::Value),
            },
            "ISREF" => Ev::V(Value::Bool(matches!(args.first().map(|a| self.eval(a)), Some(Ev::R(_))))),
            "ISFORMULA" | "FORMULATEXT" => {
                let Some(Ev::R(a)) = args.first().map(|a| self.eval(a)) else { return err(CellError::NA) };
                let Some(a) = a.first() else { return err(CellError::NA) };
                let wb = self.host.workbook();
                let f = wb.sheet(a.sheet).and_then(|s| s.cell(a.range.start)).and_then(|c| c.formula.clone());
                // A formula formatted Hidden on a protected sheet stays hidden from FORMULATEXT too.
                let hidden = wb.sheet(a.sheet).is_some_and(|s| s.is_protected() && wb.styles.get(s.style_id(a.range.start)).protection.hidden);
                if name == "ISFORMULA" {
                    Ev::V(Value::Bool(f.is_some()))
                } else {
                    match f {
                        Some(f) if !hidden => Ev::V(Value::text(format!("={}", f.text))),
                        _ => err(CellError::NA),
                    }
                }
            }
            "SHEET" | "SHEETS" => self.sheet_call(name, args),
            "OFFSET" => self.offset(args),
            "INDIRECT" => self.indirect(args),
            "INDEX" => self.index(args),
            "XLOOKUP" => self.xlookup(args),
            "CELL" => self.cell_info(args),
            "TRIMRANGE" => self.trimrange(args),
            "GETPIVOTDATA" => self.getpivotdata(args),
            "SUBTOTAL" | "AGGREGATE" => self.subtotal(name, args),
            "TEXT" => {
                if args.len() != 2 {
                    return err(CellError::Value);
                }
                let v = self.arg_val(args, 0).unwrap_or_default();
                let f = self.arg_val(args, 1).unwrap_or_default();
                let sys = self.host.workbook().date_system;
                let one = |v: &Value, f: &Value| -> Value {
                    if let Value::Error(e) = v {
                        return Value::Error(*e);
                    }
                    let code = match f.to_text() {
                        Ok(c) => c,
                        Err(e) => return Value::Error(e),
                    };
                    match gridcraft_numfmt::text_function(v, &code, sys) {
                        Ok(s) => Value::text(s),
                        Err(e) => Value::Error(e),
                    }
                };
                Ev::V(zip2(&v, &f, one))
            }
            "LET" => self.let_(args),
            "LAMBDA" => {
                if args.is_empty() {
                    return err(CellError::Value);
                }
                let mut params = Vec::new();
                for p in &args[..args.len() - 1] {
                    match p {
                        Expr::Name(n) => params.push(n.trim_start_matches('[').trim_end_matches(']').to_string()),
                        _ => return err(CellError::Value),
                    }
                }
                let Some(body) = args.last() else { return err(CellError::Value) };
                Ev::L(Arc::new(Lambda { params, body: body.clone(), env: self.env.clone() }))
            }
            "ISOMITTED" => {
                let r = matches!(args.first(), Some(Expr::Name(n)) if self.env.iter().rev().find(|(k, _)| gridcraft_formula::same_name(k, n)).is_some_and(|(_, v)| matches!(v, Ev::V(Value::Empty))));
                Ev::V(Value::Bool(r))
            }
            "MAP" | "REDUCE" | "SCAN" | "BYROW" | "BYCOL" | "MAKEARRAY" => self.higher_order(name, args),
            "GROUPBY" | "PIVOTBY" => {
                // The aggregation may be a function given by name (`SUM`): pass it as its name.
                let i = if name == "GROUPBY" { 2 } else { 3 };
                match args.get(i).and_then(|a| self.function_by_name(a)) {
                    Some(f) => {
                        let mut args = args.to_vec();
                        if let Some(a) = args.get_mut(i) {
                            *a = Expr::Text(f.into());
                        }
                        self.builtin(name, &args)
                    }
                    None => self.builtin(name, args),
                }
            }
            "NOW" | "TODAY" | "RAND" | "RANDBETWEEN" | "RANDARRAY" => {
                self.host.note_volatile();
                self.builtin(name, args)
            }
            _ => self.builtin(name, args),
        }
    }

    fn if_call(&mut self, args: &[Expr]) -> Ev {
        if args.is_empty() || args.len() > 3 {
            return err(CellError::Value);
        }
        let condition = self.arg_val(args, 0).unwrap_or_default();
        if let Value::Array(array) = &condition {
            let when_true = self.arg_val(args, 1).unwrap_or(Value::Bool(true));
            let when_false = if args.len() > 2 { self.arg_val(args, 2).unwrap_or(Value::Bool(false)) } else { Value::Bool(false) };
            return Ev::V(zip3(array, &when_true, &when_false, |condition, when_true, when_false| match condition.to_bool() {
                Ok(true) => when_true.clone(),
                Ok(false) => when_false.clone(),
                Err(error) => Value::Error(error),
            }));
        }
        match condition.to_bool() {
            Ok(true) => match args.get(1) {
                Some(Expr::Missing) => Ev::V(Value::Number(0.0)),
                Some(expression) => self.eval(expression),
                None => Ev::V(Value::Bool(true)),
            },
            Ok(false) => match args.get(2) {
                Some(Expr::Missing) => Ev::V(Value::Number(0.0)),
                Some(expression) => self.eval(expression),
                None => Ev::V(Value::Bool(false)),
            },
            Err(error) => err(error),
        }
    }

    fn if_error_call(&mut self, name: &str, args: &[Expr]) -> Ev {
        if args.len() != 2 {
            return err(CellError::Value);
        }
        let value = self.arg_ev(args, 0).unwrap_or(Ev::V(Value::Empty));
        let na_only = name == "IFNA";
        let catches = |error: CellError| !na_only || error == CellError::NA;
        match &value {
            Ev::V(Value::Error(error)) if catches(*error) => self.eval(&args[1]),
            Ev::V(Value::Array(array)) if array.data.iter().any(|item| item.as_error().is_some_and(catches)) => {
                let alternate = self.arg_val(args, 1).unwrap_or_default();
                let data =
                    array.data.iter().map(|item| if item.as_error().is_some_and(catches) { alternate.scalar() } else { item.clone() }).collect();
                Ev::V(Array::new(array.rows, array.cols, data).map(Value::from).unwrap_or(Value::Error(CellError::Value)))
            }
            Ev::R(_) => self.if_error_reference(value.clone(), args, catches),
            _ => value,
        }
    }

    fn if_error_reference(&mut self, value: Ev, args: &[Expr], catches: impl Fn(CellError) -> bool + Copy) -> Ev {
        match self.deref(value.clone()) {
            Value::Error(error) if catches(error) => self.eval(&args[1]),
            Value::Array(ref array) if array.data.iter().any(|item| item.as_error().is_some_and(catches)) => {
                let alternate = self.arg_val(args, 1).unwrap_or_default();
                let data =
                    array.data.iter().map(|item| if item.as_error().is_some_and(catches) { alternate.scalar() } else { item.clone() }).collect();
                Ev::V(Array::new(array.rows, array.cols, data).map(Value::from).unwrap_or(Value::Error(CellError::Value)))
            }
            _ => value,
        }
    }

    fn choose_call(&mut self, args: &[Expr]) -> Ev {
        if args.len() < 2 {
            return err(CellError::Value);
        }
        let index = self.value(&args[0]);
        if let Value::Array(_) = index {
            let Some(spec) = gridcraft_functions::lookup("CHOOSE") else { return err(CellError::Name) };
            let mut evaluated = vec![Arg::val(index)];
            for argument in &args[1..] {
                evaluated.push(Arg::val(self.value(argument)));
            }
            let mut context = FnCtx { ev: self, criteria: None, lambda: None };
            return gridcraft_functions::call(spec, &evaluated, &mut context).pipe(Ev::V);
        }
        let index = match index.to_number() {
            Ok(number) => number.trunc() as i64,
            Err(error) => return err(error),
        };
        if index < 1 || index as usize >= args.len() {
            return err(CellError::Value);
        }
        self.eval(&args[index as usize])
    }

    fn switch_call(&mut self, args: &[Expr]) -> Ev {
        if args.len() < 3 {
            return err(CellError::Value);
        }
        let value = self.arg_val(args, 0).unwrap_or_default().scalar();
        if let Value::Error(error) = value {
            return err(error);
        }
        let choices = &args[1..];
        for pair in choices.as_chunks::<2>().0 {
            let (candidate, result) = (&pair[0], &pair[1]);
            let candidate = self.value(candidate).scalar();
            if !candidate.is_error() && compare(&value, &candidate) == std::cmp::Ordering::Equal && same_kind(&value, &candidate) {
                return self.eval(result);
            }
        }
        if let Some(default) = choices.get(choices.len().saturating_sub(1)).filter(|_| choices.len() % 2 == 1) {
            self.eval(default)
        } else {
            err(CellError::NA)
        }
    }

    fn row_column_call(&mut self, name: &str, args: &[Expr]) -> Ev {
        let is_row = name == "ROW";
        if args.is_empty() || matches!(args.first(), Some(Expr::Missing)) {
            return Ev::V(Value::number(if is_row { self.at.row + 1 } else { self.at.col + 1 } as f64));
        }
        let areas = match self.eval(&args[0]) {
            Ev::R(areas) => areas,
            Ev::V(Value::Error(error)) => return err(error),
            _ => return err(CellError::Value),
        };
        let Some(area) = areas.first() else { return err(CellError::Ref) };
        let range = self.trim_lines(area);
        if is_row {
            let values: Vec<Value> = (range.start.row..=range.end.row).map(|row| Value::number(row as f64 + 1.0)).collect();
            if values.len() == 1 { Ev::V(values.into_iter().next().unwrap_or_default()) } else { Ev::V(Array::column(values).into()) }
        } else {
            let values: Vec<Value> = (range.start.col..=range.end.col).map(|col| Value::number(col as f64 + 1.0)).collect();
            if values.len() == 1 { Ev::V(values.into_iter().next().unwrap_or_default()) } else { Ev::V(Array::row(values).into()) }
        }
    }

    fn sheet_call(&mut self, name: &str, args: &[Expr]) -> Ev {
        let is_sheet = name == "SHEET";
        if args.is_empty() {
            let count = if is_sheet { self.sheet + 1 } else { self.host.workbook().sheets.len() };
            return Ev::V(Value::number(count as f64));
        }
        match self.eval(&args[0]) {
            Ev::R(areas) if is_sheet => areas.first().map_or(err(CellError::Ref), |area| Ev::V(Value::number(area.sheet as f64 + 1.0))),
            Ev::R(areas) => {
                let mut sheets: Vec<usize> = areas.iter().map(|area| area.sheet).collect();
                sheets.dedup();
                Ev::V(Value::number(sheets.len() as f64))
            }
            Ev::V(Value::Text(name)) if is_sheet => {
                self.host.workbook().sheet_index(&name).map_or(err(CellError::NA), |index| Ev::V(Value::number(index as f64 + 1.0)))
            }
            _ => err(CellError::NA),
        }
    }

    pub(super) fn builtin(&mut self, name: &str, args: &[Expr]) -> Ev {
        let Some(spec) = gridcraft_functions::lookup(name) else {
            // A defined name holding a LAMBDA, or a LET-bound lambda.
            return match self.name(name) {
                Ev::L(l) => self.apply_lambda(&l, args),
                Ev::V(Value::Error(x)) => err(x),
                _ => err(CellError::Name),
            };
        };
        if args.len() < spec.min_args || args.len() > spec.max_args {
            return err(CellError::Value);
        }
        if let Some(kind) = Agg::of(spec.name) {
            return self.aggregate(kind, args);
        }
        let mut evaluated = Vec::with_capacity(args.len());
        // SUMIF/AVERAGEIF: the size of the criteria range (argument 1), which the sum range takes.
        let mut criteria_shape = None;
        // Database functions: the criteria range (argument 2), whose formulas are computed criteria.
        let mut criteria = None;
        // GROUPBY/PIVOTBY: the LAMBDA given as their function.
        let mut lambda = None;
        let lambda_at = match name {
            "GROUPBY" => Some(2),
            "PIVOTBY" => Some(3),
            _ => None,
        };
        for (i, a) in args.iter().enumerate() {
            let mut ev = self.eval(a);
            if lambda_at == Some(i)
                && let Ev::L(l) = &ev
            {
                lambda = Some(l.clone());
                ev = Ev::V(Value::Empty);
            }
            if spec.category == gridcraft_functions::Category::Database
                && i == 2
                && let Ev::R(areas) = &ev
                && let [area] = areas.as_slice()
            {
                criteria = Some(*area);
            }
            if resizes_sum_range(name, args.len()) {
                if i == 0 {
                    criteria_shape = ev_shape(&ev);
                } else if i == 2
                    && let (Some((h, w)), Ev::R(areas)) = (criteria_shape, &ev)
                    && let [area] = areas.as_slice()
                {
                    ev = Ev::R(vec![Area { sheet: area.sheet, range: resized_sum_range(area.range, h, w) }]);
                }
            }
            let from_ref = matches!(ev, Ev::R(_));
            let value = match ev {
                Ev::R(areas) if areas.len() > 1 => {
                    // Multi-area references (unions): flatten into one row for aggregates.
                    let mut vals = Vec::new();
                    for ar in &areas {
                        match self.area_value(ar) {
                            Value::Array(a) => vals.extend(a.data.iter().cloned()),
                            v => vals.push(v),
                        }
                    }
                    Value::from(Array::row(vals))
                }
                ev => self.deref(ev),
            };
            // Only an argument left out (`f(a,,b)`) is omitted; one that computes a blank is given.
            let blank_value = !from_ref && value == Value::Empty && lambda_at != Some(i) && !matches!(a, Expr::Missing);
            evaluated.push(Arg { value, from_ref, blank_value });
        }
        // The function may go through every value it is given (a SUM inside each call of a
        // LAMBDA over a large array): the values count against the cell's work.
        let values = evaluated.iter().fold(0u64, |n, a| n.saturating_add(if let Value::Array(x) = &a.value { x.data.len() as u64 } else { 1 }));
        if !self.spend(values) {
            return err(CellError::Calc);
        }
        let database = criteria.is_some();
        let mut ctx = FnCtx { ev: self, criteria, lambda };
        let v = gridcraft_functions::call(spec, &evaluated, &mut ctx);
        // A computed criterion that ran out of work counted as not met: the result leaves
        // records out.
        if database && self.exhausted {
            return err(CellError::Calc);
        }
        Ev::V(v)
    }

    /// Performs aggregation operations without copying its ranges: blocks are folded over the stored
    /// cells, so no array is built and there is no `MAX_CELLS` limit. Arguments are evaluated and read one
    /// after another, as the array path does, so volatile formulas (like `RAND`) see the same order.
    /// Only `SUM`, `COUNT` and `AVG` are supported currently.
    fn aggregate(&mut self, kind: Agg, args: &[Expr]) -> Ev {
        let mut acc = Acc::default();
        let mut first_error = None;
        for a in args {
            let ev = self.eval(a);
            let steps = match ev {
                Ev::R(areas) => areas.iter().map(|area| self.aggregate_area(kind, area, &mut acc)).collect(),
                ev => {
                    let value = self.deref(ev);
                    vec![self.aggregate_value(kind, value, false, &mut acc)]
                }
            };
            if first_error.is_none() {
                first_error = steps.into_iter().find_map(Result::err);
            }
        }
        first_error.unwrap_or_else(|| Ev::V(acc.finish(kind)))
    }

    /// Adds one area to `acc`; `Err` is the result the function ends with.
    fn aggregate_area(&mut self, kind: Agg, a: &Area, acc: &mut Acc) -> Result<(), Ev> {
        if !a.range.is_single() {
            // Adding the block's remembered sum to anything but +0.0 would add in another order.
            // COUNT never reads the sum, so it may reuse every block.
            if (kind == Agg::Count || acc.sum.to_bits() == 0)
                && let Some((sum, count)) = self.host.range_sum(a.sheet, a.range)
            {
                acc.sum = sum;
                acc.count = acc.count.saturating_add(count);
                return Ok(());
            }
            // Folded without building an array, but still walked: the part inside the used
            // range counts against the cell's work.
            let r = self.read_range(a);
            if !self.spend(u64::from(r.height()) * u64::from(r.width())) {
                return Err(err(CellError::Calc));
            }
            let mut error = None;
            let walked = self.host.visit_range(a.sheet, a.range, &mut |v| match acc.add_cell(kind, v) {
                Ok(()) => true,
                Err(e) => {
                    error = Some(e);
                    false
                }
            });
            if walked {
                return error.map_or(Ok(()), |e| Err(err(e)));
            }
        }
        let value = self.area_value(a);
        self.aggregate_value(kind, value, true, acc)
    }

    /// Adds one argument's value by calling `SUM` and `COUNT` on it, so coercion and error rules
    /// stay in one place (`AVERAGE` of an argument is its `SUM` over its `COUNT`).
    fn aggregate_value(&mut self, kind: Agg, value: Value, from_ref: bool, acc: &mut Acc) -> Result<(), Ev> {
        if !from_ref && !self.spend_on(&value) {
            return Err(err(CellError::Calc));
        }
        let (Some(sum), Some(count)) = (gridcraft_functions::lookup("SUM"), gridcraft_functions::lookup("COUNT")) else {
            return Err(err(CellError::Name));
        };
        let args = [Arg { value, from_ref, blank_value: false }];
        if kind != Agg::Count {
            match gridcraft_functions::call(sum, &args, &mut FnCtx { ev: self, criteria: None, lambda: None }) {
                Value::Number(n) => acc.sum += n,
                other => return Err(Ev::V(other)),
            }
        }
        if kind != Agg::Sum {
            match gridcraft_functions::call(count, &args, &mut FnCtx { ev: self, criteria: None, lambda: None }) {
                Value::Number(n) => acc.count = acc.count.saturating_add(n as u64),
                other => return Err(Ev::V(other)),
            }
        }
        Ok(())
    }

    /// A worksheet function given by name where a function is expected (`BYROW(A1:C3,SUM)`,
    /// `GROUPBY(A2:A9,B2:B9,SUM)`): its name, unless a LET/LAMBDA variable or a defined name has
    /// that name.
    pub(super) fn function_by_name(&self, e: &Expr) -> Option<String> {
        let Expr::Name(n) = e else { return None };
        if self.env.iter().any(|(k, _)| gridcraft_formula::same_name(k, n)) || self.host.workbook().name(n, self.sheet).is_some() {
            return None;
        }
        let u = n.to_ascii_uppercase();
        gridcraft_functions::lookup(&u).map(|_| u)
    }
}
