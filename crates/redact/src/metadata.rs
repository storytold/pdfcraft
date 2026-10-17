//! Comparison-only normalization; the document's original text is never rewritten here.

use std::cell::Cell;

pub(crate) const MAX_FRAGMENTS: usize = 1_024;
pub(crate) const MAX_RUN_BYTES: usize = 16 << 10;
pub(crate) const MAX_FRAGMENT_BYTES: usize = 256 << 10;
pub(crate) const MAX_VALUE_BYTES: usize = 1 << 20;
const MAX_WORK: usize = 8 << 20;

/// One shared operation budget, including failed comparisons. Exhaustion is not a no-match:
/// the caller must remove all metadata in its scope, including packets already found clean.
pub(crate) struct Work {
    remaining: Cell<usize>,
    pub exhausted: Cell<bool>,
}

impl Work {
    pub fn new() -> Self {
        Self { remaining: Cell::new(MAX_WORK), exhausted: Cell::new(false) }
    }

    pub fn charge(&self, amount: usize) -> bool {
        if !self.exhausted.get()
            && let Some(left) = self.remaining.get().checked_sub(amount)
        {
            self.remaining.set(left);
            return true;
        }
        self.exhausted.set(true);
        false
    }

    pub fn decode_limit(&self) -> usize {
        self.remaining.get() / 4
    }
}

pub(crate) fn normalize(text: &str) -> String {
    text.split_whitespace().map(str::to_lowercase).collect::<Vec<_>>().join(" ")
}

/// A removed run with optional word spaces at forward `TJ` displacements. Kerning is ambiguous:
/// compare both interpretations without enumerating the exponentially many possible strings.
pub(crate) struct KernedFragment {
    pub text: String,
    pub gaps: Vec<usize>,
}

impl KernedFragment {
    pub fn normalized(&self) -> Self {
        let mut text = String::new();
        let mut gaps = Vec::new();
        let (mut space, mut gap) = (false, false);
        for (offset, c) in self.text.char_indices() {
            gap |= self.gaps.binary_search(&offset).is_ok();
            if c.is_whitespace() {
                space = !text.is_empty();
                continue;
            }
            if space {
                text.push(' ');
            } else if gap && !text.is_empty() {
                gaps.push(text.len());
            }
            text.extend(c.to_lowercase());
            space = false;
            gap = false;
        }
        Self { text, gaps }
    }

    #[cfg(test)]
    pub fn matches(&self, value: &str) -> bool {
        self.matches_with_budget(value, &Work::new())
    }

    pub fn matches_with_budget(&self, value: &str, work: &Work) -> bool {
        let Some(first) = self.text.chars().next() else { return false };
        // A complete removed run inside a metadata value.
        if !work.charge(value.len())
            || value.match_indices(first).any(|(start, _)| self.prefix(0, value.get(start..).unwrap_or_default(), true, work))
        {
            return true;
        }
        let Some(first) = value.chars().next() else { return false };
        // Or the complete metadata value inside a removed run. Never match isolated terms.
        !work.charge(self.text.len()) || self.text.match_indices(first).any(|(start, _)| self.prefix(start, value, false, work))
    }

    fn prefix(&self, start: usize, value: &str, whole_run: bool, work: &Work) -> bool {
        let Some(run) = self.text.get(start..) else { return false };
        let mut value = value.chars().peekable();
        for (offset, c) in run.char_indices() {
            // Charge repeated prefix attempts too. A collected run has at most 16 KiB of
            // bytes/gaps, so 16 units cover the binary search and fixed character work.
            if !work.charge(16) {
                return true;
            }
            if value.peek().is_none() {
                return !whole_run;
            }
            if offset > 0 && self.gaps.binary_search(&start.saturating_add(offset)).is_ok() && value.peek() == Some(&' ') {
                value.next();
            }
            if value.next() != Some(c) {
                return false;
            }
        }
        whole_run || value.next().is_none()
    }
}

/// XML parsers expose decoded text and attributes, excluding tag/attribute names and namespace
/// declarations. DTDs are disabled by the parser's defaults. Uninspectable XML fails closed.
pub(crate) fn packet_hit(text: &str, hit: &impl Fn(&str) -> bool) -> bool {
    let Ok(doc) = roxmltree::Document::parse(text) else { return true };
    doc.descendants().any(|node| {
        ((node.is_text() || node.is_comment()) && node.text().is_some_and(hit))
            || node.pi().filter(|pi| pi.target != "xpacket").and_then(|pi| pi.value).is_some_and(hit)
            || node.attributes().any(|attribute| hit(attribute.value()))
    })
}
