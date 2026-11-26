//! Heading levels for blocks of text laid out on the page, by the rules Auto-tag uses for text
//! objects ([`crate::autotag_text`]): the body style is the size carrying the most characters,
//! a block is a heading when it is short and larger than the body (or bold where the body is
//! not), and levels rank the heading styles over the whole document without skipping a level.

use crate::autotag_roles::{Evidence, roles};

/// What one block of text shows: a run of lines set in one style, such as a heading that wraps.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct TextBlock {
    /// The size it is shown at, in user space.
    pub size: f64,
    pub bold: bool,
    pub lines: usize,
    /// Characters shown, whitespace excluded.
    pub chars: usize,
    /// Whether it includes a letter: a number alone is never a heading.
    pub letters: bool,
}

/// The heading level of each block, in document order: `Some(1)` to `Some(6)`, or `None` for a
/// block that is not a heading. A heading's level is at most one deeper than the heading before.
pub fn heading_levels(blocks: &[TextBlock]) -> Vec<Option<u8>> {
    let evidence: Vec<Evidence> = blocks
        .iter()
        .map(|b| Evidence { size: b.size, bold: b.bold, lines: b.lines, chars: b.chars, letters: b.letters, ..Evidence::default() })
        .collect();
    roles(&evidence).into_iter().map(|role| role.strip_prefix('H').and_then(|n| n.parse().ok())).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn block(size: f64, bold: bool, lines: usize, chars: usize) -> TextBlock {
        TextBlock { size, bold, lines, chars, letters: true }
    }

    #[test]
    fn blocks_take_the_levels_auto_tag_gives_text_objects() {
        let blocks = [
            block(18.0, true, 1, 12),
            block(11.0, false, 8, 600),
            block(14.0, true, 2, 30),
            block(11.0, false, 6, 500),
            block(18.0, true, 1, 9),
            block(11.0, true, 1, 20),
            TextBlock { letters: false, ..block(30.0, false, 1, 2) },
        ];
        assert_eq!(heading_levels(&blocks), [Some(1), None, Some(2), None, Some(1), Some(2), None]);
    }

    #[test]
    fn no_text_has_no_headings() {
        assert!(heading_levels(&[]).is_empty());
        assert_eq!(heading_levels(&[TextBlock::default()]), [None]);
    }
}
