use crate::layout::{DocumentLayout, PageImage, ScriptType, TextLine};
use anyhow::{Context, Result};
use rust_xlsxwriter::{Color, Format, FormatScript, Image, Workbook};
use std::collections::HashMap;
use std::path::Path;

enum Element<'a> {
    Line(&'a TextLine),
    Image(&'a PageImage),
}

/// Exports a `DocumentLayout` to a single consolidated Excel spreadsheet (.xlsx).
pub fn export_xlsx(doc: &DocumentLayout, output_path: &Path) -> Result<()> {
    let mut workbook = Workbook::new();
    let worksheet = workbook.add_worksheet();
    worksheet
        .set_name("Export")
        .with_context(|| "Failed to set worksheet name 'Export'")?;

    // 1. Discover distinct column anchors across all pages
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
        if let Some(last) = col_anchors.last() {
            if (x - *last).abs() > 14.0 {
                col_anchors.push(x);
            }
        } else {
            col_anchors.push(x);
        }
    }

    let mut current_row: u32 = 0;
    let mut col_max_len: HashMap<u16, usize> = HashMap::new();

    // 2. Output all pages sequentially into the single worksheet
    for (page_idx, page) in doc.pages.iter().enumerate() {
        if page_idx > 0 {
            current_row += 1;
            let divider_format = Format::new()
                .set_bold()
                .set_font_size(10.5)
                .set_font_color(Color::RGB(0x475569))
                .set_background_color(Color::RGB(0xF1F5F9));
            worksheet
                .write_string_with_format(
                    current_row,
                    0,
                    format!("── Page {} ──", page.page_index + 1),
                    &divider_format,
                )
                .with_context(|| format!("Failed to write page divider for page {}", page.page_index + 1))?;
            current_row += 2;
        }

        // Interleave lines and images, sorted top-to-bottom (descending Y)
        let mut elements: Vec<Element> = Vec::with_capacity(page.lines.len() + page.images.len());
        for line in &page.lines {
            elements.push(Element::Line(line));
        }
        for img in &page.images {
            elements.push(Element::Image(img));
        }

        elements.sort_by(|a, b| {
            let y_a = match a {
                Element::Line(l) => l.baseline_y,
                Element::Image(img) => img.bounds.3,
            };
            let y_b = match b {
                Element::Line(l) => l.baseline_y,
                Element::Image(img) => img.bounds.3,
            };
            y_b.partial_cmp(&y_a).unwrap_or(std::cmp::Ordering::Equal)
        });

        for elem in elements {
            match elem {
                Element::Line(line) => {
                    let mut last_col: Option<u16> = None;

                    for run in &line.runs {
                        let mut col = col_anchors
                            .iter()
                            .position(|&a| (a - run.bounds.0).abs() <= 14.0)
                            .unwrap_or_else(|| {
                                col_anchors.partition_point(|&a| a < run.bounds.0).saturating_sub(1)
                            }) as u16;

                        // Ensure successive runs on the same row don't overwrite each other
                        if let Some(prev) = last_col {
                            if col <= prev {
                                col = prev + 1;
                            }
                        }
                        last_col = Some(col);

                        let text = run.text.trim();
                        if text.is_empty() {
                            continue;
                        }

                        col_max_len
                            .entry(col)
                            .and_modify(|len| *len = (*len).max(text.chars().count()))
                            .or_insert(text.chars().count());

                        let mut format = Format::new()
                            .set_font_name(&run.font_name)
                            .set_font_size(run.font_size as f64);

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

                        if let Ok(num) = text.parse::<f64>() {
                            worksheet
                                .write_number_with_format(current_row, col, num, &format)
                                .with_context(|| format!("Failed to write number '{}' at row {}, col {}", num, current_row, col))?;
                        } else {
                            worksheet
                                .write_string_with_format(current_row, col, text, &format)
                                .with_context(|| {
                                    format!(
                                        "Failed to write string '{}' at row {}, col {}",
                                        text, current_row, col
                                    )
                                })?;
                        }
                    }
                    current_row += 1;
                }
                Element::Image(img) => {
                    let col = col_anchors
                        .iter()
                        .position(|&a| (a - img.bounds.0).abs() <= 20.0)
                        .unwrap_or(0) as u16;

                    if let Ok(xlsx_image) = Image::new_from_buffer(&img.bytes) {
                        if worksheet.insert_image(current_row, col, &xlsx_image).is_ok() {
                            let rows_span = ((img.height_pt / 18.0).ceil() as u32).max(1);
                            current_row += rows_span + 1;
                        }
                    }
                }
            }
        }
    }

    // 3. Set auto-fit column widths with safety clamp
    for (&col, &max_len) in &col_max_len {
        let width = ((max_len as f64) + 4.0).clamp(10.0, 50.0);
        let _ = worksheet.set_column_width(col, width);
    }

    workbook
        .save(output_path)
        .with_context(|| format!("Failed to save XLSX workbook to {}", output_path.display()))?;

    Ok(())
}
