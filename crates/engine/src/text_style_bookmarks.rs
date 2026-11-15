//! Bookmarks from text styles (#1693): a document's headings are often set apart only by how
//! they look (chapter titles in one font at one size, sections in another), not tagged as
//! headings, so New Bookmarks from Structure finds nothing. [`styles`] lists the styles the
//! text is set in; [`headings`] finds the lines in the styles chosen for each bookmark level.
//!
//! A style is a font (without its subset tag: `ABCDEF+Garamond` and `GHIJKL+Garamond` are
//! subsets of one font), the size the text is shown at, and whether it is bold and italic.
//! Lines are taken in reading order down each page: top to bottom, then left to right.

use pdfcraft_cos::Document;
use pdfcraft_organize::OutlineEntry;

/// How a line of text looks.
#[derive(Clone, Debug, PartialEq)]
pub struct TextStyle {
    /// The font's name, without a subset tag.
    pub font: String,
    /// The size the text is shown at, in points (rounded to a tenth).
    pub size: f64,
    pub bold: bool,
    pub italic: bool,
}

impl TextStyle {
    /// Whether `other` is this style. Sizes within a quarter point match, as the same heading
    /// size is often written with rounding differences (12 and 11.96).
    pub(crate) fn matches(&self, other: &TextStyle) -> bool {
        self.font == other.font && self.bold == other.bold && self.italic == other.italic && (self.size - other.size).abs() <= SIZE_TOLERANCE
    }
}

/// How far apart two sizes may be and still be one style.
const SIZE_TOLERANCE: f64 = 0.25;

/// A style and how much text is set in it.
#[derive(Clone, Debug, PartialEq)]
pub struct TextStyleUse {
    pub style: TextStyle,
    /// Lines in this style.
    pub lines: usize,
    /// Pages they are on.
    pub pages: usize,
    /// The first line in it, cut to [`SAMPLE_CHARS`] characters.
    pub sample: String,
}

/// The most pages read: a longer document is listed and searched by its first pages.
pub const MAX_PAGES: usize = 5000;
/// The most bookmarks made at once.
pub const MAX_BOOKMARKS: usize = 10_000;
/// The longest bookmark title kept, in characters (a heading run on into a paragraph).
const MAX_TITLE_CHARS: usize = 300;
/// The longest sample shown for a style.
const SAMPLE_CHARS: usize = 60;
/// The most styles listed, the largest first.
const MAX_STYLES: usize = 200;

/// One line of text in reading order.
struct Line {
    page: usize,
    text: String,
    style: TextStyle,
    /// Top, bottom, left and right of its box in user space.
    top: f64,
    bottom: f64,
    left: f64,
    right: f64,
}

/// `name` without a subset tag: six capital letters and a `+` (ISO 32000-2 §9.9.2).
fn without_subset_tag(name: &str) -> &str {
    match name.split_once('+') {
        Some((tag, rest)) if tag.len() == 6 && tag.bytes().all(|b| b.is_ascii_uppercase()) && !rest.is_empty() => rest,
        _ => name,
    }
}

/// The text lines of `doc`'s first [`MAX_PAGES`] pages, each page's in reading order. Text that
/// is only whitespace, or whose size can't be read, is left out.
fn lines(doc: &Document) -> Vec<Line> {
    let pages = pdfcraft_organize::page_count(doc).unwrap_or(0).min(MAX_PAGES);
    let mut out = Vec::new();
    for page in 0..pages {
        let Ok(found) = pdfcraft_edit::text_lines(doc, page) else { continue };
        let mut on_page: Vec<Line> = found
            .into_iter()
            .filter_map(|l| {
                let text = l.text.split_whitespace().collect::<Vec<_>>().join(" ");
                // `size` is the size the line is shown at in user space (the `Tf` size times the
                // text rendering matrix), so `1 Tf` with a 14× text matrix is 14.
                let size = l.size;
                let [x0, y0, x1, y1] = l.rect;
                let ok = !text.is_empty() && size.is_finite() && size > 0.0 && [x0, y0, x1, y1].iter().all(|v| v.is_finite());
                ok.then(|| Line {
                    page,
                    text,
                    style: TextStyle {
                        font: without_subset_tag(&l.base_font).to_string(),
                        size: (size * 10.0).round() / 10.0,
                        bold: l.bold,
                        italic: l.italic,
                    },
                    top: y0.max(y1),
                    bottom: y0.min(y1),
                    left: x0.min(x1),
                    right: x0.max(x1),
                })
            })
            .collect();
        // Down the page; along each row from the left. Boxes of one row differ by a fraction
        // of a point, so rows are compared at a tenth of a point.
        on_page.sort_by(|a, b| (b.top * 10.0).round().total_cmp(&(a.top * 10.0).round()).then(a.left.total_cmp(&b.left)));
        out.extend(on_page);
    }
    out
}

/// The styles `doc`'s text is set in, the largest size first (then the most used): headings
/// are usually larger than the text around them.
pub fn styles(doc: &Document) -> Vec<TextStyleUse> {
    let mut uses: Vec<(TextStyleUse, Option<usize>)> = Vec::new();
    // The styles by font, weight, slant and size in quarter points, so a line is matched against
    // the few styles near its size rather than every style found (a page can hold thousands).
    // A size matches within a quarter point: the bucket's neighbours are looked at too.
    let mut index: std::collections::HashMap<(String, bool, bool, i64), Vec<usize>> = std::collections::HashMap::new();
    let bucket = |size: f64| (size / SIZE_TOLERANCE).floor() as i64;
    for line in lines(doc) {
        let key = |b: i64| (line.style.font.clone(), line.style.bold, line.style.italic, b);
        let b = bucket(line.style.size);
        let found = [b.saturating_sub(1), b, b.saturating_add(1)]
            .into_iter()
            .filter_map(|b| index.get(&key(b)))
            .flatten()
            .copied()
            .find(|&i| uses.get(i).is_some_and(|(u, _)| u.style.matches(&line.style)));
        match found.and_then(|i| uses.get_mut(i)) {
            Some((u, last_page)) => {
                u.lines = u.lines.saturating_add(1);
                if *last_page != Some(line.page) {
                    u.pages = u.pages.saturating_add(1);
                    *last_page = Some(line.page);
                }
            }
            None => {
                let sample = line.text.chars().take(SAMPLE_CHARS).collect();
                index.entry(key(b)).or_default().push(uses.len());
                uses.push((TextStyleUse { style: line.style, lines: 1, pages: 1, sample }, Some(line.page)));
            }
        }
    }
    let mut uses: Vec<TextStyleUse> = uses.into_iter().map(|(u, _)| u).collect();
    uses.sort_by(|a, b| b.style.size.total_cmp(&a.style.size).then(b.lines.cmp(&a.lines)));
    uses.truncate(MAX_STYLES);
    uses
}

/// The bookmarks for the lines in `levels`' styles: a line in `levels[0]`'s style is a level-1
/// bookmark, `levels[1]`'s a level-2 one under the level-1 bookmark before it, and so on. A
/// heading set on several lines (one after another in the same style, close below each other)
/// is one bookmark. Each goes to the top of its heading.
pub fn headings(doc: &Document, levels: &[TextStyle]) -> Vec<OutlineEntry> {
    let mut entries: Vec<OutlineEntry> = Vec::new();
    // The heading the previous line belonged to: its level, and the top, bottom, left and right
    // of its last line.
    let mut open: Option<(usize, f64, f64, f64, f64)> = None;
    for line in lines(doc) {
        let Some(level) = levels.iter().position(|s| s.matches(&line.style)) else {
            open = None;
            continue;
        };
        // A heading goes on to the next row down, close below its last line and under it. A line
        // on the same row (two headings side by side), or in another column, is a heading of
        // its own.
        let continues = open.is_some_and(|(l, top, bottom, left, right)| {
            let size = line.style.size;
            let under = line.left < right && left < line.right;
            l == level && line.top < top - size * 0.5 && bottom - line.top < size * 0.6 && under
        }) && entries.last().is_some_and(|e| e.page == line.page);
        if continues {
            if let Some(e) = entries.last_mut()
                && e.title.chars().count() < MAX_TITLE_CHARS
            {
                e.title.push(' ');
                e.title.push_str(&line.text);
                e.title = e.title.chars().take(MAX_TITLE_CHARS).collect();
            }
        } else {
            if entries.len() >= MAX_BOOKMARKS {
                break;
            }
            // The level counts from 1; the position in `levels` is at most its length, which a
            // caller keeps small (a few levels), so it fits a u8.
            let level = u8::try_from(level.saturating_add(1)).unwrap_or(u8::MAX);
            entries.push(OutlineEntry {
                level,
                title: line.text.chars().take(MAX_TITLE_CHARS).collect(),
                page: line.page,
                element: None,
                top: Some(line.top),
            });
        }
        open = Some((level, line.top, line.bottom, line.left, line.right));
    }
    entries
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn subset_tags_are_left_out_of_the_font_name() {
        assert_eq!(without_subset_tag("ABCDEF+Garamond-Bold"), "Garamond-Bold");
        assert_eq!(without_subset_tag("Garamond-Bold"), "Garamond-Bold");
        assert_eq!(without_subset_tag("ABCDE+Garamond"), "ABCDE+Garamond", "a tag is six letters");
        assert_eq!(without_subset_tag("abcdef+Garamond"), "abcdef+Garamond", "capital letters");
        assert_eq!(without_subset_tag("ABCDEF+"), "ABCDEF+", "nothing after the tag");
        assert_eq!(without_subset_tag("ÄBCDEF+x"), "ÄBCDEF+x");
    }

    #[test]
    fn styles_match_within_a_quarter_point() {
        let s = |size| TextStyle { font: "Garamond".into(), size, bold: true, italic: false };
        assert!(s(12.0).matches(&s(11.8)));
        assert!(!s(12.0).matches(&s(11.7)));
        assert!(!s(12.0).matches(&TextStyle { bold: false, ..s(12.0) }));
        assert!(!s(12.0).matches(&TextStyle { font: "Times".into(), ..s(12.0) }));
    }
}
