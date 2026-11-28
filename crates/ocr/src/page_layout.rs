//! Page layout analysis: a page's recognised words clustered into lines, the lines grouped into
//! typed regions, and the regions numbered in reading order.
//!
//! The input is what a recogniser reports — [`Word`]s with boxes in image pixels, top-left
//! origin — plus the page's size in those pixels. The output is a list of [`Region`]s: a
//! [`Rect`], the [`Line`]s inside it, a [`RegionKind`] and a reading-order index. A repeated
//! header or footer can only be told from a first-page heading by looking at the pages before,
//! so `previous` takes the preceding page's regions; it is `None` for a first page, where
//! position alone has to stand for repetition.
//!
//! Regions are typed from their text and position: an `Exhibit` label, a `Dated:`/`Sworn to`/
//! `By:` line, a running header or footer, a numbered or lettered list item, the party/caption
//! block at the top of a legal page, and plain body text for the rest. [`RegionKind::Table`],
//! [`RegionKind::Picture`] and [`RegionKind::Other`] are defined for the callers that add those
//! regions from page content (rules, images, unreadable graphics); this pass recognises words
//! and cannot see them.

use crate::{Line, Word};

/// What a region of a page holds.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum RegionKind {
    /// Body text: the default, and everything no other rule claims.
    Text,
    /// A table (not inferred by this pass: see the module documentation).
    Table,
    /// A picture (not inferred by this pass: see the module documentation).
    Picture,
    /// A running header: a line in the top margin that the previous page repeats.
    Header,
    /// A running footer: a line in the bottom margin that the previous page repeats.
    Footer,
    /// One item of a numbered or lettered list.
    ListItem,
    /// The party/caption block at the top of a legal page.
    Caption,
    /// A signature block: a `Dated:`, `Sworn to` or `By:` line.
    Signature,
    /// An exhibit label or the region an exhibit label heads.
    Exhibit,
    /// Anything else (not inferred by this pass: see the module documentation).
    Other,
}

/// An axis-aligned box in the space of [`Word::rect`]: image pixels, top-left origin.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Rect {
    pub left: f32,
    pub top: f32,
    pub right: f32,
    pub bottom: f32,
}

impl Rect {
    pub fn new(left: f32, top: f32, right: f32, bottom: f32) -> Rect {
        Rect { left, top, right, bottom }
    }

    /// The box around `self` and `other`.
    pub fn union(self, other: Rect) -> Rect {
        Rect {
            left: self.left.min(other.left),
            top: self.top.min(other.top),
            right: self.right.max(other.right),
            bottom: self.bottom.max(other.bottom),
        }
    }

    pub fn width(self) -> f32 {
        self.right - self.left
    }

    pub fn height(self) -> f32 {
        self.bottom - self.top
    }
}

/// One typed region of a page: its box, the lines it holds in reading order, and where it comes
/// in the page's reading order (0 is first).
#[derive(Clone, Debug, PartialEq)]
pub struct Region {
    pub kind: RegionKind,
    pub rect: Rect,
    pub lines: Vec<Line>,
    pub order: usize,
}

/// Split `words` into typed regions in reading order, for a page `page_size` (width, height)
/// pixels, given the previous page's regions (`None` for a first page) so a header or footer
/// can be told from a heading.
pub fn analyze_page(words: &[Word], page_size: (f32, f32), previous: Option<&[Region]>) -> Vec<Region> {
    let mut blocks: Vec<(Vec<Line>, Rect)> = Vec::new();
    for line in cluster_lines(words) {
        let rect = line_rect(&line);
        // Newest block first: on a page in columns the line above may be the other one's, and
        // the block it continues is further back.
        let hit = blocks.iter().enumerate().rev().find_map(|(i, (_, block))| same_block(*block, rect).then_some(i));
        match hit {
            Some(i) => {
                if let Some((lines, block)) = blocks.get_mut(i) {
                    *block = block.union(rect);
                    lines.push(line);
                }
            }
            None => blocks.push((vec![line], rect)),
        }
    }
    // Reading order: down the page, and left to right between blocks that start on one row.
    blocks.sort_by(|a, b| a.1.top.total_cmp(&b.1.top).then(a.1.left.total_cmp(&b.1.left)));
    let height = page_height(page_size);
    blocks
        .into_iter()
        .enumerate()
        .map(|(order, (lines, rect))| Region { kind: classify(&lines, rect, height, previous), rect, lines, order })
        .collect()
}

/// The page's height when it is a usable measurement (`None` for a zero, negative or non-finite
/// height: no margin or caption band can be computed from it).
fn page_height(page_size: (f32, f32)) -> Option<f32> {
    (page_size.1.is_finite() && page_size.1 > 0.0).then_some(page_size.1)
}

/// The words clustered into lines in reading order (down the page, and left to right along a
/// row), each word sorted along its line. Words whose box is not a box are left out.
fn cluster_lines(words: &[Word]) -> Vec<Line> {
    let mut boxes: Vec<&Word> = words.iter().filter(|w| usable(w)).collect();
    boxes.sort_by(|a, b| a.rect[1].total_cmp(&b.rect[1]).then(a.rect[0].total_cmp(&b.rect[0])));
    let mut lines: Vec<(Line, Rect)> = Vec::new();
    for word in boxes {
        let rect = word_rect(word);
        let row = lines.iter().position(|(_, line)| joins_row(*line, rect));
        match row {
            Some(i) => {
                if let Some((line, extent)) = lines.get_mut(i) {
                    *extent = extent.union(rect);
                    line.words.push(word.clone());
                }
            }
            None => lines.push((Line { words: vec![word.clone()] }, rect)),
        }
    }
    for (line, _) in &mut lines {
        line.words.sort_by(|a, b| a.rect[0].total_cmp(&b.rect[0]));
    }
    lines.sort_by(|a, b| a.1.top.total_cmp(&b.1.top).then(a.1.left.total_cmp(&b.1.left)));
    lines.into_iter().map(|(line, _)| line).collect()
}

/// An axis-aligned box with a positive, finite extent: what a word or line can be measured from.
fn usable(word: &Word) -> bool {
    let [l, t, r, b] = word.rect;
    [l, t, r, b].iter().all(|v| v.is_finite()) && r > l && b > t && !word.text.trim().is_empty()
}

fn word_rect(word: &Word) -> Rect {
    Rect::new(word.rect[0], word.rect[1], word.rect[2], word.rect[3])
}

/// The box around every word of `line`.
fn line_rect(line: &Line) -> Rect {
    line.words.iter().map(word_rect).reduce(Rect::union).unwrap_or(Rect::new(0.0, 0.0, 0.0, 0.0))
}

/// Whether `word` lies on the line `line` covers: their boxes overlap vertically by at least
/// half the shorter one's height, and the word is no more than [`WORD_GAP`] line heights away
/// across. A wider gap across (a gutter, a right-aligned column) makes it another line.
fn joins_row(line: Rect, word: Rect) -> bool {
    let height = line.height().min(word.height());
    if !(height.is_finite() && height > 0.0) {
        return false;
    }
    // Baselines must agree as well as boxes: two printed lines with tight leading overlap
    // vertically, and overlap alone interleaves their words into one garbled line.
    let centre = |r: Rect| (r.top + r.bottom) / 2.0;
    if (centre(line) - centre(word)).abs() > 0.5 * height {
        return false;
    }
    let overlap = line.bottom.min(word.bottom) - line.top.max(word.top);
    if overlap < 0.5 * height {
        return false;
    }
    let gap = (word.left - line.right).max(line.left - word.right);
    gap <= WORD_GAP * height
}

/// Whether `next` continues the block `prev` covers: it starts below it by no more than
/// [`BLOCK_GAP`] line heights, and the two share at least [`COLUMN_OVERLAP`] of the narrower
/// one's width, so they are one column and not two blocks side by side.
fn same_block(prev: Rect, next: Rect) -> bool {
    // The gap is measured in the height of the line joining, not the block it joins: a block
    // several lines tall would otherwise reach across the space that separates it from the next.
    let gap = next.top - prev.bottom;
    let height = next.height();
    if !(gap.is_finite() && height > 0.0) || gap < 0.0 || gap > BLOCK_GAP * height {
        return false;
    }
    let overlap = prev.right.min(next.right) - prev.left.max(next.left);
    overlap >= COLUMN_OVERLAP * prev.width().min(next.width())
}

/// What a region is: its text first (an exhibit label or a signature line says what it is
/// wherever it sits), then its position (a running header, a list item, a caption block).
fn classify(lines: &[Line], rect: Rect, page_height: Option<f32>, previous: Option<&[Region]>) -> RegionKind {
    let first = lines.first().map(Line::text).unwrap_or_default();
    let start = first.trim_start();
    if is_exhibit_label(start) {
        return RegionKind::Exhibit;
    }
    if lines.iter().any(|l| is_signature_line(&l.text())) {
        return RegionKind::Signature;
    }
    if let Some(kind) = running_kind(rect, page_height, lines, previous) {
        return kind;
    }
    if is_list_item(start) {
        return RegionKind::ListItem;
    }
    if is_caption(lines, rect, page_height) {
        return RegionKind::Caption;
    }
    RegionKind::Text
}

/// The header or footer kind of a region in a page's top or bottom margin, when the page before
/// repeats it. With no page before (`previous` is `None`) the position stands for the repetition
/// it cannot check.
fn running_kind(rect: Rect, page_height: Option<f32>, lines: &[Line], previous: Option<&[Region]>) -> Option<RegionKind> {
    let height = page_height?;
    let kind = if rect.bottom <= MARGIN * height {
        RegionKind::Header
    } else if rect.top >= (1.0 - MARGIN) * height {
        RegionKind::Footer
    } else {
        return None;
    };
    let repeats = match previous {
        None => true,
        Some(regions) => {
            let text = running_text(lines);
            regions.iter().any(|r| r.kind == kind && running_text(&r.lines) == text)
        }
    };
    repeats.then_some(kind)
}

/// A region's first line, lowercased, with every run of digits collapsed to `#`: a running
/// header or footer repeats from page to page except for the page number in it.
fn running_text(lines: &[Line]) -> String {
    let text = lines.first().map(Line::text).unwrap_or_default().to_lowercase();
    let mut out = String::with_capacity(text.len());
    let mut spaced = false;
    for c in text.chars() {
        if c.is_whitespace() {
            spaced = true;
            continue;
        }
        if spaced && !out.is_empty() {
            out.push(' ');
        }
        spaced = false;
        if c.is_ascii_digit() {
            if !out.ends_with('#') {
                out.push('#');
            }
        } else {
            out.push(c);
        }
    }
    out
}

/// Whether the line is an exhibit label — `Exhibit A`, `EXHIBIT 12` — and not a sentence that
/// happens to start with the word.
pub(crate) fn is_exhibit_label(text: &str) -> bool {
    let Some(head) = text.get(..7) else { return false };
    if !head.eq_ignore_ascii_case("exhibit") {
        return false;
    }
    let rest = text.get(7..).unwrap_or_default().trim_start();
    let mut chars = rest.chars();
    match chars.next() {
        Some(c) if c.is_ascii_digit() => true,
        Some(c) if c.is_ascii_uppercase() => chars.next().is_none_or(|n| !n.is_alphanumeric()),
        _ => false,
    }
}

/// Whether the line holds a signature marker: what a signature block, an affidavit or a
/// verification opens or closes with.
pub(crate) fn is_signature_line(text: &str) -> bool {
    text.contains("Dated:") || text.contains("Sworn to") || text.contains("By:")
}

/// Whether the line starts with a numbering marker — `1.`, `12)`, `(a)`, `A.` — and so opens a
/// list item.
pub(crate) fn is_list_item(text: &str) -> bool {
    let rest = text.strip_prefix('(').unwrap_or(text);
    let mut chars = rest.chars();
    let marker = match chars.next() {
        Some(c) if c.is_ascii_digit() => rest.chars().take_while(|c| c.is_ascii_digit()).count(),
        Some(c) if c.is_ascii_uppercase() => 1,
        _ => return false,
    };
    let Some(after) = rest.get(marker..).and_then(|t| t.strip_prefix('.').or_else(|| t.strip_prefix(')'))) else {
        return false;
    };
    after.is_empty() || after.starts_with(char::is_whitespace)
}

/// Whether a region is the party/caption block: lines of the top [`CAPTION_TOP`] of the page
/// that name a court, a party or the `against` between them.
fn is_caption(lines: &[Line], rect: Rect, page_height: Option<f32>) -> bool {
    let Some(height) = page_height else { return false };
    lines.len() >= 2 && rect.top <= CAPTION_TOP * height && lines.iter().any(|l| is_caption_line(&l.text()))
}

/// Whether the line is one a caption block is made of.
fn is_caption_line(text: &str) -> bool {
    let plain: String = text.trim().trim_end_matches([',', '.']).trim().to_lowercase();
    if plain.is_empty() {
        return false;
    }
    if COURT_NAMES.iter().any(|court| plain.contains(court)) {
        return true;
    }
    if PARTY_ROLES.iter().any(|role| plain.ends_with(role)) {
        return true;
    }
    matches!(plain.as_str(), "- against -" | "against" | "v." | "v" | "vs.")
}

/// Fraction of the page height at the top and at the bottom that a running header or footer
/// occupies: the margins a page sets them in.
const MARGIN: f32 = 0.06;

/// A line starting at most this many line heights below the block above it continues that block;
/// a bigger gap starts a new one.
const BLOCK_GAP: f32 = 0.75;

/// Two words sit on one line when the gap between them is at most this many line heights across;
/// a wider gap (a gutter, a right-aligned column) starts another line.
const WORD_GAP: f32 = 4.0;

/// Lines of one block share at least this much of the narrower one's width; blocks side by side
/// (columns, a name against its role) do not, and stay apart.
const COLUMN_OVERLAP: f32 = 0.25;

/// The top fraction of the page a party/caption block sits in.
const CAPTION_TOP: f32 = 0.40;

/// The courts a caption block names, lowercased.
const COURT_NAMES: [&str; 5] = ["supreme court", "superior court", "civil court", "district court", "surrogate's court"];

/// The parties a caption block names, lowercased.
const PARTY_ROLES: [&str; 10] =
    ["plaintiff", "plaintiffs", "defendant", "defendants", "petitioner", "petitioners", "respondent", "respondents", "appellant", "appellee"];

#[cfg(test)]
mod tests {
    use super::*;

    /// A page built for the tests: the words a recogniser would report, and the page's size in
    /// the same pixels.
    struct FixturePage {
        words: Vec<Word>,
        size: (f32, f32),
    }

    /// `text` as one line of words starting at (`left`, `top`), `height` pixels tall, each word
    /// about as wide as its letters (roughly 0.55 em) with a third of an em between words.
    fn words_at(text: &str, left: f32, top: f32, height: f32) -> Vec<Word> {
        let mut out = Vec::new();
        let mut x = left;
        for word in text.split_whitespace() {
            let width = word.chars().count() as f32 * height * 0.55;
            out.push(Word::new(word, [x, top, x + width, top + height], "ocrs"));
            x += width + height * 0.3;
        }
        out
    }

    /// A page shaped like a first page of a New York filing: a court stamp at the top, the party
    /// block, a numbered paragraph and a signature block, each where a filing puts it. No client
    /// document: every word is written here.
    fn synthetic_legal_page() -> FixturePage {
        let mut words = Vec::new();
        words.extend(words_at("RECEIVED NYSCEF: 07/22/2026", 100.0, 16.0, 12.0));
        words.extend(words_at("SUPREME COURT OF THE STATE OF NEW YORK", 100.0, 72.0, 12.0));
        words.extend(words_at("COUNTY OF NEW YORK", 100.0, 90.0, 12.0));
        words.extend(words_at("JOHN DOE,", 100.0, 108.0, 12.0));
        words.extend(words_at("Plaintiff,", 144.0, 126.0, 12.0));
        words.extend(words_at("- against -", 100.0, 144.0, 12.0));
        words.extend(words_at("JANE ROE,", 100.0, 162.0, 12.0));
        words.extend(words_at("Defendant.", 144.0, 180.0, 12.0));
        words.extend(words_at("1. First point", 100.0, 240.0, 12.0));
        words.extend(words_at("Dated: July 22, 2026", 100.0, 300.0, 12.0));
        FixturePage { words, size: (612.0, 792.0) }
    }

    fn kinds(regions: &[Region]) -> Vec<RegionKind> {
        regions.iter().map(|r| r.kind).collect()
    }

    #[test]
    fn page_is_split_into_typed_regions() {
        let page = synthetic_legal_page();
        let regions = analyze_page(&page.words, page.size, None);
        assert!(regions.iter().any(|r| r.kind == RegionKind::Header));
        assert!(regions.iter().any(|r| r.kind == RegionKind::Caption));
        assert!(regions.iter().any(|r| r.kind == RegionKind::ListItem));
        assert!(regions.iter().any(|r| r.kind == RegionKind::Signature));
    }

    /// A heading at the top of a first page and a running header sit in the same place; only the
    /// pages after tell them apart.
    #[test]
    fn a_top_line_is_a_header_only_when_the_previous_page_repeats_it() {
        let page = synthetic_legal_page();
        let first = analyze_page(&page.words, page.size, None);
        let again = analyze_page(&page.words, page.size, Some(&first));
        assert!(again.iter().any(|r| r.kind == RegionKind::Header), "the stamp repeats: {:?}", kinds(&again));

        let mut other = FixturePage { words: words_at("FILED: NEW YORK COUNTY CLERK", 100.0, 16.0, 12.0), size: page.size };
        other.words.extend(words_at("Some body text.", 100.0, 300.0, 12.0));
        let different = analyze_page(&other.words, other.size, None);
        let regions = analyze_page(&page.words, page.size, Some(&different));
        assert!(!regions.iter().any(|r| r.kind == RegionKind::Header), "nothing above matches: {:?}", kinds(&regions));
    }

    /// Every word the recogniser reported lands in exactly one region, and the regions are
    /// numbered top to bottom.
    #[test]
    fn regions_cover_every_word_in_reading_order() {
        let page = synthetic_legal_page();
        let regions = analyze_page(&page.words, page.size, None);
        let placed: usize = regions.iter().flat_map(|r| &r.lines).map(|l| l.words.len()).sum();
        assert_eq!(placed, page.words.iter().filter(|w| w.text.split_whitespace().count() > 0).count());
        let tops: Vec<f32> = regions.iter().map(|r| r.rect.top).collect();
        assert!(tops.windows(2).all(|w| w[0] <= w[1]), "reading order runs down the page: {tops:?}");
        assert_eq!(regions.iter().map(|r| r.order).collect::<Vec<_>>(), (0..regions.len()).collect::<Vec<_>>());
    }

    /// Two blocks side by side are two regions, not one wide line.
    #[test]
    fn side_by_side_blocks_are_separate_regions() {
        let mut page = FixturePage { words: Vec::new(), size: (612.0, 792.0) };
        page.words.extend(words_at("LEFT COLUMN FIRST", 60.0, 200.0, 12.0));
        page.words.extend(words_at("LEFT COLUMN SECOND", 60.0, 216.0, 12.0));
        page.words.extend(words_at("RIGHT COLUMN FIRST", 400.0, 200.0, 12.0));
        page.words.extend(words_at("RIGHT COLUMN SECOND", 400.0, 216.0, 12.0));
        let regions = analyze_page(&page.words, page.size, None);
        assert_eq!(regions.len(), 2, "{:?}", kinds(&regions));
        let lefts: Vec<f32> = regions.iter().map(|r| r.rect.left).collect();
        assert_eq!(lefts, vec![60.0, 400.0]);
    }

    /// An exhibit label and a footer are recognised where the page puts them.
    #[test]
    fn an_exhibit_label_and_a_footer_are_typed() {
        let mut page = FixturePage { words: Vec::new(), size: (612.0, 792.0) };
        page.words.extend(words_at("Exhibit A", 100.0, 300.0, 12.0));
        page.words.extend(words_at("Page 4 of 12", 100.0, 770.0, 10.0));
        let first = analyze_page(&page.words, page.size, None);
        assert_eq!(kinds(&first), vec![RegionKind::Exhibit, RegionKind::Footer]);
    }

    /// A plain heading at the top of the page is body text: a caption block names a court or a
    /// party.
    #[test]
    fn prose_at_the_top_is_not_a_caption() {
        let mut page = FixturePage { words: Vec::new(), size: (612.0, 792.0) };
        page.words.extend(words_at("MEMORANDUM OF LAW", 100.0, 80.0, 12.0));
        page.words.extend(words_at("IN SUPPORT OF THE MOTION", 100.0, 96.0, 12.0));
        assert_eq!(kinds(&analyze_page(&page.words, page.size, None)), vec![RegionKind::Text]);
    }

    /// A recogniser can report boxes that are not boxes; they are left out rather than allowed
    /// to poison the page's regions.
    #[test]
    fn unusable_word_boxes_are_ignored() {
        let mut page = FixturePage { words: Vec::new(), size: (612.0, 792.0) };
        page.words.extend(words_at("REAL TEXT ON THE PAGE", 100.0, 300.0, 12.0));
        page.words.push(Word::new("nan", [f32::NAN, 10.0, 40.0, 20.0], "ocrs"));
        page.words.push(Word::new("backwards", [200.0, 20.0, 100.0, 10.0], "ocrs"));
        page.words.push(Word::new("empty", [100.0, 400.0, 100.0, 400.0], "ocrs"));
        let regions = analyze_page(&page.words, page.size, None);
        let placed: Vec<String> = regions.iter().flat_map(|r| &r.lines).map(|l| l.text()).collect();
        assert_eq!(placed, vec!["REAL TEXT ON THE PAGE".to_string()]);
    }

    #[test]
    fn an_empty_page_has_no_regions() {
        assert!(analyze_page(&[], (612.0, 792.0), None).is_empty());
        assert!(analyze_page(&[], (0.0, 0.0), None).is_empty());
    }
}
