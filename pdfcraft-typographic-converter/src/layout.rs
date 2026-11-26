use crate::extract::RawGlyph;
use serde::{Deserialize, Serialize};

/// Typographic script classification for text runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ScriptType {
    Normal,
    Superscript,
    Subscript,
}

/// A contiguous segment of text sharing identical typographic styling.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TextRun {
    pub text: String,
    pub font_name: String,
    pub font_size: f32,
    pub is_bold: bool,
    pub is_italic: bool,
    pub script: ScriptType,
    pub has_leading_tab: bool,
    pub tracking_delta: f32,          // exact extra spacing in pt
    pub bounds: (f32, f32, f32, f32), // (min_x, min_y, max_x, max_y)
}

/// A horizontal line of clustered text runs sharing an approximate baseline.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TextLine {
    pub baseline_y: f32,
    pub runs: Vec<TextRun>,
    pub line_height: f32,
    pub bounds: (f32, f32, f32, f32), // (min_x, min_y, max_x, max_y)
}

/// An extracted embedded image (such as a barcode, QR code, or picture).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PageImage {
    #[serde(skip)]
    pub bytes: Vec<u8>,
    pub bounds: (f32, f32, f32, f32), // (left, top, right, bottom)
    pub width_pt: f32,
    pub height_pt: f32,
}

/// Layout representation of an individual document page.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PageLayout {
    pub page_index: usize,
    pub width: f32,
    pub height: f32,
    pub lines: Vec<TextLine>,
    pub images: Vec<PageImage>,
}

/// Intermediate page data containing glyphs and extracted images.
#[derive(Debug, Clone, PartialEq)]
pub struct ExtractedPageData {
    pub page_index: usize,
    pub width: f32,
    pub height: f32,
    pub glyphs: Vec<RawGlyph>,
    pub images: Vec<PageImage>,
    pub skipped_images: usize,
}

impl From<(usize, f32, f32, Vec<RawGlyph>)> for ExtractedPageData {
    fn from((page_index, width, height, glyphs): (usize, f32, f32, Vec<RawGlyph>)) -> Self {
        Self { page_index, width, height, glyphs, images: Vec::new(), skipped_images: 0 }
    }
}

impl From<(usize, f32, f32, Vec<RawGlyph>, Vec<PageImage>)> for ExtractedPageData {
    fn from((page_index, width, height, glyphs, images): (usize, f32, f32, Vec<RawGlyph>, Vec<PageImage>)) -> Self {
        Self { page_index, width, height, glyphs, images, skipped_images: 0 }
    }
}

/// Structured document representation containing the layout of all pages.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DocumentLayout {
    pub pages: Vec<PageLayout>,
}

/// Calculates the median value of a floating-point slice.
fn calculate_median(values: &[f32]) -> f32 {
    if values.is_empty() {
        return 0.0;
    }
    let mut sorted = values.to_vec();
    sorted.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let n = sorted.len();
    if n % 2 == 1 {
        sorted[n / 2]
    } else {
        (sorted[n / 2 - 1] + sorted[n / 2]) / 2.0
    }
}

/// Builds a structured `DocumentLayout` from per-page glyph and image collections.
///
/// Accepts `Vec<(usize, f32, f32, Vec<RawGlyph>)>`,
/// `Vec<(usize, f32, f32, Vec<RawGlyph>, Vec<PageImage>)>`, or `Vec<ExtractedPageData>`.
pub fn build_layout<I>(page_data: Vec<I>) -> DocumentLayout
where
    I: Into<ExtractedPageData>,
{
    let mut pages = Vec::with_capacity(page_data.len());

    for item in page_data {
        let p: ExtractedPageData = item.into();
        let lines = build_page_lines(p.glyphs);
        pages.push(PageLayout { page_index: p.page_index, width: p.width, height: p.height, lines, images: p.images });
    }

    DocumentLayout { pages }
}

fn is_arabic_digit_or_numeric_sep(c: char) -> bool {
    matches!(c, '\u{0660}'..='\u{0669}' | '\u{06F0}'..='\u{06F9}' | '\u{066B}' | '\u{066C}')
}

fn is_rtl_char(c: char) -> bool {
    if is_arabic_digit_or_numeric_sep(c) {
        return false;
    }
    matches!(
        c,
        '\u{0590}'..='\u{05FF}'
        | '\u{0600}'..='\u{06FF}'
        | '\u{0750}'..='\u{077F}'
        | '\u{08A0}'..='\u{08FF}'
        | '\u{FB1D}'..='\u{FB4F}'
        | '\u{FB50}'..='\u{FDFF}'
        | '\u{FE70}'..='\u{FEFF}'
    )
}

fn is_neutral_char(c: char) -> bool {
    c.is_whitespace() || matches!(c, '.' | ',' | '-' | '—' | ':' | ';' | '!' | '?' | '(' | ')' | '[' | ']' | '…' | '\u{066B}' | '\u{066C}')
}

fn normalized_x_bounds(g: &RawGlyph) -> (f32, f32) {
    (g.bounds.0.min(g.bounds.2), g.bounds.0.max(g.bounds.2))
}

fn cmp_ltr(a: &RawGlyph, b: &RawGlyph) -> std::cmp::Ordering {
    a.origin_x
        .partial_cmp(&b.origin_x)
        .unwrap_or(std::cmp::Ordering::Equal)
        .then_with(|| a.bounds.0.partial_cmp(&b.bounds.0).unwrap_or(std::cmp::Ordering::Equal))
}

fn cmp_rtl(a: &RawGlyph, b: &RawGlyph) -> std::cmp::Ordering {
    b.origin_x
        .partial_cmp(&a.origin_x)
        .unwrap_or(std::cmp::Ordering::Equal)
        .then_with(|| b.bounds.0.partial_cmp(&a.bounds.0).unwrap_or(std::cmp::Ordering::Equal))
}

fn cluster_lines(glyphs: Vec<RawGlyph>) -> Vec<Vec<RawGlyph>> {
    // Sort top-to-bottom in PDF coordinate space, then cluster within 2.0 pt of the cluster anchor.
    let mut sorted_by_y = glyphs;
    sorted_by_y.sort_by(|a, b| b.origin_y.partial_cmp(&a.origin_y).unwrap_or(std::cmp::Ordering::Equal));

    let mut line_clusters: Vec<Vec<RawGlyph>> = Vec::new();
    for glyph in sorted_by_y {
        if let Some(current_cluster) = line_clusters.last_mut() {
            let base_y = current_cluster.first().map(|g| g.origin_y).unwrap_or(glyph.origin_y);
            if (base_y - glyph.origin_y).abs() <= 2.0 {
                current_cluster.push(glyph);
                continue;
            }
        }
        line_clusters.push(vec![glyph]);
    }
    line_clusters
}

fn order_line_glyphs(mut cluster: Vec<RawGlyph>) -> Vec<(bool, RawGlyph)> {
    // Sort visually left-to-right first so baseline Y jitter cannot interleave LTR/RTL runs.
    cluster.sort_by(cmp_ltr);

    let has_rtl = cluster.iter().any(|g| is_rtl_char(g.char_code));
    let has_ltr = cluster.iter().any(|g| !is_rtl_char(g.char_code) && !is_neutral_char(g.char_code));

    if has_rtl && !has_ltr {
        cluster.sort_by(cmp_rtl);
        return cluster.into_iter().map(|g| (true, g)).collect();
    }
    if !has_rtl {
        return cluster.into_iter().map(|g| (false, g)).collect();
    }

    // Track whether the next non-neutral glyph in visual order is RTL so trailing boundary spaces
    // between RTL and LTR/digit runs are not moved to the opposite side when sorting RTL glyphs.
    let mut next_strong_is_rtl = vec![false; cluster.len()];
    let mut trailing_rtl = false;
    for (idx, g) in cluster.iter().enumerate().rev() {
        next_strong_is_rtl[idx] = trailing_rtl;
        if is_rtl_char(g.char_code) {
            trailing_rtl = true;
        } else if !is_neutral_char(g.char_code) {
            trailing_rtl = false;
        }
    }

    let mut segments: Vec<(bool, Vec<RawGlyph>)> = Vec::new();
    let mut current_direction = false;
    for (idx, g) in cluster.into_iter().enumerate() {
        let is_rtl = if is_rtl_char(g.char_code) {
            current_direction = true;
            true
        } else if is_neutral_char(g.char_code) {
            current_direction = current_direction && next_strong_is_rtl[idx];
            current_direction
        } else {
            current_direction = false;
            false
        };
        if let Some(last_seg) = segments.last_mut().filter(|seg| seg.0 == is_rtl) {
            last_seg.1.push(g);
        } else {
            segments.push((is_rtl, vec![g]));
        }
    }

    for (is_rtl, seg_glyphs) in &mut segments {
        if *is_rtl {
            seg_glyphs.sort_by(cmp_rtl);
        } else {
            seg_glyphs.sort_by(cmp_ltr);
        }
    }

    segments.sort_by(|(_, a), (_, b)| {
        let min_a = a.iter().map(|g| g.bounds.0.min(g.origin_x)).fold(f32::INFINITY, f32::min);
        let min_b = b.iter().map(|g| g.bounds.0.min(g.origin_x)).fold(f32::INFINITY, f32::min);
        min_a.partial_cmp(&min_b).unwrap_or(std::cmp::Ordering::Equal)
    });

    segments.into_iter().flat_map(|(is_rtl, seg)| seg.into_iter().map(move |g| (is_rtl, g))).collect()
}

fn classify_glyph_scripts(glyphs: &[(bool, RawGlyph)], baseline_y: f32, baseline_size: f32) -> Vec<ScriptType> {
    glyphs
        .iter()
        .map(|(_, g)| {
            let ratio = if baseline_size > 0.0 { g.font_size / baseline_size } else { 1.0 };
            if ratio < 0.78 && g.origin_y > baseline_y + 1.0 {
                ScriptType::Superscript
            } else if ratio < 0.78 && g.origin_y < baseline_y - 0.5 {
                ScriptType::Subscript
            } else {
                ScriptType::Normal
            }
        })
        .collect()
}

fn build_text_line(cluster: Vec<RawGlyph>) -> Option<TextLine> {
    if cluster.is_empty() {
        return None;
    }

    let origin_ys: Vec<f32> = cluster.iter().map(|g| g.origin_y).collect();
    let font_sizes: Vec<f32> = cluster.iter().map(|g| g.font_size).collect();
    let baseline_y = calculate_median(&origin_ys);
    let baseline_size = calculate_median(&font_sizes);

    let line_glyphs = order_line_glyphs(cluster);
    let glyph_scripts = classify_glyph_scripts(&line_glyphs, baseline_y, baseline_size);
    let runs = build_line_runs(&line_glyphs, &glyph_scripts);
    if runs.is_empty() {
        return None;
    }

    let line_min_x = runs.iter().map(|r| r.bounds.0).fold(f32::INFINITY, f32::min);
    let line_min_y = runs.iter().map(|r| r.bounds.1).fold(f32::INFINITY, f32::min);
    let line_max_x = runs.iter().map(|r| r.bounds.2).fold(f32::NEG_INFINITY, f32::max);
    let line_max_y = runs.iter().map(|r| r.bounds.3).fold(f32::NEG_INFINITY, f32::max);
    let line_height = if line_max_y > line_min_y { (line_max_y - line_min_y).max(baseline_size) } else { baseline_size };

    Some(TextLine { baseline_y, runs, line_height, bounds: (line_min_x, line_min_y, line_max_x, line_max_y) })
}

/// Clusters glyphs on a single page into structured lines and runs.
fn build_page_lines(glyphs: Vec<RawGlyph>) -> Vec<TextLine> {
    if glyphs.is_empty() {
        return Vec::new();
    }

    let mut lines: Vec<TextLine> = cluster_lines(glyphs).into_iter().filter_map(build_text_line).collect();

    // Sort lines top-to-bottom (descending by baseline Y, tie-breaking left-to-right)
    lines.sort_by(|a, b| {
        b.baseline_y
            .partial_cmp(&a.baseline_y)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| a.bounds.0.partial_cmp(&b.bounds.0).unwrap_or(std::cmp::Ordering::Equal))
    });

    lines
}

struct RunBuilder {
    text: String,
    bounds: Vec<(f32, f32, f32, f32)>,
    gaps: Vec<f32>,
    has_leading_tab: bool,
    font_name: String,
    font_size: f32,
    is_bold: bool,
    is_italic: bool,
    script: ScriptType,
}

impl RunBuilder {
    fn new(first: &RawGlyph, script: ScriptType) -> Self {
        Self {
            text: String::new(),
            bounds: Vec::new(),
            gaps: Vec::new(),
            has_leading_tab: false,
            font_name: first.font_name.clone(),
            font_size: first.font_size,
            is_bold: first.is_bold,
            is_italic: first.is_italic,
            script,
        }
    }

    fn matches_style(&self, g: &RawGlyph, script: ScriptType) -> bool {
        g.font_name == self.font_name
            && (g.font_size - self.font_size).abs() <= 0.01
            && g.is_bold == self.is_bold
            && g.is_italic == self.is_italic
            && script == self.script
    }

    fn push_glyph(&mut self, g: &RawGlyph) {
        if g.char_code != '\0' && (g.char_code != ' ' || !self.text.ends_with(' ')) {
            self.text.push(g.char_code);
        }
        let (min_x, max_x) = normalized_x_bounds(g);
        let min_y = g.bounds.1.min(g.bounds.3);
        let max_y = g.bounds.1.max(g.bounds.3);
        self.bounds.push((min_x, min_y, max_x, max_y));
    }

    fn flush_into(&mut self, runs: &mut Vec<TextRun>) {
        if self.text.is_empty() && self.bounds.is_empty() {
            return;
        }
        let tracking_delta = if self.gaps.is_empty() { 0.0 } else { self.gaps.iter().sum::<f32>() / self.gaps.len() as f32 };
        let bounds = if self.bounds.is_empty() {
            (0.0, 0.0, 0.0, 0.0)
        } else {
            let min_x = self.bounds.iter().map(|b| b.0).fold(f32::INFINITY, f32::min);
            let min_y = self.bounds.iter().map(|b| b.1).fold(f32::INFINITY, f32::min);
            let max_x = self.bounds.iter().map(|b| b.2).fold(f32::NEG_INFINITY, f32::max);
            let max_y = self.bounds.iter().map(|b| b.3).fold(f32::NEG_INFINITY, f32::max);
            (min_x, min_y, max_x, max_y)
        };
        runs.push(TextRun {
            text: std::mem::take(&mut self.text),
            font_name: self.font_name.clone(),
            font_size: self.font_size,
            is_bold: self.is_bold,
            is_italic: self.is_italic,
            script: self.script,
            has_leading_tab: self.has_leading_tab,
            tracking_delta,
            bounds,
        });
        self.gaps.clear();
        self.bounds.clear();
    }

    fn flush_and_reset(&mut self, runs: &mut Vec<TextRun>, next_g: &RawGlyph, next_script: ScriptType, next_leading_tab: bool) {
        self.flush_into(runs);
        self.has_leading_tab = next_leading_tab;
        self.font_name = next_g.font_name.clone();
        self.font_size = next_g.font_size;
        self.is_bold = next_g.is_bold;
        self.is_italic = next_g.is_italic;
        self.script = next_script;
    }
}

fn reading_gap(curr_rtl: bool, curr: &RawGlyph, next_rtl: bool, next: &RawGlyph) -> f32 {
    let (curr_min_x, curr_max_x) = normalized_x_bounds(curr);
    let (next_min_x, next_max_x) = normalized_x_bounds(next);
    if curr_rtl != next_rtl {
        if curr.char_code == ' ' || next.char_code == ' ' {
            0.0
        } else {
            next_min_x - curr_max_x
        }
    } else if curr_rtl {
        curr_min_x - next_max_x
    } else {
        next_min_x - curr_max_x
    }
}

/// Groups sorted line glyphs into `TextRun`s based on style changes, word spaces, and tabs.
fn build_line_runs(glyphs: &[(bool, RawGlyph)], scripts: &[ScriptType]) -> Vec<TextRun> {
    let mut runs: Vec<TextRun> = Vec::new();
    let Some((_, first_glyph)) = glyphs.first() else {
        return runs;
    };
    let mut builder = RunBuilder::new(first_glyph, scripts[0]);

    for (i, (curr_rtl, g)) in glyphs.iter().enumerate() {
        builder.push_glyph(g);

        let Some((next_rtl, next_g)) = glyphs.get(i + 1) else {
            builder.flush_into(&mut runs);
            break;
        };
        let next_script = scripts[i + 1];
        let gap = reading_gap(*curr_rtl, g, *next_rtl, next_g);
        let style_changed = !builder.matches_style(next_g, next_script);

        if gap >= g.font_size * 1.5 {
            builder.flush_and_reset(&mut runs, next_g, next_script, true);
        } else if gap >= g.font_size * 0.22 {
            if !builder.text.ends_with(' ') {
                builder.text.push(' ');
            }
            if style_changed {
                builder.flush_and_reset(&mut runs, next_g, next_script, false);
            }
        } else if style_changed {
            builder.flush_and_reset(&mut runs, next_g, next_script, false);
        } else {
            builder.gaps.push(gap);
        }
    }

    runs
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_glyph(ch: char, font: &str, x: f32, y: f32) -> RawGlyph {
        RawGlyph {
            char_code: ch,
            font_name: font.into(),
            font_size: 12.0,
            is_bold: false,
            is_italic: false,
            bounds: (x, y, x + 8.0, y + 10.0),
            origin_x: x,
            origin_y: y,
        }
    }

    #[test]
    fn rtl_segment_sorts_by_descending_origin_x_across_jittered_y() {
        // Same baseline cluster (within 2pt) where y-sorting places 'ל' (y=501) before 'ש' (y=500).
        let page = ExtractedPageData {
            page_index: 0,
            width: 400.0,
            height: 600.0,
            glyphs: vec![
                test_glyph('ש', "David", 80.0, 500.0),
                test_glyph('ל', "David", 70.0, 501.0),
                test_glyph('ו', "David", 60.0, 500.0),
                test_glyph('ם', "David", 50.0, 500.0),
            ],
            images: Vec::new(),
            skipped_images: 0,
        };
        let layout = build_layout(vec![page]);
        let text: String = layout.pages[0].lines[0].runs.iter().map(|r| r.text.as_str()).collect();
        assert_eq!(text, "שלום");
    }

    #[test]
    fn arabic_indic_and_extended_digits_stay_ltr_order() {
        let page = ExtractedPageData {
            page_index: 0,
            width: 400.0,
            height: 600.0,
            glyphs: vec![
                test_glyph('٣', "Arial", 66.0, 500.0),
                test_glyph('١', "Arial", 50.0, 500.5),
                test_glyph('٢', "Arial", 58.0, 500.0),
                test_glyph(' ', "Arial", 75.0, 500.0),
                test_glyph('۹', "Arial", 98.0, 500.0),
                test_glyph('۰', "Arial", 82.0, 500.0),
                test_glyph('۵', "Arial", 90.0, 501.0),
            ],
            images: Vec::new(),
            skipped_images: 0,
        };
        let layout = build_layout(vec![page]);
        let text: String = layout.pages[0].lines[0].runs.iter().map(|r| r.text.as_str()).collect();
        assert_eq!(text, "١٢٣ ۰۵۹");
    }

    #[test]
    fn mixed_arabic_letters_and_arabic_indic_digits_preserve_respective_orders() {
        // Arabic word "عدد" on the left (x=50..74) followed by space and LTR digits "١٢٣" (x=88..112).
        let page = ExtractedPageData {
            page_index: 0,
            width: 400.0,
            height: 600.0,
            glyphs: vec![
                test_glyph('ع', "Arial", 70.0, 500.0),
                test_glyph('د', "Arial", 60.0, 500.0),
                test_glyph('د', "Arial", 50.0, 500.0),
                test_glyph(' ', "Arial", 80.0, 500.0),
                test_glyph('٣', "Arial", 104.0, 500.0),
                test_glyph('١', "Arial", 88.0, 500.2),
                test_glyph('٢', "Arial", 96.0, 500.0),
            ],
            images: Vec::new(),
            skipped_images: 0,
        };
        let layout = build_layout(vec![page]);
        let text: String = layout.pages[0].lines[0].runs.iter().map(|r| r.text.as_str()).collect();
        assert_eq!(text, "عدد ١٢٣");
    }
}
