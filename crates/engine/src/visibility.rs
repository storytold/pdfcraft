//! Read-only audit of content a human does not see but a text extractor reads. The input of
//! prompt-injection checks: an agent (or the file-sharing layer around one) audits a document
//! before trusting what extraction returns about it. What counts as invisible is ours, not the
//! format's; every finding says why, and none of them says the document is safe.
//!
//! Kinds, in the priority a glyph is counted under (each glyph under at most one of the five
//! shape kinds; the review lenses below are reported on top of a shape kind):
//!
//! - `invisible`: painted by no render mode (ISO 32000-2 §9.3.3 render modes 3 and 7, and
//!   clip-shaping glyphs);
//! - `off-page`: wholly outside the displayed page;
//! - `transparent`: fill alpha below [`Options::TRANSPARENT_A`];
//! - `light-fill`: solid fill whose luminance is at least [`Options::light_luma`] — near the
//!   paper a viewer defaults to, so close to invisible in practice;
//! - `tiny`: em-box height below [`Options::tiny_pt`];
//! - `hidden-characters`: zero-width, bidi-control or tag characters in the text an extractor
//!   returns (U+200B–200D, U+2060–2064, U+FEFF, U+202A–202E, U+2066–2069, U+E0000–E007F);
//! - `actual-text`: the glyph carries a `/ActualText` replacement (ISO 32000-2 §14.9.4) —
//!   an extractor reads the replacement string while a reader sees whatever the source span
//!   paints. A review lens: a visible `.` can stand for text the reader never sees, so the
//!   pair is reported however both sides look.

use std::collections::BTreeMap;

use pdfcraft_render::text::GlyphAudit;

use crate::export::ExportSource;

/// Most samples one finding lists. The count is always exact.
pub const MAX_SAMPLES: usize = 8;

/// Most characters of one sample.
const MAX_SAMPLE: usize = 80;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Options {
    /// Em-box height (points) below which text is `tiny`.
    pub tiny_pt: f32,
    /// Fill luminance (0–1) at or above which text is `light-fill`.
    pub light_luma: f32,
}

impl Default for Options {
    fn default() -> Self {
        Self { tiny_pt: 1.5, light_luma: 0.95 }
    }
}

impl Options {
    /// Fill alpha (0–255) below which text is `transparent`.
    const TRANSPARENT_A: u8 = 13;
}

/// One finding: a kind on one page, with samples and rectangles (at most [`MAX_SAMPLES`]).
#[derive(Clone, Debug, PartialEq)]
pub struct Finding {
    pub kind: &'static str,
    pub count: usize,
    /// First samples, in content order; characters beyond [`MAX_SAMPLE`] are dropped.
    pub samples: Vec<String>,
    /// Rectangles of the sampled glyphs, page view space (points, origin top-left, y down).
    pub rects: Vec<[f64; 4]>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct PageReport {
    /// 1-based page number.
    pub page: usize,
    pub glyphs: usize,
    pub findings: Vec<Finding>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Report {
    /// Zero-based pages, in the requested order, without duplicates.
    pub pages: Vec<PageReport>,
    /// Count per kind over all reported pages.
    pub totals: BTreeMap<&'static str, usize>,
}

/// Audit the current document snapshot. `progress` can cancel between pages. A failed page is
/// an error, never a clean bill (the audit fails closed, as redaction auditing does).
pub fn audit(source: ExportSource, pages: &[usize], options: Options, progress: impl FnMut(usize) -> bool) -> Result<Report, String> {
    crate::guard(|| audit_inner(source, pages, options, progress))?
}

/// The audit body ([`audit`]) inside `crate::guard`: pages are 0-based here, 1-based in the report.
fn audit_inner(source: ExportSource, pages: &[usize], options: Options, mut progress: impl FnMut(usize) -> bool) -> Result<Report, String> {
    if pages.iter().any(|&page| page >= source.pages) {
        return Err("the visibility-audit page range is out of bounds".into());
    }
    if !progress(0) {
        return Err("visibility audit cancelled".into());
    }
    let mut renderer = pdfcraft_render::PageRenderer::new(source.bytes, source.config);
    let mut report = Report::default();
    let mut seen = std::collections::HashSet::new();
    for &page in pages {
        if !seen.insert(page) {
            continue;
        }
        if !progress(report.pages.len()) {
            return Err("visibility audit cancelled".into());
        }
        // The classifier holds only counts and samples, so a hostile page's glyph volume
        // costs memory it does not earn: every glyph is classified, none is stored whole.
        let mut classifier = Classifier::new(options);
        renderer.audit_with(page, &mut |glyph| classifier.push(&glyph)).map_err(|e| format!("page {}: {e}", page.saturating_add(1)))?;
        report.pages.push(classifier.finish(page));
        progress(report.pages.len());
    }
    for page in &report.pages {
        for finding in &page.findings {
            *report.totals.entry(finding.kind).or_default() += finding.count;
        }
    }
    Ok(report)
}

/// kind → (count, samples, rects), in first-seen kind order.
type Bucket = (usize, Vec<String>, Vec<[f64; 4]>);

/// Classifies a page's glyphs as they stream in ([`pdfcraft_render::text::audit_page_with`]):
/// exact counts per kind, samples capped at [`MAX_SAMPLES`].
pub(crate) struct Classifier {
    options: Options,
    glyphs: usize,
    order: Vec<&'static str>,
    buckets: BTreeMap<&'static str, Bucket>,
}

impl Classifier {
    pub(crate) fn new(options: Options) -> Self {
        Self { options, glyphs: 0, order: Vec::new(), buckets: BTreeMap::new() }
    }

    /// Classifies one glyph as it streams in: a review lens first (`actual-text`), then the
    /// shape kinds (unpainted, off-page, transparent, light-fill, tiny), then concealed
    /// Unicode characters. Samples cap at [`MAX_SAMPLES`]; only counts and samples are held.
    pub(crate) fn push(&mut self, glyph: &GlyphAudit) {
        self.glyphs += 1;
        let note = |classifier: &mut Self, kind: &'static str| {
            let bucket = classifier.buckets.entry(kind).or_insert_with(|| {
                classifier.order.push(kind);
                (0, Vec::new(), Vec::new())
            });
            bucket.0 += 1;
            if bucket.1.len() < MAX_SAMPLES {
                let mut sample: String = glyph.text.chars().take(MAX_SAMPLE).flat_map(char::escape_debug).collect();
                if glyph.text.chars().count() > MAX_SAMPLE {
                    sample.push('…');
                }
                bucket.1.push(sample);
                bucket.2.push(glyph.rect.map(f64::from));
            }
        };
        // A review lens first, on top of the shape kinds: the replacement text may read
        // nothing like what the reader sees.
        if glyph.actual_text {
            note(self, "actual-text");
        }
        if glyph.unpainted {
            note(self, "invisible");
            return;
        }
        if glyph.off_page {
            note(self, "off-page");
            return;
        }
        match glyph.fill {
            Some([_, _, _, a]) if a < Options::TRANSPARENT_A => note(self, "transparent"),
            Some([r, g, b, _]) if luma(r, g, b) >= self.options.light_luma => note(self, "light-fill"),
            _ if f64::from(glyph.rect[3] - glyph.rect[1]) < f64::from(self.options.tiny_pt) => note(self, "tiny"),
            _ => {}
        }
        if glyph.fill.is_none_or(|[_, _, _, a]| a >= Options::TRANSPARENT_A)
            && !glyph.unpainted
            && !glyph.off_page
            && let Some(kind) = glyph.text.chars().filter_map(hidden_kind).next()
        {
            note(self, kind);
        }
    }

    /// The page's report: 1-based page number, total glyphs and the findings in first-seen kind order.
    pub(crate) fn finish(mut self, page: usize) -> PageReport {
        PageReport {
            page: page.saturating_add(1),
            glyphs: self.glyphs,
            findings: self
                .order
                .into_iter()
                .filter_map(|kind| self.buckets.remove(kind).map(|(count, samples, rects)| Finding { kind, count, samples, rects }))
                .collect(),
        }
    }
}

/// Rec. 709 luminance of an sRGB-encoded colour, 0–1. A heuristic for "close to the paper",
/// not colour science: the PDF's blend space is unknown here.
fn luma(r: u8, g: u8, b: u8) -> f32 {
    (0.2126 * f32::from(r) + 0.7152 * f32::from(g) + 0.0722 * f32::from(b)) / 255.0
}

/// The finding kind a concealed Unicode character belongs to, if it is one. Emoji ZWJ sequences
/// (U+200D between emoji) land here too: flagged for review, not convicted.
fn hidden_kind(c: char) -> Option<&'static str> {
    match c {
        '\u{200B}' | '\u{200C}' | '\u{200D}' | '\u{2060}'..='\u{2064}' | '\u{FEFF}' => Some("hidden-characters"),
        '\u{202A}'..='\u{202E}' | '\u{2066}'..='\u{2069}' => Some("hidden-characters"),
        '\u{E0000}'..='\u{E007F}' => Some("hidden-characters"),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A painted, on-page glyph 12 pt tall at (10, 20), unless overridden.
    fn glyph(text: &str, unpainted: bool, off_page: bool, fill: Option<[u8; 4]>, height: f32) -> GlyphAudit {
        GlyphAudit { text: text.to_string(), rect: [10.0, 20.0, 60.0, 20.0 + height], unpainted, off_page, fill, actual_text: false }
    }

    fn classified(glyphs: Vec<GlyphAudit>) -> Vec<(&'static str, usize)> {
        let mut c = Classifier::new(Options::default());
        for g in &glyphs {
            c.push(g);
        }
        c.finish(0).findings.into_iter().map(|f| (f.kind, f.count)).collect()
    }

    #[test]
    fn ordinary_visible_text_has_no_findings() {
        let mut c = Classifier::new(Options::default());
        c.push(&glyph("Hello", false, false, Some([0, 0, 0, 255]), 12.0));
        let report = c.finish(0);
        assert_eq!(report.page, 1);
        assert_eq!(report.glyphs, 1);
        assert!(report.findings.is_empty());
    }

    #[test]
    fn each_shape_kind_is_reported_once_per_kind() {
        let glyphs = vec![
            glyph("a", true, false, Some([0, 0, 0, 255]), 12.0),        // invisible: priority over fill
            glyph("b", false, true, Some([0, 0, 0, 255]), 12.0),        // off-page
            glyph("c", false, false, Some([0, 0, 0, 0]), 12.0),         // transparent
            glyph("d", false, false, Some([255; 4]), 12.0),             // light-fill (white)
            glyph("e", false, false, Some([0, 0, 0, 255]), 1.0),        // tiny
            glyph("f", false, false, Some([128, 128, 128, 255]), 12.0), // mid-gray: none of the above
        ];
        assert_eq!(classified(glyphs), vec![("invisible", 1), ("off-page", 1), ("transparent", 1), ("light-fill", 1), ("tiny", 1)]);
    }

    #[test]
    fn hidden_characters_are_counted_on_visible_glyphs() {
        assert_eq!(classified(vec![glyph("a\u{200b}b", false, false, Some([0, 0, 0, 255]), 12.0)]), vec![("hidden-characters", 1)]);
        assert_eq!(classified(vec![glyph("\u{202e}x", false, false, Some([0, 0, 0, 255]), 12.0)]), vec![("hidden-characters", 1)]);
        assert_eq!(classified(vec![glyph("a\u{e0041}", false, false, Some([0, 0, 0, 255]), 12.0)]), vec![("hidden-characters", 1)]);
        // An invisible glyph is not double-reported: its characters ride the invisible finding.
        assert_eq!(classified(vec![glyph("\u{200b}", true, false, Some([0, 0, 0, 255]), 12.0)]), vec![("invisible", 1)]);
        // A white glyph can carry both findings: near-invisible paint and concealed characters.
        assert_eq!(classified(vec![glyph("\u{feff}", false, false, Some([255; 4]), 12.0)]), vec![("light-fill", 1), ("hidden-characters", 1)]);
        // Ordinary characters (and emoji ZWJ joins outside the flagged ranges) stay clean.
        assert!(classified(vec![glyph("ab", false, false, Some([0, 0, 0, 255]), 12.0)]).is_empty());
        assert!(hidden_kind('\u{1F468}').is_none());
    }

    #[test]
    fn actual_text_is_a_review_lens_on_top_of_the_shape_kinds() {
        // A replacement whose span paints nothing: both findings, the pair a caller reviews.
        let mut g = glyph("\u{feff}", true, false, Some([0, 0, 0, 255]), 12.0);
        g.actual_text = true;
        assert_eq!(classified(vec![g]), vec![("actual-text", 1), ("invisible", 1)]);
        // A painted replacement is still reported as a replacement.
        let mut g = glyph(".", false, false, Some([0, 0, 0, 255]), 12.0);
        g.actual_text = true;
        assert_eq!(classified(vec![g]), vec![("actual-text", 1)]);
    }

    #[test]
    fn samples_are_capped_and_escaped() {
        let glyphs: Vec<GlyphAudit> = (0..10).map(|i| glyph(&format!("h{i}"), true, false, Some([0, 0, 0, 255]), 12.0)).collect();
        let mut c = Classifier::new(Options::default());
        for g in &glyphs {
            c.push(g);
        }
        let report = c.finish(0);
        let f = &report.findings[0];
        assert_eq!(f.kind, "invisible");
        assert_eq!(f.count, 10);
        assert_eq!(f.samples.len(), MAX_SAMPLES);
        assert_eq!(f.rects.len(), MAX_SAMPLES);
        assert_eq!(f.samples[0], "h0");
    }

    #[test]
    fn long_samples_are_truncated() {
        let text: String = "ж".repeat(MAX_SAMPLE + 10);
        let mut c = Classifier::new(Options::default());
        c.push(&glyph(&text, true, false, Some([0, 0, 0, 255]), 12.0));
        let f = &c.finish(0).findings[0];
        assert_eq!(f.samples[0].chars().count(), MAX_SAMPLE + 1); // + the ellipsis
        assert!(f.samples[0].ends_with('…'));
    }

    #[test]
    fn hidden_kind_covers_the_documented_ranges() {
        for c in [
            '\u{200B}',
            '\u{200C}',
            '\u{200D}',
            '\u{2060}',
            '\u{2064}',
            '\u{FEFF}',
            '\u{202A}',
            '\u{202E}',
            '\u{2066}',
            '\u{2069}',
            '\u{E0000}',
            '\u{E007F}',
        ] {
            assert_eq!(hidden_kind(c), Some("hidden-characters"), "{c:?}");
        }
        for c in ['a', ' ', '\u{00AD}', '\u{200E}', '\u{2065}', '\u{2028}'] {
            assert_eq!(hidden_kind(c), None, "{c:?}");
        }
    }
}
