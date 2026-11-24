//! Legacy Excel 97-2003 `.xls` (BIFF8) import. `craft-xls` reads the file (the compound file,
//! the Workbook stream, cells, formulas as text, formats, layout and names); this module maps
//! its workbook onto GridCraft's model: formulas parsed, Excel's implicit `@` for legacy
//! formulas, widths and heights in pixels, formats interned as styles, sheet-name rules, the
//! sheet the workbook opens on, print areas and titles, and the warnings worded for GridCraft.
//!
//! Every formula in the file is a legacy (pre-dynamic-array) one, so it is read with Excel's
//! implicit `@` (see `array_formula.rs`), like an `.xlsx` formula saved without `cm`.

use std::sync::Arc;

use craft_xls as x;
use gridcraft_core::{CellError, CellRef, DateSystem, RangeRef, Value};
use gridcraft_model::style::Protection;
use gridcraft_model::{
    Alignment, BorderLine, BorderStyle, Borders, CalcMode, CalcSettings, Cell, CfOperator, Color, DefinedName, ErrorStyle, Fill, Font, Formula,
    HAlign, LineInfo, NumFmt, PatternType, Sheet, Style, StyleId, StyleTable, Underline, VAlign, Validation, ValidationKind, VertAlign, Visibility,
    Workbook,
};

use crate::tables::{col_width_to_px, pt_to_px};
use crate::{FunctionInfo, IoError, ReadReport};

pub(crate) fn invalid(msg: impl Into<String>) -> IoError {
    IoError::Format(format!("XLS: {}", msg.into()))
}

/// The message for an encrypted workbook (a FilePass record).
const ENCRYPTED: &str =
    "this .xls workbook is password-protected (encrypted). GridCraft can't open encrypted .xls files; remove the password in Excel and try again.";

/// Whether the bytes are a compound file holding an Excel workbook stream (BIFF8 `Workbook`
/// or BIFF5 `Book`).
pub(crate) fn is_xls(bytes: &[u8]) -> bool {
    x::is_xls(bytes)
}

/// Reads an Excel 97-2003 workbook, with the default [`FunctionInfo`]. Inspect the report's
/// warnings for what was skipped.
pub fn read_xls(bytes: &[u8]) -> Result<(Workbook, ReadReport), IoError> {
    read_xls_budgeted(bytes, x::Limits::default(), FunctionInfo::default())
}

/// [`read_xls`] with the function library's facts, which decide where the legacy formulas get
/// Excel's implicit `@`.
pub fn read_xls_with(bytes: &[u8], functions: FunctionInfo) -> Result<(Workbook, ReadReport), IoError> {
    read_xls_budgeted(bytes, x::Limits::default(), functions)
}

fn error(e: x::Error) -> IoError {
    match e {
        x::Error::Encrypted => invalid(ENCRYPTED),
        x::Error::Biff5 => invalid("Excel 5.0/95 workbooks (BIFF5) are not supported; save the file as .xlsx in Excel and try again."),
        x::Error::TooLarge(why) => IoError::TooLarge(format!("XLS: {why}")),
        x::Error::Invalid(why) => invalid(why),
        e => invalid(e.to_string()),
    }
}

/// GridCraft's wording of what the read skipped.
fn warning(w: &x::Warning) -> String {
    use x::Warning as W;
    match w {
        W::ShortFormatting => "Some cell formatting records were shorter than expected; the formatting present was kept.".into(),
        W::ShortNumberFormat => "Some number formats were shorter than their declared length; the characters present were kept.".into(),
        W::ChartSheet => "Chart sheets in .xls files are not imported yet and were skipped.".into(),
        W::MacroSheet => "Excel 4.0 macro sheets were skipped.".into(),
        W::OtherSheet => "VBA modules and other non-worksheet sheets were skipped.".into(),
        W::DialogSheet => "Dialog sheets and unreadable sheets were skipped.".into(),
        W::MissingSharedString => "Some cells refer to missing shared strings and were left empty.".into(),
        W::DataTable => "What-if data tables are not supported; their values were kept.".into(),
        W::DamagedObjects => "Some comments or drawing objects in .xls files could not be read and were skipped.".into(),
        W::Charts => "Charts in .xls files are not imported yet.".into(),
        W::PicturesSkipped => "Some pictures in .xls files were skipped.".into(),
        W::ConditionalFormattingSkipped | W::ConditionalFormatting12 => "Some conditional formatting in .xls files was not imported.".into(),
        W::DataValidationSkipped => "Some data validation rules in .xls files could not be read and were skipped.".into(),
        W::DataValidationIgnored => "Some data validation records in .xls files were ignored, as Excel ignores them: they have no DVal record before them, or come after the number of rules it gives.".into(),
        W::Hyperlinks => "Hyperlinks in .xls files are not imported yet.".into(),
        W::PivotTables => "PivotTables in .xls files are not imported yet; their values were kept.".into(),
        W::CellsOutsideGrid(n) => format!("{n} cells outside the grid were skipped."),
        W::FormulaKept(why) => format!("Some formulas use {why}; their saved results were kept as values."),
        W::NameSkipped(why) => format!("Some defined names use {why} and were skipped."),
        W::MacroNames => "Macro names were skipped.".into(),
        w => format!("{w}."),
    }
}

/// [`read_xls_with`] with explicit limits.
pub(crate) fn read_xls_budgeted(bytes: &[u8], limits: x::Limits, functions: FunctionInfo) -> Result<(Workbook, ReadReport), IoError> {
    let read = x::read_with(bytes, limits).map_err(error)?;
    let book = read.workbook;
    let mut report = ReadReport::default();
    for w in &read.warnings {
        report.warn(warning(w));
    }

    let mut wb = Workbook::new();
    wb.sheets.clear();
    wb.date_system = if book.date_1904 { DateSystem::D1904 } else { DateSystem::D1900 };
    // One model style per XF, in file order, as the cells refer to them.
    if let Some(font) = &book.normal_font {
        wb.styles = StyleTable::with_default(Style { font: font_of(font), ..Style::default() });
    }
    let styles: Vec<StyleId> = book.formats.iter().map(|f| wb.styles.intern(style_of(f))).collect();
    wb.cell_styles = book.cell_styles.iter().map(|(name, f)| (name.clone(), style_of(f))).collect();

    let mut names: Vec<&str> = Vec::new();
    for s in &book.sheets {
        wb.check_sheet_name(&s.name, None).map_err(invalid)?;
        if names.iter().any(|n| n.eq_ignore_ascii_case(&s.name)) {
            return Err(invalid("That name is already taken. Try a different one."));
        }
        names.push(&s.name);
    }
    let mut unparsed = false;
    for s in &book.sheets {
        // Report rules craft-xls kept that the adapter cannot represent. craft-xls already warns
        // about records it could not decode.
        for (dropped, skipped) in [
            (!s.conditional_formats.is_empty(), x::Warning::ConditionalFormattingSkipped),
            (s.validations.iter().any(|v| validation_of(v).is_none()), x::Warning::DataValidationSkipped),
        ] {
            if dropped {
                report.warn(warning(&skipped));
            }
        }
        let sheet = sheet_of(s, &styles, &mut unparsed);
        wb.sheets.push(Arc::new(sheet));
    }
    if unparsed {
        report.warn("Some formulas use syntax GridCraft can't parse yet; their saved results were kept as values.");
    }
    wb.calc = calc_of(&book.calc);
    wb.calc.precision_as_displayed = book.precision_as_displayed;
    for n in &book.names {
        wb.names.push(DefinedName { name: n.name.clone(), scope: n.scope, formula: n.formula.clone(), comment: String::new(), hidden: n.hidden });
    }
    // Every .xls formula is a legacy (pre-dynamic-array) one: read with Excel's implicit `@`, like
    // an .xlsx formula without `cm`. Ctrl+Shift+Enter arrays are read as they are.
    let legacy = wb.sheets.iter().enumerate().map(|(si, s)| (si, legacy_cells(s))).collect();
    crate::array_formula::apply_legacy(&mut wb, legacy, functions);
    // The sheet the file was saved on, as Excel opens it, when it was read and is visible; else the
    // first visible sheet.
    let saved = book.active_sheet.filter(|&i| wb.sheets.get(i).is_some_and(|s| s.visibility == Visibility::Visible));
    wb.set_active_sheet(saved.or_else(|| wb.sheets.iter().position(|s| s.visibility == Visibility::Visible)).unwrap_or(0));
    // The overview comes first.
    report.warnings.insert(
        0,
        "Imported from an Excel 97-2003 (.xls) workbook: values, formulas, cell formatting, merged cells, column widths, row heights, defined names and data validation were read. Charts, pictures, comments and other features are not imported yet. Save as .xlsx; saving .xls is not supported.".into(),
    );
    Ok((wb, report))
}

/// The cells holding a formula that is not a Ctrl+Shift+Enter array.
fn legacy_cells(sheet: &Sheet) -> Vec<CellRef> {
    sheet.cells.iter().filter(|(_, c)| c.formula.as_ref().is_some_and(|f| f.array.is_none())).map(|(at, _)| at).collect()
}

fn cell_ref(c: x::CellRef) -> CellRef {
    CellRef::new(c.row, c.col)
}

fn range(r: x::Range) -> RangeRef {
    RangeRef::new(cell_ref(r.first), cell_ref(r.last))
}

fn value(v: &x::Value) -> Value {
    match v {
        x::Value::Number(n) => Value::number(*n),
        x::Value::Text(s) => Value::Text(s.clone()),
        x::Value::Bool(b) => Value::Bool(*b),
        x::Value::Error(e) => Value::Error(CellError::parse(e.as_str()).unwrap_or(CellError::NA)),
        _ => Value::Empty,
    }
}

fn calc_of(c: &x::Calc) -> CalcSettings {
    let mut calc = CalcSettings::default();
    if let Some(mode) = c.mode {
        calc.mode = match mode {
            x::CalcMode::Manual => CalcMode::Manual,
            x::CalcMode::AutomaticExceptTables => CalcMode::AutomaticExceptTables,
            _ => CalcMode::Automatic,
        };
    }
    if let Some(n) = c.max_iterations {
        calc.set_max_iterations(u32::from(n));
    }
    if let Some(on) = c.iterative {
        calc.iterative = on;
    }
    if let Some(d) = c.max_change {
        calc.set_max_change(d);
    }
    calc
}

/// One worksheet. `unparsed` is set when a formula's text is one GridCraft can't parse: the cell
/// keeps its saved result.
fn sheet_of(s: &x::Sheet, styles: &[StyleId], unparsed: &mut bool) -> Sheet {
    let style = |f: Option<u16>| f.and_then(|i| styles.get(usize::from(i)).copied()).unwrap_or(StyleId::DEFAULT);
    let mut sheet = Sheet::new(&s.name);
    sheet.visibility = match s.visibility {
        x::Visibility::Hidden => Visibility::Hidden,
        x::Visibility::VeryHidden => Visibility::VeryHidden,
        _ => Visibility::Visible,
    };
    for (at, c) in &s.cells {
        let formula = c.formula.as_ref().and_then(|f| {
            let mut parsed = Formula::new(&f.text);
            if parsed.expr.is_none() {
                *unparsed = true;
                return None;
            }
            // A one-cell Ctrl+Shift+Enter array keeps its range, as when an .xlsx is read: it
            // evaluates as an array, not with implicit intersection.
            parsed.array = f.array.map(range);
            Some(Arc::new(parsed))
        });
        sheet.cells.set(cell_ref(*at), Cell { value: value(&c.value), formula, style: style(c.format), py: None });
    }
    sheet.merges = s.merges.iter().map(|r| range(*r)).collect();
    if let Some(h) = s.default_row_height {
        sheet.default_row_height = pt_to_px(f64::from(h) / 20.0);
    }
    for (&col, c) in &s.columns {
        let px = col_width_to_px(f64::from(c.width) / 256.0);
        let info = LineInfo {
            size: (c.custom_width || px != sheet.default_col_width).then_some(px),
            hidden: c.hidden,
            outline: c.outline,
            collapsed: c.collapsed,
            style: c.format.map(|f| style(Some(f))).filter(|&id| id != StyleId::DEFAULT),
            ..LineInfo::default()
        };
        if info != LineInfo::default() {
            sheet.cols.insert(u32::from(col), info);
        }
    }
    for (&r, row) in &s.rows {
        let px = pt_to_px(f64::from(row.height) / 20.0);
        let info = LineInfo {
            custom: row.custom_height,
            hidden: row.hidden,
            size: (row.custom_height || (px - sheet.default_row_height).abs() > 0.5).then_some(px),
            outline: row.outline,
            collapsed: row.collapsed,
            style: row.format.map(|f| style(Some(f))).filter(|&id| id != StyleId::DEFAULT),
        };
        if info != LineInfo::default() {
            sheet.rows.insert(r, info);
        }
    }
    if let Some(v) = s.view {
        sheet.show_formulas = v.show_formulas;
        sheet.show_gridlines = v.show_gridlines;
        sheet.show_headings = v.show_headings;
        sheet.show_zeros = v.show_zeros;
        sheet.right_to_left = v.right_to_left;
    }
    sheet.validations = s.validations.iter().filter_map(validation_of).collect();
    if let Some(text) = &s.print_area {
        sheet.print.print_area = crate::read::parse_print_area(text);
    }
    if let Some(text) = &s.print_titles {
        let (rows, cols) = crate::read::parse_print_titles(text);
        sheet.print.title_rows = rows;
        sheet.print.title_cols = cols;
    }
    sheet
}

/// A data validation rule, as `sheet_read` maps a SpreadsheetML `dataValidation`: a list that
/// is not a literal refers to cells (`=…`).
fn validation_of(v: &x::DataValidation) -> Option<Validation> {
    let ranges: Vec<RangeRef> = v.ranges.iter().map(|r| range(*r)).collect();
    if ranges.is_empty() {
        return None;
    }
    let kind = match v.kind {
        x::ValidationType::Whole => ValidationKind::Whole,
        x::ValidationType::Decimal => ValidationKind::Decimal,
        x::ValidationType::List => ValidationKind::List,
        x::ValidationType::Date => ValidationKind::Date,
        x::ValidationType::Time => ValidationKind::Time,
        x::ValidationType::TextLength => ValidationKind::TextLength,
        x::ValidationType::Custom => ValidationKind::Custom,
        _ => ValidationKind::Any,
    };
    let f1 = if kind == ValidationKind::List && !v.formula1.is_empty() && !v.formula1.starts_with('"') {
        format!("={}", v.formula1)
    } else {
        v.formula1.clone()
    };
    Some(Validation {
        ranges,
        kind,
        op: match v.operator {
            x::Operator::NotBetween => CfOperator::NotBetween,
            x::Operator::Equal => CfOperator::Equal,
            x::Operator::NotEqual => CfOperator::NotEqual,
            x::Operator::Greater => CfOperator::Greater,
            x::Operator::Less => CfOperator::Less,
            x::Operator::GreaterOrEqual => CfOperator::GreaterOrEqual,
            x::Operator::LessOrEqual => CfOperator::LessOrEqual,
            _ => CfOperator::Between,
        },
        f1,
        f2: v.formula2.clone(),
        allow_blank: v.allow_blank,
        in_cell_dropdown: v.in_cell_dropdown,
        input_title: v.prompt_title.clone(),
        input_message: v.prompt.clone(),
        show_input: v.show_prompt,
        error_title: v.error_title.clone(),
        error_message: v.error.clone(),
        error_style: match v.error_style {
            x::ErrorStyle::Warning => ErrorStyle::Warning,
            x::ErrorStyle::Information => ErrorStyle::Information,
            _ => ErrorStyle::Stop,
        },
        show_error: v.show_error,
    })
}

fn color(c: x::Color) -> Color {
    match c {
        x::Color::Rgb(rgb) => Color::Rgb(rgb),
        _ => Color::Auto,
    }
}

fn font_of(f: &x::Font) -> Font {
    let mut font = Font::default();
    if let Some(name) = &f.name {
        font.name = name.clone();
    }
    if let Some(size) = f.size {
        font.size = size;
    }
    font.bold = f.bold;
    font.italic = f.italic;
    font.strike = f.strike;
    font.underline = match f.underline {
        x::Underline::Single => Underline::Single,
        x::Underline::Double => Underline::Double,
        x::Underline::SingleAccounting => Underline::SingleAccounting,
        x::Underline::DoubleAccounting => Underline::DoubleAccounting,
        _ => Underline::None,
    };
    font.vert = match f.script {
        x::Script::Superscript => VertAlign::Superscript,
        x::Script::Subscript => VertAlign::Subscript,
        _ => VertAlign::Baseline,
    };
    font.color = color(f.color);
    font.family = f.family;
    font.charset = f.charset;
    font
}

fn border_style(s: x::BorderStyle) -> BorderStyle {
    use x::BorderStyle as B;
    match s {
        B::Thin => BorderStyle::Thin,
        B::Medium => BorderStyle::Medium,
        B::Dashed => BorderStyle::Dashed,
        B::Dotted => BorderStyle::Dotted,
        B::Thick => BorderStyle::Thick,
        B::Double => BorderStyle::Double,
        B::Hair => BorderStyle::Hair,
        B::MediumDashed => BorderStyle::MediumDashed,
        B::DashDot => BorderStyle::DashDot,
        B::MediumDashDot => BorderStyle::MediumDashDot,
        B::DashDotDot => BorderStyle::DashDotDot,
        B::MediumDashDotDot => BorderStyle::MediumDashDotDot,
        B::SlantDashDot => BorderStyle::SlantDashDot,
        _ => BorderStyle::None,
    }
}

fn line(l: x::BorderLine) -> BorderLine {
    let style = border_style(l.style);
    if style == BorderStyle::None {
        return BorderLine::default();
    }
    BorderLine { style, color: color(l.color) }
}

fn pattern(p: x::Pattern) -> PatternType {
    use x::Pattern as P;
    match p {
        P::Solid => PatternType::Solid,
        P::Gray50 => PatternType::Gray50,
        P::Gray75 => PatternType::Gray75,
        P::Gray25 => PatternType::Gray25,
        P::DarkHorizontal => PatternType::DarkHorizontal,
        P::DarkVertical => PatternType::DarkVertical,
        P::DarkDown => PatternType::DarkDown,
        P::DarkUp => PatternType::DarkUp,
        P::DarkGrid => PatternType::DarkGrid,
        P::DarkTrellis => PatternType::DarkTrellis,
        P::LightHorizontal => PatternType::LightHorizontal,
        P::LightVertical => PatternType::LightVertical,
        P::LightDown => PatternType::LightDown,
        P::LightUp => PatternType::LightUp,
        P::LightGrid => PatternType::LightGrid,
        P::LightTrellis => PatternType::LightTrellis,
        P::Gray125 => PatternType::Gray125,
        P::Gray0625 => PatternType::Gray0625,
        _ => PatternType::None,
    }
}

/// The model style of one XF.
fn style_of(f: &x::CellFormat) -> Style {
    let a = f.align;
    let b = f.borders;
    Style {
        font: font_of(&f.font),
        num_fmt: if f.number_format.eq_ignore_ascii_case("General") { NumFmt::default() } else { NumFmt::new(&f.number_format) },
        protection: Protection { locked: f.locked, hidden: f.hidden },
        align: Alignment {
            h: match a.h {
                x::HAlign::Left => HAlign::Left,
                x::HAlign::Center => HAlign::Center,
                x::HAlign::Right => HAlign::Right,
                x::HAlign::Fill => HAlign::Fill,
                x::HAlign::Justify => HAlign::Justify,
                x::HAlign::CenterAcross => HAlign::CenterAcross,
                x::HAlign::Distributed => HAlign::Distributed,
                _ => HAlign::General,
            },
            v: match a.v {
                x::VAlign::Top => VAlign::Top,
                x::VAlign::Center => VAlign::Center,
                x::VAlign::Justify => VAlign::Justify,
                x::VAlign::Distributed => VAlign::Distributed,
                _ => VAlign::Bottom,
            },
            wrap: a.wrap,
            shrink: a.shrink,
            indent: a.indent,
            rotation: a.rotation,
            reading_order: None,
        },
        borders: Borders {
            left: line(b.left),
            right: line(b.right),
            top: line(b.top),
            bottom: line(b.bottom),
            diag_down: line(b.diag_down),
            diag_up: line(b.diag_up),
        },
        fill: match pattern(f.fill.pattern) {
            PatternType::None => Fill::default(),
            PatternType::Solid => Fill::solid(color(f.fill.fg)),
            p => Fill { pattern: p, fg: color(f.fill.fg), bg: color(f.fill.bg) },
        },
        ..Style::default()
    }
}
