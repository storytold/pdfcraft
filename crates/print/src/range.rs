//! Pages to Print: all, a range ("1-3, 6, 9-", page labels allowed), odd or even pages, reverse.

use crate::PrintError;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Subset {
    #[default]
    All,
    Odd,
    Even,
}

/// Resolve one range token: a page label, or a number (1-based).
fn page_of(tok: &str, count: usize, labels: &[String]) -> Result<usize, PrintError> {
    let t = tok.trim();
    // Page labels win over numbers (Acrobat's "use logical page numbers").
    if let Some(i) = labels.iter().position(|l| l == t) {
        return Ok(i);
    }
    match t.parse::<usize>() {
        Ok(n) if n >= 1 && n <= count => Ok(n - 1),
        Ok(n) => Err(PrintError::OutOfRange(n, count)),
        Err(_) => Err(PrintError::NotAPage(t.to_string())),
    }
}

/// Split a range whose endpoint is a logical page label containing a hyphen, such
/// as "A-1-A-3" or "A-2-". The plain range parser must not mistake the first
/// hyphen inside the label for the delimiter between endpoints.
fn hyphenated_label_range(part: &str, count: usize, labels: &[String]) -> Option<(usize, usize)> {
    for (split, _) in part.match_indices('-') {
        let (Some(first), Some(last)) = (part.get(..split), part.get(split + 1..)) else { continue };
        let (first, last) = (first.trim(), last.trim());
        // Avoid changing the meaning of ordinary numeric and open-ended ranges.
        if !labels.iter().any(|l| l.contains('-') && (l == first || l == last)) {
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
/// numbers and labels with `-` ranges (open-ended allowed: `5-`, `-3`). The subset keeps the odd or
/// even pages of those chosen, by page number (see [`narrow`]).
pub fn select_pages(count: usize, range: Option<&str>, labels: &[String], subset: Subset, reverse: bool) -> Result<Vec<usize>, PrintError> {
    let mut pages = Vec::new();
    match range.map(str::trim).filter(|r| !r.is_empty()) {
        None => pages.extend(0..count),
        Some(r) => {
            for part in r.split([',', ';']).map(str::trim).filter(|p| !p.is_empty()) {
                // A complete label (e.g. "A-1") still wins over interpreting it as a range.
                // A range of such labels (e.g. "A-1-A-3") needs a later '-' split.
                if !labels.iter().any(|l| l == part)
                    && let Some((from, to)) = hyphenated_label_range(part, count, labels)
                {
                    if from <= to {
                        pages.extend(from..=to);
                    } else {
                        pages.extend((to..=from).rev());
                    }
                    continue;
                }
                match part.split_once('-') {
                    // A label may itself contain a dash ("A-1"): try the whole token first.
                    Some(_) if labels.iter().any(|l| l == part) => pages.push(page_of(part, count, labels)?),
                    Some((a, b)) => {
                        let from = if a.trim().is_empty() { 0 } else { page_of(a, count, labels)? };
                        let to = if b.trim().is_empty() { count.checked_sub(1).ok_or(PrintError::NoPages)? } else { page_of(b, count, labels)? };
                        if from <= to {
                            pages.extend(from..=to);
                        } else {
                            pages.extend((to..=from).rev());
                        }
                    }
                    None => pages.push(page_of(part, count, labels)?),
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
    fn open_ended_range_covers_all_pages() {
        assert_eq!(select_pages(5, Some("-"), &[], Subset::All, false).unwrap(), vec![0, 1, 2, 3, 4]);
    }
}
