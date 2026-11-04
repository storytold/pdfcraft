//! Copy with formatting: selected text as paragraphs of styled spans, written as an HTML
//! fragment for the clipboard (inline styles, so a word processor keeps font, size, weight,
//! slant and colour when it pastes).

use crate::as_html::allowed;
use crate::{esc, hex};

/// Spans kept for one copy; text past it is left out and [`ClipText::truncated`] is set.
pub const MAX_CLIP_SPANS: usize = 20_000;
/// Text bytes kept for one copy (before escaping).
pub const MAX_CLIP_TEXT_BYTES: usize = 2 << 20;
/// The HTML written for one copy never passes this many bytes.
pub const MAX_CLIP_HTML_BYTES: usize = 16 << 20;
/// Font sizes are written within this range (points), so a hostile size stays a font size.
const CLIP_SIZE_PT: std::ops::RangeInclusive<f64> = 1.0..=1000.0;
/// A family name longer than this is not a family name.
const MAX_FAMILY_CHARS: usize = 64;

/// How a span looks.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ClipStyle {
    /// The family a word processor knows ([`font_family`]); `None` keeps the target's own.
    pub family: Option<String>,
    /// Size in points; zero or not finite leaves the target's size, and any other is written
    /// within 1 to 1000.
    pub size: f64,
    pub bold: bool,
    pub italic: bool,
    /// Fill colour as RGB in 0..=1.
    pub color: [f64; 3],
}

/// A run of text in one style.
#[derive(Clone, Debug, PartialEq)]
pub struct ClipSpan {
    pub text: String,
    pub style: ClipStyle,
}

/// The copied text: paragraphs of spans, and whether a cap left some out.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ClipText {
    pub paragraphs: Vec<Vec<ClipSpan>>,
    pub truncated: bool,
}

/// Builds a [`ClipText`] piece by piece, merging pieces of equal style into one span and
/// stopping at the caps.
#[derive(Debug, Default)]
pub struct ClipBuilder {
    out: ClipText,
    /// The separator owed before the next piece in this paragraph (a space or a line break).
    pending: Option<char>,
    new_paragraph: bool,
    spans: usize,
    bytes: usize,
}

impl ClipBuilder {
    pub fn new() -> Self {
        Self { new_paragraph: true, ..Self::default() }
    }

    /// A word space before the next piece (dropped at the start of a paragraph).
    pub fn space(&mut self) {
        self.pending.get_or_insert(' ');
    }

    /// A line break inside the paragraph before the next piece.
    pub fn line_break(&mut self) {
        self.pending = Some('\n');
    }

    /// The next piece starts a new paragraph.
    pub fn paragraph(&mut self) {
        self.new_paragraph = true;
        self.pending = None;
    }

    /// Append `text` in `style`. Returns `false` once a cap is reached; nothing more is kept.
    pub fn push(&mut self, text: &str, style: &ClipStyle) -> bool {
        if self.out.truncated {
            return false;
        }
        if text.is_empty() {
            return true;
        }
        let sep = if self.new_paragraph { None } else { self.pending };
        let need = text.len().saturating_add(sep.map_or(0, char::len_utf8));
        let Some(bytes) = self.bytes.checked_add(need).filter(|b| *b <= MAX_CLIP_TEXT_BYTES) else {
            self.out.truncated = true;
            return false;
        };
        if self.new_paragraph {
            self.out.paragraphs.push(Vec::new());
            self.new_paragraph = false;
        }
        self.pending = None;
        let Some(spans) = self.out.paragraphs.last_mut() else { return false };
        match spans.last_mut() {
            Some(last) if last.style == *style => {
                last.text.extend(sep);
                last.text.push_str(text);
            }
            last => {
                if self.spans >= MAX_CLIP_SPANS {
                    self.out.truncated = true;
                    return false;
                }
                // A separator belongs to the span before it, so a new style starts at its text.
                if let (Some(last), Some(sep)) = (last, sep) {
                    last.text.push(sep);
                }
                spans.push(ClipSpan { text: text.to_owned(), style: style.clone() });
                self.spans += 1;
            }
        }
        self.bytes = bytes;
        true
    }

    pub fn finish(self) -> ClipText {
        self.out
    }
}

/// The family a word processor knows, from a font's `/BaseFont`: subset tag and style suffix
/// removed, `PSMT`/`MT`/`PS` dropped, camel case spaced (`ABCDEF+TimesNewRomanPSMT` →
/// `Times New Roman`). Only letters, digits, spaces, `-`, `_` and `.` are kept, so the name
/// cannot leave its CSS string. `None` when nothing is left.
pub fn font_family(base_font: &str) -> Option<String> {
    let name = match base_font.split_once('+') {
        Some((tag, rest)) if tag.len() == 6 && tag.bytes().all(|b| b.is_ascii_uppercase()) => rest,
        _ => base_font,
    };
    let family = name.split_once([',', '-']).map_or(name, |(family, _)| family);
    let family = family.strip_suffix("PSMT").or_else(|| family.strip_suffix("MT")).or_else(|| family.strip_suffix("PS")).unwrap_or(family);
    let mut spaced = String::with_capacity(family.len().min(MAX_FAMILY_CHARS * 2));
    let mut prev_lower = false;
    for c in family.chars().take(MAX_FAMILY_CHARS) {
        if prev_lower && c.is_ascii_uppercase() {
            spaced.push(' ');
        }
        prev_lower = c.is_ascii_lowercase();
        spaced.push(c);
    }
    safe_family(&spaced)
}

/// `family` with only letters, digits, spaces, `-`, `_` and `.`, so it cannot leave a CSS
/// string; `None` when nothing is left.
fn safe_family(family: &str) -> Option<String> {
    let kept: String = family.chars().filter(|c| c.is_alphanumeric() || matches!(c, ' ' | '-' | '_' | '.')).take(MAX_FAMILY_CHARS * 2).collect();
    let kept = kept.trim();
    (!kept.is_empty()).then(|| kept.to_owned())
}

/// The copied text as an HTML fragment: one `<p>` per paragraph, a `<span>` per run with its
/// family, size and colour inline, bold as `<strong>` and italic as `<em>` (so a target that
/// drops style attributes still keeps them). Text is escaped; a line break is `<br>` and a run
/// of spaces keeps its width. Never longer than [`MAX_CLIP_HTML_BYTES`]: a span that would pass
/// it ends the fragment.
pub fn clip_html(text: &ClipText) -> String {
    const OPEN: &str = "<p style=\"margin:0\">";
    const CLOSE: &str = "</p>\n";
    let mut out = String::new();
    for spans in &text.paragraphs {
        let mut p = String::from(OPEN);
        for span in spans {
            let s = span_html(span);
            if out.len().saturating_add(p.len()).saturating_add(s.len()).saturating_add(CLOSE.len()) > MAX_CLIP_HTML_BYTES {
                if p.len() > OPEN.len() {
                    out.push_str(&p);
                    out.push_str(CLOSE);
                }
                return out;
            }
            p.push_str(&s);
        }
        p.push_str(CLOSE);
        out.push_str(&p);
    }
    out
}

fn span_html(span: &ClipSpan) -> String {
    let st = &span.style;
    let mut css = Vec::new();
    if let Some(f) = st.family.as_deref().and_then(safe_family) {
        css.push(format!("font-family:'{f}'"));
    }
    if st.size.is_finite() && st.size > 0.0 {
        css.push(format!("font-size:{}pt", num(st.size.clamp(*CLIP_SIZE_PT.start(), *CLIP_SIZE_PT.end()))));
    }
    if let Some(h) = hex(st.color) {
        css.push(format!("color:#{h}"));
    }
    let mut body = text_html(&span.text);
    if st.italic {
        body = format!("<em>{body}</em>");
    }
    if st.bold {
        body = format!("<strong>{body}</strong>");
    }
    if css.is_empty() { body } else { format!("<span style=\"{}\">{body}</span>", esc(&css.join(";"))) }
}

/// Escape for HTML text in one pass; `\n` becomes `<br>` and a run of spaces keeps its width
/// with `&nbsp;`. Dropped: what [`esc`] drops (C0 controls), C1 controls, a carriage return
/// and the bidirectional overrides and isolates, which would reorder the pasted text.
fn text_html(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut prev_space = false;
    for c in text.chars().filter(|c| clip_char(*c)) {
        match c {
            '\n' => out.push_str("<br>"),
            ' ' if prev_space => out.push_str("&nbsp;"),
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            c => out.push(c),
        }
        prev_space = c == ' ';
    }
    out
}

/// Whether `c` may go into the clipboard HTML's text.
fn clip_char(c: char) -> bool {
    allowed(c) && !matches!(c, '\r' | '\u{80}'..='\u{9F}' | '\u{202A}'..='\u{202E}' | '\u{2066}'..='\u{2069}')
}

/// A size with at most one decimal and no trailing `.0`.
fn num(v: f64) -> String {
    let r = (v * 10.0).round() / 10.0;
    if r.fract() == 0.0 { format!("{r:.0}") } else { format!("{r:.1}") }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn style(size: f64, bold: bool, italic: bool, color: [f64; 3]) -> ClipStyle {
        ClipStyle { family: font_family("ABCDEF+Helvetica-Bold"), size, bold, italic, color }
    }

    #[test]
    fn heading_body_and_coloured_span_keep_their_formatting() {
        let mut b = ClipBuilder::new();
        let heading = style(24.0, true, false, [0.0; 3]);
        let body = style(11.5, false, false, [0.0; 3]);
        let red = style(11.5, false, true, [1.0, 0.0, 0.0]);
        b.push("Title", &heading);
        b.paragraph();
        b.push("Body", &body);
        b.space();
        b.push("text", &body);
        b.space();
        b.push("red", &red);
        let clip = b.finish();
        assert_eq!(clip.paragraphs.len(), 2);
        assert_eq!(clip.paragraphs.get(1).map(Vec::len), Some(2), "{clip:?}");
        let html = clip_html(&clip);
        assert_eq!(
            html,
            "<p style=\"margin:0\"><span style=\"font-family:'Helvetica';font-size:24pt\"><strong>Title</strong></span></p>\n\
             <p style=\"margin:0\"><span style=\"font-family:'Helvetica';font-size:11.5pt\">Body text </span>\
             <span style=\"font-family:'Helvetica';font-size:11.5pt;color:#FF0000\"><em>red</em></span></p>\n"
        );
    }

    #[test]
    fn special_characters_are_escaped_and_cannot_leave_the_style() {
        let st = ClipStyle { family: Some("Evil';}</style><script>\\\n".into()), size: f64::NAN, ..ClipStyle::default() };
        let mut b = ClipBuilder::new();
        b.push("<a href=\"x\">&\u{0}\u{1b}</a>  two\nline", &st);
        let html = clip_html(&b.finish());
        assert_eq!(
            html,
            "<p style=\"margin:0\"><span style=\"font-family:'Evilstylescript'\">&lt;a href=&quot;x&quot;&gt;&amp;&lt;/a&gt; &nbsp;two<br>line</span></p>\n"
        );
    }

    #[test]
    fn invisible_controls_and_bidi_overrides_are_dropped() {
        let mut b = ClipBuilder::new();
        b.push("a\u{85}b\u{9F}c\rd\u{202E}e\u{2066}f\u{2069}g\u{202A}h\u{2067}i\tj", &ClipStyle::default());
        assert_eq!(clip_html(&b.finish()), "<p style=\"margin:0\">abcdefghi\tj</p>\n");
    }

    #[test]
    fn font_sizes_stay_within_range() {
        for (size, want) in [(1e300, "1000"), (f64::MAX, "1000"), (2e306, "1000"), (0.2, "1"), (9.96, "10")] {
            let st = ClipStyle { size, ..ClipStyle::default() };
            let mut b = ClipBuilder::new();
            b.push("x", &st);
            let html = clip_html(&b.finish());
            assert_eq!(html, format!("<p style=\"margin:0\"><span style=\"font-size:{want}pt\">x</span></p>\n"), "{size}");
        }
    }

    #[test]
    fn families_read_like_a_word_processor_names_them() {
        assert_eq!(font_family("ABCDEF+TimesNewRomanPSMT").as_deref(), Some("Times New Roman"));
        assert_eq!(font_family("Arial,BoldItalic").as_deref(), Some("Arial"));
        assert_eq!(font_family("Arial-BoldMT").as_deref(), Some("Arial"));
        assert_eq!(font_family("abcdef+Foo").as_deref(), Some("abcdefFoo"), "a lower-case prefix is not a subset tag");
        assert_eq!(font_family(""), None);
        assert_eq!(font_family("+'\"\\;"), None);
        assert!(font_family(&"Ab".repeat(10_000)).is_some_and(|f| f.chars().count() <= MAX_FAMILY_CHARS * 2));
    }

    #[test]
    fn empty_and_huge_selections_are_bounded() {
        let b = ClipBuilder::new();
        assert_eq!(clip_html(&b.finish()), "");
        let mut b = ClipBuilder::new();
        b.space();
        assert!(b.push("", &ClipStyle::default()));
        assert!(b.finish().paragraphs.is_empty(), "nothing pushed, nothing kept");

        // Alternating styles: the span cap stops it.
        let (a, c) = (style(10.0, false, false, [0.0; 3]), style(10.0, true, false, [0.0; 3]));
        let mut b = ClipBuilder::new();
        let mut kept = 0usize;
        for i in 0..MAX_CLIP_SPANS + 10 {
            if b.push("x", if i % 2 == 0 { &a } else { &c }) {
                kept += 1;
            }
        }
        let clip = b.finish();
        assert!(clip.truncated);
        assert_eq!(kept, MAX_CLIP_SPANS);
        // One style: the text cap stops it.
        let mut b = ClipBuilder::new();
        let chunk = "y".repeat(1 << 16);
        // Bounded, so the test fails rather than runs on when the cap is gone.
        let pushes = (0..MAX_CLIP_TEXT_BYTES / chunk.len() + 2).take_while(|_| b.push(&chunk, &a)).count();
        assert!(pushes <= MAX_CLIP_TEXT_BYTES / chunk.len(), "the cap stopped it: {pushes}");
        let clip = b.finish();
        assert!(clip.truncated);
        assert!(clip.paragraphs.iter().flatten().map(|s| s.text.len()).sum::<usize>() <= MAX_CLIP_TEXT_BYTES);
        // HTML: escaping grows text up to sixfold; the writer still stops at its cap.
        let big = ClipText { paragraphs: vec![vec![ClipSpan { text: "<".repeat(1 << 20), style: a.clone() }]; 8], truncated: false };
        let html = clip_html(&big);
        assert!(html.len() <= MAX_CLIP_HTML_BYTES && html.ends_with("</p>\n"), "{}", html.len());
        assert!(html.len() > MAX_CLIP_HTML_BYTES / 2);
    }
}
