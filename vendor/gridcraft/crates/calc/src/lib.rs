//! GridCraft's calculation engine: the evaluator (references, names, tables, LET/LAMBDA,
//! dynamic arrays) and dependency-driven recalculation with spills and cycle detection.
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]
#![forbid(unsafe_code)]

pub mod eval;
mod pivotdata;
mod precision;
pub mod recalc;
mod spill_index;
mod trimrange;

pub use eval::{Area, SPECIAL_FUNCTIONS, is_known_function};
pub use precision::DisplayedPrecision;
pub use recalc::{Bounded, Calc, Key, evaluate, evaluate_bounded, now_serial};

#[cfg(test)]
mod tests;
