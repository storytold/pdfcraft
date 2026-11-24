//! Opening and saving files: XLSX, legacy XLS import, XLSB and ODS data import, SYLK import, CSV/TSV, JSON (debug), HTML export.

use std::fmt::Write as _;

use gridcraft_core::{CellRef, RangeRef};
use gridcraft_model::{LineIndex, Sheet, Workbook};

use crate::{EngineError, Result};

mod sylk;

/// The function library's answers to what the XLSX and XLS readers and the XLSX writer need to know about functions
/// (which names are built in, which parameters take a single value).
pub const XLSX_FUNCTIONS: gridcraft_xlsx::FunctionInfo = gridcraft_xlsx::FunctionInfo {
    is_builtin: |name| gridcraft_functions::lookup(name).is_some(),
    is_scalar_arg: |name, i| gridcraft_functions::lookup(name).is_none_or(|s| s.is_scalar_arg(i)),
};

/// File formats GridCraft reads or writes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FileKind {
    Xlsx,
    /// Excel 97-2003 workbook (import only).
    Xls,
    Xlsb,
    Ods,
    Csv,
    Tsv,
    Json,
    Html,
    /// SYLK symbolic link text (import only).
    Sylk,
}

impl FileKind {
    pub fn from_path(path: &str) -> Option<FileKind> {
        let ext = std::path::Path::new(path).extension()?.to_str()?.to_ascii_lowercase();
        Some(match ext.as_str() {
            "xlsx" | "xlsm" | "xltx" | "xltm" => FileKind::Xlsx,
            "xls" | "xlt" => FileKind::Xls,
            "xlsb" => FileKind::Xlsb,
            "ods" => FileKind::Ods,
            "csv" => FileKind::Csv,
            "tsv" | "tab" | "txt" => FileKind::Tsv,
            ext if craft_gridcraft_format::is_extension(ext) => FileKind::Json,
            "html" | "htm" => FileKind::Html,
            "slk" => FileKind::Sylk,
            _ => return None,
        })
    }
}

/// The error for an encrypted workbook opened without a password contains this. The interface
/// asks for the password when it sees it; programmatic callers pass `password` instead.
pub const PASSWORD_REQUIRED: &str = "is password-protected. Enter its password to open it.";
/// The error for a wrong password contains this.
pub const WRONG_PASSWORD: &str = "The password is incorrect.";
/// The first open warning of a workbook decrypted with its password.
pub const DECRYPTED_NOTICE: &str = "This workbook was password-protected. GridCraft saves it without the password.";

/// Parses a file's bytes into a workbook. `name` picks the format by extension when the bytes
/// don't say. An encrypted workbook needs [`open_bytes_with_password`].
pub fn open_bytes(name: &str, bytes: &[u8]) -> Result<(Workbook, Vec<String>)> {
    open_bytes_with_password(name, bytes, None)
}

/// [`open_bytes`], first decrypting a password-protected workbook with `password`. Errors never
/// quote the password.
pub fn open_bytes_with_password(name: &str, bytes: &[u8], password: Option<&str>) -> Result<(Workbook, Vec<String>)> {
    open_bytes_with(name, bytes, password, false)
}

/// [`open_bytes_with_password`], and with `csv_as_text` a CSV/TSV/TXT file keeps every field as
/// the text it holds, with no number, date or boolean conversion (identifiers such as GTINs, SKUs
/// and ZIP codes survive). It does not apply to other formats.
pub fn open_bytes_with(name: &str, bytes: &[u8], password: Option<&str>, csv_as_text: bool) -> Result<(Workbook, Vec<String>)> {
    if gridcraft_xlsx::sniff(bytes) != gridcraft_xlsx::Format::Encrypted {
        let (mut wb, warnings) = open_plain(name, bytes, csv_as_text)?;
        fit_imported_rows(&mut wb);
        return Ok((wb, warnings));
    }
    // A scheme no password opens is said straight away, rather than after asking for one.
    let decrypt_error = |e: gridcraft_xlsx::DecryptError| EngineError::Other(decrypt_message(name, e));
    let Some(password) = password else {
        gridcraft_xlsx::check_encryption(bytes).map_err(decrypt_error)?;
        return Err(EngineError::Other(format!("'{name}' {PASSWORD_REQUIRED}")));
    };
    let plain = gridcraft_xlsx::decrypt(bytes, password).map_err(decrypt_error)?;
    if gridcraft_xlsx::sniff(&plain) == gridcraft_xlsx::Format::Encrypted {
        return Err(EngineError::Other(format!("We can't open '{name}': the encrypted file is damaged (it holds another encrypted file).")));
    }
    let (mut wb, mut warnings) = open_plain(name, &plain, csv_as_text)?;
    fit_imported_rows(&mut wb);
    warnings.insert(0, DECRYPTED_NOTICE.into());
    Ok((wb, warnings))
}

/// Excel auto-fits rows to their tallest font when opening a file: rows without a user-set
/// height grow to fit their content. Rows the file made taller keep their height — our
/// line-height estimate can differ from Excel's, and shrinking files that already look
/// right would be a visible regression. Custom-height rows are left alone entirely.
fn fit_imported_rows(wb: &mut Workbook) {
    // The whole pass shares one budget of merge comparisons: later sheets keep their heights once it is spent.
    let mut left = crate::cmd::MERGE_FIT_WORK;
    for si in 0..wb.sheets.len() {
        let Some(sh) = wb.sheet(si) else { continue };
        let mut rows: Vec<u32> = sh.cells.iter().map(|(c, _)| c.row).collect();
        rows.sort_unstable();
        rows.dedup();
        // One sweep over the sheet's merges for all of its rows, in ascending order.
        let mut sweep = crate::cmd::MergeSweep::new(left);
        for row in rows {
            let before = wb.sheet(si).and_then(|sh| sh.rows.get(&row)).and_then(|i| i.size);
            crate::cmd::fit_row_height(wb, si, row, &mut sweep);
            let Some(before) = before else { continue };
            let after = wb.sheet(si).and_then(|sh| sh.rows.get(&row)).and_then(|i| i.size);
            if after.is_none_or(|a| before > a + 0.5)
                && let Some(sh) = wb.sheet_mut(si)
            {
                sh.rows.entry(row).or_default().size = Some(before);
            }
        }
        left = sweep.left();
    }
}

/// What to tell the user when an encrypted workbook can't be decrypted. Never quotes the password.
fn decrypt_message(name: &str, e: gridcraft_xlsx::DecryptError) -> String {
    match e {
        gridcraft_xlsx::DecryptError::WrongPassword => format!("{WRONG_PASSWORD} Check that Caps Lock is off and try again."),
        gridcraft_xlsx::DecryptError::Unsupported(scheme) => {
            format!("'{name}' uses {scheme} encryption, which GridCraft can't open yet. Remove the password in Excel and try again.")
        }
        gridcraft_xlsx::DecryptError::Malformed(detail) => format!("We can't open '{name}': the encrypted file is damaged ({detail})."),
    }
}

/// Parses bytes that aren't encrypted, then takes the workbook's macros, and the button actions
/// xlsx has no place for, out of their hidden names, and adopts the Python cells they hold (see
/// [`adopt_python`]) within the limits of [`enforce_python_limits`].
fn open_plain(name: &str, bytes: &[u8], csv_as_text: bool) -> Result<(Workbook, Vec<String>)> {
    let (mut wb, mut warnings) = read_plain(name, bytes, csv_as_text)?;
    warnings.extend(crate::macros::load(&mut wb));
    crate::cmd::controls::load_action_names(&mut wb);
    warnings.extend(adopt_python(&mut wb));
    warnings.extend(enforce_python_limits(&mut wb));
    Ok((wb, warnings))
}

/// The most result cells [`adopt_python`] rebuilds from a file's cached spills, over all its
/// Python cells (each one is also limited to [`gridcraft_model::python::MAX_RESULT_CELLS`]).
pub const MAX_ADOPTED_RESULT_CELLS: u64 = 4_000_000;

/// Python cells from a file: a cell whose formula is `PY("…",0|1)` (a format that kept it as a
/// formula) becomes a Python cell, and a Python cell whose spilled values the file cached gets
/// them back as its result, so it shows them until it runs again. Results are rebuilt for at most
/// [`MAX_ADOPTED_RESULT_CELLS`] cells in all; the rest keep only their own cached value, and the
/// values they spilled stay as plain values. Returns the warnings.
pub fn adopt_python(wb: &mut Workbook) -> Vec<String> {
    let mut budget = MAX_ADOPTED_RESULT_CELLS;
    let mut skipped = 0usize;
    for si in 0..wb.sheets.len() {
        let Some(sh) = wb.sheets.get(si) else { continue };
        let formulas: Vec<(CellRef, gridcraft_model::PyCode)> = sh
            .cells
            .iter()
            .filter_map(|(c, cell)| cell.formula.as_ref().and_then(|f| gridcraft_model::PyCode::from_formula(&f.text)).map(|p| (c, p)))
            .collect();
        let max = gridcraft_model::python::MAX_RESULT_CELLS as u64;
        let spills: Vec<(CellRef, RangeRef)> = sh
            .spill_ranges
            .iter()
            .filter(|(a, r)| r.count() <= max && sh.cell(**a).and_then(|c| c.py.as_ref()).is_some_and(|p| p.output.is_none()))
            .map(|(a, r)| (*a, *r))
            .collect();
        if formulas.is_empty() && spills.is_empty() {
            continue;
        }
        let Some(sh) = wb.sheet_mut(si) else { continue };
        for (c, py) in formulas {
            if let Some(cell) = sh.cells.get_mut(c) {
                cell.formula = None;
                cell.py = Some(std::sync::Arc::new(py));
            }
        }
        for (anchor, range) in spills {
            let Some(left) = budget.checked_sub(range.count()) else {
                skipped += 1;
                // The cell keeps only its own value; the values it spilled stay as plain values, so
                // nothing shows as a spill that no result holds, and nothing the file showed is lost.
                sh.settle_spill(anchor);
                continue;
            };
            budget = left;
            let data: Vec<gridcraft_core::Value> =
                range.iter().map(|c| if c == anchor { sh.cell(c).map(|x| x.value.clone()).unwrap_or_default() } else { sh.value(c) }).collect();
            let Some(array) = gridcraft_core::Array::new(range.height() as usize, range.width() as usize, data) else { continue };
            if let Some(py) = sh.cells.get_mut(anchor).and_then(|x| x.py.as_mut()) {
                std::sync::Arc::make_mut(py).output =
                    Some(gridcraft_model::PyOutput { value: array.into(), type_name: "list".into(), preview: String::new(), convertible: true });
            }
        }
    }
    if skipped == 0 {
        return Vec::new();
    }
    vec![format!(
        "The file's Python results are larger than {MAX_ADOPTED_RESULT_CELLS} cells in all, so {skipped} Python cell(s) have no result until they run again; the values they spilled were kept as plain values."
    )]
}

/// A file is somebody else's work: a Python cell whose code doesn't fit in Excel's
/// 8,192-character formula keeps only its cached value, and a kept result that is not a
/// well-formed array of at most [`gridcraft_model::python::MAX_RESULT_CELLS`] cells is dropped
/// (the cell keeps its value). Either way the values the cell spilled stay as plain values
/// ([`gridcraft_model::Sheet::settle_spill`]). Returns the warnings.
pub fn enforce_python_limits(wb: &mut Workbook) -> Vec<String> {
    let mut long = Vec::new();
    for si in 0..wb.sheets.len() {
        let Some(sh) = wb.sheets.get(si) else { continue };
        let mut over = Vec::new();
        let mut bad_output = Vec::new();
        for (c, cell) in sh.cells.iter() {
            let Some(py) = &cell.py else { continue };
            if !py.fits() {
                long.push(format!("{}!{}", sh.name, c.a1()));
                over.push(c);
            } else if py.output.as_ref().is_some_and(|o| !sane_python_output(&o.value)) {
                bad_output.push(c);
            }
        }
        if over.is_empty() && bad_output.is_empty() {
            continue;
        }
        let dropped: Vec<CellRef> = over.iter().chain(&bad_output).copied().collect();
        let Some(sh) = wb.sheet_mut(si) else { continue };
        for c in over {
            if let Some(cell) = sh.cells.get_mut(c) {
                cell.py = None;
            }
        }
        for c in bad_output {
            if let Some(py) = sh.cells.get_mut(c).and_then(|x| x.py.as_mut()) {
                std::sync::Arc::make_mut(py).output = None;
            }
        }
        // A dropped result (or code dropped to its value) leaves the values it spilled as plain
        // values, as the file showed them, with no live spill.
        for c in dropped {
            sh.settle_spill(c);
        }
    }
    let Some(first) = long.first() else { return vec![] };
    let more = if long.len() > 1 { format!(" (and {} more)", long.len() - 1) } else { String::new() };
    vec![format!(
        "A Python cell's code is longer than Excel's {}-character formula limit, so {first}{more} kept only its last calculated value.",
        gridcraft_model::python::MAX_FORMULA_CHARS
    )]
}

/// A Python result a file may hold: a scalar, or an array whose size matches its data and has at
/// most [`gridcraft_model::python::MAX_RESULT_CELLS`] cells.
fn sane_python_output(v: &gridcraft_core::Value) -> bool {
    match v {
        gridcraft_core::Value::Array(a) => {
            a.rows >= 1
                && a.cols >= 1
                && a.rows.checked_mul(a.cols).is_some_and(|n| n == a.data.len() && n <= gridcraft_model::python::MAX_RESULT_CELLS)
        }
        _ => true,
    }
}

/// Parses bytes that aren't encrypted, as they are.
fn read_plain(name: &str, bytes: &[u8], csv_as_text: bool) -> Result<(Workbook, Vec<String>)> {
    let kind = FileKind::from_path(name);
    let sniffed = gridcraft_xlsx::sniff(bytes);
    if sniffed == gridcraft_xlsx::Format::Xls {
        let (wb, report) =
            gridcraft_xlsx::read_xls_with(bytes, XLSX_FUNCTIONS).map_err(|e| EngineError::Other(format!("We can't open '{name}': {e}")))?;
        return Ok((wb, report.warnings));
    }
    if sniffed == gridcraft_xlsx::Format::LegacyBinary {
        return Err(EngineError::Other(format!(
            "'{name}' is a legacy binary file but not an Excel workbook (no workbook stream). GridCraft can't open it."
        )));
    }
    // Content wins over the extension: an ODS or XLSB package is imported as what it is,
    // whatever it is called; the extension only decides when the bytes don't say.
    let import = match sniffed {
        gridcraft_xlsx::Format::Ods => Some(FileKind::Ods),
        gridcraft_xlsx::Format::Xlsb => Some(FileKind::Xlsb),
        // An XLSX package is never read as ODS/XLSB just because of its name.
        gridcraft_xlsx::Format::Xlsx => None,
        _ => kind.filter(|k| matches!(k, FileKind::Ods | FileKind::Xlsb)),
    };
    if let Some(import) = import {
        let read = if import == FileKind::Ods { gridcraft_xlsx::read_ods } else { gridcraft_xlsx::read_xlsb };
        let (wb, report) = read(bytes).map_err(|e| EngineError::Other(format!("We can't import '{name}': {e}")))?;
        return Ok((wb, report.warnings));
    }
    // Text named `.xlsx` is read as text.
    if sniffed == gridcraft_xlsx::Format::Xlsx || (kind == Some(FileKind::Xlsx) && sniffed != gridcraft_xlsx::Format::Csv) {
        // Google Sheets' export wrapper is read as the formula inside when GridCraft calculates it (#2871).
        let (wb, report) = gridcraft_xlsx::read_xlsx_with(bytes, (XLSX_FUNCTIONS, &gridcraft_calc::is_known_function as &dyn Fn(&str) -> bool))
            .map_err(|e| {
                EngineError::Other(match (&e, gridcraft_xlsx::oversized_part(bytes)) {
                    (gridcraft_xlsx::IoError::TooLarge(_), Some((part, size))) => part_too_large_message(name, &part, size),
                    _ => format!("We can't open '{name}': {e}"),
                })
            })?;
        return Ok((wb, report.warnings));
    }
    if let Some(read) = read_html_table(name, kind, bytes, csv_as_text) {
        return read;
    }
    match kind {
        Some(FileKind::Json) => {
            let mut wb: Workbook = craft_gridcraft_format::read(bytes).map_err(|e| EngineError::Other(e.to_string()))?;
            wb.styles.rebuild_index();
            if wb.sheets.is_empty() {
                // The file's macros are read after this (`macros::load`), so they come along.
                let macros = std::mem::take(&mut wb.macros_read);
                wb = Workbook::new();
                wb.macros_read = macros;
            }
            // The file's band placement and cell count are derived state: rebuild them from the
            // cells themselves, dropping what is outside the sheet grid (#1008).
            let dropped: usize = wb.sheets.iter_mut().map(|sh| std::sync::Arc::make_mut(sh).cells.rebuild()).sum();
            let mut warnings = Vec::new();
            if dropped > 0 {
                warnings.push(format!("{name}: {dropped} cells outside the 1,048,576 × 16,384 grid were dropped"));
            }
            Ok((wb, warnings))
        }
        Some(FileKind::Sylk) => {
            let (mut wb, warnings) = sylk::read(name, bytes)?;
            rename_first_sheet(&mut wb, name);
            Ok((wb, warnings))
        }
        Some(FileKind::Tsv) => {
            let opts = gridcraft_xlsx::CsvOptions { delimiter: b'\t', parse_values: !csv_as_text, ..Default::default() };
            let (mut wb, report) = gridcraft_xlsx::read_csv_with_report(bytes, &opts).map_err(|e| EngineError::Other(e.to_string()))?;
            // Tab is what makes it a .tsv/.txt; saved as .csv it gets the CSV default.
            wb.csv_delimiter = None;
            rename_first_sheet(&mut wb, name);
            Ok((wb, report.warnings))
        }
        _ => {
            let opts = gridcraft_xlsx::CsvOptions { delimiter: 0, parse_values: !csv_as_text, ..Default::default() };
            let (mut wb, report) = gridcraft_xlsx::read_csv_with_report(bytes, &opts).map_err(|e| EngineError::Other(e.to_string()))?;
            rename_first_sheet(&mut wb, name);
            Ok((wb, report.warnings))
        }
    }
}

/// What to tell the user when one part of an XLSX package unpacks to more than the reader's
/// per-part limit: the sizes, why the limit is there, and what gets the data in. The limit is
/// per part, so smaller sheets fit; the shared strings are one part for every sheet, so only a
/// CSV, which has no parts, helps there.
fn part_too_large_message(name: &str, part: &str, size: u64) -> String {
    let limit = gridcraft_xlsx::MAX_PART >> 20;
    let mb = size >> 20;
    let size = if mb > limit { format!("{mb} MB") } else { format!("more than {limit} MB") };
    let remedy = if part.contains("worksheets/") {
        "To open it, split the data across several sheets, since the limit applies to each part, or save the sheet as CSV, which keeps only its values."
    } else {
        "To open it, save the data as CSV, which keeps only its values."
    };
    format!(
        "We can't open '{name}': its part {part} unpacks to {size}, over GridCraft's limit of {limit} MB for one part \
         (a guard against damaged or malicious files). {remedy}"
    )
}

/// How much of a text file is read to tell whether it is a web page.
const HTML_SNIFF_BYTES: usize = 64 << 10;

/// A web page that holds a table, whatever its extension: a web app's "Export to Excel" is often
/// HTML saved as `.xls` (#3166). Excel opens its table as a sheet (after warning that the format
/// and extension don't match): the cells read like typed input (or as text with `csv_as_text`),
/// header cells (`<th>`) are bold and centred, and the sheet takes the name the page gives it
/// (`<x:Name>`) or the file's. `None` when the bytes aren't such a page.
fn read_html_table(name: &str, kind: Option<FileKind>, bytes: &[u8], csv_as_text: bool) -> Option<Result<(Workbook, Vec<String>)>> {
    // Every other text file (CSV, TSV, JSON) passes through here: look at the start only, and decode
    // the whole file once it is known to be a page within the size a text import takes.
    if matches!(kind, Some(FileKind::Json)) || bytes.len() > gridcraft_xlsx::MAX_CSV_BYTES {
        return None;
    }
    let start = gridcraft_xlsx::decode_text(bytes.get(..HTML_SNIFF_BYTES).unwrap_or(bytes));
    // Comments an exporter writes before the page (`<!-- ... -->`) don't make it something else.
    let mut rest = start.trim_start_matches('\u{feff}').trim_start();
    while let Some(after) = rest.strip_prefix("<!--") {
        match after.find("-->").and_then(|end| after.get(end + 3..)) {
            Some(more) => rest = more.trim_start(),
            None => break,
        }
    }
    let head: String = rest.chars().take(16).collect::<String>().to_ascii_lowercase();
    if !["<!doctype html", "<html", "<table", "<head", "<body", "<meta"].iter().any(|t| head.starts_with(t)) {
        return None;
    }
    // Decoded as a CSV is (BOM, UTF-8, else Windows-1252/1256), so a legacy page keeps its letters.
    let text = gridcraft_xlsx::decode_text(bytes);
    let rows = crate::clip_html::table_cells(&text, gridcraft_core::MAX_ROWS as usize, gridcraft_core::MAX_COLS as usize);
    if rows.iter().all(Vec::is_empty) {
        return None;
    }
    // The cells go through the CSV reader, so numbers, dates and text read exactly as they do there.
    let mut tsv = String::new();
    for row in &rows {
        let fields: Vec<String> = row
            .iter()
            .map(|cell| {
                // The number the page recorded (`x:num`) only when values are read; as text, the
                // cell keeps the text it shows.
                let field = match cell.num.filter(|_| !csv_as_text) {
                    Some(n) => gridcraft_core::number_to_text(n),
                    // Whitespace is already collapsed; non-breaking spaces stay, as in Excel.
                    None => cell.text.clone(),
                };
                if field.contains(['\t', '\n', '\r', '"']) { format!("\"{}\"", field.replace('"', "\"\"")) } else { field }
            })
            .collect();
        tsv.push_str(&fields.join("\t"));
        tsv.push('\n');
    }
    let opts = gridcraft_xlsx::CsvOptions { delimiter: b'\t', parse_values: !csv_as_text, ..Default::default() };
    let read = gridcraft_xlsx::read_csv_with_report(tsv.as_bytes(), &opts).map_err(|e| EngineError::Other(e.to_string()));
    Some(read.map(|(mut wb, report)| {
        wb.csv_delimiter = None;
        // Header cells are bold and centred on top of the style the CSV reader gave them, so a
        // date in a header keeps its date format.
        let headers: Vec<CellRef> = rows
            .iter()
            .enumerate()
            .flat_map(|(r, row)| row.iter().enumerate().filter(|(_, cell)| cell.header && !cell.text.trim().is_empty()).map(move |(c, _)| CellRef::new(r as u32, c as u32)))
            .collect();
        let mut bold: std::collections::HashMap<gridcraft_model::StyleId, gridcraft_model::StyleId> = std::collections::HashMap::new();
        for at in headers {
            let Some(base) = wb.sheet(0).map(|sh| sh.style_id(at)) else { continue };
            let id = *bold.entry(base).or_insert_with(|| {
                wb.styles.derive(base, |st| {
                    st.font.bold = true;
                    st.align.h = gridcraft_model::HAlign::Center;
                })
            });
            if let Some(sh) = wb.sheet_mut(0) {
                sh.set_style(at, id);
            }
        }
        let page_name = find_between(&text, "<x:Name>", "</x:Name>").map(|n| crate::clip_html::normalized(n.trim()));
        match page_name.filter(|n| !n.is_empty()) {
            Some(sheet) => set_first_sheet_name(&mut wb, &sheet),
            None => rename_first_sheet(&mut wb, name),
        }
        let mut warnings = report.warnings;
        if !matches!(kind, Some(FileKind::Html)) {
            warnings.insert(0, format!("'{name}' is a web page (HTML), not a workbook in the format its name says: its table was opened as a sheet. Save as .xlsx to keep your changes."));
        }
        (wb, warnings)
    }))
}

/// The text between the first `open` and the `close` after it, matched case-insensitively.
fn find_between<'a>(text: &'a str, open: &str, close: &str) -> Option<&'a str> {
    let lower = text.to_ascii_lowercase();
    let start = lower.find(&open.to_ascii_lowercase())?.checked_add(open.len())?;
    let len = lower.get(start..)?.find(&close.to_ascii_lowercase())?;
    text.get(start..start.checked_add(len)?)
}

/// A CSV opens as a sheet named after the file (like Excel).
fn rename_first_sheet(wb: &mut Workbook, path: &str) {
    let stem = std::path::Path::new(path).file_stem().and_then(std::ffi::OsStr::to_str).unwrap_or("Sheet1");
    set_first_sheet_name(wb, stem);
}

/// Names the first sheet `name`, without the characters a sheet name can't hold and cut to 31
/// characters; an empty or reserved result keeps the default name.
fn set_first_sheet_name(wb: &mut Workbook, stem: &str) {
    let clean: String = stem.chars().filter(|c| !matches!(c, ':' | '\\' | '/' | '?' | '*' | '[' | ']')).take(31).collect();
    let clean = clean.trim_matches('\'').to_string();
    // "History" is reserved (see `Workbook::check_sheet_name`): keep the default name instead.
    if !clean.is_empty()
        && !clean.eq_ignore_ascii_case("History")
        && let Some(sh) = wb.sheet_mut(0)
    {
        sh.name = clean;
    }
}

/// Encodes a workbook in the format chosen by `path`'s extension.
pub fn save_bytes(wb: &Workbook, path: &str) -> Result<Vec<u8>> {
    // An unknown or missing extension is refused rather than written as an XLSX under that name:
    // a typo like `.cvs` would otherwise produce a file no other app recognises.
    let Some(kind) = FileKind::from_path(path) else {
        let name = std::path::Path::new(path).file_name().and_then(std::ffi::OsStr::to_str).unwrap_or(path);
        return Err(EngineError::Other(format!("{name}: unknown file format. Save as .xlsx, .csv, .tsv, .json or .html.")));
    };
    save_bytes_as(wb, path, kind)
}

/// Encodes a workbook as `kind`, whatever `path`'s extension says.
pub fn save_bytes_as(wb: &Workbook, path: &str, kind: FileKind) -> Result<Vec<u8>> {
    let sheet = wb.active_sheet();
    match kind {
        // The extension picks the package kind; only the macro-enabled kinds (.xlsm, .xltm) carry
        // the VBA project, so any other name saves without it, as Excel does.
        // Macros, and button actions xlsx has no place for, travel in hidden names.
        FileKind::Xlsx => {
            let named = crate::macros::with_names(wb)?;
            let named = match crate::cmd::controls::with_action_names(named.as_ref().unwrap_or(wb))? {
                Some(w) => Some(w),
                None => named,
            };
            let book = std::path::Path::new(path).file_name().and_then(std::ffi::OsStr::to_str).unwrap_or_default();
            gridcraft_xlsx::write_xlsx_named_with(named.as_ref().unwrap_or(wb), gridcraft_xlsx::XlsxKind::from_path(path), book, XLSX_FUNCTIONS)
                .map_err(|e| EngineError::Other(e.to_string()))
        }
        FileKind::Xls => {
            Err(EngineError::Other("Excel 97-2003 (.xls) is supported for import only. Save as .xlsx or another supported export format.".into()))
        }
        FileKind::Xlsb => Err(EngineError::Other("XLSB is supported for data import only. Save as .xlsx or another supported export format.".into())),
        FileKind::Ods => Err(EngineError::Other("ODS is supported for data import only. Save as .xlsx or another supported export format.".into())),
        FileKind::Sylk => {
            Err(EngineError::Other("SYLK (.slk) is supported for import only. Save as .xlsx or another supported export format.".into()))
        }
        // A workbook read from CSV keeps the separator it was read with (a `;` file stays `;`).
        FileKind::Csv => wb.sheet(sheet).map(|sh| csv(wb, sh, wb.csv_delimiter.unwrap_or(b','))).transpose().map(Option::unwrap_or_default),
        FileKind::Tsv => wb.sheet(sheet).map(|sh| csv(wb, sh, b'\t')).transpose().map(Option::unwrap_or_default),
        FileKind::Json => crate::macros::json_bytes(wb),
        FileKind::Html => to_html(wb, sheet).map(String::into_bytes),
    }
}

/// [`save_bytes`] for the file at `path`: relative paths to linked workbooks are rewritten for
/// its folder, so they still name the same files.
pub fn save_file_bytes(wb: &Workbook, path: &str) -> Result<Vec<u8>> {
    match source_dir(path).and_then(|f| gridcraft_xlsx::rebase_links(wb, &f)) {
        Some(rebased) => save_bytes(&rebased, path),
        None => save_bytes(wb, path),
    }
}

/// The folder of the file at `path`, which relative paths to linked workbooks are relative to.
pub fn source_dir(path: &str) -> Option<std::path::PathBuf> {
    std::path::absolute(path).ok()?.parent().map(std::path::Path::to_path_buf)
}

/// The file at `path` as `CELL("filename")` reports it: absolute, so the folder is the one the
/// user sees. A workbook that arrived as bytes has no path and keeps only its name.
pub fn source_path(path: &str) -> Option<std::path::PathBuf> {
    std::path::absolute(path).ok()
}

/// Save As CSV/Text writes "the text and values as they are displayed in cells" with "all
/// characters in each cell" (Microsoft Support, *Excel formatting and features that are not
/// transferred to other file formats*): number formats applied, never column-width `#####`.
/// One intentional difference: a number shown as General is not narrowed to the cell width and
/// never gets an exponent (`gridcraft_numfmt::format_value_full`). Below 1E+15 it is written as
/// `&` converts it (15 significant digits); from 1E+15 as the shortest digits that read back as
/// the same value (every integer below 1e16 exactly, so long identifiers survive); small numbers
/// that `&` writes in scientific notation as their 15 digits in full.
fn csv(wb: &Workbook, sh: &Sheet, delimiter: u8) -> Result<Vec<u8>> {
    gridcraft_xlsx::write_csv(sh, delimiter, |c| crate::display::value_text(wb, sh, c)).map_err(|e| EngineError::Other(e.to_string()))
}

/// The active sheet as CSV of raw cell values, for automation: numbers at full precision, dates and
/// times as ISO 8601, booleans `TRUE`/`FALSE`, errors as their text, text as stored. Save As CSV
/// writes displayed text instead ([`save_bytes`]) and stays the default.
pub fn csv_values_bytes(wb: &Workbook) -> Result<Vec<u8>> {
    let Some(sh) = wb.sheet(wb.active_sheet()) else { return Ok(Vec::new()) };
    gridcraft_xlsx::write_csv(sh, wb.csv_delimiter.unwrap_or(b','), |c| match sh.value(c) {
        gridcraft_core::Value::Number(n) if n.is_finite() => {
            let shape = crate::display::parsed(wb.styles.get(sh.style_id(c)).num_fmt.as_str()).date_shape(n);
            match shape.zip(gridcraft_core::date::datetime_from_serial(wb.date_system, n)) {
                Some(((date, time), d)) => iso(&d, date, time),
                None => n.to_string(),
            }
        }
        gridcraft_core::Value::Text(t) => t.to_string(),
        v => crate::display::value_text_as(wb, &v, "General"),
    })
    .map_err(|e| EngineError::Other(e.to_string()))
}

/// `date` and `time` say which parts the cell's format shows; a format with neither is not a date.
fn iso(d: &gridcraft_core::date::DateTime, date: bool, time: bool) -> String {
    let day = format!("{:04}-{:02}-{:02}", d.year, d.month, d.day);
    let mut clock = format!("{:02}:{:02}:{:02}", d.hour, d.minute, d.second);
    if d.milli != 0 {
        clock.push_str(&format!(".{:03}", d.milli));
    }
    match (date, time) {
        (true, true) => format!("{day}T{clock}"),
        (false, true) => clock,
        _ => day,
    }
}

pub(crate) fn esc(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;")
}

/// [`esc`], or `None` once the escaped text would pass `limit` bytes (nothing larger is built).
fn esc_within(s: &str, limit: usize) -> Option<String> {
    let mut out = String::with_capacity(s.len().min(limit));
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            c => out.push(c),
        }
        if out.len() > limit {
            return None;
        }
    }
    Some(out)
}

fn html_cell_css(wb: &Workbook, sh: &Sheet, c: CellRef, dir: CellDir) -> String {
    let st = wb.styles.get(sh.style_id(c));
    let mut css = String::new();
    if st.font.bold {
        css.push_str("font-weight:bold;");
    }
    if st.font.italic {
        css.push_str("font-style:italic;");
    }
    if let Some(h) = st.font.color.hex(&wb.theme) {
        let _ = write!(css, "color:{h};");
    }
    if st.fill.pattern != gridcraft_model::PatternType::None
        && let Some(h) = st.fill.fg.hex(&wb.theme)
    {
        let _ = write!(css, "background:{h};");
    }
    let v = sh.value(c);
    let align = match st.align.h {
        gridcraft_model::HAlign::Center => Some("center"),
        gridcraft_model::HAlign::Right => Some("right"),
        gridcraft_model::HAlign::Left => Some("left"),
        _ if v.is_number() => Some(if sh.right_to_left { "left" } else { "right" }),
        _ if dir == CellDir::Auto => None,
        _ => Some("left"),
    };
    if let Some(align) = align {
        let _ = write!(css, "text-align:{align};");
    }
    css
}

/// How a cell marks its direction in HTML. Text on a right-to-left sheet, or text with
/// right-to-left letters, follows its first strong character (`dir="auto"`), so General
/// alignment puts Arabic and Hebrew on the right. Numbers on a right-to-left sheet stay left to
/// right so `-5` doesn't read `5-`.
#[derive(Clone, Copy, PartialEq, Eq)]
enum CellDir {
    Inherit,
    Auto,
    Ltr,
}

impl CellDir {
    fn of(sh: &Sheet, c: CellRef, text: &str) -> CellDir {
        if sh.value(c).is_number() {
            if sh.right_to_left { CellDir::Ltr } else { CellDir::Inherit }
        } else if sh.right_to_left || text.chars().any(is_right_to_left_letter) {
            CellDir::Auto
        } else {
            CellDir::Inherit
        }
    }

    fn attr(self) -> &'static str {
        match self {
            CellDir::Inherit => "",
            CellDir::Auto => " dir=\"auto\"",
            CellDir::Ltr => " dir=\"ltr\"",
        }
    }
}

/// Hebrew, Arabic, Syriac, Thaana, N'Ko and their presentation forms.
fn is_right_to_left_letter(c: char) -> bool {
    matches!(c, '\u{0590}'..='\u{08FF}' | '\u{FB1D}'..='\u{FDFF}' | '\u{FE70}'..='\u{FEFF}')
}

fn table_dir(sh: &Sheet) -> &'static str {
    if sh.right_to_left { " dir=\"rtl\"" } else { "" }
}

const CLIPBOARD_HTML_MAX_BYTES: usize = 4 * 1024 * 1024;
const CLIPBOARD_HTML_MAX_CELLS: u64 = 10_000;

/// Most cells the merge index holds: one entry per merged cell inside the exported range.
const MERGE_INDEX_MAX_ENTRIES: usize = 1_000_000;

/// The merge covering each cell of `rect`, as `Sheet::merge_at` would find it (the first in the
/// sheet's list), built once so a table lookup is one probe instead of a scan of every merge.
/// `None` when overlapping merges would cost more than a few visits per cell of `rect`, or merged
/// cells fill more than [`MERGE_INDEX_MAX_ENTRIES`]: a hostile file's merge list can't multiply the
/// work or the memory of a bounded export.
fn merge_index(sh: &Sheet, rect: RangeRef) -> Option<std::collections::HashMap<CellRef, RangeRef>> {
    let mut map = std::collections::HashMap::new();
    let mut left = rect.count().saturating_mul(4);
    for m in &sh.merges {
        let Some(part) = m.intersection(&rect) else { continue };
        left = left.checked_sub(part.count())?;
        for c in part.iter() {
            map.entry(c).or_insert(*m);
        }
        if map.len() > MERGE_INDEX_MAX_ENTRIES {
            return None;
        }
    }
    Some(map)
}

/// An HTML fragment for the same range as `plain_text`, previously produced by
/// `display::range_text`. Large copies keep plain text only; never return a partial table.
pub(crate) fn range_html(wb: &Workbook, sheet: usize, r: RangeRef, plain_text: &str) -> Option<String> {
    let r = crate::display::clipboard_range(r);
    if r.count() > CLIPBOARD_HTML_MAX_CELLS || plain_text.len() > CLIPBOARD_HTML_MAX_BYTES {
        return None;
    }
    let sh = wb.sheet(sheet)?;
    let merges = merge_index(sh, r)?;
    let mut out = ClipboardHtml(String::new());
    write!(out, "<table{} style=\"border-collapse:collapse;font-family:Calibri,Carlito,Arial,sans-serif;font-size:11pt\">", table_dir(sh)).ok()?;
    for row in r.start.row..=r.end.row {
        if sh.is_row_hidden(row) {
            continue;
        }
        out.write_str("<tr>").ok()?;
        for col in r.start.col..=r.end.col {
            let c = CellRef::new(row, col);
            let span = if let Some(merged) = merges.get(&c).and_then(|m| m.intersection(&r)) {
                let first_row = (merged.start.row..=merged.end.row).find(|&row| !sh.is_row_hidden(row))?;
                if c != CellRef::new(first_row, merged.start.col) {
                    continue;
                }
                let rows = (first_row..=merged.end.row).filter(|&row| !sh.is_row_hidden(row)).count();
                format!(" rowspan=\"{rows}\" colspan=\"{}\"", merged.width())
            } else {
                String::new()
            };
            // Use the visible selected cell, even when the merge's original anchor
            // is outside the range or hidden. This matches the plain-text copy.
            let text = crate::display::cell_text(wb, sh, c);
            let dir = CellDir::of(sh, c, &text);
            let css = html_cell_css(wb, sh, c, dir);
            write!(out, "<td style=\"border:1px solid #d4d4d4;padding:2px 4px;white-space:pre-wrap;{css}\"{}{span}>", dir.attr()).ok()?;
            let mut chars = text.chars().peekable();
            while let Some(ch) = chars.next() {
                match ch {
                    '&' => out.write_str("&amp;"),
                    '<' => out.write_str("&lt;"),
                    '>' => out.write_str("&gt;"),
                    '"' => out.write_str("&quot;"),
                    '\r' => {
                        if chars.peek() == Some(&'\n') {
                            chars.next();
                        }
                        out.write_str("<br>")
                    }
                    '\n' => out.write_str("<br>"),
                    _ => out.write_char(ch),
                }
                .ok()?;
            }
            out.write_str("</td>").ok()?;
        }
        out.write_str("</tr>").ok()?;
    }
    out.write_str("</table>").ok()?;
    Some(out.0)
}

/// Enforce the budget while escaping, before HTML expansion can allocate a large string.
struct ClipboardHtml(String);

impl std::fmt::Write for ClipboardHtml {
    fn write_str(&mut self, s: &str) -> std::fmt::Result {
        if s.len() > CLIPBOARD_HTML_MAX_BYTES.saturating_sub(self.0.len()) {
            return Err(std::fmt::Error);
        }
        self.0.push_str(s);
        Ok(())
    }
}

/// Most rows Save as Web Page writes. A sheet whose used range is taller fails instead of losing cells.
const HTML_EXPORT_MAX_ROWS: u32 = 100_001;

/// Rows × columns of one web-page table. Save as Web Page writes every kept row's full used
/// column span into one growing String, so a sparse sheet with a lone far-right cell (or a
/// tall, wide one) would otherwise be gigabytes of `<td></td>` before finishing (#2387).
const HTML_EXPORT_MAX_CELLS: u64 = 4_000_000;

/// Most bytes of one web page. Every cell may hold long text (shared across many cells in the
/// file), so the cell count alone doesn't bound the page; past this the export fails instead.
const HTML_EXPORT_MAX_BYTES: usize = 256 * 1024 * 1024;

fn html_too_big() -> EngineError {
    EngineError::Other(format!("the web page would be over {} MiB. Save as .xlsx or CSV instead.", HTML_EXPORT_MAX_BYTES / (1024 * 1024)))
}

/// A sheet as a standalone HTML table (Save as Web Page). A sheet with pictures gets the sheet's
/// own column widths and row heights so each picture and chart (inline SVG) can sit at its anchor.
pub fn to_html(wb: &Workbook, sheet: usize) -> Result<String> {
    let Some(sh) = wb.sheet(sheet) else { return Ok(String::new()) };
    let fixed = !(sh.images.is_empty() && sh.charts.is_empty());
    let used = sh.used_range();
    let mut out = String::new();
    let _ = write!(
        out,
        "<!doctype html><html><head><meta charset=\"utf-8\"><title>{}</title><style>table{{border-collapse:collapse;font-family:Calibri,Carlito,Arial,sans-serif;font-size:11pt{}}}td{{border:1px solid #d4d4d4;padding:{};white-space:nowrap{}}}</style></head><body>",
        esc(&sh.name),
        if fixed { ";table-layout:fixed" } else { "" },
        if fixed { "0 4px" } else { "2px 4px" },
        if fixed { ";overflow:hidden;line-height:1" } else { "" }
    );
    if fixed {
        out.push_str("<div style=\"position:relative\">");
    }
    let _ = write!(out, "<table{}", table_dir(sh));
    if let (true, Some(r)) = (fixed, used) {
        let width: f32 = (r.start.col..=r.end.col).map(|c| sh.col_width(c)).sum();
        let _ = write!(out, " style=\"width:{width}px\"");
    }
    out.push('>');
    if let Some(r) = used {
        if r.height() > HTML_EXPORT_MAX_ROWS {
            return Err(EngineError::Other(format!(
                "the sheet is too tall for a web page ({} rows, at most {HTML_EXPORT_MAX_ROWS}). Save as .xlsx or CSV instead.",
                r.height()
            )));
        }
        // The table is one growing String: every kept row writes a `<td>` for the whole used
        // column span (unless the sheet has no row that is that wide). A tall, wide sheet
        // (or a sparse one with a lone far-right cell) would be gigabytes before finishing,
        // so the rectangle itself is bounded (#2387).
        if u64::from(r.height()) * u64::from(r.width()) > HTML_EXPORT_MAX_CELLS {
            return Err(EngineError::Other(format!(
                "the sheet is too tall × wide for a web page ({} × {} cells, at most {HTML_EXPORT_MAX_CELLS}). Save as .xlsx or CSV instead.",
                r.height(),
                r.width()
            )));
        }
        let merges = merge_index(sh, r)
            .ok_or_else(|| EngineError::Other("the sheet's merged cells overlap too much, or cover too many cells, for a web page.".into()))?;
        // Visible rows and columns of the table before each line, so a merge's span is counted
        // in constant time whatever the file says its size is.
        let visible_before = |hidden: &dyn Fn(u32) -> bool, from: u32, to: u32| {
            let mut v = Vec::with_capacity((to - from) as usize + 2);
            v.push(0u32);
            for line in from..=to {
                v.push(v.last().copied().unwrap_or(0) + u32::from(!hidden(line)));
            }
            v
        };
        let rows_before = visible_before(&|y| sh.is_row_hidden(y), r.start.row, r.end.row);
        let cols_before = visible_before(&|x| sh.is_col_hidden(x), r.start.col, r.end.col);
        let visible = |before: &[u32], from: u32, a: u32, b: u32| {
            let at = |line: u32| before.get((line - from) as usize).copied().unwrap_or(0);
            at(b + 1) - at(a)
        };
        if fixed {
            out.push_str("<colgroup>");
            for col in (r.start.col..=r.end.col).filter(|&c| !sh.is_col_hidden(c)) {
                let _ = write!(out, "<col style=\"width:{}px\">", sh.col_width(col));
            }
            out.push_str("</colgroup>");
        }
        for row in r.start.row..=r.end.row {
            if sh.is_row_hidden(row) {
                continue;
            }
            if fixed {
                let _ = write!(out, "<tr style=\"height:{}px\">", sh.row_height(row));
            } else {
                out.push_str("<tr>");
            }
            for col in r.start.col..=r.end.col {
                let c = CellRef::new(row, col);
                let merge = merges.get(&c);
                if merge.is_some_and(|m| m.start != c) || (fixed && sh.is_col_hidden(col)) {
                    continue;
                }
                let text = crate::display::cell_text(wb, sh, c);
                let dir = CellDir::of(sh, c, &text);
                let css = html_cell_css(wb, sh, c, dir);
                let span = merge
                    .map(|m| {
                        if fixed {
                            // Spans count the table's visible lines: the merge is clipped to the table.
                            let rows = visible(&rows_before, r.start.row, m.start.row, m.end.row.min(r.end.row));
                            let cols = visible(&cols_before, r.start.col, m.start.col, m.end.col.min(r.end.col));
                            format!(" rowspan=\"{rows}\" colspan=\"{cols}\"")
                        } else {
                            format!(" rowspan=\"{}\" colspan=\"{}\"", m.height(), m.width())
                        }
                    })
                    .unwrap_or_default();
                // The text is escaped only as far as the page still has room for it.
                let Some(escaped) = esc_within(&text, HTML_EXPORT_MAX_BYTES.saturating_sub(out.len())) else { return Err(html_too_big()) };
                let _ = write!(out, "<td style=\"{css}\"{}{span}>{escaped}</td>", dir.attr());
                if out.len() > HTML_EXPORT_MAX_BYTES {
                    return Err(html_too_big());
                }
            }
            out.push_str("</tr>");
        }
    }
    out.push_str("</table>");
    if fixed {
        // Sheet units are CSS pixels; the table starts at the used range, not at A1.
        // Indexed positions: each anchor would otherwise sum every custom row and column before it.
        let ri = LineIndex::new(&sh.rows, sh.default_row_height, gridcraft_core::MAX_ROWS);
        let ci = LineIndex::new(&sh.cols, sh.default_col_width, gridcraft_core::MAX_COLS);
        let (left, top) = used.map_or((0.0, 0.0), |r| (ci.start(r.start.col), ri.start(r.start.row)));
        let place = |a: &gridcraft_model::Anchor| {
            format!(
                "position:absolute;left:{}px;top:{}px;width:{}px;height:{}px",
                ci.start(a.cell.col) - left + f64::from(a.dx),
                ri.start(a.cell.row) - top + f64::from(a.dy),
                a.width.max(0.0),
                a.height.max(0.0)
            )
        };
        for im in &sh.images {
            // The base64 text is a third longer than the picture: refused before it is built.
            if out.len().saturating_add(im.data.len().div_ceil(3).saturating_mul(4)) > HTML_EXPORT_MAX_BYTES {
                return Err(html_too_big());
            }
            // The attributes are escaped only as far as the page has room for them.
            let room = HTML_EXPORT_MAX_BYTES.saturating_sub(out.len());
            let (Some(mime), Some(alt)) = (esc_within(&im.mime, room), esc_within(&im.alt, room)) else { return Err(html_too_big()) };
            let _ = write!(out, "<img src=\"data:{mime};base64,{}\" alt=\"{alt}\" style=\"{}\">", base64_encode(&im.data), place(&im.anchor));
            if out.len() > HTML_EXPORT_MAX_BYTES {
                return Err(html_too_big());
            }
        }
        let mut svg_budget = crate::chart_svg::MAX_PAGE_SVG_BYTES;
        for ch in &sh.charts {
            if let Some(svg) = crate::chart_svg::chart_svg(wb, sheet, ch, &place(&ch.anchor), svg_budget)? {
                svg_budget -= svg.len();
                out.push_str(&svg);
                if out.len() > HTML_EXPORT_MAX_BYTES {
                    return Err(html_too_big());
                }
            }
        }
        out.push_str("</div>");
    }
    out.push_str("</body></html>");
    if out.len() > HTML_EXPORT_MAX_BYTES {
        return Err(html_too_big());
    }
    Ok(out)
}

thread_local! {
    /// How many [`ScriptSandbox`]es are alive on this thread.
    static SCRIPT_SANDBOX: std::cell::Cell<u32> = const { std::cell::Cell::new(0) };
}

/// While one is alive, [`read_file`] and [`write_file`] on this thread refuse every path: a
/// stored script's steps run inside one (#2178), so no command reaches the file system from a
/// script whatever its parameters are called. Dropping it (also on unwind) lifts the refusal.
pub struct ScriptSandbox(std::marker::PhantomData<*const ()>);

impl ScriptSandbox {
    pub fn enter() -> Self {
        let _ = SCRIPT_SANDBOX.try_with(|c| c.set(c.get().saturating_add(1)));
        ScriptSandbox(std::marker::PhantomData)
    }
}

impl Drop for ScriptSandbox {
    fn drop(&mut self) {
        let _ = SCRIPT_SANDBOX.try_with(|c| c.set(c.get().saturating_sub(1)));
    }
}

/// Refuses file access while a [`ScriptSandbox`] is alive (and, failing closed, while the
/// thread is being torn down).
fn script_sandbox_check(path: &str) -> Result<()> {
    if SCRIPT_SANDBOX.try_with(|c| c.get() > 0).unwrap_or(true) {
        return Err(EngineError::Other(format!("{path}: scripts, and macro steps without the permission, can't read or write files")));
    }
    Ok(())
}

/// The largest native file the reader still touches: its own cap plus one, used to stop
/// reads whose bytes arrive faster than metadata can claim (device files, growing files).
#[cfg(not(target_arch = "wasm32"))]
const MAX_READ_BYTES: u64 = 2 * 1024 * 1024 * 1024;

#[cfg(not(target_arch = "wasm32"))]
pub fn read_file(path: &str) -> Result<Vec<u8>> {
    use std::io::Read;
    script_sandbox_check(path)?;
    // Checked before opening: opening a FIFO nobody writes to blocks the calling (GUI) thread.
    // shortcut: a FIFO swapped in between this check and the open still blocks; close that with
    // O_NONBLOCK when the crate takes a libc dependency.
    if !std::fs::metadata(path).map_err(|e| EngineError::Other(format!("{path}: {e}")))?.is_file() {
        return Err(EngineError::Other(format!("{path}: only regular files can be opened")));
    }
    let f = std::fs::File::open(path).map_err(|e| EngineError::Other(format!("{path}: {e}")))?;
    // A symlink to an endless device reports length 0 but streams forever, so metadata
    // alone never bounds the read: require a real file and cap the bytes actually taken.
    if !f.metadata().map_err(|e| EngineError::Other(format!("{path}: {e}")))?.is_file() {
        return Err(EngineError::Other(format!("{path}: only regular files can be opened")));
    }
    let mut buf = Vec::new();
    let mut limited = (&f).take(MAX_READ_BYTES + 1);
    limited.read_to_end(&mut buf).map_err(|e| EngineError::Other(format!("{path}: {e}")))?;
    if buf.len() as u64 > MAX_READ_BYTES {
        return Err(EngineError::Other("files larger than 2 GB aren't supported".into()));
    }
    Ok(buf)
}

#[cfg(target_arch = "wasm32")]
pub fn read_file(path: &str) -> Result<Vec<u8>> {
    script_sandbox_check(path)?;
    Err(EngineError::Other(format!("{path}: no file system in the browser")))
}

#[cfg(not(target_arch = "wasm32"))]
pub fn write_file(path: &str, bytes: &[u8]) -> Result<()> {
    use std::io::Write;
    script_sandbox_check(path)?;
    // Write to a temporary file, then rename: a crash never leaves a half-written workbook.
    // The temporary file is created exclusively, so an existing file or symlink at that
    // path (planted by another local user) makes the save fail instead of truncating the
    // link's target; and it inherits the destination's access restrictions, so saving a
    // private workbook doesn't widen who can read it.
    let tmp = format!("{path}.sctmp");
    let prior_meta = std::fs::metadata(path).ok();
    let prior = prior_meta.as_ref().map(std::fs::Metadata::permissions);
    let mut opts = std::fs::OpenOptions::new();
    opts.write(true).create_new(true);
    // Over an existing file the replacement starts owner-only, whatever the umask: its group is
    // still the saving user's primary one until it is changed below, and a member of that group
    // must not be able to open it (and keep the descriptor) before then.
    #[cfg(unix)]
    if prior.is_some() {
        use std::os::unix::fs::OpenOptionsExt;
        opts.mode(0o600);
    }
    // A brand-new file gets the restrictive default: no group/other write, whatever the umask.
    #[cfg(unix)]
    if prior.is_none() {
        use std::os::unix::fs::OpenOptionsExt;
        opts.mode(0o644);
    }
    let mut f = opts.open(&tmp).map_err(|e| {
        let _ = std::fs::remove_file(&tmp);
        EngineError::Other(format!("{path}: {e}"))
    })?;
    // The replacement belongs to the saving user's primary group: restore the old file's group and
    // then its mode. When the group can't be restored its access is dropped rather than handed to a
    // different group; failing to read that back is a failed save.
    #[cfg(unix)]
    if let Some(m) = &prior_meta {
        use std::os::unix::fs::{MetadataExt, PermissionsExt};
        let _ = std::os::unix::fs::fchown(&f, None, Some(m.gid()));
        let group_kept = f.metadata().map(|now| now.gid() == m.gid()).map_err(|e| {
            let _ = std::fs::remove_file(&tmp);
            EngineError::Other(format!("{path}: {e}"))
        })?;
        // Best effort: a file system that can't change modes keeps the owner-only one. Without the
        // group, the old owner and other bits are kept and the group, setuid and setgid bits dropped.
        let _ = f.set_permissions(std::fs::Permissions::from_mode(restored_mode(m.mode(), group_kept)));
    }
    #[cfg(not(unix))]
    if let Some(p) = &prior {
        let _ = f.set_permissions(p.clone());
    }
    // `sync_all` before the rename: without it a power loss right after the rename can leave
    // the new name pointing at an empty or truncated file while the old workbook is gone.
    // A file system that cannot flush at all (some network mounts; on Apple targets std asks
    // for F_FULLFSYNC, which they refuse) keeps the old behaviour: skip the flush, still rename.
    let synced = |f: &std::fs::File| f.sync_all().or_else(|e| if full_sync_unsupported(&e) { Ok(()) } else { Err(e) });
    f.write_all(bytes).and_then(|()| f.flush()).and_then(|()| synced(&f)).map_err(|e| {
        let _ = std::fs::remove_file(&tmp);
        EngineError::Other(format!("{path}: {e}"))
    })?;
    drop(f);
    std::fs::rename(&tmp, path).map_err(|e| {
        // A write or rename that fails (full disk, the workbook locked by another app) leaves
        // no temporary file beside the user's workbook.
        let _ = std::fs::remove_file(&tmp);
        EngineError::Other(format!("{path}: {e}"))
    })?;
    // Make the rename itself durable. Best effort: the workbook is already in place, so a
    // directory that can't be opened or synced (some file systems) is not a failed save.
    #[cfg(unix)]
    {
        let dir = std::path::Path::new(path).parent().filter(|d| !d.as_os_str().is_empty()).unwrap_or_else(|| std::path::Path::new("."));
        if let Ok(d) = std::fs::File::open(dir) {
            let _ = d.sync_all();
        }
    }
    Ok(())
}

/// True when a flush failed only because the file system cannot do it: on Unix ENOTTY (25, what
/// `fcntl(F_FULLFSYNC)` returns on such volumes; on Windows 25 is ERROR_SEEK, a real failure), on
/// Apple targets ENOTSUP (45, which std leaves uncategorized there), or an unsupported-operation
/// error (EOPNOTSUPP/ENOSYS, and ENOTSUP where it equals EOPNOTSUPP). Any other error is a real
/// failure.
#[cfg(not(target_arch = "wasm32"))]
fn full_sync_unsupported(e: &std::io::Error) -> bool {
    flush_unsupported_on(e, cfg!(unix), cfg!(target_vendor = "apple"))
}

/// [`full_sync_unsupported`] for a given platform, so each platform's raw codes can be tested
/// on any host.
#[cfg(not(target_arch = "wasm32"))]
fn flush_unsupported_on(e: &std::io::Error, unix: bool, apple: bool) -> bool {
    let raw = e.raw_os_error();
    (unix && raw == Some(25)) || (apple && raw == Some(45)) || e.kind() == std::io::ErrorKind::Unsupported
}

/// The mode a saved file gets from the file it replaces (`old`): all of it when its group was
/// restored; without the group, the owner and other bits only.
#[cfg(unix)]
fn restored_mode(old: u32, group_kept: bool) -> u32 {
    if group_kept { old & 0o7777 } else { old & 0o707 }
}

#[cfg(target_arch = "wasm32")]
pub fn write_file(path: &str, _bytes: &[u8]) -> Result<()> {
    script_sandbox_check(path)?;
    Err(EngineError::Other(format!("{path}: no file system in the browser")))
}

const B64: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

pub fn base64_encode(data: &[u8]) -> String {
    let mut out = String::with_capacity(data.len().div_ceil(3) * 4);
    for chunk in data.chunks(3) {
        let b = [chunk.first().copied().unwrap_or(0), chunk.get(1).copied().unwrap_or(0), chunk.get(2).copied().unwrap_or(0)];
        let n = ((b[0] as u32) << 16) | ((b[1] as u32) << 8) | b[2] as u32;
        for i in 0..4 {
            if i <= chunk.len() {
                out.push(B64[((n >> (18 - 6 * i)) & 63) as usize] as char);
            } else {
                out.push('=');
            }
        }
    }
    out
}

pub fn base64_decode(s: &str) -> Option<Vec<u8>> {
    let s = s.split_once(',').filter(|(h, _)| h.starts_with("data:")).map(|(_, b)| b).unwrap_or(s);
    let mut out = Vec::with_capacity(s.len() / 4 * 3);
    let mut buf = 0u32;
    let mut bits = 0;
    for c in s.bytes() {
        if c == b'=' || c.is_ascii_whitespace() {
            continue;
        }
        let v = B64.iter().position(|&x| x == c)? as u32;
        buf = (buf << 6) | v;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((buf >> bits) as u8);
            buf &= (1 << bits) - 1;
        }
    }
    Some(out)
}

/// Pixel size of a PNG or JPEG from its header.
pub fn image_size(data: &[u8]) -> Option<(u32, u32)> {
    if data.starts_with(&[0x89, b'P', b'N', b'G']) {
        let w = u32::from_be_bytes(data.get(16..20)?.try_into().ok()?);
        let h = u32::from_be_bytes(data.get(20..24)?.try_into().ok()?);
        return Some((w, h));
    }
    if data.starts_with(&[0xFF, 0xD8]) {
        let mut i = 2;
        while i + 9 < data.len() {
            if *data.get(i)? != 0xFF {
                i += 1;
                continue;
            }
            let marker = *data.get(i + 1)?;
            let len = u16::from_be_bytes([*data.get(i + 2)?, *data.get(i + 3)?]) as usize;
            if (0xC0..=0xCF).contains(&marker) && !matches!(marker, 0xC4 | 0xC8 | 0xCC) {
                let h = u16::from_be_bytes([*data.get(i + 5)?, *data.get(i + 6)?]) as u32;
                let w = u16::from_be_bytes([*data.get(i + 7)?, *data.get(i + 8)?]) as u32;
                return Some((w, h));
            }
            i += 2 + len;
        }
    }
    None
}

#[cfg(test)]
mod tests;
