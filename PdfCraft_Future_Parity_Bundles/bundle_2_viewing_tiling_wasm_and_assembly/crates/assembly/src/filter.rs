//! Page Selection Filters (Odd/Even, Orientation, Range Syntax).

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PageOrientation {
    Portrait,
    Landscape,
    Square,
}

#[derive(Debug, Clone)]
pub struct PageMetadata {
    pub index: usize, // 0-based
    pub width: f32,
    pub height: f32,
}

impl PageMetadata {
    pub fn orientation(&self) -> PageOrientation {
        if self.width > self.height {
            PageOrientation::Landscape
        } else if self.height > self.width {
            PageOrientation::Portrait
        } else {
            PageOrientation::Square
        }
    }
}

pub struct PageSelectionEngine;

impl PageSelectionEngine {
    /// Selects odd pages (1-based: 1, 3, 5, ...; 0-based index: 0, 2, 4, ...).
    pub fn select_odd(total_pages: usize) -> Vec<usize> {
        (0..total_pages).filter(|i| i % 2 == 0).collect()
    }

    /// Selects even pages (1-based: 2, 4, 6, ...; 0-based index: 1, 3, 5, ...).
    pub fn select_even(total_pages: usize) -> Vec<usize> {
        (0..total_pages).filter(|i| i % 2 == 1).collect()
    }

    /// Selects pages by orientation (Portrait or Landscape).
    pub fn select_by_orientation(pages: &[PageMetadata], target: PageOrientation) -> Vec<usize> {
        pages.iter()
            .filter(|p| p.orientation() == target)
            .map(|p| p.index)
            .collect()
    }

    /// Parses complex user range syntax (e.g., "1-3, 5, 8-10, end").
    pub fn parse_range_syntax(expr: &str, total_pages: usize) -> Vec<usize> {
        let mut selected = Vec::new();
        if total_pages == 0 {
            return selected;
        }

        for token in expr.split(',') {
            let token = token.trim();
            if token.is_empty() {
                continue;
            }

            if token.eq_ignore_ascii_case("end") {
                selected.push(total_pages - 1);
            } else if let Some((start_str, end_str)) = token.split_once('-') {
                let start_1based = start_str.trim().parse::<usize>().unwrap_or(1);
                let end_1based = if end_str.trim().eq_ignore_ascii_case("end") {
                    total_pages
                } else {
                    end_str.trim().parse::<usize>().unwrap_or(total_pages)
                };

                let s = start_1based.max(1).min(total_pages) - 1;
                let e = end_1based.max(1).min(total_pages) - 1;

                for idx in s..=e {
                    if !selected.contains(&idx) {
                        selected.push(idx);
                    }
                }
            } else if let Ok(single_1based) = token.parse::<usize>() {
                if single_1based >= 1 && single_1based <= total_pages {
                    let idx = single_1based - 1;
                    if !selected.contains(&idx) {
                        selected.push(idx);
                    }
                }
            }
        }

        selected.sort_unstable();
        selected
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_page_selection_range_syntax() {
        let total = 10;
        let res = PageSelectionEngine::parse_range_syntax("1-3, 5, 8-end", total);
        // Expect: 0, 1, 2, 4, 7, 8, 9
        assert_eq!(res, vec![0, 1, 2, 4, 7, 8, 9]);
    }

    #[test]
    fn test_orientation_selection() {
        let pages = vec![
            PageMetadata { index: 0, width: 612.0, height: 792.0 }, // Portrait
            PageMetadata { index: 1, width: 1000.0, height: 500.0 }, // Landscape
            PageMetadata { index: 2, width: 612.0, height: 792.0 }, // Portrait
        ];

        let landscape = PageSelectionEngine::select_by_orientation(&pages, PageOrientation::Landscape);
        assert_eq!(landscape, vec![1]);
    }
}
