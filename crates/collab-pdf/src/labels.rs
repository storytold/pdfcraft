//! Page labels as room operations (RFC §3.1.8, `docs/collab/STATUS.md` §5 p.3).
//!
//! A page label is a number tree range: from a page on, every page shows a
//! prefix and a number in a style, until the next range starts (ISO 32000-2
//! §12.4.2). The room rewrites that tree through the engine's own "Number
//! pages" (`Edit::NumberPages`), so the labels a client sees and the labels the
//! journal replays are produced by the same code.
//!
//! The operation is addressed by the page the selected span starts on and names
//! the span's last page: `page.label.set` labels `start..=end`,
//! `page.label.clear` removes the range that starts at `start`. Everything a
//! write needs is read and checked before the first one — the range must be
//! inside the document and inside the one existing range it starts in (a set
//! that would swallow another range is refused rather than silently renumbered),
//! every label must be non-empty and at most [`MAX_PAGE_LABEL_BYTES`], and the
//! result must stay under [`MAX_PAGE_LABEL_RANGES`].
//!
//! The accepted operation carries the whole range it replaced (`before`, with
//! its own last page). A set that lands inside a range splits it, so its undo
//! names the ranges the set left over that span (a compare-and-set) and writes
//! one range over them; a clear's undo is a plain set over the range it removed.
//! Neither needs a server-side copy, and neither guesses.

use pdfcraft_collab_proto::{ErrorCode, MAX_PAGE_LABEL_BYTES, MAX_PAGE_LABEL_RANGES, PageLabelStyle, Target};
use pdfcraft_engine::{Edit, LabelStyle};
use pdfcraft_organize::LabelRange;
use serde_json::{Value, json};

use crate::{AdapterError, PdfAdapter};

/// One label range as the room reads it: where it starts, the last page it
/// covers (the page before the next range, or the document's last page) and the
/// numbering it gives.
#[derive(Clone, Debug)]
struct Range {
    start: usize,
    end: usize,
    style: LabelStyle,
    prefix: String,
    first: u32,
}

impl Range {
    fn to_value(&self) -> Value {
        json!({
            "start": self.start,
            "end": self.end,
            "style": style_name(self.style),
            "prefix": self.prefix,
            "first": self.first,
        })
    }

    fn same(&self, other: &Self) -> bool {
        self.start == other.start && self.end == other.end && self.style == other.style && self.prefix == other.prefix && self.first == other.first
    }
}

/// A range as an undo names it: the whole range, its last page included.
fn read_range(value: &Value) -> Result<Range, AdapterError> {
    let number = |field: &str| {
        value.get(field).and_then(Value::as_u64).and_then(|n| usize::try_from(n).ok()).ok_or_else(|| invalid(format!("the range names no {field}")))
    };
    let style = value.get("style").and_then(Value::as_str).and_then(PageLabelStyle::parse).ok_or_else(|| invalid("a range names no known style"))?;
    let prefix = value.get("prefix").and_then(Value::as_str).unwrap_or_default().to_owned();
    let first = value
        .get("first")
        .and_then(Value::as_u64)
        .and_then(|n| u32::try_from(n).ok())
        .filter(|n| *n > 0)
        .ok_or_else(|| invalid("a range names no first number"))?;
    Ok(Range { start: number("start")?, end: number("end")?, style: engine_style(style), prefix, first })
}

/// The wire name of a style the engine knows.
const fn style_name(style: LabelStyle) -> &'static str {
    match style {
        LabelStyle::Decimal => "decimal",
        LabelStyle::UpperRoman => "upper_roman",
        LabelStyle::LowerRoman => "lower_roman",
        LabelStyle::UpperAlpha => "upper_alpha",
        LabelStyle::LowerAlpha => "lower_alpha",
        LabelStyle::None => "none",
    }
}

/// The engine style a wire name means.
fn engine_style(style: PageLabelStyle) -> LabelStyle {
    match style {
        PageLabelStyle::Decimal => LabelStyle::Decimal,
        PageLabelStyle::UpperRoman => LabelStyle::UpperRoman,
        PageLabelStyle::LowerRoman => LabelStyle::LowerRoman,
        PageLabelStyle::UpperAlpha => LabelStyle::UpperAlpha,
        PageLabelStyle::LowerAlpha => LabelStyle::LowerAlpha,
        PageLabelStyle::None => LabelStyle::None,
    }
}

fn invalid(message: impl Into<String>) -> AdapterError {
    AdapterError::new(ErrorCode::Invalid, message.into(), false)
}

fn limit(message: impl Into<String>) -> AdapterError {
    AdapterError::new(ErrorCode::Limit, message.into(), false)
}

/// The page a label operation starts on, from its target.
fn range_start(target: &Target) -> Result<usize, AdapterError> {
    match target {
        Target::PageLabel { start } => usize::try_from(*start).map_err(|_| invalid("the page index is too large")),
        _ => Err(invalid("a page-label operation needs a page-label target")),
    }
}

/// The style, prefix and first number a `page.label.set` names. Every value is
/// checked here, before the document is touched: a missing or unknown style, a
/// zero first number and a prefix over the label bound are refusals.
fn read_spec(payload: &Value) -> Result<(LabelStyle, String, u32), AdapterError> {
    let name = payload.get("style").and_then(Value::as_str).ok_or_else(|| invalid("the range names no numbering style"))?;
    let style = PageLabelStyle::parse(name).ok_or_else(|| invalid(format!("unknown page-label style {name:?}")))?;
    let prefix = payload.get("prefix").and_then(Value::as_str).unwrap_or_default().to_owned();
    if prefix.len() > MAX_PAGE_LABEL_BYTES {
        return Err(limit(format!("a prefix is {} bytes; one label is at most {MAX_PAGE_LABEL_BYTES}", prefix.len())));
    }
    let first = match payload.get("first") {
        None | Some(Value::Null) => 1,
        Some(value) => value.as_u64().and_then(|n| u32::try_from(n).ok()).ok_or_else(|| invalid("the first number is not a number"))?,
    };
    if first == 0 {
        return Err(invalid("the first number of a range is at least 1"));
    }
    Ok((engine_style(style), prefix, first))
}

impl PdfAdapter {
    /// The label ranges the document has, each with the last page it covers.
    /// Reading is strict on purpose: a tree that is cyclic, too deep or over its
    /// work bound is refused, so the room never rewrites a tree it cannot see
    /// whole. A document with no `/PageLabels` has no ranges.
    fn label_ranges(&self) -> Result<Vec<Range>, AdapterError> {
        let document = self.document().ok_or_else(|| invalid("the document is gone"))?;
        let pages = document.info.pages.len();
        let ranges = document.page_label_ranges().map_err(|error| invalid(error.to_string()))?;
        Ok(ranges
            .iter()
            .enumerate()
            .map(|(index, range)| Range {
                start: range.start,
                end: ranges.get(index + 1).map_or(pages.saturating_sub(1), |next| next.start.saturating_sub(1)),
                style: range.style,
                prefix: range.prefix.clone(),
                first: range.first,
            })
            .collect())
    }

    /// `page.label.set` (RFC §3.1.8): label pages `start..=end` with `style`,
    /// `prefix` and `first`. The span must be inside the one range that currently
    /// covers `start`; a span that would cross another range is `conflict`,
    /// because the undo image is one range, and silently renumbering the other
    /// one would not be an undo.
    pub(crate) fn set_page_label(&mut self, target: &Target, payload: &mut Value) -> Result<Value, AdapterError> {
        let start = range_start(target)?;
        let pages = self.document_or_error()?.info.pages.len();
        let end = payload
            .get("end")
            .and_then(Value::as_u64)
            .and_then(|end| usize::try_from(end).ok())
            .ok_or_else(|| invalid("the range names no last page"))?;
        if start >= pages || end < start || end >= pages {
            return Err(invalid(format!("pages {} to {} are not a range of this {pages}-page document", start + 1, end + 1)));
        }
        let (style, prefix, first) = read_spec(payload)?;
        let ranges = self.label_ranges()?;
        // An accepted undo carries the ranges it replaces, whole (`before` as a
        // list): the set that split a range is undone by one write over the whole
        // span. The list is a compare-and-set — if anyone relabelled a page in
        // the span since, the undo is refused rather than overwriting it.
        if let Some(list) = payload.get("before").and_then(Value::as_array) {
            let expected = list.iter().map(read_range).collect::<Result<Vec<_>, _>>()?;
            let Some(first_range) = expected.first() else { return Err(invalid("the undo names no range to replace")) };
            let last = expected.last().map_or(first_range.end, |range| range.end);
            if first_range.start != start || end != last {
                return Err(invalid("the undo does not match the span its target names"));
            }
            if last >= pages || expected.windows(2).any(|pair| pair[0].end.saturating_add(1) != pair[1].start) {
                return Err(invalid("the undo's ranges are not one contiguous span of this document"));
            }
            let current: Vec<&Range> = ranges.iter().filter(|range| range.start >= start && range.end <= last).collect();
            if current.len() != expected.len() || !current.iter().zip(&expected).all(|(now, then)| now.same(then)) {
                return Err(AdapterError::new(ErrorCode::Conflict, "the page labels changed since the operation was built", false));
            }
            if ranges.len() + 1 - expected.len() > MAX_PAGE_LABEL_RANGES {
                return Err(limit(format!("a room holds at most {MAX_PAGE_LABEL_RANGES} label ranges")));
            }
            if style == LabelStyle::None && prefix.is_empty() {
                return Err(invalid("a label would be empty: a prefix-only range needs a prefix"));
            }
            pdfcraft_organize::check_page_label(&LabelRange { start, style, prefix: prefix.clone(), first }, last)
                .map_err(|error| limit(format!("the label cannot be written: {error}")))?;
            payload["action"] = json!("page.label.set");
            payload["start"] = json!(start);
            payload["end"] = json!(last);
            payload["style"] = json!(style_name(style));
            payload["prefix"] = json!(prefix);
            payload["first"] = json!(first);
            payload["before"] = json!(expected.iter().map(Range::to_value).collect::<Vec<_>>());
            self.apply_edit(Edit::NumberPages { from: start, to: last, style, prefix, first })?;
            self.epoch = self.epoch.saturating_add(1);
            return Ok(json!({ "start": start, "end": last }));
        }
        let covering = ranges.iter().find(|range| range.start <= start && start <= range.end).cloned();
        // A document with no labels shows plain 1, 2, 3: the whole document is
        // one implicit decimal range, and it is the undo image a set replaces.
        let before = covering.clone().unwrap_or_else(|| Range {
            start: 0,
            end: ranges.first().map_or(pages - 1, |range| range.start.saturating_sub(1)),
            style: LabelStyle::Decimal,
            prefix: String::new(),
            first: 1,
        });
        if end > before.end {
            return Err(AdapterError::new(
                ErrorCode::Conflict,
                format!("the range crosses the label range that begins at page {}", before.end.saturating_add(2)),
                false,
            ));
        }
        let replaces = covering.as_ref().is_some_and(|range| range.start == start);
        let mut count = ranges.len();
        if !replaces {
            count += 1;
        }
        if end < before.end {
            count += 1;
        }
        if count > MAX_PAGE_LABEL_RANGES {
            return Err(limit(format!("a room holds at most {MAX_PAGE_LABEL_RANGES} label ranges")));
        }
        if style == LabelStyle::None && prefix.is_empty() {
            return Err(invalid("a label would be empty: a prefix-only range needs a prefix"));
        }
        pdfcraft_organize::check_page_label(&LabelRange { start, style, prefix: prefix.clone(), first }, end)
            .map_err(|error| limit(format!("the label cannot be written: {error}")))?;
        payload["action"] = json!("page.label.set");
        payload["start"] = json!(start);
        payload["end"] = json!(end);
        payload["style"] = json!(style_name(style));
        payload["prefix"] = json!(prefix);
        payload["first"] = json!(first);
        // A document with no labels shows plain numbers: the undo image is the
        // implicit whole-document range. It is marked, because the pages before
        // a set inside it are unlabelled, not covered by a range.
        let mut before_value = before.to_value();
        if covering.is_none() {
            before_value["implicit"] = json!(true);
        }
        payload["before"] = before_value;
        self.apply_edit(Edit::NumberPages { from: start, to: end, style, prefix, first })?;
        self.epoch = self.epoch.saturating_add(1);
        Ok(json!({ "start": start, "end": end }))
    }

    /// `page.label.clear`: remove the label range that begins at `start`. The
    /// pages it labelled take the numbering of the range before them; a page
    /// that begins no range is `invalid`, and nothing changes.
    pub(crate) fn clear_page_label(&mut self, target: &Target, payload: &mut Value) -> Result<Value, AdapterError> {
        let start = range_start(target)?;
        let ranges = self.label_ranges()?;
        let Some(range) = ranges.into_iter().find(|range| range.start == start) else {
            return Err(invalid(format!("no label range begins at page {}", start.saturating_add(1))));
        };
        payload["action"] = json!("page.label.clear");
        payload["start"] = json!(start);
        payload["before"] = range.to_value();
        self.apply_edit(Edit::ClearPageLabels { start })?;
        self.epoch = self.epoch.saturating_add(1);
        Ok(json!({ "start": start }))
    }
}
