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
    Date,
    Type,
}

impl SummarySort {
    pub const ALL: [SummarySort; 4] = [SummarySort::Page, SummarySort::Author, SummarySort::Date, SummarySort::Type];
    pub fn name(self) -> &'static str {
        match self {
            SummarySort::Page => "Page",
            SummarySort::Author => "Author",
            SummarySort::Date => "Date",
            SummarySort::Type => "Type",
        }
    }
    pub fn parse(s: &str) -> Option<SummarySort> {
        Self::ALL.into_iter().find(|k| k.name().eq_ignore_ascii_case(s))
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

/// Layout of Summarize Comments (Acrobat's Summarize Options).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum SummaryLayout {
    /// A list of the comments, grouped by page.
    #[default]
    CommentsOnly,
    /// Each page beside its comments, with a connector line from each comment to its entry.
    DocumentAndCommentsWithLines,
    /// Each page beside its comments, with each comment's number marked on the page.
    DocumentAndComments,
}

impl SummaryLayout {
    pub const ALL: [SummaryLayout; 3] =
        [SummaryLayout::CommentsOnly, SummaryLayout::DocumentAndCommentsWithLines, SummaryLayout::DocumentAndComments];
    /// The name Summarize Options shows.
    pub fn name(self) -> &'static str {
        match self {
            SummaryLayout::CommentsOnly => "Comments only",
            SummaryLayout::DocumentAndCommentsWithLines => "Document and comments with connector lines on single pages",
            SummaryLayout::DocumentAndComments => "Document and comments on single pages",
        }
    }
    /// The name `comments_summarize` takes.
    pub fn key(self) -> &'static str {
        match self {
            SummaryLayout::CommentsOnly => "comments_only",
            SummaryLayout::DocumentAndCommentsWithLines => "document_and_comments_with_lines",
            SummaryLayout::DocumentAndComments => "document_and_comments",
        }
    }
    pub fn parse(s: &str) -> Option<SummaryLayout> {
        Self::ALL.into_iter().find(|k| k.key().eq_ignore_ascii_case(s))
    }
}

/// One comment of a summary: its number on its page, and its replies (each with its depth,
/// 1 for a direct reply) in the order they are written.
pub struct SummaryItem<'a> {
    pub number: usize,
    pub comment: &'a pdfcraft_render::Annotation,
    pub replies: Vec<(usize, &'a pdfcraft_render::Annotation)>,
}

/// The comments a summary lists, in `sort` order. Checkmarks and status replies are left out,
/// as are pop-ups; the number is the comment's position on its page.
pub fn summary_items(all: &[pdfcraft_render::Annotation], sort: SummarySort) -> Vec<SummaryItem<'_>> {
    // Each reply under the annotation its `/IRT` points at, in document order, gathered once.
    type Children<'a> = std::collections::HashMap<(usize, usize), Vec<&'a pdfcraft_render::Annotation>>;
    let mut children = Children::new();
    for r in all.iter().filter(|r| r.state.is_none()) {
        if let Some(parent) = r.parent {
            children.entry(parent).or_default().push(r);
        }
    }
    // A comment's replies, a level deeper each. A reply to a reply keeps its `/IRT` on the
    // direct parent, so following one level would leave the deeper replies out of the summary.
    // A group subordinate (`/RT /Group`, ISO 32000-2 §12.5.6.2) is part of the comment, not a
    // reply: it is not listed, and replies to it are listed at its level, as the comment's.
    // `depth` bounds the walk; `level` is the reply's depth.
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
        SummarySort::Date => items.sort_by(|x, y| x.comment.modified.cmp(&y.comment.modified)),
        SummarySort::Type => items.sort_by(|x, y| comment_kind(x.comment).cmp(comment_kind(y.comment))),
    }
    items
}

/// The text of a comment summary: one block per comment (replies indented under it), with a
/// "Page N" heading when sorted by page. Checkmarks and status replies are left out, as are
/// pop-ups; the number is the comment's position on its page.
pub fn comment_summary(name: &str, all: &[pdfcraft_render::Annotation], sort: SummarySort) -> String {
    use std::fmt::Write;
    let items = summary_items(all, sort);
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

/// The comments of a summary that shows each page beside them, for `pdfcraft_print::summary_with_pages`:
/// grouped by page, in `sort` order within each page.
pub fn summary_comments(all: &[pdfcraft_render::Annotation], sort: SummarySort) -> Vec<pdfcraft_print::SummaryComment> {
    let mut items = summary_items(all, sort);
    // Stable: the sort order holds within each page.
    items.sort_by_key(|i| i.comment.page);
    items
        .into_iter()
        .map(|item| {
            let a = item.comment;
            let mut lines = vec![(
                0,
                format!("{}. {}  {}  {}", item.number, comment_kind(a), a.author.as_deref().unwrap_or(""), a.modified.as_deref().unwrap_or(""))
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
                number: item.number,
                index: Some(a.index),
                rect: a.rect.map(f64::from),
                color: a.color.map(|c| c.map(f64::from)),
                lines,
            }
        })
        .collect()
}
