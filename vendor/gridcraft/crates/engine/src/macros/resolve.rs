//! Macro tokens: references in step parameters that are worked out when the step runs, so a
//! macro made on one block of data works on another.
//!
//! A token is a base followed by methods:
//!
//! - bases: a declared input (`Input`, `Rates`), `Output` (the output rule's anchor), `Row` or
//!   `Column` (the current item of the Repeat step around it), and `Name:Total` (a defined name or
//!   a table's data; a name can hold periods, and the longest one that exists is used, so
//!   `Name:Sales.Q1.cell(1,1)` is a cell of `Sales.Q1`);
//! - methods, 1-based and chainable: `.col(n)` or `.col("Amount")` (by an input's header row,
//!   also after `.data`), `.row(n)`, `.cols(a..b)`, `.rows(a..)`, `.cell(r,c)` (relative to the
//!   top-left cell; any whole numbers, so `cell(0,1)` is the cell above), `.offset(r,c)`,
//!   `.resize(h,w)`, `.data` (without the header row), `.below(n)` and `.right(n)` (the n rows
//!   below or columns right of the range).
//!
//! In parameters a token is written three ways: `{"$ref": "Input.col(2)"}` anywhere; the whole
//! string `"Input.col(2)"` in a reference parameter (`cell`, `range`, `target`, `to`, keys ending
//! in `Cell` or `Range`…); or `{{Input.col(2)}}` inside any string (`"=SUM({{Input.col(2)}})"`).
//! Each becomes A1 text before the command runs, so commands never see a token. Text that would
//! read as a token is written `{"$text": "Output"}`: it is passed as it is, never resolved or
//! interpolated (a defined name called `Output`, or `Dear {{name}}`). Plain A1
//! references in steps are absolute: they don't follow row inserts (as in VBA); a `Name:` anchor
//! does.

use gridcraft_core::{CellRef, MAX_COLS, MAX_ROWS, RangeRef};
use gridcraft_model::{RepeatOf, Workbook};
use serde_json::{Map, Value as Json};

/// A resolved reference: a block of cells on a sheet.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Place {
    pub sheet: usize,
    pub range: RangeRef,
}

/// Why a reference couldn't be worked out.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ResolveError {
    /// It runs past the edge of the sheet.
    Edge(String),
    /// Anything else (an unknown input, a malformed token).
    Bad(String),
}

impl ResolveError {
    pub fn message(&self) -> &str {
        match self {
            ResolveError::Edge(m) | ResolveError::Bad(m) => m,
        }
    }
}

impl std::fmt::Display for ResolveError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.message())
    }
}

type R<T> = Result<T, ResolveError>;

fn bad<T>(msg: impl Into<String>) -> R<T> {
    Err(ResolveError::Bad(msg.into()))
}

/// What tokens refer to while a step runs.
#[derive(Clone, Copy, Debug)]
pub struct Env<'a> {
    pub wb: &'a Workbook,
    /// The sheet a reference without a sheet name is on.
    pub active_sheet: usize,
    /// Declared inputs, with whether each has a header row.
    pub inputs: &'a [(String, Place, bool)],
    pub output: Option<Place>,
    /// The innermost Repeat step's current row or column.
    pub item: Option<(RepeatOf, Place)>,
}

/// Parameter keys that hold references, where a whole-string token is resolved. Elsewhere a
/// string is text (a cell may hold "Row", a chart's type is "column"), and a token needs
/// `{"$ref": …}` or `{{…}}`.
const REF_KEYS: &[&str] = &[
    "active",
    "anchor",
    "area",
    "areas",
    "at",
    "cell",
    "cells",
    "changing",
    "copyTo",
    "destination",
    "from",
    "fromCell",
    "location",
    "objective",
    "over",
    "range",
    "ranges",
    "reference",
    "resultCells",
    "source",
    "sources",
    "target",
    "to",
    "variables",
];

/// Whether a parameter called `key` holds a reference.
pub fn is_ref_key(key: &str) -> bool {
    let k = key.to_ascii_lowercase();
    REF_KEYS.iter().any(|r| r.eq_ignore_ascii_case(key)) || ["cell", "cells", "range", "ranges", "ref"].iter().any(|end| k.ends_with(end))
}

/// Reference keys that other commands use for plain words: `to` is a replacement word in
/// Review › Change Spelling, `area` is `row` or `column` in a pivot field move. Under these a
/// bare `Row` or `Column` stays text; `Row.col(2)`, `{{Row}}` or `{"$ref": "Row"}` is the token.
fn is_general_key(key: &str) -> bool {
    ["area", "areas", "at", "destination", "from", "location", "source", "sources", "target", "to"].iter().any(|k| k.eq_ignore_ascii_case(key))
}

/// `Row` or `Column` on its own, with no method or sheet after it.
fn is_bare_item_word(text: &str) -> bool {
    let t = text.trim();
    t.eq_ignore_ascii_case("row") || t.eq_ignore_ascii_case("column")
}

/// The most nesting [`resolve_params`] walks into.
const MAX_JSON_DEPTH: usize = 64;

/// Whether `text` starts like a token (its base is one this environment knows). A reference on a
/// sheet with such a name (`Output!A1`) is not a token.
pub fn is_token(text: &str, env: &Env) -> bool {
    starts_like_token(text, |base| env.inputs.iter().any(|(n, ..)| n.eq_ignore_ascii_case(base)))
}

/// Whether `text` would read as a token in a macro whose inputs are `inputs` (see [`is_token`]).
pub fn reads_as_token(text: &str, inputs: &[&str]) -> bool {
    starts_like_token(text, |base| inputs.iter().any(|n| n.eq_ignore_ascii_case(base)))
}

fn starts_like_token(text: &str, is_input: impl Fn(&str) -> bool) -> bool {
    let t = text.trim();
    if t.get(..5).is_some_and(|h| h.eq_ignore_ascii_case("name:")) {
        return true;
    }
    let n: usize = t.chars().take_while(|c| c.is_alphanumeric() || *c == '_').map(char::len_utf8).sum();
    let base = t.get(..n).unwrap_or("");
    if t.get(n..).unwrap_or("").trim_start().starts_with('!') {
        return false;
    }
    is_builtin(base) || is_input(base)
}

fn is_builtin(base: &str) -> bool {
    ["Output", "Row", "Column"].iter().any(|b| b.eq_ignore_ascii_case(base))
}

/// Resolves a token, or a plain reference (`B2`, `Sheet2!A1:C3`, a defined name).
pub fn resolve_ref(text: &str, env: &Env) -> R<Place> {
    let t = text.trim();
    if is_token(t, env) {
        return resolve_token(t, env);
    }
    plain_ref(t, env)
}

/// `A1`, `'My Sheet'!A1:B2` or a defined name or table.
fn plain_ref(t: &str, env: &Env) -> R<Place> {
    let (sheet, body) = crate::cmd::split_sheet(t);
    let si = match &sheet {
        Some(name) => env.wb.sheet_index(name).ok_or_else(|| ResolveError::Bad(format!("There is no sheet named '{name}'.")))?,
        None => env.active_sheet,
    };
    if let Some(range) = RangeRef::parse(body) {
        return Ok(Place { sheet: si, range });
    }
    if sheet.is_none() {
        return named(t, env);
    }
    bad(format!("'{t}' isn't a reference."))
}

/// A defined name (by its formula) or a table (its data rows).
fn named(name: &str, env: &Env) -> R<Place> {
    if let Some(n) = env.wb.name(name, env.active_sheet) {
        // The sheet part keeps its characters (a sheet may be called `US$`); only the reference
        // loses its `$` anchors.
        let f = n.formula.trim().trim_start_matches('=');
        let (sheet, body) = crate::cmd::split_sheet(f);
        let si = match sheet {
            Some(s) => env.wb.sheet_index(&s),
            None => n.scope.or(Some(env.active_sheet)),
        };
        if let (Some(sheet), Some(range)) = (si, RangeRef::parse(&body.replace('$', ""))) {
            return Ok(Place { sheet, range });
        }
        return bad(format!("The name '{name}' doesn't refer to a range."));
    }
    if let Some((si, ti)) = env.wb.table(name)
        && let Some(t) = env.wb.sheet(si).and_then(|sh| sh.tables.get(ti))
    {
        // A table stands for its data rows; one with none (a header, perhaps a totals row) has
        // nothing to give.
        let Some(range) = t.data_range() else {
            return bad(format!("The table '{name}' has no data rows."));
        };
        return Ok(Place { sheet: si, range });
    }
    bad(format!("There is no name or table called '{name}'."))
}

/// Token method names.
const METHODS: &[&str] = &["col", "row", "cols", "rows", "cell", "offset", "resize", "data", "below", "right"];

/// Where the name ends in `run`, the text after `Name:` up to the first character a name can't
/// hold: at the end or at a period, the longest choice that is a defined name or table. When
/// none is, the name ends before the first method (`Nope.cell`), so the error names it.
fn name_end(run: &str, env: &Env) -> usize {
    // One pass over the names and tables, as a single lookup makes.
    let fits = |name: &str| {
        !name.is_empty()
            && run.get(..name.len()).is_some_and(|head| head.eq_ignore_ascii_case(name))
            && run.get(name.len()..).is_some_and(|rest| rest.is_empty() || rest.starts_with('.'))
    };
    let names = env.wb.names.iter().filter(|n| n.scope.is_none() || n.scope == Some(env.active_sheet)).map(|n| n.name.as_str());
    let tables = env.wb.sheets.iter().flat_map(|sh| sh.tables.iter().map(|t| t.name.as_str()));
    if let Some(end) = names.chain(tables).filter(|n| fits(n)).map(str::len).max() {
        return end;
    }
    run.match_indices('.')
        .map(|(i, _)| i)
        .find(|&i| {
            let next = run.get(i + 1..).unwrap_or("").split('.').next().unwrap_or("");
            METHODS.iter().any(|m| m.eq_ignore_ascii_case(next))
        })
        .unwrap_or(run.len())
}

/// Resolves a token (see the module docs).
pub fn resolve_token(text: &str, env: &Env) -> R<Place> {
    let t = text.trim();
    let mut p = Parser { s: t, at: 0 };
    let (mut place, mut header) = p.base(env)?;
    // The row of column names `col("…")` reads: the input's header row, kept through `data`.
    let mut names_row = header.then_some(place.range.start.row);
    while p.eat('.') {
        let name = p.ident();
        if name.is_empty() {
            return bad(format!("'{t}': expected a method name after '.'."));
        }
        let args = if p.eat('(') { p.args(t)? } else { vec![] };
        let r = place.range;
        let method = name.to_ascii_lowercase();
        let range = match (method.as_str(), args.as_slice()) {
            ("col", [Arg::Int(n)]) => sub(r, None, Some((*n, *n)), t)?,
            ("col", [Arg::Text(h)]) => {
                let Some(row) = names_row else {
                    return bad(format!("'{t}': col(\"{h}\") needs an input with a header row; use col(n)."));
                };
                let c = header_col(env, place, row, h).ok_or_else(|| ResolveError::Bad(format!("'{t}': no column is headed '{h}'.")))?;
                sub(r, None, Some((c, c)), t)?
            }
            ("row", [Arg::Int(n)]) => sub(r, Some((*n, *n)), None, t)?,
            ("cols", [Arg::Span(a, b)]) => sub(r, None, Some(span(*a, *b, i64::from(r.width()))), t)?,
            ("rows", [Arg::Span(a, b)]) => sub(r, Some(span(*a, *b, i64::from(r.height()))), None, t)?,
            ("cell", [Arg::Int(row), Arg::Int(col)]) => {
                let c = shift(r.start, row - 1, col - 1, t)?;
                RangeRef::cell(c)
            }
            ("offset", [Arg::Int(dr), Arg::Int(dc)]) => RangeRef::new(shift(r.start, *dr, *dc, t)?, shift(r.end, *dr, *dc, t)?),
            ("resize", [Arg::Int(h), Arg::Int(w)]) => {
                if *h < 1 || *w < 1 {
                    return bad(format!("'{t}': resize needs at least 1 row and 1 column."));
                }
                RangeRef::new(r.start, shift(r.start, h - 1, w - 1, t)?)
            }
            ("data", []) => {
                if header {
                    // A header row with nothing under it has no data rows to give.
                    if r.height() < 2 {
                        return bad(format!("'{t}': the range is only a header row, with no data under it."));
                    }
                    RangeRef::new(CellRef::new(r.start.row + 1, r.start.col), r.end)
                } else {
                    r
                }
            }
            ("below", []) | ("below", [Arg::Int(_)]) => {
                let n = count_arg(&args, t)?;
                let top = shift(CellRef::new(r.end.row, r.start.col), 1, 0, t)?;
                RangeRef::new(top, shift(CellRef::new(r.end.row, r.end.col), n, 0, t)?)
            }
            ("right", []) | ("right", [Arg::Int(_)]) => {
                let n = count_arg(&args, t)?;
                let left = shift(CellRef::new(r.start.row, r.end.col), 0, 1, t)?;
                RangeRef::new(left, shift(r.end, 0, n, t)?)
            }
            _ => {
                return bad(format!("'{t}': '{name}' with these arguments isn't a token method ({}).", METHODS.join(", ")));
            }
        };
        // After a method the result is plain cells, except that `col` and `cols` keep the input's top
        // row, so a later `data` still takes the header off; only `data` keeps the header row for
        // `col("…")`.
        if !matches!(method.as_str(), "col" | "cols") {
            header = false;
        }
        if method != "data" {
            names_row = None;
        }
        place = Place { sheet: place.sheet, range };
    }
    p.skip_ws();
    if p.at < p.s.len() {
        return bad(format!("'{t}' isn't a token: unexpected '{}'.", p.s.get(p.at..).unwrap_or("")));
    }
    Ok(place)
}

/// The count `below(n)` and `right(n)` take: 1 when left out, and never less than 1.
fn count_arg(args: &[Arg], t: &str) -> R<i64> {
    match args.first() {
        Some(Arg::Int(n)) if *n < 1 => bad(format!("'{t}': the count must be at least 1.")),
        Some(Arg::Int(n)) => Ok(*n),
        _ => Ok(1),
    }
}

/// `a..b` with open ends filled from 1 and `len`.
fn span(a: Option<i64>, b: Option<i64>, len: i64) -> (i64, i64) {
    (a.unwrap_or(1), b.unwrap_or(len))
}

/// Rows `rows` and columns `cols` (1-based, inclusive) of `r`; `None` keeps them all.
fn sub(r: RangeRef, rows: Option<(i64, i64)>, cols: Option<(i64, i64)>, t: &str) -> R<RangeRef> {
    let (h, w) = (i64::from(r.height()), i64::from(r.width()));
    let (r0, r1) = rows.unwrap_or((1, h));
    let (c0, c1) = cols.unwrap_or((1, w));
    if r0 < 1 || c0 < 1 || r1 < r0 || c1 < c0 || r1 > h || c1 > w {
        return bad(format!("'{t}' is outside the range it starts from ({} rows by {} columns).", h, w));
    }
    Ok(RangeRef::new(shift(r.start, r0 - 1, c0 - 1, t)?, shift(r.start, r1 - 1, c1 - 1, t)?))
}

fn shift(c: CellRef, dr: i64, dc: i64, t: &str) -> R<CellRef> {
    let row = i64::from(c.row).checked_add(dr);
    let col = i64::from(c.col).checked_add(dc);
    match (row, col) {
        (Some(row), Some(col)) if (0..i64::from(MAX_ROWS)).contains(&row) && (0..i64::from(MAX_COLS)).contains(&col) => {
            Ok(CellRef::new(row as u32, col as u32))
        }
        (Some(row), Some(_)) if row < 0 => Err(ResolveError::Edge(format!("'{t}' runs past the top of the sheet."))),
        (Some(row), Some(_)) if row >= i64::from(MAX_ROWS) => Err(ResolveError::Edge(format!("'{t}' runs past the last row of the sheet."))),
        _ => Err(ResolveError::Edge(format!("'{t}' runs past the edge of the sheet."))),
    }
}

/// The 1-based column of `place` whose header (in sheet row `row`) reads `h`.
fn header_col(env: &Env, place: Place, row: u32, h: &str) -> Option<i64> {
    let sh = env.wb.sheet(place.sheet)?;
    let r = place.range;
    (r.start.col..=r.end.col)
        // The header's value, however the sheet shows it: with Show Formulas on, the cell's
        // display text is its formula.
        .position(|c| crate::display::value_text(env.wb, sh, CellRef::new(row, c)).trim().eq_ignore_ascii_case(h.trim()))
        .map(|i| i as i64 + 1)
}

#[derive(Debug)]
enum Arg {
    Int(i64),
    Span(Option<i64>, Option<i64>),
    Text(String),
}

struct Parser<'a> {
    s: &'a str,
    at: usize,
}

impl Parser<'_> {
    fn rest(&self) -> &str {
        self.s.get(self.at..).unwrap_or("")
    }
    fn skip_ws(&mut self) {
        let n = self.rest().len() - self.rest().trim_start().len();
        self.at += n;
    }
    fn eat(&mut self, c: char) -> bool {
        self.skip_ws();
        if self.rest().starts_with(c) {
            self.at += c.len_utf8();
            true
        } else {
            false
        }
    }
    fn ident(&mut self) -> String {
        self.skip_ws();
        let n: usize = self.rest().chars().take_while(|c| c.is_alphanumeric() || *c == '_').map(char::len_utf8).sum();
        let out = self.rest().get(..n).unwrap_or("").to_string();
        self.at += n;
        out
    }
    fn int(&mut self) -> Option<i64> {
        self.skip_ws();
        let neg = self.rest().starts_with('-');
        let start = self.at + usize::from(neg);
        let digits: usize = self.s.get(start..).unwrap_or("").chars().take_while(char::is_ascii_digit).count();
        if digits == 0 || digits > 9 {
            return None;
        }
        let n: i64 = self.s.get(start..start + digits)?.parse().ok()?;
        self.at = start + digits;
        Some(if neg { -n } else { n })
    }
    /// The base of a token and whether it has a header row.
    fn base(&mut self, env: &Env) -> R<(Place, bool)> {
        self.skip_ws();
        if self.rest().get(..5).is_some_and(|h| h.eq_ignore_ascii_case("name:")) {
            self.at += 5;
            // A name can hold periods (`Sales.Q1`), so it is the longest part of the run, up to
            // a period or the end, that names something; methods follow it.
            let n: usize = self.rest().chars().take_while(|c| c.is_alphanumeric() || matches!(c, '_' | '\\' | '.')).map(char::len_utf8).sum();
            let run = self.rest().get(..n).unwrap_or("").to_string();
            let end = name_end(&run, env);
            let name = run.get(..end).unwrap_or("");
            if name.is_empty() {
                return bad("'Name:' needs a defined name after it.");
            }
            self.at += end;
            return Ok((named(name, env)?, false));
        }
        let base = self.ident();
        if base.eq_ignore_ascii_case("Output") {
            return env.output.map(|p| (p, false)).ok_or_else(|| ResolveError::Bad("'Output' is used, but the macro has no output anchor.".into()));
        }
        if base.eq_ignore_ascii_case("Row") || base.eq_ignore_ascii_case("Column") {
            return match env.item {
                Some((_, p)) => Ok((p, false)),
                None => bad(format!("'{base}' only means something inside a Repeat step.")),
            };
        }
        match env.inputs.iter().find(|(n, ..)| n.eq_ignore_ascii_case(&base)) {
            Some((_, p, header)) => Ok((*p, *header)),
            None => bad(format!("'{base}' isn't an input of this macro.")),
        }
    }
    fn args(&mut self, t: &str) -> R<Vec<Arg>> {
        let mut out = Vec::new();
        if self.eat(')') {
            return Ok(out);
        }
        loop {
            self.skip_ws();
            if self.rest().starts_with('"') {
                self.at += 1;
                let n = self.rest().find('"').ok_or_else(|| ResolveError::Bad(format!("'{t}': a quote isn't closed.")))?;
                out.push(Arg::Text(self.rest().get(..n).unwrap_or("").to_string()));
                self.at += n + 1;
            } else {
                let a = self.int();
                self.skip_ws();
                if self.rest().starts_with("..") {
                    self.at += 2;
                    let b = self.int();
                    out.push(Arg::Span(a, b));
                } else {
                    out.push(Arg::Int(a.ok_or_else(|| ResolveError::Bad(format!("'{t}': expected a whole number.")))?));
                }
            }
            if out.len() > 4 {
                return bad(format!("'{t}': too many arguments."));
            }
            if self.eat(')') {
                return Ok(out);
            }
            if !self.eat(',') {
                return bad(format!("'{t}': expected ',' or ')'."));
            }
        }
    }
}

/// `place` as A1 text, with the sheet name when it isn't the active sheet.
pub fn a1(place: Place, env: &Env) -> String {
    if place.sheet == env.active_sheet {
        return place.range.a1();
    }
    let name = env.wb.sheet(place.sheet).map(|s| s.name.as_str()).unwrap_or("");
    format!("{}!{}", quote_sheet(name), place.range.a1())
}

/// `Sheet1` or `'My Sheet'` (quotes doubled).
pub fn quote_sheet(name: &str) -> String {
    // The engine's one rule: a name that reads as a cell reference, an R1C1 token or a boolean
    // is quoted too, or `Q1!A1` would be read as a reference to cell Q1.
    gridcraft_core::quote_sheet(name)
}

/// Step parameters with every token replaced by A1 text.
pub fn resolve_params(params: &Json, env: &Env) -> R<Json> {
    walk(params, None, env, 0)
}

fn walk(v: &Json, key: Option<&str>, env: &Env, depth: usize) -> R<Json> {
    if depth > MAX_JSON_DEPTH {
        return bad("The step's parameters nest too deeply.");
    }
    Ok(match v {
        Json::Object(o) => {
            if o.len() == 1
                && let Some(Json::String(t)) = o.get("$ref")
            {
                return Ok(Json::String(a1(resolve_ref(t, env)?, env)));
            }
            // Text passed as it is: never a token, and no `{{…}}` in it is read.
            if o.len() == 1
                && let Some(Json::String(t)) = o.get("$text")
            {
                return Ok(Json::String(t.clone()));
            }
            let mut out = Map::new();
            for (k, x) in o {
                out.insert(k.clone(), walk(x, Some(k), env, depth + 1)?);
            }
            Json::Object(out)
        }
        // Items of a list keep their key's kind (`ranges: ["Input", …]`).
        Json::Array(a) => Json::Array(a.iter().map(|x| walk(x, key, env, depth + 1)).collect::<R<Vec<_>>>()?),
        Json::String(s) => {
            if key.is_some_and(|k| is_ref_key(k) && !(is_general_key(k) && is_bare_item_word(s))) && is_token(s, env) {
                Json::String(a1(resolve_token(s, env)?, env))
            } else {
                Json::String(interpolate(s, env)?)
            }
        }
        other => other.clone(),
    })
}

/// Replaces each `{{token}}` in `s`. `{{{{` is a literal `{{`, for text that needs the braces
/// themselves (`"Dear {{{{name}}"` writes `Dear {{name}}`).
pub fn interpolate(s: &str, env: &Env) -> R<String> {
    if !s.contains("{{") {
        return Ok(s.to_string());
    }
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(i) = rest.find("{{") {
        out.push_str(rest.get(..i).unwrap_or(""));
        if rest.get(i..i + 4) == Some("{{{{") {
            out.push_str("{{");
            rest = rest.get(i + 4..).unwrap_or("");
            continue;
        }
        let after = rest.get(i + 2..).unwrap_or("");
        let Some(j) = after.find("}}") else {
            out.push_str(rest.get(i..).unwrap_or(""));
            return Ok(out);
        };
        let token = after.get(..j).unwrap_or("");
        out.push_str(&a1(resolve_ref(token, env)?, env));
        rest = after.get(j + 2..).unwrap_or("");
    }
    out.push_str(rest);
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn wb() -> Workbook {
        let mut wb = Workbook::new();
        wb.sheets.push(std::sync::Arc::new(gridcraft_model::Sheet::new("My Data")));
        let sh = wb.sheet_mut(0).unwrap();
        for (i, h) in ["Name", "Amount", "Date"].iter().enumerate() {
            sh.set_value(CellRef::new(1, 1 + i as u32), gridcraft_core::Value::text(*h));
        }
        wb.names.push(gridcraft_model::DefinedName {
            name: "Total".into(),
            scope: None,
            formula: "Sheet1!$F$20".into(),
            comment: String::new(),
            hidden: false,
        });
        wb.names.push(gridcraft_model::DefinedName {
            name: "Rates".into(),
            scope: None,
            formula: "'My Data'!$A$1:$A$5".into(),
            comment: String::new(),
            hidden: false,
        });
        wb
    }

    fn place(a1: &str) -> Place {
        Place { sheet: 0, range: RangeRef::parse(a1).unwrap() }
    }

    fn check(env: &Env, token: &str, want: &str) {
        match resolve_ref(token, env) {
            Ok(p) => assert_eq!(a1(p, env), want, "{token}"),
            Err(e) => panic!("{token}: {e}"),
        }
    }

    #[test]
    fn a_formula_header_matches_while_formulas_are_shown() {
        // The header was read through the cell's display text, which is the formula while Show
        // Formulas is on, so `col("Amount")` found no column headed by a formula.
        let mut wb = wb();
        let sh = wb.sheet_mut(0).unwrap();
        let formula = Some(std::sync::Arc::new(gridcraft_model::Formula::new("\"Amo\"&\"unt\"")));
        sh.set_cell(
            CellRef::new(1, 2),
            gridcraft_model::Cell { value: gridcraft_core::Value::text("Amount"), formula, style: Default::default(), py: None },
        );
        sh.show_formulas = true;
        let inputs = vec![("Input".to_string(), place("B2:D11"), true)];
        let env = Env { wb: &wb, active_sheet: 0, inputs: &inputs, output: None, item: None };
        check(&env, "Input.col(\"Amount\")", "C2:C11");
    }

    #[test]
    fn every_token_resolves() {
        let wb = wb();
        // Input B2:D11 with a header row; Other on another sheet.
        let inputs = vec![
            ("Input".to_string(), place("B2:D11"), true),
            ("Other".to_string(), Place { sheet: 1, range: RangeRef::parse("C3:C4").unwrap() }, false),
        ];
        let env = Env { wb: &wb, active_sheet: 0, inputs: &inputs, output: Some(place("G2")), item: Some((RepeatOf::Rows, place("B5:D5"))) };
        for (token, want) in [
            ("Input", "B2:D11"),
            ("input", "B2:D11"),
            ("Input.col(2)", "C2:C11"),
            ("Input.col(\"Amount\")", "C2:C11"),
            ("Input.col(\"date\").data", "D3:D11"),
            ("Input.cols(2..3).data", "C3:D11"),
            ("Input.col(2).data.col(1)", "C3:C11"),
            ("Input.data", "B3:D11"),
            ("Input.data.col(2)", "C3:C11"),
            ("Input.data.col(\"Amount\")", "C3:C11"),
            ("Input.row(1)", "B2:D2"),
            ("Input.rows(2..)", "B3:D11"),
            ("Input.rows(2..4)", "B3:D5"),
            ("Input.rows(..2)", "B2:D3"),
            ("Input.cols(2..3)", "C2:D11"),
            ("Input.cell(1,1)", "B2"),
            ("Input.cell(3,2)", "C4"),
            ("Input.cell(0,1)", "B1"),
            ("Input.cell(12,3)", "D13"),
            ("Input.offset(0,4)", "F2:H11"),
            ("Input.offset(-1,-1)", "A1:C10"),
            ("Input.cell(1,1).resize(2,3)", "B2:D3"),
            ("Input.below()", "B12:D12"),
            ("Input.below(2)", "B12:D13"),
            ("Input.col(2).below", "C12"),
            ("Input.right", "E2:E11"),
            (" Input . col( 2 ) ", "C2:C11"),
            ("Output", "G2"),
            ("Output.offset(1,0)", "G3"),
            ("Row", "B5:D5"),
            ("Row.cell(1,2)", "C5"),
            ("Column", "B5:D5"),
            ("Name:Total", "F20"),
            ("Name:Rates", "'My Data'!A1:A5"),
            ("Name:Rates.cell(2,1)", "'My Data'!A2"),
            ("Other", "'My Data'!C3:C4"),
            ("B7", "B7"),
            ("'My Data'!A1:B2", "'My Data'!A1:B2"),
            ("Total", "F20"),
        ] {
            check(&env, token, want);
        }
    }

    #[test]
    fn bad_tokens_say_why() {
        let wb = wb();
        // The same cells, read with and without a header row.
        let inputs = vec![("Input".to_string(), place("B2:D11"), false), ("Headed".to_string(), place("B2:D11"), true)];
        let env = Env { wb: &wb, active_sheet: 0, inputs: &inputs, output: None, item: None };
        for (token, needle, edge) in [
            ("Input.col(4)", "outside the range", false),
            ("Input.col(0)", "outside the range", false),
            ("Input.rows(5..2)", "outside the range", false),
            ("Headed.col(\"Nope\")", "no column is headed", false),
            ("Input.col(\"Name\")", "needs an input with a header row", false),
            ("Headed.col(2).col(\"Amount\")", "needs an input with a header row", false),
            ("Name:Rates.col(\"Name\")", "needs an input with a header row", false),
            ("Input.colour(1)", "isn't a token method", false),
            ("Input.col(1", "expected", false),
            ("Input.col(x)", "whole number", false),
            ("Input.col(99999999999)", "whole number", false),
            ("Input.", "method name", false),
            ("Input junk", "unexpected", false),
            ("Input.resize(0,1)", "at least 1 row", false),
            ("Output", "no output anchor", false),
            ("Row", "inside a Repeat", false),
            ("Name:", "needs a defined name", false),
            ("Name:Nothing", "no name or table", false),
            ("Nowhere!A1", "no sheet named", false),
            ("Input.offset(-2,0)", "past the top", true),
            ("Input.offset(0,-2)", "edge of the sheet", true),
            ("Input.offset(1048570,0)", "past the last row", true),
            ("Input.cell(1,16384)", "edge of the sheet", true),
        ] {
            let e = resolve_ref(token, &env).unwrap_err();
            assert!(e.message().contains(needle), "{token}: {e}");
            assert_eq!(matches!(e, ResolveError::Edge(_)), edge, "{token}");
        }
    }

    #[test]
    fn params_resolve_tokens_in_reference_keys_and_braces_anywhere() {
        let wb = wb();
        let inputs = vec![("Input".to_string(), place("B2:D11"), true)];
        let env = Env { wb: &wb, active_sheet: 0, inputs: &inputs, output: Some(place("F2")), item: None };
        let p = json!({
            "cell": "Output",
            "range": "Input.col(2)",
            "ranges": ["Input.row(1)", "A1"],
            "input": "Row",
            "value": "Input",
            "formula": "=SUM({{Input.data.col(2)}})/{{Name:Total}}",
            "nested": {"target": {"$ref": "Input.cell(1,1)"}, "n": 3, "flag": true},
            "text": {"$ref": "Output"},
            "type": "column",
            "sheet": "Input",
            "rowInputCell": "Input.cell(1,1)",
            "unclosed": "a {{b"
        });
        let r = resolve_params(&p, &env).unwrap();
        assert_eq!(
            r,
            json!({
                "cell": "F2",
                "range": "C2:C11",
                "ranges": ["B2:D2", "A1"],
                "input": "Row",
                "value": "Input",
                "formula": "=SUM(C3:C11)/F20",
                "nested": {"target": "B2", "n": 3, "flag": true},
                "text": "F2",
                "type": "column",
                "sheet": "Input",
                "rowInputCell": "B2",
                "unclosed": "a {{b"
            })
        );
        // Deeply nested parameters are refused, not followed.
        let mut deep = json!(1);
        for _ in 0..100 {
            deep = json!([deep]);
        }
        assert!(resolve_params(&deep, &env).is_err());
        assert!(resolve_params(&json!({"cell": "Input.col(9)"}), &env).is_err());
    }

    #[test]
    fn references_on_sheets_named_like_tokens_stay_references() {
        // Sheets named after the built-in bases and after an input: `Output!A1` is a cell on the
        // sheet Output, in a reference key, in `$ref` and in `{{…}}`.
        let mut wb = Workbook::new();
        for name in ["Output", "Row", "Column", "Input", "Rates"] {
            wb.sheets.push(std::sync::Arc::new(gridcraft_model::Sheet::new(name)));
        }
        let inputs = vec![("Input".to_string(), place("B2:D11"), true), ("Rates".to_string(), place("F2:F5"), false)];
        for output in [None, Some(place("H2"))] {
            let env = Env { wb: &wb, active_sheet: 0, inputs: &inputs, output, item: Some((RepeatOf::Rows, place("B5:D5"))) };
            for (sheet, i) in [("Output", 1), ("Row", 2), ("Column", 3), ("Input", 4), ("Rates", 5)] {
                for text in [format!("{sheet}!A1:B2"), format!("'{sheet}'!A1:B2")] {
                    let want = Place { sheet: i, range: RangeRef::parse("A1:B2").unwrap() };
                    assert_eq!(resolve_ref(&text, &env), Ok(want), "{text}");
                    let a = a1(want, &env);
                    let p = json!({"range": text, "target": {"$ref": text}, "formula": format!("=SUM({{{{{text}}}}})")});
                    let r = resolve_params(&p, &env).unwrap();
                    // A plain reference in a reference key is passed on as written.
                    assert_eq!(r, json!({"range": text, "target": a, "formula": format!("=SUM({a})")}), "{text}");
                }
            }
            assert!(!is_token("Output !A1", &env));
            // The bare names are still tokens.
            check(&env, "Input.col(2)", "C2:C11");
            check(&env, "Rates", "F2:F5");
            check(&env, "Row", "B5:D5");
        }
    }

    #[test]
    fn defined_names_can_hold_periods() {
        // `Sales.Q1` is one defined name, beside `Sales`; `Data.row` has a segment that reads like
        // a method. The longest name that exists is used, and methods follow it.
        let mut wb = wb();
        for (name, formula) in [("Sales", "Sheet1!$A$1"), ("Sales.Q1", "Sheet1!$B$2:$C$5"), ("Data.row", "'My Data'!$E$1:$E$3")] {
            wb.names.push(gridcraft_model::DefinedName {
                name: name.into(),
                scope: None,
                formula: formula.into(),
                comment: String::new(),
                hidden: false,
            });
        }
        let env = Env { wb: &wb, active_sheet: 0, inputs: &[], output: None, item: None };
        for (token, want) in [
            ("Name:Sales.Q1", "B2:C5"),
            ("Name:sales.q1", "B2:C5"),
            ("Name:Sales.Q1.cell(1,1)", "B2"),
            ("Name:Sales.Q1.col(2).row(4)", "C5"),
            ("Name:Sales", "A1"),
            ("Name:Sales.offset(1,0)", "A2"),
            ("Name:Data.row", "'My Data'!E1:E3"),
            ("Name:Data.row.row(2)", "'My Data'!E2"),
            ("Name:Data.row.cell(1,1)", "'My Data'!E1"),
            ("Sales.Q1", "B2:C5"),
        ] {
            check(&env, token, want);
        }
        // In a reference parameter, `$ref` and `{{…}}`, with and without a method.
        let p = json!({
            "range": "Name:Sales.Q1",
            "cell": "Name:Sales.Q1.cell(1,1)",
            "a": {"$ref": "Name:Sales.Q1"},
            "b": {"$ref": "Name:Sales.Q1.cell(2,2)"},
            "formula": "=SUM({{Name:Sales.Q1}})+{{Name:Sales.Q1.cell(1,2)}}"
        });
        let r = resolve_params(&p, &env).unwrap();
        assert_eq!(r, json!({"range": "B2:C5", "cell": "B2", "a": "B2:C5", "b": "C3", "formula": "=SUM(B2:C5)+C2"}));
        // Unknown names and bad methods still fail, naming what is wrong.
        for (token, needle) in [
            ("Name:Nope.Q1", "no name or table called 'Nope.Q1'"),
            ("Name:Nope.cell(1,1)", "no name or table called 'Nope'"),
            ("Name:Sales.Q1.bogus(1)", "'bogus' with these arguments isn't a token method"),
            ("Name:Sales.Q1.cell(1", "expected"),
            ("Name:Sales.Q1.", "method name"),
            ("Name:.cell(1,1)", "needs a defined name"),
        ] {
            let e = resolve_ref(token, &env).unwrap_err();
            assert!(e.message().contains(needle), "{token}: {e}");
            let e = resolve_params(&json!({"formula": format!("={{{{{token}}}}}")}), &env).unwrap_err();
            assert!(e.message().contains(needle), "{token} in braces: {e}");
        }
    }

    #[test]
    fn sheet_names_are_quoted_when_needed() {
        // The engine's rule: a name that reads as a cell reference, an R1C1 token or a boolean
        // is quoted too, and a period on its own is not a reason to quote.
        for (name, want) in [
            ("Sheet1", "Sheet1"),
            ("My Data", "'My Data'"),
            ("O'Hara", "'O''Hara'"),
            ("2024", "'2024'"),
            ("", "''"),
            ("Q1", "'Q1'"),
            ("R1C1", "'R1C1'"),
            ("TRUE", "'TRUE'"),
            ("Sales.Q1", "Sales.Q1"),
        ] {
            assert_eq!(quote_sheet(name), want, "{name}");
        }
    }

    #[test]
    fn a_bare_row_or_column_under_a_general_key_is_text() {
        // `to` is a replacement word in Change Spelling and `area` is `row` or `column` in a
        // pivot field move, so a bare `Row` or `Column` there stays text, inside a Repeat or not;
        // the token forms still resolve there, and the bare word resolves under `range`.
        let wb = wb();
        let inputs = vec![("Input".to_string(), place("B2:D11"), true)];
        for item in [None, Some((RepeatOf::Rows, place("B5:D5")))] {
            let env = Env { wb: &wb, active_sheet: 0, inputs: &inputs, output: Some(place("F2")), item };
            let p = json!({"word": "Colum", "to": "Column", "area": "row", "from": "Input", "at": "Output"});
            let r = resolve_params(&p, &env).unwrap();
            assert_eq!(r, json!({"word": "Colum", "to": "Column", "area": "row", "from": "B2:D11", "at": "F2"}), "{item:?}");
        }
        let env = Env { wb: &wb, active_sheet: 0, inputs: &inputs, output: None, item: Some((RepeatOf::Rows, place("B5:D5"))) };
        let p = json!({"to": "Row.col(2)", "range": "Row", "area": {"$ref": "Row"}, "text": "{{Row}}"});
        let r = resolve_params(&p, &env).unwrap();
        assert_eq!(r, json!({"to": "C5", "range": "B5:D5", "area": "B5:D5", "text": "B5:D5"}));
    }

    #[test]
    fn a_header_row_alone_and_a_table_without_data_rows_are_refused() {
        let mut wb = wb();
        let sh = wb.sheet_mut(0).unwrap();
        sh.tables.push(gridcraft_model::Table {
            id: 1,
            name: "Empty".into(),
            range: RangeRef::parse("H1:J1").unwrap(),
            header_row: true,
            totals_row: false,
            columns: Vec::new(),
            style: String::new(),
            banded_rows: false,
            banded_cols: false,
            first_col: false,
            last_col: false,
            filter_button: false,
            filter: Vec::new(),
            table_type: None,
            connection_id: None,
        });
        let inputs = vec![("Input".to_string(), place("B2:D2"), true), ("Plain".to_string(), place("B2:D2"), false)];
        let env = Env { wb: &wb, active_sheet: 0, inputs: &inputs, output: None, item: None };
        assert!(resolve_ref("Input.data", &env).unwrap_err().to_string().contains("only a header row"));
        assert!(resolve_ref("Input.data.col(2)", &env).unwrap_err().to_string().contains("only a header row"));
        check(&env, "Plain.data", "B2:D2");
        check(&env, "Input.col(2)", "C2");
        assert!(resolve_ref("Name:Empty", &env).unwrap_err().to_string().contains("no data rows"));
    }

    #[test]
    fn doubled_braces_are_literal_braces() {
        let wb = wb();
        let inputs = vec![("Input".to_string(), place("B2:D11"), true)];
        let env = Env { wb: &wb, active_sheet: 0, inputs: &inputs, output: None, item: None };
        assert_eq!(interpolate("Dear {{{{name}},", &env).unwrap(), "Dear {{name}},");
        assert_eq!(interpolate("{{{{ {{Input.cell(1,1)}} }}", &env).unwrap(), "{{ B2 }}");
        assert!(interpolate("Dear {{name}},", &env).is_err(), "an unknown token is still an error");
    }

    #[test]
    fn text_marked_as_text_is_never_a_token() {
        // A defined name called Output or Input in a reference parameter, a Name: prefix and
        // braces: `$text` passes each as it is, in any key; unmarked, each reads as a token.
        let wb = wb();
        let inputs = vec![("Input".to_string(), place("B2:D11"), true)];
        let env = Env { wb: &wb, active_sheet: 0, inputs: &inputs, output: None, item: None };
        for text in ["Output", "Input", "Name:Nowhere", "Row", "Dear {{name}}"] {
            let p = json!({"range": {"$text": text}, "input": {"$text": text}});
            assert_eq!(resolve_params(&p, &env).unwrap(), json!({"range": text, "input": text}), "{text}");
            assert_ne!(resolve_params(&json!({"range": text}), &env).ok(), Some(json!({"range": text})), "{text}");
            assert!(reads_as_token(text, &["Input"]) || text.contains("{{"), "{text}");
        }
        assert!(!reads_as_token("Output!A1", &["Input"]) && !reads_as_token("B2", &["Input"]));
    }

    #[test]
    fn a_defined_name_on_a_sheet_with_a_dollar_sign_resolves() {
        let mut wb = wb();
        wb.sheets.push(std::sync::Arc::new(gridcraft_model::Sheet::new("US$")));
        wb.names.push(gridcraft_model::DefinedName {
            name: "Dollars".into(),
            scope: None,
            formula: "'US$'!$F$20".into(),
            comment: String::new(),
            hidden: false,
        });
        let env = Env { wb: &wb, active_sheet: 0, inputs: &[], output: None, item: None };
        check(&env, "Name:Dollars", "'US$'!F20");
    }

    #[test]
    fn below_and_right_refuse_a_count_under_one() {
        let wb = wb();
        let inputs = vec![("Input".to_string(), place("B2:D11"), true)];
        let env = Env { wb: &wb, active_sheet: 0, inputs: &inputs, output: None, item: None };
        check(&env, "Input.below", "B12:D12");
        check(&env, "Input.below(2)", "B12:D13");
        check(&env, "Input.right(1)", "E2:E11");
        for bad in ["Input.below(0)", "Input.below(-3)", "Input.right(0)"] {
            assert!(resolve_ref(bad, &env).unwrap_err().to_string().contains("at least 1"), "{bad}");
        }
    }
}
