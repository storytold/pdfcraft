//! FormCalc (XFA 3.3 part 8), the default scripting language of older Designer forms: a lexer,
//! a parser and a tree-walking evaluator over the same form table and effects as the JavaScript
//! model ([`crate::xfa`]), so the engine treats both languages alike.
//!
//! Covered: expressions with FormCalc's operators and coercions, `var`, assignment to fields
//! and properties, `if`/`elseif`/`else`, `while`, `for … upto/downto … step`, `foreach`,
//! `func … endfunc`, `break`/`continue`/`return`/`exit`, accessors (`$`, `$host`, `$form`,
//! `$record`, `$layout`, `$event`, `xfa.…`, `..name`, `[n]`, `[*]`, `_name` instance managers),
//! and the arithmetic, logical, string, date/time, financial and unit built-in functions.
//! `Get`, `Post`, `Put` (network) refuse.

use std::collections::{HashMap, HashSet};
use std::rc::Rc;

use crate::Limits;
use crate::xfa::{
    HNode, XHost, XfaDoc, XfaEffect, XfaEvent, XfaKind, XfaNode, XfaOutcome, child_named, clip, clip_result, descendants_named, instances,
    resolve_som, run_within,
};

/// Deepest nesting the parser follows.
const MAX_PARSE_DEPTH: usize = 64;
/// Most statements and expressions evaluated in one run (loops included).
const MAX_STEPS: u64 = 5_000_000;
/// Longest string a script may build.
const MAX_STRING: usize = 1 << 20;
/// Most elements a list may hold.
const MAX_LIST: usize = 100_000;
/// Most operators and accessors in one expression (`a + b + …`, `a.b.c…`, brackets included):
/// the tree is as deep as the chain, and dropping or cloning it recurses.
const MAX_CHAIN: usize = 1000;
// The errors a run stops with when it reaches a limit (rather than a fault in the script).
const RAN_TOO_LONG: &str = "the script ran too long";
const TOO_DEEP: &str = "the script is nested too deeply";
const CALLS_TOO_DEEP: &str = "functions are nested too deeply";
const LOOPED_TOO_MUCH: &str = "the script looped too many times";
const LIST_TOO_LONG: &str = "a list is too long";
const STRING_TOO_LONG: &str = "a string is too long";

/// Whether `e` is a limit the run reached: those stop the script even where a fault is
/// otherwise an answer (`Exists`).
fn is_limit(e: &str) -> bool {
    [RAN_TOO_LONG, TOO_DEEP, CALLS_TOO_DEEP, LOOPED_TOO_MUCH, LIST_TOO_LONG, STRING_TOO_LONG].contains(&e)
}

/// Steps between looks at the clock.
#[cfg(not(target_arch = "wasm32"))]
const CLOCK_EVERY: u64 = 256;

// ── lexer ───────────────────────────────────────────────────────────────────────────────────

#[derive(Clone, Debug, PartialEq)]
enum Tok {
    Num(f64),
    Str(String),
    Ident(String),
    Op(&'static str),
    Newline,
    Eof,
}

#[derive(Clone, Debug, PartialEq)]
struct Token {
    tok: Tok,
    line: usize,
}

const OPS: [&str; 20] = ["<=", ">=", "==", "<>", "..", "(", ")", "[", "]", ".", ",", "=", "<", ">", "+", "-", "*", "/", "&", "|"];

fn lex(src: &str) -> Result<Vec<Token>, String> {
    let chars: Vec<char> = src.chars().collect();
    let mut out = Vec::new();
    let mut i = 0;
    let mut line = 1;
    let mut depth = 0usize;
    while i < chars.len() {
        let c = chars[i];
        match c {
            '\n' => {
                line += 1;
                i += 1;
                // Newlines inside brackets continue the expression.
                if depth == 0 {
                    out.push(Token { tok: Tok::Newline, line });
                }
            }
            ' ' | '\t' | '\r' => i += 1,
            ';' => {
                while i < chars.len() && chars[i] != '\n' {
                    i += 1;
                }
            }
            '/' if chars.get(i + 1) == Some(&'/') => {
                while i < chars.len() && chars[i] != '\n' {
                    i += 1;
                }
            }
            '"' => {
                let mut s = String::new();
                i += 1;
                loop {
                    match chars.get(i) {
                        None => return Err(format!("line {line}: unterminated string")),
                        Some('"') if chars.get(i + 1) == Some(&'"') => {
                            s.push('"');
                            i += 2;
                        }
                        Some('"') => {
                            i += 1;
                            break;
                        }
                        Some(&ch) => {
                            if ch == '\n' {
                                line += 1;
                            }
                            s.push(ch);
                            i += 1;
                        }
                    }
                    if s.len() > MAX_STRING {
                        return Err(format!("line {line}: a string is too long"));
                    }
                }
                out.push(Token { tok: Tok::Str(s), line });
            }
            c if c.is_ascii_digit() || (c == '.' && chars.get(i + 1).is_some_and(|d| d.is_ascii_digit())) => {
                let start = i;
                while i < chars.len() && (chars[i].is_ascii_digit() || (chars[i] == '.' && chars.get(i + 1) != Some(&'.'))) {
                    i += 1;
                }
                if i < chars.len() && (chars[i] == 'e' || chars[i] == 'E') {
                    let mut j = i + 1;
                    if j < chars.len() && (chars[j] == '+' || chars[j] == '-') {
                        j += 1;
                    }
                    if j < chars.len() && chars[j].is_ascii_digit() {
                        i = j;
                        while i < chars.len() && chars[i].is_ascii_digit() {
                            i += 1;
                        }
                    }
                }
                let text: String = chars[start..i].iter().collect();
                let n: f64 = text.parse().map_err(|_| format!("line {line}: bad number {text}"))?;
                out.push(Token { tok: Tok::Num(n), line });
            }
            '!' => {
                out.push(Token { tok: Tok::Ident("!".into()), line });
                i += 1;
            }
            c if c.is_alphabetic() || c == '_' || c == '$' => {
                let start = i;
                i += 1;
                while i < chars.len() && (chars[i].is_alphanumeric() || chars[i] == '_' || chars[i] == '$') {
                    i += 1;
                }
                let text: String = chars[start..i].iter().collect();
                out.push(Token { tok: Tok::Ident(text), line });
            }
            _ => {
                let rest: String = chars[i..chars.len().min(i + 2)].iter().collect();
                let op = OPS.iter().find(|o| rest.starts_with(*o)).copied().ok_or_else(|| format!("line {line}: unexpected character {c:?}"))?;
                match op {
                    "(" | "[" => depth += 1,
                    ")" | "]" => depth = depth.saturating_sub(1),
                    _ => {}
                }
                out.push(Token { tok: Tok::Op(op), line });
                i += op.len();
            }
        }
    }
    out.push(Token { tok: Tok::Newline, line });
    out.push(Token { tok: Tok::Eof, line });
    Ok(out)
}

// ── syntax ──────────────────────────────────────────────────────────────────────────────────

#[derive(Clone, Debug, PartialEq)]
enum Expr {
    Num(f64),
    Str(String),
    Null,
    Ident(String),
    /// `base.name`
    Member(Box<Expr>, String),
    /// `base[n]`, `base[*]`, `base[-n]`
    Index(Box<Expr>, Idx),
    /// `base..name`
    Descend(Box<Expr>, String),
    Call(Box<Expr>, Vec<Expr>),
    Neg(Box<Expr>),
    Not(Box<Expr>),
    Bin(&'static str, Box<Expr>, Box<Expr>),
}

#[derive(Clone, Debug, PartialEq)]
enum Idx {
    /// `[*]`: every instance.
    All,
    /// `[n]`: the n-th instance (from 0).
    Abs(Box<Expr>),
    /// `[-n]` / `[+n]`: n instances before or after the one the script runs in.
    Rel(i64),
}

#[derive(Clone, Debug, PartialEq)]
enum Stmt {
    Var(String, Option<Expr>),
    Assign(Expr, Expr),
    Expr(Expr),
    If(Vec<(Expr, Vec<Stmt>)>, Option<Vec<Stmt>>),
    While(Expr, Vec<Stmt>),
    For { var: String, from: Expr, to: Expr, up: bool, step: Option<Expr>, body: Vec<Stmt> },
    Foreach { var: String, list: Vec<Expr>, body: Vec<Stmt> },
    Func { name: String, f: Rc<Func> },
    Return(Option<Expr>),
    Break,
    Continue,
    Exit,
    Throw(Expr),
}

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

fn parse(src: &str) -> Result<Vec<Stmt>, String> {
    let toks = lex(src)?;
    let mut p = Parser { toks, at: 0, depth: 0, chain: 0 };
    let body = p.block(&[])?;
    match p.peek() {
        Tok::Eof => Ok(body),
        t => Err(format!("line {}: unexpected {}", p.line(), describe(t))),
    }
}

// ── values ──────────────────────────────────────────────────────────────────────────────────

#[derive(Clone, Debug, PartialEq)]
enum Val {
    Null,
    Num(f64),
    Str(String),
    /// A form object, by index in the host.
    Node(usize),
    /// Several objects or values (`Row[*]`, `Row[*].Amount`).
    List(Vec<Val>),
    Host,
    Layout,
    Event,
    /// `xfa` itself (`xfa.form`, `xfa.host`, …).
    Xfa,
    /// The instance manager of `name` under `parent`.
    Im(usize, String),
    /// A property object whose contents are not modelled (`$.border.fill…`): reads give
    /// another sink, writes change nothing.
    Sink,
}

/// A number as FormCalc prints it: integers without a fraction, otherwise up to 15 digits.
fn num_text(n: f64) -> String {
    if !n.is_finite() {
        return if n.is_nan() {
            "NaN".into()
        } else if n > 0.0 {
            "Infinity".into()
        } else {
            "-Infinity".into()
        };
    }
    if n.fract() == 0.0 && n.abs() < 1e15 {
        return format!("{}", n as i64);
    }
    let s = format!("{n:.12}");
    let s = s.trim_end_matches('0').trim_end_matches('.');
    if s.is_empty() || s == "-0" { "0".into() } else { s.to_string() }
}

/// The number a string holds: its leading numeric part (`12abc` → 12), `None` for none.
fn parse_num(s: &str) -> Option<f64> {
    let t = s.trim();
    if t.is_empty() {
        return None;
    }
    let mut end = 0;
    let b = t.as_bytes();
    let mut seen_digit = false;
    let mut seen_dot = false;
    let mut seen_e = false;
    let mut i = 0;
    if i < b.len() && (b[i] == b'-' || b[i] == b'+') {
        i += 1;
    }
    while i < b.len() {
        let c = b[i];
        if c.is_ascii_digit() {
            seen_digit = true;
            end = i + 1;
        } else if c == b'.' && !seen_dot && !seen_e {
            seen_dot = true;
        } else if (c == b'e' || c == b'E') && seen_digit && !seen_e {
            seen_e = true;
            if i + 1 < b.len() && (b[i + 1] == b'-' || b[i + 1] == b'+') {
                i += 1;
            }
        } else {
            break;
        }
        i += 1;
    }
    if !seen_digit {
        return None;
    }
    t.get(..end).and_then(|p| p.parse::<f64>().ok()).filter(|v| v.is_finite())
}

fn is_numeric_text(s: &str) -> bool {
    s.trim().parse::<f64>().is_ok_and(|v| v.is_finite())
}

// ── the evaluator ───────────────────────────────────────────────────────────────────────────

enum Flow {
    Next,
    Break,
    Continue,
    Return(Val),
    Exit,
}

#[derive(Debug, PartialEq)]
struct Func {
    params: Vec<String>,
    body: Vec<Stmt>,
}

struct Interp<'h> {
    h: &'h mut XHost,
    scopes: Vec<HashMap<String, Val>>,
    /// User functions by lower-case name (FormCalc names are case-insensitive).
    funcs: HashMap<String, Rc<Func>>,
    event: XfaEvent,
    steps: u64,
    loops_left: u64,
    depth: usize,
    max_depth: usize,
    /// `exit` ran: every block ends at once.
    exited: bool,
    /// When the caller stops waiting: the script stops itself soon after (no thread is left
    /// burning a core to the loop limit).
    #[cfg(not(target_arch = "wasm32"))]
    deadline: Option<std::time::Instant>,
}

type R<T> = Result<T, String>;

impl Interp<'_> {
    fn step(&mut self) -> R<()> {
        self.charge(1)
    }

    /// `n` units of work done (a step per statement or expression, more for work on long
    /// strings and lists).
    fn charge(&mut self, n: u64) -> R<()> {
        let before = self.steps;
        self.steps = self.steps.saturating_add(n);
        if self.steps > MAX_STEPS {
            return Err(RAN_TOO_LONG.into());
        }
        #[cfg(not(target_arch = "wasm32"))]
        if self.steps / CLOCK_EVERY != before / CLOCK_EVERY
            && let Some(d) = self.deadline
            && std::time::Instant::now() > d
        {
            return Err(RAN_TOO_LONG.into());
        }
        #[cfg(target_arch = "wasm32")]
        let _ = before;
        Ok(())
    }

    /// `more` nodes into `out`, each once, never past [`MAX_LIST`].
    fn gather(&mut self, out: &mut Vec<usize>, seen: &mut HashSet<usize>, more: impl IntoIterator<Item = usize>) -> R<()> {
        for i in more {
            if seen.insert(i) {
                out.push(i);
                if out.len() > MAX_LIST {
                    return Err(LIST_TOO_LONG.into());
                }
            }
        }
        self.charge(1)
    }

    /// The nodes of `v`, each once.
    fn nodes_list(&self, v: &Val) -> Vec<usize> {
        let mut seen = HashSet::new();
        self.nodes_of(v).into_iter().filter(|i| seen.insert(*i)).collect()
    }

    fn node(&self, i: usize) -> Option<&HNode> {
        self.h.nodes.get(i).filter(|n| !n.gone)
    }

    // ── coercions

    /// A node's value as a FormCalc value.
    fn node_value(&self, i: usize) -> Val {
        let Some(n) = self.node(i) else { return Val::Null };
        if n.value.is_empty() {
            return Val::Null;
        }
        if n.numeric
            && let Some(v) = parse_num(&n.value)
        {
            return Val::Num(v);
        }
        Val::Str(n.value.clone())
    }

    /// What copying `v` (or the values it stands for) costs, in steps: one per 256 bytes, so
    /// copying long strings over and over runs out of steps even where there is no clock
    /// (wasm). A form object counts its value; a list its items too.
    fn cost(&self, v: &Val) -> u64 {
        let mut bytes = 0usize;
        let mut todo = vec![v];
        while let Some(v) = todo.pop() {
            match v {
                Val::Str(s) => bytes = bytes.saturating_add(s.len()),
                Val::Node(i) => bytes = bytes.saturating_add(self.node(*i).map_or(0, |n| n.value.len())),
                Val::List(items) => {
                    bytes = bytes.saturating_add(items.len().saturating_mul(16));
                    todo.extend(items.iter());
                }
                _ => {}
            }
        }
        (bytes / 256) as u64
    }

    /// [`Self::deref`], charged for the copy.
    fn take(&mut self, v: Val) -> R<Val> {
        self.charge(self.cost(&v))?;
        Ok(self.deref(v))
    }

    /// Objects become their values; lists of objects become lists of values.
    fn deref(&self, v: Val) -> Val {
        match v {
            Val::Node(i) => self.node_value(i),
            Val::List(items) => Val::List(items.into_iter().map(|x| self.deref(x)).collect()),
            other => other,
        }
    }

    fn to_num(&self, v: &Val) -> f64 {
        match v {
            Val::Null => 0.0,
            Val::Num(n) => *n,
            Val::Str(s) => parse_num(s).unwrap_or(0.0),
            Val::Node(i) => self.to_num(&self.node_value(*i)),
            Val::List(items) => items.first().map_or(0.0, |x| self.to_num(x)),
            _ => 0.0,
        }
    }

    fn to_str(&self, v: &Val) -> String {
        match v {
            Val::Null => String::new(),
            Val::Num(n) => num_text(*n),
            Val::Str(s) => s.clone(),
            Val::Node(i) => self.to_str(&self.node_value(*i)),
            Val::List(items) => items.first().map_or(String::new(), |x| self.to_str(x)),
            Val::Host => "host".into(),
            Val::Layout => "layout".into(),
            Val::Event => "event".into(),
            Val::Xfa => "xfa".into(),
            Val::Im(..) => "instanceManager".into(),
            Val::Sink => String::new(),
        }
    }

    fn truthy(&self, v: &Val) -> bool {
        match v {
            Val::Null => false,
            Val::Num(n) => *n != 0.0 && !n.is_nan(),
            Val::Str(s) => parse_num(s).is_some_and(|n| n != 0.0),
            Val::Node(i) => {
                let d = self.node_value(*i);
                self.truthy(&d)
            }
            Val::List(items) => !items.is_empty(),
            _ => true,
        }
    }

    fn is_null(&self, v: &Val) -> bool {
        match v {
            Val::Null => true,
            Val::Node(i) => self.node(*i).is_none_or(|n| n.value.is_empty()),
            Val::List(items) => items.is_empty(),
            _ => false,
        }
    }

    // ── scopes

    fn lookup(&self, name: &str) -> Option<Val> {
        self.scopes.iter().rev().find_map(|s| s.get(name).cloned())
    }

    fn set_var(&mut self, name: &str, v: Val) -> bool {
        for s in self.scopes.iter_mut().rev() {
            if let Some(slot) = s.get_mut(name) {
                *slot = v;
                return true;
            }
        }
        false
    }

    fn declare(&mut self, name: &str, v: Val) {
        if let Some(s) = self.scopes.last_mut() {
            s.insert(name.to_string(), v);
        }
    }

    // ── accessors

    /// The objects an identifier names from the current node: `$`, the special objects, an
    /// instance manager (`_name`), a variable, or a form object by name (children first, then
    /// the containing subforms).
    fn ident(&self, name: &str) -> R<Val> {
        if let Some(v) = self.lookup(name) {
            return Ok(v);
        }
        let lower = name.to_ascii_lowercase();
        Ok(match lower.as_str() {
            "$" | "this" => Val::Node(self.h.current),
            "$host" => Val::Host,
            "$layout" => Val::Layout,
            "$event" => Val::Event,
            "$form" | "$data" | "$dataset" | "$datasets" => Val::Node(0),
            "$record" => Val::Node(if self.node(1).is_some() { 1 } else { 0 }),
            "$template" | "$xfa" => Val::Xfa,
            "xfa" => Val::Xfa,
            _ => {
                if let Some(child) = name.strip_prefix('_') {
                    // The instance manager of `child`, looked for like a name.
                    let found = resolve_som(self.h, self.h.current, child);
                    let Some(&first) = found.first() else { return Err(format!("unknown instance manager _{child}")) };
                    let parent = self.node(first).and_then(|n| n.parent).ok_or_else(|| format!("unknown instance manager _{child}"))?;
                    return Ok(Val::Im(parent, child.to_string()));
                }
                let found = resolve_som(self.h, self.h.current, name);
                match found.as_slice() {
                    [] => return Err(format!("unknown name {name}")),
                    [one] => Val::Node(*one),
                    many => Val::List(many.iter().map(|i| Val::Node(*i)).collect()),
                }
            }
        })
    }

    /// Every object a value stands for.
    fn nodes_of(&self, v: &Val) -> Vec<usize> {
        match v {
            Val::Node(i) => vec![*i],
            Val::List(items) => items.iter().flat_map(|x| self.nodes_of(x)).collect(),
            _ => Vec::new(),
        }
    }

    fn member(&mut self, base: Val, name: &str) -> R<Val> {
        match base {
            Val::Node(i) => self.node_member(i, name),
            Val::List(items) => {
                let mut out = Vec::new();
                let mut seen = HashSet::new();
                for it in items {
                    match self.member(it, name)? {
                        Val::List(more) => {
                            for m in more {
                                self.list_push(&mut out, &mut seen, m)?;
                            }
                        }
                        Val::Null => {}
                        v => self.list_push(&mut out, &mut seen, v)?,
                    }
                }
                Ok(match out.len() {
                    0 => Val::Null,
                    1 => out.remove(0),
                    _ => Val::List(out),
                })
            }
            Val::Host => Ok(match name.to_ascii_lowercase().as_str() {
                "numpages" => Val::Num(self.h.doc.page_count as f64),
                "currentpage" => Val::Num(self.h.doc.page as f64),
                "name" => Val::Str("Acrobat".into()),
                "apptype" => Val::Str("Exchange-Pro".into()),
                "version" => Val::Str("11".into()),
                "variation" => Val::Str("Full".into()),
                "language" => Val::Str("en".into()),
                "platform" => Val::Str(crate::platform().into()),
                "title" => Val::Str(self.h.doc.file_name.clone()),
                "calculationsenabled" | "validationsenabled" => Val::Num(1.0),
                _ => Val::Sink,
            }),
            Val::Layout => Ok(Val::Sink),
            Val::Event => Ok(match name.to_ascii_lowercase().as_str() {
                "name" => Val::Str(self.event.activity.clone()),
                "newtext" | "fulltext" => Val::Str(self.event.new_text.clone()),
                "prevtext" => Val::Str(self.event.prev_text.clone()),
                "change" => Val::Str(String::new()),
                "target" => Val::Node(self.h.current),
                "selstart" | "selend" | "cancelaction" | "reenter" | "shift" | "modifier" | "keydown" | "commitkey" => Val::Num(0.0),
                _ => Val::Sink,
            }),
            Val::Xfa => Ok(match name.to_ascii_lowercase().as_str() {
                "form" | "datasets" | "data" => Val::Node(0),
                "record" => Val::Node(if self.node(1).is_some() { 1 } else { 0 }),
                "host" => Val::Host,
                "layout" => Val::Layout,
                "event" => Val::Event,
                _ => Val::Sink,
            }),
            Val::Im(parent, child) => Ok(match name.to_ascii_lowercase().as_str() {
                "count" => Val::Num(instances(self.h, parent, &child).len() as f64),
                "min" => Val::Num(instances(self.h, parent, &child).first().and_then(|i| self.node(*i)).map_or(0.0, |n| n.occur_min as f64)),
                "max" => Val::Num(
                    instances(self.h, parent, &child).first().and_then(|i| self.node(*i)).and_then(|n| n.occur_max).map_or(-1.0, |m| m as f64),
                ),
                "classname" => Val::Str("instanceManager".into()),
                _ => Val::Sink,
            }),
            Val::Sink => Ok(Val::Sink),
            Val::Null => Err(format!("no object to take {name} from")),
            Val::Num(_) | Val::Str(_) => Err(format!("{} has no property {name}", self.to_str(&base))),
        }
    }

    /// `v` onto `out`: a node only once, never past [`MAX_LIST`].
    fn list_push(&mut self, out: &mut Vec<Val>, seen: &mut HashSet<usize>, v: Val) -> R<()> {
        if let Val::Node(i) = v
            && !seen.insert(i)
        {
            return Ok(());
        }
        out.push(v);
        if out.len() > MAX_LIST {
            return Err(LIST_TOO_LONG.into());
        }
        self.charge(1)
    }

    fn node_member(&mut self, i: usize, name: &str) -> R<Val> {
        let Some(n) = self.node(i) else { return Ok(Val::Null) };
        Ok(match name {
            "rawValue" | "value" | "formattedValue" => self.node_value(i),
            "name" => Val::Str(n.name.clone()),
            "somExpression" => Val::Str(n.som.clone()),
            "className" => Val::Str(n.kind.class_name().into()),
            "index" | "instanceIndex" => Val::Num(n.index as f64),
            "presence" => Val::Str(n.presence.clone()),
            "access" => Val::Str(n.access.clone()),
            "isNull" => Val::Num(if n.value.is_empty() { 1.0 } else { 0.0 }),
            "mandatory" => Val::Str("disabled".into()),
            "parent" => n.parent.map_or(Val::Null, Val::Node),
            "nodes" => Val::List(n.children.iter().copied().filter(|c| self.node(*c).is_some()).map(Val::Node).collect()),
            "all" => match n.parent {
                Some(p) => Val::List(instances(self.h, p, &n.name).into_iter().map(Val::Node).collect()),
                None => Val::Node(i),
            },
            "instanceManager" => match n.parent {
                Some(p) => Val::Im(p, n.name.clone()),
                None => Val::Null,
            },
            "border" | "ui" | "font" | "caption" | "fillColor" | "borderColor" | "fontColor" | "margin" | "para" | "assist" | "validate"
            | "format" | "items" | "bind" | "keep" | "h" | "w" | "x" | "y" | "minH" | "maxH" | "minW" | "maxW" | "layout" | "model"
            | "oneOfChild" | "locale" | "relevant" | "colSpan" | "dataNode" | "defaultValue" | "editValue" | "selectedIndex" | "event"
            | "calculate" | "traversal" | "extras" | "desc" => Val::Sink,
            other => {
                if let Some(child) = other.strip_prefix('_') {
                    return Ok(if child_named(self.h, i, child).is_some() { Val::Im(i, child.to_string()) } else { Val::Null });
                }
                let found = instances(self.h, i, other);
                match found.as_slice() {
                    [] => Val::Null,
                    [one] => Val::Node(*one),
                    many => Val::List(many.iter().map(|k| Val::Node(*k)).collect()),
                }
            }
        })
    }

    /// `base[n]` / `base[*]`: all the instances the last step named, then one or all.
    fn index(&mut self, base: &Expr, idx: &Idx) -> R<Val> {
        let all = self.all_of(base)?;
        match idx {
            Idx::All => Ok(Val::List(all.into_iter().map(Val::Node).collect())),
            Idx::Abs(e) => {
                let v = self.eval(e)?;
                let n = self.to_num(&v);
                if !n.is_finite() || n < 0.0 {
                    return Ok(Val::Null);
                }
                Ok(all.get(n as usize).map_or(Val::Null, |i| Val::Node(*i)))
            }
            Idx::Rel(k) => {
                // From the instance the script runs in (`row[-1]` in a row is the row above),
                // else from the first.
                let here = all.iter().position(|&i| self.encloses(i, self.h.current)).unwrap_or(0);
                let at = (here as i64).saturating_add(*k);
                Ok(usize::try_from(at).ok().and_then(|at| all.get(at)).map_or(Val::Null, |i| Val::Node(*i)))
            }
        }
    }

    /// Whether `node` is `inner` or one of its containers.
    fn encloses(&self, node: usize, inner: usize) -> bool {
        let mut at = Some(inner);
        let mut guard = 0;
        while let Some(i) = at
            && guard < MAX_PARSE_DEPTH
        {
            if i == node {
                return true;
            }
            at = self.h.nodes.get(i).and_then(|n| n.parent);
            guard += 1;
        }
        false
    }

    /// Every instance an accessor names (`Row` → all rows, not just the first), each once.
    fn all_of(&mut self, e: &Expr) -> R<Vec<usize>> {
        let mut out = Vec::new();
        let mut seen = HashSet::new();
        match e {
            Expr::Ident(name) => {
                if self.lookup(name).is_some() || name.starts_with('$') || name.eq_ignore_ascii_case("xfa") {
                    let v = self.eval(e)?;
                    return Ok(self.nodes_list(&v));
                }
                // The name resolves to its first instance; `row[*]` means every sibling of it.
                for first in resolve_som(self.h, self.h.current, name) {
                    match self.node(first).and_then(|n| n.parent.map(|p| (p, n.name.clone()))) {
                        Some((p, n)) => self.gather(&mut out, &mut seen, instances(self.h, p, &n))?,
                        None => self.gather(&mut out, &mut seen, [first])?,
                    }
                }
            }
            Expr::Member(base, name) => {
                let b = self.eval(base)?;
                for i in self.nodes_list(&b) {
                    self.gather(&mut out, &mut seen, instances(self.h, i, name))?;
                }
                if out.is_empty() {
                    let v = self.member(b, name)?;
                    out = self.nodes_list(&v);
                }
            }
            Expr::Descend(base, name) => {
                let b = self.eval(base)?;
                for i in self.nodes_list(&b) {
                    self.gather(&mut out, &mut seen, descendants_named(self.h, i, name, 0))?;
                }
            }
            other => {
                let v = self.eval(other)?;
                out = self.nodes_list(&v);
            }
        }
        Ok(out)
    }

    // ── expressions

    fn eval(&mut self, e: &Expr) -> R<Val> {
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

    fn binary(&self, op: &str, a: Val, b: Val) -> R<Val> {
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

    fn arg(&mut self, args: &[Expr], i: usize) -> R<Val> {
        match args.get(i) {
            Some(e) => {
                let v = self.eval(e)?;
                self.take(v)
            }
            None => Ok(Val::Null),
        }
    }

    fn arg_num(&mut self, args: &[Expr], i: usize) -> R<f64> {
        let v = self.arg(args, i)?;
        Ok(self.to_num(&v))
    }

    fn arg_str(&mut self, args: &[Expr], i: usize) -> R<String> {
        let v = self.arg(args, i)?;
        Ok(self.to_str(&v))
    }

    /// Every value the arguments hold, lists flattened, nulls left out.
    fn arg_values(&mut self, args: &[Expr]) -> R<Vec<Val>> {
        self.arg_values_keeping(args, false)
    }

    /// [`Self::arg_values`]; `objects` keeps form objects (fields, subforms) as themselves, so
    /// `Count(row[*])` counts rows rather than filled-in values.
    fn arg_values_keeping(&mut self, args: &[Expr], objects: bool) -> R<Vec<Val>> {
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
                    let names: Vec<String> = list.split(',').map(str::trim).filter(|s| !s.is_empty()).map(str::to_string).collect();
                    self.h.push(XfaEffect::ResetData(names));
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
                            if let Some(n) = self.h.nodes.get_mut(i) {
                                n.presence = p.clone();
                            }
                            self.h.push(XfaEffect::SetPresence { som, presence: p });
                        }
                    }
                    "access" => {
                        let a = self.to_str(&v);
                        if matches!(a.as_str(), "open" | "readOnly" | "protected" | "nonInteractive") {
                            if let Some(n) = self.h.nodes.get_mut(i) {
                                n.access = a.clone();
                            }
                            self.h.push(XfaEffect::SetAccess { som, access: a });
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
                            self.assign_to(Val::Node(k), "rawValue", v.clone())?;
                        }
                    }
                }
                Ok(())
            }
            Val::List(items) => {
                for it in items {
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
    fn block(&mut self, body: &[Stmt]) -> R<(Flow, Val)> {
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

// ── built-in functions ──────────────────────────────────────────────────────────────────────

/// Days from 1900-01-01 (day 1, as FormCalc counts) to a civil date.
fn days_from_civil(y: i64, m: u32, d: u32) -> i64 {
    // Howard Hinnant's algorithm, epoch 1970-01-01, shifted to FormCalc's 1900-01-01 = 1.
    let y = if m <= 2 { y - 1 } else { y };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let mp = (i64::from(m) + 9) % 12;
    let doy = (153 * mp + 2) / 5 + i64::from(d) - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let unix = era * 146_097 + doe - 719_468;
    unix + 25_568
}

fn civil_from_days(n: i64) -> (i64, u32, u32) {
    let z = n - 25_568 + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (if m <= 2 { y + 1 } else { y }, m, d)
}

/// Day numbers (and years) beyond this are refused rather than computed.
const MAX_DAYS: f64 = 1e8;

const MONTHS: [&str; 12] = ["January", "February", "March", "April", "May", "June", "July", "August", "September", "October", "November", "December"];

/// Parse `text` as a date by `picture` (`YYYY-MM-DD`, `MM/DD/YYYY`, `MMM D, YYYY`, …).
fn parse_date(text: &str, picture: &str) -> Option<(i64, u32, u32)> {
    let pic: Vec<char> = picture.chars().collect();
    let txt: Vec<char> = text.trim().chars().collect();
    let (mut y, mut m, mut d) = (None, None, None);
    let (mut i, mut j) = (0, 0);
    while i < pic.len() {
        let c = pic[i].to_ascii_uppercase();
        if c == 'Y' || c == 'M' || c == 'D' {
            let mut run = 1;
            while i + run < pic.len() && pic[i + run].to_ascii_uppercase() == c {
                run += 1;
            }
            i += run;
            if c == 'M' && run >= 3 {
                // Month name.
                let rest: String = txt.get(j..).unwrap_or(&[]).iter().collect();
                let lower = rest.to_ascii_lowercase();
                let found = MONTHS
                    .iter()
                    .position(|name| lower.starts_with(&name.to_ascii_lowercase()) || lower.starts_with(&name[..3].to_ascii_lowercase()));
                let k = found?;
                m = Some(k as u32 + 1);
                let full = MONTHS[k].to_ascii_lowercase();
                j += if lower.starts_with(&full) { full.chars().count() } else { 3 };
                continue;
            }
            let width = if run == 1 { 2 } else { run };
            let mut digits = String::new();
            while j < txt.len() && txt[j].is_ascii_digit() && digits.len() < width {
                digits.push(txt[j]);
                j += 1;
            }
            if digits.is_empty() {
                return None;
            }
            let v: i64 = digits.parse().ok()?;
            match c {
                'Y' => y = Some(if run <= 2 { if v < 50 { 2000 + v } else { 1900 + v } } else { v }),
                'M' => m = Some(v as u32),
                _ => d = Some(v as u32),
            }
        } else {
            if txt.get(j) != Some(&pic[i]) {
                return None;
            }
            i += 1;
            j += 1;
        }
    }
    let (y, m, d) = (y?, m.unwrap_or(1), d.unwrap_or(1));
    ((-200_000..=200_000).contains(&y) && (1..=12).contains(&m) && (1..=31).contains(&d)).then_some((y, m, d))
}

fn format_date(y: i64, m: u32, d: u32, picture: &str) -> String {
    let pic: Vec<char> = picture.chars().collect();
    let mut out = String::new();
    let mut i = 0;
    while i < pic.len() {
        let c = pic[i].to_ascii_uppercase();
        if c == 'Y' || c == 'M' || c == 'D' {
            let mut run = 1;
            while i + run < pic.len() && pic[i + run].to_ascii_uppercase() == c {
                run += 1;
            }
            i += run;
            match (c, run) {
                ('Y', 1 | 2) => out.push_str(&format!("{:02}", y.rem_euclid(100))),
                ('Y', _) => out.push_str(&format!("{y:04}")),
                ('M', 1) => out.push_str(&m.to_string()),
                ('M', 2) => out.push_str(&format!("{m:02}")),
                ('M', 3) => out.push_str(MONTHS.get((m as usize).saturating_sub(1)).map_or("", |n| &n[..3])),
                ('M', _) => out.push_str(MONTHS.get((m as usize).saturating_sub(1)).copied().unwrap_or("")),
                ('D', 1) => out.push_str(&d.to_string()),
                (_, _) => out.push_str(&format!("{d:02}")),
            }
        } else {
            out.push(pic[i]);
            i += 1;
        }
    }
    out
}

const ONES: [&str; 20] = [
    "Zero",
    "One",
    "Two",
    "Three",
    "Four",
    "Five",
    "Six",
    "Seven",
    "Eight",
    "Nine",
    "Ten",
    "Eleven",
    "Twelve",
    "Thirteen",
    "Fourteen",
    "Fifteen",
    "Sixteen",
    "Seventeen",
    "Eighteen",
    "Nineteen",
];
const TENS: [&str; 10] = ["", "", "Twenty", "Thirty", "Forty", "Fifty", "Sixty", "Seventy", "Eighty", "Ninety"];

fn words_below_1000(n: u64) -> String {
    let mut parts = Vec::new();
    if n >= 100 {
        parts.push(format!("{} Hundred", ONES[(n / 100) as usize]));
    }
    let r = n % 100;
    if r >= 20 {
        parts.push(if r.is_multiple_of(10) {
            TENS[(r / 10) as usize].to_string()
        } else {
            format!("{}-{}", TENS[(r / 10) as usize], ONES[(r % 10) as usize])
        });
    } else if r > 0 || n == 0 {
        parts.push(ONES[r as usize].to_string());
    }
    parts.join(" ")
}

/// `WordNum`: an integer in English words (up to the billions).
fn word_num(n: f64, cents: bool) -> String {
    if !n.is_finite() || !(0.0..1e12).contains(&n) {
        return String::new();
    }
    let cents_total = (n * 100.0).round() as u64;
    let whole = if cents { cents_total / 100 } else { n.trunc() as u64 };
    let mut parts = Vec::new();
    let groups = [(1_000_000_000u64, "Billion"), (1_000_000, "Million"), (1_000, "Thousand")];
    let mut rest = whole;
    for (unit, name) in groups {
        if rest >= unit {
            parts.push(format!("{} {name}", words_below_1000(rest / unit)));
            rest %= unit;
        }
    }
    if rest > 0 || parts.is_empty() {
        parts.push(words_below_1000(rest));
    }
    let mut out = parts.join(" ");
    if cents {
        let c = cents_total % 100;
        let dollars = if whole == 1 { "Dollar" } else { "Dollars" };
        let cent = if c == 1 { "Cent" } else { "Cents" };
        out.push_str(&format!(" {dollars} And {c:02} {cent}"));
    }
    out
}

impl Interp<'_> {
    /// A built-in function: its result is never a string past [`MAX_STRING`], and work on
    /// long strings is charged to the step budget.
    fn builtin(&mut self, name: &str, args: &[Expr]) -> R<Val> {
        let v = self.builtin_inner(name, args)?;
        if let Val::Str(s) = &v {
            if s.len() > MAX_STRING {
                return Err(STRING_TOO_LONG.into());
            }
            self.charge((s.len() / 256) as u64)?;
        }
        Ok(v)
    }

    fn builtin_inner(&mut self, name: &str, args: &[Expr]) -> R<Val> {
        let lower = name.to_ascii_lowercase();
        let n = |v: f64| Ok(Val::Num(v));
        Ok(match lower.as_str() {
            // ── arithmetic
            "abs" => return n(self.arg_num(args, 0)?.abs()),
            "ceil" => return n(self.arg_num(args, 0)?.ceil()),
            "floor" => return n(self.arg_num(args, 0)?.floor()),
            "mod" => {
                let (a, b) = (self.arg_num(args, 0)?, self.arg_num(args, 1)?);
                if b == 0.0 {
                    return Err("division by zero".into());
                }
                Val::Num(a - b * (a / b).trunc())
            }
            "round" => {
                let a = self.arg_num(args, 0)?;
                let places = if args.len() > 1 { self.arg_num(args, 1)?.clamp(0.0, 12.0) as i32 } else { 0 };
                let k = 10f64.powi(places);
                Val::Num((a * k).round() / k)
            }
            "count" => {
                // Non-null values and objects: an empty field doesn't count, a subform does.
                let vals = self.arg_values_keeping(args, true)?;
                let counts = |v: &Val| match v {
                    Val::Node(i) => self.node(*i).is_some_and(|n| !matches!(n.kind, XfaKind::Field | XfaKind::ExclGroup) || !n.value.is_empty()),
                    other => !self.is_null(other),
                };
                Val::Num(vals.iter().filter(|v| counts(v)).count() as f64)
            }
            "sum" | "avg" | "max" | "min" => {
                let vals = self.arg_values(args)?;
                let nums: Vec<f64> = vals.iter().map(|v| self.to_num(v)).collect();
                match lower.as_str() {
                    "sum" => Val::Num(nums.iter().sum()),
                    "avg" => {
                        if nums.is_empty() {
                            Val::Null
                        } else {
                            Val::Num(nums.iter().sum::<f64>() / nums.len() as f64)
                        }
                    }
                    "max" => nums.iter().copied().fold(None, |m: Option<f64>, x| Some(m.map_or(x, |m| m.max(x)))).map_or(Val::Null, Val::Num),
                    _ => nums.iter().copied().fold(None, |m: Option<f64>, x| Some(m.map_or(x, |m| m.min(x)))).map_or(Val::Null, Val::Num),
                }
            }
            // ── logical
            "choose" => {
                let i = self.arg_num(args, 0)?;
                if !i.is_finite() || i < 1.0 {
                    return Ok(Val::Null);
                }
                self.arg(args, i as usize)?
            }
            "exists" => {
                let Some(e) = args.first() else { return Ok(Val::Num(0.0)) };
                // A name that resolves to nothing is the answer "no"; a limit the lookup ran
                // into (time, steps, depth, size) still stops the script.
                match self.all_of(e) {
                    Ok(v) => Val::Num(if v.is_empty() { 0.0 } else { 1.0 }),
                    Err(e) if is_limit(&e) => return Err(e),
                    Err(_) => Val::Num(0.0),
                }
            }
            "hasvalue" => {
                let v = self.arg(args, 0)?;
                Val::Num(if self.is_null(&v) || matches!(&v, Val::Str(s) if s.trim().is_empty()) { 0.0 } else { 1.0 })
            }
            "oneof" => {
                let v = self.arg(args, 0)?;
                let rest = self.arg_values(args.get(1..).unwrap_or_default())?;
                let mut hit = false;
                for x in &rest {
                    // Each comparison copies and compares `v`: charged per item.
                    self.charge(self.cost(&v).saturating_add(self.cost(x)).saturating_add(1))?;
                    if self.binary("==", v.clone(), x.clone()).is_ok_and(|r| self.truthy(&r)) {
                        hit = true;
                        break;
                    }
                }
                Val::Num(if hit { 1.0 } else { 0.0 })
            }
            "within" => {
                let v = self.arg(args, 0)?;
                let (lo, hi) = (self.arg(args, 1)?, self.arg(args, 2)?);
                let ok = self.binary(">=", v.clone(), lo).is_ok_and(|r| self.truthy(&r)) && self.binary("<=", v, hi).is_ok_and(|r| self.truthy(&r));
                Val::Num(if ok { 1.0 } else { 0.0 })
            }
            // ── strings
            "at" => {
                let (s, sub) = (self.arg_str(args, 0)?, self.arg_str(args, 1)?);
                Val::Num(if sub.is_empty() { 0.0 } else { s.find(&sub).map_or(0.0, |b| s[..b].chars().count() as f64 + 1.0) })
            }
            "concat" => {
                let mut out = String::new();
                for i in 0..args.len() {
                    out.push_str(&self.arg_str(args, i)?);
                    if out.len() > MAX_STRING {
                        return Err(STRING_TOO_LONG.into());
                    }
                }
                Val::Str(out)
            }
            "left" => {
                let (s, k) = (self.arg_str(args, 0)?, self.arg_num(args, 1)?.max(0.0) as usize);
                Val::Str(s.chars().take(k).collect())
            }
            "right" => {
                let (s, k) = (self.arg_str(args, 0)?, self.arg_num(args, 1)?.max(0.0) as usize);
                let len = s.chars().count();
                Val::Str(s.chars().skip(len.saturating_sub(k)).collect())
            }
            "len" => Val::Num(self.arg_str(args, 0)?.chars().count() as f64),
            "lower" => Val::Str(self.arg_str(args, 0)?.to_lowercase()),
            "upper" => Val::Str(self.arg_str(args, 0)?.to_uppercase()),
            "ltrim" => Val::Str(self.arg_str(args, 0)?.trim_start().to_string()),
            "rtrim" => Val::Str(self.arg_str(args, 0)?.trim_end().to_string()),
            "space" => {
                let k = self.arg_num(args, 0)?.clamp(0.0, MAX_STRING as f64) as usize;
                Val::Str(" ".repeat(k))
            }
            "str" => {
                let v = self.arg_num(args, 0)?;
                let width = if args.len() > 1 { self.arg_num(args, 1)?.clamp(0.0, 1000.0) as usize } else { 10 };
                let prec = if args.len() > 2 { self.arg_num(args, 2)?.clamp(0.0, 20.0) as usize } else { 0 };
                let text = format!("{v:.prec$}");
                Val::Str(if text.len() > width { "*".repeat(width) } else { format!("{text:>width$}") })
            }
            "substr" => {
                let s = self.arg_str(args, 0)?;
                let start = self.arg_num(args, 1)?.max(1.0) as usize;
                let len = self.arg_num(args, 2)?.max(0.0) as usize;
                Val::Str(s.chars().skip(start - 1).take(len).collect())
            }
            "stuff" => {
                let s = self.arg_str(args, 0)?;
                let start = self.arg_num(args, 1)?.max(1.0) as usize;
                let del = self.arg_num(args, 2)?.max(0.0) as usize;
                let ins = if args.len() > 3 { self.arg_str(args, 3)? } else { String::new() };
                let chars: Vec<char> = s.chars().collect();
                let a = start.saturating_sub(1).min(chars.len());
                let b = a.saturating_add(del).min(chars.len());
                let out: String = chars[..a].iter().chain(ins.chars().collect::<Vec<_>>().iter()).chain(chars[b..].iter()).collect();
                if out.len() > MAX_STRING {
                    return Err(STRING_TOO_LONG.into());
                }
                Val::Str(out)
            }
            "replace" => {
                let (s, old) = (self.arg_str(args, 0)?, self.arg_str(args, 1)?);
                let new = if args.len() > 2 { self.arg_str(args, 2)? } else { String::new() };
                if old.is_empty() {
                    return Ok(Val::Str(s));
                }
                let grown = s.matches(old.as_str()).count().saturating_mul(new.len()).saturating_add(s.len());
                if grown > MAX_STRING {
                    return Err(STRING_TOO_LONG.into());
                }
                Val::Str(s.replace(&old, &new))
            }
            "wordnum" => {
                let v = self.arg_num(args, 0)?;
                let kind = if args.len() > 1 { self.arg_num(args, 1)? } else { 0.0 };
                Val::Str(word_num(v, kind == 2.0))
            }
            "uuid" => Val::Str(format!("{:032x}", (self.steps as u128).wrapping_mul(0x9E37_79B9_7F4A_7C15_F39C_C060_5CED_C835))),
            "encode" | "decode" => {
                let s = self.arg_str(args, 0)?;
                let kind = if args.len() > 1 { self.arg_str(args, 1)?.to_ascii_lowercase() } else { "url".into() };
                Val::Str(match (lower.as_str(), kind.as_str()) {
                    ("encode", "html" | "xml") => s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;"),
                    ("decode", "html" | "xml") => s.replace("&lt;", "<").replace("&gt;", ">").replace("&quot;", "\"").replace("&amp;", "&"),
                    ("encode", _) => s
                        .bytes()
                        .map(|b| if b.is_ascii_alphanumeric() || b"-_.~".contains(&b) { (b as char).to_string() } else { format!("%{b:02X}") })
                        .collect(),
                    (_, _) => {
                        let mut out = Vec::new();
                        let b = s.as_bytes();
                        let mut i = 0;
                        while i < b.len() {
                            if b[i] == b'%'
                                && let Some(h) =
                                    b.get(i + 1..i + 3).and_then(|h| std::str::from_utf8(h).ok()).and_then(|h| u8::from_str_radix(h, 16).ok())
                            {
                                out.push(h);
                                i += 3;
                            } else {
                                out.push(b[i]);
                                i += 1;
                            }
                        }
                        String::from_utf8_lossy(&out).into_owned()
                    }
                })
            }
            "format" => {
                // Number pictures: `9` digit, `z` optional digit, `,` separator, `.` point, `$`.
                let pic = self.arg_str(args, 0)?;
                let v = self.arg(args, 1)?;
                if pic.to_ascii_uppercase().contains("YY") || pic.to_ascii_uppercase().contains("DD") {
                    let text = self.to_str(&v);
                    let days = parse_num(&text).filter(|d| is_numeric_text(&text) && d.abs() < MAX_DAYS).map(|d| d as i64);
                    return Ok(match days {
                        Some(d) => {
                            let (y, m, dd) = civil_from_days(d);
                            Val::Str(format_date(y, m, dd, &pic))
                        }
                        None => parse_date(&text, "YYYY-MM-DD").map_or(Val::Str(text), |(y, m, d)| Val::Str(format_date(y, m, d, &pic))),
                    });
                }
                let num = self.to_num(&v);
                let decimals = pic.split_once('.').map_or(0, |(_, f)| f.chars().filter(|c| matches!(c, '9' | 'z' | 'Z')).count()).min(20);
                let grouped = pic.contains(',');
                let mut text = format!("{:.decimals$}", num.abs());
                if grouped {
                    let (int, frac) = text.split_once('.').map_or((text.as_str(), ""), |(a, b)| (a, b));
                    let mut g = String::new();
                    for (k, c) in int.chars().enumerate() {
                        if k > 0 && (int.len() - k) % 3 == 0 {
                            g.push(',');
                        }
                        g.push(c);
                    }
                    text = if frac.is_empty() { g } else { format!("{g}.{frac}") };
                }
                if pic.contains('$') {
                    text = format!("${text}");
                }
                if num < 0.0 {
                    text = format!("-{text}");
                }
                Val::Str(text)
            }
            "parse" => {
                let pic = self.arg_str(args, 0)?;
                let s = self.arg_str(args, 1)?;
                if pic.to_ascii_uppercase().contains("YY") {
                    // The canonical form of a date is ISO.
                    return Ok(parse_date(&s, &pic).map_or(Val::Null, |(y, m, d)| Val::Str(format_date(y, m, d, "YYYY-MM-DD"))));
                }
                let cleaned: String = s.chars().filter(|c| c.is_ascii_digit() || *c == '.' || *c == '-').collect();
                parse_num(&cleaned).map_or(Val::Null, Val::Num)
            }
            // ── dates and times (days since 1900-01-01 = 1; milliseconds since midnight)
            "date" => {
                #[cfg(not(target_arch = "wasm32"))]
                {
                    let secs = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0);
                    Val::Num((secs.div_euclid(86_400) + 25_568) as f64)
                }
                #[cfg(target_arch = "wasm32")]
                {
                    Val::Num(25_568.0)
                }
            }
            "time" => {
                #[cfg(not(target_arch = "wasm32"))]
                {
                    let ms = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis() as i64).unwrap_or(0);
                    Val::Num(ms.rem_euclid(86_400_000) as f64)
                }
                #[cfg(target_arch = "wasm32")]
                {
                    Val::Num(0.0)
                }
            }
            "date2num" | "isodate2num" => {
                let s = self.arg_str(args, 0)?;
                let pic = if lower == "isodate2num" || args.len() < 2 { "YYYY-MM-DD".to_string() } else { self.arg_str(args, 1)? };
                let parsed = parse_date(&s, &pic).or_else(|| parse_date(&s, "YYYYMMDD")).or_else(|| parse_date(&s, "YYYY-MM-DD"));
                parsed.map_or(Val::Num(0.0), |(y, m, d)| Val::Num(days_from_civil(y, m, d) as f64))
            }
            "num2date" => {
                let days = self.arg_num(args, 0)?;
                let pic = if args.len() > 1 { self.arg_str(args, 1)? } else { "MMM D, YYYY".into() };
                if !days.is_finite() || days.abs() >= MAX_DAYS {
                    return Ok(Val::Null);
                }
                let (y, m, d) = civil_from_days(days as i64);
                Val::Str(format_date(y, m, d, &pic))
            }
            "datefmt" | "localdatefmt" => {
                let style = if args.is_empty() { 0.0 } else { self.arg_num(args, 0)? };
                Val::Str(
                    match style as i32 {
                        1 => "M/D/YY",
                        2 => "MMM D, YYYY",
                        3 => "MMMM D, YYYY",
                        4 => "EEEE, MMMM D, YYYY",
                        _ => "MMM D, YYYY",
                    }
                    .into(),
                )
            }
            "timefmt" | "localtimefmt" => Val::Str("HH:MM:SS".into()),
            "num2time" | "num2gmtime" => {
                let ms = self.arg_num(args, 0)?;
                if !ms.is_finite() || ms < 0.0 {
                    return Ok(Val::Null);
                }
                let s = (ms / 1000.0) as u64;
                Val::Str(format!("{:02}:{:02}:{:02}", (s / 3600) % 24, (s / 60) % 60, s % 60))
            }
            "time2num" | "isotime2num" => {
                let s = self.arg_str(args, 0)?;
                let parts: Vec<u64> = s.split(':').filter_map(|p| p.trim().parse::<u64>().ok()).map(|v| v.min(1_000_000)).collect();
                let (h, m, sec) = (parts.first().copied().unwrap_or(0), parts.get(1).copied().unwrap_or(0), parts.get(2).copied().unwrap_or(0));
                Val::Num(((h * 3600 + m * 60 + sec) * 1000) as f64)
            }
            // ── financial
            "fv" => {
                let (p, r, k) = (self.arg_num(args, 0)?, self.arg_num(args, 1)?, self.arg_num(args, 2)?);
                Val::Num(if r == 0.0 { p * k } else { p * (((1.0 + r).powf(k) - 1.0) / r) })
            }
            "pv" => {
                let (p, r, k) = (self.arg_num(args, 0)?, self.arg_num(args, 1)?, self.arg_num(args, 2)?);
                Val::Num(if r == 0.0 { p * k } else { p * ((1.0 - (1.0 + r).powf(-k)) / r) })
            }
            "pmt" => {
                let (pv, r, k) = (self.arg_num(args, 0)?, self.arg_num(args, 1)?, self.arg_num(args, 2)?);
                if k <= 0.0 {
                    return Err("Pmt needs a positive number of periods".into());
                }
                Val::Num(if r == 0.0 { pv / k } else { pv * r / (1.0 - (1.0 + r).powf(-k)) })
            }
            "term" => {
                let (p, r, fv) = (self.arg_num(args, 0)?, self.arg_num(args, 1)?, self.arg_num(args, 2)?);
                if p <= 0.0 || r <= 0.0 || fv <= 0.0 {
                    return Err("Term needs positive arguments".into());
                }
                Val::Num(((fv * r / p) + 1.0).ln() / (1.0 + r).ln())
            }
            "cterm" => {
                let (r, fv, pv) = (self.arg_num(args, 0)?, self.arg_num(args, 1)?, self.arg_num(args, 2)?);
                if r <= 0.0 || fv <= 0.0 || pv <= 0.0 {
                    return Err("CTerm needs positive arguments".into());
                }
                Val::Num((fv / pv).ln() / (1.0 + r).ln())
            }
            "rate" => {
                let (fv, pv, k) = (self.arg_num(args, 0)?, self.arg_num(args, 1)?, self.arg_num(args, 2)?);
                if fv <= 0.0 || pv <= 0.0 || k <= 0.0 {
                    return Err("Rate needs positive arguments".into());
                }
                Val::Num((fv / pv).powf(1.0 / k) - 1.0)
            }
            "npv" => {
                let r = self.arg_num(args, 0)?;
                let flows = self.arg_values(args.get(1..).unwrap_or_default())?;
                Val::Num(flows.iter().enumerate().map(|(i, v)| self.to_num(v) / (1.0 + r).powi(i as i32 + 1)).sum())
            }
            "ipmt" | "ppmt" => {
                let (pv, r, pmt, first, count) = (
                    self.arg_num(args, 0)?,
                    self.arg_num(args, 1)?,
                    self.arg_num(args, 2)?,
                    self.arg_num(args, 3)?.max(1.0),
                    self.arg_num(args, 4)?.max(1.0),
                );
                let (mut balance, mut interest, mut principal) = (pv, 0.0, 0.0);
                for period in 1..=((first + count - 1.0).min(10_000.0) as u32) {
                    self.step()?;
                    let i = balance * r;
                    let p = (pmt - i).min(balance);
                    if period >= first as u32 {
                        interest += i;
                        principal += p;
                    }
                    balance -= p;
                    if balance <= 0.0 {
                        break;
                    }
                }
                Val::Num(if lower == "ipmt" { interest } else { principal })
            }
            "apr" => {
                let (principal, payment, periods) = (self.arg_num(args, 0)?, self.arg_num(args, 1)?, self.arg_num(args, 2)?);
                if principal <= 0.0 || payment <= 0.0 || periods <= 0.0 {
                    return Err("Apr needs positive arguments".into());
                }
                // Bisection on the monthly rate.
                let (mut lo, mut hi) = (0.0f64, 1.0f64);
                for _ in 0..200 {
                    let mid = (lo + hi) / 2.0;
                    let pmt = if mid == 0.0 { principal / periods } else { principal * mid / (1.0 - (1.0 + mid).powf(-periods)) };
                    if pmt > payment {
                        hi = mid;
                    } else {
                        lo = mid;
                    }
                }
                Val::Num((lo + hi) / 2.0 * 12.0)
            }
            // ── units and references
            "unittype" => {
                let s = self.arg_str(args, 0)?;
                let unit: String = s.trim().chars().skip_while(|c| c.is_ascii_digit() || *c == '.' || *c == '-' || *c == '+').collect();
                Val::Str(
                    match unit.trim().to_ascii_lowercase().as_str() {
                        "in" | "inch" | "inches" => "in",
                        "mm" | "millimeter" | "millimeters" => "mm",
                        "cm" | "centimeter" | "centimeters" => "cm",
                        "pt" | "point" | "points" => "pt",
                        "pc" | "pica" => "pc",
                        _ => "in",
                    }
                    .into(),
                )
            }
            "unitvalue" => {
                let s = self.arg_str(args, 0)?;
                let to = if args.len() > 1 { self.arg_str(args, 1)? } else { String::new() };
                let v = parse_num(&s).unwrap_or(0.0);
                let unit: String = s.trim().chars().skip_while(|c| c.is_ascii_digit() || *c == '.' || *c == '-' || *c == '+').collect();
                let per_in = |u: &str| match u.trim().to_ascii_lowercase().as_str() {
                    "mm" => 25.4,
                    "cm" => 2.54,
                    "pt" => 72.0,
                    "pc" => 6.0,
                    _ => 1.0,
                };
                let target = if to.trim().is_empty() { unit.clone() } else { to };
                Val::Num(v / per_in(&unit) * per_in(&target))
            }
            "ref" => {
                let Some(e) = args.first() else { return Ok(Val::Null) };
                self.eval(e)?
            }
            "eval" => {
                let src = self.arg_str(args, 0)?;
                self.charge((src.len() / 16) as u64)?;
                let body = parse(&src)?;
                let (_, v) = self.block(&body)?;
                v
            }
            "get" | "post" | "put" => return Err(format!("{name}: network access is not available to form scripts")),
            _ => return Err(format!("unknown function {name}")),
        })
    }
}

// ── entry points ────────────────────────────────────────────────────────────────────────────

/// Run a FormCalc `script` for `event` on the form `root`, as [`crate::xfa::run_xfa`] runs a
/// JavaScript one.
pub fn run_formcalc(script: &str, event: &XfaEvent, doc: &XfaDoc, root: &XfaNode, limits: Limits) -> XfaOutcome {
    run_formcalc_at(script, event, doc, root.clone(), limits, None)
}

/// [`run_formcalc`] on an owned form, giving up at `deadline` (the clock is read every
/// [`CLOCK_EVERY`] steps; unused on wasm, which has no clock).
pub(crate) fn run_formcalc_at(
    script: &str,
    event: &XfaEvent,
    doc: &XfaDoc,
    root: XfaNode,
    limits: Limits,
    deadline: Option<std::time::Duration>,
) -> XfaOutcome {
    // FormCalc has its own nesting limits (the shared `refuse` reads JavaScript syntax).
    if script.len() > crate::MAX_SCRIPT_BYTES {
        return XfaOutcome {
            error: Some(format!("the script is too long ({} KiB; the limit is {} KiB)", script.len() / 1024, crate::MAX_SCRIPT_BYTES / 1024)),
            ..Default::default()
        };
    }
    let body = match parse(script) {
        Ok(b) => b,
        Err(e) => return XfaOutcome { error: Some(e), ..Default::default() },
    };
    let mut host = XHost::new(root, &event.target, doc);
    #[cfg(target_arch = "wasm32")]
    let _ = deadline;
    let mut interp = Interp {
        h: &mut host,
        scopes: vec![HashMap::new()],
        funcs: HashMap::new(),
        event: event.clone(),
        steps: 0,
        loops_left: limits.loop_iterations,
        depth: 0,
        // The JavaScript recursion limit, kept to 8..=128: statement nesting counts against it
        // too, and the interpreter's frames are large.
        max_depth: limits.recursion.clamp(8, 128),
        exited: false,
        #[cfg(not(target_arch = "wasm32"))]
        deadline: deadline.and_then(|d| std::time::Instant::now().checked_add(d)),
    };
    let mut out = XfaOutcome::default();
    match interp.block(&body) {
        Ok((_, last)) => {
            // As the JavaScript path reports: null is an empty result (a calculate clears the
            // field) with no truth value.
            let last = interp.deref(last);
            out.result = Some(interp.to_str(&last));
            if !matches!(last, Val::Null) {
                out.result_bool = Some(interp.truthy(&last));
            }
        }
        Err(e) => out.error = Some(e),
    }
    out.notes = host.notes();
    out.effects = host.effects;
    out.console = host.console;
    clip_result(&mut out);
    out
}

/// [`run_formcalc`] on its own thread, abandoned after `timeout` (and told to stop itself then).
pub fn run_formcalc_within(script: &str, event: &XfaEvent, doc: &XfaDoc, root: XfaNode, limits: Limits, timeout: std::time::Duration) -> XfaOutcome {
    let (script, event, doc) = (script.to_string(), event.clone(), doc.clone());
    run_within("pdfcraft-formcalc", timeout, move || run_formcalc_at(&script, &event, &doc, root, limits, Some(timeout)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dates_round_trip_through_formcalc_day_numbers() {
        assert_eq!(days_from_civil(1900, 1, 1), 1);
        assert_eq!(days_from_civil(1970, 1, 1), 25_568);
        assert_eq!(civil_from_days(days_from_civil(2024, 2, 29)), (2024, 2, 29));
        assert_eq!(parse_date("2001-02-03", "YYYY-MM-DD"), Some((2001, 2, 3)));
        assert_eq!(parse_date("Feb 3, 2001", "MMM D, YYYY"), Some((2001, 2, 3)));
        assert_eq!(parse_date("2/3/01", "M/D/YY"), Some((2001, 2, 3)));
        assert_eq!(format_date(2001, 2, 3, "MMMM D, YYYY"), "February 3, 2001");
        assert_eq!(word_num(1234.5, true), "One Thousand Two Hundred Thirty-Four Dollars And 50 Cents");
        assert_eq!(num_text(3.0), "3");
        assert_eq!(num_text(2.5), "2.5");
        assert_eq!(num_text(0.1 + 0.2), "0.3");
        assert_eq!(parse_num("12abc"), Some(12.0));
        assert_eq!(parse_num("abc"), None);
    }
}
