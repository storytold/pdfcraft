//! Page labels (ISO 32000-2 §12.4.2): "i, ii, iii, 1, 2, A-1…", execution plan M4.4.
//!
//! Labels are a number tree of ranges: from page `start` on, use a numbering `style`, a
//! `prefix` and a first number. [`number_pages`] is Acrobat's "Number pages": relabel pages
//! `from..=to`, and keep every later page showing the label it had before. The tree is written
//! back as one flat `/Nums` array, with ranges that merely continue the previous one merged.

use std::collections::HashSet;

use pdfcraft_cos::page_labels::{MAX_LABEL_BYTES, MAX_LABEL_TOTAL_BYTES, MAX_LABEL_TREE_DEPTH, MAX_LABEL_TREE_WORK, MAX_PREFIX_BYTES, alpha, roman};
use pdfcraft_cos::{Dict, Document, ObjRef, Object, PdfString};

use crate::{OrganizeError, page_count, pages_root};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LabelStyle {
    /// 1, 2, 3
    Decimal,
    /// I, II, III
    UpperRoman,
    /// i, ii, iii
    LowerRoman,
    /// A … Z, AA … ZZ
    UpperAlpha,
    /// a … z, aa … zz
    LowerAlpha,
    /// Prefix only (no number).
    None,
}

impl LabelStyle {
    fn from_name(n: &[u8]) -> Self {
        match n {
            b"D" => Self::Decimal,
            b"R" => Self::UpperRoman,
            b"r" => Self::LowerRoman,
            b"A" => Self::UpperAlpha,
            b"a" => Self::LowerAlpha,
            _ => Self::None,
        }
    }

    fn name(self) -> Option<&'static str> {
        match self {
            Self::Decimal => Some("D"),
            Self::UpperRoman => Some("R"),
            Self::LowerRoman => Some("r"),
            Self::UpperAlpha => Some("A"),
            Self::LowerAlpha => Some("a"),
            Self::None => None,
        }
    }

    /// Format `n` (≥ 1) in this style for previews.
    /// A numeral exceeding 1024 bytes falls back to decimal; editing and document label
    /// queries use the checked formatter and return an error instead of storing a fallback.
    pub fn format(self, n: u32) -> String {
        self.format_checked(n, MAX_LABEL_BYTES).unwrap_or_else(|_| n.to_string())
    }

    fn format_checked(self, n: u32, max_bytes: usize) -> Result<String, OrganizeError> {
        let mut out = match self {
            Self::Decimal => Some(n.to_string()),
            Self::UpperRoman | Self::LowerRoman => roman(u64::from(n), max_bytes),
            Self::UpperAlpha | Self::LowerAlpha => alpha(u64::from(n), max_bytes),
            Self::None => Some(String::new()),
        }
        .ok_or_else(|| invalid_label("a label exceeds 1024 UTF-8 bytes"))?;
        if out.len() > max_bytes {
            return Err(invalid_label("a label exceeds 1024 UTF-8 bytes"));
        }
        if matches!(self, Self::UpperRoman | Self::UpperAlpha) {
            out.make_ascii_uppercase();
        }
        Ok(out)
    }
}

/// One labelling range.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LabelRange {
    /// First page (0-based) of the range.
    pub start: usize,
    pub style: LabelStyle,
    pub prefix: String,
    /// The number of the range's first page (`/St`, at least 1).
    pub first: u32,
}

impl LabelRange {
    fn number(&self, page: usize) -> Result<u32, OrganizeError> {
        if self.style == LabelStyle::None {
            return Ok(self.first); // /St is ignored when the range has no numbering style.
        }
        let offset = page.checked_sub(self.start).and_then(|n| u32::try_from(n).ok()).ok_or_else(|| invalid_label("a page number is too large"))?;
        self.first.checked_add(offset).ok_or_else(|| invalid_label("a page number overflows"))
    }

    fn label(&self, page: usize) -> Result<String, OrganizeError> {
        let remaining = MAX_LABEL_BYTES.checked_sub(self.prefix.len()).ok_or_else(|| invalid_label("a prefix exceeds 1024 UTF-8 bytes"))?;
        let number = self.style.format_checked(self.number(page)?, remaining)?;
        let mut label = self.prefix.clone();
        label.push_str(&number);
        Ok(label)
    }
}

fn invalid_label(reason: &str) -> OrganizeError {
    OrganizeError::Invalid(format!("Page labels could not be processed: {reason}"))
}

fn default_range() -> LabelRange {
    LabelRange { start: 0, style: LabelStyle::Decimal, prefix: String::new(), first: 1 }
}

/// The document's label ranges, sorted by start page (empty when it has no `/PageLabels`).
/// This compatibility query returns an empty list for a tree exceeding resource limits.
/// Label display and edits use the checked reader and return an error instead.
pub fn page_label_ranges(doc: &Document) -> Vec<LabelRange> {
    checked_ranges(doc).unwrap_or_default()
}

// Borrow direct objects instead of cloning complete untrusted dictionaries or arrays.
fn with_label_object<T>(doc: &Document, object: &Object, f: impl FnOnce(&Object) -> Result<T, OrganizeError>) -> Result<T, OrganizeError> {
    let Object::Ref(r) = object else { return f(object) };
    let mut seen = HashSet::from([*r]);
    let mut object = doc.get(*r);
    while let Object::Ref(r) = object.as_ref() {
        if seen.len() >= MAX_LABEL_TREE_DEPTH || !seen.insert(*r) {
            return Err(invalid_label("a number tree reference is cyclic or too deeply nested"));
        }
        object = doc.get(*r);
    }
    f(&object)
}

fn label_work(left: &mut usize, count: usize) -> Result<(), OrganizeError> {
    *left = left.checked_sub(count).ok_or_else(|| invalid_label("the number tree has too many entries"))?;
    Ok(())
}

fn read_label_node(
    doc: &Document,
    node: &Object,
    depth: usize,
    left: &mut usize,
    seen: &mut HashSet<ObjRef>,
    entries: &mut Vec<LabelRange>,
) -> Result<(), OrganizeError> {
    label_work(left, 1)?;
    if depth > MAX_LABEL_TREE_DEPTH {
        return Err(invalid_label("the number tree is too deep"));
    }
    // Only indirect nodes can be reached twice; key them by reference, not by the address of
    // a resolved object (a missing reference resolves to a fresh, short-lived `Null`).
    if let Object::Ref(r) = node
        && !seen.insert(*r)
    {
        return Err(invalid_label("the number tree repeats a node or contains a cycle"));
    }
    with_label_object(doc, node, |node| {
        let Some(d) = node.as_dict() else { return Ok(()) };
        if let Some(nums) = d.get(b"Nums") {
            with_label_object(doc, nums, |nums| {
                let Some(nums) = nums.as_array() else { return Ok(()) };
                label_work(left, nums.len().div_ceil(2))?;
                for pair in nums.as_chunks::<2>().0 {
                    let Some(start) = pair[0].as_int().and_then(|n| usize::try_from(n).ok()) else { continue };
                    with_label_object(doc, &pair[1], |spec| {
                        let Some(spec) = spec.as_dict() else { return Ok(()) };
                        let prefix = match spec.get(b"P") {
                            Some(prefix) => with_label_object(doc, prefix, |p| match p.as_string() {
                                Some(p) if p.bytes.len() > MAX_PREFIX_BYTES => Err(invalid_label("a raw prefix exceeds 2050 bytes")),
                                Some(p) => Ok(p.to_text()),
                                None => Ok(String::new()),
                            })?,
                            None => String::new(),
                        };
                        if prefix.len() > MAX_LABEL_BYTES {
                            return Err(invalid_label("a decoded prefix exceeds 1024 UTF-8 bytes"));
                        }
                        let style = spec.name(b"S").map_or(LabelStyle::None, LabelStyle::from_name);
                        let first = spec.int(b"St").unwrap_or(1).max(1);
                        let first = if style == LabelStyle::None {
                            // /St has no effect without /S; keep the existing compatibility clamp.
                            first.min(i64::from(u32::MAX)) as u32
                        } else {
                            u32::try_from(first).map_err(|_| invalid_label("a first number exceeds the supported integer range"))?
                        };
                        entries.push(LabelRange { start, style, prefix, first });
                        Ok(())
                    })?;
                }
                Ok(())
            })?;
        }
        if let Some(kids) = d.get(b"Kids") {
            with_label_object(doc, kids, |kids| {
                let Some(kids) = kids.as_array() else { return Ok(()) };
                label_work(left, kids.len())?;
                if !kids.is_empty() && depth == MAX_LABEL_TREE_DEPTH {
                    return Err(invalid_label("the number tree is too deep"));
                }
                // Recursion is bounded before descending, and arrays are borrowed throughout.
                for kid in kids {
                    read_label_node(doc, kid, depth + 1, left, seen, entries)?;
                }
                Ok(())
            })?;
        }
        Ok(())
    })
}

fn checked_ranges(doc: &Document) -> Result<Vec<LabelRange>, OrganizeError> {
    let Some(root) = doc.root() else { return Ok(Vec::new()) };
    let catalog = doc.get(root);
    let Some(tree) = catalog.as_dict().and_then(|c| c.get(b"PageLabels")) else { return Ok(Vec::new()) };
    let mut entries = Vec::new();
    let mut left = MAX_LABEL_TREE_WORK;
    read_label_node(doc, tree, 0, &mut left, &mut HashSet::new(), &mut entries)?;
    // A start page given twice keeps its later range, as readers that overwrite do (the sort is
    // stable, so the later one is last among equals).
    entries.sort_by_key(|r| r.start);
    let mut unique: Vec<LabelRange> = Vec::with_capacity(entries.len());
    for r in entries {
        match unique.last_mut() {
            Some(last) if last.start == r.start => *last = r,
            _ => unique.push(r),
        }
    }
    Ok(unique)
}

/// Every page's label. `lenient` (reading a file): a range whose label can't be formatted
/// shows physical page numbers for its pages instead of failing every label.
fn labels_from_ranges(n: usize, ranges: &[LabelRange], lenient: bool) -> Result<Vec<String>, OrganizeError> {
    let mut labels = Vec::new();
    let mut total = 0usize;
    let mut ranges = ranges.iter().peekable();
    let mut active = None;
    for p in 0..n {
        while ranges.peek().is_some_and(|r| r.start <= p) {
            active = ranges.next();
        }
        let label = match active {
            Some(r) => {
                let label = match r.label(p) {
                    Ok(label) => label,
                    Err(_) if lenient => String::new(),
                    Err(e) => return Err(e),
                };
                total = total.checked_add(label.len()).ok_or_else(|| invalid_label("the total label size overflows"))?;
                if total > MAX_LABEL_TOTAL_BYTES {
                    return Err(invalid_label("custom labels exceed 4 MiB in total"));
                }
                if label.is_empty() { (p + 1).to_string() } else { label }
            }
            None => (p + 1).to_string(),
        };
        labels.push(label);
    }
    Ok(labels)
}

/// Every page's label, as a viewer shows it. Resource limits return an actionable error.
pub fn page_labels(doc: &Document) -> Result<Vec<String>, OrganizeError> {
    labels_from_ranges(page_count(doc)?, &checked_ranges(doc)?, true)
}

/// Replace all label ranges (an empty list removes `/PageLabels`).
pub fn set_page_label_ranges(doc: &mut Document, ranges: &[LabelRange]) -> Result<(), OrganizeError> {
    pages_root(doc)?;
    let root = doc.root().ok_or(OrganizeError::NoPageTree)?;
    if ranges.is_empty() {
        doc.update_dict(root, |c| {
            c.remove(b"PageLabels");
        })?;
        return Ok(());
    }
    if ranges.len() >= MAX_LABEL_TREE_WORK {
        return Err(invalid_label("the number tree has too many entries"));
    }
    // Validate every range and all effective labels before changing any document object.
    for r in ranges {
        if r.first == 0 || i64::try_from(r.start).is_err() {
            return Err(invalid_label("range starts and first numbers must be valid positive PDF integers"));
        }
        r.label(r.start)?;
    }
    let mut sorted = ranges.to_vec();
    sorted.sort_by_key(|r| r.start);
    if sorted.windows(2).any(|pair| pair.first().zip(pair.get(1)).is_some_and(|(a, b)| a.start == b.start)) {
        return Err(invalid_label("range start pages must be unique"));
    }
    labels_from_ranges(page_count(doc)?, &sorted, false)?;
    let mut nums = Vec::new();
    for r in &sorted {
        let mut spec = Dict::new();
        if let Some(s) = r.style.name() {
            spec.set(b"S".to_vec(), Object::name(s));
        }
        if !r.prefix.is_empty() {
            spec.set(b"P".to_vec(), Object::String(PdfString::text(&r.prefix)));
        }
        if r.first != 1 {
            spec.set(b"St".to_vec(), Object::Int(i64::from(r.first)));
        }
        nums.push(Object::Int(r.start as i64));
        nums.push(Object::Dict(spec));
    }
    let mut tree = Dict::new();
    tree.set(b"Nums".to_vec(), Object::Array(nums));
    let tree = doc.add(Object::Dict(tree));
    doc.update_dict(root, |c| c.set(b"PageLabels".to_vec(), Object::Ref(tree)))?;
    Ok(())
}

/// Acrobat's "Number pages": label pages `from..=to` (0-based) with `style`, `prefix` and
/// starting number `first`; pages after `to` keep the labels they had.
pub fn number_pages(doc: &mut Document, from: usize, to: usize, style: LabelStyle, prefix: &str, first: u32) -> Result<(), OrganizeError> {
    if prefix.len() > MAX_LABEL_BYTES {
        return Err(invalid_label("a prefix exceeds 1024 UTF-8 bytes"));
    }
    let n = page_count(doc)?;
    if to >= n {
        return Err(OrganizeError::NoSuchPage(to));
    }
    if from > to {
        return Err(OrganizeError::NoSuchPage(from));
    }
    let mut old = checked_ranges(doc)?;
    if old.first().is_none_or(|r| r.start > 0) {
        old.insert(0, default_range());
    }
    // `old` starts at page 0, so a range is always found.
    let at = |p: usize| old.iter().rev().find(|r| r.start <= p).cloned().unwrap_or_else(default_range);
    let mut ranges: Vec<LabelRange> = old.iter().filter(|r| r.start < from).cloned().collect();
    ranges.push(LabelRange { start: from, style, prefix: prefix.to_string(), first: first.max(1) });
    if to + 1 < n {
        let r = at(to + 1);
        let first = r.number(to + 1)?;
        ranges.push(LabelRange { start: to + 1, first, ..r });
        ranges.extend(old.iter().filter(|r| r.start > to + 1).cloned());
    }
    // Drop ranges that only continue the previous one.
    let mut merged: Vec<LabelRange> = Vec::new();
    for r in ranges {
        let continues = merged.last().is_some_and(|p| {
            p.style == r.style
                && p.prefix == r.prefix
                && r.start.checked_sub(p.start).and_then(|n| u32::try_from(n).ok()).and_then(|n| p.first.checked_add(n)) == Some(r.first)
        });
        if !continues {
            merged.push(r);
        }
    }
    // Plain 1, 2, 3 everywhere is what a document without labels shows: remove them.
    if merged.len() == 1 && merged[0] == default_range() {
        merged.clear();
    }
    set_page_label_ranges(doc, &merged)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn page_label_limits_bound_organizer_total_output() {
        let range = LabelRange { start: 0, style: LabelStyle::None, prefix: "x".repeat(MAX_LABEL_BYTES), first: u32::MAX };
        let count = MAX_LABEL_TOTAL_BYTES / MAX_LABEL_BYTES;
        assert_eq!(labels_from_ranges(count, std::slice::from_ref(&range), false).unwrap().len(), count);
        assert!(labels_from_ranges(count + 1, &[range], false).unwrap_err().to_string().contains("4 MiB"));
    }

    #[test]
    fn styles_format_like_the_spec() {
        assert_eq!(LabelStyle::LowerRoman.format(14), "xiv");
        assert_eq!(LabelStyle::UpperRoman.format(1994), "MCMXCIV");
        assert_eq!(LabelStyle::UpperAlpha.format(1), "A");
        assert_eq!(LabelStyle::UpperAlpha.format(27), "AA");
        assert_eq!(LabelStyle::LowerAlpha.format(53), "aaa");
        assert_eq!(LabelStyle::None.format(5), "");
    }
}
