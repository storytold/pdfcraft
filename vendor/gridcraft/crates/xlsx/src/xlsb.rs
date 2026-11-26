//! XLSB import, delegated to `craft_xlsx::Package`.
//! Cell values, formulas, defined names, and merged cells are imported; formatting, charts,
//! images, macros and other workbook features are not imported.

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::sync::Arc;

use gridcraft_core::{CellError, CellRef, DateSystem, RangeRef, Value};
use gridcraft_model::{Cell, DefinedName, Formula, Preserved, PreservedPart, PreservedRel, PreservedXml, Sheet, StyleId, Visibility, Workbook};

use crate::write::{NS_MAIN, NS_REL, XML_DECL};
use crate::xml::{esc, esc_attr};
use crate::{IoError, ReadReport};

fn map_error(err: craft_xlsx::Error) -> IoError {
    match err {
        craft_xlsx::Error::Zip(m) => IoError::Zip(m),
        craft_xlsx::Error::Xml(m) => IoError::Xml(m),
        craft_xlsx::Error::Invalid(m) => IoError::Format(m),
        craft_xlsx::Error::TooLarge(m) => IoError::TooLarge(m),
    }
}

fn map_cell_error(err: craft_xlsx::CellError) -> CellError {
    match err {
        craft_xlsx::CellError::Null => CellError::Null,
        craft_xlsx::CellError::Div0 => CellError::Div0,
        craft_xlsx::CellError::Value => CellError::Value,
        craft_xlsx::CellError::Ref => CellError::Ref,
        craft_xlsx::CellError::Name => CellError::Name,
        craft_xlsx::CellError::Num => CellError::Num,
        craft_xlsx::CellError::NA => CellError::NA,
        craft_xlsx::CellError::GettingData => CellError::GettingData,
        craft_xlsx::CellError::Spill => CellError::Spill,
        craft_xlsx::CellError::Calc => CellError::Calc,
        craft_xlsx::CellError::Blocked => CellError::Blocked,
        craft_xlsx::CellError::Python => CellError::Python,
        craft_xlsx::CellError::Busy => CellError::Busy,
        craft_xlsx::CellError::Timeout => CellError::Timeout,
        _ => CellError::Value,
    }
}

fn map_value(value: craft_xlsx::Value) -> Value {
    match value {
        craft_xlsx::Value::Empty => Value::Empty,
        craft_xlsx::Value::Number(n) => Value::number(n),
        craft_xlsx::Value::Text(t) => Value::Text(t),
        craft_xlsx::Value::Bool(b) => Value::Bool(b),
        craft_xlsx::Value::Error(e) => Value::Error(map_cell_error(e)),
    }
}

fn map_visibility(visibility: craft_xlsx::Visibility) -> Visibility {
    match visibility {
        craft_xlsx::Visibility::Visible => Visibility::Visible,
        craft_xlsx::Visibility::Hidden => Visibility::Hidden,
        craft_xlsx::Visibility::VeryHidden => Visibility::VeryHidden,
    }
}

pub(super) fn import_limits() -> craft_xlsx::Limits {
    let limits = craft_xlsx::Limits::default();
    // Keep the lower decoded-text budget in the browser's WebAssembly heap.
    if cfg!(target_arch = "wasm32") { craft_xlsx::Limits { binary_text: 64 << 20, ..limits } } else { limits }
}

fn map_formula(f: &craft_xlsx::CellFormula, at: craft_xlsx::CellRef, report: &mut ReadReport) -> Option<Formula> {
    let text = crate::fmla::from_file(f.text.trim());
    if text.is_empty() {
        return None;
    }
    match f.kind {
        craft_xlsx::FormulaKind::Normal | craft_xlsx::FormulaKind::Shared => Some(Formula::new(&text)),
        craft_xlsx::FormulaKind::Array => {
            let pos = CellRef::new(at.row, at.col);
            let mut fm = Formula::new(&text);
            fm.array = Some(
                f.range
                    .map(|r| RangeRef::new(CellRef::new(r.first.row, r.first.col), CellRef::new(r.last.row, r.last.col)))
                    .unwrap_or_else(|| RangeRef::cell(pos)),
            );
            Some(fm)
        }
        craft_xlsx::FormulaKind::DataTable => {
            report.warn("what-if data tables are not supported; kept their values");
            None
        }
    }
}

fn import_defined_names(book: &craft_xlsx::Workbook, file_to_model: &[Option<usize>], wb: &mut Workbook) {
    for d in &book.defined_names {
        let scope = match d.local_sheet_id {
            Some(i) => match file_to_model.get(i as usize).copied().flatten() {
                Some(m) => Some(m),
                None => continue,
            },
            None => None,
        };
        let lname = d.name.to_ascii_lowercase();
        if lname == "_xlnm._filterdatabase" || lname.starts_with("_xlchart.") || lname == "_xlnm.print_area" || lname == "_xlnm.print_titles" {
            continue;
        }
        wb.names.push(DefinedName {
            name: d.name.clone(),
            scope,
            formula: crate::fmla::from_file(d.formula.trim()),
            comment: d.comment.clone(),
            hidden: d.hidden,
        });
    }
}

/// Imports worksheet cell values, formulas, defined names, and merged cells. Number formats are
/// omitted; dates remain serial numbers with the workbook's original date system. Inspect warnings.
pub fn read_xlsb(bytes: &[u8]) -> Result<(Workbook, ReadReport), IoError> {
    let limits = import_limits();
    let mut package = craft_xlsx::Package::open(bytes, &limits).map_err(map_error)?;
    if !package.is_binary() {
        return Err(IoError::Format("XLSB: not a binary workbook".into()));
    }
    let ct_bytes = package
        .opc()
        .read("[Content_Types].xml")
        .map_err(|e| IoError::Format(format!("XLSB: {e}")))?
        .ok_or_else(|| IoError::Format("XLSB: missing [Content_Types].xml".into()))?;
    let canonical = craft_xml::Canonical::new(&[("ct", craft_opc::NS_CONTENT_TYPES)]);
    let ct_el = craft_xml::Options::canonical(&canonical).strict().parse(&ct_bytes).map_err(|e| IoError::Xml(e.to_string()))?;
    if ct_el.name != "ct:Types" {
        return Err(IoError::Format("XLSB: wrong [Content_Types].xml namespace".into()));
    }
    if let Some(root_rels) = package.opc().read("_rels/.rels").map_err(|e| IoError::Format(format!("XLSB: {e}")))? {
        craft_xml::Options::local().strict().parse(&root_rels).map_err(|e| IoError::Xml(e.to_string()))?;
    }
    let book_part = package
        .opc()
        .rels("")
        .map_err(|e| IoError::Format(format!("XLSB: {e}")))?
        .first("officeDocument")
        .map(|r| r.target.clone())
        .unwrap_or_else(|| "xl/workbook.bin".into());
    let book_rels_path =
        if let Some((dir, file)) = book_part.rsplit_once('/') { format!("{dir}/_rels/{file}.rels") } else { format!("_rels/{book_part}.rels") };
    if let Some(book_rels) = package.opc().read(&book_rels_path).map_err(|e| IoError::Format(format!("XLSB: {e}")))? {
        craft_xml::Options::local().strict().parse(&book_rels).map_err(|e| IoError::Xml(e.to_string()))?;
    }
    let ct = craft_opc::ContentTypes::parse(&ct_bytes);
    let book = package.workbook().map_err(map_error)?;
    let strings = package.shared_strings().map_err(map_error)?;
    let mut wb = Workbook::new();
    wb.sheets.clear();
    wb.date_system = if book.date_1904 { DateSystem::D1904 } else { DateSystem::D1900 };
    let mut report = ReadReport::default();
    report.warn("XLSB import: formatting, charts, images, macros and other features are not imported; dates, currency and percentages may display as numbers. Save a new .xlsx file; XLSB saving is not supported.");
    let mut file_to_model = Vec::with_capacity(book.sheets.len());
    for entry in &book.sheets {
        if entry.kind != craft_xlsx::SheetKind::Worksheet {
            if !matches!(entry.kind, craft_xlsx::SheetKind::Chartsheet | craft_xlsx::SheetKind::Dialogsheet | craft_xlsx::SheetKind::Macrosheet) {
                return Err(IoError::Format("XLSB: unknown sheet relationship type".into()));
            }
            report.warn("XLSB non-worksheet sheets were skipped.");
            file_to_model.push(None);
            continue;
        }
        wb.check_sheet_name(&entry.name, None).map_err(|e| IoError::Format(format!("XLSB: {e}")))?;
        if let Some(ref part_name) = entry.part
            && ct.of(part_name) != Some("application/vnd.ms-excel.worksheet")
        {
            return Err(IoError::Format("XLSB: wrong worksheet content type".into()));
        }
        let mut sheet = Sheet::new(&entry.name);
        sheet.visibility = map_visibility(entry.visibility);
        let part = package
            .sheet(entry, |row| {
                for cell in row.cells {
                    let value = map_value(cell.value.resolve(&strings, book.date_1904));
                    let formula = cell.formula.as_ref().and_then(|f| map_formula(f, cell.at, &mut report));
                    if !value.is_empty() || formula.is_some() {
                        sheet.cells.set(
                            CellRef::new(cell.at.row, cell.at.col),
                            Cell { value, formula: formula.map(Arc::new), style: StyleId::DEFAULT, py: None },
                        );
                    }
                }
                Ok(())
            })
            .map_err(map_error)?;
        sheet.merges =
            part.merges.into_iter().map(|r| RangeRef::new(CellRef::new(r.first.row, r.first.col), CellRef::new(r.last.row, r.last.col))).collect();
        file_to_model.push(Some(wb.sheets.len()));
        wb.sheets.push(Arc::new(sheet));
    }
    import_defined_names(&book, &file_to_model, &mut wb);
    let external_links = package.external_links(&book).map_err(map_error)?;
    let (preserved, ordinal_map) = preserve_external_links(&external_links, &mut report);
    if let Some(preserved) = preserved {
        wb.preserved = Some(Arc::new(preserved));
    }
    if let Some(remap) = ordinal_map {
        remap_workbook_external_ordinals(&mut wb, &remap, &mut report);
    }
    for warning in package.warnings() {
        report.warn(warning);
    }
    if wb.sheets.is_empty() {
        return Err(IoError::Format("XLSB: no supported worksheets".into()));
    }
    wb.set_active_sheet(wb.sheets.iter().position(|s| s.visibility == Visibility::Visible).unwrap_or(0));
    Ok((wb, report))
}

pub(crate) fn is_xlsb(bytes: &[u8]) -> bool {
    craft_xlsx::is_xlsb(bytes)
}

fn preserve_external_links(
    links: &[(String, Option<craft_xlsx::ExternalLink>)],
    report: &mut ReadReport,
) -> (Option<Preserved>, Option<Vec<Option<usize>>>) {
    if links.is_empty() {
        return (None, None);
    }
    const CT_EXT_LINK: &str = "application/vnd.openxmlformats-officedocument.spreadsheetml.externalLink+xml";
    let mut p = Preserved::default();
    let mut refs_xml = format!("<externalReferences xmlns:r=\"{NS_REL}\">");
    let mut remap = vec![None; links.len() + 1];
    let mut next_ordinal = 0usize;
    let mut had_unpreserved = false;
    for (i, (_, link)) in links.iter().enumerate() {
        let Some(craft_xlsx::ExternalLink::Book(book)) = link else {
            had_unpreserved = true;
            continue;
        };
        if book.target.is_none() {
            had_unpreserved = true;
            continue;
        }
        next_ordinal += 1;
        let old_ordinal = i + 1;
        remap[old_ordinal] = Some(next_ordinal);
        let rid = format!("rId{next_ordinal}");
        let part_name = format!("xl/externalLinks/externalLink{next_ordinal}.xml");
        let (xml, rels) = external_link_part_xml(book);
        p.rels.push(PreservedRel { id: rid.clone(), rel_type: format!("{NS_REL}/externalLink"), target: part_name.clone(), external: false });
        p.parts.push(PreservedPart { name: part_name, content_type: CT_EXT_LINK.into(), data: xml.into_bytes().into(), rels });
        let _ = write!(refs_xml, "<externalReference r:id=\"{rid}\"/>");
    }
    if had_unpreserved {
        report.warn("XLSB: unresolved or non-workbook external links kept their cached values; their targets could not be preserved");
    }
    let preserved = if next_ordinal == 0 {
        None
    } else {
        refs_xml.push_str("</externalReferences>");
        p.elements.push(PreservedXml { slot: "externalReferences".into(), xml: refs_xml });
        Some(p)
    };
    (preserved, had_unpreserved.then_some(remap))
}

fn remap_workbook_external_ordinals(wb: &mut Workbook, remap: &[Option<usize>], report: &mut ReadReport) {
    let mut dropped = false;
    for sheet_arc in &mut wb.sheets {
        let sheet = Arc::make_mut(sheet_arc);
        let coords: Vec<CellRef> = sheet.cells.iter().filter(|(_, c)| c.formula.is_some()).map(|(pos, _)| pos).collect();
        for pos in coords {
            let Some(cell) = sheet.cells.get_mut(pos) else { continue };
            let Some(fm) = &cell.formula else { continue };
            match remap_external_formula(&fm.text, remap) {
                Ok(Some(rewritten)) => {
                    let mut updated = (**fm).clone();
                    updated.text = rewritten;
                    cell.formula = Some(Arc::new(updated));
                }
                Ok(None) => {}
                Err(()) => {
                    cell.formula = None;
                    dropped = true;
                }
            }
        }
    }
    let mut kept_names = Vec::with_capacity(wb.names.len());
    for mut name in std::mem::take(&mut wb.names) {
        match remap_external_formula(&name.formula, remap) {
            Ok(Some(rewritten)) => {
                name.formula = rewritten;
                kept_names.push(name);
            }
            Ok(None) => kept_names.push(name),
            Err(()) => dropped = true,
        }
    }
    wb.names = kept_names;
    if dropped {
        report.warn("XLSB: formulas referencing unresolved external links were replaced with their cached values");
    }
}

pub(crate) fn remap_external_formula(text: &str, remap: &[Option<usize>]) -> Result<Option<String>, ()> {
    let bytes = text.as_bytes();
    let mut out: Option<String> = None;
    let mut i = 0;
    let mut copied = 0;
    let mut in_string = false;
    while i < bytes.len() {
        if bytes[i] == b'"' {
            in_string = !in_string;
            i += 1;
            continue;
        }
        if !in_string && bytes[i] == b'[' {
            let mut j = i + 1;
            while j < bytes.len() && bytes[j].is_ascii_digit() {
                j += 1;
            }
            if j > i + 1
                && j < bytes.len()
                && bytes[j] == b']'
                && is_external_ordinal(text, i, j)
                && let Ok(old) = text[i + 1..j].parse::<usize>()
            {
                let Some(&Some(new_n)) = remap.get(old) else {
                    return Err(());
                };
                if new_n != old {
                    let buf = out.get_or_insert_with(|| String::with_capacity(text.len()));
                    buf.push_str(&text[copied..=i]);
                    let _ = write!(buf, "{new_n}]");
                    i = j + 1;
                    copied = i;
                    continue;
                }
            }
        }
        i += 1;
    }
    if let Some(mut buf) = out {
        buf.push_str(&text[copied..]);
        Ok(Some(buf))
    } else {
        Ok(None)
    }
}

/// True when `[digits]` at `open..=close` is an external-book ordinal (`[1]Sheet!A1`,
/// `'[1]Other Sheet'!A1`, `[1]!Name`) rather than a numeric structured-table column (`Sales[2024]`,
/// `Sales[[#Headers],[2024]]`).
fn is_external_ordinal(text: &str, open: usize, close: usize) -> bool {
    let bytes = text.as_bytes();
    let prev = open.checked_sub(1).map(|k| bytes[k]);
    if let Some(b) = prev
        && (b.is_ascii_alphanumeric() || matches!(b, b'_' | b'.' | b'\\' | b']' | b'['))
    {
        return false;
    }
    let mut k = close + 1;
    if prev == Some(b'\'') {
        while k < bytes.len() {
            if bytes[k] == b'\'' {
                if bytes.get(k + 1) == Some(&b'\'') {
                    k += 2;
                    continue;
                }
                return bytes.get(k + 1) == Some(&b'!');
            }
            k += 1;
        }
        return false;
    }
    if bytes.get(k) == Some(&b'!') {
        return true;
    }
    let start = k;
    while k < bytes.len() && (bytes[k].is_ascii_alphanumeric() || matches!(bytes[k], b'_' | b'.') || bytes[k] >= 0x80) {
        k += 1;
    }
    k > start && bytes.get(k) == Some(&b'!')
}

fn external_link_part_xml(book: &craft_xlsx::ExternalBook) -> (String, Vec<PreservedRel>) {
    let mut xml = format!("{XML_DECL}<externalLink xmlns=\"{NS_MAIN}\" xmlns:r=\"{NS_REL}\">");
    let mut rels = Vec::new();
    xml.push_str("<externalBook");
    if let Some(target) = &book.target {
        xml.push_str(" r:id=\"rId1\"");
        rels.push(PreservedRel { id: "rId1".into(), rel_type: format!("{NS_REL}/externalLinkPath"), target: target.clone(), external: true });
    }
    xml.push('>');
    if !book.sheet_names.is_empty() {
        xml.push_str("<sheetNames>");
        for name in &book.sheet_names {
            let _ = write!(xml, "<sheetName val=\"{}\"/>", esc_attr(name));
        }
        xml.push_str("</sheetNames>");
    }
    if !book.defined_names.is_empty() {
        xml.push_str("<definedNames>");
        for dn in &book.defined_names {
            let _ = write!(xml, "<definedName name=\"{}\"", esc_attr(&dn.name));
            if let Some(refers_to) = &dn.refers_to {
                let _ = write!(xml, " refersTo=\"{}\"", esc_attr(refers_to));
            }
            if let Some(sheet_id) = dn.sheet_id {
                let _ = write!(xml, " sheetId=\"{sheet_id}\"");
            }
            xml.push_str("/>");
        }
        xml.push_str("</definedNames>");
    }
    if !book.sheet_data.is_empty() {
        xml.push_str("<sheetDataSet>");
        for sd in &book.sheet_data {
            let _ = write!(xml, "<sheetData sheetId=\"{}\"", sd.sheet_id);
            if sd.refresh_error {
                xml.push_str(" refreshError=\"1\"");
            }
            xml.push('>');
            let mut rows: BTreeMap<u32, Vec<(craft_xlsx::CellRef, &craft_xlsx::Value)>> = BTreeMap::new();
            for (pos, val) in &sd.cells {
                rows.entry(pos.row).or_default().push((*pos, val));
            }
            for (r, cells) in rows {
                let _ = write!(xml, "<row r=\"{}\">", r + 1);
                for (pos, val) in cells {
                    let r_attr = CellRef::new(pos.row, pos.col);
                    match val {
                        craft_xlsx::Value::Empty => {
                            let _ = write!(xml, "<cell r=\"{r_attr}\"/>");
                        }
                        craft_xlsx::Value::Number(n) => {
                            let _ = write!(xml, "<cell r=\"{r_attr}\"><v>{n}</v></cell>");
                        }
                        craft_xlsx::Value::Text(t) => {
                            let _ = write!(xml, "<cell r=\"{r_attr}\" t=\"str\"><v>{}</v></cell>", esc(t));
                        }
                        craft_xlsx::Value::Bool(b) => {
                            let _ = write!(xml, "<cell r=\"{r_attr}\" t=\"b\"><v>{}</v></cell>", if *b { "1" } else { "0" });
                        }
                        craft_xlsx::Value::Error(e) => {
                            let _ = write!(xml, "<cell r=\"{r_attr}\" t=\"e\"><v>{}</v></cell>", esc(map_cell_error(*e).as_str()));
                        }
                    }
                }
                xml.push_str("</row>");
            }
            xml.push_str("</sheetData>");
        }
        xml.push_str("</sheetDataSet>");
    }
    xml.push_str("</externalBook></externalLink>");
    (xml, rels)
}
