//! Suggest outline (#6020): a bookmark tree proposed for the whole document, for review before
//! it is applied. Tagged headings (H, H1–H6) are used when the document has them. Otherwise the
//! headings are inferred from how the text looks, by the rules Auto-tag uses
//! ([`pdfcraft_a11y::heading_levels`]): lines in one style that follow each other down the page
//! are one block (a heading that wraps is one bookmark), the body style is the size carrying the
//! most characters, a short block larger than the body (or bold where the body is not) is a
//! heading, and levels rank the heading styles over the document without skipping a level.
//! Captions (`Figure 3`, `Table 2`) are not headings. Each inferred bookmark goes to the top of
//! its heading as displayed (ISO 32000-2 §12.3.2.2 `/XYZ`); tagged ones go to their page.

use pdfcraft_a11y::TextBlock;
use pdfcraft_cos::Document;
use pdfcraft_organize::{OutlineEntry, OutlineNode};

use crate::text_style_bookmarks::{Line, MAX_BOOKMARKS, MAX_PAGES, MAX_TITLE_CHARS, lines};

/// Blocks looked back over for one a line continues: a heading's next line can come after lines
/// of other columns on the same row.
const LOOKBACK: usize = 8;
/// The deepest a suggested tree nests: bookmark_import takes 33 levels, counting a parent.
const MAX_DEPTH: usize = 32;

/// Where the suggested headings come from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OutlineSource {
    /// The structure tree's headings.
    Structure,
    /// The size and weight of the text.
    Layout,
}

/// A suggested outline.
#[derive(Clone, Debug, PartialEq)]
pub struct OutlineSuggestion {
    pub source: OutlineSource,
    /// The headings in document order, nested by level (see [`OutlineSuggestion::tree`]).
    pub entries: Vec<OutlineEntry>,
    /// Pages in the document.
    pub pages: usize,
    /// Pages whose text was read: at most [`MAX_PAGES`] for a suggestion from layout.
    pub pages_read: usize,
}

impl OutlineSuggestion {
    /// The entries as a bookmark tree, every bookmark expanded: a level-2 entry goes under the
    /// level-1 entry before it, and so on. Levels deeper than [`MAX_DEPTH`] stay at that depth.
    pub fn tree(&self) -> Vec<OutlineNode> {
        nest(&self.entries, &mut 0, 0, 0)
    }
}

/// The entries from `*at` on that go under a parent of level `above` at `depth` (recursion
/// bounded by [`MAX_DEPTH`]); `*at` ends at the first that doesn't.
fn nest(entries: &[OutlineEntry], at: &mut usize, above: u8, depth: usize) -> Vec<OutlineNode> {
    let mut out = Vec::new();
    while let Some(e) = entries.get(*at) {
        if depth > 0 && e.level <= above {
            break;
        }
        *at = at.saturating_add(1);
        let mut n = node(e);
        if depth.saturating_add(1) < MAX_DEPTH {
            n.children = nest(entries, at, e.level, depth.saturating_add(1));
        }
        out.push(n);
    }
    out
}

fn node(e: &OutlineEntry) -> OutlineNode {
    let view = (e.left.is_some() || e.top.is_some()).then(|| (b"XYZ".to_vec(), vec![e.left, e.top, None]));
    OutlineNode { title: e.title.clone(), page: Some(e.page), view, color: None, italic: false, bold: false, open: true, children: Vec::new() }
}

/// The tagged headings as outline entries, each linked to its element (`/SE`).
pub(crate) fn structure_entries(doc: &Document) -> Vec<OutlineEntry> {
    pdfcraft_a11y::headings(doc)
        .into_iter()
        .map(|h| OutlineEntry { level: h.level, title: h.title, page: h.page, element: h.obj, left: None, top: None })
        .collect()
}

/// The outline suggested for `doc`: its tagged headings, or else headings inferred from the
/// first [`MAX_PAGES`] pages' text. Empty when neither finds a heading.
pub fn suggest(doc: &Document) -> OutlineSuggestion {
    let pages = pdfcraft_organize::page_count(doc).unwrap_or(0);
    let tagged = structure_entries(doc);
    if !tagged.is_empty() {
        return OutlineSuggestion { source: OutlineSource::Structure, entries: tagged, pages, pages_read: pages };
    }
    OutlineSuggestion { source: OutlineSource::Layout, entries: inferred(lines(doc)), pages, pages_read: pages.min(MAX_PAGES) }
}

/// Lines in one style, one after another down a page.
struct Block {
    first: Line,
    last: Line,
    title: String,
    lines: usize,
    chars: usize,
    letters: bool,
}

impl Block {
    fn new(line: Line) -> Block {
        let mut b = Block { first: line.clone(), last: line, title: String::new(), lines: 0, chars: 0, letters: false };
        b.count(&b.first.text.clone());
        b
    }

    fn add(&mut self, line: Line) {
        if self.title.chars().count() < MAX_TITLE_CHARS {
            self.title.push(' ');
        }
        self.count(&line.text);
        self.last = line;
    }

    fn count(&mut self, text: &str) {
        self.lines = self.lines.saturating_add(1);
        self.chars = self.chars.saturating_add(text.chars().filter(|c| !c.is_whitespace()).count());
        self.letters |= text.chars().any(char::is_alphabetic);
        let room = MAX_TITLE_CHARS.saturating_sub(self.title.chars().count());
        self.title.extend(text.chars().take(room));
    }

    fn evidence(&self) -> TextBlock {
        let s = &self.first.style;
        TextBlock { size: s.size, bold: s.bold, lines: self.lines, chars: self.chars, letters: self.letters }
    }
}

/// `lines`, in reading order, grouped into blocks: a line joins the most recent of the last
/// [`LOOKBACK`] blocks in its style that it continues ([`Line::continues`]).
fn blocks(lines: Vec<Line>) -> Vec<Block> {
    let mut out: Vec<Block> = Vec::new();
    for line in lines {
        let start = out.len().saturating_sub(LOOKBACK);
        let found = out.get_mut(start..).and_then(|recent| {
            recent.iter_mut().rev().find(|b| b.last.page == line.page && b.first.style.matches(&line.style) && line.continues(&b.last))
        });
        match found {
            Some(b) => b.add(line),
            None => out.push(Block::new(line)),
        }
    }
    out
}

/// A caption's lead: `Figure`, `Fig.`, `Table`, `Chart`, `Exhibit` or `Plate`, then a number.
fn is_caption(text: &str) -> bool {
    let t = text.trim_start();
    ["Figure", "Fig.", "Table", "Chart", "Exhibit", "Plate"]
        .iter()
        .any(|lead| t.strip_prefix(lead).is_some_and(|rest| rest.trim_start().chars().next().is_some_and(|c| c.is_ascii_digit())))
}

/// The headings among `lines`, as outline entries.
fn inferred(lines: Vec<Line>) -> Vec<OutlineEntry> {
    let blocks: Vec<Block> = blocks(lines).into_iter().filter(|b| !is_caption(&b.title)).collect();
    let evidence: Vec<TextBlock> = blocks.iter().map(Block::evidence).collect();
    blocks
        .iter()
        .zip(pdfcraft_a11y::heading_levels(&evidence))
        .filter_map(|(b, level)| {
            let [left, top] = b.first.anchor;
            Some(OutlineEntry { level: level?, title: b.title.clone(), page: b.first.page, element: None, left, top })
        })
        .take(MAX_BOOKMARKS)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::text_style_bookmarks::TextStyle;

    /// A line at `top` (its box 0.8 of its size tall) from x 72 to 400.
    fn line(page: usize, top: f64, size: f64, bold: bool, text: &str) -> Line {
        let style = TextStyle { font: if bold { "Helvetica-Bold" } else { "Helvetica" }.into(), size, bold, italic: false };
        Line { page, text: text.into(), style, top, bottom: top - size * 0.8, left: 72.0, right: 400.0, anchor: [None, Some(top)] }
    }

    fn body(page: usize, top: f64) -> Line {
        line(page, top, 11.0, false, "Body text that runs on for a good many characters across the page.")
    }

    fn titles(entries: &[OutlineEntry]) -> Vec<(u8, &str, usize)> {
        entries.iter().map(|e| (e.level, e.title.as_str(), e.page)).collect()
    }

    #[test]
    fn headings_take_levels_by_size_and_weight_and_wrapped_lines_join() {
        let lines = vec![
            line(0, 760.0, 20.0, true, "Getting started"),
            line(0, 738.0, 20.0, true, "with the toolkit"),
            body(0, 700.0),
            body(0, 686.0),
            body(0, 672.0),
            body(0, 658.0),
            line(0, 630.0, 11.0, true, "Installing"),
            body(0, 610.0),
            body(0, 596.0),
            line(1, 760.0, 20.0, true, "Reference"),
            body(1, 730.0),
            body(1, 716.0),
        ];
        let found = inferred(lines);
        assert_eq!(titles(&found), [(1, "Getting started with the toolkit", 0), (2, "Installing", 0), (1, "Reference", 1)]);
        assert_eq!((found[0].left, found[0].top), (None, Some(760.0)), "a bookmark goes to the top of its first line");
    }

    #[test]
    fn a_wrapped_heading_joins_across_a_line_of_another_column() {
        let mut right = body(0, 760.0);
        (right.left, right.right) = (420.0, 560.0);
        let lines = vec![line(0, 760.0, 20.0, true, "Two column"), right, line(0, 738.0, 20.0, true, "heading"), body(0, 700.0), body(0, 686.0)];
        assert_eq!(titles(&inferred(lines)), [(1, "Two column heading", 0)]);
    }

    #[test]
    fn captions_and_numbers_are_not_headings() {
        let lines = vec![line(0, 760.0, 16.0, true, "Figure 3: The toolkit"), line(0, 730.0, 16.0, true, "42"), body(0, 700.0), body(0, 686.0)];
        assert!(inferred(lines).is_empty());
        assert!(is_caption("Table 12 Results") && is_caption("Fig. 2") && !is_caption("Tables are useful"));
    }

    #[test]
    fn the_tree_nests_by_level_and_caps_the_depth() {
        let entry = |level: u8, title: &str| OutlineEntry { level, title: title.into(), page: 0, element: None, left: None, top: Some(700.0) };
        let mut entries = vec![entry(1, "A"), entry(2, "A.1"), entry(3, "A.1.a"), entry(2, "A.2"), entry(1, "B")];
        let s = OutlineSuggestion { source: OutlineSource::Layout, entries: entries.clone(), pages: 1, pages_read: 1 };
        let tree = s.tree();
        assert_eq!(tree.iter().map(|n| n.title.as_str()).collect::<Vec<_>>(), ["A", "B"]);
        assert_eq!(tree[0].children.iter().map(|n| n.title.as_str()).collect::<Vec<_>>(), ["A.1", "A.2"]);
        assert_eq!(tree[0].children[0].children[0].title, "A.1.a");
        assert_eq!(tree[0].view, Some((b"XYZ".to_vec(), vec![None, Some(700.0), None])));
        // A tagged H nested in 40 sections stops at the deepest level bookmark_import takes.
        entries = (1..=40).map(|l| entry(l, "deep")).collect();
        let deep = OutlineSuggestion { entries, ..s }.tree();
        let mut depth = 0;
        let mut at = &deep;
        while let Some(n) = at.first() {
            depth += 1;
            at = &n.children;
        }
        assert_eq!(depth, MAX_DEPTH);
    }
}
