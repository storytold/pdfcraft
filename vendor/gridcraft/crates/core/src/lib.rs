//! GridCraft core types: cell addresses, values and Excel's coercion rules, error values,
//! date serials, regional formats and typed-input parsing. No dependencies on the rest of the workspace.
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]
#![forbid(unsafe_code)]

pub mod addr;
pub mod color;
pub mod date;
pub mod parse;
pub mod region;
pub mod value;

pub use addr::{
    CategoryLevels, CellRef, MAX_COLS, MAX_ROWS, RangeRef, category_levels, col_to_letters, letters_to_col, quote_sheet, sheet_needs_quotes,
    split_areas, split_sheet, unquote_sheet,
};
pub use date::DateSystem;
pub use region::Region;
pub use value::{Array, CellError, Value, compare, compare_numbers, compare_text, number_to_text, sort_compare};
