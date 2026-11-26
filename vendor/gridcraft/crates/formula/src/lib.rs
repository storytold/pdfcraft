//! The Excel formula language: tokenizer, parser, syntax tree, printer (A1 and R1C1) and
//! reference adjustment for copy/fill and structural edits.
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]
#![forbid(unsafe_code)]

pub mod adjust;
pub mod ast;
mod input;
pub mod lexer;
pub mod parser;
pub mod printer;

pub use ast::{Anchor, BinOp, Expr, RefKind, Reference, SheetSel, StructItem, StructRef, TrimMode, UnOp};
pub use input::{FormulaLocale, parse_input, print_input};
pub use parser::{MAX_FORMULA_LEN, ParseError, normalize_function_name, parse, parse_closing_parens, parse_name};
pub use printer::{print, print_r1c1, quote_sheet};

/// Whether two LET/LAMBDA names are the same: case-insensitive in any script, as in Excel
/// (`dług` = `DŁUG`, `ß` = `ẞ`). Close to Unicode's default case folding: lower case, plus the
/// folds lower-casing misses for letters a name may hold (`ß`/`ẞ` -> `ss`, `ſ` -> `s`, `ς` -> `σ`).
/// Dotless `ı` stays distinct from `i`, as outside Turkic languages.
pub fn same_name(a: &str, b: &str) -> bool {
    if a.is_ascii() && b.is_ascii() {
        return a.eq_ignore_ascii_case(b);
    }
    a == b || fold_name(a) == fold_name(b)
}

/// The case-folded form of a name: two names are [`same_name`] when their folded forms are equal.
pub fn fold_name(s: &str) -> String {
    s.chars()
        .flat_map(char::to_lowercase)
        .flat_map(|c| match c {
            'ß' => ['s', 's'].into_iter().take(2),
            'ſ' => ['s', 's'].into_iter().take(1),
            'ς' => ['σ', 'σ'].into_iter().take(1),
            c => [c, c].into_iter().take(1),
        })
        .collect()
}

#[cfg(test)]
mod tests;
