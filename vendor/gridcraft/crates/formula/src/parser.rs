//! Pratt parser: tokens → [`Expr`].

use std::sync::Arc;

use gridcraft_core::addr::{letters_to_col, parse_a1_prefix};
use gridcraft_core::{CellError, MAX_ROWS};

use crate::ast::{Anchor, BinOp, Expr, RefKind, Reference, SheetSel, StructItem, StructRef, TrimMode, UnOp};
use crate::lexer::{Tok, Token, tokenize_locale};

#[derive(Clone, Debug, PartialEq, thiserror::Error)]
#[error("{msg}")]
pub struct ParseError {
    pub msg: String,
    pub pos: usize,
}

pub(crate) const MAX_DEPTH: usize = 256;

/// Longest formula accepted, in characters: Excel's limit for formula contents. It also bounds
/// the depth of the trees long operator chains build (`1+1+…`), which dropping and evaluating
/// walk recursively; without it a formula of a few hundred kilobytes in a file overflows the stack.
pub const MAX_FORMULA_LEN: usize = 8_192;

/// Parses formula text. A leading `=` (or `+`/`-` as Lotus users type) is accepted and skipped
/// for `=`.
pub fn parse(src: &str) -> Result<Expr, ParseError> {
    parse_syntax(src, crate::FormulaLocale::En)
}

/// Parses a defined name's formula. Unlike a cell formula, a comma at the top level is the
/// union operator: Excel stores and shows a name over several ranges as `Sheet1!$A$1,Sheet1!$C$1`.
pub fn parse_name(src: &str) -> Result<Expr, ParseError> {
    parse_top(src, crate::FormulaLocale::En, true)
}

/// Most closing parentheses [`parse_closing_parens`] adds.
const MAX_CLOSED_PARENS: usize = 8;

/// Parses formula text as typed into a cell with `parse`, closing up to eight missing `)` at the
/// end as Excel does (`SUM(1,2` enters `SUM(1,2)`). When no number of `)` makes it parse, the
/// error describes the text as typed: `1+` is "unexpected end of formula", not "unexpected `)`".
pub fn parse_closing_parens<T>(src: &str, parse: impl Fn(&str) -> Result<T, ParseError>) -> Result<T, ParseError> {
    let typed = match parse(src) {
        Ok(parsed) => return Ok(parsed),
        Err(e) => e,
    };
    let mut text = src.to_string();
    for _ in 0..MAX_CLOSED_PARENS {
        text.push(')');
        if let Ok(parsed) = parse(&text) {
            return Ok(parsed);
        }
    }
    Err(typed)
}

pub(crate) fn parse_syntax(src: &str, locale: crate::FormulaLocale) -> Result<Expr, ParseError> {
    parse_top(src, locale, false)
}

fn parse_top(src: &str, locale: crate::FormulaLocale, allow_union: bool) -> Result<Expr, ParseError> {
    let body = src.strip_prefix('=').unwrap_or(src);
    if body.len() > MAX_FORMULA_LEN && body.chars().count() > MAX_FORMULA_LEN {
        return Err(ParseError { msg: format!("formula is longer than {MAX_FORMULA_LEN} characters"), pos: 0 });
    }
    let toks = tokenize_locale(body, locale).map_err(|e| ParseError { msg: e.msg, pos: e.pos })?;
    let toks = significant_spaces(toks);
    let mut p = Parser { toks, i: 0, depth: 0, input: locale == crate::FormulaLocale::Es };
    let e = p.expr(0, allow_union)?;
    match p.peek() {
        Tok::Eof if wrapper_height(&e) > MAX_DEPTH => Err(ParseError { msg: "formula is nested too deeply".into(), pos: 0 }),
        Tok::Eof => Ok(e),
        t => Err(p.err(format!("unexpected {}", describe(t)))),
    }
}

/// The most unary operators, parentheses and invocations on one path of `e`. The parser bounds
/// each level's postfix wrappers by its own depth, so levels inside levels can still stack them
/// far past [`MAX_DEPTH`]; code that maps over the tree recurses once per wrapper.
fn wrapper_height(e: &Expr) -> usize {
    let mut most = 0;
    let mut pending = vec![(e, 0usize)];
    while let Some((e, h)) = pending.pop() {
        most = most.max(h);
        match e {
            Expr::Unary(_, a) | Expr::Paren(a) => pending.push((a, h + 1)),
            Expr::Invoke(callee, args) => {
                pending.push((callee, h + 1));
                pending.extend(args.iter().map(|a| (a, h + 1)));
            }
            Expr::Binary(_, a, b) => pending.extend([(&**a, h), (&**b, h)]),
            Expr::Call(_, args) => pending.extend(args.iter().map(|a| (a, h))),
            Expr::Array(rows) => pending.extend(rows.iter().flatten().map(|a| (a, h))),
            _ => {}
        }
    }
    most
}

fn describe(t: &Tok) -> String {
    match t {
        Tok::RParen => "`)`".into(),
        Tok::Comma => "`,`".into(),
        Tok::Eof => "end of formula".into(),
        other => format!("{other:?}"),
    }
}

/// Keeps a space token only where it is the intersection operator: between the end of an
/// operand and the start of another.
fn significant_spaces(toks: Vec<Token>) -> Vec<Token> {
    let mut out: Vec<Token> = Vec::with_capacity(toks.len());
    for (i, t) in toks.iter().enumerate() {
        if t.tok == Tok::Space {
            let prev_end = out.last().is_some_and(|p| matches!(p.tok, Tok::Word(_) | Tok::RParen | Tok::Struct(_) | Tok::Error(_)));
            let next_start =
                toks.get(i + 1).is_some_and(|n| matches!(n.tok, Tok::Word(_) | Tok::Sheet(..) | Tok::LParen | Tok::Func(_) | Tok::Error(_)));
            if prev_end && next_start {
                out.push(t.clone());
            }
            continue;
        }
        out.push(t.clone());
    }
    out
}

struct Parser {
    toks: Vec<Token>,
    i: usize,
    depth: usize,
    input: bool,
}

fn word_is_cols(w: &str) -> Option<(u32, bool)> {
    let abs = w.starts_with('$');
    letters_to_col(w.trim_start_matches('$')).map(|c| (c, abs))
}

fn word_is_row(w: &str) -> Option<(u32, bool)> {
    let abs = w.starts_with('$');
    let n: u32 = w.trim_start_matches('$').parse().ok()?;
    if n == 0 || n > MAX_ROWS { None } else { Some((n - 1, abs)) }
}

fn word_is_cell(w: &str) -> Option<Anchor> {
    let (col, row, col_abs, row_abs, rest) = parse_a1_prefix(w)?;
    if !rest.is_empty() {
        return None;
    }
    Some(Anchor { row, col, row_abs, col_abs })
}

/// `_xlfn.XLOOKUP` → `XLOOKUP`. `_xludf.` is kept (`_xludf.SUM`): it marks a user-defined
/// function, never the built-in of the same name, and files need it back.
pub fn normalize_function_name(name: &str) -> String {
    let u = name.to_ascii_uppercase();
    for p in ["_XLFN._XLWS.", "_XLFN.", "_XLWS."] {
        if let Some(r) = u.strip_prefix(p) {
            return r.to_string();
        }
    }
    match u.strip_prefix("_XLUDF.") {
        Some(r) => format!("_xludf.{r}"),
        None => u,
    }
}

impl Parser {
    fn peek(&self) -> &Tok {
        self.toks.get(self.i).map(|t| &t.tok).unwrap_or(&Tok::Eof)
    }
    fn peek_at(&self, k: usize) -> &Tok {
        self.toks.get(self.i + k).map(|t| &t.tok).unwrap_or(&Tok::Eof)
    }
    fn pos(&self) -> usize {
        self.toks.get(self.i).map(|t| t.start).unwrap_or(0)
    }
    fn next(&mut self) -> Tok {
        let t = self.peek().clone();
        if self.i < self.toks.len() {
            self.i += 1;
        }
        t
    }
    fn err(&self, msg: impl Into<String>) -> ParseError {
        ParseError { msg: msg.into(), pos: self.pos() }
    }
    fn expect(&mut self, t: Tok) -> Result<(), ParseError> {
        if *self.peek() == t {
            self.next();
            Ok(())
        } else {
            Err(self.err(format!("expected {}, found {}", describe(&t), describe(self.peek()))))
        }
    }

    fn binop(&self, allow_union: bool) -> Option<BinOp> {
        Some(match self.peek() {
            Tok::Op("+") => BinOp::Add,
            Tok::Op("-") => BinOp::Sub,
            Tok::Op("*") => BinOp::Mul,
            Tok::Op("/") => BinOp::Div,
            Tok::Op("^") => BinOp::Pow,
            Tok::Op("&") => BinOp::Concat,
            Tok::Op("=") => BinOp::Eq,
            Tok::Op("<>") => BinOp::Ne,
            Tok::Op("<") => BinOp::Lt,
            Tok::Op(">") => BinOp::Gt,
            Tok::Op("<=") => BinOp::Le,
            Tok::Op(">=") => BinOp::Ge,
            Tok::Colon => BinOp::Range,
            Tok::Space => BinOp::Intersect,
            Tok::Comma if allow_union => BinOp::Union,
            _ => return None,
        })
    }

    fn expr(&mut self, min_bp: u8, allow_union: bool) -> Result<Expr, ParseError> {
        self.depth += 1;
        if self.depth > MAX_DEPTH {
            return Err(self.err("formula is nested too deeply"));
        }
        let mut lhs = self.prefix(allow_union)?;
        // Postfix operators (`%`, invoking a result) wrap `lhs` without recursing: they count
        // towards the nesting limit, so `1%%%…` can't build a tree deeper than it.
        let mut postfix = 0;
        loop {
            if *self.peek() == Tok::Op("%") {
                if 70 < min_bp {
                    break;
                }
                postfix += 1;
                if self.depth + postfix > MAX_DEPTH {
                    return Err(self.err("formula is nested too deeply"));
                }
                self.next();
                lhs = Expr::Unary(UnOp::Percent, Box::new(lhs));
                continue;
            }
            if *self.peek() == Tok::LParen && matches!(lhs, Expr::Call(..) | Expr::Invoke(..) | Expr::Paren(_) | Expr::Name(_)) {
                // Invoking a LAMBDA result.
                if let Expr::Name(_) = lhs {
                    // A name followed by '(' was lexed as Func; this is `(name)(…)` only.
                }
                postfix += 1;
                if self.depth + postfix > MAX_DEPTH {
                    return Err(self.err("formula is nested too deeply"));
                }
                self.next();
                let args = self.args()?;
                lhs = Expr::Invoke(Box::new(lhs), args);
                continue;
            }
            if let Tok::Trim(mode) = *self.peek() {
                // A trim reference is the range operator with a modifier: same binding power.
                let bp = BinOp::Range.precedence();
                if bp < min_bp {
                    break;
                }
                self.next();
                let rhs = self.expr(bp + 1, allow_union)?;
                lhs = Expr::Unary(UnOp::Trim(mode), Box::new(combine(BinOp::Range, lhs, rhs)));
                continue;
            }
            let Some(op) = self.binop(allow_union) else { break };
            let bp = op.precedence();
            if bp < min_bp {
                break;
            }
            self.next();
            let rhs = self.expr(bp + 1, allow_union)?;
            lhs = combine(op, lhs, rhs);
        }
        self.depth -= 1;
        Ok(lhs)
    }

    fn args(&mut self) -> Result<Vec<Expr>, ParseError> {
        let mut args = Vec::new();
        if *self.peek() == Tok::RParen {
            self.next();
            return Ok(args);
        }
        loop {
            let a = match self.peek() {
                Tok::Comma | Tok::RParen => Expr::Missing,
                _ => self.expr(0, false)?,
            };
            args.push(a);
            match self.next() {
                Tok::Comma => continue,
                Tok::RParen => break,
                t => return Err(self.err(format!("expected `,` or `)`, found {}", describe(&t)))),
            }
        }
        if args.len() > 255 {
            return Err(self.err("too many arguments"));
        }
        Ok(args)
    }

    fn prefix(&mut self, allow_union: bool) -> Result<Expr, ParseError> {
        match self.next() {
            Tok::Number(n) => Ok(Expr::Number(n)),
            Tok::Text(s) => Ok(Expr::Text(Arc::from(s))),
            Tok::Bool(b) => Ok(Expr::Bool(b)),
            Tok::Error(e) => Ok(Expr::Error(e)),
            Tok::Op("-") => Ok(Expr::Unary(UnOp::Neg, Box::new(self.expr(60, allow_union)?))),
            Tok::Op("+") => Ok(Expr::Unary(UnOp::Plus, Box::new(self.expr(60, allow_union)?))),
            Tok::Op("@") => Ok(Expr::Unary(UnOp::At, Box::new(self.expr(95, allow_union)?))),
            Tok::LParen => {
                let e = self.expr(0, true)?;
                self.expect(Tok::RParen)?;
                Ok(Expr::Paren(Box::new(e)))
            }
            Tok::LBrace => self.array(),
            Tok::Func(name) => {
                let name = normalize_function_name(&name);
                let mut args = self.args()?;
                // Files write `A1#` as `_xlfn.ANCHORARRAY(A1)`.
                if name == "ANCHORARRAY"
                    && let [Expr::Ref(Reference { kind: RefKind::Cell(_), .. })] = args.as_slice()
                    && let Some(r) = args.pop()
                {
                    return Ok(Expr::Unary(UnOp::Spill, Box::new(r)));
                }
                // Files write `@x` as `_xlfn.SINGLE(x)`; `SINGLE(A1:A3*2)` is `@(A1:A3*2)`.
                if name == "SINGLE"
                    && args.len() == 1
                    && let Some(x) = args.pop()
                {
                    let x = match x {
                        Expr::Binary(..) | Expr::Unary(UnOp::Neg | UnOp::Plus | UnOp::Percent, _) => Expr::Paren(Box::new(x)),
                        x => x,
                    };
                    return Ok(Expr::Unary(UnOp::At, Box::new(x)));
                }
                // Files may spell a trim reference as a function call (`_xlfn._TRO_ALL(A1:E10)`).
                let trim = match name.as_str() {
                    "_TRO_LEADING" => Some(TrimMode::Leading),
                    "_TRO_TRAILING" => Some(TrimMode::Trailing),
                    "_TRO_ALL" => Some(TrimMode::All),
                    _ => None,
                };
                if let Some(mode) = trim
                    && args.len() == 1
                    && let Some(x) = args.pop()
                {
                    return Ok(Expr::Unary(UnOp::Trim(mode), Box::new(x)));
                }
                Ok(Expr::Call(name, args))
            }
            Tok::Sheet(a, b) => {
                let sheet = match b {
                    Some(b) => SheetSel::Span(a, b),
                    None => SheetSel::Named(a),
                };
                match self.next() {
                    Tok::Word(w) => self.reference_from_word(sheet, &w),
                    Tok::Error(CellError::Ref) => Ok(Expr::Error(CellError::Ref)),
                    t => Err(self.err(format!("expected a reference after the sheet name, found {}", describe(&t)))),
                }
            }
            Tok::Word(w) => {
                if let Tok::Struct(_) = self.peek()
                    && word_is_cell(&w).is_none()
                {
                    let pos = self.pos();
                    let Tok::Struct(body) = self.next() else { return Err(self.err("internal")) };
                    return parse_struct(&w, &body).map(Expr::Struct).map_err(|msg| ParseError { msg, pos });
                }
                self.reference_from_word(SheetSel::Current, &w)
            }
            Tok::Struct(body) => parse_struct("", &body).map(Expr::Struct).map_err(|msg| self.err(msg)),
            t => Err(self.err(format!("unexpected {}", describe(&t)))),
        }
    }

    /// A word after an optional sheet: a cell, a column/row range (`A:C`, `3:5`) or a name. A
    /// trim operator (`A1:.E10`, `A:.A`) in place of the colon wraps the range in
    /// [`UnOp::Trim`].
    fn reference_from_word(&mut self, sheet: SheetSel, w: &str) -> Result<Expr, ParseError> {
        let trim = match self.peek() {
            Tok::Colon => Some(None),
            Tok::Trim(mode) => Some(Some(*mode)),
            _ => None,
        };
        let wrap = |kind: RefKind, sheet: SheetSel| {
            let r = Expr::Ref(Reference { sheet, kind });
            match trim {
                Some(Some(mode)) => Expr::Unary(UnOp::Trim(mode), Box::new(r)),
                _ => r,
            }
        };
        if let Some(a) = word_is_cell(w) {
            // Cell, possibly `A1:B2` (merged here so the sheet applies to both ends).
            if trim.is_some()
                && let Tok::Word(w2) = self.peek_at(1)
                && let Some(b) = word_is_cell(w2)
            {
                self.next();
                self.next();
                return Ok(wrap(RefKind::Range(a, b), sheet));
            }
            let cell = Expr::Ref(Reference { sheet, kind: RefKind::Cell(a) });
            if *self.peek() == Tok::Op("#") {
                self.next();
                return Ok(Expr::Unary(UnOp::Spill, Box::new(cell)));
            }
            return Ok(cell);
        }
        let second = match self.peek_at(1) {
            Tok::Word(w2) => Some(w2.clone()),
            Tok::Number(n) if n.fract() == 0.0 && *n >= 1.0 => Some(format!("{n}")),
            _ => None,
        };
        if trim.is_some()
            && let Some(w2) = second
        {
            if let (Some((c0, a0)), Some((c1, a1))) = (word_is_cols(w), word_is_cols(&w2)) {
                self.next();
                self.next();
                return Ok(wrap(RefKind::Cols(c0, a0, c1, a1), sheet));
            }
            if let (Some((r0, a0)), Some((r1, a1))) = (word_is_row(w), word_is_row(&w2)) {
                self.next();
                self.next();
                return Ok(wrap(RefKind::Rows(r0, a0, r1, a1), sheet));
            }
        }
        if sheet != SheetSel::Current {
            // Sheet-scoped name `Sheet1!MyName`.
            if let SheetSel::Named(s) = sheet {
                return Ok(Expr::Name(format!("{s}!{w}")));
            }
            return Err(self.err("invalid 3-D reference"));
        }
        if w.chars().next().is_some_and(|c| c.is_ascii_digit()) {
            return Err(self.err(format!("unexpected `{w}`")));
        }
        // A function passed by name is saved as `_xleta.SUM`.
        if let Some(f) = w.get(..7).filter(|p| p.eq_ignore_ascii_case("_xleta.")).and_then(|_| w.get(7..)) {
            return Ok(Expr::Name(f.to_ascii_uppercase()));
        }
        Ok(Expr::Name(w.to_string()))
    }

    fn array(&mut self) -> Result<Expr, ParseError> {
        let mut rows = vec![Vec::new()];
        loop {
            let el = match self.next() {
                Tok::Number(n) => Expr::Number(n),
                Tok::Op("-") => match self.next() {
                    Tok::Number(n) => Expr::Number(-n),
                    t => return Err(self.err(format!("expected a number in array, found {}", describe(&t)))),
                },
                Tok::Op("+") => match self.next() {
                    Tok::Number(n) => Expr::Number(n),
                    t => return Err(self.err(format!("expected a number in array, found {}", describe(&t)))),
                },
                Tok::Text(s) => Expr::Text(Arc::from(s)),
                Tok::Bool(b) => Expr::Bool(b),
                Tok::Word(w) if self.input => crate::input::boolean_alias(&w)
                    .map(Expr::Bool)
                    .ok_or_else(|| self.err(format!("array constants may only contain constants, found {}", describe(&Tok::Word(w)))))?,
                Tok::Error(e) => Expr::Error(e),
                t => return Err(self.err(format!("array constants may only contain constants, found {}", describe(&t)))),
            };
            if let Some(r) = rows.last_mut() {
                r.push(el);
            }
            match self.next() {
                Tok::Comma => {}
                Tok::Semicolon => rows.push(Vec::new()),
                Tok::RBrace => break,
                t => return Err(self.err(format!("expected `,`, `;` or `}}`, found {}", describe(&t)))),
            }
        }
        let w = rows.first().map_or(0, Vec::len);
        if rows.iter().any(|r| r.len() != w) {
            return Err(self.err("array rows must have the same length"));
        }
        Ok(Expr::Array(rows))
    }
}

/// Folds `A1:B2`-like range operations of two plain refs into a single reference.
fn combine(op: BinOp, lhs: Expr, rhs: Expr) -> Expr {
    if op == BinOp::Range
        && let (Expr::Ref(a), Expr::Ref(b)) = (&lhs, &rhs)
        && b.sheet == SheetSel::Current
        && let (RefKind::Cell(x), RefKind::Cell(y)) = (&a.kind, &b.kind)
    {
        return Expr::Ref(Reference { sheet: a.sheet.clone(), kind: RefKind::Range(*x, *y) });
    }
    Expr::Binary(op, Box::new(lhs), Box::new(rhs))
}

/// Parses the inside of `Table[...]`. Text between the bracketed items may only be the `,` and
/// `:` separators, except one unbracketed column after `[#This Row],` (`[[#This Row],Units]`,
/// how some writers spell `[@Units]`); anything else is an error rather than dropped.
pub fn parse_struct(table: &str, body: &str) -> Result<StructRef, String> {
    let mut specifiers = Vec::new();
    let mut cols: Vec<String> = Vec::new();
    // Spaces at the edges of a column name are part of it (`Table1[Qty ]` names `Qty `), so the
    // untrimmed body is what a lone column is read from.
    let raw = body;
    let body = body.trim();
    let unescape = |s: &str| {
        let mut out = String::new();
        let mut it = s.chars();
        while let Some(c) = it.next() {
            if c == '\'' {
                if let Some(n) = it.next() {
                    out.push(n);
                }
            } else {
                out.push(c);
            }
        }
        out
    };
    let item = |s: &str| -> Option<StructItem> {
        match s.trim().to_ascii_lowercase().as_str() {
            "#all" => Some(StructItem::All),
            "#data" => Some(StructItem::Data),
            "#headers" => Some(StructItem::Headers),
            "#totals" => Some(StructItem::Totals),
            "#this row" => Some(StructItem::ThisRow),
            _ => None,
        }
    };
    let unexpected = |gap: &str| format!("unexpected `{gap}` in structured reference");
    // A column name may hold `[` and `]` only when escaped with `'`.
    let column = |s: &str| -> Result<String, String> {
        let mut esc = false;
        for c in s.chars() {
            match c {
                _ if esc => esc = false,
                '\'' => esc = true,
                '[' | ']' => return Err(unexpected(s)),
                _ => {}
            }
        }
        Ok(unescape(s))
    };
    if !body.starts_with('[') {
        // Simple: [Col], [@Col], [#Headers], [@]
        if let Some(rest) = body.strip_prefix('@') {
            specifiers.push(StructItem::ThisRow);
            let rest = rest.trim();
            let rest = rest.strip_prefix('[').and_then(|r| r.strip_suffix(']')).unwrap_or(rest);
            if !rest.is_empty() {
                cols.push(column(rest)?);
            }
        } else if let Some(i) = item(body) {
            specifiers.push(i);
        } else if !body.is_empty() {
            cols.push(column(raw)?);
        }
    } else {
        // [[#Headers],[Col1]:[Col3]], [[#This Row],[Col 1]] or [[#This Row],Col1]
        let mut parts = Vec::new();
        let mut depth = 0;
        let mut cur = String::new();
        // Text between bracketed items.
        let mut gap = String::new();
        let mut esc = false;
        for c in body.chars() {
            if esc {
                cur.push(c);
                esc = false;
                continue;
            }
            match c {
                '\'' if depth > 0 => {
                    cur.push(c);
                    esc = true;
                }
                '[' => {
                    if depth == 0 {
                        if !matches!(gap.trim(), "" | "," | ":") {
                            return Err(unexpected(gap.trim()));
                        }
                        gap.clear();
                    }
                    depth += 1;
                    if depth > 1 {
                        cur.push(c);
                    }
                }
                ']' => {
                    if depth == 0 {
                        return Err(unexpected("]"));
                    }
                    depth -= 1;
                    if depth > 0 {
                        cur.push(c);
                    } else {
                        parts.push(std::mem::take(&mut cur));
                    }
                }
                _ if depth > 0 => cur.push(c),
                _ => gap.push(c),
            }
        }
        if depth > 0 {
            return Err("unclosed `[` in structured reference".to_string());
        }
        for p in parts {
            if let Some(i) = item(&p) {
                specifiers.push(i);
            } else {
                cols.push(column(&p)?);
            }
        }
        let tail = gap.trim();
        if !tail.is_empty() {
            let bare = tail.strip_prefix(',').map(str::trim).unwrap_or("");
            let simple = !bare.is_empty() && !bare.chars().any(|c| matches!(c, '[' | ']' | '#' | '\'' | ',' | ':'));
            if !(simple && specifiers == [StructItem::ThisRow] && cols.is_empty()) {
                return Err(unexpected(tail));
            }
            cols.push(bare.to_string());
        }
    }
    if cols.len() > 2 {
        return Err("a structured reference names at most two columns".to_string());
    }
    let mut it = cols.into_iter();
    let col_start = it.next();
    let col_end = it.next();
    Ok(StructRef { table: table.to_string(), specifiers, col_start, col_end })
}
