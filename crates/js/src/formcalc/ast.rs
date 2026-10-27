use super::*;

// ── syntax ──────────────────────────────────────────────────────────────────────────────────

#[derive(Clone, Debug, PartialEq)]
pub(super) enum Expr {
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
pub(super) enum Idx {
    /// `[*]`: every instance.
    All,
    /// `[n]`: the n-th instance (from 0).
    Abs(Box<Expr>),
    /// `[-n]` / `[+n]`: n instances before or after the one the script runs in.
    Rel(i64),
}

#[derive(Clone, Debug, PartialEq)]
pub(super) enum Stmt {
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
