use crate::layout::{DocumentLayout, PageImage, ScriptType, TextLine};
use anyhow::{Context, Result};
use docx_rs::{BreakType, Docx, Paragraph, Pic, Run, RunFonts, VertAlignType};
use std::fs::File;
use std::path::Path;

enum PageItem<'a> {
    Line(&'a TextLine),
    Image(&'a PageImage),
}

/// Exports a `DocumentLayout` to a Microsoft Word (.docx) document.
pub fn export_docx(doc: &DocumentLayout, output_path: &Path) -> Result<()> {
    let mut docx = Docx::new();

    for (page_idx, page) in doc.pages.iter().enumerate() {
        // If transitioning to subsequent pages, insert a page break
        if page_idx > 0 {
            let break_p = Paragraph::new().add_run(Run::new().add_break(BreakType::Page));
            docx = docx.add_paragraph(break_p);
        }

        // Interleave lines and images by vertical position (descending y / top to bottom)
        let mut items: Vec<(f32, PageItem)> = Vec::with_capacity(page.lines.len() + page.images.len());
        for line in &page.lines {
            items.push((line.baseline_y, PageItem::Line(line)));
        }
        for img in &page.images {
            items.push((img.bounds.1, PageItem::Image(img)));
        }
        items.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));

        for (_, item) in items {
            match item {
                PageItem::Line(line) => {
                    let mut paragraph = Paragraph::new();

                    for run in &line.runs {
                        let mut r = Run::new();

                        // If run has leading tab, prepend a tab
                        if run.has_leading_tab {
                            r = r.add_tab();
                        }

                        // Add text content
                        r = r.add_text(&run.text);

                        // Set font family
                        r = r.fonts(RunFonts::new().ascii(&run.font_name));

                        // Set font size in half-points
                        let half_points = (run.font_size * 2.0).round() as usize;
                        r = r.size(half_points);

                        // Bold and Italic styling
                        if run.is_bold {
                            r = r.bold();
                        }
                        if run.is_italic {
                            r = r.italic();
                        }

                        // Superscript / Subscript vertical alignment
                        if run.script == ScriptType::Superscript {
                            r.run_property = r.run_property.vert_align(VertAlignType::SuperScript);
                        } else if run.script == ScriptType::Subscript {
                            r.run_property = r.run_property.vert_align(VertAlignType::SubScript);
                        }

                        paragraph = paragraph.add_run(r);
                    }

                    docx = docx.add_paragraph(paragraph);
                }
                PageItem::Image(img) => {
                    if !img.bytes.is_empty() {
                        let pic = Pic::new(&img.bytes);
                        let paragraph = Paragraph::new().add_run(Run::new().add_image(pic));
                        docx = docx.add_paragraph(paragraph);
                    }
                }
            }
        }
    }

    let file = File::create(output_path)
        .with_context(|| format!("Failed to create DOCX output file at {}", output_path.display()))?;

    docx.build()
        .pack(file)
        .with_context(|| format!("Failed to pack DOCX document to {}", output_path.display()))?;

    Ok(())
}
