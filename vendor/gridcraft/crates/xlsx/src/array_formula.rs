//! Legacy (pre-dynamic-array) cell formulas and implicit intersection, without evaluating.
//!
//! Excel evaluates a cell formula saved without dynamic-array metadata (no `cm` attribute) the
//! legacy way: a reference that may cover several cells, used where one value is expected, is
//! intersected with the formula's row or column, and a multi-valued result shows its top-left
//! value. Dynamic-array Excel displays such a formula with the implicit intersection operator `@`
//! inserted where that happens (Microsoft support, "Implicit intersection operator: @"). The
//! reader does the same ([`apply_legacy`]), so the dynamic-array evaluator computes the legacy
//! result; the writer ([`export`]) saves a formula that is the legacy reading of some text as
//! that text, without the implied `@` (an `@` of a single value stays, as `_xlfn.SINGLE`), and
//! gives every other formula dynamic-array metadata.
//!
//! A range is harmless in SUM, but not in SQRT or arithmetic: `SUM(A1:A3*B1:B3)` intersects both
//! operands. Legacy array-aware parameters such as SUMPRODUCT's evaluate what is inside as
//! arrays, so nothing there is intersected, except IF's condition and IFERROR's and IFNA's
//! arguments ([`Param::Single`]). This is file-format policy, not a second evaluator: unknown
//! results are never calculated.

use std::collections::HashMap;
use std::sync::Arc;

use gridcraft_core::CellRef;
use gridcraft_formula::{BinOp, Expr, RefKind, SheetSel, StructItem, UnOp};
use gridcraft_model::{Formula, Workbook};

use crate::FunctionInfo;

/// What an expression produces, as far as implicit intersection is concerned.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Kind {
    /// One value, or a reference to one cell.
    Scalar,
    /// A reference that may cover several cells.
    Ref,
    /// Several values that are not a reference.
    Array,
}

impl Kind {
    fn join(self, other: Kind) -> Kind {
        match (self, other) {
            (Kind::Array, _) | (_, Kind::Array) => Kind::Array,
            (Kind::Ref, _) | (_, Kind::Ref) => Kind::Ref,
            _ => Kind::Scalar,
        }
    }
}

/// How a position consumes its operand in a legacy formula.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Ctx {
    /// One value: a multi-cell reference is intersected.
    Value,
    /// A reference or array as such (SUM's arguments); nested operations still take values.
    Ref,
    /// Array evaluation of everything inside (SUMPRODUCT's arguments): nothing is intersected.
    Array,
}
/// A function parameter's legacy evaluation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Param {
    Value,
    /// One value even inside array evaluation: IF's condition and IFERROR's and IFNA's
    /// arguments. Legacy Excel evaluates them as arrays inside SUMPRODUCT only in a
    /// Ctrl+Shift+Enter formula, which this analysis never sees. A name's definition is still
    /// evaluated as an array throughout ([`Legacy::kind_of`]).
    Single,
    Ref,
    Array,
    /// The parameter is evaluated like the call itself (IF's and CHOOSE's results).
    Pass,
    /// A value read from the top-left cell of a reference, never intersected (T: Excel caches
    /// `T(H12:J12)` as T of H12 in a formula outside columns H:J). The function lifts over the
    /// cells, so the legacy result is the top-left one.
    First,
    /// An array parameter of a dynamic-array-era function: whether Excel evaluates a legacy
    /// formula there as an array is not established, so such formulas are written as dynamic.
    Uncertain,
}

/// How the writer saves a formula that neither spills nor is a legacy CSE array.
pub(crate) enum Export {
    /// As it is, without dynamic-array metadata.
    Unchanged,
    /// As this expression (the formula without its implicit `@`), without dynamic-array metadata.
    Legacy(Expr),
    /// With dynamic-array metadata: legacy evaluation would differ.
    Dynamic,
}

pub(crate) fn export(wb: &Workbook, sheet: usize, at: CellRef, formula: &Formula, functions: FunctionInfo, names: &mut NameCache) -> Export {
    let Some(expr) = formula.expr_arc() else { return Export::Unchanged };
    let mut explicit = false;
    expr.walk(&mut |e| explicit |= matches!(e, Expr::Unary(UnOp::At, _)));
    let mut legacy = Legacy::new(wb, at, functions, names);
    if !explicit {
        legacy.formula(expr.as_ref().clone(), sheet);
        return if !legacy.certain {
            Export::Dynamic
        } else if !legacy.complete || legacy.inserted == 0 {
            // A formula that does not spill and needs no `@`. One the analysis cannot finish
            // reads back with at most a whole-formula `@`, which keeps its one value.
            Export::Unchanged
        } else {
            Export::Dynamic
        };
    }
    // The legacy text of the formula: each `@` legacy evaluation implies at its position is
    // dropped (the reader puts it back), and so is one of a range an edit made one cell; any other
    // `@` of a single value stays, written as `_xlfn.SINGLE` in the legacy formula as Excel does.
    legacy.explicit = true;
    let out = legacy.formula(expr.as_ref().clone(), sheet);
    if !legacy.complete || !legacy.certain {
        // A formula this analysis cannot finish or whose semantics it does not know, but that is
        // the reader's reading of some text: saved as that text. The reader adds a whole-formula
        // `@` over such formulas, or `@` where it intersects.
        let mut candidates = vec![remove_at(expr.as_ref().clone())];
        if let Expr::Unary(UnOp::At, x) = expr.as_ref() {
            candidates.insert(0, unwrap_at(x.as_ref().clone()));
        }
        for text in candidates {
            let read = self::legacy(wb, sheet, at, &Formula::from_expr(text.clone()), functions, names);
            if read.map_or(text == *expr, |f| f.expr_arc().is_some_and(|e| *e == *expr)) {
                return reads_back(text).map_or(Export::Dynamic, Export::Legacy);
            }
        }
        return Export::Dynamic;
    }
    if !legacy.foreign && legacy.inserted == 0 { reads_back(out).map_or(Export::Dynamic, Export::Legacy) } else { Export::Dynamic }
}

/// `e` without any `@` and the parentheses [`at`] adds with one.
fn remove_at(e: Expr) -> Expr {
    e.map(&mut |x| match x {
        Expr::Unary(UnOp::At, x) => unwrap_at(*x),
        other => other,
    })
}

/// Rewrites the legacy cell formulas at `cells` (model sheet index, positions) to their
/// dynamic-array reading, with `@` where Excel intersects. Runs once names and tables are loaded.
pub(crate) fn apply_legacy(wb: &mut Workbook, cells: Vec<(usize, Vec<CellRef>)>, functions: FunctionInfo) {
    // Defined names do not change during the pass: each is analysed once for all the cells.
    let mut names = NameCache::default();
    for (si, positions) in cells {
        let Some(sheet) = wb.sheet(si) else { continue };
        let mut changed = Vec::new();
        for at in positions {
            let Some(formula) = sheet.cell(at).and_then(|c| c.formula.as_deref()) else { continue };
            if let Some(f) = legacy(wb, si, at, formula, functions, &mut names) {
                changed.push((at, f));
            }
        }
        if changed.is_empty() {
            continue;
        }
        let Some(sheet) = wb.sheet_mut(si) else { continue };
        for (at, f) in changed {
            if let Some(cell) = sheet.cells.get_mut(at) {
                cell.formula = Some(Arc::new(f));
            }
        }
    }
}

/// The dynamic-array reading of a legacy formula, when it differs.
fn legacy(wb: &Workbook, sheet: usize, at: CellRef, formula: &Formula, functions: FunctionInfo, names: &mut NameCache) -> Option<Formula> {
    if formula.array.is_some() {
        return None;
    }
    let expr = formula.expr_arc()?;
    let mut legacy = Legacy::new(wb, at, functions, names);
    let out = legacy.formula(expr.as_ref().clone(), sheet);
    if legacy.certain && legacy.complete {
        if legacy.inserted == 0 {
            return None;
        }
        // Text round trip: the stored expression is exactly what the text parses to.
        if let Some(out) = reads_back(out) {
            return Some(Formula::new(&gridcraft_formula::print(&out)));
        }
        // The reading's `@` take it past the parser's nesting limit: handled as unfinished.
    }
    // An analysis that stopped at its limits, or met semantics it does not know (the array
    // arguments of dynamic-array functions, LAMBDA), leaves the formula as it was read, with only
    // the whole-formula `@` when the result may be several values: legacy Excel shows one value,
    // so the formula must not spill. When that text does not parse either, the formula stays as
    // it was read.
    let whole = matches!(expr.as_ref(), Expr::Unary(UnOp::At, _));
    let multi = legacy.top != Kind::Scalar || (legacy.certain && legacy.complete);
    (multi && !whole).then(|| self::at(expr.as_ref().clone())).and_then(reads_back).map(|e| Formula::new(&gridcraft_formula::print(&e)))
}

/// `e` when its text (the printer writes only the parentheses the expression has) reads back as
/// the same expression, so a dropped `@` never takes parentheses the text needs with it. An `@`
/// of a bare union has no such text, so its formula is saved as a dynamic one.
fn reads_back(e: Expr) -> Option<Expr> {
    gridcraft_formula::parse(&gridcraft_formula::print(&e)).is_ok_and(|p| p == e).then_some(e)
}

/// The operand of an `@` without the parentheses [`at`] adds.
fn unwrap_at(e: Expr) -> Expr {
    match e {
        Expr::Paren(p) if !bare(&p) => *p,
        other => other,
    }
}

/// `@e`, parenthesized unless `e` prints as one operand. An operand already in parentheses (the
/// file's own) gets a second pair, so [`unwrap_at`] can tell them from the ones added here.
fn at(e: Expr) -> Expr {
    let e = if bare(&e) { e } else { Expr::Paren(Box::new(e)) };
    Expr::Unary(UnOp::At, Box::new(e))
}

/// Prints as one operand of `@`: Excel shows a spill reference's `@` without parentheses
/// (`=@F4#`, `=@F5#%`).
fn bare(e: &Expr) -> bool {
    atomic(e) || matches!(e, Expr::Unary(UnOp::Spill, r) if matches!(**r, Expr::Ref(_)))
}

fn atomic(e: &Expr) -> bool {
    matches!(e, Expr::Ref(_) | Expr::Name(_) | Expr::Struct(_) | Expr::Call(..) | Expr::Array(_))
}

/// Nesting the analysis follows: the formula parser's own limit. Functions, parentheses and
/// prefix operators count one level each here as there, so any formula that parses is analysed
/// in full unless names, or levels the parser does not count (postfix `%` and `#`), take it
/// deeper; one that is not keeps its legacy reading ([`legacy`]). Operator chains do not count
/// (see [`Legacy::binary`]).
const MAX_DEPTH: usize = 256;

/// What the analysis of one defined name leaves for a later formula that refers to it.
#[derive(Clone)]
struct NameInfo {
    kind: Kind,
    /// The definition reached a one-cell reference written as a range.
    one_cell_range: bool,
    /// The analysis covered only established legacy semantics.
    certain: bool,
    /// Nodes of the per-formula budget the analysis used.
    cost: usize,
    /// Nesting below the reference that the analysis reached.
    depth: usize,
    /// Every name the analysis read, with its result (itself last): a hit records them all, as a
    /// fresh analysis does, so a later reference to one of them in the formula costs nothing.
    closure: Vec<(NameKey, Kind, bool)>,
}

/// A defined name as [`Legacy::names`] keys it: upper-case name, scope, collapsed.
type NameKey = (String, usize, bool);

/// Defined names analysed so far, shared by the formulas of one pass (reading a workbook, or
/// writing one) so that a name is parsed and analysed once, not once per formula that refers
/// to it. Keyed like [`Legacy::names`]. Only analyses that finished within their limits, did not
/// depend on the formula's own position and did not reuse a name the formula had already read
/// (or meet a cycle) are kept. A hit charges the budget and nesting the analysis used and records
/// every name it read, and is taken only when the formula can afford it, so a formula reads the
/// same with the cache as without it.
#[derive(Default)]
pub(crate) struct NameCache {
    map: HashMap<NameKey, NameInfo>,
    /// How many names were analysed (not found in the cache).
    pub(crate) analyses: usize,
}

/// Names kept at most: the cache is bounded even when a workbook defines a great many.
const MAX_CACHED_NAMES: usize = 65_536;

struct Legacy<'a> {
    wb: &'a Workbook,
    /// Name analyses shared with the other formulas of this pass.
    shared: &'a mut NameCache,
    /// The analysis read a structured reference without a table name, which depends on `at`.
    position_dependent: bool,
    /// The analysis reused a name this formula had already read, or met a cycle: its result
    /// leaves out what that name cost.
    reused: bool,
    /// The names read so far in this formula, in order, with their results.
    read: Vec<(NameKey, Kind, bool)>,
    /// The deepest nesting reached.
    deepest: usize,
    at: CellRef,
    functions: FunctionInfo,
    remaining: usize,
    locals: Vec<(String, Kind)>,
    /// Kinds of the names analysed so far, by upper-case name, scope and whether one-cell ranges
    /// counted as collapsed ones, with whether the definition reached a one-cell range written as
    /// a range; `None` while a name's definition is being analysed (a cycle).
    names: HashMap<(String, usize, bool), Option<(Kind, bool)>>,
    /// How many `@` were inserted.
    inserted: usize,
    /// The analysis covered only established legacy semantics.
    certain: bool,
    /// The analysis finished within its nesting and size limits.
    complete: bool,
    /// Analysing a formula that has its own `@` (see [`Legacy::explicit_at`]).
    explicit: bool,
    /// A one-cell reference written as a range (`A1:A1`, what an edit leaves of `A1:A3`) counts
    /// as one that may cover several cells, so an `@` the range had before the edit is implied.
    collapsed: bool,
    /// The analysis reached a one-cell reference written as a range, which `collapsed` changes.
    one_cell_range: bool,
    /// An explicit `@` is neither implied by legacy evaluation nor of a single value.
    foreign: bool,
    /// Analysing an expression whose rewrite is discarded (see [`Legacy::kind_of`]).
    discarded: bool,
    /// Analysing a union that is a function's single-value argument (see [`Legacy::expr`]).
    union_arg: bool,
    /// The kind of the whole formula, before any whole-formula `@`.
    top: Kind,
}

impl<'a> Legacy<'a> {
    fn new(wb: &'a Workbook, at: CellRef, functions: FunctionInfo, shared: &'a mut NameCache) -> Self {
        Legacy {
            wb,
            shared,
            position_dependent: false,
            reused: false,
            read: Vec::new(),
            deepest: 0,
            at,
            functions,
            remaining: 16384,
            locals: Vec::new(),
            names: HashMap::new(),
            inserted: 0,
            certain: true,
            complete: true,
            explicit: false,
            collapsed: false,
            one_cell_range: false,
            foreign: false,
            discarded: false,
            union_arg: false,
            top: Kind::Scalar,
        }
    }

    /// The kind of an expression analysed only for its kind (a name's definition, INDIRECT's
    /// text), evaluated as legacy Excel evaluates a name: as an array throughout, IF's condition
    /// included. Its rewrite is discarded, so it inserts no `@` into the formula.
    fn kind_of(&mut self, e: Expr, sheet: usize, depth: usize) -> Kind {
        let outer = (std::mem::replace(&mut self.discarded, true), std::mem::replace(&mut self.explicit, false), self.inserted);
        let (_, kind) = self.expr(e, sheet, Ctx::Array, depth);
        (self.discarded, self.explicit, self.inserted) = outer;
        kind
    }

    /// The whole cell formula: a multi-valued result shows its top-left value.
    fn formula(&mut self, e: Expr, sheet: usize) -> Expr {
        let e = match e {
            Expr::Unary(UnOp::At, x) if self.explicit => return self.explicit_at(*x, sheet, Ctx::Value, 0, true).0,
            e => e,
        };
        let (e, kind) = self.expr(e, sheet, Ctx::Value, 0);
        self.top = kind;
        if kind == Kind::Array && self.certain {
            self.inserted += 1;
            at(e)
        } else {
            e
        }
    }

    /// An `@` already in the formula, at a position the analysis reads in `ctx`: analysed as its
    /// operand alone. The `@` is the one legacy evaluation implies when the analysis puts it back
    /// (that insertion is then not counted, and the `@` is dropped from the legacy text). An `@`
    /// of a single value stays, as Excel keeps it (`_xlfn.SINGLE` in a legacy formula); any other
    /// `@` makes the formula a dynamic-array one (`foreign`).
    fn explicit_at(&mut self, x: Expr, sheet: usize, ctx: Ctx, depth: usize, top: bool) -> (Expr, Kind) {
        let added = matches!(&x, Expr::Paren(p) if !bare(p));
        let operand = unwrap_at(x);
        let nested = matches!(operand, Expr::Unary(UnOp::At, _));
        let before = self.inserted;
        // A one-cell range the `@` applies to, directly or through what a name returns, may be
        // what an edit left of a larger one: when the `@` is otherwise of a single value, it is
        // implied if it is with such ranges counted as several cells. The retry's analysis is
        // kept only when it implies the `@`, and runs only when the first pass reached such a
        // range (otherwise it cannot differ).
        let retry = (!nested && !self.collapsed && matches!(operand, Expr::Ref(_) | Expr::Name(_))).then(|| operand.clone());
        let seen = std::mem::replace(&mut self.one_cell_range, false);
        let (mut y, mut kind) = self.expr(operand, sheet, ctx, depth + 1);
        let mut implied = !nested && self.inserted > before;
        let shrinkable = self.one_cell_range;
        self.one_cell_range |= seen;
        if let Some(operand) = retry.filter(|_| !implied && kind == Kind::Scalar && shrinkable) {
            let state = (self.remaining, self.certain, self.complete, self.foreign);
            self.collapsed = true;
            let (y2, kind2) = self.expr(operand, sheet, ctx, depth + 1);
            self.collapsed = false;
            if self.inserted > before {
                (y, kind, implied) = (y2, kind2, true);
            } else {
                (self.remaining, self.certain, self.complete, self.foreign) = state;
            }
        }
        match y {
            Expr::Unary(UnOp::At, inner) if implied => {
                self.inserted -= 1;
                (unwrap_at(*inner), Kind::Scalar)
            }
            y if top && kind == Kind::Array => {
                // The whole-formula `@` the reader puts back over a multi-valued result.
                self.top = kind;
                (y, Kind::Scalar)
            }
            y => {
                if kind != Kind::Scalar {
                    self.foreign = true;
                }
                let y = if added { Expr::Paren(Box::new(y)) } else { y };
                (Expr::Unary(UnOp::At, Box::new(y)), Kind::Scalar)
            }
        }
    }

    fn expr(&mut self, e: Expr, sheet: usize, ctx: Ctx, depth: usize) -> (Expr, Kind) {
        // Names can be cyclic or nest deeply; their kinds are remembered, so repeated names
        // cost one analysis each.
        self.deepest = self.deepest.max(depth);
        if depth >= MAX_DEPTH || self.remaining == 0 {
            self.complete = false;
            return (e, Kind::Array);
        }
        self.remaining -= 1;
        // The flag reaches the union through parentheses and `@`, and not its operands.
        let union_arg = std::mem::take(&mut self.union_arg);
        self.union_arg = union_arg && matches!(e, Expr::Paren(_) | Expr::Unary(UnOp::At, _));
        let (e, kind) = self.node(e, sheet, ctx, depth);
        self.union_arg = false;
        if ctx == Ctx::Value && kind == Kind::Ref {
            if matches!(e, Expr::Binary(BinOp::Union, ..)) && !union_arg {
                // A union, which only stands in parentheses, as an operand: Excel has no legacy
                // text for its `@` (it shows `(F25,F26)%`), and the operator gives #VALUE! either
                // way. A function's argument is intersected, as the function would spill it.
                return (e, Kind::Scalar);
            }
            self.inserted += 1;
            (at(e), Kind::Scalar)
        } else {
            (e, kind)
        }
    }

    fn node(&mut self, e: Expr, sheet: usize, ctx: Ctx, depth: usize) -> (Expr, Kind) {
        let inner = |wanted: Ctx| if ctx == Ctx::Array { Ctx::Array } else { wanted };
        match e {
            Expr::Ref(r) => {
                let one_cell_range = r.range().is_single() && !matches!(r.sheet, SheetSel::Span(_, _)) && matches!(r.kind, RefKind::Range(..));
                self.one_cell_range |= one_cell_range;
                let single = r.range().is_single() && !matches!(r.sheet, SheetSel::Span(_, _)) && !(self.collapsed && one_cell_range);
                (Expr::Ref(r), if single { Kind::Scalar } else { Kind::Ref })
            }
            Expr::Array(rows) => {
                let kind = if rows.len() > 1 || rows.first().is_some_and(|r| r.len() > 1) { Kind::Array } else { Kind::Scalar };
                (Expr::Array(rows), kind)
            }
            Expr::Name(n) => {
                let kind = self.name(&n, sheet, depth);
                (Expr::Name(n), kind)
            }
            Expr::Struct(r) => {
                let table = if r.table.is_empty() {
                    self.position_dependent = true;
                    self.wb.sheet(sheet).and_then(|s| s.table_at(self.at))
                } else {
                    self.wb.table(&r.table).and_then(|(si, ti)| self.wb.sheet(si).and_then(|s| s.tables.get(ti)))
                };
                let one_row = r.specifiers.iter().any(|s| matches!(s, StructItem::ThisRow | StructItem::Headers | StructItem::Totals));
                let one_col = match (&r.col_start, &r.col_end) {
                    (Some(a), Some(b)) => a.eq_ignore_ascii_case(b),
                    (Some(_), None) => true,
                    _ => table.is_some_and(|t| t.columns.len() == 1),
                };
                (Expr::Struct(r), if one_row && one_col { Kind::Scalar } else { Kind::Ref })
            }
            Expr::Paren(x) => {
                let (x, kind) = self.expr(*x, sheet, ctx, depth + 1);
                (Expr::Paren(Box::new(x)), kind)
            }
            Expr::Unary(UnOp::At, x) if self.explicit => self.explicit_at(*x, sheet, ctx, depth, false),
            Expr::Unary(UnOp::At, x) => {
                let (x, _) = self.expr(*x, sheet, inner(Ctx::Ref), depth + 1);
                (Expr::Unary(UnOp::At, Box::new(x)), Kind::Scalar)
            }
            // A trim reference is a reference operator, like `:`.
            Expr::Unary(op @ (UnOp::Spill | UnOp::Trim(_)), x) => {
                let (x, _) = self.expr(*x, sheet, inner(Ctx::Ref), depth + 1);
                (Expr::Unary(op, Box::new(x)), Kind::Ref)
            }
            Expr::Unary(op, x) => {
                let (x, kind) = self.expr(*x, sheet, inner(Ctx::Value), depth + 1);
                (Expr::Unary(op, Box::new(x)), if kind == Kind::Scalar { Kind::Scalar } else { Kind::Array })
            }
            Expr::Binary(..) => self.binary(e, sheet, ctx, depth),
            Expr::Call(name, args) => self.call(name, args, sheet, ctx, depth + 1),
            Expr::Invoke(f, args) => {
                self.certain = false;
                (Expr::Invoke(f, args), Kind::Array)
            }
            other => (other, Kind::Scalar),
        }
    }

    fn name(&mut self, n: &str, sheet: usize, depth: usize) -> Kind {
        if let Some((_, kind)) = self.locals.iter().rev().find(|(key, _)| key.eq_ignore_ascii_case(n)) {
            return *kind;
        }
        let (scope, bare) = match n.rsplit_once('!') {
            Some((s, name)) => (self.wb.sheet_index(&gridcraft_core::unquote_sheet(s)).unwrap_or(sheet), name),
            None => (sheet, n),
        };
        if let Some(def) = self.wb.name(bare, scope) {
            let key = (bare.to_ascii_uppercase(), scope, self.collapsed);
            match self.names.get(&key) {
                Some(Some((kind, one_cell_range))) => {
                    self.reused = true;
                    self.one_cell_range |= *one_cell_range;
                    return *kind;
                }
                Some(None) => {
                    // A name defined in terms of itself has no legacy value to keep.
                    self.certain = false;
                    self.reused = true;
                    return Kind::Array;
                }
                None => {}
            }
            let reached = depth.saturating_add(1);
            // Taken only when the formula can afford it and has read none of the names it read:
            // otherwise the fresh analysis below differs (cut short, or reusing those names).
            let hit = self.shared.map.get(&key).filter(|i| {
                reached + i.depth < MAX_DEPTH && i.cost <= self.remaining && i.closure.iter().all(|(k, _, _)| !self.names.contains_key(k))
            });
            if let Some(info) = hit {
                // Charged and recorded as the analysis was: the formula reads as a fresh one does.
                self.deepest = self.deepest.max(reached + info.depth);
                self.certain &= info.certain;
                self.one_cell_range |= info.one_cell_range;
                self.remaining -= info.cost;
                for (k, kind, one_cell_range) in &info.closure {
                    self.names.insert(k.clone(), Some((*kind, *one_cell_range)));
                }
                self.read.extend(info.closure.iter().cloned());
                return info.kind;
            }
            let Ok(expr) = gridcraft_formula::parse_name(def.formula.trim_start_matches('=')) else { return Kind::Array };
            self.shared.analyses += 1;
            self.names.insert(key.clone(), None);
            // A name's own formula does not see the formula's LET variables.
            let locals = std::mem::take(&mut self.locals);
            let seen = std::mem::replace(&mut self.one_cell_range, false);
            let before = self.remaining;
            let first_read = self.read.len();
            let outer = (
                std::mem::replace(&mut self.certain, true),
                std::mem::replace(&mut self.complete, true),
                std::mem::replace(&mut self.position_dependent, false),
                std::mem::replace(&mut self.reused, false),
                std::mem::replace(&mut self.deepest, 0),
            );
            let kind = self.kind_of(expr, def.scope.unwrap_or(scope), depth + 1);
            let one_cell_range = std::mem::replace(&mut self.one_cell_range, seen);
            self.read.push((key.clone(), kind, one_cell_range));
            let cacheable = self.complete && !self.position_dependent && !self.reused;
            let info = cacheable.then(|| NameInfo {
                kind,
                one_cell_range,
                certain: self.certain,
                cost: before.saturating_sub(self.remaining),
                depth: self.deepest.saturating_sub(depth + 1),
                closure: self.read[first_read..].to_vec(),
            });
            self.certain &= outer.0;
            self.complete &= outer.1;
            self.position_dependent |= outer.2;
            self.reused |= outer.3;
            self.deepest = self.deepest.max(outer.4);
            self.one_cell_range |= one_cell_range;
            self.locals = locals;
            if let Some(info) = info.filter(|_| self.shared.map.len() < MAX_CACHED_NAMES) {
                self.shared.map.insert(key.clone(), info);
            }
            self.names.insert(key, Some((kind, one_cell_range)));
            return kind;
        }
        if self.wb.table(bare).is_some() { Kind::Ref } else { Kind::Scalar }
    }

    fn call(&mut self, name: String, args: Vec<Expr>, sheet: usize, ctx: Ctx, depth: usize) -> (Expr, Kind) {
        if name == "LET" && args.len() >= 3 {
            return self.let_(args, sheet, ctx, depth);
        }
        let lambda_name = !(self.functions.is_builtin)(&name) && !host_function(&name) && self.wb.name(&name, sheet).is_some();
        if args.len() > 255
            || lambda_name
            || matches!(name.as_str(), "LAMBDA" | "MAP" | "REDUCE" | "SCAN" | "BYROW" | "BYCOL" | "MAKEARRAY" | "ISOMITTED")
        {
            self.certain = false;
            return (Expr::Call(name, args), Kind::Array);
        }
        let mut lifted = false;
        let mut kinds = Vec::with_capacity(args.len());
        let mut out = Vec::with_capacity(args.len());
        for (i, arg) in args.into_iter().enumerate() {
            let param = param(&name, i, self.functions);
            let arg_ctx = match param {
                Param::Value | Param::Single => Ctx::Value,
                Param::Ref | Param::First => Ctx::Ref,
                Param::Pass => ctx,
                Param::Array => Ctx::Array,
                Param::Uncertain => {
                    self.certain = false;
                    Ctx::Array
                }
            };
            let arg_ctx = if ctx == Ctx::Array && (param != Param::Single || self.discarded) { Ctx::Array } else { arg_ctx };
            // A union argument, under the file's parentheses and any `@` the reader put on it.
            let mut operand = &arg;
            while let Expr::Paren(x) | Expr::Unary(UnOp::At, x) = operand {
                operand = x;
            }
            self.union_arg = arg_ctx == Ctx::Value && matches!(operand, Expr::Binary(BinOp::Union, ..));
            let (arg, kind) = self.expr(arg, sheet, arg_ctx, depth);
            // A value parameter given several values evaluates the function once per value.
            lifted |= match param {
                Param::Value | Param::Single => kind == Kind::Array || (kind == Kind::Ref && arg_ctx == Ctx::Array),
                Param::First => kind != Kind::Scalar,
                _ => false,
            };
            kinds.push(kind);
            out.push(arg);
        }
        let first = kinds.first().copied().unwrap_or(Kind::Scalar);
        let kind = match name.as_str() {
            "IF" | "CHOOSE" if !lifted => kinds.iter().skip(1).fold(Kind::Scalar, |k, x| k.join(*x)),
            "ROW" | "COLUMN" => {
                if first == Kind::Scalar {
                    Kind::Scalar
                } else if ctx == Ctx::Array {
                    Kind::Array
                } else {
                    // Outside array evaluation, ROW and COLUMN of a range give its first row or column.
                    self.inserted += 1;
                    return (at(Expr::Call(name, out)), Kind::Scalar);
                }
            }
            "INDIRECT" => match out.first() {
                Some(Expr::Text(text)) if !matches!(out.get(1), Some(Expr::Bool(false))) => match gridcraft_formula::parse(text) {
                    Ok(e) => {
                        let kind = self.kind_of(e, sheet, depth);
                        if kind == Kind::Scalar { Kind::Scalar } else { Kind::Ref }
                    }
                    Err(_) => Kind::Ref,
                },
                _ => Kind::Ref,
            },
            "OFFSET" => offset_kind(&out, first, lifted),
            "INDEX" => index_kind(&out, &kinds, first, lifted),
            "XLOOKUP" if !lifted => match (out.get(1), out.get(2)) {
                (Some(Expr::Ref(lookup)), Some(Expr::Ref(result))) => {
                    let multi = if lookup.range().width() == 1 { result.range().width() > 1 } else { result.range().height() > 1 };
                    if multi { Kind::Ref } else { Kind::Scalar }
                }
                _ => Kind::Ref,
            },
            "ANCHORARRAY" | "TRIMRANGE" => Kind::Ref,
            "FILTER" | "SORT" | "SORTBY" | "UNIQUE" | "SEQUENCE" | "RANDARRAY" | "VSTACK" | "HSTACK" | "TOROW" | "TOCOL" | "WRAPROWS"
            | "WRAPCOLS" | "TAKE" | "DROP" | "CHOOSEROWS" | "CHOOSECOLS" | "EXPAND" | "GROUPBY" | "PIVOTBY" | "TRANSPOSE" | "MMULT" | "MINVERSE"
            | "MUNIT" | "FREQUENCY" | "LINEST" | "LOGEST" | "TREND" | "GROWTH" | "MODE.MULT" | "TEXTSPLIT" | "FILTERXML" => Kind::Array,
            // Every match (mode 1) or the capture groups (mode 2); the first match by default.
            "REGEXEXTRACT" => match out.get(2) {
                None | Some(Expr::Missing) => Kind::Scalar,
                Some(Expr::Number(n)) if *n == 0.0 => Kind::Scalar,
                _ => Kind::Array,
            },
            // Google Sheets' export placeholder fails with #NAME? so an outer IFERROR keeps the cached scalar.
            "__XLUDF.DUMMYFUNCTION" => Kind::Scalar,
            // A user-defined function may return a range or an array.
            n if user_function(n, self.functions) => Kind::Array,
            _ if lifted => Kind::Array,
            _ => Kind::Scalar,
        };
        (Expr::Call(name, out), kind)
    }
}

fn offset_kind(out: &[Expr], first: Kind, lifted: bool) -> Kind {
    // Omitted dimensions keep the source reference's shape; unknown dimensions might
    // become several cells on a later calculation.
    let first_multi = first != Kind::Scalar;
    let multi = if out.len() <= 3 { first_multi } else { !out.get(3).is_some_and(one) || out.get(4).map_or(first_multi, |e| !one(e)) };
    if lifted {
        Kind::Array
    } else if multi {
        Kind::Ref
    } else {
        Kind::Scalar
    }
}

fn index_kind(out: &[Expr], kinds: &[Kind], first: Kind, lifted: bool) -> Kind {
    let row = out.get(1);
    let col = out.get(2);
    let row_positive = row.is_some_and(|e| positive_index(e, kinds.get(1).copied()));
    let col_positive = col.is_some_and(|e| positive_index(e, kinds.get(2).copied()));
    let multi = !row_positive || col.is_some_and(|_| !col_positive) || (col.is_none() && !out.first().is_some_and(one_line));
    if multi {
        if first == Kind::Array { Kind::Array } else { Kind::Ref }
    } else if lifted {
        Kind::Array
    } else {
        Kind::Scalar
    }
}

impl Legacy<'_> {
    /// A binary operation. `A1+A2+…+A70` parses as a left-nested chain as deep as it is long, so
    /// the chain's left operands are walked iteratively, at the same depth; each operation still
    /// costs one node of the budget.
    fn binary(&mut self, e: Expr, sheet: usize, ctx: Ctx, depth: usize) -> (Expr, Kind) {
        let operand = |op: BinOp, node: Ctx| {
            if node == Ctx::Array {
                Ctx::Array
            } else if matches!(op, BinOp::Range | BinOp::Union | BinOp::Intersect) {
                Ctx::Ref
            } else {
                Ctx::Value
            }
        };
        // (operator, right operand, the operation's context), outermost first.
        let mut spine = Vec::new();
        let mut cur = e;
        let mut cur_ctx = ctx;
        while let Expr::Binary(op, a, b) = cur {
            if !spine.is_empty() {
                // The caller's `expr` counted the outermost operation.
                if self.remaining == 0 {
                    self.complete = false;
                } else {
                    self.remaining -= 1;
                }
            }
            spine.push((op, *b, cur_ctx));
            cur_ctx = operand(op, cur_ctx);
            cur = *a;
        }
        let (mut acc, mut kind) = self.expr(cur, sheet, cur_ctx, depth);
        for (i, (op, b, node_ctx)) in spine.into_iter().enumerate().rev() {
            let (b, kb) = self.expr(b, sheet, operand(op, node_ctx), depth);
            let node = Expr::Binary(op, Box::new(acc), Box::new(b));
            let k = if matches!(op, BinOp::Range | BinOp::Union | BinOp::Intersect) {
                Kind::Ref
            } else if kind == Kind::Scalar && kb == Kind::Scalar {
                Kind::Scalar
            } else {
                Kind::Array
            };
            // An inner operation is an operand: `expr` would intersect it there.
            (acc, kind) = if i > 0 && node_ctx == Ctx::Value && k == Kind::Ref {
                self.inserted += 1;
                (at(node), Kind::Scalar)
            } else {
                (node, k)
            };
        }
        (acc, kind)
    }

    fn let_(&mut self, args: Vec<Expr>, sheet: usize, ctx: Ctx, depth: usize) -> (Expr, Kind) {
        let saved = self.locals.len();
        let count = args.len();
        let mut out = Vec::with_capacity(count);
        let mut pending: Option<String> = None;
        let mut kind = Kind::Scalar;
        for (i, arg) in args.into_iter().enumerate() {
            if i + 1 == count {
                let (body, k) = self.expr(arg, sheet, ctx, depth);
                out.push(body);
                kind = k;
            } else if i % 2 == 0 {
                pending = match &arg {
                    Expr::Name(n) => Some(n.clone()),
                    _ => None,
                };
                out.push(arg);
            } else {
                let value_ctx = if ctx == Ctx::Array { Ctx::Array } else { Ctx::Ref };
                let (value, k) = self.expr(arg, sheet, value_ctx, depth);
                if let Some(n) = pending.take() {
                    self.locals.push((n, k));
                }
                out.push(value);
            }
        }
        self.locals.truncate(saved);
        (Expr::Call("LET".into(), out), kind)
    }
}

fn positive(e: &Expr) -> bool {
    matches!(e, Expr::Number(n) if *n >= 1.0)
}
fn positive_index(e: &Expr, kind: Option<Kind>) -> bool {
    // MATCH returns a one-based position or an error, never INDEX's zero whole-row selector.
    positive(e) || (matches!(e, Expr::Call(name, _) if matches!(name.as_str(), "MATCH" | "XMATCH")) && kind.is_some_and(|k| k != Kind::Ref))
}

fn one(e: &Expr) -> bool {
    matches!(e, Expr::Number(n) if *n == 1.0)
}

/// A one-column or one-row reference: INDEX with a single index picks one cell of it.
fn one_line(e: &Expr) -> bool {
    matches!(e, Expr::Ref(r) if r.range().width() == 1 || r.range().height() == 1)
}

/// Functions the calculation engine evaluates itself, whose parameters are described here
/// rather than by the function library's lifting metadata.
fn host_function(name: &str) -> bool {
    matches!(
        name,
        "IF" | "IFS"
            | "IFERROR"
            | "IFNA"
            | "CHOOSE"
            | "SWITCH"
            | "ROW"
            | "COLUMN"
            | "ROWS"
            | "COLUMNS"
            | "AREAS"
            | "ISREF"
            | "ISFORMULA"
            | "FORMULATEXT"
            | "SHEET"
            | "SHEETS"
            | "OFFSET"
            | "INDIRECT"
            | "INDEX"
            | "CELL"
            | "TRIMRANGE"
            | "SUBTOTAL"
            | "AGGREGATE"
            | "TEXT"
            | "LET"
            | "LAMBDA"
            | "ISOMITTED"
            | "MAP"
            | "REDUCE"
            | "SCAN"
            | "BYROW"
            | "BYCOL"
            | "MAKEARRAY"
            | "ANCHORARRAY"
            | "GETPIVOTDATA"
    )
}

/// Not a built-in worksheet function: an add-in or VBA function.
fn user_function(name: &str, functions: FunctionInfo) -> bool {
    !host_function(name) && !(functions.is_builtin)(name)
}

fn param(name: &str, i: usize, functions: FunctionInfo) -> Param {
    match name {
        "IF" if i == 0 => Param::Single,
        "IFERROR" | "IFNA" => Param::Single,
        "CHOOSE" if i == 0 => Param::Value,
        "IF" | "CHOOSE" => Param::Pass,
        "SUMPRODUCT" | "MMULT" | "MINVERSE" | "MDETERM" | "TRANSPOSE" | "FREQUENCY" | "LINEST" | "LOGEST" | "TREND" | "GROWTH" => Param::Array,
        "INDEX" if i == 0 => Param::Array,
        // LOOKUP's vectors and AGGREGATE's array form evaluate their arguments as arrays.
        "LOOKUP" if i > 0 => Param::Array,
        "AGGREGATE" if i == 2 => Param::Array,
        "ROW" | "COLUMN" | "ROWS" | "COLUMNS" | "AREAS" | "ISREF" | "ISFORMULA" | "FORMULATEXT" | "SHEET" | "SHEETS" | "TRIMRANGE"
        | "ANCHORARRAY" => Param::Ref,
        "CELL" | "GETPIVOTDATA" if i == 1 => Param::Ref,
        "T" => Param::First,
        "N" if i == 0 => Param::Value,
        "FILTER" | "SORT" | "SORTBY" | "UNIQUE" | "VSTACK" | "HSTACK" | "TOROW" | "TOCOL" | "WRAPROWS" | "WRAPCOLS" | "TAKE" | "DROP"
        | "CHOOSEROWS" | "CHOOSECOLS" | "EXPAND" | "GROUPBY" | "PIVOTBY"
            if !scalar_arg(name, i) =>
        {
            Param::Uncertain
        }
        n if user_function(n, functions) => Param::Ref,
        n if scalar_arg(n, i) && (host_function(n) || (functions.is_scalar_arg)(n, i)) => Param::Value,
        _ => Param::Ref,
    }
}

/// Legacy reference/array-taking parameters. Other parameters consume a single value; the
/// function library's lifting metadata refines this for the functions it implements.
fn scalar_arg(name: &str, i: usize) -> bool {
    match name {
        "SUM" | "AVERAGE" | "AVERAGEA" | "MIN" | "MINA" | "MAX" | "MAXA" | "COUNT" | "COUNTA" | "COUNTBLANK" | "PRODUCT" | "SUMSQ" | "MEDIAN"
        | "MODE" | "MODE.SNGL" | "STDEV" | "STDEV.S" | "STDEV.P" | "STDEVP" | "VAR" | "VAR.S" | "VAR.P" | "VARP" | "AVEDEV" | "DEVSQ" | "GEOMEAN"
        | "HARMEAN" | "KURT" | "SKEW" | "SKEW.P" | "AND" | "OR" | "XOR" | "SUMPRODUCT" | "SUMX2MY2" | "SUMX2PY2" | "SUMXMY2" | "CORREL" | "COVAR"
        | "COVARIANCE.P" | "COVARIANCE.S" | "PEARSON" | "RSQ" | "SLOPE" | "INTERCEPT" | "STEYX" | "ROWS" | "COLUMNS" | "AREAS" | "ROW" | "COLUMN"
        | "TRANSPOSE" | "ISREF" | "MMULT" | "MINVERSE" | "MDETERM" | "FREQUENCY" | "CONCAT" | "VSTACK" | "HSTACK" | "ANCHORARRAY" => false,
        "SUMIF" | "AVERAGEIF" => i == 1,
        "COUNTIF" => i != 0,
        "SUMIFS" | "AVERAGEIFS" | "MINIFS" | "MAXIFS" => i > 0 && i.is_multiple_of(2),
        "COUNTIFS" => i % 2 == 1,
        "INDEX" | "LARGE" | "SMALL" | "PERCENTILE" | "PERCENTILE.INC" | "PERCENTILE.EXC" | "QUARTILE" | "QUARTILE.INC" | "QUARTILE.EXC" | "IRR"
        | "MIRR" | "RANK" | "RANK.EQ" | "RANK.AVG" => i != if name.starts_with("RANK") { 1 } else { 0 },
        "VLOOKUP" | "HLOOKUP" | "MATCH" | "XMATCH" => i != 1,
        "LOOKUP" | "NPV" | "XNPV" | "SUBTOTAL" => i == 0,
        "XLOOKUP" => i != 1 && i != 2,
        "XIRR" => i > 1,
        "OFFSET" => i != 0,
        "AGGREGATE" | "TEXTJOIN" => i < 2,
        "FILTER" | "SORTBY" => i > 1,
        "SORT" | "UNIQUE" | "TAKE" | "DROP" | "CHOOSECOLS" | "CHOOSEROWS" | "TOROW" | "TOCOL" | "WRAPROWS" | "WRAPCOLS" | "EXPAND" => i != 0,
        "LINEST" | "LOGEST" | "TREND" | "GROWTH" => i > 1,
        "FORECAST" | "FORECAST.LINEAR" => i == 0,
        "FORECAST.ETS" | "FORECAST.ETS.CONFINT" => i != 1 && i != 2,
        "FORECAST.ETS.SEASONALITY" | "FORECAST.ETS.STAT" => i > 1,
        _ => true,
    }
}
