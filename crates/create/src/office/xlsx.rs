//! Excel (SpreadsheetML, ECMA-376 Part 1 §18) to PDF. Each visible sheet's used range is printed
//! the way Excel prints it by default: at 100 %, columns and rows at their widths and heights,
//! pages ordered down then across, gridlines only when the sheet asks for them. Cell values are
//! formatted with their number formats (dates, currency, percentages…), and fonts, fills,
//! borders, alignment, wrapping and merged cells are kept. Formulas show their cached results.

use std::collections::{BTreeMap, BTreeSet, HashMap};

use chrono::Datelike;
use pdfcraft_cos::Document;
use roxmltree::Node;

use super::canvas::{Canvas, Page};
use super::flow::fin;
use super::xml::{self, Rels, attr, attr_f64, child, children, flag, is, rel_attr};
use super::zip::Zip;
use super::{Notes, RunStyle, family_of, hex_color};

/// Most cells read from one sheet.
const MAX_CELLS: usize = 2_000_000;
/// Most merged ranges read from one sheet.
const MAX_MERGES: usize = 20_000;
/// Most cell text a sheet keeps, in bytes.
const MAX_TEXT_BYTES: usize = 64 << 20;
/// Most columns a sheet's texts step through when running on into their empty neighbours.
const MAX_RUN_ON_WORK: usize = 20_000_000;
/// Most merge comparisons a sheet's pages make when finding the merge each cell is in.
const MAX_MERGE_WORK: usize = 100_000_000;
const MAX_COL: u32 = 16_384;
/// Most columns a sheet's `<col>` ranges name between them: four times the sheet's width, so
/// overlapping ranges in a valid file still apply in full but repeated ones can't multiply the work.
const MAX_COL_WORK: u32 = MAX_COL * 4;
const MAX_ROW: u32 = 1_048_576;
/// Longest number format read, in characters: Excel's own limit. Every cell re-reads its format,
/// so this also bounds the padding and placeholders a format can ask for.
const MAX_FORMAT_CHARS: usize = 255;

// ── styles ──────────────────────────────────────────────────────────────────────────────────

#[derive(Clone, Debug, Default)]
struct Font {
    name: String,
    size: f64,
    bold: bool,
    italic: bool,
    underline: bool,
    strike: bool,
    color: Option<[f64; 3]>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
struct Edge {
    width: f64,
    color: [f64; 3],
}

#[derive(Clone, Copy, Debug, Default)]
struct Edges {
    left: Option<Edge>,
    right: Option<Edge>,
    top: Option<Edge>,
    bottom: Option<Edge>,
}

#[derive(Clone, Debug, Default)]
struct Xf {
    num_fmt: u32,
    font: usize,
    fill: usize,
    border: usize,
    h_align: Option<String>,
    v_align: Option<String>,
    wrap: bool,
    indent: f64,
}

#[derive(Default)]
struct Styles {
    fonts: Vec<Font>,
    fills: Vec<Option<[f64; 3]>>,
    borders: Vec<Edges>,
    xfs: Vec<Xf>,
    formats: HashMap<u32, String>,
}

/// The default palette behind `indexed` colours.
const INDEXED: [u32; 64] = [
    0x000000, 0xFFFFFF, 0xFF0000, 0x00FF00, 0x0000FF, 0xFFFF00, 0xFF00FF, 0x00FFFF, 0x000000, 0xFFFFFF, 0xFF0000, 0x00FF00, 0x0000FF, 0xFFFF00,
    0xFF00FF, 0x00FFFF, 0x800000, 0x008000, 0x000080, 0x808000, 0x800080, 0x008080, 0xC0C0C0, 0x808080, 0x9999FF, 0x993366, 0xFFFFCC, 0xCCFFFF,
    0x660066, 0xFF8080, 0x0066CC, 0xCCCCFF, 0x000080, 0xFF00FF, 0xFFFF00, 0x00FFFF, 0x800080, 0x800000, 0x008080, 0x0000FF, 0x00CCFF, 0xCCFFFF,
    0xCCFFCC, 0xFFFF99, 0x99CCFF, 0xFF99CC, 0xCC99FF, 0xFFCC99, 0x3366FF, 0x33CCCC, 0x99CC00, 0xFFCC00, 0xFF9900, 0xFF6600, 0x666699, 0x969696,
    0x003366, 0x339966, 0x003300, 0x333300, 0x993300, 0x993366, 0x333399, 0x333333,
];

fn color(n: Node<'_, '_>, theme: &[[f64; 3]]) -> Option<[f64; 3]> {
    let mut c = if let Some(rgb) = attr(n, "rgb") {
        hex_color(rgb)?
    } else if let Some(t) = attr_f64(n, "theme") {
        *theme.get(t.clamp(0.0, 64.0) as usize)?
    } else {
        let i = attr_f64(n, "indexed")?;
        let i = i.clamp(0.0, 100.0) as usize;
        if i == 64 {
            // System foreground.
            [0.0; 3]
        } else {
            let v = *INDEXED.get(i)?;
            [((v >> 16) & 0xFF) as f64 / 255.0, ((v >> 8) & 0xFF) as f64 / 255.0, (v & 0xFF) as f64 / 255.0]
        }
    };
    if let Some(t) = attr_f64(n, "tint").filter(|t| t.is_finite()) {
        let t = t.clamp(-1.0, 1.0);
        c = c.map(|v| if t < 0.0 { v * (1.0 + t) } else { v + (1.0 - v) * t });
    }
    Some(c)
}

fn theme_colors(zip: &Zip<'_>, rels: &Rels) -> Vec<[f64; 3]> {
    // Excel's order: lt1, dk1, lt2, dk2, accent1–6, hlink, folHlink.
    let mut out = vec![[1.0; 3], [0.0; 3], [0.93, 0.93, 0.88], [0.27, 0.33, 0.42]];
    let Some(part) = rels.of_type("theme") else { return out };
    let Ok(Some(text)) = zip.text(part) else { return out };
    let Ok(d) = xml::parse(&text) else { return out };
    let Some(scheme) = d.descendants().find(|n| is(*n, "clrScheme")) else { return out };
    let get = |name: &str| -> Option<[f64; 3]> {
        let e = child(scheme, name)?.children().find(Node::is_element)?;
        match e.tag_name().name() {
            "srgbClr" => hex_color(attr(e, "val")?),
            "sysClr" => attr(e, "lastClr").and_then(hex_color),
            _ => None,
        }
    };
    out = ["lt1", "dk1", "lt2", "dk2", "accent1", "accent2", "accent3", "accent4", "accent5", "accent6", "hlink", "folHlink"]
        .iter()
        .enumerate()
        .map(|(i, n)| get(n).unwrap_or(if i == 0 { [1.0; 3] } else { [0.0; 3] }))
        .collect();
    out
}

fn load_styles(zip: &Zip<'_>, rels: &Rels, theme: &[[f64; 3]], notes: &mut Notes) -> Result<Styles, String> {
    let mut s = Styles::default();
    let Some(part) = rels.of_type("styles") else { return Ok(s) };
    let Some(text) = zip.text(part)? else { return Ok(s) };
    let d = xml::parse(&text).map_err(|e| format!("{part}: {e}"))?;
    let root = d.root_element();
    if let Some(f) = child(root, "numFmts") {
        for n in children(f, "numFmt") {
            if let (Some(id), Some(code)) = (attr_f64(n, "numFmtId"), attr(n, "formatCode")) {
                let code = if code.chars().nth(MAX_FORMAT_CHARS).is_some() {
                    notes.add("number formats longer than Excel allows, shown as General");
                    "General"
                } else {
                    code
                };
                s.formats.insert(id.clamp(0.0, 1e6) as u32, code.to_owned());
            }
        }
    }
    if let Some(f) = child(root, "fonts") {
        for n in children(f, "font") {
            let val = |k: &str| child(n, k).map(|c| flag(attr(c, "val")));
            s.fonts.push(Font {
                name: child(n, "name").and_then(|c| attr(c, "val")).unwrap_or("Calibri").to_owned(),
                size: child(n, "sz").and_then(|c| attr_f64(c, "val")).unwrap_or(11.0).clamp(1.0, 400.0),
                bold: val("b").unwrap_or(false),
                italic: val("i").unwrap_or(false),
                underline: child(n, "u").is_some_and(|c| attr(c, "val") != Some("none")),
                strike: val("strike").unwrap_or(false),
                color: child(n, "color").and_then(|c| color(c, theme)),
            });
        }
    }
    if let Some(f) = child(root, "fills") {
        for n in children(f, "fill") {
            let fill = child(n, "patternFill").and_then(|p| {
                let ty = attr(p, "patternType").unwrap_or("none");
                if ty == "none" {
                    return None;
                }
                child(p, "fgColor").and_then(|c| color(c, theme)).or_else(|| child(p, "bgColor").and_then(|c| color(c, theme)))
            });
            let fill =
                fill.or_else(|| child(n, "gradientFill").and_then(|g| g.descendants().find(|n| is(*n, "color"))).and_then(|c| color(c, theme)));
            s.fills.push(fill);
        }
    }
    if let Some(f) = child(root, "borders") {
        for n in children(f, "border") {
            let edge = |names: [&str; 2]| -> Option<Edge> {
                let e = names.iter().find_map(|k| child(n, k))?;
                let width = match attr(e, "style")? {
                    "none" => return None,
                    "thin" | "hair" | "dotted" | "dashed" | "dashDot" | "dashDotDot" => 0.5,
                    "medium" | "mediumDashed" | "mediumDashDot" | "mediumDashDotDot" | "slantDashDot" => 1.0,
                    "thick" | "double" => 1.5,
                    _ => 0.5,
                };
                Some(Edge { width, color: child(e, "color").and_then(|c| color(c, theme)).unwrap_or([0.0; 3]) })
            };
            s.borders.push(Edges {
                left: edge(["left", "start"]),
                right: edge(["right", "end"]),
                top: edge(["top", "top"]),
                bottom: edge(["bottom", "bottom"]),
            });
        }
    }
    if let Some(f) = child(root, "cellXfs") {
        for n in children(f, "xf") {
            let al = child(n, "alignment");
            let idx = |k: &str| attr_f64(n, k).map_or(0, |v| v.clamp(0.0, 1e6) as usize);
            s.xfs.push(Xf {
                num_fmt: attr_f64(n, "numFmtId").map_or(0, |v| v.clamp(0.0, 1e6) as u32),
                font: idx("fontId"),
                fill: idx("fillId"),
                border: idx("borderId"),
                h_align: al.and_then(|a| attr(a, "horizontal")).map(str::to_owned),
                v_align: al.and_then(|a| attr(a, "vertical")).map(str::to_owned),
                wrap: al.is_some_and(|a| flag(attr(a, "wrapText")) && attr(a, "wrapText").is_some()),
                indent: al.and_then(|a| attr_f64(a, "indent")).unwrap_or(0.0).clamp(0.0, 250.0),
            });
        }
    }
    Ok(s)
}

// ── number formats ──────────────────────────────────────────────────────────────────────────

fn builtin_format(id: u32) -> &'static str {
    match id {
        1 => "0",
        2 => "0.00",
        3 => "#,##0",
        4 => "#,##0.00",
        5 => "\"$\"#,##0_);(\"$\"#,##0)",
        6 => "\"$\"#,##0_);[Red](\"$\"#,##0)",
        7 => "\"$\"#,##0.00_);(\"$\"#,##0.00)",
        8 => "\"$\"#,##0.00_);[Red](\"$\"#,##0.00)",
        9 => "0%",
        10 => "0.00%",
        11 => "0.00E+00",
        12 => "# ?/?",
        13 => "# ??/??",
        // The regional short date; written unambiguously.
        14 => "yyyy-mm-dd",
        15 => "d-mmm-yy",
        16 => "d-mmm",
        17 => "mmm-yy",
        18 => "h:mm AM/PM",
        19 => "h:mm:ss AM/PM",
        20 => "h:mm",
        21 => "h:mm:ss",
        22 => "yyyy-mm-dd h:mm",
        37 => "#,##0 ;(#,##0)",
        38 => "#,##0 ;[Red](#,##0)",
        39 => "#,##0.00;(#,##0.00)",
        40 => "#,##0.00;[Red](#,##0.00)",
        45 => "mm:ss",
        46 => "[h]:mm:ss",
        47 => "mm:ss.0",
        48 => "##0.0E+0",
        49 => "@",
        _ => "General",
    }
}

/// Locales that write a decimal comma (by Windows LCID), from a `[$-xxxx]` tag.
fn comma_decimal(lcid: u32) -> bool {
    matches!(
        lcid & 0x3FF,
        0x16 | 0x07
            | 0x0C
            | 0x0A
            | 0x10
            | 0x13
            | 0x15
            | 0x19
            | 0x1F
            | 0x05
            | 0x0E
            | 0x1B
            | 0x24
            | 0x1A
            | 0x06
            | 0x0B
            | 0x1D
            | 0x14
            | 0x02
            | 0x22
            | 0x26
            | 0x27
            | 0x25
            | 0x18
            | 0x08
    )
}

fn portuguese(lcid: u32) -> bool {
    lcid & 0x3FF == 0x16
}

fn general(v: f64) -> String {
    if v == 0.0 {
        return "0".into();
    }
    if v.fract() == 0.0 && v.abs() < 1e11 {
        return format!("{v:.0}");
    }
    let a = v.abs();
    if !(1e-9..1e11).contains(&a) {
        let s = format!("{v:.5E}");
        // Rust writes 1.23457E15; Excel writes 1.23457E+15.
        let (m, e) = s.split_once('E').unwrap_or((&s, "0"));
        let m = m.trim_end_matches('0').trim_end_matches('.');
        let e: i32 = e.parse().unwrap_or(0);
        return format!("{m}E{}{:02}", if e < 0 { '-' } else { '+' }, e.abs());
    }
    // Ten significant digits, trailing zeros dropped.
    let digits = (9 - a.log10().floor() as i32).clamp(0, 10) as usize;
    let s = format!("{v:.digits$}");
    if s.contains('.') { s.trim_end_matches('0').trim_end_matches('.').to_string() } else { s }
}

fn group(int: &str, sep: char) -> String {
    let mut out = String::new();
    let n = int.len();
    for (i, c) in int.chars().enumerate() {
        if i > 0 && (n - i).is_multiple_of(3) {
            out.push(sep);
        }
        out.push(c);
    }
    out
}

/// Split a format code into its `;` sections, outside quotes and brackets.
fn sections(code: &str) -> Vec<String> {
    let mut out = vec![String::new()];
    let (mut quoted, mut bracket, mut escape) = (false, false, false);
    for c in code.chars() {
        if let Some(cur) = out.last_mut() {
            if escape {
                cur.push(c);
                escape = false;
                continue;
            }
            match c {
                '\\' => {
                    escape = true;
                    cur.push(c);
                }
                '"' => {
                    quoted = !quoted;
                    cur.push(c);
                }
                '[' if !quoted => {
                    bracket = true;
                    cur.push(c);
                }
                ']' if !quoted => {
                    bracket = false;
                    cur.push(c);
                }
                ';' if !quoted && !bracket => out.push(String::new()),
                c => cur.push(c),
            }
        }
        if out.len() > 4 {
            break;
        }
    }
    out
}

/// A parsed format section: literal text and placeholders in order.
#[derive(Debug, Default)]
struct Section {
    prefix: String,
    suffix: String,
    int_zeros: usize,
    int_any: bool,
    dec_zeros: usize,
    dec_hash: usize,
    grouping: bool,
    percent: usize,
    scale: i32,
    sci: Option<usize>,
    text_at: bool,
    date: bool,
    tokens: Vec<String>,
    red: bool,
    lcid: u32,
}

/// Whether a format section is a date/time format: date or time letters outside quotes, escapes
/// and brackets, and no "General".
fn is_date_format(sec: &str, lower: &[char]) -> bool {
    let mut date = false;
    let (mut q, mut b, mut esc) = (false, false, false);
    for c in lower {
        match c {
            _ if esc => esc = false,
            '\\' if !q => esc = true,
            '"' => q = !q,
            '[' if !q => b = true,
            ']' if !q => b = false,
            'y' | 'd' | 'h' | 's' | 'm' if !q && !b => date = true,
            _ => {}
        }
    }
    if sec.to_ascii_lowercase().contains("general") {
        date = false;
    }
    date
}

impl Section {
    /// Apply a `[...]` tag's contents: a colour, a currency and locale, or an elapsed-time token.
    fn bracket_tag(&mut self, tag: &str, lit: &mut String) {
        let lower = tag.to_ascii_lowercase();
        if lower == "red" {
            self.red = true;
        } else if let Some(rest) = tag.strip_prefix('$') {
            let (sym, lcid) = rest.split_once('-').unwrap_or((rest, ""));
            self.lcid = u32::from_str_radix(lcid.trim(), 16).unwrap_or(0);
            if self.date {
                if !sym.is_empty() {
                    self.tokens.push(format!("\"{sym}"));
                }
            } else {
                lit.push_str(sym);
            }
        } else if self.date && matches!(lower.as_str(), "h" | "hh" | "m" | "mm" | "s" | "ss") {
            self.tokens.push(format!("[{lower}]"));
        }
    }

    /// Read one date/time token starting with `c`, the character just before `*i` in `chars`
    /// (`lower` is `chars` lowercased), and advance `*i` past it.
    fn date_token(&mut self, c: char, (chars, lower): (&[char], &[char]), i: &mut usize) {
        let starts_at = |at: usize, word: &str| word.chars().enumerate().all(|(k, w)| lower.get(at + k) == Some(&w));
        let lc = c.to_ascii_lowercase();
        if matches!(lc, 'y' | 'm' | 'd' | 'h' | 's') {
            let mut tok = lc.to_string();
            while chars.get(*i).is_some_and(|n| n.to_ascii_lowercase() == lc) {
                tok.push(lc);
                *i += 1;
            }
            self.tokens.push(tok);
        } else if starts_at(i.saturating_sub(1), "am/pm") {
            self.tokens.push("ampm".into());
            *i += 4;
        } else if starts_at(i.saturating_sub(1), "a/p") {
            self.tokens.push("ap".into());
            *i += 2;
        } else if c == '0' {
            let mut tok = "0".to_string();
            while chars.get(*i) == Some(&'0') {
                tok.push('0');
                *i += 1;
            }
            self.tokens.push(tok);
        } else {
            self.tokens.push(format!("\"{c}"));
        }
    }
}

fn parse_section(sec: &str) -> Section {
    let mut s = Section::default();
    let mut lit = String::new();
    let mut seen_digit = false;
    let mut in_dec = false;
    let chars: Vec<char> = sec.chars().collect();
    let lower: Vec<char> = chars.iter().map(char::to_ascii_lowercase).collect();
    s.date = is_date_format(sec, &lower);
    let mut i = 0usize;
    let flush = |s: &mut Section, lit: &mut String, seen: bool| {
        if seen {
            s.suffix.push_str(lit);
        } else {
            s.prefix.push_str(lit);
        }
        lit.clear();
    };
    while let Some(&c) = chars.get(i) {
        i += 1;
        match c {
            '"' => {
                while let Some(&q) = chars.get(i) {
                    i += 1;
                    if q == '"' {
                        break;
                    }
                    lit.push(q);
                }
                if s.date {
                    s.tokens.push(format!("\"{lit}"));
                    lit.clear();
                }
            }
            '\\' => {
                if let Some(&q) = chars.get(i) {
                    i += 1;
                    if s.date {
                        s.tokens.push(format!("\"{q}"));
                    } else {
                        lit.push(q);
                    }
                }
            }
            '_' => {
                // Space as wide as the next character.
                i += 1;
                if s.date {
                    s.tokens.push("\" ".into());
                } else {
                    lit.push(' ');
                }
            }
            '*' => i += 1,
            '[' => {
                let mut tag = String::new();
                while let Some(&q) = chars.get(i) {
                    i += 1;
                    if q == ']' {
                        break;
                    }
                    tag.push(q);
                }
                s.bracket_tag(&tag, &mut lit);
            }
            '@' => {
                s.text_at = true;
                flush(&mut s, &mut lit, seen_digit);
                seen_digit = true;
            }
            _ if s.date => s.date_token(c, (&chars, &lower), &mut i),
            '0' | '#' | '?' => {
                if !seen_digit {
                    flush(&mut s, &mut lit, false);
                }
                seen_digit = true;
                if s.sci.is_some() {
                    s.sci = s.sci.map(|n| n + 1);
                } else if in_dec {
                    if c == '0' {
                        s.dec_zeros += 1 + s.dec_hash;
                        s.dec_hash = 0;
                    } else {
                        s.dec_hash += 1;
                    }
                } else {
                    s.int_any = true;
                    if c == '0' {
                        s.int_zeros += 1;
                    }
                }
                // Trailing commas after the last digit scale by thousands.
                s.scale = 0;
            }
            '.' if seen_digit || chars.get(i).is_some_and(|n| matches!(n, '0' | '#' | '?')) => {
                if !seen_digit {
                    flush(&mut s, &mut lit, false);
                    seen_digit = true;
                }
                in_dec = true;
            }
            ',' if seen_digit && s.sci.is_none() => {
                if chars.get(i).is_some_and(|n| matches!(n, '0' | '#' | '?')) {
                    s.grouping = true;
                } else {
                    s.scale += 1;
                }
            }
            'E' | 'e' if seen_digit && chars.get(i).is_some_and(|n| matches!(n, '+' | '-')) => {
                i += 1;
                s.sci = Some(0);
            }
            '%' => {
                s.percent += 1;
                lit.push('%');
            }
            c => lit.push(c),
        }
    }
    flush(&mut s, &mut lit, seen_digit);
    s
}

const MONTHS: [&str; 12] = ["January", "February", "March", "April", "May", "June", "July", "August", "September", "October", "November", "December"];
const MESES: [&str; 12] =
    ["janeiro", "fevereiro", "março", "abril", "maio", "junho", "julho", "agosto", "setembro", "outubro", "novembro", "dezembro"];
const DAYS: [&str; 7] = ["Sunday", "Monday", "Tuesday", "Wednesday", "Thursday", "Friday", "Saturday"];
const DIAS: [&str; 7] = ["domingo", "segunda-feira", "terça-feira", "quarta-feira", "quinta-feira", "sexta-feira", "sábado"];

/// An ISO 8601 date, date-time or time (`t="d"` cells) as an Excel serial in the workbook's date
/// system: the inverse of [`date`].
fn iso_serial(v: &str, date1904: bool) -> Option<f64> {
    use chrono::{NaiveDate, NaiveDateTime, NaiveTime, Timelike};
    let v = v.trim().trim_end_matches('Z');
    let (day, time) = if let Ok(dt) = NaiveDateTime::parse_from_str(v, "%Y-%m-%dT%H:%M:%S%.f") {
        (Some(dt.date()), dt.time())
    } else if let Ok(d) = NaiveDate::parse_from_str(v, "%Y-%m-%d") {
        (Some(d), NaiveTime::MIN)
    } else {
        (None, NaiveTime::parse_from_str(v, "%H:%M:%S%.f").ok()?)
    };
    let serial_day = match day {
        None => 0,
        Some(d) => {
            let ce = i64::from(d.num_days_from_ce());
            // The 1900 system counts a 29 February 1900 that never was: serial 60, so days from
            // 1 March 1900 are one more than their distance from 30 December 1899.
            if date1904 {
                ce - 695_056
            } else if ce - 693_594 >= 61 {
                ce - 693_594
            } else {
                ce - 693_595
            }
        }
    };
    Some(serial_day as f64 + f64::from(time.num_seconds_from_midnight()) / 86_400.0)
}

/// A `t="d"` cell: its ISO date shown in the cell's number format (as an ISO date and time under
/// General), or as written, and reported, when it can't be read.
fn iso_date_cell(cell: &mut Cell, v: &str, styles: &Styles, date1904: bool, notes: &mut Notes) {
    let Some(serial) = iso_serial(v, date1904) else {
        cell.text = v.trim().to_owned();
        notes.add("date cells in a form PdfCraft can't read, shown as written");
        return;
    };
    let id = styles.xfs.get(cell.xf).map_or(0, |x| x.num_fmt);
    let code = match styles.formats.get(&id) {
        Some(code) => code.as_str(),
        None if id == 0 && serial.fract() == 0.0 => "yyyy-mm-dd",
        None if id == 0 => "yyyy-mm-dd hh:mm:ss",
        None => builtin_format(id),
    };
    let (t, red) = format_number(serial, code, date1904);
    cell.text = t;
    cell.red = red;
    cell.numeric = true;
}

fn date(serial: f64, sec: &Section, date1904: bool) -> Option<String> {
    if !serial.is_finite() || !(-1.0..3_000_000.0).contains(&serial) {
        return None;
    }
    let day = serial.floor() as i64;
    let mut secs = ((serial - serial.floor()) * 86_400.0).round() as i64;
    let day = day + secs / 86_400;
    secs %= 86_400;
    // Days from 1 January of year 1 (chrono's day 1). The 1904 system's day 0 is 1 January 1904;
    // the 1900 system's day 1 is 1 January 1900, and it counts a 29 February 1900 that never was
    // (serial 60).
    let ce_days = if date1904 {
        day + 695_056
    } else if day >= 60 {
        day + 693_594
    } else {
        day + 693_595
    };
    let date = chrono::NaiveDate::from_num_days_from_ce_opt(i32::try_from(ce_days).ok()?)?;
    let (y, m, d) = (i64::from(date.year()), date.month(), date.day());
    let weekday = date.weekday().num_days_from_sunday() as usize;
    let (h, mi, s) = (secs / 3600, (secs / 60) % 60, secs % 60);
    let ampm = sec.tokens.iter().any(|t| t == "ampm" || t == "ap");
    let pt = portuguese(sec.lcid);
    let mut out = String::new();
    let toks = &sec.tokens;
    for (i, t) in toks.iter().enumerate() {
        // `m` means minutes after an hour or before a second.
        let minute = t.starts_with('m')
            && t.len() <= 2
            && (toks.get(..i).and_then(|p| p.iter().rev().find(|x| !x.starts_with('"'))).is_some_and(|p| p.starts_with('h') || p.starts_with("[h"))
                || toks.get(i + 1..).and_then(|n| n.iter().find(|x| !x.starts_with('"'))).is_some_and(|n| n.starts_with('s')));
        let month_name = |full: bool| {
            let names = if pt { &MESES } else { &MONTHS };
            let name = names.get(m.saturating_sub(1) as usize).copied().unwrap_or("");
            if full { name.to_string() } else { name.chars().take(3).collect() }
        };
        match t.as_str() {
            "yy" => out.push_str(&format!("{:02}", y.rem_euclid(100))),
            t if t.starts_with('y') => out.push_str(&format!("{y:04}")),
            "m" if minute => out.push_str(&mi.to_string()),
            "mm" if minute => out.push_str(&format!("{mi:02}")),
            "m" => out.push_str(&m.to_string()),
            "mm" => out.push_str(&format!("{m:02}")),
            "mmm" => out.push_str(&month_name(false)),
            "mmmmm" => out.push_str(&month_name(false).chars().take(1).collect::<String>()),
            t if t.starts_with("mmmm") => out.push_str(&month_name(true)),
            "d" => out.push_str(&d.to_string()),
            "dd" => out.push_str(&format!("{d:02}")),
            "ddd" | "dddd" => {
                let names = if pt { &DIAS } else { &DAYS };
                let name = names.get(weekday).copied().unwrap_or("");
                if t.len() == 3 {
                    out.push_str(&name.chars().take(3).collect::<String>());
                } else {
                    out.push_str(name);
                }
            }
            "h" | "hh" => {
                let hv = if ampm { (h + 11) % 12 + 1 } else { h };
                out.push_str(&if t.len() == 2 { format!("{hv:02}") } else { hv.to_string() });
            }
            "[h]" | "[hh]" => out.push_str(&(day * 24 + h).to_string()),
            "[m]" | "[mm]" => out.push_str(&((day * 24 + h) * 60 + mi).to_string()),
            "s" => out.push_str(&s.to_string()),
            "ss" => out.push_str(&format!("{s:02}")),
            "ampm" => out.push_str(if h < 12 { "AM" } else { "PM" }),
            "ap" => out.push_str(if h < 12 { "A" } else { "P" }),
            t if t.starts_with('"') => out.push_str(t.get(1..).unwrap_or("")),
            _ => {}
        }
    }
    Some(out)
}

/// Format a number with a format code. Returns the text and whether it is red.
fn format_number(v: f64, code: &str, date1904: bool) -> (String, bool) {
    if code.eq_ignore_ascii_case("general") || code.is_empty() {
        return (general(v), false);
    }
    let secs = sections(code);
    let (sec_text, negative_section) = match (secs.len(), v) {
        (_, v) if v > 0.0 => (secs.first(), false),
        (1, _) => (secs.first(), false),
        (_, v) if v < 0.0 => (secs.get(1), true),
        (2, _) => (secs.first(), false),
        _ => (secs.get(2).or(secs.first()), false),
    };
    let sec = parse_section(sec_text.map_or("General", String::as_str));
    if sec.date {
        return (date(v, &sec, date1904).unwrap_or_else(|| general(v)), sec.red);
    }
    if sec_text.is_some_and(|s| s.to_ascii_lowercase().contains("general")) {
        let g = general(if negative_section { v.abs() } else { v });
        return (format!("{}{g}{}", sec.prefix.replace("General", ""), sec.suffix), sec.red);
    }
    let (dec_sep, group_sep) = if comma_decimal(sec.lcid) { (',', '.') } else { ('.', ',') };
    let mut x = if negative_section { v.abs() } else { v };
    for _ in 0..sec.percent.min(3) {
        x *= 100.0;
    }
    for _ in 0..sec.scale.clamp(0, 5) {
        x /= 1000.0;
    }
    let decimals = (sec.dec_zeros + sec.dec_hash).min(15);
    // Excel rounds halves away from zero; Rust's formatting rounds them to even.
    if sec.sci.is_none() {
        let k = 10f64.powi(decimals as i32);
        let r = (x * k).round() / k;
        if r.is_finite() {
            x = r;
        }
    }
    let body = if let Some(exp_digits) = sec.sci {
        let s = format!("{:.*E}", decimals, x.abs());
        let (m, e) = s.split_once('E').unwrap_or((&s, "0"));
        let e: i32 = e.parse().unwrap_or(0);
        format!(
            "{}{}E{}{:0w$}",
            if x < 0.0 { "-" } else { "" },
            m.replace('.', &dec_sep.to_string()),
            if e < 0 { '-' } else { '+' },
            e.abs(),
            w = exp_digits.max(1)
        )
    } else {
        let s = format!("{:.*}", decimals, x.abs());
        let (int, frac) = s.split_once('.').unwrap_or((&s, ""));
        let mut int = int.trim_start_matches('0').to_string();
        if int.len() < sec.int_zeros {
            int.insert_str(0, &"0".repeat(sec.int_zeros - int.len()));
        }
        if sec.grouping {
            int = group(&int, group_sep);
        }
        let mut frac = frac.to_string();
        while frac.len() > sec.dec_zeros && frac.ends_with('0') {
            frac.pop();
        }
        let neg = x < 0.0 && (int.chars().any(|c| c.is_ascii_digit() && c != '0') || frac.chars().any(|c| c != '0'));
        let mut b = String::new();
        if neg {
            b.push('-');
        }
        b.push_str(&int);
        if !frac.is_empty() {
            b.push(dec_sep);
            b.push_str(&frac);
        } else if decimals > 0 && sec.dec_zeros > 0 {
            b.push(dec_sep);
        }
        if !sec.int_any && sec.int_zeros == 0 && decimals == 0 && !sec.text_at {
            // A format of only literal text (e.g. `"-"` for zero).
            String::new()
        } else {
            b
        }
    };
    (format!("{}{body}{}", sec.prefix, sec.suffix), sec.red)
}

// ── sheets ──────────────────────────────────────────────────────────────────────────────────

#[derive(Clone, Debug)]
struct Cell {
    text: String,
    numeric: bool,
    boolean: bool,
    xf: usize,
    red: bool,
}

fn col_index(r: &str) -> Option<(u32, u32)> {
    let letters: String = r.chars().take_while(char::is_ascii_alphabetic).collect();
    let digits = r.get(letters.len()..)?;
    let mut col = 0u32;
    for c in letters.chars() {
        col = col.checked_mul(26)?.checked_add(u32::from(c.to_ascii_uppercase() as u8 - b'A' + 1))?;
    }
    let row: u32 = digits.parse().ok()?;
    ((1..=MAX_COL).contains(&col) && (1..=MAX_ROW).contains(&row)).then_some((row, col))
}

/// Points for an Excel column width in characters (the Calibri 11 maximum digit width, 7 px).
fn col_points(chars: f64) -> f64 {
    let px = (chars * 7.0 + 5.0).floor().max(0.0);
    px * 0.75
}

/// A stored column width (`<col width>`, `defaultColWidth`) in points. Stored widths already
/// include the 5 px of padding (ECMA-376 Part 1, §18.3.1.13): pixels are
/// `trunc((256·width + trunc(128/7)) / 256 · 7)`.
fn stored_col_points(width: f64) -> f64 {
    let px = ((256.0 * width + 18.0) / 256.0 * 7.0).trunc().max(0.0);
    px * 0.75
}

struct Sheet {
    cells: BTreeMap<(u32, u32), Cell>,
    widths: HashMap<u32, f64>,
    hidden_cols: std::collections::HashSet<u32>,
    heights: HashMap<u32, (f64, bool)>,
    hidden_rows: std::collections::HashSet<u32>,
    default_w: f64,
    default_h: f64,
    merges: Vec<(u32, u32, u32, u32)>,
    gridlines: bool,
    landscape: bool,
    /// Print scale: `pageSetup@scale`, or fit to a number of pages wide and tall.
    scale: f64,
    fit: Option<(u32, u32)>,
    paper: (f64, f64),
    margins: [f64; 4],
}

#[allow(clippy::too_many_arguments)]
fn read_sheet(zip: &Zip<'_>, part: &str, shared: &[String], styles: &Styles, date1904: bool, notes: &mut Notes) -> Result<Sheet, String> {
    let text = zip.text(part)?.ok_or_else(|| format!("{part} is missing"))?;
    let d = xml::parse(&text).map_err(|e| format!("{part}: {e}"))?;
    let root = d.root_element();
    let fmt = child(root, "sheetFormatPr");
    let base = fmt.and_then(|f| attr_f64(f, "baseColWidth")).unwrap_or(8.0);
    let default_w = fmt.and_then(|f| attr_f64(f, "defaultColWidth")).map_or(col_points(base + 0.43), stored_col_points);
    let default_h = fmt.and_then(|f| attr_f64(f, "defaultRowHeight")).unwrap_or(15.0).clamp(1.0, 409.0);
    let mut sh = Sheet {
        cells: BTreeMap::new(),
        widths: HashMap::new(),
        hidden_cols: Default::default(),
        heights: HashMap::new(),
        hidden_rows: Default::default(),
        default_w: fin(default_w, 48.0).clamp(1.0, 2000.0),
        default_h,
        merges: Vec::new(),
        gridlines: child(root, "printOptions").is_some_and(|p| flag(attr(p, "gridLines")) && attr(p, "gridLines").is_some()),
        landscape: child(root, "pageSetup").and_then(|p| attr(p, "orientation")) == Some("landscape"),
        scale: child(root, "pageSetup").and_then(|p| attr_f64(p, "scale")).map_or(1.0, |s| (s / 100.0).clamp(0.1, 4.0)),
        fit: xml::path(root, &["sheetPr", "pageSetUpPr"]).filter(|p| flag(attr(*p, "fitToPage")) && attr(*p, "fitToPage").is_some()).map(|_| {
            let setup = child(root, "pageSetup");
            let n = |k: &str| setup.and_then(|p| attr_f64(p, k)).map_or(1, |v| v.clamp(0.0, 10_000.0) as u32);
            (n("fitToWidth"), n("fitToHeight"))
        }),
        paper: match child(root, "pageSetup").and_then(|p| attr_f64(p, "paperSize")).map(|v| v as i64) {
            Some(9) => (595.28, 841.89),
            Some(5) => (612.0, 1008.0),
            Some(8) => (841.89, 1190.55),
            Some(11) => (419.53, 595.28),
            _ => (612.0, 792.0),
        },
        margins: [54.0, 50.4, 54.0, 50.4],
    };
    if let Some(m) = child(root, "pageMargins") {
        let g = |k: &str, d: f64| attr_f64(m, k).map_or(d, |v| (v * 72.0).clamp(0.0, 300.0));
        sh.margins = [g("top", 54.0), g("right", 50.4), g("bottom", 54.0), g("left", 50.4)];
    }
    if let Some(cols) = child(root, "cols") {
        let mut budget = MAX_COL_WORK;
        for c in children(cols, "col") {
            let (Some(min), Some(max)) = (attr_f64(c, "min"), attr_f64(c, "max")) else { continue };
            let (min, max) = (min.clamp(1.0, f64::from(MAX_COL)) as u32, max.clamp(1.0, f64::from(MAX_COL)) as u32);
            let span = max.saturating_sub(min).saturating_add(1);
            let Some(left) = budget.checked_sub(span) else {
                notes.add("column ranges naming more than four times a sheet's columns, ignored");
                break;
            };
            budget = left;
            let hidden = flag(attr(c, "hidden")) && attr(c, "hidden").is_some();
            let w = attr_f64(c, "width").map(stored_col_points);
            for col in min..=max {
                if hidden {
                    sh.hidden_cols.insert(col);
                } else if let Some(w) = w {
                    sh.widths.insert(col, w.clamp(0.0, 2000.0));
                }
            }
        }
    }
    let mut count = 0usize;
    let mut next_row = 1u32;
    let mut text_bytes = 0usize;
    if let Some(data) = child(root, "sheetData") {
        for row in children(data, "row") {
            let r = attr_f64(row, "r").map_or(next_row, |v| v.clamp(1.0, f64::from(MAX_ROW)) as u32);
            next_row = r.saturating_add(1);
            if flag(attr(row, "hidden")) && attr(row, "hidden").is_some() {
                sh.hidden_rows.insert(r);
                continue;
            }
            if let Some(h) = attr_f64(row, "ht") {
                sh.heights.insert(r, (h.clamp(0.0, 409.0), flag(attr(row, "customHeight")) && attr(row, "customHeight").is_some()));
            }
            let mut next_col = 1u32;
            for c in children(row, "c") {
                count += 1;
                if count > MAX_CELLS {
                    notes.add("cells beyond the first two million of a sheet, left out");
                    break;
                }
                let (rr, cc) = attr(c, "r").and_then(col_index).unwrap_or((r, next_col));
                next_col = cc.saturating_add(1);
                let xf = attr_f64(c, "s").map_or(0, |v| v.clamp(0.0, 1e6) as usize);
                let ty = attr(c, "t").unwrap_or("n");
                let v = child(c, "v").and_then(|v| v.text()).unwrap_or_default();
                if child(c, "f").is_some() && child(c, "v").is_none() && ty != "inlineStr" {
                    notes.add("formulas without a saved result, left empty");
                }
                let mut cell = Cell { text: String::new(), numeric: false, boolean: false, xf, red: false };
                match ty {
                    "s" => cell.text = v.trim().parse::<usize>().ok().and_then(|i| shared.get(i)).cloned().unwrap_or_default(),
                    "inlineStr" => cell.text = child(c, "is").map(|i| xml::text_of(i, "t")).unwrap_or_default(),
                    "str" => cell.text = v.to_owned(),
                    "b" => {
                        cell.text = if v.trim() == "1" { "TRUE".into() } else { "FALSE".into() };
                        cell.boolean = true;
                    }
                    "e" => {
                        cell.text = v.to_owned();
                        cell.boolean = true;
                    }
                    "d" => iso_date_cell(&mut cell, v, styles, date1904, notes),
                    _ => {
                        if let Ok(x) = v.trim().parse::<f64>()
                            && x.is_finite()
                        {
                            let id = styles.xfs.get(xf).map_or(0, |x| x.num_fmt);
                            let code = styles.formats.get(&id).map_or_else(|| builtin_format(id), String::as_str);
                            let (t, red) = format_number(x, code, date1904);
                            cell.text = t;
                            cell.red = red;
                            cell.numeric = true;
                        }
                    }
                }
                // A shared string is copied into every cell that uses it, so one long string used
                // by many cells could exhaust memory: past the cap, cell text is left out.
                text_bytes = text_bytes.saturating_add(cell.text.len());
                if text_bytes > MAX_TEXT_BYTES {
                    notes.add("cell text beyond 64 MiB in a sheet, left out");
                    cell.text.clear();
                }
                let styled = styles.xfs.get(xf).is_some_and(|x| x.fill > 0 || x.border > 0);
                if !cell.text.is_empty() || styled {
                    sh.cells.insert((rr, cc), cell);
                }
            }
        }
    }
    if let Some(m) = child(root, "mergeCells") {
        for mc in children(m, "mergeCell") {
            if sh.merges.len() >= MAX_MERGES {
                notes.add("merged ranges beyond the first 20 000 of a sheet, drawn unmerged");
                break;
            }
            let Some((a, b)) = attr(mc, "ref").and_then(|r| r.split_once(':')) else { continue };
            if let (Some((r1, c1)), Some((r2, c2))) = (col_index(a), col_index(b)) {
                sh.merges.push((r1.min(r2), c1.min(c2), r1.max(r2), c1.max(c2)));
            }
        }
    }
    if child(root, "drawing").is_some() {
        notes.add("charts and pictures on sheets, left out");
    }
    Ok(sh)
}

fn shared_strings(zip: &Zip<'_>, rels: &Rels) -> Result<Vec<String>, String> {
    let Some(part) = rels.of_type("sharedStrings") else { return Ok(Vec::new()) };
    let Some(text) = zip.text(part)? else { return Ok(Vec::new()) };
    let d = xml::parse(&text).map_err(|e| format!("{part}: {e}"))?;
    // Phonetic runs (rPh) are reading aids, not text.
    Ok(children(d.root_element(), "si")
        .map(|si| si.descendants().filter(|n| is(*n, "t") && !n.ancestors().any(|a| is(a, "rPh"))).filter_map(|t| t.text()).collect())
        .collect())
}

/// How wide text at `(r, c)` may draw, running on into empty, unmerged neighbours up to `c1`
/// until it is `needed` wide (it starts `w` wide). Each step spends one of the sheet's `work`;
/// when none are left the text stops where it is and `cut` is set.
fn run_on(
    sh: &Sheet,
    (r, c, c1): (u32, u32, u32),
    (w, needed): (f64, f64),
    width: &dyn Fn(u32) -> f64,
    covered: &dyn Fn(u32, u32) -> bool,
    (work, cut): (&std::cell::Cell<usize>, &std::cell::Cell<bool>),
) -> f64 {
    let mut clip_w = w;
    let mut cc = c.saturating_add(1);
    while clip_w < needed && cc <= c1 && sh.cells.get(&(r, cc)).is_none_or(|n| n.text.is_empty()) && !covered(r, cc) {
        let Some(left) = work.get().checked_sub(1) else {
            cut.set(true);
            break;
        };
        work.set(left);
        clip_w += width(cc);
        cc = cc.saturating_add(1);
    }
    clip_w
}

fn draw_sheet(canvas: &mut Canvas, sh: &Sheet, styles: &Styles, notes: &mut Notes) -> Result<(), String> {
    if sh.cells.is_empty() {
        return Ok(());
    }
    let max_row = sh.cells.keys().map(|(r, _)| *r).max().unwrap_or(1);
    let max_col = sh.cells.keys().map(|(_, c)| *c).max().unwrap_or(1);
    let (pw, ph) = if sh.landscape { (sh.paper.1, sh.paper.0) } else { sh.paper };
    let [mt, mr, mb, ml] = sh.margins;
    let area_w = (pw - ml - mr).max(72.0);
    let area_h = (ph - mt - mb).max(72.0);
    let width = |c: u32| if sh.hidden_cols.contains(&c) { 0.0 } else { sh.widths.get(&c).copied().unwrap_or(sh.default_w) };
    let font_of = |xf: usize| -> Font {
        styles.xfs.get(xf).and_then(|x| styles.fonts.get(x.font)).cloned().unwrap_or(Font { name: "Calibri".into(), size: 11.0, ..Font::default() })
    };
    let style_of = |cell: &Cell| -> RunStyle {
        let f = font_of(cell.xf);
        RunStyle {
            fam: family_of(&f.name),
            bold: f.bold,
            italic: f.italic,
            size: f.size,
            color: if cell.red { [1.0, 0.0, 0.0] } else { f.color.unwrap_or([0.0; 3]) },
            underline: f.underline,
            strike: f.strike,
            ..RunStyle::default()
        }
    };
    // Row heights; wrapped text makes a row without a custom height taller.
    let mut heights: Vec<f64> = vec![0.0; max_row as usize + 1];
    let mut wrapped: HashMap<(u32, u32), Vec<String>> = HashMap::new();
    let mut texts: HashMap<(u32, u32), String> = HashMap::new();
    for ((r, c), cell) in &sh.cells {
        // Line breaks are split on below (or become spaces), and `drawable` drops control
        // characters, so each line is made drawable on its own.
        let lines: Vec<String> = cell.text.split('\n').map(|line| notes.drawable(line)).collect();
        texts.insert((*r, *c), lines.join("\n"));
    }
    for r in 1..=max_row {
        let h = if sh.hidden_rows.contains(&r) { 0.0 } else { sh.heights.get(&r).map_or(sh.default_h, |(h, _)| *h) };
        if let Some(slot) = heights.get_mut(r as usize) {
            *slot = h;
        }
    }
    for ((r, c), cell) in &sh.cells {
        let Some(xf) = styles.xfs.get(cell.xf) else { continue };
        if !xf.wrap || cell.text.is_empty() {
            continue;
        }
        let style = style_of(cell);
        let w = (width(*c) - 4.0).max(4.0);
        let text = texts.get(&(*r, *c)).cloned().unwrap_or_default();
        let lines: Vec<String> = text
            .split('\n')
            .flat_map(|p| pdfcraft_fonts::wrap_with(p, style.size, w, |s, size| RunStyle { size, ..style.clone() }.width(s)))
            .collect();
        let need = lines.len() as f64 * style.size * 1.2 + 3.0;
        if !sh.heights.get(r).is_some_and(|(_, custom)| *custom)
            && let Some(slot) = heights.get_mut(*r as usize)
            && *slot > 0.0
        {
            *slot = slot.max(need);
        }
        wrapped.insert((*r, *c), lines);
    }
    // Pages: bands of columns across, bands of rows down; printed down, then over.
    let bands = |n: u32, size: &dyn Fn(u32) -> f64, avail: f64| -> Vec<(u32, u32)> {
        let mut out = Vec::new();
        let (mut start, mut used) = (1u32, 0.0);
        for i in 1..=n {
            let s = size(i);
            if used + s > avail && used > 0.0 {
                out.push((start, i - 1));
                start = i;
                used = 0.0;
            }
            used += s;
        }
        out.push((start, n));
        out
    };
    let row_h = |r: u32| heights.get(r as usize).copied().unwrap_or(0.0);
    // Excel's print scale: fitting only ever shrinks; 0 pages means "as many as needed".
    let k = match sh.fit {
        Some((fw, fh)) => {
            let total_w: f64 = (1..=max_col).map(width).sum();
            let total_h: f64 = (1..=max_row).map(row_h).sum();
            fit_scale((fw, fh), (total_w, total_h), (area_w, area_h))
        }
        None => sh.scale,
    };
    let col_bands = bands(max_col, &width, area_w / k);
    let row_bands = bands(max_row, &row_h, area_h / k);
    // Running totals, so a column's or row's offset on its page is two lookups instead of a sum
    // over the band before it (a band of hidden columns or rows can be thousands long):
    // `col_at[c]` is the width of the columns before `c`, `row_at[r]` the height of the rows
    // before `r`; both go one past the last, for the far edge.
    let running = |n: u32, size: &dyn Fn(u32) -> f64| -> Vec<f64> {
        let mut at = vec![0.0; 2];
        for i in 1..=n {
            let last = at.last().copied().unwrap_or(0.0);
            at.push(last + size(i));
        }
        at
    };
    let col_at = running(max_col, &width);
    let row_at = running(max_row, &row_h);
    let at = |v: &[f64], i: u32| v.get(i as usize).or(v.last()).copied().unwrap_or(0.0);
    // Only the pages a cell is on, found from the cells: a sparse sheet (one value at XFD1048576,
    // wide columns, tall rows) has billions of empty ones. Ordered down, then over.
    let band_of = |bands: &[(u32, u32)], i: u32| bands.partition_point(|(_, end)| *end < i);
    let pages: BTreeSet<(usize, usize)> = sh.cells.keys().map(|(r, c)| (band_of(&col_bands, *c), band_of(&row_bands, *r))).collect();
    // Each lookup scans the page's merges; past this many comparisons on a sheet, the rest of its
    // cells are drawn unmerged (and that is reported) rather than scanning on.
    let merge_work = std::cell::Cell::new(MAX_MERGE_WORK);
    let merge_cut = std::cell::Cell::new(false);
    // Text runs on one column at a time; past this many steps on a sheet, the rest is clipped.
    let run_on_work = std::cell::Cell::new(MAX_RUN_ON_WORK);
    let run_on_cut = std::cell::Cell::new(false);
    for (ci, ri) in pages {
        let (Some((c0, c1)), Some((r0, r1))) = (col_bands.get(ci), row_bands.get(ri)) else { continue };
        // Only the merges that touch this page are searched for each of its cells.
        let merges: Vec<(u32, u32, u32, u32)> =
            sh.merges.iter().copied().filter(|(mr1, mc1, mr2, mc2)| *mr1 <= *r1 && *mr2 >= *r0 && *mc1 <= *c1 && *mc2 >= *c0).collect();
        let merged_at = |r: u32, c: u32| {
            let Some(left) = merge_work.get().checked_sub(merges.len()) else {
                merge_cut.set(true);
                return None;
            };
            merge_work.set(left);
            merges.iter().find(|(r1, c1, r2, c2)| (*r1..=*r2).contains(&r) && (*c1..=*c2).contains(&c)).copied()
        };
        let pi = canvas.new_page(pw, ph)?;
        let Some(page) = canvas.page(pi) else { continue };
        // Scale about the printable area's top-left corner.
        page.save();
        if (k - 1.0).abs() > 1e-6 {
            page.raw(&format!(
                "{} 0 0 {} {} {} cm\n",
                super::canvas::n(k),
                super::canvas::n(k),
                super::canvas::n(ml * (1.0 - k)),
                super::canvas::n((ph - mt) * (1.0 - k))
            ));
        }
        let x_of = |c: u32| ml + at(&col_at, c) - at(&col_at, *c0);
        let y_of = |r: u32| ph - mt - (at(&row_at, r) - at(&row_at, *r0));
        if sh.gridlines {
            draw_gridlines(page, (&x_of, &y_of), (*c0, *c1), (*r0, *r1));
        }
        // Fills first, then text, then borders on top. The page's cells, row by occupied row and
        // only within its columns, already in (row, col) order: a wide sheet's row band holds
        // every column band's cells, and a band of hidden rows can be a million rows tall.
        let next_row = |from: u32| (from <= *r1).then(|| sh.cells.range((from, 0)..=(*r1, u32::MAX)).next()).flatten().map(|((r, _), _)| *r);
        let mut cells_here: Vec<(&(u32, u32), &Cell)> = Vec::new();
        let mut row = next_row(*r0);
        while let Some(r) = row {
            cells_here.extend(sh.cells.range((r, *c0)..=(r, *c1)));
            row = r.checked_add(1).and_then(next_row);
        }
        let rect = |r: u32, c: u32| -> (f64, f64, f64, f64) {
            let (r2, c2) = match merged_at(r, c) {
                Some((mr1, mc1, mr2, mc2)) if mr1 == r && mc1 == c => (mr2.min(*r1), mc2.min(*c1)),
                _ => (r, c),
            };
            let x = x_of(c);
            let w = x_of(c2 + 1) - x;
            let top = y_of(r);
            let h = top - y_of(r2 + 1);
            (x, top - h, w, h)
        };
        let covered = |r: u32, c: u32| merged_at(r, c).is_some_and(|(mr1, mc1, _, _)| (mr1, mc1) != (r, c));
        for ((r, c), cell) in &cells_here {
            if covered(*r, *c) {
                continue;
            }
            let Some(xf) = styles.xfs.get(cell.xf) else { continue };
            if let Some(Some(f)) = styles.fills.get(xf.fill) {
                let (x, y, w, h) = rect(*r, *c);
                page.fill_rect(x, y, w, h, *f);
            }
        }
        for ((r, c), cell) in &cells_here {
            if covered(*r, *c) || cell.text.is_empty() {
                continue;
            }
            let xf = styles.xfs.get(cell.xf).cloned().unwrap_or_default();
            let style = style_of(cell);
            let (x, y, w, h) = rect(*r, *c);
            if w <= 0.0 || h <= 0.0 {
                continue;
            }
            let text = texts.get(&(*r, *c)).cloned().unwrap_or_default();
            let lines = wrapped.get(&(*r, *c)).cloned().unwrap_or_else(|| vec![text.replace('\n', " ")]);
            let line_h = style.size * 1.2;
            let block = lines.len() as f64 * line_h;
            let base_top = match xf.v_align.as_deref() {
                Some("top") => y + h - 1.0,
                Some("center") => y + h / 2.0 + block / 2.0,
                _ => y + block + 1.0,
            };
            // "general" is the default, written out by LibreOffice and others.
            let align = xf.h_align.clone().filter(|a| a != "general" && !a.is_empty()).unwrap_or_else(|| {
                if cell.numeric {
                    "right".into()
                } else if cell.boolean {
                    "center".into()
                } else {
                    "left".into()
                }
            });
            // Text may run into empty neighbours unless it wraps or is a number, as in Excel.
            let mut clip_w = w;
            if !xf.wrap && !cell.numeric && align == "left" {
                let tw = lines.first().map_or(0.0, |l| style.width(l));
                clip_w = run_on(sh, (*r, *c, *c1), (w, tw + 4.0), &width, &covered, (&run_on_work, &run_on_cut));
            }
            page.save();
            page.clip_rect(x, y, clip_w, h);
            for (i, l) in lines.iter().enumerate() {
                let tw = style.width(l);
                let indent = xf.indent * 9.0;
                let lx = match align.as_str() {
                    "right" => x + w - 2.0 - tw - indent,
                    "center" | "centerContinuous" => x + (w - tw) / 2.0,
                    _ => x + 2.0 + indent,
                };
                let ly = base_top - (i as f64 + 1.0) * line_h + style.size * 0.25;
                if numeric_overflow(cell, tw, w) {
                    // Excel shows ### when a number doesn't fit its column.
                    let hashes = "#".repeat(((w - 4.0) / style.width("#")).floor().clamp(1.0, 255.0) as usize);
                    page.text(x + 2.0, ly, &style, &hashes);
                } else {
                    page.text(lx, ly, &style, l);
                }
            }
            page.restore();
        }
        for ((r, c), cell) in &cells_here {
            if covered(*r, *c) {
                continue;
            }
            let Some(xf) = styles.xfs.get(cell.xf) else { continue };
            let Some(b) = styles.borders.get(xf.border) else { continue };
            draw_borders(page, b, rect(*r, *c));
        }
        page.restore();
    }
    if run_on_cut.get() {
        notes.add("text running on into thousands of empty or hidden columns, clipped to its cell");
    }
    if merge_cut.get() {
        notes.add("merged ranges on very dense pages, drawn unmerged");
    }
    Ok(())
}

/// The print scale that fits `total` points into `fit` pages of `area` points across and down.
fn fit_scale((fw, fh): (u32, u32), (total_w, total_h): (f64, f64), (area_w, area_h): (f64, f64)) -> f64 {
    let kw = if fw > 0 && total_w > 0.0 { area_w * f64::from(fw) / total_w } else { f64::INFINITY };
    let kh = if fh > 0 && total_h > 0.0 { area_h * f64::from(fh) / total_h } else { f64::INFINITY };
    kw.min(kh).clamp(0.1, 1.0)
}

/// Draw the gridlines of a page's columns `c0..=c1` and rows `r0..=r1`, placed by `x_of`/`y_of`.
fn draw_gridlines(page: &mut Page, (x_of, y_of): (&dyn Fn(u32) -> f64, &dyn Fn(u32) -> f64), (c0, c1): (u32, u32), (r0, r1): (u32, u32)) {
    let (xa, xb) = (x_of(c0), x_of(c1 + 1));
    let (ya, yb) = (y_of(r0), y_of(r1 + 1));
    // A hidden column or row puts its gridline where the next one goes: draw it once.
    let mut last = f64::NAN;
    for c in c0..=c1 + 1 {
        let x = x_of(c);
        if (x - last).abs() > 0.01 || c == c0 {
            page.line(x, ya, x, yb, 0.25, [0.75; 3]);
            last = x;
        }
    }
    last = f64::NAN;
    for r in r0..=r1 + 1 {
        let y = y_of(r);
        if (y - last).abs() > 0.01 || r == r0 {
            page.line(xa, y, xb, y, 0.25, [0.75; 3]);
            last = y;
        }
    }
}

/// Stroke a cell's borders around its rectangle `(x, y, w, h)`, `y` at the bottom.
fn draw_borders(page: &mut Page, b: &Edges, (x, y, w, h): (f64, f64, f64, f64)) {
    if let Some(e) = b.top {
        page.line(x, y + h, x + w, y + h, e.width, e.color);
    }
    if let Some(e) = b.bottom {
        page.line(x, y, x + w, y, e.width, e.color);
    }
    if let Some(e) = b.left {
        page.line(x, y, x, y + h, e.width, e.color);
    }
    if let Some(e) = b.right {
        page.line(x + w, y, x + w, y + h, e.width, e.color);
    }
}

fn numeric_overflow(cell: &Cell, text_w: f64, w: f64) -> bool {
    cell.numeric && text_w > w - 2.0 && w > 6.0
}

pub(crate) fn convert(zip: &Zip<'_>, main: &str, doc: &mut Document, notes: &mut Notes) -> Result<usize, String> {
    let rels = Rels::load(zip, main)?;
    let text = zip.text(main)?.ok_or_else(|| format!("{main} is missing"))?;
    let wb = xml::parse(&text).map_err(|e| format!("{main}: {e}"))?;
    let root = wb.root_element();
    let date1904 = child(root, "workbookPr").is_some_and(|p| flag(attr(p, "date1904")) && attr(p, "date1904").is_some());
    let theme = theme_colors(zip, &rels);
    let styles = load_styles(zip, &rels, &theme, notes)?;
    let shared = shared_strings(zip, &rels)?;
    let mut canvas = Canvas::default();
    for s in child(root, "sheets").into_iter().flat_map(|s| children(s, "sheet")) {
        if attr(s, "state").is_some_and(|v| v != "visible") {
            notes.add("hidden sheets, left out");
            continue;
        }
        let Some(id) = rel_attr(s, "id") else { continue };
        let Some(part) = rels.target(id) else { continue };
        // Chart sheets, dialog sheets and macro sheets (xlMacrosheet) are relationships of their
        // own types; only worksheets are printed.
        if rels.type_of(id) != Some("worksheet") {
            notes.add("chart sheets and macro sheets, left out");
            continue;
        }
        let sheet = read_sheet(zip, part, &shared, &styles, date1904, notes)?;
        draw_sheet(&mut canvas, &sheet, &styles, notes)?;
    }
    canvas.finish(doc).map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn number_formats() {
        let f = |v: f64, code: &str| format_number(v, code, false).0;
        assert_eq!(f(1234.5, "General"), "1234.5");
        assert_eq!(f(1234.5, "#,##0.00"), "1,234.50");
        assert_eq!(f(-1234.5, "#,##0.00"), "-1,234.50");
        assert_eq!(f(0.256, "0.0%"), "25.6%");
        assert_eq!(f(1234.5, "[$R$-416] #,##0.00"), "R$ 1.234,50");
        assert_eq!(f(-5.0, "#,##0;(#,##0)"), "(5)");
        assert_eq!(f(0.0, "#,##0;(#,##0);\"-\""), "-");
        assert_eq!(f(12345.678, "0.00E+00"), "1.23E+04");
        assert_eq!(f(1500000.0, "#,##0,,\"M\""), "2M");
        assert_eq!(f(45000.0, "yyyy-mm-dd"), "2023-03-15");
        assert_eq!(f(45000.75, "dd/mm/yyyy hh:mm"), "15/03/2023 18:00");
        assert_eq!(f(45000.0, "[$-416]d \"de\" mmmm \"de\" yyyy"), "15 de março de 2023");
        assert_eq!(f(45000.0, "mmm d, yyyy"), "Mar 15, 2023");
        assert_eq!(f(0.5, "h:mm AM/PM"), "12:00 PM");
        assert_eq!(f(1.0 / 3.0, "General"), "0.3333333333");
        assert_eq!(f(2.5, "0"), "3", "halves round away from zero, as in Excel");
        assert_eq!(f(7.0, "000"), "007");
        assert_eq!(f(-1234.0, "000000"), "-001234");
        assert_eq!(f(45000.5, "\"Hoje é\" h AM/PM"), "Hoje é 12 PM");
        assert_eq!(f(1e20, "General"), "1E+20");
        assert_eq!(f(f64::MAX, "yyyy"), general(f64::MAX));
    }

    #[test]
    fn cell_references() {
        assert_eq!(col_index("A1"), Some((1, 1)));
        assert_eq!(col_index("AB12"), Some((12, 28)));
        assert_eq!(col_index("XFD1048576"), Some((1_048_576, 16_384)));
        assert_eq!(col_index("ZZZZ1"), None);
        assert_eq!(col_index("A0"), None);
        assert_eq!(col_index("1A"), None);
    }
}
