//! PDF layout mapping into the shared extracted-text Markdown writer.
use super::layout::{Cell, Item, Page, items};
use craft_markdown::extracted::{TableCell, Writer};

#[cfg(test)]
pub(crate) use craft_markdown::extracted::{emphasis, paragraph};

impl TableCell for Cell {
    fn text(&self) -> &str {
        &self.text
    }
    fn bold(&self) -> bool {
        self.bold
    }
    fn italic(&self) -> bool {
        self.italic
    }
    fn span(&self) -> usize {
        self.span
    }
}

/// Markdown (CommonMark with pipe tables), retaining PDF reading order and heading detection.
/// Image omission is this export's policy; formatting and syntax live in the shared writer.
pub fn markdown(pages: &[Page]) -> String {
    let mut writer = Writer::new();
    for item in items(pages) {
        match item {
            Item::Para(runs, level) => {
                // Heading weight comes from the heading marker, as in the HTML export.
                let runs: Vec<_> = runs.iter().map(|r| (r.text.as_str(), r.block.bold && level == 0, r.block.italic)).collect();
                writer.paragraph(&runs, level);
            }
            Item::Table(table) => writer.table(&table.rows),
            Item::Img(_) => {}
            Item::PageBreak => writer.page_break(),
        }
    }
    writer.finish()
}

#[cfg(test)]
mod tests {
    use super::paragraph;

    #[test]
    fn empty_or_control_run_does_not_split_styled_runs() {
        assert_eq!(paragraph(&[("hello", true, false), ("", false, false), ("world", true, false)]), "**helloworld**");
        assert_eq!(paragraph(&[("hello", true, false), ("\u{1}", false, false), ("world", true, false)]), "**helloworld**");
    }

    #[test]
    fn empty_styled_run_does_not_affect_neighbor_emphasis() {
        assert_eq!(paragraph(&[("x", true, false), ("", false, true)]), "**x**");
        assert_eq!(paragraph(&[("foo", false, false), (".", true, false), ("", true, true), ("bar", false, false)]), "foo<strong>.</strong>bar");
        // Empty/control runs between differently styled runs still look ahead to the next non-empty run.
        assert_eq!(paragraph(&[("bold", true, false), ("", false, false), ("italic", false, true)]), "<strong>bold</strong>*italic*");
        assert_eq!(paragraph(&[("bold", true, false), ("\u{1}", false, false), (" italic", false, true)]), "**bold** *italic*");
        // Leading empty/control runs do not affect the opening delimiter.
        assert_eq!(paragraph(&[("", false, true), ("\u{1}", true, false), ("lead", true, false)]), "**lead**");
        // All-empty/control paragraphs produce empty output.
        assert_eq!(paragraph(&[("", true, false), ("\u{1}", false, true), ("\u{7f}", false, false)]), "");
    }
}

#[cfg(test)]
#[test]
fn shared_table_span_limit_matches_layout_contract() {
    assert_eq!(craft_markdown::extracted::MAX_SPAN, super::layout::MAX_COLS);
}
