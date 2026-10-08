//! Page labels (ISO 32000-2 §12.4.2): "i, ii, iii, 1, 2, A-1…", execution plan M4.4.
//!
//! Labels are a number tree of ranges: from page `start` on, use a numbering `style`, a
//! `prefix` and a first number. [`number_pages`] is Acrobat's "Number pages": relabel pages
//! `from..=to`, and keep every later page showing the label it had before. The tree is written
//! back as one flat `/Nums` array, with ranges that merely continue the previous one merged.

use pdfcraft_cos::{Dict, Document, Object, PdfString};

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

    /// Format `n` (≥ 1) in this style.
    pub fn format(self, n: u32) -> String {
        match self {
            Self::Decimal => n.to_string(),
            Self::UpperRoman => roman(n).to_uppercase(),
            Self::LowerRoman => roman(n),
            // §12.4.2: A to Z, then AA to ZZ, and so on (the letter repeated).
            Self::UpperAlpha => alpha(n).to_uppercase(),
            Self::LowerAlpha => alpha(n),
            Self::None => String::new(),
        }
    }
}

fn roman(mut n: u32) -> String {
    const T: [(u32, &str); 13] = [
        (1000, "m"),
        (900, "cm"),
        (500, "d"),
        (400, "cd"),
        (100, "c"),
        (90, "xc"),
        (50, "l"),
        (40, "xl"),
        (10, "x"),
        (9, "ix"),
        (5, "v"),
        (4, "iv"),
        (1, "i"),
    ];
    let mut s = String::new();
    for (v, r) in T {
        while n >= v {
            s.push_str(r);
            n -= v;
        }
    }
    s
}

fn alpha(n: u32) -> String {
    let n = n.max(1) - 1;
    let letter = char::from(b'a' + (n % 26) as u8);
    std::iter::repeat_n(letter, (n / 26 + 1).min(1000) as usize).collect()
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
    fn label(&self, page: usize) -> String {
        let n = self.first.saturating_add((page - self.start) as u32);
        format!("{}{}", self.prefix, self.style.format(n))
    }
}

fn default_range() -> LabelRange {
    LabelRange { start: 0, style: LabelStyle::Decimal, prefix: String::new(), first: 1 }
}

/// The document's label ranges, sorted by start page (empty when it has no `/PageLabels`).
pub fn page_label_ranges(doc: &Document) -> Vec<LabelRange> {
    let Some(root) = doc.root() else { return Vec::new() };
    let Some(tree) = doc.get(root).as_dict().and_then(|c| c.get(b"PageLabels").cloned()) else { return Vec::new() };
    let mut entries = Vec::new();
    let mut stack = vec![(tree, 0u8)];
    let mut budget = 10_000;
    while let Some((node, depth)) = stack.pop() {
        budget -= 1;
        if budget == 0 || depth > 32 {
            break;
        }
        let node = doc.resolve(&node);
        let Some(d) = node.as_dict() else { continue };
        if let Some(nums) = d.get(b"Nums").map(|n| doc.resolve(n)).and_then(|n| n.as_array().cloned()) {
            for pair in nums.as_chunks::<2>().0 {
                let (Some(start), Some(spec)) = (pair[0].as_int(), doc.resolve(&pair[1]).as_dict().cloned()) else { continue };
                if start < 0 {
                    continue;
                }
                entries.push(LabelRange {
                    start: start as usize,
                    style: spec.name(b"S").map_or(LabelStyle::None, LabelStyle::from_name),
                    prefix: spec.get(b"P").map(|p| doc.resolve(p)).and_then(|p| p.as_string().map(|s| s.to_text())).unwrap_or_default(),
                    first: spec.int(b"St").unwrap_or(1).clamp(1, u32::MAX as i64) as u32,
                });
            }
        }
        if let Some(kids) = d.get(b"Kids").map(|k| doc.resolve(k)).and_then(|k| k.as_array().cloned()) {
            stack.extend(kids.into_iter().map(|k| (k, depth + 1)));
        }
    }
    entries.sort_by_key(|r| r.start);
    entries.dedup_by_key(|r| r.start);
    entries
}

/// Every page's label, as a viewer shows it.
pub fn page_labels(doc: &Document) -> Result<Vec<String>, OrganizeError> {
    let n = page_count(doc)?;
    let ranges = page_label_ranges(doc);
    Ok((0..n)
        .map(|p| match ranges.iter().rev().find(|r| r.start <= p) {
            Some(r) => r.label(p),
            // Pages before the first range have no label: viewers show the page number.
            None => (p + 1).to_string(),
        })
        .collect())
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
    let mut nums = Vec::new();
    for r in ranges {
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
    let n = page_count(doc)?;
    if to >= n {
        return Err(OrganizeError::NoSuchPage(to));
    }
    if from > to {
        return Err(OrganizeError::NoSuchPage(from));
    }
    let mut old = page_label_ranges(doc);
    if old.first().is_none_or(|r| r.start > 0) {
        old.insert(0, default_range());
    }
    // `old` starts at page 0, so a range is always found.
    let at = |p: usize| old.iter().rev().find(|r| r.start <= p).cloned().unwrap_or_else(default_range);
    let mut ranges: Vec<LabelRange> = old.iter().filter(|r| r.start < from).cloned().collect();
    ranges.push(LabelRange { start: from, style, prefix: prefix.to_string(), first: first.max(1) });
    if to + 1 < n {
        let r = at(to + 1);
        let first = r.first.saturating_add((to + 1 - r.start) as u32);
        ranges.push(LabelRange { start: to + 1, first, ..r });
        ranges.extend(old.iter().filter(|r| r.start > to + 1).cloned());
    }
    // Drop ranges that only continue the previous one.
    let mut merged: Vec<LabelRange> = Vec::new();
    for r in ranges {
        let continues = merged
            .last()
            .is_some_and(|p| p.style == r.style && p.prefix == r.prefix && u64::from(p.first) + (r.start - p.start) as u64 == u64::from(r.first));
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
    fn styles_format_like_the_spec() {
        assert_eq!(LabelStyle::LowerRoman.format(14), "xiv");
        assert_eq!(LabelStyle::UpperRoman.format(1994), "MCMXCIV");
        assert_eq!(LabelStyle::UpperAlpha.format(1), "A");
        assert_eq!(LabelStyle::UpperAlpha.format(27), "AA");
        assert_eq!(LabelStyle::LowerAlpha.format(53), "aaa");
        assert_eq!(LabelStyle::None.format(5), "");
    }
}
