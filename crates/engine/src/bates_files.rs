//! Bates numbering across several files as one run: each file's first number follows the last
//! number of the file before it. The app's Bates numbering ▸ Add to files… numbers with these
//! rules, which are the `doc_bates_multi` tool's.

use crate::{DocId, Edit, EditError, HeaderFooter, Session};

/// The most files one run numbers.
pub const BATES_MAX_FILES: usize = 1000;
/// The most characters in a prefix or a suffix.
pub const BATES_AFFIX_CHARS: usize = 128;
/// The most digits a number is padded to.
pub const BATES_MAX_DIGITS: u8 = 12;
/// The most bytes of files one run reads.
pub const BATES_MAX_BYTES: u64 = 512 * 1024 * 1024;

/// How a run's numbers read and where they are drawn.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BatesFormat {
    /// The first file's first number, at least 1.
    pub start: u64,
    /// Numbers shorter than this are padded with zeros (1 to [`BATES_MAX_DIGITS`]).
    pub digits: u8,
    pub prefix: String,
    pub suffix: String,
    /// The header and footer box the number goes in: 0 to 2 are the header's left, centre and
    /// right, 3 to 5 the footer's.
    pub position: usize,
}

/// The numbers one file gets, both ends included.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BatesSpan {
    pub first: u64,
    pub last: u64,
}

/// Why a run can't be numbered. File indexes count from 0.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum BatesError {
    #[error("the start number must be at least 1")]
    Start,
    #[error("the number of digits must be from 1 to 12")]
    Digits,
    #[error("a prefix or suffix must be at most 128 characters and can't contain # or >>")]
    Affix,
    #[error("the number must go in one of the six header and footer boxes")]
    Position,
    #[error("there are no files to number, or more than 1000")]
    Count,
    #[error("file {0} has no pages to number")]
    NoPages(usize),
    #[error("the Bates numbers of file {0} run past the largest number")]
    Overflow(usize),
}

impl BatesFormat {
    /// Refuses a format the Bates token can't carry.
    pub fn check(&self) -> Result<(), BatesError> {
        if self.start == 0 {
            return Err(BatesError::Start);
        }
        if !(1..=BATES_MAX_DIGITS).contains(&self.digits) {
            return Err(BatesError::Digits);
        }
        let bad = |s: &str| s.chars().count() > BATES_AFFIX_CHARS || s.contains('#') || s.contains(">>");
        if bad(&self.prefix) || bad(&self.suffix) {
            return Err(BatesError::Affix);
        }
        if self.position > 5 {
            return Err(BatesError::Position);
        }
        Ok(())
    }

    /// The span of each file, in order, for files of `pages` pages each.
    pub fn spans(&self, pages: &[usize]) -> Result<Vec<BatesSpan>, BatesError> {
        self.check()?;
        if pages.is_empty() || pages.len() > BATES_MAX_FILES {
            return Err(BatesError::Count);
        }
        let mut spans = Vec::with_capacity(pages.len());
        let mut next = Some(self.start);
        for (index, &count) in pages.iter().enumerate() {
            let first = next.ok_or(BatesError::Overflow(index))?;
            let offset = count.checked_sub(1).ok_or(BatesError::NoPages(index))?;
            let last = u64::try_from(offset).ok().and_then(|o| first.checked_add(o)).ok_or(BatesError::Overflow(index))?;
            spans.push(BatesSpan { first, last });
            // The last file may end on the largest number; only a file after it overflows.
            next = last.checked_add(1);
        }
        Ok(spans)
    }

    /// Number `number` as the page shows it.
    pub fn label(&self, number: u64) -> String {
        let width = usize::from(self.digits);
        format!("{}{number:0width$}{}", self.prefix, self.suffix)
    }

    /// Header and footer settings that number a file from `first`.
    pub fn settings(&self, first: u64) -> HeaderFooter {
        let mut settings = HeaderFooter::default();
        if let Some(text) = settings.text.get_mut(self.position) {
            *text = format!("<<Bates Number#{}#{first}#{}#{}>>", self.digits, self.prefix, self.suffix);
        }
        settings
    }
}

impl Session {
    /// Numbers every page of document `id` from `first`, as one header and footer edit.
    pub fn add_bates(&mut self, id: DocId, format: &BatesFormat, first: u64) -> Result<(), EditError> {
        let count = self.get(id).ok_or(EditError::NoDocument)?.info.pages.len();
        self.apply(id, Edit::AddHeaderFooter { pages: (0..count).collect(), settings: format.settings(first), replace: false })
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::{BatesError, BatesFormat};

    fn format(start: u64) -> BatesFormat {
        BatesFormat { start, digits: 6, prefix: "CASE-".into(), suffix: String::new(), position: 5 }
    }

    #[test]
    fn each_file_starts_after_the_last_page_of_the_file_before() {
        let spans = format(17).spans(&[2, 3, 1]).unwrap();
        assert_eq!(spans[0].first, 17);
        for pair in spans.windows(2) {
            assert_eq!(pair[1].first, pair[0].last + 1);
        }
        for (span, pages) in spans.iter().zip([2u64, 3, 1]) {
            assert_eq!(span.last - span.first + 1, pages);
        }
    }

    #[test]
    fn only_a_file_after_the_largest_number_overflows() {
        assert!(format(u64::MAX).spans(&[1]).is_ok());
        assert_eq!(format(u64::MAX).spans(&[1, 1]), Err(BatesError::Overflow(1)));
        assert_eq!(format(u64::MAX - 1).spans(&[3]), Err(BatesError::Overflow(0)));
        assert_eq!(format(1).spans(&[2, 0]), Err(BatesError::NoPages(1)));
    }

    #[test]
    fn a_format_the_token_cannot_carry_is_refused() {
        for bad in [
            BatesFormat { prefix: "A#B".into(), ..format(1) },
            BatesFormat { suffix: "A>>".into(), ..format(1) },
            BatesFormat { prefix: "é".repeat(129), ..format(1) },
        ] {
            assert_eq!(bad.check(), Err(BatesError::Affix));
        }
        assert!(BatesFormat { prefix: "é".repeat(128), ..format(1) }.check().is_ok());
        assert_eq!(BatesFormat { digits: 13, ..format(1) }.check(), Err(BatesError::Digits));
        assert_eq!(format(0).check(), Err(BatesError::Start));
    }
}
