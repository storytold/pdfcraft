use super::as_docx::{hex, rgb8};
use super::layout::{Item, Page, items};

/// Whether a character may appear in the output. XML 1.0 forbids the C0 controls other than tab,
/// line feed and carriage return, and U+FFFE/U+FFFF. Text extracted from PDFs often holds them
/// (fonts without a usable `/ToUnicode`), and Word refuses a whole document over one (#72).
pub(crate) fn allowed(c: char) -> bool {
    matches!(c, '\t' | '\n' | '\r') || !(c.is_ascii_control() || c == '\u{FFFE}' || c == '\u{FFFF}')
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
                for r in &runs {
                    let mut run = esc(&r.text);
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
                        let mut body = esc(&c.text);
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
