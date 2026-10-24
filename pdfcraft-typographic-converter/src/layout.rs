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
    pub tracking_delta: f32, // exact extra spacing in pt
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
}

impl From<(usize, f32, f32, Vec<RawGlyph>)> for ExtractedPageData {
    fn from((page_index, width, height, glyphs): (usize, f32, f32, Vec<RawGlyph>)) -> Self {
        Self {
            page_index,
            width,
            height,
            glyphs,
            images: Vec::new(),
        }
    }
}

impl From<(usize, f32, f32, Vec<RawGlyph>, Vec<PageImage>)> for ExtractedPageData {
    fn from((page_index, width, height, glyphs, images): (usize, f32, f32, Vec<RawGlyph>, Vec<PageImage>)) -> Self {
        Self {
            page_index,
            width,
            height,
            glyphs,
            images,
        }
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
        pages.push(PageLayout {
            page_index: p.page_index,
            width: p.width,
            height: p.height,
            lines,
            images: p.images,
        });
    }

    DocumentLayout { pages }
}

/// Clusters glyphs on a single page into structured lines and runs.
fn build_page_lines(glyphs: Vec<RawGlyph>) -> Vec<TextLine> {
    if glyphs.is_empty() {
        return Vec::new();
    }

    // 1. Line Clustering:
    // Sort glyphs by origin_y descending (top-to-bottom in PDF coordinate space).
    let mut sorted_by_y = glyphs;
    sorted_by_y.sort_by(|a, b| {
        b.origin_y
            .partial_cmp(&a.origin_y)
            .unwrap_or(std::cmp::Ordering::Equal)
    });

    // Cluster glyphs into the same line if |glyph_a.origin_y - glyph_b.origin_y| <= 2.0 pt.
    let mut line_clusters: Vec<Vec<RawGlyph>> = Vec::new();
    for glyph in sorted_by_y {
        if let Some(current_cluster) = line_clusters.last_mut() {
            let prev = current_cluster.last().unwrap();
            if (prev.origin_y - glyph.origin_y).abs() <= 2.0 {
                current_cluster.push(glyph);
                continue;
            }
        }
        line_clusters.push(vec![glyph]);
    }

    // 2. Process each line cluster
    let mut lines = Vec::with_capacity(line_clusters.len());
    for cluster in line_clusters {
        if cluster.is_empty() {
            continue;
        }

        // Calculate baseline Y and reference baseline size (median font size)
        let origin_ys: Vec<f32> = cluster.iter().map(|g| g.origin_y).collect();
        let font_sizes: Vec<f32> = cluster.iter().map(|g| g.font_size).collect();

        let baseline_y = calculate_median(&origin_ys);
        let baseline_size = calculate_median(&font_sizes);

        // Sort glyphs left-to-right (ascending by origin_x, tie-breaking by left bounds)
        let mut line_glyphs = cluster;
        line_glyphs.sort_by(|a, b| {
            a.origin_x
                .partial_cmp(&b.origin_x)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| {
                    a.bounds
                        .0
                        .partial_cmp(&b.bounds.0)
                        .unwrap_or(std::cmp::Ordering::Equal)
                })
        });

        // Determine script type for each glyph
        let glyph_scripts: Vec<ScriptType> = line_glyphs
            .iter()
            .map(|g| {
                let ratio = if baseline_size > 0.0 {
                    g.font_size / baseline_size
                } else {
                    1.0
                };
                if ratio < 0.78 && g.origin_y > baseline_y + 1.0 {
                    ScriptType::Superscript
                } else if ratio < 0.78 && g.origin_y < baseline_y - 0.5 {
                    ScriptType::Subscript
                } else {
                    ScriptType::Normal
                }
            })
            .collect();

        // 3. Script & Style Run Grouping with Word Gaps & Tab Detection
        let runs = build_line_runs(&line_glyphs, &glyph_scripts);
        if runs.is_empty() {
            continue;
        }

        // Calculate line bounding box enclosing all runs
        let line_min_x = runs
            .iter()
            .map(|r| r.bounds.0)
            .fold(f32::INFINITY, f32::min);
        let line_min_y = runs
            .iter()
            .map(|r| r.bounds.1)
            .fold(f32::INFINITY, f32::min);
        let line_max_x = runs
            .iter()
            .map(|r| r.bounds.2)
            .fold(f32::NEG_INFINITY, f32::max);
        let line_max_y = runs
            .iter()
            .map(|r| r.bounds.3)
            .fold(f32::NEG_INFINITY, f32::max);

        let line_bounds = (line_min_x, line_min_y, line_max_x, line_max_y);
        let line_height = if line_max_y > line_min_y {
            (line_max_y - line_min_y).max(baseline_size)
        } else {
            baseline_size
        };

        lines.push(TextLine {
            baseline_y,
            runs,
            line_height,
            bounds: line_bounds,
        });
    }

    // Sort lines top-to-bottom (descending by baseline Y, tie-breaking left-to-right)
    lines.sort_by(|a, b| {
        b.baseline_y
            .partial_cmp(&a.baseline_y)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| {
                a.bounds
                    .0
                    .partial_cmp(&b.bounds.0)
                    .unwrap_or(std::cmp::Ordering::Equal)
            })
    });

    lines
}

/// Flushes accumulated run properties into a finalized `TextRun`.
#[allow(clippy::too_many_arguments)]
fn flush_run(
    runs: &mut Vec<TextRun>,
    text: String,
    font_name: &str,
    font_size: f32,
    is_bold: bool,
    is_italic: bool,
    script: ScriptType,
    has_leading_tab: bool,
    inter_char_gaps: Vec<f32>,
    glyph_bounds: Vec<(f32, f32, f32, f32)>,
) {
    if text.is_empty() && glyph_bounds.is_empty() {
        return;
    }

    let tracking_delta = if inter_char_gaps.is_empty() {
        0.0
    } else {
        inter_char_gaps.iter().sum::<f32>() / inter_char_gaps.len() as f32
    };

    let bounds = if glyph_bounds.is_empty() {
        (0.0, 0.0, 0.0, 0.0)
    } else {
        let min_x = glyph_bounds
            .iter()
            .map(|b| b.0)
            .fold(f32::INFINITY, f32::min);
        let min_y = glyph_bounds
            .iter()
            .map(|b| b.1)
            .fold(f32::INFINITY, f32::min);
        let max_x = glyph_bounds
            .iter()
            .map(|b| b.2)
            .fold(f32::NEG_INFINITY, f32::max);
        let max_y = glyph_bounds
            .iter()
            .map(|b| b.3)
            .fold(f32::NEG_INFINITY, f32::max);
        (min_x, min_y, max_x, max_y)
    };

    runs.push(TextRun {
        text,
        font_name: font_name.to_string(),
        font_size,
        is_bold,
        is_italic,
        script,
        has_leading_tab,
        tracking_delta,
        bounds,
    });
}

/// Groups sorted line glyphs into `TextRun`s based on style changes, word spaces, and tabs.
fn build_line_runs(glyphs: &[RawGlyph], scripts: &[ScriptType]) -> Vec<TextRun> {
    let mut runs: Vec<TextRun> = Vec::new();
    if glyphs.is_empty() {
        return runs;
    }

    let mut current_text = String::new();
    let mut current_bounds: Vec<(f32, f32, f32, f32)> = Vec::new();
    let mut current_gaps: Vec<f32> = Vec::new();
    let mut has_leading_tab = false;

    let mut run_font_name = glyphs[0].font_name.clone();
    let mut run_font_size = glyphs[0].font_size;
    let mut run_is_bold = glyphs[0].is_bold;
    let mut run_is_italic = glyphs[0].is_italic;
    let mut run_script = scripts[0];

    for i in 0..glyphs.len() {
        let g = &glyphs[i];

        // Append character, preventing duplicate spaces
        if g.char_code != '\0' && (g.char_code != ' ' || !current_text.ends_with(' ')) {
            current_text.push(g.char_code);
        }

        // Store glyph bounding box normalized as (min_x, min_y, max_x, max_y)
        let min_x = g.bounds.0.min(g.bounds.2);
        let max_x = g.bounds.0.max(g.bounds.2);
        let min_y = g.bounds.1.min(g.bounds.3);
        let max_y = g.bounds.1.max(g.bounds.3);
        current_bounds.push((min_x, min_y, max_x, max_y));

        if i + 1 < glyphs.len() {
            let next_g = &glyphs[i + 1];
            let next_script = scripts[i + 1];

            // Horizontal gap between current glyph right and next glyph left
            let gap = next_g.bounds.0 - g.bounds.2;

            let style_changed = next_g.font_name != run_font_name
                || (next_g.font_size - run_font_size).abs() > 0.01
                || next_g.is_bold != run_is_bold
                || next_g.is_italic != run_is_italic
                || next_script != run_script;

            if gap >= g.font_size * 1.5 {
                // Tab stop: flush current run and mark has_leading_tab on next run
                flush_run(
                    &mut runs,
                    std::mem::take(&mut current_text),
                    &run_font_name,
                    run_font_size,
                    run_is_bold,
                    run_is_italic,
                    run_script,
                    has_leading_tab,
                    std::mem::take(&mut current_gaps),
                    std::mem::take(&mut current_bounds),
                );
                has_leading_tab = true;
                run_font_name = next_g.font_name.clone();
                run_font_size = next_g.font_size;
                run_is_bold = next_g.is_bold;
                run_is_italic = next_g.is_italic;
                run_script = next_script;
            } else if gap >= g.font_size * 0.22 {
                // Word space: append space to run
                if !current_text.ends_with(' ') {
                    current_text.push(' ');
                }
                if style_changed {
                    flush_run(
                        &mut runs,
                        std::mem::take(&mut current_text),
                        &run_font_name,
                        run_font_size,
                        run_is_bold,
                        run_is_italic,
                        run_script,
                        has_leading_tab,
                        std::mem::take(&mut current_gaps),
                        std::mem::take(&mut current_bounds),
                    );
                    has_leading_tab = false;
                    run_font_name = next_g.font_name.clone();
                    run_font_size = next_g.font_size;
                    run_is_bold = next_g.is_bold;
                    run_is_italic = next_g.is_italic;
                    run_script = next_script;
                }
            } else if gap < -1.0 {
                // Overlapping accents / kerning adjustments: preserve glyph without spurious spaces
                if style_changed {
                    flush_run(
                        &mut runs,
                        std::mem::take(&mut current_text),
                        &run_font_name,
                        run_font_size,
                        run_is_bold,
                        run_is_italic,
                        run_script,
                        has_leading_tab,
                        std::mem::take(&mut current_gaps),
                        std::mem::take(&mut current_bounds),
                    );
                    has_leading_tab = false;
                    run_font_name = next_g.font_name.clone();
                    run_font_size = next_g.font_size;
                    run_is_bold = next_g.is_bold;
                    run_is_italic = next_g.is_italic;
                    run_script = next_script;
                } else {
                    current_gaps.push(gap);
                }
            } else {
                // Normal character spacing (-1.0 <= gap < font_size * 0.22)
                if style_changed {
                    flush_run(
                        &mut runs,
                        std::mem::take(&mut current_text),
                        &run_font_name,
                        run_font_size,
                        run_is_bold,
                        run_is_italic,
                        run_script,
                        has_leading_tab,
                        std::mem::take(&mut current_gaps),
                        std::mem::take(&mut current_bounds),
                    );
                    has_leading_tab = false;
                    run_font_name = next_g.font_name.clone();
                    run_font_size = next_g.font_size;
                    run_is_bold = next_g.is_bold;
                    run_is_italic = next_g.is_italic;
                    run_script = next_script;
                } else {
                    current_gaps.push(gap);
                }
            }
        } else {
            // End of line: flush final run
            flush_run(
                &mut runs,
                std::mem::take(&mut current_text),
                &run_font_name,
                run_font_size,
                run_is_bold,
                run_is_italic,
                run_script,
                has_leading_tab,
                std::mem::take(&mut current_gaps),
                std::mem::take(&mut current_bounds),
            );
        }
    }

    runs
}
