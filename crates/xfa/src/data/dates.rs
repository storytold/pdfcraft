use super::path::template_field;
use super::{Field, Template};

/// The date letters (`yyyy-mm-dd`, `yyyymmdd`) of an XFA date picture clause such as
/// `date{YYYY-MM-DD}`, or `None` when it has none this reads (empty, or with symbols beyond
/// years, months, days and separators).
pub(crate) fn date_letters(picture: &str) -> Option<String> {
    let inner = picture
        .find('{')
        .and_then(|a| {
            let rest = picture.get(a + 1..)?;
            rest.find('}').and_then(|b| rest.get(..b))
        })
        .unwrap_or(picture)
        .trim();
    if inner.is_empty() || inner.chars().any(|c| !"YMDEJ-/., :".contains(c)) {
        return None;
    }
    Some(inner.replace('E', "d").to_ascii_lowercase())
}

/// The date letters of `f`'s data picture, when it holds a whole date. A data picture without a
/// year, a month or a day can't hold the date the field shows, so such a field's data keeps
/// ISO dates.
pub(crate) fn data_date_letters(f: &Field) -> Option<String> {
    let letters = f.data_picture.as_deref().and_then(date_letters)?;
    let tokens = date_tokens(&letters);
    let has = |any: &[&str]| tokens.iter().any(|t| any.contains(&t.as_str()));
    (has(&["yyyy", "yy"]) && has(&["mm", "m"]) && has(&["dd", "d"])).then_some(letters)
}

/// A date as the data holds it, written in `data_pattern` (the field's data picture), read as
/// an ISO date. A value that doesn't match, or a field without a data picture, is taken as it
/// is: the data holds ISO dates by default.
pub(crate) fn data_to_iso(value: &str, data_pattern: Option<&str>) -> String {
    data_pattern.and_then(|p| pattern_to_iso(value, p)).unwrap_or_else(|| value.to_string())
}

/// An ISO date as the data should hold it: in `data_pattern` when the field has one and the
/// date reads back from it unchanged (a two-digit year holds only 1950 to 2049), else as ISO.
pub(crate) fn iso_to_data(iso: String, data_pattern: Option<&str>) -> String {
    match data_pattern {
        Some(p) => {
            let written = iso_to_pattern(&iso, p);
            if pattern_to_iso(&written, p).as_deref() == Some(iso.as_str()) { written } else { iso }
        }
        None => iso,
    }
}

/// The date letters of the data picture of the field `som` names, when it has one.
pub(super) fn data_date_pattern(tpl: Option<&Template>, som: &str) -> Option<String> {
    tpl.and_then(|t| template_field(t, som)).and_then(data_date_letters)
}

const MONTHS: [&str; 12] = ["January", "February", "March", "April", "May", "June", "July", "August", "September", "October", "November", "December"];

const WEEKDAYS: [&str; 7] = ["Sunday", "Monday", "Tuesday", "Wednesday", "Thursday", "Friday", "Saturday"];

/// The weekday (0 = Sunday) of a proleptic Gregorian date.
fn weekday(y: u32, m: u32, d: u32) -> usize {
    const T: [i64; 12] = [0, 3, 2, 5, 0, 3, 5, 1, 4, 6, 2, 4];
    let y = i64::from(y) - i64::from(m < 3);
    let t = T.get((m as usize).wrapping_sub(1)).copied().unwrap_or(0);
    ((y + y / 4 - y / 100 + y / 400 + t + i64::from(d)).rem_euclid(7)) as usize
}

/// The index and length of the name in `names` that `text` starts with (ignoring case),
/// cut to `abbr` letters when that is set.
fn name_at(text: &str, names: &[&str], abbr: bool) -> Option<(usize, usize)> {
    names.iter().enumerate().find_map(|(i, n)| {
        let n = if abbr { n.get(..3)? } else { n };
        text.get(..n.len()).filter(|t| t.eq_ignore_ascii_case(n)).map(|_| (i, n.len()))
    })
}

/// Whether `token` is a run of the date letters (`y`, `m`, `d`) rather than a literal.
fn is_date_run(token: &str) -> bool {
    token.starts_with(['y', 'm', 'd'])
}

/// The tokens of a date pattern: runs of `y`, `m` and `d` (`yyyy`, `yy`, `mmmm`, `mmm`, `mm`,
/// `m`, `dddd`, `ddd`, `dd`, `d` are understood), or a literal character.
pub(crate) fn date_tokens(pattern: &str) -> Vec<String> {
    let mut out = Vec::new();
    let chars: Vec<char> = pattern.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i].to_ascii_lowercase();
        if "ymd".contains(c) {
            let mut j = i;
            while j < chars.len() && chars[j].to_ascii_lowercase() == c {
                j += 1;
            }
            out.push(std::iter::repeat_n(c, j - i).collect());
            i = j;
        } else {
            out.push(chars[i].to_string());
            i += 1;
        }
    }
    out
}

/// `2001-02-03` shown in `pattern` (`mm/dd/yyyy` → `02/03/2001`, `mmm d, yyyy` →
/// `Feb 3, 2001`). Patterns with a token this doesn't know and non-dates come back as they are.
pub fn iso_to_pattern(iso: &str, pattern: &str) -> String {
    let parts: Vec<&str> = iso.trim().splitn(3, '-').collect();
    let [y, m, d] = parts[..] else { return iso.to_string() };
    let (Ok(y), Ok(m), Ok(d)) = (y.parse::<u32>(), m.parse::<u32>(), d.parse::<u32>()) else { return iso.to_string() };
    let mut out = String::new();
    for t in date_tokens(pattern) {
        match t.as_str() {
            "yyyy" => out.push_str(&format!("{y:04}")),
            "yy" => out.push_str(&format!("{:02}", y % 100)),
            "mm" => out.push_str(&format!("{m:02}")),
            "m" => out.push_str(&m.to_string()),
            "dd" => out.push_str(&format!("{d:02}")),
            "d" => out.push_str(&d.to_string()),
            "mmm" | "mmmm" => {
                let Some(name) = m.checked_sub(1).and_then(|i| MONTHS.get(i as usize)) else { return iso.to_string() };
                match t.len() {
                    3 => out.push_str(name.get(..3).unwrap_or(name)),
                    _ => out.push_str(name),
                }
            }
            "ddd" | "dddd" => {
                let name = WEEKDAYS.get(weekday(y, m, d)).copied().unwrap_or_default();
                match t.len() {
                    3 => out.push_str(name.get(..3).unwrap_or(name)),
                    _ => out.push_str(name),
                }
            }
            other if is_date_run(other) => return iso.to_string(),
            other => out.push_str(other),
        }
    }
    out
}

/// A date typed as `pattern` back to `yyyy-mm-dd`. `None` when it doesn't match.
pub fn pattern_to_iso(text: &str, pattern: &str) -> Option<String> {
    let text = text.trim();
    let (mut y, mut m, mut d) = (None, None, None);
    let mut day_name = None;
    let mut rest = text;
    let tokens = date_tokens(pattern);
    for (i, t) in tokens.iter().enumerate() {
        match t.as_str() {
            "yyyy" | "yy" | "mm" | "m" | "dd" | "d" => {
                // Digits up to the next literal (or a fixed width).
                let fixed = matches!(t.as_str(), "yyyy" | "yy" | "mm" | "dd");
                let width =
                    if fixed { t.len() } else { rest.chars().take_while(char::is_ascii_digit).count().min(if t == "d" || t == "m" { 2 } else { 4 }) };
                let digits: String = rest.chars().take(width).collect();
                if digits.is_empty() || !digits.chars().all(|c| c.is_ascii_digit()) {
                    return None;
                }
                let v: u32 = digits.parse().ok()?;
                rest = &rest[digits.len()..];
                match t.as_str() {
                    "yyyy" => y = Some(v),
                    "yy" => y = Some(if v < 50 { 2000 + v } else { 1900 + v }),
                    "mm" | "m" => m = Some(v),
                    _ => d = Some(v),
                }
            }
            "mmm" | "mmmm" => {
                let (i, len) = name_at(rest, &MONTHS, t.len() == 3)?;
                m = Some(i as u32 + 1);
                rest = rest.get(len..)?;
            }
            "ddd" | "dddd" => {
                let (i, len) = name_at(rest, &WEEKDAYS, t.len() == 3)?;
                day_name = Some(i);
                rest = rest.get(len..)?;
            }
            run if is_date_run(run) => return None,
            lit => {
                rest = rest.strip_prefix(lit)?;
            }
        }
        let _ = i;
    }
    if !rest.is_empty() {
        return None;
    }
    let (y, m, d) = (y?, m?, d?);
    if day_name.is_some_and(|w| w != weekday(y, m, d)) {
        return None;
    }
    (1..=12).contains(&m).then(|| format!("{y:04}-{m:02}-{d:02}")).filter(|_| (1..=31).contains(&d))
}
