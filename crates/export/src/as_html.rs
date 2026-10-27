use super::as_docx::{hex, rgb8};
use super::layout::{Item, Page, items};

/// Whether a character may appear in the output. XML 1.0 forbids the C0 controls other than tab,
/// line feed and carriage return, and U+FFFE/U+FFFF. Text extracted from PDFs often holds them
/// (fonts without a usable `/ToUnicode`), and Word refuses a whole document over one (#72).
pub(crate) fn allowed(c: char) -> bool {
    matches!(c, '\t' | '\n' | '\r') || !(c.is_ascii_control() || c == '\u{FFFE}' || c == '\u{FFFF}')
}

/// Append one allowed character with the same escaping in HTML text and XML/attribute values.
fn escaped_char(out: &mut String, c: char) {
    match c {
        '&' => out.push_str("&amp;"),
        '<' => out.push_str("&lt;"),
        '>' => out.push_str("&gt;"),
        '"' => out.push_str("&quot;"),
        c => out.push(c),
    }
}

/// Escape XML/title/attribute values, dropping illegal characters and keeping literal whitespace.
pub(crate) fn esc(s: &str) -> String {
    let mut o = String::with_capacity(s.len());
    for c in s.chars().filter(|c| allowed(*c)) {
        escaped_char(&mut o, c);
    }
    o
}

/// Escape visible HTML text, retaining CR/LF/CRLF as breaks within its paragraph or cell.
/// The CR state spans styled runs, but only adjacent source characters form a CRLF pair.
fn text_html(s: &str, after_cr: &mut bool) -> String {
    let mut o = String::with_capacity(s.len());
    for c in s.chars() {
        let crlf = c == '\n' && *after_cr;
        *after_cr = c == '\r';
        if crlf || !allowed(c) {
            continue;
        }
        if matches!(c, '\r' | '\n') {
            o.push_str("<br>");
        } else {
            escaped_char(&mut o, c);
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
/// rule between pages. Explicit text line endings become breaks within paragraphs and cells.
pub fn html(pages: &[Page], title: &str) -> String {
    let mut s = format!(
        "<!DOCTYPE html>\n<html>\n<head>\n<meta charset=\"utf-8\">\n<title>{}</title>\n<style>body{{max-width:46em;margin:2em auto;font-family:sans-serif;line-height:1.45}}\
img{{max-width:100%}}hr{{border:0;border-top:1px solid #ccc;margin:2em 0}}\
table{{border-collapse:collapse;margin:1em 0}}td,th{{border:1px solid #999;padding:2px 6px;vertical-align:top}}</style>\n</head>\n<body>\n",
        esc(title)
    );
    for it in items(pages) {
        match it {
            Item::Para(runs, lvl) => {
                let Some(first) = runs.first().map(|r| r.block) else { continue };
                let mut t = String::new();
                let mut after_cr = false;
                for r in &runs {
                    let mut run = text_html(&r.text, &mut after_cr);
                    if r.block.italic {
                        run = format!("<em>{run}</em>");
                    }
                    if r.block.bold && lvl == 0 {
                        run = format!("<strong>{run}</strong>");
                    }
                    // The paragraph has its first run's colour; a run in another colour says so.
                    let colour = rgb8(r.block.color);
                    if colour != rgb8(first.color) {
                        let [red, green, blue] = colour;
                        run = format!("<span style=\"color:#{red:02X}{green:02X}{blue:02X}\">{run}</span>");
                    }
                    t.push_str(&run);
                }
                let style = hex(first.color).map_or(String::new(), |h| format!(" style=\"color:#{h}\""));
                match lvl {
                    0 => s.push_str(&format!("<p{style}>{t}</p>\n")),
                    l => s.push_str(&format!("<h{l}{style}>{t}</h{l}>\n")),
                }
            }
            Item::Table(t) => {
                s.push_str("<table>\n");
                for row in &t.rows {
                    s.push_str("<tr>");
                    for c in row {
                        let mut after_cr = false;
                        let mut body = text_html(&c.text, &mut after_cr);
                        if c.italic {
                            body = format!("<em>{body}</em>");
                        }
                        if c.bold {
                            body = format!("<strong>{body}</strong>");
                        }
                        let span = if c.span > 1 { format!(" colspan=\"{}\"", c.span) } else { String::new() };
                        let style = hex(c.color).map_or(String::new(), |h| format!(" style=\"color:#{h}\""));
                        s.push_str(&format!("<td{span}{style}>{body}</td>"));
                    }
                    s.push_str("</tr>\n");
                }
                s.push_str("</table>\n");
            }
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
