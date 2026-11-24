use super::layout::SheetLayout;
use super::{Font, Page, Value, Workbook};

/// Side margin of headers and footers, in points, when Align with Page Margins is off.
pub(super) const HF_FIXED_SIDE_MARGIN: f32 = 0.7 * 72.0;

// ---------------------------------------------------------------- header / footer

/// Values for header/footer codes.
#[derive(Clone, Debug, Default)]
pub struct HfContext {
    pub page: usize,
    pub pages: usize,
    pub sheet: String,
    pub file: String,
    pub path: String,
    pub date: String,
    pub time: String,
}

/// Today's date and the current time as headers and footers show them (`&D`, `&T`).
pub fn hf_date_time(wb: &Workbook) -> (String, String) {
    let now = gridcraft_core::date::serial_in_system(wb.date_system, gridcraft_calc::now_serial());
    let date = crate::display::format(&Value::Number(now.floor()), "m/d/yyyy", wb).text;
    let time = crate::display::format(&Value::Number(now), "h:mm AM/PM", wb).text;
    (date, time)
}

/// Header/footer codes and the field names the Page Layout view shows for them while editing.
pub const HF_FIELDS: [(char, &str); 8] = [
    ('P', "&[Page]"),
    ('N', "&[Pages]"),
    ('D', "&[Date]"),
    ('T', "&[Time]"),
    ('Z', "&[Path]"),
    ('F', "&[File]"),
    ('A', "&[Tab]"),
    ('G', "&[Picture]"),
];

/// Splits a header/footer string into its raw left, centre and right sections (codes kept).
/// Text before the first `&L`/`&C`/`&R` belongs to the centre, as when printing.
pub fn split_header_footer(spec: &str) -> [String; 3] {
    let mut out: [String; 3] = Default::default();
    let mut cur = 1usize;
    let chars: Vec<char> = spec.chars().take(10_000).collect();
    let mut i = 0;
    while let Some(&c) = chars.get(i) {
        let next = chars.get(i + 1).copied();
        let sec = cur.min(2);
        match (c, next) {
            ('&', Some('L' | 'l')) => cur = 0,
            ('&', Some('C' | 'c')) => cur = 1,
            ('&', Some('R' | 'r')) => cur = 2,
            ('&', Some('"')) => {
                // &"Font,Style" is kept whole.
                let end = chars[i + 2..].iter().position(|ch| *ch == '"').map_or(chars.len(), |p| i + 3 + p);
                out[sec].extend(&chars[i..end.min(chars.len())]);
                i = end;
                continue;
            }
            ('&', Some(n)) => {
                out[sec].push('&');
                out[sec].push(n);
            }
            ('&', None) => {}
            (ch, _) => {
                out[sec].push(ch);
                i += 1;
                continue;
            }
        }
        i += 2;
    }
    out
}

/// Joins raw left, centre and right sections into one header/footer string.
pub fn join_header_footer(sections: &[String; 3]) -> String {
    let mut s = String::new();
    for (code, text) in ["&L", "&C", "&R"].iter().zip(sections) {
        if !text.is_empty() {
            s.push_str(code);
            s.push_str(text);
        }
    }
    s
}

/// A raw section as the user edits it: `&P` shows as `&[Page]`, `&N` as `&[Pages]`, and so on.
pub fn codes_to_fields(raw: &str) -> String {
    let mut out = String::new();
    let mut it = raw.chars().peekable();
    while let Some(c) = it.next() {
        if c != '&' {
            out.push(c);
            continue;
        }
        // A dangling `&` at the end is dropped (printing ignores it too).
        if let Some(n) = it.next() {
            match HF_FIELDS.iter().find(|(code, _)| *code == n.to_ascii_uppercase()) {
                Some((_, field)) => out.push_str(field),
                None => {
                    out.push('&');
                    out.push(n);
                }
            }
        }
    }
    out
}

/// The inverse of [`codes_to_fields`]: field names become codes again; a dangling `&` at the end
/// becomes a literal ampersand so it can't swallow the next section's code.
pub fn fields_to_codes(text: &str) -> String {
    let chars: Vec<char> = text.chars().take(10_000).collect();
    let mut out = String::new();
    let mut i = 0;
    while let Some(&c) = chars.get(i) {
        if c != '&' {
            out.push(c);
            i += 1;
            continue;
        }
        if chars.get(i + 1) == Some(&'&') {
            out.push_str("&&");
            i += 2;
            continue;
        }
        let rest: String = chars[i..].iter().take(12).collect();
        if let Some((code, field)) = HF_FIELDS.iter().find(|(_, f)| rest.starts_with(f)) {
            out.push('&');
            out.push(*code);
            i += field.chars().count();
            continue;
        }
        if i + 1 == chars.len() {
            out.push_str("&&");
        } else {
            out.push('&');
        }
        i += 1;
    }
    out
}

/// One header/footer section: its text and whether `&B` made it bold.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct HfSection {
    pub text: String,
    pub bold: bool,
    pub italic: bool,
    pub size: Option<f32>,
}

/// Most bytes one section's expanded text may hold. A code like `&A` adds the whole sheet name
/// (or file path) each time it appears, and a JSON workbook's names have no length limit.
const MAX_SECTION_BYTES: usize = 64 * 1024;

/// Appends `s` unless the section would pass [`MAX_SECTION_BYTES`]; what doesn't fit is left out.
fn push_field(text: &mut String, s: &str) {
    if text.len().saturating_add(s.len()) <= MAX_SECTION_BYTES {
        text.push_str(s);
    }
}

/// Expands a header/footer string into its left, centre and right sections.
pub fn expand_header_footer(spec: &str, cx: &HfContext) -> [HfSection; 3] {
    let mut out: [HfSection; 3] = Default::default();
    let mut cur = 1usize;
    let chars: Vec<char> = spec.chars().take(10_000).collect();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        if c != '&' {
            if let Some(s) = out.get_mut(cur) {
                s.text.push(c);
            }
            i += 1;
            continue;
        }
        let Some(&code) = chars.get(i + 1) else { break };
        i += 2;
        let sec = &mut out[cur.min(2)];
        match code {
            'L' | 'l' => cur = 0,
            'C' | 'c' => cur = 1,
            'R' | 'r' => cur = 2,
            'P' | 'p' => {
                // &P, &P+n, &P-n
                let mut n = cx.page as i64;
                if let Some(&sign) = chars.get(i)
                    && (sign == '+' || sign == '-')
                {
                    let digits: String = chars[i + 1..].iter().take_while(|d| d.is_ascii_digit()).take(6).collect();
                    if let Ok(d) = digits.parse::<i64>() {
                        n = if sign == '+' { n + d } else { n - d };
                        i += 1 + digits.len();
                    }
                }
                push_field(&mut sec.text, &n.to_string());
            }
            'N' | 'n' => push_field(&mut sec.text, &cx.pages.to_string()),
            'D' | 'd' => push_field(&mut sec.text, &cx.date),
            'T' | 't' => push_field(&mut sec.text, &cx.time),
            'A' | 'a' => push_field(&mut sec.text, &cx.sheet),
            'F' | 'f' => push_field(&mut sec.text, &cx.file),
            'Z' | 'z' => push_field(&mut sec.text, &cx.path),
            '&' => sec.text.push('&'),
            'B' | 'b' => sec.bold = !sec.bold,
            'I' | 'i' => sec.italic = !sec.italic,
            '"' => {
                // &"Font,Style": skip the font name; honour Bold/Italic in the style.
                let rest: String = chars[i..].iter().take_while(|c| **c != '"').collect();
                let lower = rest.to_ascii_lowercase();
                if lower.contains("bold") {
                    sec.bold = true;
                }
                if lower.contains("italic") || lower.contains("oblique") {
                    sec.italic = true;
                }
                i += rest.chars().count() + 1;
            }
            d if d.is_ascii_digit() => {
                let digits: String = std::iter::once(d).chain(chars[i..].iter().copied().take_while(char::is_ascii_digit)).take(3).collect();
                i += digits.len() - 1;
                if let Ok(v) = digits.parse::<f32>() {
                    sec.size = Some(v.clamp(1.0, 409.0));
                }
            }
            // Underline, strike, super/subscript, colour, picture: not rendered.
            'K' | 'k' => {
                i = (i + 6).min(chars.len());
            }
            _ => {}
        }
    }
    out
}

pub(super) fn draw_header_footer(pg: &mut Page, lay: &SheetLayout, which: gridcraft_model::HfPage, cx: &HfContext) {
    let [_, _, _, _, hm, fm] = lay.margins;
    let (l, r) = lay.hf_sides();
    let text_scale = lay.hf_text_scale();
    let pw = lay.page_w;
    let ph = lay.page_h;
    for (spec, header) in [(lay.settings.hf(which, false), true), (lay.settings.hf(which, true), false)] {
        if spec.is_empty() {
            continue;
        }
        let secs = expand_header_footer(spec, cx);
        for (i, sec) in secs.iter().enumerate() {
            if sec.text.trim().is_empty() {
                continue;
            }
            let size = sec.size.unwrap_or(10.0).clamp(1.0, 72.0) * text_scale;
            let font = Font::styled(gridcraft_pdf::Family::Sans, sec.bold, sec.italic);
            let lines: Vec<&str> = sec.text.split('\n').take(20).collect();
            let n = lines.len() as f32;
            for (j, line) in lines.iter().enumerate() {
                let lw = gridcraft_pdf::text_width(font, size, line);
                let x = match i {
                    0 => l,
                    1 => (pw - lw) / 2.0,
                    _ => pw - r - lw,
                };
                let y = if header { hm + size * 0.8 + j as f32 * size * 1.2 } else { ph - fm - size * 0.22 - (n - 1.0 - j as f32) * size * 1.2 };
                pg.text(x, y, size, font, [0, 0, 0], line);
            }
        }
    }
}
