use super::ast::{Expr, Stmt};
use super::value::{Val, is_numeric_text, num_text};
use super::{
    CALLS_TOO_DEEP, Flow, Func, HashMap, HashSet, Interp, LIST_TOO_LONG, LOOPED_TOO_MUCH, MAX_LIST, R, Rc, TOO_DEEP, XfaEffect, child_named, clip,
    descendants_named, instances, reset_names, resolve_som,
};

impl Interp<'_> {
    // ── expressions

    pub(super) fn eval(&mut self, e: &Expr) -> R<Val> {
        self.step()?;
        self.depth += 1;
        if self.depth > self.max_depth {
            self.depth -= 1;
            return Err(TOO_DEEP.into());
        }
        let r = self.eval_inner(e);
        self.depth -= 1;
        r
    }

    fn eval_inner(&mut self, e: &Expr) -> R<Val> {
        Ok(match e {
            Expr::Num(n) => Val::Num(*n),
            Expr::Str(s) => {
                self.charge((s.len() / 256) as u64)?;
                Val::Str(s.clone())
            }
            Expr::Null => Val::Null,
            Expr::Ident(name) => {
                let v = self.ident(name)?;
                self.charge(self.cost(&v))?;
                v
            }
            Expr::Member(base, name) => {
                let b = self.eval(base)?;
                self.member(b, name)?
            }
            Expr::Index(base, idx) => self.index(base, idx)?,
            Expr::Descend(base, name) => {
                let b = self.eval(base)?;
                let mut out = Vec::new();
                let mut seen = HashSet::new();
                for i in self.nodes_list(&b) {
                    self.gather(&mut out, &mut seen, descendants_named(self.h, i, name, 0))?;
                }
                match out.as_slice() {
                    [] => Val::Null,
                    [one] => Val::Node(*one),
                    many => Val::List(many.iter().map(|k| Val::Node(*k)).collect()),
                }
            }
            Expr::Call(callee, args) => self.call(callee, args)?,
            Expr::Neg(x) => {
                let v = self.eval(x)?;
                if self.is_null(&v) { Val::Null } else { Val::Num(-self.to_num(&v)) }
            }
            Expr::Not(x) => {
                let v = self.eval(x)?;
                Val::Num(if self.truthy(&v) { 0.0 } else { 1.0 })
            }
            Expr::Bin(..) => {
                // The parser leans left (`a + b + c` is `(a + b) + c`), so a chain is walked
                // from its leftmost operand and folded without recursing per operator.
                let mut links = Vec::new();
                let mut leftmost = e;
                while let Expr::Bin(op, l, r) = leftmost {
                    links.push((*op, r.as_ref()));
                    leftmost = l;
                }
                let mut acc = self.eval(leftmost)?;
                for (op, r) in links.into_iter().rev() {
                    self.step()?;
                    acc = match op {
                        "&" => {
                            if !self.truthy(&acc) {
                                // Not evaluated: the result is settled.
                                Val::Num(0.0)
                            } else {
                                let b = self.eval(r)?;
                                Val::Num(if self.truthy(&b) { 1.0 } else { 0.0 })
                            }
                        }
                        "|" => {
                            if self.truthy(&acc) {
                                Val::Num(1.0)
                            } else {
                                let b = self.eval(r)?;
                                Val::Num(if self.truthy(&b) { 1.0 } else { 0.0 })
                            }
                        }
                        _ => {
                            let b = self.eval(r)?;
                            // Comparing (and dereferencing) long strings is work too.
                            self.charge(self.cost(&acc).saturating_add(self.cost(&b)))?;
                            self.binary(op, acc, b)?
                        }
                    };
                }
                acc
            }
        })
    }

    pub(super) fn binary(&self, op: &str, a: Val, b: Val) -> R<Val> {
        let (a, b) = (self.deref(a), self.deref(b));
        // Null with an arithmetic operator is null only when both sides are null.
        match op {
            "+" | "-" | "*" | "/" => {
                if self.is_null(&a) && self.is_null(&b) {
                    return Ok(Val::Null);
                }
                let (x, y) = (self.to_num(&a), self.to_num(&b));
                let v = match op {
                    "+" => x + y,
                    "-" => x - y,
                    "*" => x * y,
                    _ => {
                        if y == 0.0 {
                            return Err("division by zero".into());
                        }
                        x / y
                    }
                };
                Ok(Val::Num(v))
            }
            _ => {
                let ord = match (&a, &b) {
                    (Val::Null, Val::Null) => Some(std::cmp::Ordering::Equal),
                    (Val::Null, _) | (_, Val::Null) => None,
                    (Val::Str(s), Val::Str(t)) if !(is_numeric_text(s) && is_numeric_text(t)) => Some(s.cmp(t)),
                    (Val::Str(s), Val::Num(_)) if !is_numeric_text(s) => Some(s.as_str().cmp(num_text(self.to_num(&b)).as_str())),
                    (Val::Num(_), Val::Str(t)) if !is_numeric_text(t) => Some(num_text(self.to_num(&a)).as_str().cmp(t.as_str())),
                    _ => self.to_num(&a).partial_cmp(&self.to_num(&b)),
                };
                let r = match (op, ord) {
                    ("==", Some(o)) => o.is_eq(),
                    ("<>", Some(o)) => !o.is_eq(),
                    ("<>", None) => true,
                    ("<", Some(o)) => o.is_lt(),
                    ("<=", Some(o)) => o.is_le(),
                    (">", Some(o)) => o.is_gt(),
                    (">=", Some(o)) => o.is_ge(),
                    _ => false,
                };
                Ok(Val::Num(if r { 1.0 } else { 0.0 }))
            }
        }
    }

    // ── calls

    fn call(&mut self, callee: &Expr, args: &[Expr]) -> R<Val> {
        match callee {
            Expr::Ident(name) => {
                if let Some(f) = self.funcs.get(&name.to_ascii_lowercase()).cloned() {
                    let mut vals = Vec::new();
                    for a in args {
                        let v = self.eval(a)?;
                        vals.push(self.take(v)?);
                    }
                    return self.call_user(&f, vals);
                }
                self.builtin(name, args)
            }
            Expr::Member(base, method) => {
                let b = self.eval(base)?;
                self.method(b, method, args)
            }
            _ => Err("not a function".into()),
        }
    }

    fn call_user(&mut self, f: &Func, vals: Vec<Val>) -> R<Val> {
        if self.scopes.len() > self.max_depth {
            return Err(CALLS_TOO_DEEP.into());
        }
        let mut scope = HashMap::new();
        for (i, p) in f.params.iter().enumerate() {
            scope.insert(p.clone(), vals.get(i).cloned().unwrap_or(Val::Null));
        }
        self.scopes.push(scope);
        let r = self.block(&f.body);
        self.scopes.pop();
        match r? {
            (Flow::Return(v), _) => Ok(v),
            (Flow::Exit, last) => {
                self.exited = true;
                Ok(last)
            }
            (_, last) => Ok(last),
        }
    }

    pub(super) fn arg(&mut self, args: &[Expr], i: usize) -> R<Val> {
        match args.get(i) {
            Some(e) => {
                let v = self.eval(e)?;
                self.take(v)
            }
            None => Ok(Val::Null),
        }
    }

    pub(super) fn arg_num(&mut self, args: &[Expr], i: usize) -> R<f64> {
        let v = self.arg(args, i)?;
        Ok(self.to_num(&v))
    }

    pub(super) fn arg_str(&mut self, args: &[Expr], i: usize) -> R<String> {
        let v = self.arg(args, i)?;
        Ok(self.to_str(&v))
    }

    /// Every value the arguments hold, lists flattened, nulls left out.
    pub(super) fn arg_values(&mut self, args: &[Expr]) -> R<Vec<Val>> {
        self.arg_values_keeping(args, false)
    }

    /// [`Self::arg_values`]; `objects` keeps form objects (fields, subforms) as themselves, so
    /// `Count(row[*])` counts rows rather than filled-in values.
    pub(super) fn arg_values_keeping(&mut self, args: &[Expr], objects: bool) -> R<Vec<Val>> {
        let mut out = Vec::new();
        for a in args {
            let v = self.eval(a)?;
            let v = if objects { v } else { self.take(v)? };
            match v {
                Val::List(items) => out.extend(items.into_iter().filter(|x| !matches!(x, Val::Null))),
                Val::Null => {}
                other => out.push(other),
            }
            if out.len() > MAX_LIST {
                return Err(LIST_TOO_LONG.into());
            }
        }
        Ok(out)
    }

    fn method(&mut self, base: Val, method: &str, args: &[Expr]) -> R<Val> {
        let m = method.to_ascii_lowercase();
        match base {
            Val::Host => match m.as_str() {
                "messagebox" => {
                    let t = self.arg_str(args, 0)?;
                    self.h.push(XfaEffect::MessageBox(t));
                    Ok(Val::Num(1.0))
                }
                "resetdata" => {
                    let list = self.arg_str(args, 0)?;
                    self.h.push(XfaEffect::ResetData(reset_names(&list)));
                    Ok(Val::Null)
                }
                "print" => {
                    self.h.push(XfaEffect::Print);
                    Ok(Val::Null)
                }
                "beep" => {
                    self.h.push(XfaEffect::Beep);
                    Ok(Val::Null)
                }
                "gotourl" => {
                    let u = self.arg_str(args, 0)?;
                    self.h.push(XfaEffect::LaunchUrl(u));
                    Ok(Val::Null)
                }
                "setfocus" => {
                    let target = match args.first() {
                        Some(e) => {
                            let raw = self.eval(e)?;
                            match self.nodes_of(&raw).first() {
                                Some(i) => self.node(*i).map(|n| n.som.clone()).unwrap_or_default(),
                                None => {
                                    let s = self.to_str(&raw);
                                    let found = resolve_som(self.h, self.h.current, &s);
                                    found.first().and_then(|i| self.node(*i)).map(|n| n.som.clone()).unwrap_or(s)
                                }
                            }
                        }
                        None => String::new(),
                    };
                    self.h.push(XfaEffect::SetFocus(target));
                    Ok(Val::Null)
                }
                _ => Ok(Val::Null),
            },
            Val::Layout => match m.as_str() {
                "page" | "abspage" => Ok(Val::Num(self.h.doc.page as f64 + if m == "page" { 1.0 } else { 0.0 })),
                "pagecount" | "abspagecount" => Ok(Val::Num(self.h.doc.page_count as f64)),
                "relayout" | "relayoutpagearea" => {
                    self.h.push(XfaEffect::Relayout);
                    Ok(Val::Null)
                }
                _ => Ok(Val::Num(0.0)),
            },
            Val::Xfa => match m.as_str() {
                "resolvenode" | "resolvenodes" => {
                    let s = self.arg_str(args, 0)?;
                    let found = resolve_som(self.h, self.h.current, &s);
                    Ok(self.set_val(found, m == "resolvenodes"))
                }
                "recalculate" => {
                    self.h.push(XfaEffect::Recalculate);
                    Ok(Val::Null)
                }
                "remerge" => {
                    self.h.push(XfaEffect::Relayout);
                    Ok(Val::Null)
                }
                _ => Ok(Val::Null),
            },
            Val::Node(i) => match m.as_str() {
                "resolvenode" | "resolvenodes" => {
                    let s = self.arg_str(args, 0)?;
                    let found = resolve_som(self.h, i, &s);
                    Ok(self.set_val(found, m == "resolvenodes"))
                }
                "recalculate" => {
                    self.h.push(XfaEffect::Recalculate);
                    Ok(Val::Null)
                }
                "remerge" => {
                    self.h.push(XfaEffect::Relayout);
                    Ok(Val::Null)
                }
                _ => Ok(Val::Null),
            },
            Val::Im(parent, name) => match m.as_str() {
                "addinstance" | "insertinstance" => Ok(self.h.add_instance(parent, &name).map_or(Val::Null, Val::Node)),
                "removeinstance" => {
                    let i = self.arg_num(args, 0)?;
                    if i.is_finite() && i >= 0.0 {
                        self.h.remove_instance(parent, &name, i as usize);
                    }
                    Ok(Val::Null)
                }
                "setinstances" => {
                    let want = self.arg_num(args, 0)?;
                    if want.is_finite() && want >= 0.0 {
                        self.h.set_instances(parent, &name, want as usize);
                    }
                    Ok(Val::Null)
                }
                _ => Ok(Val::Null),
            },
            Val::Event | Val::Sink | Val::List(_) => Ok(Val::Null),
            Val::Null => Err(format!("no object to call {method} on")),
            Val::Num(_) | Val::Str(_) => Err(format!("{} has no method {method}", self.to_str(&base))),
        }
    }

    fn set_val(&self, found: Vec<usize>, many: bool) -> Val {
        if many {
            return Val::List(found.into_iter().map(Val::Node).collect());
        }
        match found.as_slice() {
            [] => Val::Null,
            [one] => Val::Node(*one),
            all => Val::List(all.iter().map(|i| Val::Node(*i)).collect()),
        }
    }

    // ── assignment

    fn assign(&mut self, target: &Expr, v: Val) -> R<()> {
        let v = self.deref(v);
        match target {
            Expr::Ident(name) => {
                if self.set_var(name, v.clone()) {
                    return Ok(());
                }
                let t = self.ident(name)?;
                self.assign_to(t, "rawValue", v)
            }
            Expr::Member(base, prop) => {
                let b = self.eval(base)?;
                self.assign_to(b, prop, v)
            }
            Expr::Index(..) | Expr::Descend(..) => {
                let t = self.eval(target)?;
                self.assign_to(t, "rawValue", v)
            }
            _ => Err("cannot assign to that".into()),
        }
    }

    fn assign_to(&mut self, target: Val, prop: &str, v: Val) -> R<()> {
        match target {
            Val::Node(i) => {
                let Some(n) = self.node(i) else { return Ok(()) };
                let som = n.som.clone();
                match prop {
                    "rawValue" | "value" | "formattedValue" => {
                        let text = self.to_str(&v);
                        if !self.h.set_value(i, text) {
                            return Err(format!("{} is not a field", if som.is_empty() { "the form" } else { som.as_str() }));
                        }
                    }
                    "presence" => {
                        let p = self.to_str(&v);
                        if matches!(p.as_str(), "visible" | "invisible" | "hidden" | "inactive") {
                            self.h.set_state(i, XfaEffect::SetPresence { som, presence: p });
                        }
                    }
                    "access" => {
                        let a = self.to_str(&v);
                        if matches!(a.as_str(), "open" | "readOnly" | "protected" | "nonInteractive") {
                            self.h.set_state(i, XfaEffect::SetAccess { som, access: a });
                        }
                    }
                    other => {
                        // `page1.qty = 7`: the child object named by the last step takes the value.
                        if let Some(child) = other.strip_prefix('_') {
                            if child_named(self.h, i, child).is_some() {
                                return self.assign_to(Val::Im(i, child.to_string()), "count", v);
                            }
                            return Ok(());
                        }
                        let kids = instances(self.h, i, other);
                        if kids.is_empty() {
                            return Err(format!("{som} has no {other} to assign to"));
                        }
                        for k in kids {
                            // Every destination copies the value: charge each copy.
                            self.charge(self.cost(&v))?;
                            self.assign_to(Val::Node(k), "rawValue", v.clone())?;
                        }
                    }
                }
                Ok(())
            }
            Val::List(items) => {
                for it in items {
                    self.charge(self.cost(&v))?;
                    self.assign_to(it, prop, v.clone())?;
                }
                Ok(())
            }
            Val::Im(parent, name) if prop.eq_ignore_ascii_case("count") => {
                let want = self.to_num(&v);
                if want.is_finite() && want >= 0.0 {
                    self.h.set_instances(parent, &name, want as usize);
                }
                Ok(())
            }
            _ => Ok(()),
        }
    }

    // ── statements

    /// Run a block; returns how it ended and the value of its last expression statement.
    pub(super) fn block(&mut self, body: &[Stmt]) -> R<(Flow, Val)> {
        // Nesting of statements counts like nesting of expressions: both are stack.
        self.depth += 1;
        if self.depth > self.max_depth {
            self.depth -= 1;
            return Err(TOO_DEEP.into());
        }
        let r = self.block_inner(body);
        self.depth -= 1;
        r
    }

    fn block_inner(&mut self, body: &[Stmt]) -> R<(Flow, Val)> {
        let mut last = Val::Null;
        for s in body {
            self.step()?;
            match self.exec(s)? {
                (Flow::Next, v) => {
                    if let Some(v) = v {
                        last = v;
                    }
                    if self.exited {
                        return Ok((Flow::Exit, last));
                    }
                }
                (flow, _) => return Ok((flow, last)),
            }
        }
        Ok((Flow::Next, last))
    }

    fn exec(&mut self, s: &Stmt) -> R<(Flow, Option<Val>)> {
        match s {
            Stmt::Var(name, init) => {
                let v = match init {
                    Some(e) => {
                        let v = self.eval(e)?;
                        self.take(v)?
                    }
                    None => Val::Null,
                };
                self.declare(name, v.clone());
                Ok((Flow::Next, Some(v)))
            }
            Stmt::Assign(target, e) => {
                let v = self.eval(e)?;
                let v = self.take(v)?;
                self.assign(target, v.clone())?;
                Ok((Flow::Next, Some(v)))
            }
            Stmt::Expr(e) => {
                let v = self.eval(e)?;
                let v = self.take(v)?;
                Ok((Flow::Next, Some(v)))
            }
            Stmt::If(arms, otherwise) => {
                for (cond, body) in arms {
                    let c = self.eval(cond)?;
                    if self.truthy(&c) {
                        let (f, v) = self.block(body)?;
                        return Ok((f, Some(v)));
                    }
                }
                if let Some(body) = otherwise {
                    let (f, v) = self.block(body)?;
                    return Ok((f, Some(v)));
                }
                Ok((Flow::Next, None))
            }
            Stmt::While(cond, body) => {
                loop {
                    let c = self.eval(cond)?;
                    if !self.truthy(&c) {
                        break;
                    }
                    self.iterate()?;
                    match self.block(body)? {
                        (Flow::Break, _) => break,
                        (Flow::Return(v), _) => return Ok((Flow::Return(v), None)),
                        (Flow::Exit, _) => return Ok((Flow::Exit, None)),
                        _ => {}
                    }
                }
                Ok((Flow::Next, None))
            }
            Stmt::For { var, from, to, up, step, body } => {
                let start = {
                    let v = self.eval(from)?;
                    self.to_num(&v)
                };
                let end = {
                    let v = self.eval(to)?;
                    self.to_num(&v)
                };
                let by = match step {
                    Some(e) => {
                        let v = self.eval(e)?;
                        self.to_num(&v).abs()
                    }
                    None => 1.0,
                };
                if by.is_nan() || by <= 0.0 || !start.is_finite() || !end.is_finite() {
                    return Err("bad for loop bounds".into());
                }
                self.declare(var, Val::Num(start));
                let mut i = start;
                while if *up { i <= end } else { i >= end } {
                    self.iterate()?;
                    self.set_var(var, Val::Num(i));
                    match self.block(body)? {
                        (Flow::Break, _) => break,
                        (Flow::Return(v), _) => return Ok((Flow::Return(v), None)),
                        (Flow::Exit, _) => return Ok((Flow::Exit, None)),
                        _ => {}
                    }
                    i = if *up { i + by } else { i - by };
                }
                Ok((Flow::Next, None))
            }
            Stmt::Foreach { var, list, body } => {
                let items = self.arg_values_keeping(list, true)?;
                self.declare(var, Val::Null);
                for it in items {
                    self.iterate()?;
                    self.set_var(var, it);
                    match self.block(body)? {
                        (Flow::Break, _) => break,
                        (Flow::Return(v), _) => return Ok((Flow::Return(v), None)),
                        (Flow::Exit, _) => return Ok((Flow::Exit, None)),
                        _ => {}
                    }
                }
                Ok((Flow::Next, None))
            }
            Stmt::Func { name, f } => {
                self.funcs.insert(name.to_ascii_lowercase(), Rc::clone(f));
                Ok((Flow::Next, None))
            }
            Stmt::Return(e) => {
                let v = match e {
                    Some(e) => {
                        let v = self.eval(e)?;
                        self.take(v)?
                    }
                    None => Val::Null,
                };
                Ok((Flow::Return(v), None))
            }
            Stmt::Break => Ok((Flow::Break, None)),
            Stmt::Continue => Ok((Flow::Continue, None)),
            Stmt::Exit => Ok((Flow::Exit, None)),
            Stmt::Throw(e) => {
                let v = self.eval(e)?;
                self.charge(self.cost(&v))?;
                // The thrown value becomes the error message the caller keeps: cut it as
                // messages are.
                Err(clip(self.to_str(&v)))
            }
        }
    }

    fn iterate(&mut self) -> R<()> {
        if self.loops_left == 0 {
            return Err(LOOPED_TOO_MUCH.into());
        }
        self.loops_left -= 1;
        Ok(())
    }
}
