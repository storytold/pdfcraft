//! Text extraction (bootstrap for `pdfcraft-text`, architecture §8).
//!
//! A hayro `Device` that ignores paint and records every glyph: its Unicode value (ToUnicode →
//! encoding → glyph name fallbacks, handled by hayro) and its box in *page view space*, i.e.
//! points with the origin at the top-left of the displayed page, rotation applied, y down. The UI
//! maps view space to the screen with a single scale.
//!
//! On top of the glyph list: line grouping (baseline clustering), word gaps, plain-text output,
//! case-insensitive search and reading-order selection.

use hayro::hayro_interpret::font::Glyph;
use hayro::hayro_interpret::hayro_syntax::Pdf;
use hayro::hayro_interpret::{
    BlendMode, ClipPath, Context, Device, GlyphDrawMode, Image, InterpreterCache, InterpreterSettings, Paint, PathDrawMode, SoftMask, interpret_page,
};
use kurbo::{Affine, BezPath, Rect, Shape};

/// One glyph on the page.
#[derive(Clone, Debug, PartialEq)]
pub struct TextGlyph {
    pub text: String,
    /// Box in page view space (points, y down): [x0, y0, x1, y1].
    pub rect: [f32; 4],
}

/// The text of one page, in content order, with line structure.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct PageText {
    pub glyphs: Vec<TextGlyph>,
    /// For each glyph: its line index (lines are in reading order, top to bottom).
    pub line_of: Vec<u32>,
    /// `true` when a word break precedes the glyph on the same line.
    pub space_before: Vec<bool>,
}

impl PageText {
    pub fn is_empty(&self) -> bool {
        self.glyphs.is_empty()
    }

    /// Text of glyphs `range` with spaces and line breaks reconstructed.
    pub fn text_of(&self, range: std::ops::Range<usize>) -> String {
        let mut s = String::new();
        let mut prev_line = None;
        for i in range.clone() {
            let Some(g) = self.glyphs.get(i) else { break };
            if i != range.start {
                if prev_line != Some(self.line_of[i]) {
                    s.push('\n');
                } else if self.space_before[i] {
                    s.push(' ');
                }
            }
            s.push_str(&g.text);
            prev_line = Some(self.line_of[i]);
        }
        s
    }

    pub fn plain_text(&self) -> String {
        self.text_of(0..self.glyphs.len())
    }

    /// Case-insensitive search. Returns glyph ranges of every match (words may span line breaks,
    /// which count as a single space).
    pub fn find(&self, needle: &str) -> Vec<std::ops::Range<usize>> {
        self.find_opts(needle, false, false)
    }

    /// Search with Acrobat's find options: case-sensitive, whole words only.
    pub fn find_opts(&self, needle: &str, case_sensitive: bool, whole_words: bool) -> Vec<std::ops::Range<usize>> {
        let fold = |s: &str| if case_sensitive { s.to_string() } else { s.to_lowercase() };
        let needle: Vec<char> = fold(needle).split_whitespace().collect::<Vec<_>>().join(" ").chars().collect();
        if needle.is_empty() {
            return Vec::new();
        }
        // Flatten to chars with the glyph index each char came from.
        let mut chars: Vec<(char, usize)> = Vec::new();
        for (i, g) in self.glyphs.iter().enumerate() {
            if i > 0 && (self.space_before[i] || self.line_of[i] != self.line_of[i - 1]) && chars.last().is_some_and(|c| c.0 != ' ') {
                chars.push((' ', i));
            }
            for c in fold(&g.text).chars() {
                chars.push((if c.is_whitespace() { ' ' } else { c }, i));
            }
        }
        let word = |k: Option<&(char, usize)>| k.is_some_and(|c| c.0.is_alphanumeric());
        let mut out = Vec::new();
        let mut i = 0;
        while i + needle.len() <= chars.len() {
            let bounded = !whole_words || (!word(i.checked_sub(1).and_then(|p| chars.get(p))) && !word(chars.get(i + needle.len())));
            if bounded && chars[i..i + needle.len()].iter().map(|c| c.0).eq(needle.iter().copied()) {
                let start = chars[i].1;
                let end = chars[i + needle.len() - 1].1 + 1;
                out.push(start..end);
                i += needle.len();
            } else {
                i += 1;
            }
        }
        out
    }

    /// Glyph ranges of the matches a character matcher finds in the page text (words and lines
    /// separated by one space, original case). Used for pattern search (Search & Redact).
    pub fn find_with(&self, matcher: impl Fn(&[char]) -> Vec<std::ops::Range<usize>>) -> Vec<std::ops::Range<usize>> {
        let mut chars: Vec<char> = Vec::new();
        let mut owner: Vec<usize> = Vec::new();
        for (i, g) in self.glyphs.iter().enumerate() {
            if i > 0 && (self.space_before[i] || self.line_of[i] != self.line_of[i - 1]) && chars.last().is_some_and(|c| *c != ' ') {
                chars.push(' ');
                owner.push(i);
            }
            for c in g.text.chars() {
                chars.push(if c.is_whitespace() { ' ' } else { c });
                owner.push(i);
            }
        }
        matcher(&chars).into_iter().filter(|r| r.start < r.end && r.end <= chars.len()).map(|r| owner[r.start]..owner[r.end - 1] + 1).collect()
    }

    /// Index of the glyph nearest to a view-space point (for selection anchors).
    pub fn nearest(&self, x: f32, y: f32) -> Option<usize> {
        self.glyphs
            .iter()
            .enumerate()
            .map(|(i, g)| {
                let dx = if x < g.rect[0] {
                    g.rect[0] - x
                } else if x > g.rect[2] {
                    x - g.rect[2]
                } else {
                    0.0
                };
                let dy = if y < g.rect[1] {
                    g.rect[1] - y
                } else if y > g.rect[3] {
                    y - g.rect[3]
                } else {
                    0.0
                };
                // Prefer the same line: vertical distance weighs more.
                (i, dx + dy * 3.0)
            })
            .min_by(|a, b| a.1.total_cmp(&b.1))
            .map(|(i, _)| i)
    }

    /// Merge the boxes of `range` into one rectangle per line (for highlighting).
    pub fn line_rects(&self, range: std::ops::Range<usize>) -> Vec<[f32; 4]> {
        let mut out: Vec<(u32, [f32; 4])> = Vec::new();
        for i in range {
            let Some(g) = self.glyphs.get(i) else { break };
            let line = self.line_of[i];
            match out.last_mut() {
                Some((l, r)) if *l == line => {
                    r[0] = r[0].min(g.rect[0]);
                    r[1] = r[1].min(g.rect[1]);
                    r[2] = r[2].max(g.rect[2]);
                    r[3] = r[3].max(g.rect[3]);
                }
                _ => out.push((line, g.rect)),
            }
        }
        out.into_iter().map(|(_, r)| r).collect()
    }
}

struct TextDevice {
    glyphs: Vec<TextGlyph>,
    /// How many glyphs run in each view direction: right, down, left, up.
    directions: [usize; 4],
}

impl<'a> Device<'a> for TextDevice {
    fn set_soft_mask(&mut self, _: Option<SoftMask<'a>>) {}
    fn set_blend_mode(&mut self, _: BlendMode) {}
    fn draw_path(&mut self, _: &BezPath, _: Affine, _: &Paint<'a>, _: &PathDrawMode) {}
    fn push_clip_path(&mut self, _: &ClipPath) {}
    fn push_transparency_group(&mut self, _: f32, _: Option<SoftMask<'a>>, _: BlendMode) {}
    fn draw_glyph(&mut self, glyph: &Glyph<'a>, transform: Affine, glyph_transform: Affine, _: &Paint<'a>, _: &GlyphDrawMode) {
        let Some(u) = glyph.as_unicode() else {
            if std::env::var_os("PDFCRAFT_TEXT_DEBUG").is_some() {
                eprintln!("NOUNICODE {} {:?}", if matches!(glyph, Glyph::Type3(_)) { "t3" } else { "ol" }, (transform * glyph_transform).as_coeffs());
            }
            return;
        };
        let text = match u {
            hayro::hayro_interpret::hayro_cmap::BfString::Char(c) => c.to_string(),
            hayro::hayro_interpret::hayro_cmap::BfString::String(s) => s,
        };
        if text.chars().all(|c| c.is_control()) {
            return;
        }
        // Glyph space → view space. Glyph space uses 1000 units per em for outline glyphs; the em
        // box spans descender (−200) to ascender (800), the advance gives the width.
        let t = transform * glyph_transform;
        if std::env::var_os("PDFCRAFT_TEXT_DEBUG").is_some() {
            let kind = if matches!(glyph, Glyph::Type3(_)) { "t3" } else { "ol" };
            eprintln!("{kind} {text:?} transform={:?} glyph_transform={:?}", transform.as_coeffs(), glyph_transform.as_coeffs());
        }
        let advance = match glyph {
            Glyph::Outline(g) => {
                g.advance_width().filter(|a| *a > 0.0).map(f64::from).unwrap_or_else(|| g.outline().bounding_box().width().max(500.0))
            }
            Glyph::Type3(g) => g.advance_width().filter(|a| a.is_finite() && *a > 0.0).map(f64::from).unwrap_or(600.0),
        };
        // The baseline direction in view space (y down), to the nearest quarter turn.
        let (dx, dy) = (t.as_coeffs()[0], t.as_coeffs()[1]);
        let dir = if dx.abs() >= dy.abs() {
            if dx >= 0.0 { 0 } else { 2 }
        } else if dy > 0.0 {
            1
        } else {
            3
        };
        self.directions[dir] += text.chars().count();
        let em = Rect::new(0.0, -200.0, advance, 800.0);
        let b = (t * em.to_path(0.1)).bounding_box();
        if !(b.x0.is_finite() && b.y0.is_finite() && b.x1.is_finite() && b.y1.is_finite()) || b.width() > 10_000.0 || b.height() > 10_000.0 {
            return;
        }
        for (i, ch) in text.chars().enumerate() {
            // Ligatures (e.g. "ffi") share the glyph box, split evenly.
            let n = text.chars().count().max(1) as f64;
            let w = b.width() / n;
            let x0 = b.x0 + w * i as f64;
            self.glyphs.push(TextGlyph { text: ch.to_string(), rect: [x0 as f32, b.y0 as f32, (x0 + w) as f32, b.y1 as f32] });
        }
    }
    fn draw_image(&mut self, _: Image<'a, '_>, _: Affine) {}
    fn pop_clip_path(&mut self) {}
    fn pop_transparency_group(&mut self) {}
}

/// Extract the text of one page. Panics inside the interpreter are the caller's to catch.
pub(crate) fn extract_page(pdf: &Pdf, page: usize, settings: &InterpreterSettings) -> Option<PageText> {
    let pages = pdf.pages();
    let p = pages.get(page)?;
    let (w, h) = p.render_dimensions();
    let cache = InterpreterCache::new();
    // Annotation appearances are included: form-field values and comment text are searchable,
    // as in Acrobat, and match what other extractors report.
    let settings = settings.clone();
    let initial = p.initial_transform(true).to_kurbo();
    let mut ctx = Context::new(initial, Rect::new(0.0, 0.0, w as f64, h as f64), &cache, p.xref(), settings);
    let mut dev = TextDevice { glyphs: Vec::new(), directions: [0; 4] };
    interpret_page(p, &mut ctx, &mut dev);
    // Lay out in the frame where most text runs left to right, so a page shown rotated (by
    // `/Rotate` or by its content matrix) still reads along its lines; then map the boxes back.
    // Pages whose `/Rotate` is cancelled by counter-rotated content stay as they are.
    let quarter = (0..4u8).max_by_key(|q| (dev.directions[*q as usize], *q == 0)).unwrap_or(0);
    for g in &mut dev.glyphs {
        g.rect = to_upright(g.rect, quarter, w, h);
    }
    let mut text = layout(dev.glyphs);
    for g in &mut text.glyphs {
        g.rect = from_upright(g.rect, quarter, w, h);
    }
    Some(text)
}

/// View-space box → the frame where text runs left to right, for text running `quarter` × 90°
/// clockwise from that in a `w` × `h` view (1: text runs downwards).
fn to_upright(r: [f32; 4], quarter: u8, w: f32, h: f32) -> [f32; 4] {
    let [x0, y0, x1, y1] = r;
    match quarter {
        1 => [y0, w - x1, y1, w - x0],
        2 => [w - x1, h - y1, w - x0, h - y0],
        3 => [h - y1, x0, h - y0, x1],
        _ => r,
    }
}

/// The inverse of [`to_upright`].
fn from_upright(r: [f32; 4], quarter: u8, w: f32, h: f32) -> [f32; 4] {
    let [u0, v0, u1, v1] = r;
    match quarter {
        1 => [w - v1, u0, w - v0, u1],
        2 => [w - u1, h - v1, w - u0, h - v0],
        3 => [v0, h - u1, v1, h - u0],
        _ => r,
    }
}

use hayro::hayro_interpret::TransformExt;

fn is_rtl(c: char) -> bool {
    matches!(c as u32, 0x0590..=0x08FF | 0xFB1D..=0xFDFF | 0xFE70..=0xFEFF | 0x10800..=0x10FFF | 0x1E800..=0x1EFFF)
}

/// Scripts written with explicit spaces whose combining signs can sit apart from their base
/// (Indic, Thai, Lao, Myanmar, Khmer): never infer a space inside a tight cluster.
fn is_complex(c: char) -> bool {
    matches!(c as u32, 0x0300..=0x036F | 0x0900..=0x0DFF | 0x0E00..=0x0EFF | 0x1000..=0x109F | 0x1780..=0x17FF)
}

fn is_cjk(c: char) -> bool {
    matches!(c as u32, 0x2E80..=0x9FFF | 0xAC00..=0xD7AF | 0xF900..=0xFAFF | 0xFF00..=0xFFEF | 0x20000..=0x3134F | 0x1100..=0x11FF)
}

struct Seg {
    idx: Vec<usize>,
    bbox: [f32; 4],
    h: f32,
}

impl Seg {
    fn cy(&self) -> f32 {
        (self.bbox[1] + self.bbox[3]) / 2.0
    }
}

fn union(a: &mut [f32; 4], b: &[f32; 4]) {
    a[0] = a[0].min(b[0]);
    a[1] = a[1].min(b[1]);
    a[2] = a[2].max(b[2]);
    a[3] = a[3].max(b[3]);
}

/// Reading-order layout. Glyphs are reordered so that `glyphs` is in reading order, lines are
/// numbered in that order, and word gaps are inferred per line.
///
/// Heuristics (documented so they can be tuned and tested; plan/architecture.md §8):
/// 1. *Segments:* consecutive glyphs (content order) on one baseline with no column-sized gap.
///    Segments on the same baseline that nearly touch are merged (text drawn out of order).
/// 2. *Blocks:* segments stacked with line-sized gaps and overlapping horizontally.
/// 3. *Block order:* repeatedly take the top-most remaining block, preferring a block to its left
///    that overlaps it vertically (so columns read left, then right).
/// 4. Within a segment glyphs run left to right; runs of right-to-left script are reversed so the
///    text comes out in logical order.
pub fn layout(glyphs: Vec<TextGlyph>) -> PageText {
    // Drop "fake bold" duplicates: the same character redrawn at (almost) the same place.
    let mut glyphs = glyphs;
    let mut keep = vec![true; glyphs.len()];
    for i in 1..glyphs.len() {
        let g = &glyphs[i];
        let h = (g.rect[3] - g.rect[1]).max(0.1);
        for j in (i.saturating_sub(4)..i).rev() {
            let p = &glyphs[j];
            if keep[j] && p.text == g.text && (p.rect[0] - g.rect[0]).abs() < h * 0.2 && (p.rect[1] - g.rect[1]).abs() < h * 0.2 {
                keep[i] = false;
                break;
            }
        }
    }
    if keep.iter().any(|k| !k) {
        let mut it = keep.iter();
        glyphs.retain(|_| *it.next().unwrap_or(&true));
    }
    let n = glyphs.len();
    if n == 0 {
        return PageText::default();
    }
    let height = |i: usize| (glyphs[i].rect[3] - glyphs[i].rect[1]).max(0.1);
    let cy = |i: usize| (glyphs[i].rect[1] + glyphs[i].rect[3]) / 2.0;

    // 1. Segments in content order.
    let mut segs: Vec<Seg> = Vec::new();
    for i in 0..n {
        let g = glyphs[i].rect;
        let cont = segs.last().is_some_and(|s| {
            let Some(&p) = s.idx.last() else { return false };
            let ph = height(p).min(height(i));
            let gap = g[0] - glyphs[p].rect[2];
            (cy(i) - cy(p)).abs() < ph * 0.5 && gap < ph * 3.0 && g[0] > glyphs[p].rect[0] - ph * 2.0
        });
        if cont && let Some(s) = segs.last_mut() {
            s.idx.push(i);
            union(&mut s.bbox, &g);
            s.h = s.h.max(height(i));
        } else {
            segs.push(Seg { idx: vec![i], bbox: g, h: height(i) });
        }
    }
    // Merge same-baseline segments that nearly touch.
    let mut merged = true;
    while merged {
        merged = false;
        'outer: for a in 0..segs.len() {
            for b in 0..segs.len() {
                if a == b {
                    continue;
                }
                let (sa, sb) = (&segs[a], &segs[b]);
                let h = sa.h.min(sb.h);
                let gap = (sb.bbox[0] - sa.bbox[2]).max(sa.bbox[0] - sb.bbox[2]);
                if (sa.cy() - sb.cy()).abs() < h * 0.3 && (sa.h / sb.h - 1.0).abs() < 0.35 && gap < h * 1.2 {
                    let sb = segs.remove(b);
                    let a = if b < a { a - 1 } else { a };
                    let sa = &mut segs[a];
                    sa.idx.extend(sb.idx);
                    union(&mut sa.bbox, &sb.bbox);
                    sa.h = sa.h.max(sb.h);
                    merged = true;
                    break 'outer;
                }
            }
        }
    }
    for s in &mut segs {
        s.idx.sort_by(|a, b| glyphs[*a].rect[0].total_cmp(&glyphs[*b].rect[0]));
    }

    // 2. Blocks.
    let mut by_top: Vec<usize> = (0..segs.len()).collect();
    by_top.sort_by(|a, b| segs[*a].bbox[1].total_cmp(&segs[*b].bbox[1]));
    let mut blocks: Vec<(Vec<usize>, [f32; 4])> = Vec::new();
    for si in by_top {
        let s = &segs[si];
        let target = blocks.iter().position(|(members, bb)| {
            let Some(&m) = members.last() else { return false };
            let last = &segs[m];
            let vgap = s.bbox[1] - last.bbox[3];
            let overlap = s.bbox[2].min(bb[2]) - s.bbox[0].max(bb[0]);
            let minw = (s.bbox[2] - s.bbox[0]).min(bb[2] - bb[0]).max(1.0);
            vgap > -last.h * 0.5 && vgap < last.h.max(s.h) * 1.1 && overlap > minw * 0.3 && (last.h / s.h - 1.0).abs() < 0.6
        });
        match target {
            Some(b) => {
                blocks[b].0.push(si);
                let bb = s.bbox;
                union(&mut blocks[b].1, &bb);
            }
            None => blocks.push((vec![si], s.bbox)),
        }
    }

    // 3. Block order.
    let mut remaining: Vec<usize> = (0..blocks.len()).collect();
    let mut block_order = Vec::with_capacity(blocks.len());
    while let Some(&top) = remaining.iter().min_by(|a, b| blocks[**a].1[1].total_cmp(&blocks[**b].1[1])) {
        let tb = blocks[top].1;
        let pick = remaining
            .iter()
            .copied()
            .filter(|c| {
                let cb = blocks[*c].1;
                let v = cb[3].min(tb[3]) - cb[1].max(tb[1]);
                cb[2] <= tb[0] + 1.0 && v > 0.5 * (cb[3] - cb[1]).min(tb[3] - tb[1])
            })
            .min_by(|a, b| blocks[*a].1[0].total_cmp(&blocks[*b].1[0]))
            .unwrap_or(top);
        remaining.retain(|r| *r != pick);
        block_order.push(pick);
    }

    // 4. Emit glyphs in reading order with line numbers and word gaps.
    let mut order: Vec<usize> = Vec::with_capacity(n);
    let mut line_of = Vec::with_capacity(n);
    let mut space_before = Vec::with_capacity(n);
    let mut line = 0u32;
    for b in block_order {
        for si in &blocks[b].0 {
            let s = &segs[*si];
            let mut idx = s.idx.clone();
            // Reverse right-to-left runs (visual → logical).
            let rtl = |i: usize| glyphs[i].text.chars().any(is_rtl);
            let mut k = 0;
            while k < idx.len() {
                if rtl(idx[k]) {
                    let mut e = k;
                    while e + 1 < idx.len()
                        && (rtl(idx[e + 1]) || (glyphs[idx[e + 1]].text.trim().is_empty() && e + 2 < idx.len() && rtl(idx[e + 2])))
                    {
                        e += 1;
                    }
                    idx[k..=e].reverse();
                    k = e + 1;
                } else {
                    k += 1;
                }
            }
            // Word gaps relative to this line's typical letter gap (handles tracking).
            let mut gaps: Vec<f32> = s.idx.windows(2).map(|w| glyphs[w[1]].rect[0] - glyphs[w[0]].rect[2]).collect();
            gaps.sort_by(f32::total_cmp);
            let typical = gaps.get(gaps.len() / 3).copied().unwrap_or(0.0).max(0.0);
            let threshold = (typical + s.h * 0.15).max(s.h * 0.15);
            let mut spaces = vec![false; idx.len()];
            for w in 1..s.idx.len() {
                let (p, c) = (s.idx[w - 1], s.idx[w]);
                let gap = glyphs[c].rect[0] - glyphs[p].rect[2];
                let (pt, ct) = (&glyphs[p].text, &glyphs[c].text);
                let cjk = pt.chars().any(is_cjk) && ct.chars().any(is_cjk) && gap < s.h * 0.5;
                let tight_cluster = (pt.chars().any(is_complex) || ct.chars().any(is_complex)) && gap < s.h * 0.6;
                if gap > threshold && !cjk && !tight_cluster && !pt.trim().is_empty() && !ct.trim().is_empty() {
                    // Mark the space before glyph `c` wherever it ended up after RTL reversal.
                    if let Some(pos) = idx.iter().position(|x| *x == c.max(p)) {
                        spaces[pos] = true;
                    }
                }
            }
            for (k, g) in idx.iter().enumerate() {
                order.push(*g);
                line_of.push(line);
                space_before.push(k > 0 && spaces[k]);
            }
            line += 1;
        }
    }
    let mut slots: Vec<Option<TextGlyph>> = glyphs.into_iter().map(Some).collect();
    // Each glyph is emitted exactly once.
    let glyphs = order.iter().filter_map(|i| slots.get_mut(*i).and_then(Option::take)).collect();
    PageText { glyphs, line_of, space_before }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn upright_mapping_round_trips() {
        let r = [10.0, 20.0, 30.0, 25.0];
        for q in 0..4 {
            assert_eq!(from_upright(to_upright(r, q, 300.0, 200.0), q, 300.0, 200.0), r, "quarter {q}");
        }
        // A 90° page: the view's top-right corner is the upright top-left.
        assert_eq!(to_upright([290.0, 0.0, 300.0, 10.0], 1, 300.0, 200.0), [0.0, 0.0, 10.0, 10.0]);
    }

    fn g(t: &str, x0: f32, y0: f32, x1: f32) -> TextGlyph {
        TextGlyph { text: t.into(), rect: [x0, y0, x1, y0 + 10.0] }
    }

    #[test]
    fn lines_words_and_search() {
        // "Hello world" on line 1, "Next" on line 2.
        let mut v = Vec::new();
        for (i, c) in "Hello".chars().enumerate() {
            v.push(g(&c.to_string(), 10.0 + i as f32 * 6.0, 10.0, 16.0 + i as f32 * 6.0));
        }
        for (i, c) in "world".chars().enumerate() {
            v.push(g(&c.to_string(), 50.0 + i as f32 * 6.0, 10.0, 56.0 + i as f32 * 6.0));
        }
        for (i, c) in "Next".chars().enumerate() {
            v.push(g(&c.to_string(), 10.0 + i as f32 * 6.0, 30.0, 16.0 + i as f32 * 6.0));
        }
        let t = layout(v);
        assert_eq!(t.plain_text(), "Hello world\nNext");
        assert_eq!(t.find("WORLD next"), vec![5..14]);
        assert_eq!(t.find("o"), vec![4..5, 6..7]);
        assert_eq!(t.line_rects(3..12).len(), 2);
        assert_eq!(t.nearest(52.0, 14.0), Some(5));
    }

    fn word(v: &mut Vec<TextGlyph>, s: &str, x: f32, y: f32, advance: f32) {
        for (i, c) in s.chars().enumerate() {
            let x0 = x + i as f32 * advance;
            v.push(g(&c.to_string(), x0, y, x0 + 6.0));
        }
    }

    #[test]
    fn fake_bold_duplicates_are_removed() {
        let mut v = Vec::new();
        for (i, c) in "Bold".chars().enumerate() {
            let x = 10.0 + i as f32 * 7.0;
            v.push(g(&c.to_string(), x, 10.0, x + 6.0));
            v.push(g(&c.to_string(), x + 0.3, 10.0, x + 6.3));
        }
        assert_eq!(layout(v).plain_text(), "Bold");
    }

    #[test]
    fn two_columns_read_left_then_right_even_if_drawn_interleaved() {
        let mut v = Vec::new();
        // Drawn row by row across both columns, as some producers do.
        word(&mut v, "Left1", 10.0, 10.0, 6.0);
        word(&mut v, "Right1", 200.0, 10.0, 6.0);
        word(&mut v, "Left2", 10.0, 24.0, 6.0);
        word(&mut v, "Right2", 200.0, 24.0, 6.0);
        let t = layout(v);
        assert_eq!(t.plain_text(), "Left1\nLeft2\nRight1\nRight2");
    }

    #[test]
    fn tracked_text_keeps_letters_together() {
        let mut v = Vec::new();
        word(&mut v, "CHAPTER", 10.0, 10.0, 9.0); // 3pt tracking between 6pt-wide letters
        word(&mut v, "FOUR", 10.0 + 7.0 * 9.0 + 5.0, 10.0, 9.0);
        assert_eq!(layout(v).plain_text(), "CHAPTER FOUR");
    }

    #[test]
    fn rtl_runs_come_out_in_logical_order_and_cjk_has_no_spaces() {
        let mut v = Vec::new();
        // Hebrew "שלום" drawn visually right-to-left: ם ו ל ש from left to right.
        word(&mut v, "םולש", 10.0, 10.0, 6.0);
        assert_eq!(layout(v).plain_text(), "שלום");
        let mut v = Vec::new();
        word(&mut v, "每个字", 10.0, 10.0, 9.0);
        assert_eq!(layout(v).plain_text(), "每个字");
    }
}
