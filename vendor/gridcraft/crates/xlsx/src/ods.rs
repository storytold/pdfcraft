//! Data-only OpenDocument Spreadsheet import backed by `craft-ods`.
//! Formula caches become constants; styles and other ODF features are deliberately not translated.

use std::io::{Cursor, Read};
use std::sync::Arc;

use gridcraft_core::{CellError, CellRef, RangeRef, Value};
use gridcraft_model::{Cell, NumFmt, Sheet, StyleId, Workbook};

use crate::package::MAX_ENTRIES;
use crate::read::SheetNames;
use crate::{IoError, ReadReport};

const MIME: &[u8] = b"application/vnd.oasis.opendocument.spreadsheet";
const MAX_SHEETS: usize = 1024;

/// Reads only the small, exact mimetype entry, including when the ZIP uses compression.
/// Looking at a filename or at arbitrary bytes inside the archive is not sufficient.
pub(crate) fn has_mimetype(bytes: &[u8]) -> bool {
    let Ok(mut zip) = zip::ZipArchive::new(Cursor::new(bytes)) else { return false };
    if zip.len() > MAX_ENTRIES {
        return false;
    }
    let Ok(mut file) = zip.by_name("mimetype") else { return false };
    if file.size() != MIME.len() as u64 {
        return false;
    }
    let mut value = Vec::new();
    (&mut file).take(MIME.len() as u64 + 1).read_to_end(&mut value).is_ok() && value == MIME
}

/// Imports ODS sheets, typed values, displayed text and merges. Cached formula results are
/// imported as constants, so editing their inputs does not recalculate the original formula.
/// Always inspect the returned warnings. Saving to ODS is not supported.
pub fn read_ods(bytes: &[u8]) -> Result<(Workbook, ReadReport), IoError> {
    // The exact mimetype, as `sniff` requires: craft-ods alone accepts some near misses.
    if !has_mimetype(bytes) {
        return Err(IoError::Format("missing or invalid ODS mimetype".into()));
    }
    let book = craft_ods::read(bytes, &craft_ods::Options::default()).map_err(map_error)?;
    if book.sheets.is_empty() {
        return Err(IoError::Format("ODS contains no worksheets".into()));
    }
    if book.sheets.len() > MAX_SHEETS {
        return Err(IoError::TooLarge("ODS contains more than 1024 sheets".into()));
    }

    // `Workbook::new` starts with an empty Sheet1; the ODS tables are the only sheets.
    let mut wb = Workbook::new();
    wb.sheets.clear();
    let mut names = SheetNames::default();
    let mut report = ReadReport::default();
    report.warn("ODS data-only import: formulas become their saved results and will not recalculate. Source formatting, charts, images and other features are not imported. Save a new .xlsx file; ODS saving is not supported.");

    let mut warned_rename = false;
    let mut warned_unavailable = false;

    for src_sheet in book.sheets {
        let mut name = names.unique(src_sheet.name.trim_matches('\''), wb.sheets.len());
        if wb.check_sheet_name(&name, None).is_err() {
            name = wb.next_sheet_name();
            names.insert(&name);
        }
        let mut sheet = Sheet::new(name);
        if !warned_rename && sheet.name != src_sheet.name {
            warned_rename = true;
            report.warn("Some ODS sheet names were shortened or renamed to valid, unique XLSX sheet names.");
        }

        for (coord, src_cell) in src_sheet.cells {
            // A typed value that cannot be read (an unparsable number or date, an error cache,
            // a formula without a result) keeps what the cell showed, else its raw value.
            let Some(value) = src_cell.value().map(map_value) else {
                if !warned_unavailable {
                    warned_unavailable = true;
                    report.warn("Some ODS values or formula caches were unsupported or missing; their displayed text (or raw value) was kept as text. A formula without either is marked [Unavailable ODS formula result].");
                }
                let text = if !src_cell.text.is_empty() {
                    src_cell.text.as_str()
                } else if let Some(raw) = src_cell.raw.as_deref() {
                    raw
                } else if src_cell.formula.is_some() {
                    "[Unavailable ODS formula result]"
                } else {
                    "[Unsupported ODS value]"
                };
                sheet.set_cell(CellRef::new(coord.row, coord.col), Cell::styled(Value::Text(Arc::from(text)), StyleId::DEFAULT));
                continue;
            };

            let format = match src_cell.value_type {
                Some(craft_ods::ValueType::Date) => {
                    let with_time = src_cell.raw.as_deref().and_then(craft_ods::date_serial).is_some_and(|(_, time)| time);
                    Some(if with_time { "yyyy-mm-dd hh:mm:ss.000" } else { "yyyy-mm-dd" })
                }
                Some(craft_ods::ValueType::Time) => Some("[h]:mm:ss.000"),
                Some(craft_ods::ValueType::Percentage) => Some("0.00%"),
                Some(craft_ods::ValueType::Currency) => Some("#,##0.00"),
                _ => None,
            };
            let style_id = format.map_or(StyleId::DEFAULT, |code| wb.styles.derive(StyleId::DEFAULT, |s| s.num_fmt = NumFmt::new(code)));

            let target_coord = CellRef::new(coord.row, coord.col);
            sheet.set_cell(target_coord, Cell::styled(value, style_id));
        }

        for m in src_sheet.merges {
            let range = RangeRef::new(CellRef::new(m.first.row, m.first.col), CellRef::new(m.last.row, m.last.col));
            sheet.merges.push(range);
        }
        wb.sheets.push(Arc::new(sheet));
    }

    Ok((wb, report))
}

fn map_error(e: craft_ods::Error) -> IoError {
    match e {
        craft_ods::Error::Encrypted => IoError::Format("encrypted ODS files are not supported".into()),
        craft_ods::Error::Xml(msg) => IoError::Xml(format!("ODS: {msg}")),
        craft_ods::Error::TooLarge(msg) => IoError::TooLarge(msg),
        craft_ods::Error::Invalid(msg) => IoError::Format(msg),
        other => IoError::Format(other.to_string()),
    }
}

fn map_value(v: craft_ods::Value) -> Value {
    match v {
        craft_ods::Value::Empty => Value::Empty,
        craft_ods::Value::Number(n) => Value::Number(n),
        craft_ods::Value::Text(s) => Value::Text(s),
        craft_ods::Value::Bool(b) => Value::Bool(b),
        craft_ods::Value::Error(e) => Value::Error(map_cell_error(e)),
    }
}

fn map_cell_error(e: craft_ods::CellError) -> CellError {
    match e {
        craft_ods::CellError::Null => CellError::Null,
        craft_ods::CellError::Div0 => CellError::Div0,
        craft_ods::CellError::Value => CellError::Value,
        craft_ods::CellError::Ref => CellError::Ref,
        craft_ods::CellError::Name => CellError::Name,
        craft_ods::CellError::Num => CellError::Num,
        craft_ods::CellError::NA => CellError::NA,
        craft_ods::CellError::GettingData => CellError::GettingData,
        craft_ods::CellError::Spill => CellError::Spill,
        craft_ods::CellError::Calc => CellError::Calc,
        craft_ods::CellError::Blocked => CellError::Blocked,
        _ => CellError::Value,
    }
}
