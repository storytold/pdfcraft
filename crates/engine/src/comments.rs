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
        })
        .collect();
    pdfcraft_render::link_replies(&mut list);
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

/// [`comment_type_name`], telling callouts, clouds and typewriter text apart by `/IT`.
pub fn comment_kind(a: &pdfcraft_render::Annotation) -> &str {
    match a.intent.as_deref() {
        Some("FreeTextCallout") => "Callout",
        Some("PolygonCloud") => "Cloud",
        Some("FreeTextTypeWriter") => "Typewriter",
        _ => comment_type_name(&a.subtype),
    }
}

/// The text of a comment summary: one block per comment (replies indented under it), with a
/// "Page N" heading when sorted by page. Checkmarks and status replies are left out, as are
/// pop-ups; the number is the comment's position on its page.
pub fn comment_summary(name: &str, all: &[pdfcraft_render::Annotation], sort: SummarySort) -> String {
    use std::fmt::Write;
    let top: Vec<&pdfcraft_render::Annotation> = all.iter().filter(|a| a.is_comment()).collect();
    // Each reply under the annotation its `/IRT` points at, in document order, gathered once.
    type Children<'a> = std::collections::HashMap<(usize, usize), Vec<&'a pdfcraft_render::Annotation>>;
    let mut children = Children::new();
    for r in all.iter().filter(|r| r.state.is_none()) {
        if let Some(parent) = r.parent {
            children.entry(parent).or_default().push(r);
        }
    }
    // Write a comment's replies, indenting each level. A reply to a reply keeps its `/IRT` on the
    // direct parent, so following one level would leave the deeper replies out of the summary.
    fn write_replies(out: &mut String, children: &Children<'_>, parent: &pdfcraft_render::Annotation, depth: usize) {
        use std::fmt::Write;
        if depth > 64 {
            return;
        }
        for r in children.get(&(parent.page, parent.index)).into_iter().flatten() {
            let indent = "    ".repeat(depth + 1);
            let _ =
                writeln!(out, "{indent}Author: {}  Subject: Reply  Date: {}", r.author.as_deref().unwrap_or(""), r.modified.as_deref().unwrap_or(""));
            for line in r.contents.as_deref().unwrap_or("").lines() {
                let _ = writeln!(out, "{indent}{line}");
            }
            write_replies(out, children, r, depth + 1);
        }
    }
    let mut numbered: Vec<(usize, &pdfcraft_render::Annotation)> = Vec::new();
    let mut last = usize::MAX;
    let mut n = 0;
    for a in &top {
        if a.page != last {
            last = a.page;
            n = 0;
        }
        n += 1;
        numbered.push((n, a));
    }
    match sort {
        SummarySort::Page => {}
        SummarySort::Author => numbered.sort_by(|x, y| x.1.author.cmp(&y.1.author)),
        SummarySort::Date => numbered.sort_by(|x, y| x.1.modified.cmp(&y.1.modified)),
        SummarySort::Type => numbered.sort_by(|x, y| comment_kind(x.1).cmp(comment_kind(y.1))),
    }
    let mut out = format!("Summary of Comments on {name}\n\n");
    if numbered.is_empty() {
        out.push_str("This document has no comments.\n");
        return out;
    }
    let mut heading = usize::MAX;
    for (n, a) in numbered {
        if sort == SummarySort::Page && a.page != heading {
            heading = a.page;
            let _ = writeln!(out, "Page: {}", a.page + 1);
        }
        let _ = write!(out, "Number: {n}  Author: {}  Subject: {}", a.author.as_deref().unwrap_or(""), comment_kind(a));
        if sort != SummarySort::Page {
            let _ = write!(out, "  Page: {}", a.page + 1);
        }
        let _ = writeln!(out, "  Date: {}", a.modified.as_deref().unwrap_or(""));
        if let Some(c) = a.contents.as_deref().filter(|c| !c.is_empty()) {
            let _ = writeln!(out, "{c}");
        }
        write_replies(&mut out, &children, a, 0);
        out.push('\n');
    }
    out
}
