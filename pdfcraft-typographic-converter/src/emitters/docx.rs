use crate::layout::{DocumentLayout, PageImage, ScriptType, TextLine};
use anyhow::{Context, Result};
use docx_rs::{BreakType, Docx, PageOrientationType, PageSize, Paragraph, Pic, Run, RunFonts, Section, VertAlignType};
use std::fs::File;
use std::path::Path;

enum PageItem<'a> {
    Line(&'a TextLine),
    Image(&'a PageImage),
}

/// Exports a `DocumentLayout` to a Microsoft Word (.docx) document.
pub fn export_docx(doc: &DocumentLayout, output_path: &Path) -> Result<()> {
    let mut docx = Docx::new();

    let has_mixed_page_sizes = doc.pages.windows(2).any(|w| (w[0].width - w[1].width).abs() > 1.0 || (w[0].height - w[1].height).abs() > 1.0);

    if let Some(first_page) = doc.pages.first() {
        let w_dxa = (first_page.width * 20.0).round().max(1.0) as u32;
        let h_dxa = (first_page.height * 20.0).round().max(1.0) as u32;
        docx = docx.page_size(w_dxa, h_dxa);
    }

    for (page_idx, page) in doc.pages.iter().enumerate() {
        let w_dxa = (page.width * 20.0).round().max(1.0) as u32;
        let h_dxa = (page.height * 20.0).round().max(1.0) as u32;
        let orient = if page.width > page.height { PageOrientationType::Landscape } else { PageOrientationType::Portrait };

        let mut page_paragraphs: Vec<Paragraph> = Vec::new();

        // If transitioning to subsequent pages without separate sections, insert a page break
        if page_idx > 0 && !has_mixed_page_sizes {
            let break_p = Paragraph::new().add_run(Run::new().add_break(BreakType::Page));
            page_paragraphs.push(break_p);
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

                    page_paragraphs.push(paragraph);
                }
                PageItem::Image(img) => {
                    if !img.bytes.is_empty() {
                        let min_x = img.bounds.0.min(img.bounds.2);
                        let max_x = img.bounds.0.max(img.bounds.2);
                        let min_y = img.bounds.1.min(img.bounds.3);
                        let max_y = img.bounds.1.max(img.bounds.3);
                        let w_emu = ((max_x - min_x) * 12700.0).round().max(1.0) as u32;
                        let h_emu = ((max_y - min_y) * 12700.0).round().max(1.0) as u32;
                        let pic = Pic::new(&img.bytes).size(w_emu, h_emu);
                        let paragraph = Paragraph::new().add_run(Run::new().add_image(pic));
                        page_paragraphs.push(paragraph);
                    }
                }
            }
        }

        if has_mixed_page_sizes {
            let mut section = Section::new().page_size(PageSize::new().size(w_dxa, h_dxa).orient(orient));
            for p in page_paragraphs {
                section = section.add_paragraph(p);
            }
            docx = docx.add_section(section);
        } else {
            for p in page_paragraphs {
                docx = docx.add_paragraph(p);
            }
        }
    }

    let file = File::create(output_path).with_context(|| format!("Failed to create DOCX output file at {}", output_path.display()))?;

    docx.build().pack(file).with_context(|| format!("Failed to pack DOCX document to {}", output_path.display()))?;

    Ok(())
}
