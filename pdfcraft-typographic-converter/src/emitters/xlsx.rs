use crate::layout::{DocumentLayout, PageImage, ScriptType, TextLine};
use anyhow::{Context, Result};
use rust_xlsxwriter::{Color, Format, FormatScript, Image, Workbook};
use std::collections::HashMap;
use std::path::Path;

enum Element<'a> {
    Line(&'a TextLine),
    Image(&'a PageImage),
}

fn is_safe_numeric(text: &str) -> Option<f64> {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return None;
    }
    let lower = trimmed.to_ascii_lowercase();
    if lower == "nan" || lower.contains("inf") {
        return None;
    }
    let check_zeros = if let Some(rest) = trimmed.strip_prefix('-') {
        rest
    } else if let Some(rest) = trimmed.strip_prefix('+') {
        rest
    } else {
        trimmed
    };
    if check_zeros.starts_with('0') && check_zeros.len() > 1 && !check_zeros.starts_with("0.") {
        return None;
    }
    let digit_count = check_zeros.chars().filter(|c| c.is_ascii_digit()).count();
    if digit_count > 15 {
        return None;
    }
    let val = trimmed.parse::<f64>().ok()?;
    if val.is_finite() {
        Some(val)
    } else {
        None
    }
}

struct Field<'a> {
    col: u16,
    runs: Vec<&'a crate::layout::TextRun>,
}

fn discover_col_anchors(doc: &DocumentLayout) -> Vec<f32> {
    let mut x_positions = Vec::new();
    for page in &doc.pages {
        for line in &page.lines {
            for run in &line.runs {
                x_positions.push(run.bounds.0);
            }
        }
    }
    x_positions.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));

    let mut col_anchors: Vec<f32> = Vec::new();
    for x in x_positions {
        if col_anchors.last().is_none_or(|last| (x - *last).abs() > 14.0) {
            col_anchors.push(x);
        }
    }
    col_anchors
}

fn run_format(run: &crate::layout::TextRun) -> Format {
    let mut format = Format::new().set_font_name(&run.font_name).set_font_size(run.font_size as f64);
    if run.is_bold {
        format = format.set_bold();
    }
    if run.is_italic {
        format = format.set_italic();
    }
    if run.script == ScriptType::Superscript {
        format = format.set_font_script(FormatScript::Superscript);
    } else if run.script == ScriptType::Subscript {
        format = format.set_font_script(FormatScript::Subscript);
    }
    format
}

fn group_line_fields<'a>(line: &'a TextLine, col_anchors: &[f32]) -> Vec<Field<'a>> {
    let mut fields: Vec<Field<'a>> = Vec::new();
    for run in &line.runs {
        if run.text.trim().is_empty() {
            continue;
        }
        let target_col = col_anchors
            .iter()
            .position(|&a| (a - run.bounds.0).abs() <= 14.0)
            .unwrap_or_else(|| col_anchors.partition_point(|&a| a < run.bounds.0).saturating_sub(1)) as u16;

        let can_merge = fields.last().is_some_and(|last_field| {
            last_field.runs.last().is_some_and(|prev_run| target_col == last_field.col || (run.bounds.0 - prev_run.bounds.2).abs() < 16.0)
        });

        if let Some(last_field) = fields.last_mut().filter(|_| can_merge) {
            last_field.runs.push(run);
        } else {
            fields.push(Field { col: target_col, runs: vec![run] });
        }
    }
    fields
}

fn write_line_row(
    worksheet: &mut rust_xlsxwriter::Worksheet,
    row: u32,
    line: &TextLine,
    col_anchors: &[f32],
    col_max_len: &mut HashMap<u16, usize>,
) -> Result<()> {
    let mut last_col: Option<u16> = None;
    for field in group_line_fields(line, col_anchors) {
        let mut col = field.col;
        if let Some(prev) = last_col {
            if col <= prev {
                col = prev + 1;
            }
        }
        last_col = Some(col);

        let total_chars: usize = field.runs.iter().map(|r| r.text.chars().count()).sum();
        col_max_len.entry(col).and_modify(|len| *len = (*len).max(total_chars)).or_insert(total_chars);

        if field.runs.len() == 1 {
            let run = field.runs[0];
            let text = run.text.trim();
            let format = run_format(run);
            if let Some(num) = is_safe_numeric(text) {
                worksheet
                    .write_number_with_format(row, col, num, &format)
                    .with_context(|| format!("Failed to write number '{num}' at row {row}, col {col}"))?;
            } else {
                worksheet
                    .write_string_with_format(row, col, text, &format)
                    .with_context(|| format!("Failed to write string '{text}' at row {row}, col {col}"))?;
            }
        } else {
            let formats: Vec<Format> = field.runs.iter().map(|r| run_format(r)).collect();
            let segments: Vec<(&Format, &str)> = formats.iter().zip(field.runs.iter()).map(|(f, r)| (f, r.text.as_str())).collect();
            worksheet.write_rich_string(row, col, &segments).with_context(|| format!("Failed to write rich string at row {row}, col {col}"))?;
        }
    }
    Ok(())
}

/// Exports a `DocumentLayout` to a single consolidated Excel spreadsheet (.xlsx).
pub fn export_xlsx(doc: &DocumentLayout, output_path: &Path) -> Result<()> {
    let mut workbook = Workbook::new();
    let worksheet = workbook.add_worksheet();
    worksheet.set_name("Export").with_context(|| "Failed to set worksheet name 'Export'")?;

    let col_anchors = discover_col_anchors(doc);
    let mut current_row: u32 = 0;
    let mut col_max_len: HashMap<u16, usize> = HashMap::new();

    for (page_idx, page) in doc.pages.iter().enumerate() {
        if page_idx > 0 {
            current_row += 1;
            let divider_format =
                Format::new().set_bold().set_font_size(10.5).set_font_color(Color::RGB(0x475569)).set_background_color(Color::RGB(0xF1F5F9));
            worksheet
                .write_string_with_format(current_row, 0, format!("── Page {} ──", page.page_index + 1), &divider_format)
                .with_context(|| format!("Failed to write page divider for page {}", page.page_index + 1))?;
            current_row += 2;
        }

        // Interleave lines and images, sorted top-to-bottom (descending Y)
        let mut elements: Vec<Element> = Vec::with_capacity(page.lines.len() + page.images.len());
        elements.extend(page.lines.iter().map(Element::Line));
        elements.extend(page.images.iter().map(Element::Image));
        elements.sort_by(|a, b| {
            let y = |el: &Element| match el {
                Element::Line(l) => l.baseline_y,
                Element::Image(img) => img.bounds.3,
            };
            y(b).partial_cmp(&y(a)).unwrap_or(std::cmp::Ordering::Equal)
        });

        for elem in elements {
            match elem {
                Element::Line(line) => {
                    write_line_row(worksheet, current_row, line, &col_anchors, &mut col_max_len)?;
                    current_row += 1;
                }
                Element::Image(img) => {
                    let col = col_anchors.iter().position(|&a| (a - img.bounds.0).abs() <= 20.0).unwrap_or(0) as u16;
                    let xlsx_image = Image::new_from_buffer(&img.bytes)
                        .with_context(|| format!("Failed to decode image on page {} for xlsx export", page.page_index))?;
                    worksheet
                        .insert_image(current_row, col, &xlsx_image)
                        .with_context(|| format!("Failed to insert image at row {current_row}, col {col} on page {}", page.page_index))?;
                    let rows_span = ((img.height_pt / 18.0).ceil() as u32).max(1);
                    current_row += rows_span + 1;
                }
            }
        }
    }

    for (&col, &max_len) in &col_max_len {
        let width = ((max_len as f64) + 4.0).clamp(10.0, 50.0);
        let _ = worksheet.set_column_width(col, width);
    }

    workbook.save(output_path).with_context(|| format!("Failed to save XLSX workbook to {}", output_path.display()))?;
    Ok(())
}
