//! Formula syntax tree.

use std::sync::Arc;

use gridcraft_core::{CellError, CellRef, MAX_COLS, MAX_ROWS, RangeRef};
use serde::{Deserialize, Serialize};

/// One end of a reference, with `$` anchors.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Anchor {
    pub row: u32,
    pub col: u32,
    pub row_abs: bool,
    pub col_abs: bool,
}

impl Anchor {
    pub fn cell(&self) -> CellRef {
        CellRef::new(self.row, self.col)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum RefKind {
    Cell(Anchor),
    Range(Anchor, Anchor),
    /// Whole rows `3:5` (row, abs) pairs.
    Rows(u32, bool, u32, bool),
    /// Whole columns `A:C`.
    Cols(u32, bool, u32, bool),
}

#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum SheetSel {
    /// Unqualified: the formula's own sheet.
    Current,
    Named(String),
    /// 3-D span `Sheet1:Sheet3`.
    Span(String, String),
}

#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Reference {
    pub sheet: SheetSel,
    pub kind: RefKind,
}

impl Reference {
    pub fn range(&self) -> RangeRef {
        match &self.kind {
            RefKind::Cell(a) => RangeRef::cell(a.cell()),
            RefKind::Range(a, b) => RangeRef::new(a.cell(), b.cell()),
            RefKind::Rows(r0, _, r1, _) => RangeRef::rows((*r0).min(*r1), (*r0).max(*r1)),
            RefKind::Cols(c0, _, c1, _) => RangeRef::cols((*c0).min(*c1), (*c0).max(*c1)),
        }
    }
    pub fn sheet_name(&self) -> Option<&str> {
        match &self.sheet {
            SheetSel::Named(s) => Some(s),
            _ => None,
        }
    }
}

/// Structured reference `Table1[[#Headers],[Col1]:[Col3]]`, `[@Col]`.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct StructRef {
    /// Empty for `[@Col]` inside the table itself.
    pub table: String,
    pub specifiers: Vec<StructItem>,
    pub col_start: Option<String>,
    pub col_end: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum StructItem {
    All,
    Data,
    Headers,
    Totals,
    ThisRow,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum UnOp {
    Neg,
    Plus,
    Percent,
    /// Implicit intersection `@`.
    At,
    /// Spill range operator `A1#`: the whole spill range of the dynamic array anchored at the
    /// cell (`ANCHORARRAY` in files).
    Spill,
    /// Trim reference `A1.:E10` / `A1:.E10` / `A1.:.E10` around a range: the range without its
    /// blank leading, trailing or outer rows and columns (`TRIMRANGE` with both trim codes set).
    Trim(TrimMode),
}

/// Which blank edges a trim reference drops.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum TrimMode {
    /// `.:` — leading blank rows and columns.
    Leading,
    /// `:.` — trailing blank rows and columns.
    Trailing,
    /// `.:.` — both.
    All,
}

impl TrimMode {
    pub fn symbol(self) -> &'static str {
        match self {
            TrimMode::Leading => ".:",
            TrimMode::Trailing => ":.",
            TrimMode::All => ".:.",
        }
    }
    /// The equivalent `TRIMRANGE` trim_rows / trim_cols code.
    pub fn code(self) -> u8 {
        match self {
            TrimMode::Leading => 1,
            TrimMode::Trailing => 2,
            TrimMode::All => 3,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum BinOp {
    Add,
    Sub,
    Mul,
    Div,
    Pow,
    Concat,
    Eq,
    Ne,
    Lt,
    Gt,
    Le,
    Ge,
    /// `:` between two references.
    Range,
    /// `,` inside parentheses.
    Union,
    /// space between two references.
    Intersect,
}

impl BinOp {
    pub fn symbol(&self) -> &'static str {
        match self {
            BinOp::Add => "+",
            BinOp::Sub => "-",
            BinOp::Mul => "*",
            BinOp::Div => "/",
            BinOp::Pow => "^",
            BinOp::Concat => "&",
            BinOp::Eq => "=",
            BinOp::Ne => "<>",
            BinOp::Lt => "<",
            BinOp::Gt => ">",
            BinOp::Le => "<=",
            BinOp::Ge => ">=",
            BinOp::Range => ":",
            BinOp::Union => ",",
            BinOp::Intersect => " ",
        }
    }
    /// Binding power (higher binds tighter).
    pub fn precedence(&self) -> u8 {
        match self {
            BinOp::Range => 90,
            BinOp::Intersect => 85,
            BinOp::Union => 80,
            BinOp::Pow => 50,
            BinOp::Mul | BinOp::Div => 40,
            BinOp::Add | BinOp::Sub => 30,
            BinOp::Concat => 20,
            BinOp::Eq | BinOp::Ne | BinOp::Lt | BinOp::Gt | BinOp::Le | BinOp::Ge => 10,
        }
    }
}

/// A formula's syntax tree.
///
/// A flat operator chain such as `=A1+A2+…` nests one [`Expr::Binary`] per operator on its
/// left, so a formula as long as Excel allows (8,192 characters) is about 4,000 levels deep.
/// `Clone`, `PartialEq`, [`Expr::walk`] and [`Expr::map`] therefore follow that left spine in a
/// loop and recurse only into right-hand operands and real nesting, which the parser bounds.
#[derive(Debug, Serialize, Deserialize)]
pub enum Expr {
    Number(f64),
    Text(Arc<str>),
    Bool(bool),
    Error(CellError),
    /// Array constant `{1,2;3,4}` (rows of literal expressions).
    Array(Vec<Vec<Expr>>),
    Ref(Reference),
    /// Defined name, table name or LET/LAMBDA parameter.
    Name(String),
    Struct(StructRef),
    Unary(UnOp, Box<Expr>),
    Binary(BinOp, Box<Expr>, Box<Expr>),
    /// Function call; the name is upper-case without `_xlfn.`.
    Call(String, Vec<Expr>),
    /// Calling the result of an expression, e.g. `LAMBDA(x,x+1)(2)`.
    Invoke(Box<Expr>, Vec<Expr>),
    /// An omitted argument `F(1,,3)`.
    Missing,
    Paren(Box<Expr>),
}

impl Expr {
    /// Visits every node depth-first, parents before children, left to right.
    pub fn walk<'a>(&'a self, f: &mut dyn FnMut(&'a Expr)) {
        // An explicit stack: a long operator chain must not recurse once per operand.
        // Room for a typical formula up front, so walking one costs one allocation, not several.
        let mut pending = Vec::with_capacity(8);
        pending.push(self);
        while let Some(e) = pending.pop() {
            f(e);
            match e {
                Expr::Array(rows) => pending.extend(rows.iter().flatten().rev()),
                Expr::Unary(_, e) | Expr::Paren(e) => pending.push(e),
                Expr::Binary(_, a, b) => {
                    pending.push(b);
                    pending.push(a);
                }
                Expr::Call(_, args) => pending.extend(args.iter().rev()),
                Expr::Invoke(c, args) => {
                    pending.extend(args.iter().rev());
                    pending.push(c);
                }
                _ => {}
            }
        }
    }
    /// Rewrites every node bottom-up, children left to right before their parent.
    pub fn map(self, f: &mut dyn FnMut(Expr) -> Expr) -> Expr {
        // One operator on a non-chain operand (`A1*2`), the common case, needs no spine: rewrite
        // the operands in their boxes.
        if let Expr::Binary(op, mut a, mut b) = self {
            if !matches!(*a, Expr::Binary(..)) {
                *a = std::mem::replace(&mut *a, Expr::Missing).map(f);
                *b = std::mem::replace(&mut *b, Expr::Missing).map(f);
                return f(Expr::Binary(op, a, b));
            }
            return Expr::Binary(op, a, b).map_chain(f);
        }
        self.map_chain(f)
    }
    fn map_chain(self, f: &mut dyn FnMut(Expr) -> Expr) -> Expr {
        // Unwind the left spine of a long operator chain into a list, then rebuild it from the
        // innermost operand outwards, in the order a recursive rewrite would visit the nodes.
        let mut spine = Vec::new();
        let mut node = self;
        while let Expr::Binary(op, a, b) = node {
            spine.push((op, b));
            node = *a;
        }
        let e = match node {
            Expr::Array(rows) => Expr::Array(rows.into_iter().map(|r| r.into_iter().map(|e| e.map(f)).collect()).collect()),
            Expr::Unary(op, e) => Expr::Unary(op, Box::new(e.map(f))),
            Expr::Paren(e) => Expr::Paren(Box::new(e.map(f))),
            Expr::Call(n, args) => Expr::Call(n, args.into_iter().map(|e| e.map(f)).collect()),
            Expr::Invoke(c, args) => Expr::Invoke(Box::new(c.map(f)), args.into_iter().map(|e| e.map(f)).collect()),
            other => other,
        };
        let mut acc = f(e);
        while let Some((op, b)) = spine.pop() {
            let b = b.map(f);
            acc = f(Expr::Binary(op, Box::new(acc), Box::new(b)));
        }
        acc
    }
    /// All references in the formula.
    pub fn references(&self) -> Vec<&Reference> {
        let mut v = Vec::new();
        self.walk(&mut |e| {
            if let Expr::Ref(r) = e {
                v.push(r);
            }
        });
        v
    }
    /// Function names used (upper-case).
    pub fn functions(&self) -> Vec<&str> {
        let mut v = Vec::new();
        self.walk(&mut |e| {
            if let Expr::Call(n, _) = e {
                v.push(n.as_str());
            }
        });
        v
    }
}

impl Clone for Expr {
    fn clone(&self) -> Self {
        // One operator on a non-chain operand needs no spine.
        if let Expr::Binary(op, a, b) = self
            && !matches!(**a, Expr::Binary(..))
        {
            return Expr::Binary(*op, a.clone(), b.clone());
        }
        // Copy the left spine of an operator chain in a loop (see [`Expr`]).
        let mut spine = Vec::new();
        let mut node = self;
        while let Expr::Binary(op, a, b) = node {
            spine.push((*op, b));
            node = a;
        }
        let mut acc = match node {
            Expr::Number(n) => Expr::Number(*n),
            Expr::Text(t) => Expr::Text(t.clone()),
            Expr::Bool(b) => Expr::Bool(*b),
            Expr::Error(e) => Expr::Error(*e),
            Expr::Array(rows) => Expr::Array(rows.clone()),
            Expr::Ref(r) => Expr::Ref(r.clone()),
            Expr::Name(n) => Expr::Name(n.clone()),
            Expr::Struct(s) => Expr::Struct(s.clone()),
            Expr::Unary(op, e) => Expr::Unary(*op, e.clone()),
            Expr::Binary(op, a, b) => Expr::Binary(*op, a.clone(), b.clone()),
            Expr::Call(n, args) => Expr::Call(n.clone(), args.clone()),
            Expr::Invoke(c, args) => Expr::Invoke(c.clone(), args.clone()),
            Expr::Missing => Expr::Missing,
            Expr::Paren(e) => Expr::Paren(e.clone()),
        };
        while let Some((op, b)) = spine.pop() {
            acc = Expr::Binary(op, Box::new(acc), b.clone());
        }
        acc
    }
}

impl PartialEq for Expr {
    fn eq(&self, other: &Self) -> bool {
        // Compare the left spines of operator chains in a loop (see [`Expr`]). The right-hand
        // operands are compared only once the whole spine matches, so a mismatch on the left
        // returns before any (possibly deep) right operand is walked.
        let (mut x, mut y) = (self, other);
        let mut right_operands = Vec::new();
        let equal = loop {
            match (x, y) {
                (Expr::Binary(o1, a1, b1), Expr::Binary(o2, a2, b2)) => {
                    if o1 != o2 {
                        return false;
                    }
                    right_operands.push((b1, b2));
                    x = a1;
                    y = a2;
                }
                (Expr::Number(a), Expr::Number(b)) => break a == b,
                (Expr::Text(a), Expr::Text(b)) => break a == b,
                (Expr::Bool(a), Expr::Bool(b)) => break a == b,
                (Expr::Error(a), Expr::Error(b)) => break a == b,
                (Expr::Array(a), Expr::Array(b)) => break a == b,
                (Expr::Ref(a), Expr::Ref(b)) => break a == b,
                (Expr::Name(a), Expr::Name(b)) => break a == b,
                (Expr::Struct(a), Expr::Struct(b)) => break a == b,
                (Expr::Unary(o1, a), Expr::Unary(o2, b)) => break o1 == o2 && a == b,
                (Expr::Call(n1, a), Expr::Call(n2, b)) => break n1 == n2 && a == b,
                (Expr::Invoke(c1, a), Expr::Invoke(c2, b)) => break c1 == c2 && a == b,
                (Expr::Missing, Expr::Missing) => break true,
                (Expr::Paren(a), Expr::Paren(b)) => break a == b,
                _ => break false,
            }
        };
        equal && right_operands.into_iter().rev().all(|(b1, b2)| b1 == b2)
    }
}

pub(crate) fn clamp_anchor(row: i64, col: i64) -> Option<(u32, u32)> {
    if (0..MAX_ROWS as i64).contains(&row) && (0..MAX_COLS as i64).contains(&col) { Some((row as u32, col as u32)) } else { None }
}
