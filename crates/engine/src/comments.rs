//! The comment list and its summary.

/// The comment list, read from the object graph (as `inspect` lists comments).
pub(crate) fn comment_list(doc: &pdfcraft_cos::Document) -> Vec<pdfcraft_render::Annotation> {
    let mut list: Vec<pdfcraft_render::Annotation> = pdfcraft_annot::summaries(doc)
        .into_iter()
        .map(|s| pdfcraft_render::Annotation {
            page: s.page,
            subtype: s.subtype,
            author: s.author,
            contents: s.contents,
            modified: s.modified.map(|m| pdfcraft_render::pretty_date(&m)),
            name: s.name,
            in_reply_to: s.in_reply_to,
            parent: s.parent,
            grouped: s.grouped,
            rect: s.rect,
            color: s.color,
            index: s.index,
            state: s.state,
            quads: s.quads,
            locked: s.locked,
            intent: s.intent,
            fill_sign: s.fill_sign,
            no_rotate: s.no_rotate,
            points: s.points,
            text_box: s.text_box,
            popup: s.popup.map(|p| pdfcraft_render::Popup { rect: p.rect, open: p.open }),
        })
        .collect();
    pdfcraft_render::link_replies(&mut list);
    pdfcraft_render::fill_group_text(&mut list);
    list
}

/// Order of Summarize Comments.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum SummarySort {
    #[default]
    Page,
    Author,
    /// Oldest first; the layouts that show the pages put first the page commented on first.
    Date,
    /// Oldest first, the pages by their last (newest) comment: the page commented on most
    /// recently comes last.
    LastDate,
    Type,
}

impl SummarySort {
    pub const ALL: [SummarySort; 5] = [SummarySort::Page, SummarySort::Author, SummarySort::Date, SummarySort::LastDate, SummarySort::Type];
    /// The name shown in the dialog, also accepted by the tools and saved in the preferences.
    pub fn name(self) -> &'static str {
        match self {
            SummarySort::Page => "Page",
            SummarySort::Author => "Author",
            SummarySort::Date => "Date",
            SummarySort::LastDate => "Date of last comment",
            SummarySort::Type => "Type",
        }
    }
    /// The sort order a [`name`](Self::name) stands for, ignoring case, with `_` for a space
    /// (`date_of_last_comment`); `None` for any other text.
    pub fn parse(s: &str) -> Option<SummarySort> {
        let s = s.replace('_', " ");
        Self::ALL.into_iter().find(|k| k.name().eq_ignore_ascii_case(&s))
    }
    /// It sorts by the comments' dates, which are then read in full ([`summary_dates`]).
    pub fn by_date(self) -> bool {
        matches!(self, SummarySort::Date | SummarySort::LastDate)
    }
}

/// Acrobat's name for a comment type, as comment lists show it.
pub fn comment_type_name(subtype: &str) -> &str {
    match subtype {
        "Text" => "Sticky Note",
        "FreeText" => "Text Box",
        "Highlight" => "Highlight",
        "Underline" => "Underline",
        "StrikeOut" => "Strikethrough",
        "Squiggly" => "Squiggly",
        "Square" => "Rectangle",
        "Circle" => "Oval",
        "Line" => "Line",
        "Ink" => "Pencil",
        "Polygon" => "Polygon",
        "PolyLine" => "Polygonal Line",
        "Stamp" => "Stamp",
        "Caret" => "Inserted Text",
        "FileAttachment" => "File Attachment",
        "Sound" => "Sound",
        "Redact" => "Redaction",
        other => other,
    }
}

/// [`comment_type_name`], telling replaced text, callouts, clouds and typewriter text apart by `/IT`.
pub fn comment_kind(a: &pdfcraft_render::Annotation) -> &str {
    match a.intent.as_deref() {
        Some("StrikeOutTextEdit" | "Replace") => "Replace Text",
        Some("FreehandHighlight") if a.subtype == "Ink" => "Freehand Highlight",
        Some("FreeTextCallout") => "Callout",
        Some("PolygonCloud") => "Cloud",
        Some("FreeTextTypeWriter") => "Typewriter",
        _ => comment_type_name(&a.subtype),
    }
}

/// Layout of Summarize Comments (Acrobat's Summarize Options), in the order the dialog lists
/// them.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum SummaryLayout {
    /// Each page beside its comments, with a connector line from each comment to its entry.
    DocumentAndCommentsWithLines,
    /// A list of the comments, grouped by page.
    #[default]
    CommentsOnly,
    /// Each page on a sheet of its own with its comments' numbers on it, then its comments.
    DocumentAndCommentsWithNumbers,
}

impl SummaryLayout {
    pub const ALL: [SummaryLayout; 3] =
        [SummaryLayout::DocumentAndCommentsWithLines, SummaryLayout::CommentsOnly, SummaryLayout::DocumentAndCommentsWithNumbers];
    /// The name Summarize Options shows.
    pub fn name(self) -> &'static str {
        match self {
            SummaryLayout::DocumentAndCommentsWithLines => "Document and comments with connector lines on single pages",
            SummaryLayout::CommentsOnly => "Comments only",
            SummaryLayout::DocumentAndCommentsWithNumbers => "Document and comments with sequence numbers on separate pages",
        }
    }
    /// The name `comments_summarize` and the settings file use.
    pub fn key(self) -> &'static str {
        match self {
            SummaryLayout::DocumentAndCommentsWithLines => "document_and_comments_with_lines",
            SummaryLayout::CommentsOnly => "comments_only",
            SummaryLayout::DocumentAndCommentsWithNumbers => "document_and_comments_with_numbers",
        }
    }
    /// The layout a [`key`](Self::key) stands for, ignoring case; `None` for any other text.
    pub fn parse(s: &str) -> Option<SummaryLayout> {
        Self::ALL.into_iter().find(|k| k.key().eq_ignore_ascii_case(s))
    }
    /// The layout shows the pages, so pages without comments can be left in or out.
    pub fn shows_pages(self) -> bool {
        self != SummaryLayout::CommentsOnly
    }
}

/// The papers Summarize Options offers, by their names in `pdfcraft_print::PAPERS`.
pub const SUMMARY_PAPERS: [&str; 6] = ["US Letter", "US Legal", "Tabloid", "A3", "A4", "A5"];

/// What Summarize Comments makes.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SummarizeOptions {
    pub sort: SummarySort,
    pub layout: SummaryLayout,
    /// The paper (width, height in points); `None` takes the paper the document's first page is
    /// printed on ([`summary_paper`]).
    pub paper: Option<(f64, f64)>,
    /// Pages without comments are shown too (layouts that show the pages).
    pub pages_without_comments: bool,
}

impl Default for SummarizeOptions {
    fn default() -> Self {
        Self { sort: SummarySort::Page, layout: SummaryLayout::CommentsOnly, paper: None, pages_without_comments: true }
    }
}

/// The paper a summary of a document whose first page is `first` (display size, points) is made
/// on when none is chosen: the offered paper ([`SUMMARY_PAPERS`]) that page is (A4 for an A4
/// page, either way round), or US Letter for any other page: a poster's A0 or a label's 4 × 6 in
/// would make the comments' text too small or their column too narrow.
pub fn summary_paper(first: Option<(f64, f64)>) -> (&'static str, (f64, f64)) {
    first
        .and_then(pdfcraft_print::matching_paper)
        .and_then(|i| pdfcraft_print::PAPERS.get(i).copied())
        .filter(|(name, _)| SUMMARY_PAPERS.contains(name))
        .unwrap_or(("US Letter", (612.0, 792.0)))
}

/// Each comment's date (`/M`) by `(page, index)`, read in full, for sorting a summary by date:
/// the dates comment lists show are cut to the minute and leave out the time zone.
pub type SummaryDates = std::collections::HashMap<(usize, usize), chrono::DateTime<chrono::Utc>>;

/// The dates of document `doc`'s comments, for [`summary_items`].
pub fn summary_dates(doc: &pdfcraft_cos::Document) -> SummaryDates {
    pdfcraft_annot::summaries(doc).into_iter().filter_map(|s| Some(((s.page, s.index), pdf_date(s.modified.as_deref()?)?))).collect()
}

/// A PDF date (`D:YYYYMMDDHHmmSSOHH'mm'`, ISO 32000-2 §7.9.4) as a moment, the fields after the
/// year optional; `None` when it can't be read. Without a time zone it is taken as UTC.
pub(crate) fn pdf_date(s: &str) -> Option<chrono::DateTime<chrono::Utc>> {
    use chrono::TimeZone as _;
    let s = s.trim();
    let s = s.strip_prefix("D:").unwrap_or(s);
    let digits = s.bytes().take_while(u8::is_ascii_digit).count();
    // ASCII digits, so every slice below is on a character boundary.
    let field = |from: usize, len: usize, default: u32| -> Option<u32> {
        if digits >= from + len { s.get(from..from + len)?.parse().ok() } else { Some(default) }
    };
    if digits < 4 {
        return None;
    }
    let year: i32 = s.get(0..4)?.parse().ok()?;
    let date = chrono::NaiveDate::from_ymd_opt(year, field(4, 2, 1)?, field(6, 2, 1)?)?;
    let time = date.and_hms_opt(field(8, 2, 0)?, field(10, 2, 0)?, field(12, 2, 0)?.min(59))?;
    let rest = s.get(digits.min(14)..).unwrap_or("");
    // After the digits only a time zone may follow (`Z`, or `+HH'mm'`); anything else (an ISO
    // date's `-07-04T10:00`, say) is not a PDF date, rather than a date in some made-up zone.
    let zone = match rest.as_bytes() {
        [] => true,
        [b'Z' | b'z', tail @ ..] => tail.iter().all(|b| b.is_ascii_digit() || *b == b'\''),
        [b'+' | b'-', h, m, tail @ ..] => h.is_ascii_digit() && m.is_ascii_digit() && tail.iter().all(|b| b.is_ascii_digit() || *b == b'\''),
        _ => false,
    };
    if !zone {
        return None;
    }
    let offset = match rest.as_bytes().first() {
        Some(sign @ (b'+' | b'-')) => {
            let nums: Vec<i32> =
                rest.get(1..).unwrap_or("").split(|c: char| !c.is_ascii_digit()).filter(|p| !p.is_empty()).filter_map(|p| p.parse().ok()).collect();
            let secs = nums.first().copied().unwrap_or(0).clamp(0, 23) * 3600 + nums.get(1).copied().unwrap_or(0).clamp(0, 59) * 60;
            chrono::FixedOffset::east_opt(if *sign == b'-' { -secs } else { secs })?
        }
        _ => chrono::FixedOffset::east_opt(0)?,
    };
    Some(offset.from_local_datetime(&time).single()?.with_timezone(&chrono::Utc))
}

/// One comment of a summary: its number on its page, and its replies (each with its depth,
/// 1 for a direct reply) in the order they are written.
pub struct SummaryItem<'a> {
    pub number: usize,
    pub comment: &'a pdfcraft_render::Annotation,
    pub replies: Vec<(usize, &'a pdfcraft_render::Annotation)>,
}

/// The comments a summary lists, in `sort` order. Checkmarks and status replies are left out,
/// as are pop-ups; the number is the comment's position on its page. By date, oldest first, by
/// the full date in `dates` where it has one, then those with only the date as listed; undated
/// comments last.
pub fn summary_items<'a>(all: &'a [pdfcraft_render::Annotation], sort: SummarySort, dates: &SummaryDates) -> Vec<SummaryItem<'a>> {
    // Each reply under the annotation its `/IRT` points at, in document order, gathered once.
    type Children<'a> = std::collections::HashMap<(usize, usize), Vec<&'a pdfcraft_render::Annotation>>;
    let mut children = Children::new();
    for r in all.iter().filter(|r| r.state.is_none()) {
        if let Some(parent) = r.parent {
            children.entry(parent).or_default().push(r);
        }
    }
    /// A comment's replies, a level deeper each. A reply to a reply keeps its `/IRT` on the
    /// direct parent, so following one level would leave the deeper replies out of the summary.
    /// A group subordinate (`/RT /Group`, ISO 32000-2 §12.5.6.2) is part of the comment, not a
    /// reply: it is not listed, and replies to it are listed at its level, as the comment's.
    /// `depth` bounds the walk; `level` is the reply's depth.
    fn replies<'a>(
        out: &mut Vec<(usize, &'a pdfcraft_render::Annotation)>,
        children: &Children<'a>,
        parent: &pdfcraft_render::Annotation,
        depth: usize,
        level: usize,
    ) {
        if depth > 64 {
            return;
        }
        for &r in children.get(&(parent.page, parent.index)).into_iter().flatten() {
            if r.is_group_member() {
                replies(out, children, r, depth + 1, level);
                continue;
            }
            out.push((level + 1, r));
            replies(out, children, r, depth + 1, level + 1);
        }
    }
    let mut items: Vec<SummaryItem<'_>> = Vec::new();
    let mut last = usize::MAX;
    let mut n = 0;
    for a in all.iter().filter(|a| a.is_comment()) {
        if a.page != last {
            last = a.page;
            n = 0;
        }
        n += 1;
        let mut r = Vec::new();
        replies(&mut r, &children, a, 0, 0);
        items.push(SummaryItem { number: n, comment: a, replies: r });
    }
    match sort {
        SummarySort::Page => {}
        SummarySort::Author => items.sort_by(|x, y| x.comment.author.cmp(&y.comment.author)),
        SummarySort::Date | SummarySort::LastDate => {
            // Fully dated first, then those with only the date as listed (one that can't be read
            // in full), then the undated.
            let rank = |a: &pdfcraft_render::Annotation| match (dates.contains_key(&(a.page, a.index)), a.modified.is_some()) {
                (true, _) => 0u8,
                (false, true) => 1,
                (false, false) => 2,
            };
            let when = |a: &'a pdfcraft_render::Annotation| (rank(a), dates.get(&(a.page, a.index)).copied(), a.modified.as_deref());
            items.sort_by(|x, y| when(x.comment).cmp(&when(y.comment)));
            if sort == SummarySort::LastDate {
                // Each page where its last dated comment comes, each page's comments kept oldest
                // first; a page with only undated comments last, pages in document order on a tie.
                let mut last = std::collections::HashMap::new();
                for item in items.iter().filter(|i| rank(i.comment) < 2) {
                    last.insert(item.comment.page, when(item.comment));
                }
                items.sort_by_key(|i| (last.get(&i.comment.page).copied().unwrap_or((2, None, None)), i.comment.page));
            }
        }
        SummarySort::Type => items.sort_by(|x, y| comment_kind(x.comment).cmp(comment_kind(y.comment))),
    }
    items
}

/// The text of a comment summary: one block per comment (replies indented under it), with a
/// "Page N" heading when sorted by page. Checkmarks and status replies are left out, as are
/// pop-ups; the number is the comment's position on its page.
pub fn comment_summary(name: &str, all: &[pdfcraft_render::Annotation], sort: SummarySort) -> String {
    comment_summary_with(name, all, sort, &SummaryDates::new())
}

/// [`comment_summary`], sorting by the full dates in `dates` ([`summary_items`]).
pub fn comment_summary_with(name: &str, all: &[pdfcraft_render::Annotation], sort: SummarySort, dates: &SummaryDates) -> String {
    use std::fmt::Write;
    let items = summary_items(all, sort, dates);
    let mut out = format!("Summary of Comments on {name}\n\n");
    if items.is_empty() {
        out.push_str("This document has no comments.\n");
        return out;
    }
    let mut heading = usize::MAX;
    for item in items {
        let a = item.comment;
        if sort == SummarySort::Page && a.page != heading {
            heading = a.page;
            let _ = writeln!(out, "Page: {}", a.page + 1);
        }
        let _ = write!(out, "Number: {}  Author: {}  Subject: {}", item.number, a.author.as_deref().unwrap_or(""), comment_kind(a));
        if sort != SummarySort::Page {
            let _ = write!(out, "  Page: {}", a.page + 1);
        }
        let _ = writeln!(out, "  Date: {}", a.modified.as_deref().unwrap_or(""));
        if let Some(c) = a.contents.as_deref().filter(|c| !c.is_empty()) {
            let _ = writeln!(out, "{c}");
        }
        for (level, r) in item.replies {
            let indent = "    ".repeat(level);
            let _ =
                writeln!(out, "{indent}Author: {}  Subject: Reply  Date: {}", r.author.as_deref().unwrap_or(""), r.modified.as_deref().unwrap_or(""));
            for line in r.contents.as_deref().unwrap_or("").lines() {
                let _ = writeln!(out, "{indent}{line}");
            }
        }
        out.push('\n');
    }
    out
}

/// The comments of a summary that shows the pages, for `pdfcraft_print::summary_with_pages`, in
/// `sort` order and grouped by page: the pages come in the order of their first comment (by
/// date, the page commented on first leads), each page's comments in `sort` order, numbered
/// 1, 2, 3 on each page in that order.
pub fn summary_comments(all: &[pdfcraft_render::Annotation], sort: SummarySort, dates: &SummaryDates) -> Vec<pdfcraft_print::SummaryComment> {
    let items = summary_items(all, sort, dates);
    // Each page's place: where its first comment comes. A stable sort by it keeps the sort
    // order within each page.
    let mut place = std::collections::HashMap::new();
    for item in &items {
        let next = place.len();
        place.entry(item.comment.page).or_insert(next);
    }
    let mut items: Vec<_> = items.into_iter().map(|item| (place.get(&item.comment.page).copied().unwrap_or(usize::MAX), item)).collect();
    items.sort_by_key(|(at, _)| *at);
    let mut last = usize::MAX;
    let mut number = 0;
    items
        .into_iter()
        .map(|(_, item)| {
            if item.comment.page != last {
                (last, number) = (item.comment.page, 0);
            }
            number += 1;
            let a = item.comment;
            let mut lines = vec![(
                0,
                format!("{number}. {}  {}  {}", comment_kind(a), a.author.as_deref().unwrap_or(""), a.modified.as_deref().unwrap_or(""))
                    .trim_end()
                    .to_string(),
            )];
            if let Some(c) = a.contents.as_deref().filter(|c| !c.is_empty()) {
                lines.push((0, c.to_string()));
            }
            for (level, r) in item.replies {
                lines.push((
                    level,
                    format!("Reply  {}  {}", r.author.as_deref().unwrap_or(""), r.modified.as_deref().unwrap_or("")).trim_end().to_string(),
                ));
                if let Some(c) = r.contents.as_deref().filter(|c| !c.is_empty()) {
                    lines.push((level, c.to_string()));
                }
            }
            pdfcraft_print::SummaryComment {
                page: a.page,
                number,
                index: Some(a.index),
                rect: a.rect.map(f64::from),
                color: a.color.map(|c| c.map(f64::from)),
                lines,
            }
        })
        .collect()
}
