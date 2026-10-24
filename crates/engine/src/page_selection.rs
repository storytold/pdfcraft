//! Bounded physical page-number selections shared by extraction and Organize Pages.
use crate::{PageParity, print::PrintError};
use std::collections::BTreeSet;

const MAX_PAGES: usize = 1_000_000;

/// Blank clears the explicit selection. Repeated/reversed ranges become a sorted set.
/// Bound both the input and expansion work, including repeated overlapping ranges.
pub fn parse(text: &str, count: usize) -> Result<Vec<usize>, PrintError> {
    if text.len() > 16_384 {
        return Err(PrintError::NotAPage(text.chars().take(80).collect()));
    }
    if text.trim().is_empty() {
        return Ok(Vec::new());
    }
    let mut pages = BTreeSet::new();
    let mut budget = MAX_PAGES;
    let number = |token: &str| {
        let n = token.trim().parse::<usize>().map_err(|_| PrintError::NotAPage(token.into()))?;
        if n == 0 || n > count { Err(PrintError::OutOfRange(n, count)) } else { Ok(n - 1) }
    };
    for part in text.split([',', ';']).map(str::trim) {
        let (a, b) = if let Some((a, b)) = part.split_once('-') {
            (number(a)?, number(b)?)
        } else {
            let p = number(part)?;
            (p, p)
        };
        let (a, b) = (a.min(b), a.max(b));
        let size = b.saturating_sub(a).saturating_add(1);
        budget = budget.checked_sub(size).ok_or_else(|| PrintError::NotAPage(part.into()))?;
        pages.extend(a..=b);
    }
    Ok(pages.into_iter().collect())
}

/// Resolve either explicit 0-based pages or a physical 1-based range, then apply parity.
/// With neither selector, use all pages. Typed ranges are sorted and deduplicated;
/// explicit lists retain their order and duplicates. Odd/even uses physical page numbers,
/// never positions within the selection. An empty result is returned for callers to reject.
/// Bound input/expansion work and reject conflicting selectors or out-of-range indices.
pub fn select(count: usize, pages: Option<&[usize]>, range: Option<&str>, parity: PageParity) -> Result<Vec<usize>, PrintError> {
    if pages.is_some() && range.is_some() {
        return Err(PrintError::Invalid("choose either pages or range, not both".into()));
    }
    let mut selected = match (pages, range) {
        (Some(pages), None) => {
            if pages.len() > MAX_PAGES {
                return Err(PrintError::Invalid("page selection exceeds 1000000 entries".into()));
            }
            if let Some(page) = pages.iter().find(|&&p| p >= count) {
                return Err(PrintError::OutOfRange(page.saturating_add(1), count));
            }
            pages.to_vec()
        }
        (_, Some(range)) => parse(range, count)?,
        _ => {
            if count > MAX_PAGES {
                return Err(PrintError::Invalid("choose a range of at most 1000000 pages".into()));
            }
            (0..count).collect()
        }
    };
    selected.retain(|p| match parity {
        PageParity::Both => true,
        PageParity::Even => p % 2 == 1,
        PageParity::Odd => p % 2 == 0,
    });
    Ok(selected)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Physical parity must not restart at the first page of a disjoint or even-starting range.
    #[test]
    fn extraction_ranges_use_physical_parity_and_keep_explicit_order() {
        assert_eq!(select(15, None, Some("1-5, 8, 12-15"), PageParity::Both).unwrap(), [0, 1, 2, 3, 4, 7, 11, 12, 13, 14]);
        assert_eq!(select(15, None, Some("2-5, 8, 12-15"), PageParity::Odd).unwrap(), [2, 4, 12, 14]);
        assert_eq!(select(15, None, Some("2-5, 8, 12-15"), PageParity::Even).unwrap(), [1, 3, 7, 11, 13]);
        assert_eq!(select(5, None, None, PageParity::Odd).unwrap(), [0, 2, 4]);
        assert_eq!(select(5, None, None, PageParity::Even).unwrap(), [1, 3]);
        assert_eq!(select(5, Some(&[4, 0, 4, 1]), None, PageParity::Odd).unwrap(), [4, 0, 4]);
        assert_eq!(parse("5-3,4;3", 5).unwrap(), [2, 3, 4]);
    }

    /// Malformed, oversized and empty inputs remain bounded and never become all-pages selections.
    #[test]
    fn extraction_page_selection_rejects_bad_or_unbounded_inputs() {
        for bad in ["0", "6", "1,", "x", "1--3", "18446744073709551616", "1-100000000"] {
            assert!(parse(bad, 5).is_err(), "{bad}");
        }
        assert!(parse(&"1,".repeat(10_000), 5).is_err());
        assert!(parse("1-1000000,1-1000000", MAX_PAGES).is_err());
        assert!(select(5, Some(&[0]), Some("1"), PageParity::Both).is_err());
        assert!(select(5, Some(&[usize::MAX]), None, PageParity::Both).is_err());
        assert!(select(usize::MAX, None, None, PageParity::Both).is_err());
        assert!(select(5, None, Some(" "), PageParity::Both).unwrap().is_empty());
        assert!(select(1, None, None, PageParity::Even).unwrap().is_empty());
        assert!(select(0, None, None, PageParity::Both).unwrap().is_empty());
    }
}
