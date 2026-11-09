use super::as_docx::{hex, rgb8};
use super::layout::{Item, Page, Run, Table, items};

/// Whether a character may appear in the output. XML 1.0 forbids the C0 controls other than tab,
/// line feed and carriage return, and U+FFFE/U+FFFF. Text extracted from PDFs often holds them
/// (fonts without a usable `/ToUnicode`), and Word refuses a whole document over one (#72).
pub(crate) fn allowed(c: char) -> bool {
    matches!(c, '\t' | '\n' | '\r') || !(c.is_ascii_control() || c == '\u{FFFE}' || c == '\u{FFFF}')
}

/// The longest language tag the exports write. BCP 47 sets no maximum (35 characters is only
/// the least a protocol must hold), so this bounds only what a PDF's `/Lang` can put in a file.
const LONGEST_TAG: usize = 255;

/// `lang` when it is a well formed language tag (BCP 47, as a PDF's `/Lang` holds one): a primary
/// language of two or three letters, then subtags of one to eight letters or digits, joined by
/// hyphens, [`LONGEST_TAG`] characters at most. A singleton (one letter or digit) starts an
/// extension, or with `x` private use, and needs a subtag after it. Anything else names no
/// language.
pub(crate) fn language_tag(lang: Option<&str>) -> Option<&str> {
    let tag = lang?.trim();
    let mut subtags = tag.split('-');
    let primary = subtags.next()?;
    if tag.len() > LONGEST_TAG || !(2..=3).contains(&primary.len()) || !primary.bytes().all(|b| b.is_ascii_alphabetic()) {
        return None;
    }
    // In private use every subtag is data, one character long or not.
    let (mut private, mut singleton) = (false, false);
    for s in subtags {
        if !(1..=8).contains(&s.len()) || !s.bytes().all(|b| b.is_ascii_alphanumeric()) {
            return None;
        }
        if s.len() == 1 && !private {
            if singleton {
                return None;
            }
            private = s.eq_ignore_ascii_case("x");
            singleton = true;
        } else {
            singleton = false;
        }
    }
    (!singleton).then_some(tag)
}

pub(crate) fn esc(s: &str) -> String {
    let mut o = String::with_capacity(s.len());
    for c in s.chars().filter(|c| allowed(*c)) {
        match c {
            '&' => o.push_str("&amp;"),
            '<' => o.push_str("&lt;"),
            '>' => o.push_str("&gt;"),
            '"' => o.push_str("&quot;"),
            c => o.push(c),
        }
    }
    o
}

pub(crate) fn base64(data: &[u8]) -> String {
    const A: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut s = String::with_capacity(data.len().div_ceil(3) * 4);
    for c in data.chunks(3) {
        let n = (c[0] as u32) << 16 | (*c.get(1).unwrap_or(&0) as u32) << 8 | *c.get(2).unwrap_or(&0) as u32;
        s.push(A[(n >> 18) as usize & 63] as char);
        s.push(A[(n >> 12) as usize & 63] as char);
        s.push(if c.len() > 1 { A[(n >> 6) as usize & 63] as char } else { '=' });
        s.push(if c.len() > 2 { A[n as usize & 63] as char } else { '=' });
    }
    s
}

/// One HTML file: headings and paragraphs, tables as real `<table>` elements, images inline, a
/// rule between pages.
pub fn html(pages: &[Page], title: &str) -> String {
    html_with_language(pages, title, None)
}

/// A simple HTML export with no CSS; page layout and colours are omitted.
pub fn html_simple(pages: &[Page], title: &str) -> String {
    html_simple_with_language(pages, title, None)
}

/// [`html`] for a document in `language` (a BCP 47 tag such as `bg-BG`, as a PDF's `/Lang` gives
/// it), which the page names (`<html lang>`) for spell checking, speech and hyphenation. A
/// language that is not a well formed tag is left out.
pub fn html_with_language(pages: &[Page], title: &str, language: Option<&str>) -> String {
    html_with_mode(pages, title, false, language)
}

/// [`html_simple`] for a document in `language`, named as [`html_with_language`] names it: a
/// language is not styling, so the simple export keeps it.
pub fn html_simple_with_language(pages: &[Page], title: &str, language: Option<&str>) -> String {
    html_with_mode(pages, title, true, language)
}

fn style_color(color: [f64; 3], simple: bool) -> String {
    if simple { String::new() } else { hex(color).map_or(String::new(), |h| format!(" style=\"color:#{h}\"")) }
}

fn wrap_inline(mut text: String, bold: bool, italic: bool) -> String {
    if text.trim().is_empty() {
        return text;
    }
    if italic {
        text = format!("<em>{text}</em>");
    }
    if bold {
        text = format!("<strong>{text}</strong>");
    }
    text
}

fn para_html(runs: &[Run<'_>], lvl: u8, simple: bool) -> Option<String> {
    let non_empty: Vec<_> = runs.iter().map(|r| (r.block, esc(&r.text))).filter(|(_, text)| !text.is_empty()).collect();
    let &(first, _) = non_empty.iter().find(|(_, text)| !text.trim().is_empty())?;
    let mut t = String::new();
    for (block, text) in non_empty {
        if text.trim().is_empty() {
            t.push_str(&text);
            continue;
        }
        let mut run = wrap_inline(text, block.bold && lvl == 0, block.italic);
        // The paragraph has its first visible run's colour; a run in another colour says so.
        let colour = rgb8(block.color);
        if !simple && colour != rgb8(first.color) {
            let [red, green, blue] = colour;
            run = format!("<span style=\"color:#{red:02X}{green:02X}{blue:02X}\">{run}</span>");
        }
        t.push_str(&run);
    }
    let style = style_color(first.color, simple);
    Some(match lvl.min(6) {
        0 => format!("<p{style}>{t}</p>\n"),
        l => format!("<h{l}{style}>{t}</h{l}>\n"),
    })
}

fn table_html(t: &Table, simple: bool) -> String {
    let mut s = String::from("<table>\n");
    for row in &t.rows {
        s.push_str("<tr>");
        for c in row {
            let body = wrap_inline(esc(&c.text), c.bold, c.italic);
            let span = if c.span > 1 { format!(" colspan=\"{}\"", c.span) } else { String::new() };
            let style = style_color(c.color, simple);
            s.push_str(&format!("<td{span}{style}>{body}</td>"));
        }
        s.push_str("</tr>\n");
    }
    s.push_str("</table>\n");
    s
}

fn html_with_mode(pages: &[Page], title: &str, simple: bool, language: Option<&str>) -> String {
    let lang = language_tag(language).map(|tag| format!(" lang=\"{}\"", esc(tag))).unwrap_or_default();
    let mut s = format!(
        "<!DOCTYPE html>\n<html{lang}>\n<head>\n<meta charset=\"utf-8\">\n<title>{}</title>\n{}\n</head>\n<body>\n",
        esc(title),
        if simple {
            ""
        } else {
            "<style>body{max-width:46em;margin:2em auto;font-family:sans-serif;line-height:1.45}img{max-width:100%}hr{border:0;border-top:1px solid #ccc;margin:2em 0}table{border-collapse:collapse;margin:1em 0}td,th{border:1px solid #999;padding:2px 6px;vertical-align:top}</style>"
        }
    );
    for it in items(pages) {
        match it {
            Item::Para(runs, lvl) => {
                if let Some(p) = para_html(&runs, lvl, simple) {
                    s.push_str(&p);
                }
            }
            Item::Table(t) => s.push_str(&table_html(&t, simple)),
            Item::Img(im) => {
                let mime = if im.ext == "jpg" { "image/jpeg" } else { "image/png" };
                s.push_str(&format!("<p><img alt=\"\" src=\"data:{mime};base64,{}\"></p>\n", base64(&im.bytes)));
            }
            Item::PageBreak => s.push_str("<hr>\n"),
        }
    }
    s.push_str("</body>\n</html>\n");
    s
}

#[cfg(test)]
mod tests {
    use super::{html, para_html, table_html};
    use crate::layout::{Block, Cell, Page, Run, Table};

    fn block(text: &str, x0: f64, y0: f64, x1: f64, y1: f64) -> Block {
        Block {
            text: text.into(),
            rect: [x0, y0, x1, y1],
            size: 12.0,
            bold: false,
            italic: false,
            color: [0.0; 3],
            rotation: 0.0,
            space_around: [false; 2],
        }
    }

    #[test]
    fn empty_or_control_only_runs_and_blocks_do_not_emit_empty_tags() {
        let page = Page {
            width: 612.0,
            height: 792.0,
            blocks: vec![
                Block { bold: true, italic: true, color: [1.0, 0.0, 0.0], ..block("\u{1}\u{FFFF}", 72.0, 700.0, 90.0, 712.0) },
                Block { color: [0.0, 0.5, 0.0], ..block("Hello", 90.5, 700.0, 140.0, 712.0) },
                Block { bold: true, ..block("\u{0}\u{2}", 72.0, 660.0, 120.0, 672.0) },
            ],
            images: Vec::new(),
            background: None,
        };
        let out = html(&[page], "Clean");
        assert!(out.contains("<p style=\"color:#008000\">Hello</p>"), "{out}");
        assert!(!out.contains("<strong></strong>"), "{out}");
        assert!(!out.contains("<em></em>"), "{out}");
        assert_eq!(out.matches("<p").count(), 1, "{out}");

        // Styled whitespace-only paragraphs are omitted, and leading whitespace runs do not set
        // the paragraph colour or emit emphasis/colour wrappers.
        let ws_block = Block { bold: true, italic: true, color: [1.0, 0.0, 0.0], ..block("   ", 72.0, 700.0, 80.0, 712.0) };
        let vis_block = Block { color: [0.0, 0.5, 0.0], ..block("Visible", 80.0, 700.0, 120.0, 712.0) };
        assert_eq!(para_html(&[Run { text: "   \t\n".into(), block: &ws_block }], 0, false), None);
        assert_eq!(
            para_html(&[Run { text: " ".into(), block: &ws_block }, Run { text: "Visible".into(), block: &vis_block }], 0, false).as_deref(),
            Some("<p style=\"color:#008000\"> Visible</p>\n")
        );
    }

    #[test]
    fn heading_level_is_clamped_and_empty_styled_cells_stay_clean() {
        let b = block("Deep Heading", 72.0, 700.0, 200.0, 712.0);
        let runs = [Run { text: "Deep Heading".into(), block: &b }];
        assert_eq!(para_html(&runs, 9, true).as_deref(), Some("<h6>Deep Heading</h6>\n"));

        let table = Table {
            rect: [0.0, 0.0, 100.0, 20.0],
            cols: vec![0.0, 50.0],
            rows: vec![vec![
                Cell { text: "\u{1}".into(), size: 11.0, bold: true, italic: true, color: [0.0; 3], span: 1 },
                Cell { text: "Val".into(), size: 11.0, bold: true, italic: false, color: [0.0; 3], span: 1 },
            ]],
        };
        assert_eq!(table_html(&table, true), "<table>\n<tr><td></td><td><strong>Val</strong></td></tr>\n</table>\n");
    }
}
