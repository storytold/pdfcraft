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

use crate::text_style_bookmarks::{Line, MAX_BOOKMARKS, MAX_TITLE_CHARS, read_lines};

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
    /// Pages whose text was read: at most [`MAX_PAGES`](crate::text_style_bookmarks::MAX_PAGES)
    /// for a suggestion from layout, fewer when a page's text can't be read.
    pub pages_read: usize,
    /// Headings may have been left out: pages weren't read, layout found more than
    /// [`MAX_BOOKMARKS`], or the structure tree ran past a limit of
    /// [`pdfcraft_a11y::headings_with_truncation`].
    pub truncated: bool,
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

/// The tagged headings as outline entries, each linked to its element (`/SE`), and whether
/// headings may be missing ([`pdfcraft_a11y::headings_with_truncation`]).
pub(crate) fn structure_entries(doc: &Document) -> (Vec<OutlineEntry>, bool) {
    let (found, cut) = pdfcraft_a11y::headings_with_truncation(doc);
    let entries =
        found.into_iter().map(|h| OutlineEntry { level: h.level, title: h.title, page: h.page, element: h.obj, left: None, top: None }).collect();
    (entries, cut)
}

/// The outline suggested for `doc`: its tagged headings, or else headings inferred from the
/// first [`MAX_PAGES`](crate::text_style_bookmarks::MAX_PAGES) pages' text. Empty when neither
/// finds a heading.
pub fn suggest(doc: &Document) -> OutlineSuggestion {
    let pages = pdfcraft_organize::page_count(doc).unwrap_or(0);
    let (tagged, cut) = structure_entries(doc);
    if !tagged.is_empty() {
        return OutlineSuggestion { source: OutlineSource::Structure, entries: tagged, pages, pages_read: pages, truncated: cut };
    }
    let (lines, pages_read) = read_lines(doc);
    let (entries, capped) = inferred(lines);
    OutlineSuggestion { source: OutlineSource::Layout, entries, pages, pages_read, truncated: capped || pages_read < pages }
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

/// A caption's lead in any case: `Figure`, `Fig.`, `Table`, `Chart`, `Exhibit` or `Plate`, then a
/// number.
fn is_caption(text: &str) -> bool {
    let t = text.trim_start();
    ["Figure", "Fig.", "Table", "Chart", "Exhibit", "Plate"].iter().any(|lead| {
        let rest = t.get(..lead.len()).filter(|head| head.eq_ignore_ascii_case(lead)).and_then(|_| t.get(lead.len()..));
        rest.is_some_and(|rest| rest.trim_start().chars().next().is_some_and(|c| c.is_ascii_digit()))
    })
}

/// The headings among `lines`, as outline entries: the first [`MAX_BOOKMARKS`], and whether there
/// were more.
fn inferred(lines: Vec<Line>) -> (Vec<OutlineEntry>, bool) {
    let blocks: Vec<Block> = blocks(lines).into_iter().filter(|b| !is_caption(&b.title)).collect();
    let evidence: Vec<TextBlock> = blocks.iter().map(Block::evidence).collect();
    let mut entries: Vec<OutlineEntry> = blocks
        .iter()
        .zip(pdfcraft_a11y::heading_levels(&evidence))
        .filter_map(|(b, level)| {
            let [left, top] = b.first.anchor;
            Some(OutlineEntry { level: level?, title: b.title.clone(), page: b.first.page, element: None, left, top })
        })
        .take(MAX_BOOKMARKS.saturating_add(1))
        .collect();
    let capped = entries.len() > MAX_BOOKMARKS;
    entries.truncate(MAX_BOOKMARKS);
    (entries, capped)
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
        let (found, capped) = inferred(lines);
        assert!(!capped);
        assert_eq!(titles(&found), [(1, "Getting started with the toolkit", 0), (2, "Installing", 0), (1, "Reference", 1)]);
        assert_eq!((found[0].left, found[0].top), (None, Some(760.0)), "a bookmark goes to the top of its first line");
    }

    #[test]
    fn a_wrapped_heading_joins_across_a_line_of_another_column() {
        let mut right = body(0, 760.0);
        (right.left, right.right) = (420.0, 560.0);
        let lines = vec![line(0, 760.0, 20.0, true, "Two column"), right, line(0, 738.0, 20.0, true, "heading"), body(0, 700.0), body(0, 686.0)];
        assert_eq!(titles(&inferred(lines).0), [(1, "Two column heading", 0)]);
    }

    #[test]
    fn captions_and_numbers_are_not_headings() {
        let lines = vec![line(0, 760.0, 16.0, true, "Figure 3: The toolkit"), line(0, 730.0, 16.0, true, "42"), body(0, 700.0), body(0, 686.0)];
        assert!(inferred(lines).0.is_empty());
        assert!(is_caption("Table 12 Results") && is_caption("Fig. 2") && !is_caption("Tables are useful"));
    }

    #[test]
    fn captions_match_in_any_case() {
        let lines = vec![line(0, 760.0, 16.0, true, "FIGURE 3: Architecture"), body(0, 700.0), body(0, 686.0)];
        assert!(inferred(lines).0.is_empty(), "an upper-case caption was suggested as a heading");
        assert!(is_caption("table 2") && is_caption("FIG. 4") && !is_caption("TABLES are useful") && !is_caption("Figé 3"));
    }

    #[test]
    fn more_headings_than_the_cap_are_reported() {
        // Each heading on a page of its own, with a line of body text under it.
        let pages = |n: usize| (0..n).flat_map(|p| [line(p, 760.0, 20.0, true, "Heading"), body(p, 700.0)]).collect::<Vec<_>>();
        let (at_cap, capped) = inferred(pages(MAX_BOOKMARKS));
        assert_eq!((at_cap.len(), capped), (MAX_BOOKMARKS, false));
        let (over, capped) = inferred(pages(MAX_BOOKMARKS + 1));
        assert_eq!((over.len(), capped), (MAX_BOOKMARKS, true), "a heading past the cap was dropped unreported");
    }

    /// A page past the page decode allowance (two compressed streams of spaces, each just over
    /// half of it) can't be read: it doesn't count as read, and the suggestion says headings
    /// may be missing.
    #[test]
    fn a_page_whose_text_cant_be_read_is_not_counted() {
        let packed = pdfcraft_filters::encode_flate(&vec![b' '; pdfcraft_cos::MAX_PAGE_DECODED / 2 + 1]);
        let stream = |dict: &str, data: &[u8]| [format!("<< /Length {} {dict} >>\nstream\n", data.len()).as_bytes(), data, b"\nendstream"].concat();
        let objs: Vec<Vec<u8>> = vec![
            b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
            b"<< /Type /Pages /Kids [3 0 R 4 0 R] /Count 2 /MediaBox [0 0 612 792] /Resources << /Font << /F 7 0 R >> >> >>".to_vec(),
            b"<< /Type /Page /Parent 2 0 R /Contents 5 0 R >>".to_vec(),
            b"<< /Type /Page /Parent 2 0 R /Contents [6 0 R 6 0 R] >>".to_vec(),
            stream("", b"BT /F 11 Tf 72 700 Td (Body) Tj ET"),
            stream("/Filter /FlateDecode", &packed),
            b"<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>".to_vec(),
        ];
        let s = suggest(&doc_of(&objs));
        assert_eq!((s.pages, s.pages_read, s.truncated), (2, 1, true), "the unread page counted as read");
    }

    /// A PDF of these objects (object n = `objs[n - 1]`), the first the catalog.
    fn doc_of(objs: &[Vec<u8>]) -> Document {
        let mut bytes = b"%PDF-1.7\n".to_vec();
        let mut offsets = Vec::new();
        for (i, o) in objs.iter().enumerate() {
            offsets.push(bytes.len());
            bytes.extend_from_slice(&[format!("{} 0 obj\n", i + 1).as_bytes(), o, b"\nendobj\n"].concat());
        }
        let xref = bytes.len();
        bytes.extend_from_slice(format!("xref\n0 {}\n0000000000 65535 f \n", objs.len() + 1).as_bytes());
        for o in offsets {
            bytes.extend_from_slice(format!("{o:010} 00000 n \n").as_bytes());
        }
        bytes.extend_from_slice(format!("trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n", objs.len() + 1).as_bytes());
        Document::open(std::sync::Arc::new(bytes)).unwrap()
    }

    /// More tagged headings than the structure reader returns: the suggestion says some are missing.
    #[test]
    fn tagged_headings_past_the_cap_are_reported() {
        let tagged = |count: usize| {
            let kids: String = (0..count).map(|i| format!("{} 0 R ", i + 6)).collect();
            let mut objs = vec![
                b"<< /Type /Catalog /Pages 2 0 R /StructTreeRoot 4 0 R >>".to_vec(),
                b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_vec(),
                b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] >>".to_vec(),
                b"<< /Type /StructTreeRoot /K 5 0 R >>".to_vec(),
                format!("<< /S /Document /K [{kids}] >>").into_bytes(),
            ];
            objs.extend((0..count).map(|_| b"<< /S /H1 /Pg 3 0 R /ActualText (Head) >>".to_vec()));
            suggest(&doc_of(&objs))
        };
        let cap = pdfcraft_a11y::MAX_HEADINGS;
        let at_cap = tagged(cap);
        assert_eq!((at_cap.source, at_cap.entries.len(), at_cap.truncated), (OutlineSource::Structure, cap, false));
        let over = tagged(cap + 1);
        assert_eq!((over.entries.len(), over.truncated), (cap, true), "a tagged heading past the cap was dropped unreported");
    }

    #[test]
    fn the_tree_nests_by_level_and_caps_the_depth() {
        let entry = |level: u8, title: &str| OutlineEntry { level, title: title.into(), page: 0, element: None, left: None, top: Some(700.0) };
        let mut entries = vec![entry(1, "A"), entry(2, "A.1"), entry(3, "A.1.a"), entry(2, "A.2"), entry(1, "B")];
        let s = OutlineSuggestion { source: OutlineSource::Layout, entries: entries.clone(), pages: 1, pages_read: 1, truncated: false };
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
