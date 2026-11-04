use super::as_html::allowed;
use super::layout::{Cell, Item, MAX_COLS, Page, items};

/// Markdown's deepest heading.
const MAX_HEADING: u8 = 6;

/// `s` as Markdown text on one line: line breaks become spaces, control characters are dropped,
/// and every character Markdown would read as markup is escaped with a backslash.
fn md_text(s: &str) -> String {
    let mut o = String::with_capacity(s.len());
    for c in s.chars().filter(|c| allowed(*c)) {
        match c {
            '\n' | '\r' | '\t' => o.push(' '),
            '\\' | '`' | '*' | '_' | '[' | ']' | '<' | '>' | '#' | '|' | '~' | '!' | '&' => {
                o.push('\\');
                o.push(c);
            }
            c => o.push(c),
        }
    }
    o
}

/// `line` with its start escaped where Markdown would read a list item there (`- `, `+ `,
/// `1. `, `1) `); the other block markers (`#`, `>`) are escaped by [`md_text`] already.
fn block_start(line: String) -> String {
    let digits = line.bytes().take_while(u8::is_ascii_digit).count();
    match line.as_bytes().get(digits) {
        Some(b'.' | b')') if digits > 0 => format!("{}\\{}", line.get(..digits).unwrap_or_default(), line.get(digits..).unwrap_or_default()),
        Some(b'-' | b'+') if digits == 0 => format!("\\{line}"),
        _ => line,
    }
}

/// `text` (already escaped) in bold and/or italic, its outer spaces kept outside. `open` and
/// `close` say whether `*` markers would be read as emphasis on that side (see [`marks`]);
/// where they would not, as in `foo` + bold `.` + `bar`, the run is wrapped in HTML tags, which
/// CommonMark takes inline. The text's own outer spaces count as a side that marks.
pub(crate) fn emphasis(text: &str, bold: bool, italic: bool, open: bool, close: bool) -> String {
    let core = text.trim();
    if core.is_empty() || !(bold || italic) {
        return text.to_string();
    }
    let lead = text.get(..text.len().saturating_sub(text.trim_start().len())).unwrap_or_default();
    let trail = text.get(text.trim_end().len()..).unwrap_or_default();
    let markers = (open || !lead.is_empty()) && (close || !trail.is_empty());
    let (before, after) = match (markers, bold, italic) {
        (true, true, true) => ("***", "***"),
        (true, true, false) => ("**", "**"),
        (true, false, _) => ("*", "*"),
        (false, true, true) => ("<strong><em>", "</em></strong>"),
        (false, true, false) => ("<strong>", "</strong>"),
        (false, false, _) => ("<em>", "</em>"),
    };
    format!("{lead}{before}{core}{after}{trail}")
}

/// A paragraph's runs (text, bold, italic) as one line of Markdown. Neighbouring runs in the
/// same style are joined first, so two bold runs never meet as `****`.
pub(crate) fn paragraph(runs: &[(&str, bool, bool)]) -> String {
    let mut merged: Vec<(String, bool, bool)> = Vec::new();
    for &(text, bold, italic) in runs {
        match merged.last_mut() {
            Some(last) if last.1 == bold && last.2 == italic => last.0.push_str(text),
            _ => merged.push((text.to_string(), bold, italic)),
        }
    }
    let texts: Vec<String> = merged.iter().map(|(text, ..)| md_text(text)).collect();
    let mut out = String::new();
    for (k, ((_, bold, italic), text)) in merged.iter().zip(&texts).enumerate() {
        let core = text.trim();
        let open = marks(core.chars().next(), out.chars().last());
        // Markers of a styled neighbour right next to these would join them into one run.
        let close = match (merged.get(k.saturating_add(1)), texts.get(k.saturating_add(1))) {
            (Some((_, b, i)), Some(next)) if (*b || *i) && !next.starts_with(char::is_whitespace) => false,
            (_, next) => marks(core.chars().last(), next.and_then(|n| n.chars().next())),
        };
        out.push_str(&emphasis(text, *bold, *italic, open, close));
    }
    out
}

/// Whether a `*` marker between the text's `edge` character and the `outside` one is read as
/// emphasis (CommonMark's flanking rules for `*`): always next to a space or the line's edge,
/// never next to another `*`, and otherwise unless the text's edge is punctuation while the
/// outside is a letter or digit.
fn marks(edge: Option<char>, outside: Option<char>) -> bool {
    match (edge, outside) {
        (_, None) => true,
        (_, Some(o)) if o.is_whitespace() => true,
        (_, Some('*')) => false,
        (Some(e), Some(o)) => e.is_alphanumeric() || o.is_ascii_punctuation(),
        (None, Some(_)) => false,
    }
}

fn cell(c: &Cell) -> String {
    // A cell sits between `| ` and ` |`, so its markers always have a space outside.
    emphasis(&md_text(&c.text), c.bold, c.italic, true, true)
}

/// Markdown (CommonMark, with GitHub's pipe tables): headings, paragraphs with bold and italic,
/// tables with their first row as the header, and a rule between pages. Images are left out.
pub fn markdown(pages: &[Page]) -> String {
    let mut blocks: Vec<String> = Vec::new();
    for it in items(pages) {
        match it {
            Item::Para(runs, lvl) => {
                // A heading's weight is the heading's own, as in the HTML export.
                let runs: Vec<(&str, bool, bool)> = runs.iter().map(|r| (r.text.as_str(), r.block.bold && lvl == 0, r.block.italic)).collect();
                let text = paragraph(&runs);
                let text = text.trim();
                if text.is_empty() {
                    continue;
                }
                blocks.push(match lvl {
                    0 => block_start(text.to_string()),
                    l => format!("{} {text}", "#".repeat(usize::from(l.min(MAX_HEADING)))),
                });
            }
            Item::Table(t) => {
                // Each row as wide as the widest; a cell spanning columns is followed by empty ones.
                // Detected tables have at most MAX_COLS columns, and a span never counts for more.
                let span = |c: &Cell| c.span.clamp(1, MAX_COLS);
                let width = t.rows.iter().map(|r| r.iter().map(span).fold(0usize, usize::saturating_add)).max().unwrap_or(0);
                if width == 0 {
                    continue;
                }
                let mut lines = Vec::with_capacity(t.rows.len().saturating_add(1));
                for (i, row) in t.rows.iter().enumerate() {
                    let mut cells: Vec<String> = Vec::with_capacity(width);
                    for c in row {
                        cells.push(cell(c));
                        cells.extend(std::iter::repeat_n(String::new(), span(c).saturating_sub(1)));
                    }
                    cells.resize(width, String::new());
                    lines.push(format!("| {} |", cells.join(" | ")));
                    if i == 0 {
                        lines.push(format!("|{}", " --- |".repeat(width)));
                    }
                }
                blocks.push(lines.join("\n"));
            }
            Item::Img(_) => {}
            Item::PageBreak => blocks.push("---".into()),
        }
    }
    let mut s = blocks.join("\n\n");
    s.push('\n');
    s
}
