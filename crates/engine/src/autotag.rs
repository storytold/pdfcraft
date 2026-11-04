//! Reviewable structure proposals for pages that have not yet been tagged.
//!
//! A proposal is deliberately not a PDF tag. It records the visible text blocks and the
//! evidence for a tentative role, so a later edit can attach marked content and a parent tree
//! without presenting a font-size guess as a finished accessible document.

use crate::Document;

/// A bounded text block proposed for a structure element.
#[derive(Clone, Debug, PartialEq)]
pub struct Candidate {
    pub kind: &'static str,
    pub reason: &'static str,
    pub text: String,
    pub text_truncated: bool,
    pub rect: [f64; 4],
    pub size: f64,
    pub bold: bool,
}

/// The visible text portion of one page's proposed structure.
#[derive(Clone, Debug, PartialEq)]
pub struct Preview {
    pub candidates: Vec<Candidate>,
    /// Raster or grouped artwork requires separate figure/artifact review.
    pub unclassified_images: usize,
    /// Text blocks omitted because their geometry or font size was invalid.
    pub unclassified_text_blocks: usize,
    /// Existing structure must be reconciled before any proposal is applied.
    pub existing_structure: bool,
}

const MAX_BLOCKS: usize = 2_048;
const MAX_TEXT_CHARS: usize = 512;

impl Document {
    /// Propose text tags for one 0-based page. This reads the current working PDF and does not
    /// alter it. Every result needs review before tags are written; image and table semantics
    /// are outside this text-only proposal.
    pub fn autotag_preview(&self, page: usize) -> Result<Preview, String> {
        if self.info.pages.get(page).is_none() {
            return Err(format!("page {} is outside the document", page.saturating_add(1)));
        }
        let editor = self.editor.as_ref().ok_or("the document can't be read")?;
        let existing_structure = editor.cos.root().is_some_and(|r| editor.cos.get(r).as_dict().is_some_and(|d| d.contains(b"StructTreeRoot")));
        let blocks = pdfcraft_edit::text_blocks(&editor.cos, page).map_err(|e| e.to_string())?;
        if blocks.len() > MAX_BLOCKS {
            return Err(format!("page {} has too many text blocks to preview (maximum {MAX_BLOCKS})", page.saturating_add(1)));
        }
        let mut sizes: Vec<f64> = blocks.iter().map(|b| b.size).filter(|s| s.is_finite() && *s > 0.0).collect();
        sizes.sort_by(f64::total_cmp);
        let body_size = sizes.get(sizes.len().saturating_sub(1) / 2).copied().unwrap_or(12.0);
        let mut unclassified_text_blocks = 0;
        let candidates = blocks
            .into_iter()
            .filter(|b| {
                let valid = !b.text.trim().is_empty() && b.rect.iter().all(|v| v.is_finite()) && b.size.is_finite() && b.size > 0.0;
                if !valid {
                    unclassified_text_blocks += 1;
                }
                valid
            })
            .map(|b| {
                let mut chars = b.text.chars();
                let text: String = chars.by_ref().take(MAX_TEXT_CHARS).collect();
                let text_truncated = chars.next().is_some();
                let short_line = b.rows.len() == 1 && b.text.chars().count() <= 120;
                let heading = short_line && b.size.is_finite() && b.size > body_size * 1.35;
                Candidate {
                    kind: if heading { "H1" } else { "P" },
                    reason: if heading {
                        "short, single-line text larger than the page body"
                    } else {
                        "visible text block; semantic role needs review"
                    },
                    text,
                    text_truncated,
                    rect: b.rect,
                    size: b.size,
                    bold: b.bold,
                }
            })
            .collect();
        Ok(Preview { candidates, unclassified_images: self.page_images(page).len(), unclassified_text_blocks, existing_structure })
    }
}
