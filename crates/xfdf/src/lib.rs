//! Comment and form data exchange (execution plan M5.6, M6.7). Layer L4.
//!
//! - **XFDF** (ISO 19444-1, Acrobat's "Export all to data file" / "Import comments", and form
//!   data): comments with their geometry, colours, flags, authors, dates, replies, pop-ups,
//!   ink, quadrilaterals, line ends and callouts; field values, nested by name.
//! - **FDF** (ISO 32000-2 §12.7.8): fields and comments in PDF syntax.
//! - Form data as **XML**, **CSV** and **tab-delimited text** (Acrobat's Export data formats).
//!
//! Import merges: a comment whose name (`/NM`) already exists on its page replaces it; field
//! values go through the form's own checks (formats, validation) and recalculate.

#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

use std::collections::{HashMap, HashSet};
use std::fmt::Write as _;
use std::io::Write as _;

use pdfcraft_cos::{Dict, Document, ObjRef, Object, PdfString};
use pdfcraft_forms::{FieldKind, FieldValue};

mod common;
mod fdf;
mod xfdf;

#[cfg(test)]
use common::{MAX_DEPTH, MAX_XML_DEPTH, arr, nums_of, parse_hex, text_of, xml_nesting_depth};
use common::{check_form_xml_depth, esc, parse_form_xml, walk_xfdf, xml_name};
pub use fdf::export_fdf;
use fdf::{ImportBudget, fdf_body, import_fdf, walk_fdf};
#[cfg(test)]
use fdf::{MAX_FDF_BYTES, MAX_FDF_OBJECTS};
pub use xfdf::export_xfdf;
#[cfg(test)]
use xfdf::{LOCKED, find_by_name};
use xfdf::{apply_values, import_xfdf};

#[cfg(test)]
mod tests;

#[derive(Debug, thiserror::Error, PartialEq)]
pub enum DataError {
    #[error("this file isn't XFDF, FDF, XML or text form data")]
    UnknownFormat,
    #[error("the file can't be read: {0}")]
    Malformed(String),
    #[error("nothing in the file matches this document")]
    NothingImported,
    #[error("FDF export exceeds the {0} limit; no data was exported")]
    ExportLimit(&'static str),
    #[error(transparent)]
    Cos(#[from] pdfcraft_cos::CosError),
}

/// What export writes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Format {
    Xfdf,
    Fdf,
    Xml,
    Csv,
    Txt,
    /// An XFA form's data as an XML Data Package (the engine writes it; for a form without
    /// XFA packets it is the XML format).
    Xdp,
}

impl Format {
    pub fn from_extension(ext: &str) -> Option<Format> {
        Some(match ext.to_ascii_lowercase().as_str() {
            "xfdf" => Format::Xfdf,
            "fdf" => Format::Fdf,
            "xml" => Format::Xml,
            "xdp" => Format::Xdp,
            "csv" => Format::Csv,
            "txt" | "tab" | "tsv" => Format::Txt,
            _ => return None,
        })
    }
}

/// What import did.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Report {
    pub comments: usize,
    pub fields: usize,
    /// Field values the form refused (format, validation, unknown options).
    pub rejected: Vec<String>,
}

/// Form data as XML, CSV or tab-delimited text (Acrobat's Export data formats).
pub fn export_data(doc: &Document, format: Format) -> String {
    let fields: Vec<(String, String)> = pdfcraft_forms::fields(doc)
        .into_iter()
        .filter(|f| !matches!(f.kind, FieldKind::PushButton | FieldKind::Signature))
        .map(|f| {
            let v = match f.kind {
                FieldKind::CheckBox | FieldKind::Radio if f.value.is_empty() => "Off".to_string(),
                _ => f.export_values().join(", "),
            };
            (f.name, v)
        })
        .collect();
    match format {
        Format::Xml | Format::Xdp => {
            let mut out = String::from("<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<fields xmlns:xfdf=\"http://ns.adobe.com/xfdf-transition/\">\n");
            for (k, v) in &fields {
                let tag = xml_name(k);
                let orig = if tag == *k { String::new() } else { format!(" xfdf:original=\"{}\"", esc(k)) };
                let _ = writeln!(out, "<{tag}{orig}>{}</{tag}>", esc(v));
            }
            out.push_str("</fields>\n");
            out
        }
        Format::Csv => {
            // A formula-like cell gets a leading `'`, as in `merge_csv`; import takes it off again.
            let q = |s: &str| {
                let s = if csv_guarded(s) { format!("'{s}") } else { s.to_string() };
                if s.contains([',', '"', '\n', '\r', '\t']) { format!("\"{}\"", s.replace('"', "\"\"")) } else { s }
            };
            format!(
                "{}\n{}\n",
                fields.iter().map(|f| q(&f.0)).collect::<Vec<_>>().join(","),
                fields.iter().map(|f| q(&f.1)).collect::<Vec<_>>().join(",")
            )
        }
        _ => {
            let clean = |s: &str| s.replace(['\t', '\n', '\r'], " ");
            format!(
                "{}\n{}\n",
                fields.iter().map(|f| clean(&f.0)).collect::<Vec<_>>().join("\t"),
                fields.iter().map(|f| clean(&f.1)).collect::<Vec<_>>().join("\t")
            )
        }
    }
}

/// The field values in a form data file (FDF or XFDF) or a filled-in PDF form, in file order.
pub fn data_values(bytes: &[u8]) -> Result<Vec<(String, Vec<String>)>, DataError> {
    if bytes.starts_with(b"%FDF") {
        let fdf = Document::open(std::sync::Arc::new(bytes.to_vec())).map_err(|e| DataError::Malformed(e.to_string()))?;
        let body = fdf_body(&fdf)?;
        let mut values = Vec::new();
        let mut budget = ImportBudget::new();
        if let Some(list) = body.get(b"Fields").map(|f| fdf.resolve(f)).and_then(|f| f.as_array().cloned()) {
            walk_fdf(&fdf, &list, "", &mut values, 0, &mut Vec::new(), &mut budget)?;
        }
        return Ok(values);
    }
    if bytes.starts_with(b"%PDF") || bytes.windows(5).take(1024).any(|w| w == b"%PDF-") {
        let doc = Document::open(std::sync::Arc::new(bytes.to_vec())).map_err(|e| DataError::Malformed(e.to_string()))?;
        return Ok(pdfcraft_forms::fields(&doc)
            .into_iter()
            .filter(|f| !matches!(f.kind, FieldKind::PushButton | FieldKind::Signature))
            .map(|f| {
                let v = if matches!(f.kind, FieldKind::CheckBox | FieldKind::Radio) && f.value.is_empty() {
                    vec!["Off".to_string()]
                } else {
                    f.export_values()
                };
                (f.name, v)
            })
            .collect());
    }
    let text = String::from_utf8_lossy(bytes);
    let t = text.trim_start_matches('\u{feff}').trim_start();
    check_form_xml_depth(t)?;
    let xml = parse_form_xml(t).map_err(|_| DataError::UnknownFormat)?;
    let root = xml.root_element();
    if root.tag_name().name() != "xfdf" {
        return Err(DataError::UnknownFormat);
    }
    let mut values = Vec::new();
    if let Some(fields) = root.children().find(|c| c.has_tag_name("fields")) {
        walk_xfdf(fields, "", &mut values, 0)?;
    }
    Ok(values)
}

/// Merge Data Files into Spreadsheet: one CSV row per file, one column per field name (in the
/// order they first appear).
///
/// The names and values come from other people's files, and spreadsheet programs evaluate a cell
/// that starts with `=`, `+`, `-` or `@` (after any leading whitespace) as a formula. Such a cell
/// is written with a leading `'`, which keeps it text; a plain number such as `-12.5` is not.
pub fn merge_csv(files: &[Vec<(String, Vec<String>)>]) -> String {
    let mut columns: Vec<&str> = Vec::new();
    for f in files {
        for (k, _) in f {
            if !columns.contains(&k.as_str()) {
                columns.push(k);
            }
        }
    }
    let literal = |s: &str| if runs_as_formula(s) { format!("'{s}") } else { s.to_string() };
    let q = |s: &str| {
        let s = literal(s);
        if s.contains([',', '"', '\n', '\r', '\t']) { format!("\"{}\"", s.replace('"', "\"\"")) } else { s }
    };
    let mut out = columns.iter().map(|c| q(c)).collect::<Vec<_>>().join(",");
    out.push('\n');
    for f in files {
        let row: Vec<String> = columns.iter().map(|c| f.iter().find(|(k, _)| k == c).map_or(String::new(), |(_, v)| q(&v.join(", ")))).collect();
        out.push_str(&row.join(","));
        out.push('\n');
    }
    out
}

/// Whether a spreadsheet program runs CSV cell `s` as a formula: after any leading whitespace it
/// starts with `=` or `@`, or with `+` or `-` and is not a plain number.
fn runs_as_formula(s: &str) -> bool {
    let t = s.trim_start();
    match t.chars().next() {
        Some('=' | '@') => true,
        Some('+' | '-') => !t.parse::<f64>().is_ok_and(f64::is_finite),
        _ => false,
    }
}

/// Whether Export Data writes CSV cell `s` with a leading `'`: it runs as a formula, or it already
/// starts with `'` in front of one. That second case gets one more `'`, so [`csv_text`] can take
/// exactly one off on import and every value comes back unchanged.
fn csv_guarded(s: &str) -> bool {
    runs_as_formula(s.trim_start_matches('\''))
}

/// A CSV cell's text without the `'` that Export Data added (see [`csv_guarded`]).
fn csv_text(cell: String) -> String {
    match cell.strip_prefix('\'') {
        Some(rest) if csv_guarded(rest) => rest.to_string(),
        _ => cell,
    }
}

// Inspect the first logical record: tabs within quoted CSV cells are data. A quote
// in the middle of an unquoted TSV name stays literal, as in the TSV reader.
fn has_tab_delimiter(text: &str) -> bool {
    let mut quoted = false;
    let mut cell_start = true;
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '"' if quoted && chars.peek() == Some(&'"') => {
                chars.next();
            }
            '"' if quoted || cell_start => {
                quoted = !quoted;
                cell_start = false;
            }
            '\t' if !quoted => return true,
            '\r' | '\n' if !quoted => return false,
            ',' if !quoted => cell_start = true,
            _ => cell_start = false,
        }
    }
    // An unclosed quote can be a literal TSV field-name character.
    quoted && text.lines().next().is_some_and(|line| line.contains('\t'))
}

/// Form data from XML, CSV or tab-delimited text.
fn import_table(doc: &mut Document, text: &str) -> Result<Report, DataError> {
    let mut report = Report::default();
    let t = text.trim_start_matches('\u{feff}');
    let values: Vec<(String, Vec<String>)> = if t.trim_start().starts_with('<') {
        check_form_xml_depth(t)?;
        let xml = parse_form_xml(t).map_err(|e| DataError::Malformed(e.to_string()))?;
        xml.root_element()
            .children()
            .filter(|c| c.is_element())
            .map(|c| {
                let name =
                    c.attributes().find(|a| a.name() == "original").map(|a| a.value().to_string()).unwrap_or_else(|| c.tag_name().name().to_string());
                // Comments and processing instructions do not end a field's character data.
                (name, vec![c.children().filter(|node| node.is_text()).filter_map(|node| node.text()).collect()])
            })
            .collect()
    } else if has_tab_delimiter(t) {
        tab_delimited(t)?
    } else {
        // CSV with quotes: a quoted value may hold commas, doubled quotes and line breaks.
        let mut records: Vec<(Vec<String>, bool)> = Vec::new();
        let mut record = Vec::new();
        let mut cur = String::new();
        let mut quoted = false;
        let mut has_quotes = false;
        let mut chars = t.chars().peekable();
        while let Some(c) = chars.next() {
            match c {
                '"' if quoted && chars.peek() == Some(&'"') => {
                    cur.push('"');
                    chars.next();
                }
                '"' => {
                    quoted = !quoted;
                    has_quotes = true;
                }
                ',' if !quoted => record.push(std::mem::take(&mut cur)),
                '\r' if !quoted && chars.peek() == Some(&'\n') => {}
                '\n' if !quoted => {
                    record.push(std::mem::take(&mut cur));
                    records.push((std::mem::take(&mut record), std::mem::take(&mut has_quotes)));
                }
                c => cur.push(c),
            }
        }
        record.push(cur);
        records.push((record, has_quotes));
        // Blank lines (such as the one a trailing line break leaves) are not records, but a
        // quoted empty value is a record that clears a field.
        let mut records =
            records.into_iter().filter(|(r, has_quotes)| *has_quotes || !matches!(r.as_slice(), [only] if only.is_empty())).map(|(r, _)| r);
        let (Some(head), Some(row)) = (records.next(), records.next()) else { return Err(DataError::UnknownFormat) };
        // Text exported from a one-field form has no tab to show for it (#1321). CSV quotes a
        // value with a comma, so one name over several unquoted cells is that text, not CSV.
        if head.len() == 1 && row.len() > 1 {
            tab_delimited(t)?
        } else {
            head.into_iter().zip(row).map(|(k, v)| (csv_text(k), vec![csv_text(v)])).collect()
        }
    };
    apply_values(doc, &values, &mut report);
    Ok(report)
}

/// Tab-delimited text: a row of names, a row of values.
fn tab_delimited(t: &str) -> Result<Vec<(String, Vec<String>)>, DataError> {
    let mut lines = t.lines();
    let (Some(head), Some(row)) = (lines.next(), lines.next()) else { return Err(DataError::UnknownFormat) };
    Ok(head.split('\t').map(str::to_string).zip(row.split('\t')).map(|(k, v)| (k, vec![v.to_string()])).collect())
}

/// Import comments and/or field values from XFDF, FDF, XML, CSV or tab-delimited text
/// (detected from the content).
pub fn import(doc: &mut Document, bytes: &[u8]) -> Result<Report, DataError> {
    let report = if bytes.starts_with(b"%FDF") {
        import_fdf(doc, bytes)?
    } else {
        let text = String::from_utf8_lossy(bytes);
        let t = text.trim_start_matches('\u{feff}').trim_start();
        if t.starts_with("<?xml") || t.starts_with("<xfdf") || t.starts_with('<') {
            match import_xfdf(doc, t) {
                Err(DataError::UnknownFormat) => import_table(doc, t)?,
                other => other?,
            }
        } else {
            import_table(doc, &text)?
        }
    };
    if report.comments == 0 && report.fields == 0 && report.rejected.is_empty() {
        return Err(DataError::NothingImported);
    }
    Ok(report)
}
