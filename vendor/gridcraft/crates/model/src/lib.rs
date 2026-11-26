//! The GridCraft document model: workbooks, sheets, a copy-on-write cell store, interned
//! styles, defined names, tables, conditional formats, validation, comments, charts and print
//! settings. Pure data; calculation lives in `gridcraft-calc`.
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]
#![forbid(unsafe_code)]

mod b64;
pub mod cell;
mod cellmap;
pub mod features;
pub mod macros;
pub mod password;
pub mod preserved;
pub mod python;
pub mod sheet;
pub mod store;
pub mod style;
pub mod workbook;

pub use cell::{Cell, CellPicture, Formula};
pub use features::*;
pub use macros::*;
pub use password::PasswordHash;
pub use preserved::{Preserved, PreservedPart, PreservedRel, PreservedSheet, PreservedXml};
pub use python::{PyCode, PyOutput};
pub use sheet::{DEFAULT_COL_WIDTH, DEFAULT_ROW_HEIGHT, LineIndex, Sheet, SheetUid, SheetViewMode, Visibility, points_to_chars};
pub use store::CellStore;
pub use style::{
    Alignment, BorderLine, BorderStyle, Borders, Color, Fill, Font, HAlign, NumFmt, PatternType, ReadingOrder, Style, StyleId, StyleTable, Theme,
    Underline, VAlign, VertAlign,
};
pub use workbook::{
    CalcMode, CalcSettings, DefinedName, DocProps, EQUATION_BUDGET_MESSAGE, MAX_EQUATION_LAYOUT_BYTES, PhoneticProps, PhoneticRun, PhoneticString,
    RefHost, ShowObjects, Workbook, WorkbookView,
};

/// Font used for new workbooks. Only a name: renderers substitute an available open font
/// (e.g. the metric-compatible Carlito) when it isn't installed.
pub const DEFAULT_FONT: &str = "Calibri";
pub const DEFAULT_FONT_SIZE: f32 = 11.0;
