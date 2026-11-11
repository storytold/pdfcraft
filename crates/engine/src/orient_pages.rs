//! Scan & OCR ▸ Auto-orient pages: detect each page's upright quarter turn and apply
//! lossless `/Rotate` corrections. Uses OCR word plausibility when models are installed,
//! otherwise a pixel-only projection-profile heuristic.

use std::sync::Arc;

use pdfcraft_ocr::{Line, Models, Ocr, OcrImage, orient, preprocess};
use pdfcraft_render::{RenderConfig, RenderRequest};

use crate::{DocId, Edit, EditError, Session};

/// Longest side of the raster used for orientation, in pixels.
const SAMPLE_SIDE: f32 = 900.0;
/// OCR confidence at or below this (0–1) is flagged for review.
const LOW_CONFIDENCE: f64 = 0.55;
/// Winner must beat the runner-up by this absolute share of plausible characters.
const MIN_MARGIN: f64 = 0.08;

/// How a suggested turn was decided.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OrientSource {
    /// Pixel banding / bottom-heaviness only (no OCR models).
    Pixels,
    /// Dual OCR readings scored by word plausibility.
    Ocr,
}

/// One page's orientation assessment (before any edit).
#[derive(Clone, Debug, PartialEq)]
pub struct OrientSuggestion {
    /// 0-based page index.
    pub page: usize,
    /// Quarter turns clockwise to apply via `/Rotate` (0, 90, 180, or 270).
    pub degrees_cw: i32,
    /// 0–1: how clearly the upright turn won (OCR ratio, or 0.5 for a pixel-only call).
    pub confidence: f64,
    /// True when the user should review before applying.
    pub low_confidence: bool,
    pub source: OrientSource,
    /// Why no correction is suggested, if any (already upright, too little ink, …).
    pub note: Option<String>,
}

/// Result of assessing and optionally applying auto-orient.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct OrientReport {
    pub suggestions: Vec<OrientSuggestion>,
    /// Pages whose `/Rotate` was changed.
    pub rotated: usize,
}

impl Session {
    /// Assess orientation for each page in `pages` (0-based). Does not modify the document.
    ///
    /// Progress is `(done, total)` including a final `(total, total)`. Cancelled when `cancel`
    /// returns true between pages; already-finished suggestions are still returned.
    pub fn assess_orientation(
        &self,
        id: DocId,
        pages: &[usize],
        cancel: &dyn Fn() -> bool,
        progress: &dyn Fn(usize, usize),
    ) -> Result<Vec<OrientSuggestion>, EditError> {
        let doc = self.get(id).ok_or(EditError::NoDocument)?;
        let total = pages.len();
        progress(0, total);
        let config = RenderConfig { password: doc.password.as_deref().map(Arc::from), ..Default::default() };
        let mut renderer = pdfcraft_render::PageRenderer::new(doc.bytes.clone(), config);
        let ocr = Models::find().and_then(|m| Ocr::load(&m).ok());
        let mut out = Vec::with_capacity(pages.len());
        for (i, &page) in pages.iter().enumerate() {
            if cancel() {
                break;
            }
            let info = doc.info.pages.get(page).ok_or_else(|| EditError::Invalid(format!("there is no page {}", page.saturating_add(1))))?;
            let long = info.width.max(info.height).max(1.0);
            let scale = (SAMPLE_SIDE / long).clamp(0.25, 4.0);
            let shot = renderer.render(RenderRequest { page, scale, ..Default::default() });
            if let Some(e) = shot.error {
                out.push(OrientSuggestion {
                    page,
                    degrees_cw: 0,
                    confidence: 0.0,
                    low_confidence: true,
                    source: OrientSource::Pixels,
                    note: Some(format!("could not render: {e}")),
                });
                progress(i.saturating_add(1), total);
                continue;
            }
            out.push(assess_rgba(&shot.rgba, shot.width, shot.height, page, ocr.as_ref()));
            progress(i.saturating_add(1), total);
        }
        progress(total, total);
        Ok(out)
    }

    /// Apply accepted suggestions as one undoable batch of `/Rotate` edits.
    /// Only entries with `degrees_cw != 0` are applied. Returns how many pages changed.
    pub fn apply_orientation(&mut self, id: DocId, suggestions: &[OrientSuggestion]) -> Result<OrientReport, EditError> {
        let doc = self.get(id).ok_or(EditError::NoDocument)?;
        if let Some(why) = &doc.read_only_reason {
            return Err(EditError::ReadOnly(why.clone()));
        }
        let mut edits = Vec::new();
        let mut rotated = 0usize;
        for s in suggestions {
            if s.degrees_cw == 0 {
                continue;
            }
            if s.degrees_cw % 90 != 0 {
                return Err(EditError::Invalid(format!("page {}: rotation must be a multiple of 90 degrees", s.page.saturating_add(1))));
            }
            edits.push(Edit::RotatePages { pages: vec![s.page], degrees: i64::from(s.degrees_cw) });
            rotated = rotated.saturating_add(1);
        }
        if edits.is_empty() {
            return Ok(OrientReport { suggestions: suggestions.to_vec(), rotated: 0 });
        }
        self.apply(id, Edit::Batch { label: "Smart Page Orient".into(), edits })?;
        Ok(OrientReport { suggestions: suggestions.to_vec(), rotated })
    }

    /// Assess every listed page and apply corrections that are confident enough.
    /// Low-confidence suggestions are left out of the edit (still reported).
    pub fn orient_pages(&mut self, id: DocId, pages: &[usize]) -> Result<OrientReport, EditError> {
        let suggestions = self.assess_orientation(id, pages, &|| false, &|_, _| {})?;
        let apply: Vec<OrientSuggestion> = suggestions
            .iter()
            .cloned()
            .map(|mut s| {
                if s.low_confidence || s.degrees_cw == 0 {
                    s.degrees_cw = 0;
                }
                s
            })
            .collect();
        let mut report = self.apply_orientation(id, &apply)?;
        // Return the original assessments (with angles) so callers can show what was skipped.
        report.suggestions = suggestions;
        Ok(report)
    }
}

fn assess_rgba(rgba: &[u8], width: u32, height: u32, page: usize, ocr: Option<&Ocr>) -> OrientSuggestion {
    if let Some(ocr) = ocr {
        return assess_with_ocr(rgba, width, height, page, ocr);
    }
    assess_pixels(rgba, width, height, page)
}

fn assess_pixels(rgba: &[u8], width: u32, height: u32, page: usize) -> OrientSuggestion {
    let image = match OcrImage::new(width, height, rgba.to_vec()) {
        Ok(img) => img,
        Err(_) => {
            return OrientSuggestion {
                page,
                degrees_cw: 0,
                confidence: 0.0,
                low_confidence: true,
                source: OrientSource::Pixels,
                note: Some("page image could not be read".into()),
            };
        }
    };
    let turns = preprocess::suggest_quarter_turns_cw(&image);
    let degrees_cw = (turns.saturating_mul(90) % 360) as i32;
    let note = if degrees_cw == 0 { Some("already upright (or unclear)".into()) } else { None };
    // Pixel-only calls are medium confidence: enough to suggest, flagged for review.
    OrientSuggestion {
        page,
        degrees_cw,
        confidence: if degrees_cw == 0 { 0.5 } else { 0.6 },
        low_confidence: degrees_cw != 0,
        source: OrientSource::Pixels,
        note,
    }
}

fn assess_with_ocr(rgba: &[u8], width: u32, height: u32, page: usize, ocr: &Ocr) -> OrientSuggestion {
    let skew = orient::skew(rgba, width, height);
    let pairs: &[[u8; 2]] = match skew {
        Some(s) if s.vertical => &[[1, 3], [0, 2]],
        Some(_) => &[[0, 2], [1, 3]],
        None => {
            // No measurable text lines: try pixel heuristic, then stop.
            return assess_pixels(rgba, width, height, page);
        }
    };

    let mut reads: Vec<(u8, Vec<Line>, f64)> = Vec::new();
    for pair in pairs {
        for &q in pair {
            let (px, w, h) = if q == 0 { (rgba.to_vec(), width, height) } else { orient::quarter_turn(rgba, width, height, q) };
            match ocr.recognize(&px, w, h) {
                Ok(lines) => {
                    let (good, all) = orient::plausibility(&lines);
                    let score = if all == 0 { 0.0 } else { good as f64 / all as f64 };
                    reads.push((q, lines, score));
                }
                Err(_) => continue,
            }
        }
        // Too little text on this axis: do not try the other pair.
        if reads.last().is_some_and(|(_, lines, _)| orient::reads_as_words(lines).is_none())
            && reads.iter().all(|(_, lines, _)| orient::reads_as_words(lines).is_none())
        {
            break;
        }
    }

    if reads.is_empty() {
        return OrientSuggestion {
            page,
            degrees_cw: 0,
            confidence: 0.0,
            low_confidence: true,
            source: OrientSource::Ocr,
            note: Some("recognition failed".into()),
        };
    }

    reads.sort_by(|a, b| b.2.partial_cmp(&a.2).unwrap_or(std::cmp::Ordering::Equal));
    let best = &reads[0];
    let second = reads.get(1).map(|r| r.2).unwrap_or(0.0);
    let margin = best.2 - second;
    let upright = orient::reads_as_words(&best.1) == Some(true);
    let degrees_cw = if upright { i32::from(best.0).saturating_mul(90) % 360 } else { 0 };
    let low = degrees_cw != 0 && (best.2 < LOW_CONFIDENCE || margin < MIN_MARGIN);
    let note = if !upright {
        Some("too little clear text to tell which way is up".into())
    } else if degrees_cw == 0 {
        Some("already upright".into())
    } else if low {
        Some("low confidence — review before applying".into())
    } else {
        None
    };
    OrientSuggestion { page, degrees_cw, confidence: best.2.clamp(0.0, 1.0), low_confidence: low, source: OrientSource::Ocr, note }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    /// Minimal multi-page PDF with blank pages.
    fn fixture(n: usize) -> Vec<u8> {
        let mut out = Vec::new();
        out.extend_from_slice(b"%PDF-1.4\n");
        let mut offsets = Vec::new();
        let mut objects: Vec<Vec<u8>> = Vec::new();
        objects.push(b"<< /Type /Catalog /Pages 2 0 R >>".to_vec());
        let mut kids_s = String::new();
        for i in 0..n {
            kids_s.push_str(&format!("{} 0 R ", 3 + i));
        }
        objects.push(format!("<< /Type /Pages /Kids [{kids_s}] /Count {n} >>").into_bytes());
        for _ in 0..n {
            objects.push(b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 200 200] >>".to_vec());
        }
        for o in &objects {
            offsets.push(out.len());
            let i = offsets.len();
            out.extend_from_slice(format!("{i} 0 obj\n").as_bytes());
            out.extend_from_slice(o);
            out.extend_from_slice(b"\nendobj\n");
        }
        let xref = out.len();
        out.extend_from_slice(format!("xref\n0 {}\n0000000000 65535 f \n", objects.len() + 1).as_bytes());
        for o in &offsets {
            out.extend_from_slice(format!("{o:010} 00000 n \n").as_bytes());
        }
        out.extend_from_slice(format!("trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n", objects.len() + 1).as_bytes());
        out
    }

    fn session_with_blank() -> (Session, DocId) {
        let mut s = Session::new().with_clock(|| 1_700_000_000);
        let id = s.open("orient.pdf", None, Arc::new(fixture(2)), None).expect("opens");
        (s, id)
    }

    #[test]
    fn assess_blank_pages_suggest_no_turn() {
        let (s, id) = session_with_blank();
        let sug = s.assess_orientation(id, &[0, 1], &|| false, &|_, _| {}).expect("assess");
        assert_eq!(sug.len(), 2);
        assert!(sug.iter().all(|p| p.degrees_cw == 0));
    }

    #[test]
    fn apply_zero_degrees_is_a_no_op() {
        let (mut s, id) = session_with_blank();
        let report = s
            .apply_orientation(
                id,
                &[OrientSuggestion { page: 0, degrees_cw: 0, confidence: 1.0, low_confidence: false, source: OrientSource::Pixels, note: None }],
            )
            .expect("apply");
        assert_eq!(report.rotated, 0);
        assert!(s.get(id).expect("doc").can_undo().is_none());
    }

    #[test]
    fn apply_quarter_turn_is_undoable() {
        let (mut s, id) = session_with_blank();
        let report = s
            .apply_orientation(
                id,
                &[OrientSuggestion { page: 0, degrees_cw: 180, confidence: 0.9, low_confidence: false, source: OrientSource::Ocr, note: None }],
            )
            .expect("apply");
        assert_eq!(report.rotated, 1);
        let doc = s.get(id).expect("doc");
        assert_eq!(doc.can_undo(), Some("Smart Page Orient"));
        assert_eq!(doc.info.pages[0].rotation, 180);
        s.undo(id).expect("undo");
        let doc = s.get(id).expect("doc");
        assert_eq!(doc.info.pages[0].rotation, 0);
    }
}
