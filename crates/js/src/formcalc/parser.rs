use super::ast::{Expr, Idx, Stmt};
use super::lexer::{Tok, Token, lex};
use super::{Func, MAX_CHAIN, MAX_PARSE_DEPTH, Rc};

struct Parser {
    toks: Vec<Token>,
    at: usize,
    depth: usize,
    /// Operators and accessors chained so far in the expression being parsed.
    chain: usize,
}

fn kw(t: &Tok, word: &str) -> bool {
    matches!(t, Tok::Ident(s) if s.eq_ignore_ascii_case(word))
}

impl Parser {
    fn peek(&self) -> &Tok {
        self.toks.get(self.at).map_or(&Tok::Eof, |t| &t.tok)
    }

    fn line(&self) -> usize {
        self.toks.get(self.at).map_or(0, |t| t.line)
    }

    fn next(&mut self) -> Tok {
        let t = self.peek().clone();
        self.at += 1;
        t
    }

    fn is_op(&self, op: &str) -> bool {
        matches!(self.peek(), Tok::Op(o) if *o == op)
    }

    fn eat_op(&mut self, op: &str) -> bool {
        if self.is_op(op) {
            self.at += 1;
            true
        } else {
            false
        }
    }

    fn expect_op(&mut self, op: &str) -> Result<(), String> {
        if self.eat_op(op) { Ok(()) } else { Err(format!("line {}: expected {op}", self.line())) }
    }

    fn eat_kw(&mut self, word: &str) -> bool {
        if kw(self.peek(), word) {
            self.at += 1;
            true
        } else {
            false
        }
    }

    fn expect_kw(&mut self, word: &str) -> Result<(), String> {
        if self.eat_kw(word) { Ok(()) } else { Err(format!("line {}: expected {word}", self.line())) }
    }

    fn skip_newlines(&mut self) {
        while matches!(self.peek(), Tok::Newline) {
            self.at += 1;
        }
    }

    fn enter(&mut self) -> Result<(), String> {
        self.depth += 1;
        if self.depth > MAX_PARSE_DEPTH {
            return Err("the script is nested too deeply".into());
        }
        Ok(())
    }

    fn leave(&mut self) {
        self.depth = self.depth.saturating_sub(1);
    }

    /// One more link in an operator or accessor chain.
    fn link(&mut self) -> Result<(), String> {
        self.chain += 1;
        if self.chain > MAX_CHAIN {
            return Err(format!("line {}: an expression is too long", self.line()));
        }
        Ok(())
    }

    /// Statements up to one of `enders` (not consumed).
    fn block(&mut self, enders: &[&str]) -> Result<Vec<Stmt>, String> {
        self.enter()?;
        let mut out = Vec::new();
        loop {
            self.skip_newlines();
            match self.peek() {
                Tok::Eof => break,
                Tok::Ident(w) if enders.iter().any(|e| w.eq_ignore_ascii_case(e)) => break,
                _ => out.push(self.statement()?),
            }
        }
        self.leave();
        Ok(out)
    }

    fn statement(&mut self) -> Result<Stmt, String> {
        let line = self.line();
        match self.peek().clone() {
            Tok::Ident(w) if w.eq_ignore_ascii_case("var") => {
                self.at += 1;
                let Tok::Ident(name) = self.next() else { return Err(format!("line {line}: expected a name after var")) };
                let init = if self.eat_op("=") { Some(self.top_expr()?) } else { None };
                Ok(Stmt::Var(name, init))
            }
            Tok::Ident(w) if w.eq_ignore_ascii_case("if") => {
                self.at += 1;
                let mut arms = Vec::new();
                let cond = self.top_expr()?;
                self.expect_kw("then")?;
                let body = self.block(&["elseif", "else", "endif"])?;
                arms.push((cond, body));
                let mut otherwise = None;
                loop {
                    if self.eat_kw("elseif") {
                        let c = self.top_expr()?;
                        self.expect_kw("then")?;
                        let b = self.block(&["elseif", "else", "endif"])?;
                        arms.push((c, b));
                    } else if self.eat_kw("else") {
                        otherwise = Some(self.block(&["endif"])?);
                    } else {
                        self.expect_kw("endif")?;
                        break;
                    }
                }
                Ok(Stmt::If(arms, otherwise))
            }
            Tok::Ident(w) if w.eq_ignore_ascii_case("while") => {
                self.at += 1;
                let cond = self.top_expr()?;
                self.expect_kw("do")?;
                let body = self.block(&["endwhile"])?;
                self.expect_kw("endwhile")?;
                Ok(Stmt::While(cond, body))
            }
            Tok::Ident(w) if w.eq_ignore_ascii_case("for") => {
                self.at += 1;
                let Tok::Ident(var) = self.next() else { return Err(format!("line {line}: expected a variable after for")) };
                self.expect_op("=")?;
                let from = self.top_expr()?;
                let up = if self.eat_kw("upto") {
                    true
                } else if self.eat_kw("downto") {
                    false
                } else {
                    return Err(format!("line {line}: expected upto or downto"));
                };
                let to = self.top_expr()?;
                let step = if self.eat_kw("step") { Some(self.top_expr()?) } else { None };
                self.expect_kw("do")?;
                let body = self.block(&["endfor"])?;
                self.expect_kw("endfor")?;
                Ok(Stmt::For { var, from, to, up, step, body })
            }
            Tok::Ident(w) if w.eq_ignore_ascii_case("foreach") => {
                self.at += 1;
                let Tok::Ident(var) = self.next() else { return Err(format!("line {line}: expected a variable after foreach")) };
                self.expect_kw("in")?;
                self.expect_op("(")?;
                let mut list = Vec::new();
                if !self.is_op(")") {
                    loop {
                        list.push(self.top_expr()?);
                        if !self.eat_op(",") {
                            break;
                        }
                    }
                }
                self.expect_op(")")?;
                self.expect_kw("do")?;
                let body = self.block(&["endfor"])?;
                self.expect_kw("endfor")?;
                Ok(Stmt::Foreach { var, list, body })
            }
            Tok::Ident(w) if w.eq_ignore_ascii_case("func") => {
                self.at += 1;
                let Tok::Ident(name) = self.next() else { return Err(format!("line {line}: expected a name after func")) };
                self.expect_op("(")?;
                let mut params = Vec::new();
                if !self.is_op(")") {
                    loop {
                        let Tok::Ident(p) = self.next() else { return Err(format!("line {line}: expected a parameter name")) };
                        params.push(p);
                        if !self.eat_op(",") {
                            break;
                        }
                    }
                }
                self.expect_op(")")?;
                self.expect_kw("do")?;
                let body = self.block(&["endfunc"])?;
                self.expect_kw("endfunc")?;
                Ok(Stmt::Func { name, f: Rc::new(Func { params, body }) })
            }
            Tok::Ident(w) if w.eq_ignore_ascii_case("return") => {
                self.at += 1;
                let v = if matches!(self.peek(), Tok::Newline | Tok::Eof) { None } else { Some(self.top_expr()?) };
                Ok(Stmt::Return(v))
            }
            Tok::Ident(w) if w.eq_ignore_ascii_case("break") => {
                self.at += 1;
                Ok(Stmt::Break)
            }
            Tok::Ident(w) if w.eq_ignore_ascii_case("continue") => {
                self.at += 1;
                Ok(Stmt::Continue)
            }
            Tok::Ident(w) if w.eq_ignore_ascii_case("exit") => {
                self.at += 1;
                Ok(Stmt::Exit)
            }
            Tok::Ident(w) if w.eq_ignore_ascii_case("throw") => {
                self.at += 1;
                Ok(Stmt::Throw(self.top_expr()?))
            }
            Tok::Ident(w) if w.eq_ignore_ascii_case("do") => {
                // `do … end`: a block as one statement.
                self.at += 1;
                let body = self.block(&["end"])?;
                self.expect_kw("end")?;
                Ok(Stmt::If(vec![(Expr::Num(1.0), body)], None))
            }
            _ => {
                let e = self.top_expr()?;
                if self.eat_op("=") {
                    let v = self.top_expr()?;
                    return Ok(Stmt::Assign(e, v));
                }
                Ok(Stmt::Expr(e))
            }
        }
    }

    /// An expression that starts a tree of its own (a condition, a bound, a statement's
    /// expression): its operator chain is counted from zero, whatever came before it.
    fn top_expr(&mut self) -> Result<Expr, String> {
        self.chain = 0;
        self.expr()
    }

    fn expr(&mut self) -> Result<Expr, String> {
        self.enter()?;
        let e = self.or_expr();
        self.leave();
        e
    }

    fn or_expr(&mut self) -> Result<Expr, String> {
        let mut l = self.and_expr()?;
        loop {
            if self.eat_op("|") || self.eat_kw("or") {
                self.link()?;
                let r = self.and_expr()?;
                l = Expr::Bin("|", Box::new(l), Box::new(r));
            } else {
                return Ok(l);
            }
        }
    }

    fn and_expr(&mut self) -> Result<Expr, String> {
        let mut l = self.cmp_expr()?;
        loop {
            if self.eat_op("&") || self.eat_kw("and") {
                self.link()?;
                let r = self.cmp_expr()?;
                l = Expr::Bin("&", Box::new(l), Box::new(r));
            } else {
                return Ok(l);
            }
        }
    }

    fn cmp_expr(&mut self) -> Result<Expr, String> {
        let mut l = self.add_expr()?;
        loop {
            let op = match self.peek() {
                Tok::Op("==") => "==",
                Tok::Op("<>") => "<>",
                Tok::Op("<=") => "<=",
                Tok::Op(">=") => ">=",
                Tok::Op("<") => "<",
                Tok::Op(">") => ">",
                Tok::Ident(w) if w.eq_ignore_ascii_case("eq") => "==",
                Tok::Ident(w) if w.eq_ignore_ascii_case("ne") => "<>",
                Tok::Ident(w) if w.eq_ignore_ascii_case("le") => "<=",
                Tok::Ident(w) if w.eq_ignore_ascii_case("ge") => ">=",
                Tok::Ident(w) if w.eq_ignore_ascii_case("lt") => "<",
                Tok::Ident(w) if w.eq_ignore_ascii_case("gt") => ">",
                _ => return Ok(l),
            };
            self.at += 1;
            self.link()?;
            let r = self.add_expr()?;
            l = Expr::Bin(op, Box::new(l), Box::new(r));
        }
    }

    fn add_expr(&mut self) -> Result<Expr, String> {
        let mut l = self.mul_expr()?;
        loop {
            let op = match self.peek() {
                Tok::Op("+") => "+",
                Tok::Op("-") => "-",
                _ => return Ok(l),
            };
            self.at += 1;
            self.link()?;
            let r = self.mul_expr()?;
            l = Expr::Bin(op, Box::new(l), Box::new(r));
        }
    }

    fn mul_expr(&mut self) -> Result<Expr, String> {
        let mut l = self.unary()?;
        loop {
            let op = match self.peek() {
                Tok::Op("*") => "*",
                Tok::Op("/") => "/",
                _ => return Ok(l),
            };
            self.at += 1;
            self.link()?;
            let r = self.unary()?;
            l = Expr::Bin(op, Box::new(l), Box::new(r));
        }
    }

    fn unary(&mut self) -> Result<Expr, String> {
        self.enter()?;
        let e = if self.eat_op("-") {
            Ok(Expr::Neg(Box::new(self.unary()?)))
        } else if self.eat_op("+") {
            self.unary()
        } else if self.eat_kw("not") || matches!(self.peek(), Tok::Ident(w) if w == "!") {
            if matches!(self.peek(), Tok::Ident(w) if w == "!") {
                self.at += 1;
            }
            Ok(Expr::Not(Box::new(self.unary()?)))
        } else {
            self.postfix()
        };
        self.leave();
        e
    }

    fn postfix(&mut self) -> Result<Expr, String> {
        let mut e = self.primary()?;
        loop {
            if matches!(self.peek(), Tok::Op("." | ".." | "[" | "(")) {
                self.link()?;
            }
            if self.eat_op(".") {
                let Tok::Ident(name) = self.next() else { return Err(format!("line {}: expected a name after .", self.line())) };
                e = Expr::Member(Box::new(e), name);
            } else if self.eat_op("..") {
                let Tok::Ident(name) = self.next() else { return Err(format!("line {}: expected a name after ..", self.line())) };
                e = Expr::Descend(Box::new(e), name);
            } else if self.eat_op("[") {
                let sign = match self.peek() {
                    Tok::Op("-") => Some(-1),
                    Tok::Op("+") => Some(1),
                    _ => None,
                };
                let relative = sign.and_then(|sign| match (self.toks.get(self.at + 1).map(|t| &t.tok), self.toks.get(self.at + 2).map(|t| &t.tok)) {
                    (Some(Tok::Num(n)), Some(Tok::Op("]"))) if n.is_finite() && *n >= 0.0 && *n < 1e6 => Some(sign * (*n as i64)),
                    _ => None,
                });
                let idx = if let Some(k) = relative {
                    self.at += 2;
                    Idx::Rel(k)
                } else if self.eat_op("*") {
                    Idx::All
                } else {
                    Idx::Abs(Box::new(self.expr()?))
                };
                self.expect_op("]")?;
                e = Expr::Index(Box::new(e), idx);
            } else if self.eat_op("(") {
                let mut args = Vec::new();
                self.skip_newlines();
                if !self.is_op(")") {
                    loop {
                        args.push(self.expr()?);
                        self.skip_newlines();
                        if !self.eat_op(",") {
                            break;
                        }
                        self.skip_newlines();
                    }
                }
                self.expect_op(")")?;
                e = Expr::Call(Box::new(e), args);
            } else {
                return Ok(e);
            }
        }
    }

    fn primary(&mut self) -> Result<Expr, String> {
        let line = self.line();
        match self.next() {
            Tok::Num(n) => Ok(Expr::Num(n)),
            Tok::Str(s) => Ok(Expr::Str(s)),
            Tok::Op("(") => {
                self.skip_newlines();
                let e = self.expr()?;
                self.skip_newlines();
                self.expect_op(")")?;
                Ok(e)
            }
            Tok::Ident(w) if w.eq_ignore_ascii_case("null") => Ok(Expr::Null),
            Tok::Ident(w) if w.eq_ignore_ascii_case("nan") => Ok(Expr::Num(f64::NAN)),
            Tok::Ident(w) if w.eq_ignore_ascii_case("infinity") => Ok(Expr::Num(f64::INFINITY)),
            Tok::Ident(w) if w.eq_ignore_ascii_case("true") => Ok(Expr::Num(1.0)),
            Tok::Ident(w) if w.eq_ignore_ascii_case("false") => Ok(Expr::Num(0.0)),
            Tok::Ident(w) => Ok(Expr::Ident(w)),
            t => Err(format!("line {line}: unexpected {}", describe(&t))),
        }
    }
}

fn describe(t: &Tok) -> String {
    match t {
        Tok::Num(n) => format!("number {n}"),
        Tok::Str(s) => format!("string {s:?}"),
        Tok::Ident(s) => s.clone(),
        Tok::Op(o) => format!("{o:?}"),
        Tok::Newline => "end of line".into(),
        Tok::Eof => "end of script".into(),
    }
}

pub(super) fn parse(src: &str) -> Result<Vec<Stmt>, String> {
    let toks = lex(src)?;
    let mut p = Parser { toks, at: 0, depth: 0, chain: 0 };
    let body = p.block(&[])?;
    match p.peek() {
        Tok::Eof => Ok(body),
        t => Err(format!("line {}: unexpected {}", p.line(), describe(t))),
    }
}
