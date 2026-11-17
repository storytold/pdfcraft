//! Hybrid outline auto-generation: Tier 1 Smart TOC parser with `/PageLabels`
//! resolution and Tier 2 4-stage statistical typography analyzer.

use std::collections::{HashMap, HashSet};

use pdfcraft_content::Matrix;
use pdfcraft_cos::Document;
use pdfcraft_organize::{OutlineEntry, PageLabels, sanitize_bookmark_title};
use regex::RegexBuilder;

use crate::EditError;

const MAX_PAGES_SCAN: usize = 5_000;
const MAX_TOC_SCAN_PAGES: usize = 15;
const MAX_HEADING_LINE_CHARS: usize = 120;
const MAX_TITLE_CHARS: usize = 128;
const REGEX_SIZE_LIMIT: usize = 256 * 1024;
const REGEX_NEST_LIMIT: u32 = 64;

/// Options controlling [`generate`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AutoGenerateOptions {
    pub max_depth: u8,
    pub prefer_toc: bool,
    pub replace_existing: bool,
}

impl Default for AutoGenerateOptions {
    fn default() -> Self {
        Self { max_depth: 3, prefer_toc: true, replace_existing: true }
    }
}

#[derive(Clone, Debug)]
struct NormLine {
    page: usize,
    text: String,
    user_top: f64,
    disp: [f64; 4],
    disp_h: f64,
    size: f64,
    bold: bool,
    italic: bool,
}

impl NormLine {
    fn disp_ymid(&self) -> f64 {
        (self.disp[1] + self.disp[3]) * 0.5
    }
}

#[derive(Clone, Debug)]
struct CoalescedRow {
    text: String,
    disp_left: f64,
}

/// Automatically extract document outline entries using Tier 1 (TOC) then Tier 2 (Typography).
pub fn generate(doc: &Document, options: &AutoGenerateOptions) -> Result<Vec<OutlineEntry>, EditError> {
    let max_depth = options.max_depth.clamp(1, 6);
    let pages = pdfcraft_model::pages(doc);
    let total_pages = pages.len().min(MAX_PAGES_SCAN);
    if total_pages == 0 {
        return Err(EditError::Invalid("this document has no pages".into()));
    }

    let extracted = extract_pages(doc, &pages[..total_pages]);
    if extracted.iter().all(Vec::is_empty) {
        return Err(EditError::Invalid("no extractable text was found to generate bookmarks".into()));
    }

    if options.prefer_toc
        && let Some(toc_entries) = try_parse_toc(doc, &extracted, total_pages, max_depth)
    {
        return Ok(toc_entries);
    }

    let entries = extract_typography_headings(&extracted, total_pages, max_depth);
    if entries.is_empty() {
        return Err(EditError::Invalid("no headings were found to create bookmarks".into()));
    }
    Ok(entries)
}

fn extract_pages(doc: &Document, pages: &[pdfcraft_model::Page]) -> Vec<Vec<NormLine>> {
    let mut out = Vec::with_capacity(pages.len());
    for (idx, p) in pages.iter().enumerate() {
        let (_, disp_h) = p.display_size(doc);
        let to_disp = Matrix(p.view_matrix(doc)).invert();
        let Ok(raw) = pdfcraft_edit::text_lines(doc, idx) else {
            out.push(Vec::new());
            continue;
        };
        let mut lines = Vec::new();
        for l in raw {
            let text = l.text.split_whitespace().collect::<Vec<_>>().join(" ");
            let valid_box = l.rect.iter().all(|v| v.is_finite());
            if text.is_empty() || !l.size.is_finite() || l.size <= 0.0 || !valid_box {
                continue;
            }
            let norm_user = [l.rect[0].min(l.rect[2]), l.rect[1].min(l.rect[3]), l.rect[0].max(l.rect[2]), l.rect[1].max(l.rect[3])];
            let disp = to_disp.map_or(norm_user, |m| m.bbox(norm_user));
            let user_top = norm_user[3];
            lines.push(NormLine { page: idx, text, user_top, disp, disp_h: disp_h.max(1.0), size: l.size, bold: l.bold, italic: l.italic });
        }
        out.push(lines);
    }
    out
}

fn coalesce_same_baseline(lines: &[NormLine]) -> Vec<CoalescedRow> {
    let mut sorted: Vec<&NormLine> = lines.iter().collect();
    sorted.sort_by(|a, b| b.disp_ymid().total_cmp(&a.disp_ymid()).then(a.disp[0].total_cmp(&b.disp[0])));

    let mut groups: Vec<Vec<&NormLine>> = Vec::new();
    for line in sorted {
        let tol = (line.size.max(4.0) * 0.35).min(12.0);
        if let Some(last) = groups.last_mut()
            && last.first().is_some_and(|first| (first.disp_ymid() - line.disp_ymid()).abs() <= tol)
        {
            last.push(line);
        } else {
            groups.push(vec![line]);
        }
    }

    let mut rows = Vec::with_capacity(groups.len());
    for mut g in groups {
        g.sort_by(|a, b| a.disp[0].total_cmp(&b.disp[0]));
        let Some(first) = g.first() else { continue };
        let mut text = first.text.clone();
        let mut prev_right = first.disp[2];
        for seg in g.iter().skip(1) {
            let gap = seg.disp[0] - prev_right;
            let wide_gap = gap > seg.size.max(6.0) * 0.9;
            text.push_str(if wide_gap { "   " } else { " " });
            text.push_str(&seg.text);
            prev_right = prev_right.max(seg.disp[2]);
        }
        rows.push(CoalescedRow { text, disp_left: first.disp[0] });
    }
    rows
}

fn build_regex(pattern: &str, case_insensitive: bool) -> Option<regex::Regex> {
    RegexBuilder::new(pattern)
        .case_insensitive(case_insensitive)
        .size_limit(REGEX_SIZE_LIMIT)
        .dfa_size_limit(REGEX_SIZE_LIMIT)
        .nest_limit(REGEX_NEST_LIMIT)
        .build()
        .ok()
}

fn try_parse_toc(doc: &Document, pages: &[Vec<NormLine>], total_pages: usize, max_depth: u8) -> Option<Vec<OutlineEntry>> {
    let header_re = build_regex(r"^(?:table\s+of\s+contents|contents|index|outline|inhalt|sommaire)\b", true)?;
    let leader_re = build_regex(
        r"^(?P<title>.+?)\s*(?:[.\u{2024}\u{2026}\x{00B7}\u{2022}_-]\s*){2,}(?P<page>[^\s.\u{2024}\u{2026}\x{00B7}\u{2022}_-]\S{0,31})\s*$",
        false,
    )?;
    let gap_re = build_regex(r"^(?P<title>.+?)\s{2,}(?P<page>\S{1,32})\s*$", false)?;
    let num_re = build_regex(r"^(?P<num>\d+(?:\.\d+)*)[.)]?\s+", false)?;

    let label_lookup = build_page_label_map(doc, total_pages);
    let scan_limit = total_pages.min(MAX_TOC_SCAN_PAGES);

    struct RawTocItem {
        title: String,
        target_page: usize,
        indent: f64,
    }

    let mut collected: Vec<RawTocItem> = Vec::new();
    let mut saw_toc_header = false;
    let mut toc_page_streak = 0usize;

    for (page_idx, page_lines) in pages.iter().enumerate().take(scan_limit) {
        if page_lines.is_empty() {
            if toc_page_streak > 0 {
                break;
            }
            continue;
        }
        let rows = coalesce_same_baseline(page_lines);
        let page_has_header = rows.iter().take(4).any(|r| header_re.is_match(r.text.trim()));
        saw_toc_header |= page_has_header;

        let mut prev_resolved = collected.last().map(|c| c.target_page);
        let mut page_hits = Vec::new();
        for row in &rows {
            let trimmed = row.text.trim();
            let caps = leader_re.captures(trimmed).or_else(|| if page_has_header || toc_page_streak > 0 { gap_re.captures(trimmed) } else { None });
            let Some(caps) = caps else { continue };
            let raw_title = caps.name("title").map_or("", |m| m.as_str());
            let cleaned = clean_toc_title(raw_title);
            if cleaned.is_empty() || cleaned.chars().all(|c| c.is_ascii_digit() || !c.is_alphanumeric()) {
                continue;
            }
            let Some(token) = caps.name("page").map(|m| m.as_str()) else { continue };
            if let Some(target_page) = resolve_toc_page(&label_lookup, token, total_pages, page_idx, prev_resolved) {
                prev_resolved = Some(target_page);
                page_hits.push(RawTocItem { title: cleaned, target_page, indent: row.disp_left });
            }
        }

        let min_hits = if page_has_header { 2 } else { 3 };
        if page_hits.len() >= min_hits {
            toc_page_streak = toc_page_streak.saturating_add(1);
            collected.extend(page_hits);
        } else if toc_page_streak > 0 {
            break;
        }
    }

    let required_total = if saw_toc_header { 2 } else { 3 };
    if collected.len() < required_total {
        return None;
    }

    // Build indent tiers (nearest distinct cluster within 6 pt) mapped 1..=max_depth.
    let mut indents: Vec<f64> = Vec::new();
    for item in &collected {
        if !indents.iter().any(|&x| (x - item.indent).abs() <= 6.0) {
            indents.push(item.indent);
        }
    }
    indents.sort_by(f64::total_cmp);

    let mut entries = Vec::with_capacity(collected.len());
    for item in collected {
        let indent_level = indents.iter().position(|&x| (x - item.indent).abs() <= 6.0).map_or(1, |pos| pos.saturating_add(1));
        let num_level = num_re.captures(&item.title).and_then(|c| c.name("num")).map(|m| m.as_str().split('.').count()).unwrap_or(1);
        let raw_level = if indents.len() > 1 { indent_level } else { num_level };
        let level = u8::try_from(raw_level).unwrap_or(max_depth).clamp(1, max_depth);
        let title = sanitize_bookmark_title(&item.title, MAX_TITLE_CHARS);
        if title.is_empty() {
            continue;
        }
        entries.push(OutlineEntry { level, title, page: item.target_page, element: None, top: None });
    }

    normalize_monotonic_levels(&mut entries, max_depth);
    (!entries.is_empty()).then_some(entries)
}

fn clean_toc_title(raw: &str) -> String {
    let stripped = raw.trim().trim_end_matches(['.', '\u{2024}', '\u{2026}', '\u{00B7}', '\u{2022}', '_', '-', ' ']);
    sanitize_bookmark_title(stripped, MAX_TITLE_CHARS)
}

fn build_page_label_map(doc: &Document, total_pages: usize) -> HashMap<String, Vec<usize>> {
    let mut map: HashMap<String, Vec<usize>> = HashMap::new();
    let Ok(labels) = PageLabels::new(doc) else { return map };
    for p in 0..total_pages {
        if let Ok(lbl) = labels.label(p) {
            let key = lbl.trim().to_lowercase();
            if !key.is_empty() {
                map.entry(key).or_default().push(p);
            }
        }
    }
    map
}

fn resolve_toc_page(
    label_map: &HashMap<String, Vec<usize>>,
    token: &str,
    total_pages: usize,
    toc_page: usize,
    prev_resolved: Option<usize>,
) -> Option<usize> {
    let key = token.trim().to_lowercase();
    if let Some(candidates) = label_map.get(&key) {
        if let Some(prev) = prev_resolved
            && let Some(&p) = candidates.iter().find(|&&p| p > toc_page && p >= prev)
        {
            return Some(p);
        }
        if let Some(&p) = candidates.iter().find(|&&p| p > toc_page) {
            return Some(p);
        }
    }
    // Numeric fallback when document labels differ from printed numbering.
    if let Ok(n) = key.parse::<usize>()
        && n >= 1
    {
        let p = n.saturating_sub(1);
        if p > toc_page && p < total_pages && prev_resolved.is_none_or(|prev| p >= prev) {
            return Some(p);
        }
    }
    None
}

fn normalize_for_header_footer(text: &str) -> String {
    text.trim().chars().map(|c| if c.is_ascii_digit() { '#' } else { c.to_ascii_lowercase() }).collect()
}

fn is_header_or_footer_excluded(
    line: &NormLine,
    top_counts: &HashMap<String, usize>,
    bot_counts: &HashMap<String, usize>,
    total_pages: usize,
) -> bool {
    if total_pages < 3 {
        return false;
    }
    let ymid = line.disp_ymid();
    let key = normalize_for_header_footer(&line.text);
    let in_top = ymid >= line.disp_h * 0.92;
    let in_bot = ymid <= line.disp_h * 0.08;
    if in_top && top_counts.get(&key).is_some_and(|&n| n * 10 > total_pages * 3) {
        return true;
    }
    if in_bot && bot_counts.get(&key).is_some_and(|&n| n * 10 > total_pages * 3) {
        return true;
    }
    false
}

fn collect_repeating_margin_counts(pages: &[Vec<NormLine>]) -> (HashMap<String, usize>, HashMap<String, usize>) {
    let mut top_pages: HashMap<String, HashSet<usize>> = HashMap::new();
    let mut bot_pages: HashMap<String, HashSet<usize>> = HashMap::new();
    for (p_idx, lines) in pages.iter().enumerate() {
        for l in lines {
            let ymid = l.disp_ymid();
            if ymid >= l.disp_h * 0.92 {
                top_pages.entry(normalize_for_header_footer(&l.text)).or_default().insert(p_idx);
            } else if ymid <= l.disp_h * 0.08 {
                bot_pages.entry(normalize_for_header_footer(&l.text)).or_default().insert(p_idx);
            }
        }
    }
    let top = top_pages.into_iter().map(|(k, v)| (k, v.len())).collect();
    let bot = bot_pages.into_iter().map(|(k, v)| (k, v.len())).collect();
    (top, bot)
}

#[derive(Clone, Debug)]
struct HeadingCandidate {
    page: usize,
    text: String,
    user_top: f64,
    size: f64,
    bold: bool,
    italic: bool,
    bottom_disp: f64,
    top_disp: f64,
    num_depth: Option<u8>,
}

fn extract_typography_headings(pages: &[Vec<NormLine>], total_pages: usize, max_depth: u8) -> Vec<OutlineEntry> {
    let (top_counts, bot_counts) = collect_repeating_margin_counts(pages);
    let numbered_re = build_regex(r"^(?:(?P<num>\d+(?:\.\d+)*)[.)]?\s+\S|(?:Chapter|Section|Part|Appendix)\s+[\dIVXLCDMA-Z])", true);
    let url_re = build_regex(r"^(?:https?://|www\.)", true);

    // Stage B: character-weighted body font baseline (half-point buckets).
    let mut size_chars: HashMap<i32, usize> = HashMap::new();
    let mut non_bold_size_chars: HashMap<i32, usize> = HashMap::new();
    let mut bold_chars: usize = 0;
    let mut non_bold_chars: usize = 0;

    let mut content_lines: Vec<&NormLine> = Vec::new();
    for lines in pages {
        for l in lines {
            if is_header_or_footer_excluded(l, &top_counts, &bot_counts, total_pages) {
                continue;
            }
            content_lines.push(l);
            let chars = l.text.chars().filter(|c| !c.is_whitespace()).count();
            let bucket = (l.size * 2.0).round() as i32;
            *size_chars.entry(bucket).or_insert(0) += chars;
            if l.bold {
                bold_chars = bold_chars.saturating_add(chars);
            } else {
                non_bold_chars = non_bold_chars.saturating_add(chars);
                *non_bold_size_chars.entry(bucket).or_insert(0) += chars;
            }
        }
    }

    let total_chars = bold_chars.saturating_add(non_bold_chars);
    // When non-bold body text forms a meaningful share (>= 20%), use its modal size so sparse
    // brochures/slides with short body paragraphs still distinguish larger/bold headings.
    let body_is_bold = non_bold_chars.saturating_mul(5) < total_chars;
    let baseline_map = if body_is_bold || non_bold_size_chars.is_empty() { &size_chars } else { &non_bold_size_chars };
    let modal_bucket = baseline_map.iter().max_by_key(|&(_, count)| *count).map_or(24, |(&b, _)| b);
    let s_body = f64::from(modal_bucket) * 0.5;

    // Stage C: collect & coalesce heading candidates.
    let mut raw_candidates: Vec<HeadingCandidate> = Vec::new();
    let mut uninterrupted_heading = false;
    for l in content_lines {
        let len = l.text.chars().count();
        if !(2..=MAX_HEADING_LINE_CHARS).contains(&len) {
            uninterrupted_heading = false;
            continue;
        }
        if l.text.chars().all(|c| c.is_ascii_digit() || !c.is_alphanumeric()) {
            uninterrupted_heading = false;
            continue;
        }
        if url_re.as_ref().is_some_and(|re| re.is_match(&l.text)) {
            uninterrupted_heading = false;
            continue;
        }
        let is_larger = l.size >= s_body * 1.15 && (l.size - s_body) >= 0.9;
        let is_bold_contrast = !body_is_bold && l.bold && l.size >= s_body * 0.95;
        let num_match = numbered_re.as_ref().and_then(|re| re.captures(&l.text));
        let num_depth = num_match
            .as_ref()
            .map(|caps| caps.name("num").and_then(|m| u8::try_from(m.as_str().split('.').filter(|s| !s.is_empty()).count()).ok()).unwrap_or(1));
        let ends_sentence = l.text.ends_with(['.', ',', ';']);
        let qualifies = is_larger || is_bold_contrast || (num_match.is_some() && !ends_sentence);
        if !qualifies || (!is_larger && ends_sentence) {
            uninterrupted_heading = false;
            continue;
        }

        if uninterrupted_heading
            && let Some(prev) = raw_candidates.last_mut()
            && prev.page == l.page
            && prev.bold == l.bold
            && prev.italic == l.italic
            && (prev.size - l.size).abs() <= 0.6
            && num_match.is_none()
            && (prev.bottom_disp - l.disp[3]).abs() <= l.size * 1.6
            && prev.text.chars().count().saturating_add(l.text.chars().count()) <= MAX_TITLE_CHARS
        {
            prev.text.push(' ');
            prev.text.push_str(&l.text);
            prev.bottom_disp = prev.bottom_disp.min(l.disp[1]);
            prev.top_disp = prev.top_disp.max(l.disp[3]);
            uninterrupted_heading = true;
            continue;
        }

        raw_candidates.push(HeadingCandidate {
            page: l.page,
            text: l.text.clone(),
            user_top: l.user_top,
            size: l.size,
            bold: l.bold,
            italic: l.italic,
            bottom_disp: l.disp[1],
            top_disp: l.disp[3],
            num_depth,
        });
        uninterrupted_heading = true;
    }

    if raw_candidates.is_empty() {
        return Vec::new();
    }

    // Natural-breaks style clustering: largest font size first, then bold first.
    let mut style_tiers: Vec<(i32, bool)> = raw_candidates.iter().map(|c| ((c.size * 2.0).round() as i32, c.bold)).collect();
    style_tiers.sort_unstable_by(|a, b| b.0.cmp(&a.0).then(b.1.cmp(&a.1)));
    style_tiers.dedup();

    // Cluster nearby sizes (<= 1.0 pt = 2 half-points gap when same bold flag) into level tiers.
    let mut clusters: Vec<(i32, bool)> = Vec::new();
    for tier in style_tiers {
        if clusters.last().is_some_and(|&(sz, b)| b == tier.1 && (sz - tier.0).abs() <= 2) {
            continue;
        }
        clusters.push(tier);
    }

    let mut entries = Vec::with_capacity(raw_candidates.len());
    for c in raw_candidates {
        let bucket = (c.size * 2.0).round() as i32;
        let cluster_idx = clusters
            .iter()
            .position(|&(sz, b)| b == c.bold && (sz - bucket).abs() <= 2)
            .or_else(|| clusters.iter().position(|&(sz, _)| (sz - bucket).abs() <= 2))
            .unwrap_or(clusters.len().saturating_sub(1));
        let style_level = u8::try_from(cluster_idx.saturating_add(1)).unwrap_or(max_depth);
        let level = if clusters.len() == 1 { c.num_depth.unwrap_or(style_level) } else { style_level }.clamp(1, max_depth);

        let title = sanitize_bookmark_title(&c.text, MAX_TITLE_CHARS);
        if title.is_empty() {
            continue;
        }
        entries.push(OutlineEntry { level, title, page: c.page, element: None, top: Some(c.user_top) });
    }

    normalize_monotonic_levels(&mut entries, max_depth);
    entries
}

/// Normalize outline entry levels via a Monotonic Stack so depth never skips a level.
fn normalize_monotonic_levels(entries: &mut [OutlineEntry], max_depth: u8) {
    // Stack of raw input levels along the active chain; emitted depth is `stack.len()`.
    let mut stack: Vec<u8> = Vec::new();
    for entry in entries {
        let raw = entry.level.max(1);
        while stack.last().is_some_and(|&top| top >= raw) {
            stack.pop();
        }
        stack.push(raw);
        let emitted = u8::try_from(stack.len()).unwrap_or(max_depth).clamp(1, max_depth);
        entry.level = emitted;
    }
}
