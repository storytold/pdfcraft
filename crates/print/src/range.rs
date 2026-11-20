//! Pages to Print: all, a range ("1-3, 6, 9-", page labels allowed, "(n)" for the page at
//! position n), odd or even pages, reverse.

use std::collections::HashMap;

use crate::PrintError;

/// Most pages one range may select (more when the document itself has more): every selection
/// becomes a placement on a sheet, and repeating `1-` costs a few bytes of input for each copy of
/// the whole document.
pub const MAX_SELECTED: usize = 100_000;

/// The first page with each label, looked up once per token instead of scanning every label.
struct Labels<'a> {
    pages: HashMap<&'a str, usize>,
    /// The longest label, in bytes: a token longer than that is no label, and is not hashed (a
    /// dash-heavy token would otherwise hash every split of itself).
    longest: usize,
}

impl<'a> Labels<'a> {
    fn new(labels: &'a [String]) -> Self {
        let mut pages = HashMap::with_capacity(labels.len());
        for (i, l) in labels.iter().enumerate() {
            pages.entry(l.as_str()).or_insert(i);
        }
        let longest = pages.keys().map(|l| l.len()).max().unwrap_or(0);
        Labels { pages, longest }
    }

    fn get(&self, token: &str) -> Option<usize> {
        if token.len() > self.longest { None } else { self.pages.get(token).copied() }
    }

    fn contains_key(&self, token: &str) -> bool {
        self.get(token).is_some()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Subset {
    #[default]
    All,
    Odd,
    Even,
}

/// Resolve one range token: a page label, a number in parentheses for the page at that position,
/// or a number (1-based).
fn page_of(tok: &str, count: usize, labels: &Labels) -> Result<usize, PrintError> {
    let t = tok.trim();
    // Page labels win over numbers (Acrobat's "use logical page numbers").
    if let Some(i) = labels.get(t) {
        return Ok(i);
    }
    let number = parenthesized(t).unwrap_or(t);
    // `usize::from_str` accepts a leading '+', which is not part of a page number.
    if !number.bytes().all(|b| b.is_ascii_digit()) {
        return Err(PrintError::NotAPage(t.to_string()));
    }
    match number.parse::<usize>() {
        Ok(n) if n >= 1 && n <= count => Ok(n - 1),
        Ok(n) => Err(PrintError::OutOfRange(n, count)),
        Err(_) => Err(PrintError::NotAPage(t.to_string())),
    }
}

/// What a token holds between one pair of parentheses, trimmed: "( 3 )" holds "3".
fn parenthesized(t: &str) -> Option<&str> {
    t.trim().strip_prefix('(')?.strip_suffix(')').map(str::trim)
}

/// The page position (1-based) a token names in parentheses. A page's label and its position
/// need not coincide (ISO 32000-2 §12.4.2): "(n)" always names the nth page, so a page can be
/// named by position where a label such as "1" belongs to another page. `None` for anything
/// else. Callers look labels up first, so a label spelled "(1)" keeps naming its own page.
pub fn position(token: &str) -> Option<usize> {
    parenthesized(token).filter(|n| n.bytes().all(|b| b.is_ascii_digit()))?.parse().ok()
}

/// Split a range whose endpoint is a logical page label containing a hyphen, such
/// as "A-1-A-3" or "A-2-". The plain range parser must not mistake the first
/// hyphen inside the label for the delimiter between endpoints.
fn hyphenated_label_range(part: &str, count: usize, labels: &Labels) -> Option<(usize, usize)> {
    for (split, _) in part.match_indices('-') {
        let (Some(first), Some(last)) = (part.get(..split), part.get(split + 1..)) else { continue };
        let (first, last) = (first.trim(), last.trim());
        // Avoid changing the meaning of ordinary numeric and open-ended ranges.
        if ![first, last].iter().any(|l| labels.contains_key(l) && l.contains('-')) {
            continue;
        }
        let from = if first.is_empty() { Some(0) } else { page_of(first, count, labels).ok() };
        let to = if last.is_empty() { count.checked_sub(1) } else { page_of(last, count, labels).ok() };
        if let (Some(from), Some(to)) = (from, to) {
            return Some((from, to));
        }
    }
    None
}

/// The pages to print (0-based, in print order). `range` is `None` for all pages; it may list
/// numbers, labels and `(n)` positions with `-` ranges (open-ended allowed: `5-`, `-3`). The subset
/// keeps the odd or even pages of those chosen, by page number (see [`narrow`]).
pub fn select_pages(count: usize, range: Option<&str>, labels: &[String], subset: Subset, reverse: bool) -> Result<Vec<usize>, PrintError> {
    let mut pages = Vec::new();
    match range.map(str::trim).filter(|r| !r.is_empty()) {
        None => pages.extend(0..count),
        Some(r) => {
            let index = Labels::new(labels);
            let labels = &index;
            let limit = count.max(MAX_SELECTED);
            let mut selected = 0usize;
            // Checked before a span is expanded, so the list never grows past the limit.
            let mut take = |from: usize, to: usize| {
                selected = selected.saturating_add(from.abs_diff(to).saturating_add(1));
                if selected > limit { Err(PrintError::TooManySelected(limit)) } else { Ok(()) }
            };
            for part in r.split([',', ';']).map(str::trim).filter(|p| !p.is_empty()) {
                // A complete label (e.g. "A-1") still wins over interpreting it as a range.
                // A range of such labels (e.g. "A-1-A-3") needs a later '-' split.
                if !labels.contains_key(part)
                    && let Some((from, to)) = hyphenated_label_range(part, count, labels)
                {
                    take(from, to)?;
                    if from <= to {
                        pages.extend(from..=to);
                    } else {
                        pages.extend((to..=from).rev());
                    }
                    continue;
                }
                match part.split_once('-') {
                    // A label may itself contain a dash ("A-1"): try the whole token first.
                    Some(_) if labels.contains_key(part) => {
                        take(0, 0)?;
                        pages.push(page_of(part, count, labels)?);
                    }
                    Some((a, b)) => {
                        let from = if a.trim().is_empty() { 0 } else { page_of(a, count, labels)? };
                        let to = if b.trim().is_empty() { count.checked_sub(1).ok_or(PrintError::NoPages)? } else { page_of(b, count, labels)? };
                        take(from, to)?;
                        if from <= to {
                            pages.extend(from..=to);
                        } else {
                            pages.extend((to..=from).rev());
                        }
                    }
                    None => {
                        take(0, 0)?;
                        pages.push(page_of(part, count, labels)?);
                    }
                }
            }
        }
    }
    narrow(pages, subset, reverse)
}

/// The pages to print from an explicit list (0-based, e.g. the thumbnails selected in the
/// interface), in the order given. Unlike a typed range the entries are page positions, never
/// labels. The subset and `reverse` apply as in [`select_pages`].
pub fn select_listed(count: usize, pages: &[usize], subset: Subset, reverse: bool) -> Result<Vec<usize>, PrintError> {
    if let Some(p) = pages.iter().find(|p| **p >= count) {
        return Err(PrintError::OutOfRange(p.saturating_add(1), count));
    }
    narrow(pages.to_vec(), subset, reverse)
}

/// The odd or even pages of those chosen, by page number (the first page of the document is 1,
/// odd; `pages` are 0-based), then the print order. Acrobat counts the same way (#3984): "if you
/// set the range to 2, 7–10 and choose Even Pages Only, only pages 2, 8, and 10 print" (Adobe
/// Acrobat DC Help, last updated 11/28/2019, Options in the Print dialog box, p. 515; the same
/// example in Using Adobe Acrobat 9 Pro, p. 413). Counting places in the range instead printed
/// 7 and 9 there, and a range starting on an even page started its "even" pages on an odd one.
fn narrow(pages: Vec<usize>, subset: Subset, reverse: bool) -> Result<Vec<usize>, PrintError> {
    let mut out: Vec<usize> = match subset {
        Subset::All => pages,
        Subset::Odd => pages.into_iter().filter(|p| p % 2 == 0).collect(),
        Subset::Even => pages.into_iter().filter(|p| p % 2 == 1).collect(),
    };
    if reverse {
        out.reverse();
    }
    if out.is_empty() {
        return Err(PrintError::NoPages);
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn open_ended_range_of_empty_document_is_an_error() {
        assert!(select_pages(0, Some("-"), &[], Subset::All, false).is_err());
    }

    #[test]
    fn plus_sign_is_not_a_page() {
        for r in ["+3", "+2-4", "2-+4"] {
            assert!(matches!(select_pages(5, Some(r), &[], Subset::All, false), Err(PrintError::NotAPage(_))), "accepted {r}");
        }
    }

    #[test]
    fn open_ended_range_covers_all_pages() {
        assert_eq!(select_pages(5, Some("-"), &[], Subset::All, false).unwrap(), vec![0, 1, 2, 3, 4]);
    }

    /// Repeating a range is allowed up to the limit, and refused past it before the pages are
    /// listed: 1,000 pages selected 100 times is the limit, once more is not.
    #[test]
    fn a_range_selects_at_most_the_limit() {
        let whole = vec!["1-"; MAX_SELECTED / 1000].join(",");
        assert_eq!(select_pages(1000, Some(&whole), &[], Subset::All, false).unwrap().len(), MAX_SELECTED);
        let e = select_pages(1000, Some(&format!("{whole},1")), &[], Subset::All, false).unwrap_err();
        assert!(e.to_string().contains("more than 100000 pages"), "{e}");
        // A document longer than the limit can still be printed whole, once.
        assert_eq!(select_pages(MAX_SELECTED + 1, Some("1-"), &[], Subset::All, false).unwrap().len(), MAX_SELECTED + 1);
    }

    /// Labels resolve through one index: the first page with a label wins, and a hyphenated
    /// label works as an endpoint, among many labels and many dashes.
    #[test]
    fn labels_resolve_through_the_index() {
        let labels: Vec<String> = (0..50_000).map(|i| format!("A-{i}")).collect();
        assert_eq!(select_pages(labels.len(), Some("A-3-A-5"), &labels, Subset::All, false).unwrap(), vec![3, 4, 5]);
        let dashes = "-".repeat(10_000);
        assert!(select_pages(labels.len(), Some(&dashes), &labels, Subset::All, false).is_err());
        // a token with no label as long as any of its halves costs no hashing of them
        let long_token = format!("{}A-3", "-".repeat(200_000));
        assert!(select_pages(labels.len(), Some(&long_token), &labels, Subset::All, false).is_err());
        let twice = vec!["x".to_string(), "x".to_string()];
        assert_eq!(select_pages(2, Some("x"), &twice, Subset::All, false).unwrap(), vec![0]);
    }
}
