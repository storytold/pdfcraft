//! Portable standalone HTML document writer (no PDF layout, reading order, or export policy).

pub(crate) use craft_mime::transfer::base64;

/// Whether a character may appear in the output. XML 1.0 forbids the C0 controls other than tab,
/// line feed and carriage return, and U+FFFE/U+FFFF.
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

/// An inline run of text for portable HTML rendering.
pub(crate) struct HtmlRun<'a> {
    pub text: &'a str,
    pub bold: bool,
    pub italic: bool,
    pub color: [u8; 3],
}

/// A single table cell for portable HTML rendering.
pub(crate) struct HtmlCell<'a> {
    pub text: &'a str,
    pub bold: bool,
    pub italic: bool,
    pub color: [u8; 3],
    pub span: usize,
}

/// A table of rows of cells for portable HTML rendering.
pub(crate) struct HtmlTable<'a> {
    pub rows: Vec<Vec<HtmlCell<'a>>>,
}

/// A block-level element in a portable HTML document.
pub(crate) enum HtmlBlock<'a> {
    Para { runs: Vec<HtmlRun<'a>>, level: u8 },
    Table(HtmlTable<'a>),
    Image { mime: &'a str, bytes: &'a [u8] },
    PageBreak,
}

fn rgb_hex(rgb: [u8; 3]) -> Option<String> {
    let [r, g, b] = rgb;
    (r | g | b != 0).then(|| format!("{r:02X}{g:02X}{b:02X}"))
}

fn style_color(rgb: [u8; 3], simple: bool) -> String {
    if simple { String::new() } else { rgb_hex(rgb).map_or(String::new(), |h| format!(" style=\"color:#{h}\"")) }
}

pub(crate) fn wrap_inline(mut text: String, bold: bool, italic: bool) -> String {
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

pub(crate) fn render_para(runs: &[HtmlRun<'_>], lvl: u8, simple: bool) -> Option<String> {
    let non_empty: Vec<_> = runs.iter().map(|r| (r, esc(r.text))).filter(|(_, text)| !text.is_empty()).collect();
    let &(first, _) = non_empty.iter().find(|(_, text)| !text.trim().is_empty())?;
    let mut t = String::new();
    for (run, text) in non_empty {
        if text.trim().is_empty() {
            t.push_str(&text);
            continue;
        }
        let mut out = wrap_inline(text, run.bold && lvl == 0, run.italic);
        // The paragraph has its first visible run's colour; a run in another colour says so.
        if !simple && run.color != first.color {
            let [red, green, blue] = run.color;
            out = format!("<span style=\"color:#{red:02X}{green:02X}{blue:02X}\">{out}</span>");
        }
        t.push_str(&out);
    }
    let style = style_color(first.color, simple);
    Some(match lvl.min(6) {
        0 => format!("<p{style}>{t}</p>\n"),
        l => format!("<h{l}{style}>{t}</h{l}>\n"),
    })
}

pub(crate) fn render_table(t: &HtmlTable<'_>, simple: bool) -> String {
    let mut s = String::from("<table>\n");
    for row in &t.rows {
        s.push_str("<tr>");
        for c in row {
            let body = wrap_inline(esc(c.text), c.bold, c.italic);
            let span = if c.span > 1 { format!(" colspan=\"{}\"", c.span) } else { String::new() };
            let style = style_color(c.color, simple);
            s.push_str(&format!("<td{span}{style}>{body}</td>"));
        }
        s.push_str("</tr>\n");
    }
    s.push_str("</table>\n");
    s
}

/// Render a sequence of portable HTML blocks into a standalone HTML5 document.
pub(crate) fn render(blocks: &[HtmlBlock<'_>], title: &str, simple: bool, language: Option<&str>) -> String {
    let lang = language.map(|tag| format!(" lang=\"{}\"", esc(tag))).unwrap_or_default();
    let mut s = format!(
        "<!DOCTYPE html>\n<html{lang}>\n<head>\n<meta charset=\"utf-8\">\n<title>{}</title>\n{}\n</head>\n<body>\n",
        esc(title),
        if simple {
            ""
        } else {
            "<style>body{max-width:46em;margin:2em auto;font-family:sans-serif;line-height:1.45}img{max-width:100%}hr{border:0;border-top:1px solid #ccc;margin:2em 0}table{border-collapse:collapse;margin:1em 0}td,th{border:1px solid #999;padding:2px 6px;vertical-align:top}</style>"
        }
    );
    for block in blocks {
        match block {
            HtmlBlock::Para { runs, level } => {
                if let Some(p) = render_para(runs, *level, simple) {
                    s.push_str(&p);
                }
            }
            HtmlBlock::Table(t) => s.push_str(&render_table(t, simple)),
            HtmlBlock::Image { mime, bytes } => {
                s.push_str(&format!("<p><img alt=\"\" src=\"data:{mime};base64,{}\"></p>\n", base64(bytes)));
            }
            HtmlBlock::PageBreak => s.push_str("<hr>\n"),
        }
    }
    s.push_str("</body>\n</html>\n");
    s
}
